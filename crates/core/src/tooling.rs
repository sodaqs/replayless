//! External-tool detection and installation: ensure `ffmpeg`/`ffprobe` are
//! present, and install them via `winget` on Windows when they're not.

use std::process::Stdio;

use anyhow::{Context, Result};

use crate::proc::command;

use crate::progress::{Event, ProgressSink};

/// Availability of the ffmpeg tools required for compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Both `ffmpeg` and `ffprobe` are runnable.
    Ready,
    /// One or both could not be launched.
    Missing,
}

/// The winget package id ffmpeg is installed from (matches the manual setup in
/// CLAUDE.md: `winget install Gyan.FFmpeg`).
pub const FFMPEG_WINGET_ID: &str = "Gyan.FFmpeg";

/// Check whether both `ffmpeg` and `ffprobe` run from the current `PATH`.
pub fn ffmpeg_status() -> ToolStatus {
    status_from(tool_runs("ffmpeg"), tool_runs("ffprobe"))
}

/// Pure decision: [`ToolStatus::Ready`] only when both tools launch.
fn status_from(ffmpeg_ok: bool, ffprobe_ok: bool) -> ToolStatus {
    if ffmpeg_ok && ffprobe_ok {
        ToolStatus::Ready
    } else {
        ToolStatus::Missing
    }
}

/// True if `<tool> -version` launches and exits successfully.
fn tool_runs(tool: &str) -> bool {
    command(tool)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The winget args to install ffmpeg non-interactively.
fn winget_install_args() -> Vec<String> {
    [
        "install",
        "--id",
        FFMPEG_WINGET_ID,
        "-e",
        "--source",
        "winget",
        "--accept-package-agreements",
        "--accept-source-agreements",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Whether ffmpeg must be installed before compression can run.
fn needs_install(status: ToolStatus) -> bool {
    matches!(status, ToolStatus::Missing)
}

/// Ensure ffmpeg/ffprobe are usable: a no-op when they're already on `PATH`,
/// otherwise install them via winget (see [`install_ffmpeg`]). Returns the
/// resolved [`ToolStatus`] — callers should treat [`ToolStatus::Missing`] as
/// "still unusable" (e.g. the user dismissed the UAC prompt or winget failed)
/// and surface a manual-install fallback.
pub fn ensure_ffmpeg(sink: &mut dyn ProgressSink) -> Result<ToolStatus> {
    let status = ffmpeg_status();
    if !needs_install(status) {
        return Ok(status);
    }
    sink.emit(Event::Log {
        message: "ffmpeg/ffprobe not found on PATH.".to_string(),
    });
    install_ffmpeg(sink)
}

/// Install ffmpeg via winget, streaming output to `sink` as log events. Returns
/// the [`ToolStatus`] re-checked after the attempt.
///
/// Note: winget may raise a UAC prompt and is absent on very old Windows builds;
/// callers should surface a manual-install fallback if this errors.
pub fn install_ffmpeg(sink: &mut dyn ProgressSink) -> Result<ToolStatus> {
    sink.emit(Event::Log {
        message: format!("Installing ffmpeg via winget ({FFMPEG_WINGET_ID})…"),
    });
    let output = command("winget")
        .args(winget_install_args())
        .output()
        .context("launching winget (is it installed?)")?;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if !line.is_empty() {
            sink.emit(Event::Log {
                message: line.to_string(),
            });
        }
    }
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        sink.emit(Event::Log {
            message: format!("winget exited with {}: {}", output.status, err.trim()),
        });
    }
    Ok(ffmpeg_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_requires_both_tools() {
        assert_eq!(status_from(true, true), ToolStatus::Ready);
        assert_eq!(status_from(true, false), ToolStatus::Missing);
        assert_eq!(status_from(false, true), ToolStatus::Missing);
        assert_eq!(status_from(false, false), ToolStatus::Missing);
    }

    #[test]
    fn needs_install_only_when_missing() {
        assert!(needs_install(ToolStatus::Missing));
        assert!(!needs_install(ToolStatus::Ready));
    }

    #[test]
    fn winget_args_install_ffmpeg_silently() {
        let args = winget_install_args().join(" ");
        assert!(args.contains("install --id Gyan.FFmpeg -e"));
        assert!(args.contains("--accept-package-agreements"));
        assert!(args.contains("--accept-source-agreements"));
    }
}
