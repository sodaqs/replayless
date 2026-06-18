//! Library-wide output-size estimation for the pre-flight summary.
//!
//! Combines exact sizes for files already compressed (read from the manifest)
//! with per-file estimates for the rest. Pending files are probed via ffprobe,
//! cached so repeated folder selections are cheap (see [`crate::probe_cache`]),
//! and the seeded model is nudged toward this library's realized compaction (see
//! [`crate::estimate::calibration_factor`]).

use std::path::Path;

use anyhow::Result;

use crate::config::{Config, EncodeConfig};
use crate::estimate::{self, ProbeFacts};
use crate::manifest::Manifest;
use crate::probe;
use crate::probe_cache::{self, ProbeCache};
use crate::{compress, paths, scan};

/// The pre-flight tally shown before a run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryEstimate {
    /// Source video count.
    pub files: usize,
    /// Total source bytes.
    pub bytes_now: u64,
    /// Estimated total output bytes (exact for done files, modeled for pending).
    pub bytes_est: u64,
    /// How many of `files` are already compressed (counted with exact sizes).
    pub done_files: usize,
}

/// Estimate the compressed size of the `source` library, given the `output`
/// directory (whose manifest/cache calibrate and accelerate the estimate) and
/// the encode settings. Probes pending files (caching results); the cache and
/// manifest are optional — missing ones just mean a less-refined estimate.
pub fn library_estimate(
    source: &Path,
    output: &Path,
    enc: &EncodeConfig,
) -> Result<LibraryEstimate> {
    let cfg = Config {
        source_dir: source.to_path_buf(),
        output_dir: output.to_path_buf(),
        ..Config::default()
    };
    let videos = scan::collect_videos(&cfg)?;
    let bytes_now: u64 = videos.iter().map(|v| v.bytes).sum();

    let manifest = Manifest::load(&paths::manifest_path(output)).unwrap_or_default();
    let calib = estimate::calibration_factor(&manifest, enc.cq).unwrap_or(1.0);

    let cache_path = paths::probe_cache_path(output);
    let mut cache = ProbeCache::load(&cache_path);

    let mut done_files = 0usize;
    let mut items: Vec<(Option<u64>, ProbeFacts)> = Vec::with_capacity(videos.len());
    for v in &videos {
        let label = compress::rel_label(source, &v.path);
        if manifest.is_compressed(&label)
            && let Some(out) = manifest.entries.get(&label).and_then(|e| e.output_bytes)
        {
            // Already compressed: use the real output size, no probe needed.
            done_files += 1;
            items.push((Some(out), zero_facts(v.bytes)));
            continue;
        }
        items.push((None, facts_for(&mut cache, &v.path, v.bytes)));
    }

    let bytes_est = estimate::aggregate(items, enc, calib);
    let _ = cache.save(&cache_path); // best-effort; the cache is only an optimization

    Ok(LibraryEstimate {
        files: videos.len(),
        bytes_now,
        bytes_est,
        done_files,
    })
}

/// Probe facts for a pending file, served from `cache` on a fingerprint hit and
/// probed (then cached) on a miss. A probe failure yields zero-duration facts, so
/// estimation falls back to treating the file as an unshrinkable pass-through
/// rather than dropping it from the total.
fn facts_for(cache: &mut ProbeCache, path: &Path, size: u64) -> ProbeFacts {
    let key = path.to_string_lossy().into_owned();
    let mtime = std::fs::metadata(path)
        .map(|m| probe_cache::mtime_ms(&m))
        .unwrap_or(0);

    if let Some(facts) = cache.get(&key, size, mtime) {
        return facts;
    }
    match probe::probe(path) {
        Ok(info) => {
            cache.insert(key, size, mtime, &info);
            ProbeFacts {
                duration_secs: info.duration_secs,
                fps: info.fps,
                src_bytes: size,
            }
        }
        Err(_) => zero_facts(size),
    }
}

/// Placeholder facts that carry only the source size (no duration/fps).
fn zero_facts(size: u64) -> ProbeFacts {
    ProbeFacts {
        duration_secs: 0.0,
        fps: 0.0,
        src_bytes: size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Entry, Status};
    use crate::probe::MediaInfo;

    fn enc() -> EncodeConfig {
        EncodeConfig {
            codec: "hevc".into(),
            cq: 30,
            maxrate: "12M".into(),
            fps_cap: 30,
            audio: "copy".into(),
            jobs: 1,
            scale: None,
        }
    }

    /// End-to-end estimate with NO ffprobe: the one pending file is pre-seeded in
    /// the cache (fresh fingerprint), so `facts_for` hits the cache and never
    /// spawns a subprocess. Exercises scan + manifest matching + cache + aggregate.
    #[test]
    fn library_estimate_blends_done_and_cached_pending() {
        let root = std::env::temp_dir().join(format!("vu_preflight_{}", std::process::id()));
        let source = root.join("src");
        let output = root.join("out");
        let game = source.join("Game");
        std::fs::create_dir_all(&game).unwrap();

        // Two source clips with known sizes.
        let done = game.join("done.mp4");
        let pending = game.join("pending.mp4");
        std::fs::write(&done, vec![0u8; 1000]).unwrap();
        std::fs::write(&pending, vec![0u8; 2000]).unwrap();

        // Manifest: done.mp4 already compressed to a known 300 bytes.
        std::fs::create_dir_all(paths::data_dir(&output)).unwrap();
        let mut manifest = Manifest::default();
        manifest.entries.insert(
            "Game/done.mp4".into(),
            Entry {
                status: Status::Compressed,
                source_bytes: 1000,
                output_bytes: Some(300),
            },
        );
        manifest.save(&paths::manifest_path(&output)).unwrap();

        // Pre-seed the cache for the pending file with its real fingerprint.
        let meta = std::fs::metadata(&pending).unwrap();
        let mut cache = ProbeCache::default();
        cache.insert(
            pending.to_string_lossy().into_owned(),
            2000,
            probe_cache::mtime_ms(&meta),
            &MediaInfo {
                duration_secs: 100.0,
                fps: 30.0,
                width: 2560,
                height: 1440,
            },
        );
        cache.save(&paths::probe_cache_path(&output)).unwrap();

        let est = library_estimate(&source, &output, &enc()).unwrap();

        let pending_facts = ProbeFacts {
            duration_secs: 100.0,
            fps: 30.0,
            src_bytes: 2000,
        };
        let expected_pending = estimate::estimate_output_bytes(&pending_facts, &enc(), 1.0);
        assert_eq!(est.files, 2);
        assert_eq!(est.done_files, 1);
        assert_eq!(est.bytes_now, 3000);
        assert_eq!(est.bytes_est, 300 + expected_pending);

        let _ = std::fs::remove_dir_all(&root);
    }
}
