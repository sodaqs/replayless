use std::path::PathBuf;

use futures::StreamExt;
use gpui::{App, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use replayless_core::compress::{self, Overrides};
use replayless_core::config::{Config, EncodeConfig};
use replayless_core::progress::{CancelToken, Event, NullSink, ProgressSink};
use replayless_core::tooling::{self, ToolStatus};

use crate::features::folders::view::{folder_row, preflight_strip};
use crate::features::folders::{Preflight, Target, compute_preflight};
use crate::features::progress::model::{ChannelSink, RunState};
use crate::features::progress::view::run_panel;
use crate::features::settings::Quality;
use crate::features::settings::view::settings_panel;
use crate::shared::components::card::card;
use crate::shared::components::title_bar::WindowTitleBar;

/// Root application state.
pub struct AppView {
    pub input_dir: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    /// `None` while the availability check is still running (at startup or after
    /// a Re-check); resolved off the UI thread by [`start_checks`].
    pub ffmpeg: Option<ToolStatus>,
    pub quality: Quality,
    pub run: Option<RunState>,
    pub preflight: Option<Preflight>,
    pub installing: bool,
}

impl AppView {
    /// Build the view with no blocking work, so the window opens instantly.
    /// The ffmpeg probe (subprocess) and the source-folder pre-flight (file
    /// scan) are kicked off on background threads by [`start_checks`] and fill
    /// in once ready.
    pub fn new() -> Self {
        let cfg = Config::default();
        Self {
            input_dir: Some(cfg.source_dir),
            output_dir: Some(cfg.output_dir),
            ffmpeg: None,
            quality: Quality::Balanced,
            run: None,
            preflight: None,
            installing: false,
        }
    }

    pub fn set_dir(&mut self, target: Target, path: PathBuf) {
        match target {
            Target::Input => {
                self.preflight = compute_preflight(&path);
                self.input_dir = Some(path);
            }
            Target::Output => self.output_dir = Some(path),
        }
    }

    pub fn running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| !r.finished)
    }

    pub fn can_start(&self) -> bool {
        !self.running()
            && self.input_dir.is_some()
            && self.output_dir.is_some()
            && self.ffmpeg == Some(ToolStatus::Ready)
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = cx.theme();
        let bg = theme.background;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let secondary = theme.secondary;
        let accent = theme.primary;
        let success = theme.success;
        let danger = theme.danger;

        // ── ffmpeg status badge ──────────────────────────────────────────────
        // `None` = the check is still running; show a neutral, icon-less pill so
        // we never flash a scary "not found" before the probe finishes.
        let (icon, badge_color, badge_text) = match self.ffmpeg {
            Some(ToolStatus::Ready) => (Some(IconName::CircleCheck), success, "ffmpeg ready"),
            Some(ToolStatus::Missing) => (Some(IconName::CircleX), danger, "ffmpeg not found"),
            None => (None, muted, "checking ffmpeg…"),
        };
        let missing = self.ffmpeg == Some(ToolStatus::Missing);
        let ffmpeg_badge = h_flex()
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(badge_color.opacity(0.4))
                    .bg(badge_color.opacity(0.1))
                    .children(icon.map(|i| Icon::new(i).text_color(badge_color).small()))
                    .child(div().text_xs().text_color(badge_color).child(badge_text)),
            )
            // Re-check is only useful while ffmpeg is missing (e.g. after a
            // manual install); once it's ready there's nothing to re-check.
            .children(missing.then(|| {
                let v_re = view.clone();
                Button::new("recheck")
                    .label("Re-check")
                    .small()
                    .on_click(move |_ev, _w, cx| spawn_ffmpeg_check(&v_re, cx))
            }))
            .children(if missing {
                let v_inst = view.clone();
                let installing = self.installing;
                Some(
                    Button::new("install-ffmpeg")
                        .label(if installing {
                            "Installing…"
                        } else {
                            "Install ffmpeg"
                        })
                        .small()
                        .disabled(installing)
                        .on_click(move |_ev, _w, cx| install_ffmpeg_action(&v_inst, cx)),
                )
            } else {
                None
            });

        // ── custom title bar ─────────────────────────────────────────────────
        // Brand on the left, Windows-native-sized minimize / maximize / close
        // buttons on the right — see `shared::components::title_bar`.
        let title_bar = WindowTitleBar;

        // ── header (ffmpeg status, right-aligned) ────────────────────────────
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_end()
            .child(ffmpeg_badge);

        // ── folders card ─────────────────────────────────────────────────────
        let folders_card = card(border, secondary).child(
            v_flex()
                .gap_3()
                .child(folder_row(
                    &view,
                    "Source",
                    self.input_dir.as_ref(),
                    Target::Input,
                    "browse-in",
                    muted,
                    fg,
                ))
                .child(folder_row(
                    &view,
                    "Output",
                    self.output_dir.as_ref(),
                    Target::Output,
                    "browse-out",
                    muted,
                    fg,
                )),
        );

        // ── settings card ────────────────────────────────────────────────────
        let settings_card =
            card(border, secondary).child(settings_panel(&view, self.quality, muted, border));

        // ── pre-flight strip ─────────────────────────────────────────────────
        let pf_strip = self.preflight.as_ref().map(|pf| {
            card(border, secondary).child(preflight_strip(
                pf,
                self.quality.est_ratio(),
                muted,
                fg,
                border,
            ))
        });

        // ── ffmpeg warning ───────────────────────────────────────────────────
        let ffmpeg_warn = missing.then(|| {
            div()
                .text_xs()
                .text_color(danger)
                .child("ffmpeg is required for compression — install it above.")
        });

        // ── progress card ────────────────────────────────────────────────────
        let progress_card = self.run.as_ref().map(|r| {
            card(border, secondary).child(run_panel(r, muted, accent, fg, danger, success))
        });

        // ── actions ──────────────────────────────────────────────────────────
        let v_start = view.clone();
        let start_btn = Button::new("start")
            .label("Start")
            .primary()
            .disabled(!self.can_start())
            .on_click(move |_ev, _w, cx| start_run(&v_start, cx));
        let cancel_btn = self.running().then(|| {
            let v = view.clone();
            Button::new("cancel")
                .label("Cancel")
                .on_click(move |_ev, _w, cx| {
                    v.update(cx, |this, c| {
                        if let Some(r) = &this.run {
                            r.cancel.cancel();
                        }
                        c.notify();
                    })
                })
        });
        let actions = h_flex()
            .gap_2()
            .justify_end()
            .w_full()
            .children(cancel_btn)
            .child(start_btn);

        // ── root layout ──────────────────────────────────────────────────────
        // Title bar on top (fixed height), the padded content fills the rest.
        div().size_full().bg(bg).text_color(fg).child(
            v_flex().size_full().child(title_bar).child(
                div().flex_1().p_5().child(
                    v_flex()
                        .gap_4()
                        .size_full()
                        .child(header)
                        .child(folders_card)
                        .child(settings_card)
                        .children(pf_strip)
                        .children(ffmpeg_warn)
                        .children(progress_card)
                        .child(actions),
                ),
            ),
        )
    }
}

/// Install ffmpeg via winget on a worker thread, then update ffmpeg status.
pub fn install_ffmpeg_action(view: &Entity<AppView>, cx: &mut App) {
    view.update(cx, |this, c| {
        this.installing = true;
        c.notify();
    });
    let (tx, rx) = futures::channel::oneshot::channel::<ToolStatus>();
    std::thread::spawn(move || {
        let mut sink = NullSink;
        let status = tooling::install_ffmpeg(&mut sink).unwrap_or(ToolStatus::Missing);
        let _ = tx.send(status);
    });
    let view = view.clone();
    cx.spawn(async move |cx| {
        let status = rx.await.unwrap_or(ToolStatus::Missing);
        let _ = cx.update(|app| {
            view.update(app, |this, c| {
                this.installing = false;
                this.ffmpeg = Some(status);
                c.notify();
            });
        });
    })
    .detach();
}

/// Kick off the startup probes — ffmpeg availability and the source-folder
/// pre-flight — on background threads, so opening the window never blocks on
/// subprocess or filesystem work. Call once, right after the view is created.
pub fn start_checks(view: &Entity<AppView>, cx: &mut App) {
    spawn_ffmpeg_check(view, cx);

    let Some(source) = view.read(cx).input_dir.clone() else {
        return;
    };
    let (tx, rx) = futures::channel::oneshot::channel::<Option<Preflight>>();
    std::thread::spawn(move || {
        let _ = tx.send(compute_preflight(&source));
    });
    let view = view.clone();
    cx.spawn(async move |cx| {
        if let Ok(preflight) = rx.await {
            let _ = cx.update(|app| {
                view.update(app, |this, c| {
                    this.preflight = preflight;
                    c.notify();
                });
            });
        }
    })
    .detach();
}

/// Resolve ffmpeg/ffprobe availability on a worker thread, then update the
/// badge. Used at startup and by the Re-check button so the UI never blocks on
/// the `-version` subprocess probes. Sets `ffmpeg = None` ("checking…") for the
/// duration of the check.
pub fn spawn_ffmpeg_check(view: &Entity<AppView>, cx: &mut App) {
    view.update(cx, |this, c| {
        this.ffmpeg = None;
        c.notify();
    });
    let (tx, rx) = futures::channel::oneshot::channel::<ToolStatus>();
    std::thread::spawn(move || {
        let _ = tx.send(tooling::ffmpeg_status());
    });
    let view = view.clone();
    cx.spawn(async move |cx| {
        let status = rx.await.unwrap_or(ToolStatus::Missing);
        let _ = cx.update(|app| {
            view.update(app, |this, c| {
                this.ffmpeg = Some(status);
                c.notify();
            });
        });
    })
    .detach();
}

/// Kick off a compress run on a worker thread and drain its progress events
/// into the view on gpui's foreground executor.
pub fn start_run(view: &Entity<AppView>, cx: &mut App) {
    let (input, output, quality) = {
        let v = view.read(cx);
        (v.input_dir.clone(), v.output_dir.clone(), v.quality)
    };
    let Some(output) = output else { return };
    let Some(input) = input else { return };

    let cancel = CancelToken::new();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Event>();

    view.update(cx, |this, c| {
        this.run = Some(RunState::new(cancel.clone()));
        c.notify();
    });

    let manifest = output.join(".replayless").join("manifest.json");
    if let Some(dir) = manifest.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cfg = Config {
        source_dir: input,
        output_dir: output,
        manifest,
        encode: EncodeConfig::default(),
    };

    let worker_cancel = cancel.clone();
    std::thread::spawn(move || {
        let mut sink = ChannelSink(tx);
        let ov = Overrides {
            cq: Some(quality.cq()),
            maxrate: Some(quality.maxrate().to_string()),
            // Single stream: NVENC saturates at 1 job, so extra jobs only add
            // load without finishing faster (see CLAUDE.md).
            jobs: Some(1),
            ..Default::default()
        };
        if let Err(e) = compress::run(&cfg, &ov, &mut sink, &worker_cancel) {
            sink.emit(Event::Log {
                message: format!("compress error: {e:#}"),
            });
        }
        // `tx` drops here → the foreground drain loop ends.
    });

    let v = view.clone();
    cx.spawn(async move |cx| {
        while let Some(ev) = rx.next().await {
            if cx
                .update(|app| {
                    v.update(app, |this, c| {
                        if let Some(r) = this.run.as_mut() {
                            r.apply(ev);
                        }
                        c.notify();
                    });
                })
                .is_err()
            {
                break;
            }
        }
        let _ = cx.update(|app| {
            v.update(app, |this, c| {
                if let Some(r) = this.run.as_mut() {
                    r.finished = true;
                }
                c.notify();
            });
        });
    })
    .detach();
}
