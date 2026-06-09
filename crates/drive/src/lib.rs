//! Google Drive integration for **video-uploader**: OAuth via `.env` (auth) and
//! resumable chunked uploads.
//!
//! This crate sits on top of [`vu_core`] — it reuses core's [`Config`],
//! [`Manifest`], and [`progress`](vu_core::progress) types but adds the heavier
//! HTTP/OAuth dependency surface (`reqwest`, `tiny_http`, `sha2`, …) that the
//! rest of core doesn't need. Front-ends depend on both crates.

pub mod auth;
pub mod upload;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use vu_core::config::Config;
use vu_core::manifest::{Manifest, Status};
use vu_core::progress::{CancelToken, Event, ProgressSink, Stage};
use vu_core::scan::human_size;

const FILES_API: &str = "https://www.googleapis.com/drive/v3/files";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const DEFAULT_ROOT: &str = "NVIDIA Replays";

/// Options for an upload run.
#[derive(Debug, Default)]
pub struct Options {
    pub dry_run: bool,
    pub limit: Option<usize>,
}

/// Upload all compressed-but-not-uploaded files to Drive, reporting progress
/// through `sink` and honoring `cancel` (checked between files).
pub fn run(
    cfg: &Config,
    opts: &Options,
    sink: &mut dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<()> {
    let root_name = std::env::var("DRIVE_ROOT_FOLDER").unwrap_or_else(|_| DEFAULT_ROOT.to_string());
    let manifest_path = cfg.manifest.as_path();
    let mut manifest = Manifest::load(manifest_path)?;

    // Collect compressed entries whose local output still exists.
    let mut jobs: Vec<(String, PathBuf)> = Vec::new();
    for (key, entry) in &manifest.entries {
        if entry.status != Status::Compressed {
            continue;
        }
        let local = output_path(&cfg.output_dir, key);
        if local.exists() {
            jobs.push((key.clone(), local));
        } else {
            sink.emit(Event::Log {
                message: format!(
                    "skipping {key}: compressed file not found at {}",
                    local.display()
                ),
            });
        }
    }
    jobs.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(limit) = opts.limit {
        jobs.truncate(limit);
    }

    sink.emit(Event::Log {
        message: format!(
            "{} file(s) ready to upload into Drive folder '{}'.",
            jobs.len(),
            root_name
        ),
    });

    if opts.dry_run {
        for (key, local) in &jobs {
            let size = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
            sink.emit(Event::Log {
                message: format!("DRY  {key}  ({})", human_size(size)),
            });
        }
        return Ok(());
    }

    let total = jobs.len();
    let total_bytes: u64 = jobs
        .iter()
        .map(|(_, p)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    sink.emit(Event::StageStarted {
        stage: Stage::Upload,
        files: total,
        total_bytes,
    });

    if jobs.is_empty() {
        sink.emit(upload_finished(0, 0, 0, 0));
        return Ok(());
    }

    let token = auth::access_token().context("getting a Drive access token (run `auth` first?)")?;
    let mut drive = Drive::new(token);
    let root_id = drive
        .ensure_folder(&root_name, None)
        .context("ensuring Drive root folder")?;

    let (mut uploaded, mut skipped, mut failed, mut sent_bytes) = (0u64, 0u64, 0u64, 0u64);
    for (index, (key, local)) in jobs.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        let bytes = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        let game = game_of(key);
        let file_name = drive_file_name(key);
        let folder_id = drive
            .ensure_folder(&game, Some(&root_id))
            .with_context(|| format!("ensuring Drive folder '{game}'"))?;

        sink.emit(Event::FileStarted {
            stage: Stage::Upload,
            key: key.clone(),
            index: index + 1,
            total,
            bytes,
        });

        // Dedup: if it's already in the target folder, mark uploaded and skip.
        if let Some(existing) = drive.find_file(&file_name, &folder_id)? {
            manifest.mark_uploaded(key, &existing);
            manifest.save(manifest_path)?;
            skipped += 1;
            sink.emit(Event::FileSkipped {
                stage: Stage::Upload,
                key: key.clone(),
                reason: "already on Drive".to_string(),
            });
            continue;
        }

        let progress_key = key.clone();
        let result = upload::resumable_upload(
            &drive.client,
            &drive.token,
            local,
            &file_name,
            &folder_id,
            |sent, tot| {
                let fraction = if tot > 0 {
                    (sent as f64 / tot as f64) as f32
                } else {
                    0.0
                };
                sink.emit(Event::FileProgress {
                    key: progress_key.clone(),
                    fraction,
                    speed: None,
                    eta_secs: None,
                });
            },
        );

        match result {
            Ok(id) => {
                manifest.mark_uploaded(key, &id);
                manifest.save(manifest_path)?;
                uploaded += 1;
                sent_bytes += bytes;
                sink.emit(Event::FileFinished {
                    stage: Stage::Upload,
                    key: key.clone(),
                    out_bytes: None,
                    drive_id: Some(id),
                });
            }
            Err(e) => {
                failed += 1;
                sink.emit(Event::FileFailed {
                    stage: Stage::Upload,
                    key: key.clone(),
                    error: format!("{e:#}"),
                });
            }
        }
    }

    sink.emit(upload_finished(uploaded, skipped, failed, sent_bytes));
    Ok(())
}

/// Build a `StageFinished` event for the upload stage (`in_bytes` carries the
/// total bytes sent).
fn upload_finished(uploaded: u64, skipped: u64, failed: u64, sent_bytes: u64) -> Event {
    Event::StageFinished {
        stage: Stage::Upload,
        ok: uploaded,
        skipped,
        failed,
        in_bytes: sent_bytes,
        out_bytes: 0,
    }
}

/// Minimal Drive REST client with a folder-id cache.
struct Drive {
    client: reqwest::blocking::Client,
    token: String,
    folders: HashMap<String, String>,
}

#[derive(Deserialize)]
struct FileList {
    files: Vec<FileMeta>,
}

#[derive(Deserialize)]
struct FileMeta {
    id: String,
}

#[derive(Deserialize)]
struct IdResp {
    id: String,
}

impl Drive {
    fn new(token: String) -> Self {
        Drive {
            client: reqwest::blocking::Client::new(),
            token,
            folders: HashMap::new(),
        }
    }

    /// Ensure a folder named `name` exists under `parent` (None = My Drive
    /// root), creating it if needed. Results are cached for the run.
    fn ensure_folder(&mut self, name: &str, parent: Option<&str>) -> Result<String> {
        let cache_key = format!("{}\u{0}{name}", parent.unwrap_or("root"));
        if let Some(id) = self.folders.get(&cache_key) {
            return Ok(id.clone());
        }

        let mut q = format!(
            "name = '{}' and mimeType = '{FOLDER_MIME}' and trashed = false",
            escape(name)
        );
        if let Some(p) = parent {
            q.push_str(&format!(" and '{}' in parents", escape(p)));
        }

        let id = match self.list(&q)?.into_iter().next() {
            Some(found) => found.id,
            None => self.create_folder(name, parent)?,
        };
        self.folders.insert(cache_key, id.clone());
        Ok(id)
    }

    fn create_folder(&self, name: &str, parent: Option<&str>) -> Result<String> {
        let mut body = serde_json::json!({ "name": name, "mimeType": FOLDER_MIME });
        if let Some(p) = parent {
            body["parents"] = serde_json::json!([p]);
        }
        let resp = self
            .client
            .post(FILES_API)
            .bearer_auth(&self.token)
            .query(&[("fields", "id")])
            .json(&body)
            .send()
            .context("create-folder request")?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("create folder '{name}' failed ({status}): {text}");
        }
        Ok(serde_json::from_str::<IdResp>(&text)
            .context("parsing create-folder response")?
            .id)
    }

    /// Find a non-folder file named `name` directly in `parent`.
    fn find_file(&self, name: &str, parent: &str) -> Result<Option<String>> {
        let q = format!(
            "name = '{}' and '{}' in parents and trashed = false and mimeType != '{FOLDER_MIME}'",
            escape(name),
            escape(parent)
        );
        Ok(self.list(&q)?.into_iter().next().map(|f| f.id))
    }

    fn list(&self, q: &str) -> Result<Vec<FileMeta>> {
        let resp = self
            .client
            .get(FILES_API)
            .bearer_auth(&self.token)
            .query(&[
                ("q", q),
                ("fields", "files(id,name)"),
                ("spaces", "drive"),
                ("pageSize", "100"),
            ])
            .send()
            .context("Drive list request")?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("Drive list failed ({status}): {text}");
        }
        Ok(serde_json::from_str::<FileList>(&text)
            .context("parsing Drive list response")?
            .files)
    }
}

/// Local output path for a manifest key (mirrors compress's mapping).
fn output_path(output_dir: &Path, key: &str) -> PathBuf {
    let rel = key.replace('/', std::path::MAIN_SEPARATOR_STR);
    output_dir.join(rel).with_extension("mp4")
}

/// The game (top-level folder) component of a manifest key.
fn game_of(key: &str) -> String {
    key.split('/').next().unwrap_or("(root)").to_string()
}

/// The Drive file name for a key: the basename forced to `.mp4`.
fn drive_file_name(key: &str) -> String {
    Path::new(key)
        .with_extension("mp4")
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| key.to_string())
}

/// Escape a value for inclusion in a single-quoted Drive query string.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_path_mirrors_key() {
        let out = Path::new(r"C:\out");
        assert_eq!(
            output_path(out, "Far Cry 6/clip.mp4"),
            PathBuf::from(r"C:\out\Far Cry 6\clip.mp4")
        );
    }

    #[test]
    fn game_and_filename_split_the_key() {
        assert_eq!(game_of("Far Cry 6/clip.mp4"), "Far Cry 6");
        assert_eq!(drive_file_name("Far Cry 6/clip.mp4"), "clip.mp4");
        // non-mp4 source still uploads as .mp4
        assert_eq!(drive_file_name("Game/clip.mkv"), "clip.mp4");
    }

    #[test]
    fn escape_protects_quotes_and_backslashes() {
        assert_eq!(escape("Tom's Game"), "Tom\\'s Game");
        assert_eq!(escape(r"a\b"), r"a\\b");
        assert_eq!(escape("plain"), "plain");
    }
}
