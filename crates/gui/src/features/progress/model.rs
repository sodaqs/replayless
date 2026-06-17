use std::collections::VecDeque;

use futures::channel::mpsc::UnboundedSender;
use vu_core::compress;
use vu_core::progress::{CancelToken, Event, ProgressSink, Stage};
use vu_core::scan::human_size;

pub struct RunState {
    pub cancel: CancelToken,
    pub stage: Stage,
    pub files_total: usize,
    pub bytes_total: u64,
    pub bytes_out: u64,
    pub files_done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub current_file: Option<String>,
    pub current_fraction: f32,
    pub current_speed: Option<f32>,
    pub current_eta: Option<u64>,
    pub finished: bool,
    pub log: VecDeque<String>,
}

impl RunState {
    pub fn new(cancel: CancelToken) -> Self {
        Self {
            cancel,
            stage: Stage::Compress,
            files_total: 0,
            bytes_total: 0,
            bytes_out: 0,
            files_done: 0,
            failed: 0,
            skipped: 0,
            current_file: None,
            current_fraction: 0.0,
            current_speed: None,
            current_eta: None,
            finished: false,
            log: VecDeque::new(),
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > 8 {
            self.log.pop_front();
        }
    }

    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::StageStarted {
                stage,
                files,
                total_bytes,
            } => {
                self.stage = stage;
                self.files_total = files;
                self.bytes_total = total_bytes;
                self.bytes_out = 0;
                self.files_done = 0;
                self.current_file = None;
            }
            Event::FileStarted { key, .. } => {
                self.current_file = Some(key);
                self.current_fraction = 0.0;
                self.current_speed = None;
                self.current_eta = None;
            }
            Event::FileProgress {
                fraction,
                speed,
                eta_secs,
                ..
            } => {
                self.current_fraction = fraction;
                self.current_speed = speed;
                self.current_eta = eta_secs;
            }
            Event::FileFinished { out_bytes, .. } => {
                self.files_done += 1;
                if let Some(b) = out_bytes {
                    self.bytes_out += b;
                }
                self.current_file = None;
                self.current_fraction = 0.0;
            }
            Event::FileSkipped { .. } => {
                self.files_done += 1;
                self.skipped += 1;
            }
            Event::FileFailed { key, error, .. } => {
                self.files_done += 1;
                self.failed += 1;
                self.current_file = None;
                self.current_fraction = 0.0;
                self.push_log(format!("✗ {}: {error}", basename(&key)));
            }
            Event::StageFinished {
                stage: _,
                ok,
                skipped,
                failed,
                in_bytes,
                out_bytes,
            } => {
                let summary = format!(
                    "Compress: {ok} done, {failed} failed, {skipped} skipped — {} → {} ({})",
                    human_size(in_bytes),
                    human_size(out_bytes),
                    compress::ratio(in_bytes, out_bytes)
                );
                self.push_log(summary);
            }
            Event::Log { message } => self.push_log(message),
        }
    }
}

/// Forwards core progress events to the UI over an unbounded channel.
pub struct ChannelSink(pub UnboundedSender<Event>);

impl ProgressSink for ChannelSink {
    fn emit(&mut self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

pub fn basename(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// Format a remaining-seconds estimate compactly (`1h02m`, `3m05s`, `42s`).
pub fn fmt_eta(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}
