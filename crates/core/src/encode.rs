use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{Context, Result, bail};

use crate::config::EncodeConfig;
use crate::probe::MediaInfo;
use crate::progress::CancelToken;

/// Build the ffmpeg argument list (everything after `ffmpeg`, excluding the
/// fixed `-hide_banner -loglevel error -nostdin` prefix added by [`run`]).
///
/// Deliberately omits `-hwaccel cuda`: full-GPU decode errors on this RTX 5070 /
/// ffmpeg 8.0.1 and falls back to CPU decode anyway (see CLAUDE.md).
pub fn build_args(enc: &EncodeConfig, src: &Path, dst: &Path, info: &MediaInfo) -> Vec<String> {
    let mut a: Vec<String> = vec!["-y".into(), "-i".into(), src.to_string_lossy().into_owned()];

    // Codec-specific video options.
    match enc.codec.as_str() {
        "av1" => a.extend(["-c:v", "av1_nvenc", "-preset", "p6", "-rc", "vbr"].map(String::from)),
        // hevc is the default for any unrecognized value.
        _ => a.extend(
            [
                "-c:v",
                "hevc_nvenc",
                "-preset",
                "p6",
                "-tune",
                "hq",
                "-rc",
                "vbr",
            ]
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
        a.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            enc.audio.clone(),
        ]);
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
    let split = rate
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rate.len());
    let (num, unit) = rate.split_at(split);
    match num.parse::<u64>() {
        Ok(n) => format!("{}{}", n * 2, unit),
        Err(_) => rate.to_string(),
    }
}

/// Per-block progress parsed from ffmpeg's `-progress` stream.
#[derive(Clone, Copy, Debug)]
pub struct EncodeProgress {
    /// Seconds of output produced so far.
    pub out_secs: f64,
    /// Encoding speed as a realtime multiple (e.g. `3.9` = 3.9× realtime).
    pub speed: f32,
}

/// Whether an ffmpeg run finished on its own or was cancelled mid-stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
}

/// Run ffmpeg with the given args, streaming `-progress` updates to
/// `on_progress`. Honors `cancel`: if the token trips mid-encode the child is
/// killed and `Ok(RunOutcome::Cancelled)` is returned. A non-zero exit that
/// wasn't a cancel is an error carrying ffmpeg's stderr.
///
/// Uses `spawn()` (not `output()`) so the encode can be both observed and
/// cancelled; stderr is drained on a side thread so a full pipe never deadlocks.
pub fn run_with_progress(
    args: &[String],
    cancel: &CancelToken,
    mut on_progress: impl FnMut(EncodeProgress),
) -> Result<RunOutcome> {
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-nostats"])
        .args(["-progress", "pipe:1"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("launching ffmpeg (is it on PATH?)")?;

    // Drain stderr on a side thread so a full pipe never blocks the encode.
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_handle = thread::spawn(move || {
        let mut buf = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buf);
        buf
    });

    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let (mut out_secs, mut speed) = (0.0f64, 0.0f32);
    let mut cancelled = false;

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .context("reading ffmpeg progress")?;
        if n == 0 {
            break; // ffmpeg closed stdout -> it's exiting
        }
        let trimmed = line.trim_end();
        if let Some(v) = trimmed.strip_prefix("out_time=") {
            if let Some(secs) = parse_progress_time(v) {
                out_secs = secs;
            }
        } else if let Some(v) = trimmed.strip_prefix("speed=") {
            speed = v
                .trim()
                .trim_end_matches('x')
                .trim()
                .parse()
                .unwrap_or(speed);
        } else if trimmed.starts_with("progress=") {
            on_progress(EncodeProgress { out_secs, speed });
            if cancel.is_cancelled() {
                let _ = child.kill();
                cancelled = true;
                break;
            }
            if trimmed == "progress=end" {
                break;
            }
        }
    }

    let status = child.wait().context("waiting for ffmpeg")?;
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if cancelled {
        return Ok(RunOutcome::Cancelled);
    }
    if !status.success() {
        bail!("ffmpeg exited with {}: {}", status, stderr_text.trim());
    }
    Ok(RunOutcome::Completed)
}

/// Parse ffmpeg's `out_time=HH:MM:SS.ffffff` field into seconds. Returns `None`
/// for the `N/A` ffmpeg emits before the first frame.
fn parse_progress_time(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
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
        let args = build_args(
            &cfg(),
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(30.0),
        );
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
        let a60 = build_args(
            &cfg(),
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(60.0),
        );
        assert!(joined(&a60).contains("-vf fps=30"));

        // 30 fps source -> no filter
        let a30 = build_args(
            &cfg(),
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(30.0),
        );
        assert!(!joined(&a30).contains("-vf"));

        // 29.97 fps source -> no filter (within epsilon of cap)
        let a2997 = build_args(
            &cfg(),
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(29.97),
        );
        assert!(!joined(&a2997).contains("-vf"));
    }

    #[test]
    fn scale_and_fps_chain_in_one_vf() {
        let mut c = cfg();
        c.scale = Some("1920x1080".into());
        let args = build_args(
            &c,
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(60.0),
        );
        assert!(joined(&args).contains("-vf scale=1920:1080,fps=30"));
    }

    #[test]
    fn fps_cap_zero_disables_filter() {
        let mut c = cfg();
        c.fps_cap = 0;
        let args = build_args(
            &c,
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(60.0),
        );
        assert!(!joined(&args).contains("fps="));
    }

    #[test]
    fn av1_codec_selected_without_tune() {
        let mut c = cfg();
        c.codec = "av1".into();
        let s = joined(&build_args(
            &c,
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(30.0),
        ));
        assert!(s.contains("-c:v av1_nvenc"));
        assert!(!s.contains("-tune")); // av1_nvenc path omits -tune hq
    }

    #[test]
    fn audio_bitrate_reencodes_when_not_copy() {
        let mut c = cfg();
        c.audio = "128k".into();
        let s = joined(&build_args(
            &c,
            &PathBuf::from("in.mp4"),
            &PathBuf::from("out.mp4"),
            &info(30.0),
        ));
        assert!(s.contains("-c:a aac -b:a 128k"));
    }

    #[test]
    fn double_rate_doubles_numeric_prefix() {
        assert_eq!(double_rate("12M"), "24M");
        assert_eq!(double_rate("8M"), "16M");
        assert_eq!(double_rate("20000k"), "40000k");
        assert_eq!(double_rate("garbage"), "garbage");
    }

    #[test]
    fn parse_progress_time_reads_hms() {
        assert_eq!(parse_progress_time("00:01:16.000000"), Some(76.0));
        assert_eq!(parse_progress_time("01:02:03.500000"), Some(3723.5));
        assert_eq!(parse_progress_time("00:00:00.000000"), Some(0.0));
    }

    #[test]
    fn parse_progress_time_rejects_na() {
        assert_eq!(parse_progress_time("N/A"), None);
        assert_eq!(parse_progress_time(""), None);
    }
}
