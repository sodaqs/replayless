use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::config::{Config, EncodeConfig};
use crate::manifest::Manifest;
use crate::probe::{self, MediaInfo};
use crate::scan::{self, human_size};
use crate::encode;

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

pub fn run(cfg: &Config, ov: &Overrides) -> Result<()> {
    let enc = effective_encode(&cfg.encode, ov);
    let worker_count = enc.jobs.max(1);

    let videos = scan::collect_videos(cfg)?;
    let manifest = Manifest::load(&cfg.manifest)?;

    // Build the work list, skipping anything already compressed.
    let mut jobs: Vec<Job> = Vec::new();
    let mut skipped = 0usize;
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

    println!(
        "{} videos found: {} to compress, {} already done.",
        videos.len(),
        jobs.len(),
        skipped
    );

    if ov.dry_run {
        for j in &jobs {
            let info = probe::probe(&j.src).unwrap_or(ZERO_INFO);
            let args = encode::build_args(&enc, &j.src, &j.dst, &info);
            println!("DRY  {} -> {}", j.label, j.dst.display());
            println!("     ffmpeg {}", args.join(" "));
        }
        return Ok(());
    }
    if jobs.is_empty() {
        println!("Nothing to do.");
        return Ok(());
    }

    // Shared state across workers (borrowed by scoped threads).
    let total = jobs.len() as u64;
    let queue = Mutex::new(VecDeque::from(jobs));
    let manifest = Mutex::new(manifest);
    let in_bytes = AtomicU64::new(0);
    let out_bytes = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let failed = AtomicU64::new(0);

    let mp = MultiProgress::new();
    let overall = mp.add(ProgressBar::new(total));
    overall.set_style(
        ProgressStyle::with_template("{bar:30.cyan/blue} {pos}/{len} files  {msg}").unwrap(),
    );

    let start = Instant::now();
    let manifest_path = cfg.manifest.as_path();
    thread::scope(|s| {
        for _ in 0..worker_count {
            let queue = &queue;
            let manifest = &manifest;
            let mp = &mp;
            let overall = &overall;
            let in_bytes = &in_bytes;
            let out_bytes = &out_bytes;
            let done = &done;
            let failed = &failed;
            let enc = &enc;
            s.spawn(move || {
                let spinner = mp.add(ProgressBar::new_spinner());
                spinner.set_style(ProgressStyle::with_template("  {spinner} {msg}").unwrap());
                loop {
                    let Some(job) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    spinner.set_message(format!("Encoding {}", job.label));
                    spinner.enable_steady_tick(Duration::from_millis(120));

                    match encode_one(enc, &job) {
                        Ok(out_sz) => {
                            in_bytes.fetch_add(job.src_bytes, Ordering::Relaxed);
                            out_bytes.fetch_add(out_sz, Ordering::Relaxed);
                            done.fetch_add(1, Ordering::Relaxed);
                            let mut m = manifest.lock().unwrap();
                            m.mark_compressed(&job.label, job.src_bytes, out_sz);
                            if let Err(e) = m.save(manifest_path) {
                                tracing::warn!("manifest save failed: {e:#}");
                            }
                        }
                        Err(e) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("encode failed for {}: {e:#}", job.label);
                            let _ = std::fs::remove_file(&job.dst); // drop partial output
                        }
                    }

                    spinner.disable_steady_tick();
                    let (i, o) = (in_bytes.load(Ordering::Relaxed), out_bytes.load(Ordering::Relaxed));
                    overall.set_message(format!(
                        "{} -> {} ({})",
                        human_size(i),
                        human_size(o),
                        ratio(i, o)
                    ));
                    overall.inc(1);
                }
                spinner.finish_and_clear();
            });
        }
    });
    overall.finish_and_clear();

    let (i, o) = (in_bytes.load(Ordering::Relaxed), out_bytes.load(Ordering::Relaxed));
    println!(
        "Done: {} compressed, {} failed in {:.0}s.",
        done.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        start.elapsed().as_secs_f64()
    );
    println!(
        "Size: {} -> {} ({}, saved {}).",
        human_size(i),
        human_size(o),
        ratio(i, o),
        human_size(i.saturating_sub(o))
    );
    Ok(())
}

const ZERO_INFO: MediaInfo = MediaInfo {
    duration_secs: 0.0,
    fps: 0.0,
    width: 0,
    height: 0,
};

/// Encode one job, returning the output file size on success.
fn encode_one(enc: &EncodeConfig, job: &Job) -> Result<u64> {
    if let Some(parent) = job.dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let info = probe::probe(&job.src)?;
    let args = encode::build_args(enc, &job.src, &job.dst, &info);
    encode::run(&args)?;
    let size = std::fs::metadata(&job.dst)
        .with_context(|| format!("stat output {}", job.dst.display()))?
        .len();
    Ok(size)
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

/// Stable, forward-slashed label relative to the source root.
fn rel_label(source_dir: &Path, src: &Path) -> String {
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
fn ratio(input: u64, output: u64) -> String {
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
        let dst = dest_path(src_dir, out_dir, Path::new(r"C:\Videos\NVIDIA\Far Cry 6\clip.mp4"));
        assert_eq!(dst, PathBuf::from(r"C:\Videos\NVIDIA_compact\Far Cry 6\clip.mp4"));
    }

    #[test]
    fn dest_forces_mp4_extension() {
        let src_dir = Path::new(r"C:\Videos\NVIDIA");
        let out_dir = Path::new(r"C:\out");
        let dst = dest_path(src_dir, out_dir, Path::new(r"C:\Videos\NVIDIA\Game\clip.mkv"));
        assert_eq!(dst, PathBuf::from(r"C:\out\Game\clip.mp4"));
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
