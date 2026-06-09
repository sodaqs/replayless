//! CLI rendering of core [`Event`]s via `indicatif` progress bars.

use std::collections::HashMap;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use vu_core::compress::ratio;
use vu_core::progress::{Event, ProgressSink, Stage};
use vu_core::scan::human_size;

/// A per-file progress bar plus its fixed label prefix.
struct FileBar {
    pb: ProgressBar,
    prefix: String,
}

/// Renders progress events to the terminal: an overall bar plus one bar per
/// in-flight file, with log lines and a final summary.
pub struct CliSink {
    mp: MultiProgress,
    overall: Option<ProgressBar>,
    files: HashMap<String, FileBar>,
}

impl CliSink {
    pub fn new() -> Self {
        Self {
            mp: MultiProgress::new(),
            overall: None,
            files: HashMap::new(),
        }
    }
}

impl Default for CliSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressSink for CliSink {
    fn emit(&mut self, event: Event) {
        match event {
            Event::Log { message } => {
                self.mp.suspend(|| println!("{message}"));
            }
            Event::StageStarted { stage, files, .. } => {
                let pb = self.mp.add(ProgressBar::new(files as u64));
                pb.set_style(
                    ProgressStyle::with_template("{bar:30.cyan/blue} {pos}/{len} files  {msg}")
                        .unwrap(),
                );
                pb.set_message(stage_label(stage).to_string());
                self.overall = Some(pb);
            }
            Event::FileStarted {
                key, index, total, ..
            } => {
                let pb = self.mp.add(ProgressBar::new(100));
                pb.set_style(
                    ProgressStyle::with_template(
                        "  {spinner} [{bar:20.green}] {percent:>3}% {msg}",
                    )
                    .unwrap(),
                );
                let prefix = format!("({index}/{total}) {}", short(&key));
                pb.set_message(prefix.clone());
                pb.enable_steady_tick(Duration::from_millis(120));
                self.files.insert(key, FileBar { pb, prefix });
            }
            Event::FileProgress {
                key,
                fraction,
                speed,
                eta_secs,
            } => {
                if let Some(fb) = self.files.get(&key) {
                    fb.pb.set_position((fraction * 100.0).round() as u64);
                    let extra = match (speed, eta_secs) {
                        (Some(s), Some(e)) => format!("  {s:.1}x  ETA {}", fmt_eta(e)),
                        (Some(s), _) => format!("  {s:.1}x"),
                        _ => String::new(),
                    };
                    fb.pb.set_message(format!("{}{}", fb.prefix, extra));
                }
            }
            Event::FileFinished { key, out_bytes, .. } => {
                if let Some(fb) = self.files.remove(&key) {
                    fb.pb.finish_and_clear();
                }
                if let Some(o) = &self.overall {
                    o.inc(1);
                }
                let size = out_bytes.map(human_size).unwrap_or_default();
                let suffix = if size.is_empty() {
                    String::new()
                } else {
                    format!(" ({size})")
                };
                self.mp.suspend(|| println!("✓ {}{suffix}", short(&key)));
            }
            Event::FileSkipped { key, reason, .. } => {
                self.mp
                    .suspend(|| println!("• {} skipped: {reason}", short(&key)));
            }
            Event::FileFailed { key, error, .. } => {
                if let Some(fb) = self.files.remove(&key) {
                    fb.pb.finish_and_clear();
                }
                if let Some(o) = &self.overall {
                    o.inc(1);
                }
                self.mp.suspend(|| println!("✗ {}: {error}", short(&key)));
            }
            Event::StageFinished {
                stage,
                ok,
                skipped,
                failed,
                in_bytes,
                out_bytes,
            } => {
                if let Some(o) = self.overall.take() {
                    o.finish_and_clear();
                }
                match stage {
                    Stage::Compress => {
                        self.mp.suspend(|| {
                            println!("Compress: {ok} done, {failed} failed, {skipped} skipped.")
                        });
                        if in_bytes > 0 || out_bytes > 0 {
                            self.mp.suspend(|| {
                                println!(
                                    "Size: {} -> {} ({}, saved {}).",
                                    human_size(in_bytes),
                                    human_size(out_bytes),
                                    ratio(in_bytes, out_bytes),
                                    human_size(in_bytes.saturating_sub(out_bytes))
                                )
                            });
                        }
                    }
                    Stage::Upload => {
                        self.mp.suspend(|| {
                            println!(
                                "Upload: {ok} uploaded, {skipped} skipped, {failed} failed. {} sent.",
                                human_size(in_bytes)
                            )
                        });
                    }
                }
            }
        }
    }
}

/// The last path component of a forward-slashed manifest key.
fn short(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Compress => "Compress",
        Stage::Upload => "Upload",
    }
}

/// Format a remaining-seconds estimate compactly (`1h02m`, `3m05s`, `42s`).
fn fmt_eta(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_takes_basename() {
        assert_eq!(short("Far Cry 6/clip.mp4"), "clip.mp4");
        assert_eq!(short("loose.mp4"), "loose.mp4");
    }

    #[test]
    fn fmt_eta_scales_units() {
        assert_eq!(fmt_eta(42), "42s");
        assert_eq!(fmt_eta(185), "3m05s");
        assert_eq!(fmt_eta(3725), "1h02m");
    }
}
