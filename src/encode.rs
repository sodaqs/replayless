use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::EncodeConfig;
use crate::probe::MediaInfo;

/// Build the ffmpeg argument list (everything after `ffmpeg`, excluding the
/// fixed `-hide_banner -loglevel error -nostdin` prefix added by [`run`]).
///
/// Deliberately omits `-hwaccel cuda`: full-GPU decode errors on this RTX 5070 /
/// ffmpeg 8.0.1 and falls back to CPU decode anyway (see CLAUDE.md).
pub fn build_args(enc: &EncodeConfig, src: &Path, dst: &Path, info: &MediaInfo) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-y".into(),
        "-i".into(),
        src.to_string_lossy().into_owned(),
    ];

    // Codec-specific video options.
    match enc.codec.as_str() {
        "av1" => a.extend(
            ["-c:v", "av1_nvenc", "-preset", "p6", "-rc", "vbr"]
                .map(String::from),
        ),
        // hevc is the default for any unrecognized value.
        _ => a.extend(
            ["-c:v", "hevc_nvenc", "-preset", "p6", "-tune", "hq", "-rc", "vbr"]
                .map(String::from),
        ),
    }
    a.extend([
        "-cq".into(),
        enc.cq.to_string(),
        "-b:v".into(),
        "0".into(),
        "-maxrate".into(),
        enc.maxrate.clone(),
        "-bufsize".into(),
        double_rate(&enc.maxrate),
    ]);

    // Video filters: optional downscale, then optional frame-rate cap.
    if let Some(vf) = build_vf(enc, info) {
        a.push("-vf".into());
        a.push(vf);
    }

    // Audio.
    if enc.audio == "copy" {
        a.extend(["-c:a", "copy"].map(String::from));
    } else {
        a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), enc.audio.clone()]);
    }

    a.extend(["-movflags", "+faststart"].map(String::from));
    a.push(dst.to_string_lossy().into_owned());
    a
}

/// Build the `-vf` filter string, or `None` if no filtering is needed.
///
/// The frame-rate cap is applied **only when the source exceeds it** (e.g. a
/// 60 fps clip with `fps_cap = 30`); clips already at/under the cap pass through
/// untouched so we don't needlessly re-time them.
fn build_vf(enc: &EncodeConfig, info: &MediaInfo) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(scale) = &enc.scale {
        parts.push(format!("scale={}", scale.replace('x', ":")));
    }
    if enc.fps_cap > 0 && info.fps > enc.fps_cap as f64 + 0.1 {
        parts.push(format!("fps={}", enc.fps_cap));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

/// Double a bitrate string like `12M` -> `24M` (for `-bufsize`).
fn double_rate(rate: &str) -> String {
    let rate = rate.trim();
    let split = rate.find(|c: char| !c.is_ascii_digit()).unwrap_or(rate.len());
    let (num, unit) = rate.split_at(split);
    match num.parse::<u64>() {
        Ok(n) => format!("{}{}", n * 2, unit),
        Err(_) => rate.to_string(),
    }
}

/// Run ffmpeg with the given args, returning an error (with stderr) on failure.
pub fn run(args: &[String]) -> Result<()> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(args)
        .output()
        .context("launching ffmpeg (is it on PATH?)")?;

    if !out.status.success() {
        bail!(
            "ffmpeg exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg() -> EncodeConfig {
        EncodeConfig {
            codec: "hevc".into(),
            cq: 30,
            maxrate: "12M".into(),
            fps_cap: 30,
            audio: "copy".into(),
            jobs: 2,
            scale: None,
        }
    }

    fn info(fps: f64) -> MediaInfo {
        MediaInfo {
            duration_secs: 100.0,
            fps,
            width: 2560,
            height: 1440,
        }
    }

    fn joined(args: &[String]) -> String {
        args.join(" ")
    }

    #[test]
    fn default_hevc_command_has_expected_flags() {
        let args = build_args(&cfg(), &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(30.0));
        let s = joined(&args);
        assert!(s.contains("-c:v hevc_nvenc"));
        assert!(s.contains("-cq 30"));
        assert!(s.contains("-maxrate 12M"));
        assert!(s.contains("-bufsize 24M"));
        assert!(s.contains("-c:a copy"));
        assert!(s.contains("-movflags +faststart"));
        assert!(!s.contains("-hwaccel")); // GPU decode intentionally omitted
    }

    #[test]
    fn fps_cap_applies_only_above_threshold() {
        // 60 fps source -> filter added
        let a60 = build_args(&cfg(), &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(60.0));
        assert!(joined(&a60).contains("-vf fps=30"));

        // 30 fps source -> no filter
        let a30 = build_args(&cfg(), &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(30.0));
        assert!(!joined(&a30).contains("-vf"));

        // 29.97 fps source -> no filter (within epsilon of cap)
        let a2997 = build_args(&cfg(), &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(29.97));
        assert!(!joined(&a2997).contains("-vf"));
    }

    #[test]
    fn scale_and_fps_chain_in_one_vf() {
        let mut c = cfg();
        c.scale = Some("1920x1080".into());
        let args = build_args(&c, &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(60.0));
        assert!(joined(&args).contains("-vf scale=1920:1080,fps=30"));
    }

    #[test]
    fn fps_cap_zero_disables_filter() {
        let mut c = cfg();
        c.fps_cap = 0;
        let args = build_args(&c, &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(60.0));
        assert!(!joined(&args).contains("fps="));
    }

    #[test]
    fn av1_codec_selected_without_tune() {
        let mut c = cfg();
        c.codec = "av1".into();
        let s = joined(&build_args(&c, &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(30.0)));
        assert!(s.contains("-c:v av1_nvenc"));
        assert!(!s.contains("-tune")); // av1_nvenc path omits -tune hq
    }

    #[test]
    fn audio_bitrate_reencodes_when_not_copy() {
        let mut c = cfg();
        c.audio = "128k".into();
        let s = joined(&build_args(&c, &PathBuf::from("in.mp4"), &PathBuf::from("out.mp4"), &info(30.0)));
        assert!(s.contains("-c:a aac -b:a 128k"));
    }

    #[test]
    fn double_rate_doubles_numeric_prefix() {
        assert_eq!(double_rate("12M"), "24M");
        assert_eq!(double_rate("8M"), "16M");
        assert_eq!(double_rate("20000k"), "40000k");
        assert_eq!(double_rate("garbage"), "garbage");
    }
}
