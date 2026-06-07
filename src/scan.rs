use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use walkdir::WalkDir;

use crate::config::Config;

/// Extensions we treat as videos to compress.
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "mov", "avi"];

/// One source video discovered under the library root.
#[derive(Debug, Clone)]
pub struct Video {
    pub game: String,
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Default, Clone, Copy)]
struct GameStats {
    files: u64,
    bytes: u64,
}

/// Recursively collect all source videos, tagged with their game folder.
pub fn collect_videos(cfg: &Config) -> Result<Vec<Video>> {
    let root = &cfg.source_dir;
    if !root.is_dir() {
        bail!(
            "source_dir does not exist or is not a directory: {}",
            root.display()
        );
    }

    let mut videos = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || !is_video(entry.path()) {
            continue;
        }
        videos.push(Video {
            game: game_of(root, entry.path()),
            path: entry.path().to_path_buf(),
            bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }
    Ok(videos)
}

/// Walk the source directory, group videos by game folder, and print a table.
pub fn run(cfg: &Config) -> Result<()> {
    let videos = collect_videos(cfg)?;

    let mut games: HashMap<String, GameStats> = HashMap::new();
    let mut total = GameStats::default();
    for v in &videos {
        let stats = games.entry(v.game.clone()).or_default();
        stats.files += 1;
        stats.bytes += v.bytes;
        total.files += 1;
        total.bytes += v.bytes;
    }

    let mut rows: Vec<(String, GameStats)> = games.into_iter().collect();
    rows.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(a.0.cmp(&b.0)));

    println!("{:<40} {:>6} {:>11}", "Game", "Files", "Size");
    println!("{}", "-".repeat(59));
    for (game, st) in &rows {
        println!(
            "{:<40} {:>6} {:>11}",
            truncate(game, 40),
            st.files,
            human_size(st.bytes)
        );
    }
    println!("{}", "-".repeat(59));
    println!(
        "{:<40} {:>6} {:>11}",
        format!("TOTAL ({} games)", rows.len()),
        total.files,
        human_size(total.bytes)
    );

    Ok(())
}

/// The "game" for a video is the first path component beneath `root`.
/// A file directly in `root` (no game folder) is bucketed as "(root)".
fn game_of(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut comps = rel.components();
    let first = comps.next();
    let nested = comps.next().is_some();
    match first {
        Some(c) if nested => c.as_os_str().to_string_lossy().into_owned(),
        _ => "(root)".to_string(),
    }
}

/// True if the path has a known video extension (case-insensitive).
fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Human-readable byte size, e.g. `1.5 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Truncate to `max` characters, appending an ellipsis when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_of_uses_top_level_folder() {
        let root = Path::new(r"C:\Videos\NVIDIA");
        assert_eq!(
            game_of(root, Path::new(r"C:\Videos\NVIDIA\Far Cry 6\clip.mp4")),
            "Far Cry 6"
        );
        // Nested deeper still rolls up to the top-level game folder.
        assert_eq!(
            game_of(root, Path::new(r"C:\Videos\NVIDIA\Far Cry 6\sub\clip.mp4")),
            "Far Cry 6"
        );
    }

    #[test]
    fn game_of_root_file_is_bucketed() {
        let root = Path::new(r"C:\Videos\NVIDIA");
        assert_eq!(game_of(root, Path::new(r"C:\Videos\NVIDIA\loose.mp4")), "(root)");
    }

    #[test]
    fn is_video_matches_known_extensions_case_insensitively() {
        assert!(is_video(Path::new("a.mp4")));
        assert!(is_video(Path::new("a.MP4")));
        assert!(is_video(Path::new("a.mkv")));
        assert!(!is_video(Path::new("a.txt")));
        assert!(!is_video(Path::new("noext")));
    }

    #[test]
    fn human_size_scales_units() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn truncate_shortens_long_strings() {
        assert_eq!(truncate("short", 40), "short");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }
}
