use std::path::Path;

use replayless_core::config::{Config, EncodeConfig};

/// Identifies which folder a folder-picker session is choosing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Input,
    Output,
}

/// Pre-flight tally for the source library. `bytes_est` is filled in by the
/// heavier [`refine_preflight`] pass; until then the strip shows a seeded flat
/// estimate.
pub struct Preflight {
    pub files: usize,
    pub bytes_now: u64,
    /// Calibrated estimate of total output bytes; `None` until refinement runs.
    pub bytes_est: Option<u64>,
    /// How many source files are already compressed (counted at their real size).
    pub done_files: usize,
}

/// Scan `source` for videos and tally count + total bytes (fast; no ffprobe).
/// The estimate is left unrefined ([`Preflight::bytes_est`] = `None`).
pub fn compute_preflight(source: &Path) -> Option<Preflight> {
    let cfg = Config {
        source_dir: source.to_path_buf(),
        ..Config::default()
    };
    let videos = replayless_core::scan::collect_videos(&cfg).ok()?;
    let bytes_now = videos.iter().map(|v| v.bytes).sum();
    Some(Preflight {
        files: videos.len(),
        bytes_now,
        bytes_est: None,
        done_files: 0,
    })
}

/// Heavier pass: probe pending files (cached) and use exact sizes for already-
/// compressed ones to produce a calibrated output estimate. Spawns `ffprobe`, so
/// callers run it off the UI thread.
pub fn refine_preflight(source: &Path, output: &Path, enc: &EncodeConfig) -> Option<Preflight> {
    let est = replayless_core::preflight::library_estimate(source, output, enc).ok()?;
    Some(Preflight {
        files: est.files,
        bytes_now: est.bytes_now,
        bytes_est: Some(est.bytes_est),
        done_files: est.done_files,
    })
}

pub mod view;
