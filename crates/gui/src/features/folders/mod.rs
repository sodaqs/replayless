use std::path::Path;

use vu_core::config::Config;

/// Identifies which folder a folder-picker session is choosing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Input,
    Output,
}

/// Pre-flight tally derived from scanning the source folder.
pub struct Preflight {
    pub files: usize,
    pub bytes_now: u64,
}

/// Scan `source` for videos and tally count + total bytes (fast; no ffprobe).
pub fn compute_preflight(source: &Path) -> Option<Preflight> {
    let cfg = Config {
        source_dir: source.to_path_buf(),
        ..Config::default()
    };
    let videos = vu_core::scan::collect_videos(&cfg).ok()?;
    let bytes_now = videos.iter().map(|v| v.bytes).sum();
    Some(Preflight {
        files: videos.len(),
        bytes_now,
    })
}

pub mod view;
