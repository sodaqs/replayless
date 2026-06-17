use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::proc::command;

/// Basic stream/format info we need to build an encode command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaInfo {
    pub duration_secs: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
}

/// Probe a video with `ffprobe` for duration, frame rate, and dimensions.
pub fn probe(path: &Path) -> Result<MediaInfo> {
    let out = command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate,width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .context("running ffprobe (is it on PATH?)")?;

    if !out.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_ffprobe(&String::from_utf8_lossy(&out.stdout))
        .with_context(|| format!("parsing ffprobe output for {}", path.display()))
}

/// Parse the `key=value` lines ffprobe prints with `default=noprint_wrappers=1`.
fn parse_ffprobe(text: &str) -> Result<MediaInfo> {
    let mut info = MediaInfo {
        duration_secs: 0.0,
        fps: 0.0,
        width: 0,
        height: 0,
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "width" => info.width = value.parse().unwrap_or(0),
            "height" => info.height = value.parse().unwrap_or(0),
            "r_frame_rate" => info.fps = parse_rational(value),
            "duration" => info.duration_secs = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    Ok(info)
}

/// Parse ffprobe's `num/den` frame-rate form (e.g. `60/1`, `30000/1001`) to fps.
fn parse_rational(s: &str) -> f64 {
    match s.split_once('/') {
        Some((num, den)) => {
            let num: f64 = num.parse().unwrap_or(0.0);
            let den: f64 = den.parse().unwrap_or(1.0);
            if den == 0.0 { 0.0 } else { num / den }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_output() {
        let text = "width=2560\nheight=1440\nr_frame_rate=60/1\nduration=180.565478\n";
        let info = parse_ffprobe(text).unwrap();
        assert_eq!(info.width, 2560);
        assert_eq!(info.height, 1440);
        assert!((info.fps - 60.0).abs() < 1e-6);
        assert!((info.duration_secs - 180.565478).abs() < 1e-6);
    }

    #[test]
    fn parse_rational_handles_forms() {
        assert!((parse_rational("30/1") - 30.0).abs() < 1e-6);
        assert!((parse_rational("60/1") - 60.0).abs() < 1e-6);
        assert!((parse_rational("30000/1001") - 29.97).abs() < 0.01);
        assert!((parse_rational("0/0")).abs() < 1e-6); // guards divide-by-zero
        assert!((parse_rational("24") - 24.0).abs() < 1e-6);
    }

    #[test]
    fn missing_fields_default_to_zero() {
        let info = parse_ffprobe("width=1920\n").unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 0);
        assert_eq!(info.fps, 0.0);
    }
}
