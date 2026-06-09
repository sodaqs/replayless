//! Frontend-agnostic progress reporting + cooperative cancellation.
//!
//! Core stages ([`crate::compress`], [`crate::drive`]) report what they're doing
//! by emitting [`Event`]s into a [`ProgressSink`] and check a [`CancelToken`]
//! between units of work. This keeps the core free of any specific UI: the CLI
//! renders events with `indicatif`, the GUI forwards them to a channel, and tests
//! collect them into a `Vec`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Which stage an [`Event`] belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Compress,
    Upload,
}

/// A single progress event. Units that differ between stages (encoded seconds
/// vs. bytes sent) are normalized to a `fraction` in `0.0..=1.0` so front-ends
/// don't need to know which stage produced them.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A stage began, with the size of its work list.
    StageStarted {
        stage: Stage,
        files: usize,
        total_bytes: u64,
    },
    /// A file began processing.
    FileStarted {
        stage: Stage,
        key: String,
        index: usize,
        total: usize,
        bytes: u64,
    },
    /// Progress within the current file. `fraction` is `0.0..=1.0`; `speed` is the
    /// realtime multiple for compression (e.g. `3.9` = 3.9× realtime), `None` for
    /// uploads.
    FileProgress {
        key: String,
        fraction: f32,
        speed: Option<f32>,
        eta_secs: Option<u64>,
    },
    /// A file finished successfully. `out_bytes` is set for compression,
    /// `drive_id` for uploads.
    FileFinished {
        stage: Stage,
        key: String,
        out_bytes: Option<u64>,
        drive_id: Option<String>,
    },
    /// A file was skipped (already compressed, or already present on Drive).
    FileSkipped {
        stage: Stage,
        key: String,
        reason: String,
    },
    /// A file failed; the run continues with the next file.
    FileFailed {
        stage: Stage,
        key: String,
        error: String,
    },
    /// A stage finished, with totals.
    StageFinished {
        stage: Stage,
        ok: u64,
        skipped: u64,
        failed: u64,
        in_bytes: u64,
        out_bytes: u64,
    },
    /// A free-form, user-facing line (warnings, winget output, etc.).
    Log { message: String },
}

/// Receives [`Event`]s from a running stage. `Send` so the work can run on a
/// background thread.
pub trait ProgressSink: Send {
    fn emit(&mut self, event: Event);
}

/// A sink that discards every event (dry runs, tests that don't assert output).
pub struct NullSink;

impl ProgressSink for NullSink {
    fn emit(&mut self, _event: Event) {}
}

/// Adapt any `FnMut(Event)` closure into a [`ProgressSink`].
pub struct FnSink<F>(pub F);

impl<F: FnMut(Event) + Send> ProgressSink for FnSink<F> {
    fn emit(&mut self, event: Event) {
        (self.0)(event)
    }
}

/// A cheap, clonable cooperative-cancellation flag shared between a front-end
/// (which calls [`CancelToken::cancel`]) and the worker (which polls
/// [`CancelToken::is_cancelled`] between files and kills the live child process).
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once [`cancel`](Self::cancel) has been called on this token (or any
    /// clone of it).
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn cancel_token_starts_uncancelled() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn cancel_token_flips_and_is_shared_across_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled()); // clone observes the same flag
    }

    #[test]
    fn cancel_is_idempotent() {
        let t = CancelToken::new();
        t.cancel();
        t.cancel();
        assert!(t.is_cancelled());
    }

    #[test]
    fn null_sink_swallows_events() {
        let mut s = NullSink;
        s.emit(Event::Log {
            message: "ignored".into(),
        });
        // nothing to assert beyond "does not panic"
    }

    #[test]
    fn fn_sink_forwards_events() {
        let collected = Mutex::new(Vec::new());
        {
            let mut sink = FnSink(|e: Event| collected.lock().unwrap().push(e));
            sink.emit(Event::Log {
                message: "a".into(),
            });
            sink.emit(Event::FileFailed {
                stage: Stage::Compress,
                key: "k".into(),
                error: "boom".into(),
            });
        }
        let events = collected.into_inner().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            Event::Log {
                message: "a".into()
            }
        );
    }
}
