use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::config::{Config, EncodeConfig};
use crate::encode::{self, RunOutcome};
use crate::manifest::Manifest;
use crate::probe::{self, MediaInfo};
use crate::progress::{CancelToken, Event, ProgressSink, Stage};
use crate::scan;

/// CLI overrides for a compress run; `None` falls back to the config value.
#[derive(Debug, Default)]
pub struct Overrides {
    pub codec: Option<String>,
    pub cq: Option<u32>,
    pub maxrate: Option<String>,
    pub fps_cap: Option<u32>,
    pub scale: Option<String>,
    pub jobs: Option<usize>,
    pub dry_run: bool,
    pub limit: Option<usize>,
}

struct Job {
    src: PathBuf,
    dst: PathBuf,
    src_bytes: u64,
    /// Forward-slashed path relative to the source root; manifest key + label.
    label: String,
}

/// Outcome of encoding one job (distinct from a hard error).
enum EncodeResult {
    Done(u64),
    Cancelled,
}

/// Compress all pending videos, reporting progress through `sink` and honoring
/// `cancel`. Idempotent: already-compressed files (per manifest) and fresh
/// outputs are skipped.
pub fn run(
    cfg: &Config,
    ov: &Overrides,
    sink: &mut dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<()> {
    let enc = effective_encode(&cfg.encode, ov);
    let worker_count = enc.jobs.max(1);

    let videos = scan::collect_videos(cfg)?;
    let manifest = Manifest::load(&cfg.manifest)?;

    // Build the work list, skipping anything already compressed.
    let mut jobs: Vec<Job> = Vec::new();
    let mut skipped = 0u64;
    for v in &videos {
        let dst = dest_path(&cfg.source_dir, &cfg.output_dir, &v.path);
        let label = rel_label(&cfg.source_dir, &v.path);
        if manifest.is_compressed(&label) || is_fresh_output(&v.path, &dst) {
            skipped += 1;
            continue;
        }
        jobs.push(Job {
            src: v.path.clone(),
            dst,
            src_bytes: v.bytes,
            label,
        });
    }

    // Largest first: biggest savings land early, and it's the natural test order.
    jobs.sort_by_key(|j| std::cmp::Reverse(j.src_bytes));
    if let Some(limit) = ov.limit {
        jobs.truncate(limit);
    }

    sink.emit(Event::Log {
        message: format!(
            "{} videos found: {} to compress, {} already done.",
            videos.len(),
            jobs.len(),
            skipped
        ),
    });

    if ov.dry_run {
        for j in &jobs {
            let info = probe::probe(&j.src).unwrap_or(ZERO_INFO);
            let args = encode::build_args(&enc, &j.src, &j.dst, &info);
            sink.emit(Event::Log {
                message: format!("DRY  {} -> {}", j.label, j.dst.display()),
            });
            sink.emit(Event::Log {
                message: format!("     ffmpeg {}", args.join(" ")),
            });
        }
        return Ok(());
    }

    let total = jobs.len();
    let total_bytes: u64 = jobs.iter().map(|j| j.src_bytes).sum();
    sink.emit(Event::StageStarted {
        stage: Stage::Compress,
        files: total,
        total_bytes,
    });

    if jobs.is_empty() {
        sink.emit(stage_finished(skipped, 0, 0, 0, 0));
        return Ok(());
    }

    // Shared state across workers (borrowed by scoped threads).
    let queue: Mutex<VecDeque<(usize, Job)>> = Mutex::new(
        jobs.into_iter()
            .enumerate()
            .map(|(i, j)| (i + 1, j))
            .collect(),
    );
    let manifest = Mutex::new(manifest);
    let in_bytes = AtomicU64::new(0);
    let out_bytes = AtomicU64::new(0);
    let ok = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let manifest_path = cfg.manifest.as_path();
    let sink = Mutex::new(sink);

    thread::scope(|s| {
        for _ in 0..worker_count {
            let queue = &queue;
            let manifest = &manifest;
            let sink = &sink;
            let in_bytes = &in_bytes;
            let out_bytes = &out_bytes;
            let ok = &ok;
            let failed = &failed;
            let enc = &enc;
            s.spawn(move || {
                loop {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let Some((index, job)) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    sink.lock().unwrap().emit(Event::FileStarted {
                        stage: Stage::Compress,
                        key: job.label.clone(),
                        index,
                        total,
                        bytes: job.src_bytes,
                    });

                    match encode_one(enc, &job, cancel, sink) {
                        Ok(EncodeResult::Done(out_sz)) => {
                            in_bytes.fetch_add(job.src_bytes, Ordering::Relaxed);
                            out_bytes.fetch_add(out_sz, Ordering::Relaxed);
                            ok.fetch_add(1, Ordering::Relaxed);
                            {
                                let mut m = manifest.lock().unwrap();
                                m.mark_compressed(&job.label, job.src_bytes, out_sz);
                                if let Err(e) = m.save(manifest_path) {
                                    sink.lock().unwrap().emit(Event::Log {
                                        message: format!("manifest save failed: {e:#}"),
                                    });
                                }
                            }
                            sink.lock().unwrap().emit(Event::FileFinished {
                                stage: Stage::Compress,
                                key: job.label.clone(),
                                out_bytes: Some(out_sz),
                            });
                        }
                        Ok(EncodeResult::Cancelled) => break,
                        Err(e) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            sink.lock().unwrap().emit(Event::FileFailed {
                                stage: Stage::Compress,
                                key: job.label.clone(),
                                error: format!("{e:#}"),
                            });
                        }
                    }
                }
            });
        }
    });

    let sink = sink.into_inner().unwrap();
    sink.emit(stage_finished(
        skipped,
        ok.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        in_bytes.load(Ordering::Relaxed),
        out_bytes.load(Ordering::Relaxed),
    ));
    Ok(())
}

/// Build a `StageFinished` event for the compress stage.
fn stage_finished(skipped: u64, ok: u64, failed: u64, in_bytes: u64, out_bytes: u64) -> Event {
    Event::StageFinished {
        stage: Stage::Compress,
        ok,
        skipped,
        failed,
        in_bytes,
        out_bytes,
    }
}

const ZERO_INFO: MediaInfo = MediaInfo {
    duration_secs: 0.0,
    fps: 0.0,
    width: 0,
    height: 0,
};

/// Encode one job to a temp file, then rename it into place on success. Emits
/// per-file progress through `sink`. Returns the output size, or `Cancelled` if
/// the token tripped mid-encode (in which case the temp file is removed).
fn encode_one(
    enc: &EncodeConfig,
    job: &Job,
    cancel: &CancelToken,
    sink: &Mutex<&mut dyn ProgressSink>,
) -> Result<EncodeResult> {
    if let Some(parent) = job.dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let info = probe::probe(&job.src)?;
    let dur = info.duration_secs.max(0.001);
    let tmp = temp_path(&job.dst);
    let args = encode::build_args(enc, &job.src, &tmp, &info);

    let key = job.label.clone();
    let outcome = match encode::run_with_progress(&args, cancel, |p| {
        let fraction = (p.out_secs / dur).clamp(0.0, 1.0) as f32;
        let remaining = (dur - p.out_secs).max(0.0);
        let eta_secs = if p.speed > 0.01 {
            Some((remaining / p.speed as f64) as u64)
        } else {
            None
        };
        sink.lock().unwrap().emit(Event::FileProgress {
            key: key.clone(),
            fraction,
            speed: Some(p.speed),
            eta_secs,
        });
    }) {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp); // drop partial temp on failure
            return Err(e);
        }
    };

    match outcome {
        RunOutcome::Completed => {
            std::fs::rename(&tmp, &job.dst)
                .with_context(|| format!("finalizing {}", job.dst.display()))?;
            let size = std::fs::metadata(&job.dst)
                .with_context(|| format!("stat output {}", job.dst.display()))?
                .len();
            Ok(EncodeResult::Done(size))
        }
        RunOutcome::Cancelled => {
            let _ = std::fs::remove_file(&tmp);
            Ok(EncodeResult::Cancelled)
        }
    }
}

/// Apply CLI overrides on top of the configured encode settings.
fn effective_encode(base: &EncodeConfig, ov: &Overrides) -> EncodeConfig {
    let mut e = base.clone();
    if let Some(c) = &ov.codec {
        e.codec = c.clone();
    }
    if let Some(c) = ov.cq {
        e.cq = c;
    }
    if let Some(m) = &ov.maxrate {
        e.maxrate = m.clone();
    }
    if let Some(f) = ov.fps_cap {
        e.fps_cap = f;
    }
    if let Some(scale) = &ov.scale {
        e.scale = Some(scale.clone());
    }
    if let Some(j) = ov.jobs {
        e.jobs = j;
    }
    e
}

/// Map a source path to its output path, mirroring the game folder under
/// `output_dir` and forcing a `.mp4` extension.
fn dest_path(source_dir: &Path, output_dir: &Path, src: &Path) -> PathBuf {
    let rel = src.strip_prefix(source_dir).unwrap_or(src);
    output_dir.join(rel).with_extension("mp4")
}

/// A sibling temp path (`<stem>.part.mp4`) so a crashed/cancelled encode never
/// leaves a "fresh" final output that resume would mistake for done. Keeps the
/// `.mp4` extension so ffmpeg still infers the muxer.
fn temp_path(dst: &Path) -> PathBuf {
    let stem = dst.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let name = format!("{stem}.part.mp4");
    match dst.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Stable, forward-slashed label relative to the source root. This is the
/// manifest key for a source file; pre-flight estimation reuses it to match
/// scanned videos against already-compressed manifest entries.
pub fn rel_label(source_dir: &Path, src: &Path) -> String {
    src.strip_prefix(source_dir)
        .unwrap_or(src)
        .to_string_lossy()
        .replace('\\', "/")
}

/// True if a usable output already exists (present and at least as new as src).
fn is_fresh_output(src: &Path, dst: &Path) -> bool {
    let (Ok(s), Ok(d)) = (mtime(src), mtime(dst)) else {
        return false;
    };
    output_is_fresh(s, d)
}

fn mtime(path: &Path) -> std::io::Result<SystemTime> {
    std::fs::metadata(path)?.modified()
}

/// Pure freshness rule: output is fresh when it's not older than the source.
fn output_is_fresh(src_mtime: SystemTime, dst_mtime: SystemTime) -> bool {
    dst_mtime >= src_mtime
}

/// Format a compaction ratio like `8.4x`, guarding divide-by-zero.
pub fn ratio(input: u64, output: u64) -> String {
    if output == 0 {
        "—".to_string()
    } else {
        format!("{:.1}x", input as f64 / output as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dest_mirrors_game_folder_as_mp4() {
        let src_dir = Path::new(r"C:\Videos\NVIDIA");
        let out_dir = Path::new(r"C:\Videos\NVIDIA_compact");
        let dst = dest_path(
            src_dir,
            out_dir,
            Path::new(r"C:\Videos\NVIDIA\Far Cry 6\clip.mp4"),
        );
        assert_eq!(
            dst,
            PathBuf::from(r"C:\Videos\NVIDIA_compact\Far Cry 6\clip.mp4")
        );
    }

    #[test]
    fn dest_forces_mp4_extension() {
        let src_dir = Path::new(r"C:\Videos\NVIDIA");
        let out_dir = Path::new(r"C:\out");
        let dst = dest_path(
            src_dir,
            out_dir,
            Path::new(r"C:\Videos\NVIDIA\Game\clip.mkv"),
        );
        assert_eq!(dst, PathBuf::from(r"C:\out\Game\clip.mp4"));
    }

    #[test]
    fn temp_path_keeps_mp4_extension_as_sibling() {
        let dst = Path::new(r"C:\out\Far Cry 6\clip.mp4");
        assert_eq!(
            temp_path(dst),
            PathBuf::from(r"C:\out\Far Cry 6\clip.part.mp4")
        );
    }

    #[test]
    fn rel_label_is_forward_slashed() {
        let src_dir = Path::new(r"C:\Videos\NVIDIA");
        let label = rel_label(src_dir, Path::new(r"C:\Videos\NVIDIA\Far Cry 6\clip.mp4"));
        assert_eq!(label, "Far Cry 6/clip.mp4");
    }

    #[test]
    fn overrides_take_precedence_over_config() {
        let base = EncodeConfig {
            codec: "hevc".into(),
            cq: 30,
            maxrate: "12M".into(),
            fps_cap: 30,
            audio: "copy".into(),
            jobs: 2,
            scale: None,
        };
        let ov = Overrides {
            cq: Some(34),
            jobs: Some(1),
            codec: Some("av1".into()),
            ..Default::default()
        };
        let e = effective_encode(&base, &ov);
        assert_eq!(e.cq, 34);
        assert_eq!(e.jobs, 1);
        assert_eq!(e.codec, "av1");
        assert_eq!(e.maxrate, "12M"); // untouched
    }

    #[test]
    fn output_freshness_compares_mtimes() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(10);
        assert!(output_is_fresh(t0, t1)); // output newer -> fresh
        assert!(output_is_fresh(t0, t0)); // equal -> fresh
        assert!(!output_is_fresh(t1, t0)); // output older -> stale
    }

    #[test]
    fn ratio_guards_zero() {
        assert_eq!(ratio(1000, 125), "8.0x");
        assert_eq!(ratio(1000, 0), "—");
    }
}
