//! video-uploader desktop GUI (gpui + gpui-component).
//!
//! M7: native folder pickers + ffmpeg banner.
//! M8: mode selector, Start/Cancel, and live progress driven by a background
//! worker thread that streams `vu_core::progress::Event`s into the view.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, Hsla, IntoElement, ParentElement,
    PathPromptOptions, Render, SharedString, Styled, Window, WindowBounds, WindowOptions, div, px,
    relative, size,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use vu_core::compress::{self, Overrides};
use vu_core::config::{Config, EncodeConfig};
use vu_core::progress::{CancelToken, Event, NullSink, ProgressSink, Stage};
use vu_core::scan::human_size;
use vu_core::tooling::{self, ToolStatus};

/// Which folder a picker session is choosing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Input,
    Output,
}

/// What a run should do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Compress,
    Upload,
    Both,
}

impl Mode {
    fn does_compress(self) -> bool {
        matches!(self, Mode::Compress | Mode::Both)
    }
    fn does_upload(self) -> bool {
        matches!(self, Mode::Upload | Mode::Both)
    }
    fn label(self) -> &'static str {
        match self {
            Mode::Compress => "Compress",
            Mode::Upload => "Upload",
            Mode::Both => "Compress + Upload",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Mode::Compress => "mode-compress",
            Mode::Upload => "mode-upload",
            Mode::Both => "mode-both",
        }
    }
}

/// Rough output-size estimate shown before a compress run.
struct Preflight {
    files: usize,
    bytes_now: u64,
}

/// Average compaction ratio observed on the real library (187.6 GB → 28.9 GB);
/// used for an instant estimate before any per-file probing.
const AVG_RATIO: f64 = 6.5;

/// Scan `source` for videos and tally count + total bytes (fast; no ffprobe).
fn compute_preflight(source: &Path) -> Option<Preflight> {
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

/// Live state of an in-flight (or finished) run, updated from worker events.
struct RunState {
    cancel: CancelToken,
    stage: Stage,
    files_total: usize,
    bytes_total: u64,
    files_done: usize,
    failed: usize,
    skipped: usize,
    current_file: Option<String>,
    current_fraction: f32,
    current_speed: Option<f32>,
    current_eta: Option<u64>,
    finished: bool,
    log: VecDeque<String>,
}

impl RunState {
    fn new(cancel: CancelToken) -> Self {
        Self {
            cancel,
            stage: Stage::Compress,
            files_total: 0,
            bytes_total: 0,
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

    fn apply(&mut self, ev: Event) {
        match ev {
            Event::StageStarted {
                stage,
                files,
                total_bytes,
            } => {
                self.stage = stage;
                self.files_total = files;
                self.bytes_total = total_bytes;
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
            Event::FileFinished { .. } => {
                self.files_done += 1;
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
                stage,
                ok,
                skipped,
                failed,
                in_bytes,
                out_bytes,
            } => {
                let summary = match stage {
                    Stage::Compress => format!(
                        "Compress: {ok} done, {failed} failed, {skipped} skipped — {} → {} ({})",
                        human_size(in_bytes),
                        human_size(out_bytes),
                        compress::ratio(in_bytes, out_bytes)
                    ),
                    Stage::Upload => format!(
                        "Upload: {ok} uploaded, {skipped} skipped, {failed} failed — {} sent",
                        human_size(in_bytes)
                    ),
                };
                self.push_log(summary);
            }
            Event::Log { message } => self.push_log(message),
        }
    }
}

/// Root application view + state.
struct AppView {
    input_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    ffmpeg: ToolStatus,
    mode: Mode,
    run: Option<RunState>,
    preflight: Option<Preflight>,
    installing: bool,
}

impl AppView {
    fn new() -> Self {
        let cfg = Config::default();
        let input_dir = Some(cfg.source_dir);
        let preflight = input_dir.as_deref().and_then(compute_preflight);
        Self {
            input_dir,
            output_dir: Some(cfg.output_dir),
            ffmpeg: tooling::ffmpeg_status(),
            mode: Mode::Compress,
            run: None,
            preflight,
            installing: false,
        }
    }

    fn set_dir(&mut self, target: Target, path: PathBuf) {
        match target {
            Target::Input => {
                self.preflight = compute_preflight(&path);
                self.input_dir = Some(path);
            }
            Target::Output => self.output_dir = Some(path),
        }
    }

    fn recheck_ffmpeg(&mut self, cx: &mut Context<Self>) {
        self.ffmpeg = tooling::ffmpeg_status();
        cx.notify();
    }

    fn running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| !r.finished)
    }

    /// Whether Start can be pressed in the current state.
    fn can_start(&self) -> bool {
        if self.running() || self.output_dir.is_none() {
            return false;
        }
        if self.mode.does_compress() {
            self.input_dir.is_some() && self.ffmpeg == ToolStatus::Ready
        } else {
            true
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = cx.theme();
        let (bg, fg, muted, accent, success, danger) = (
            theme.background,
            theme.foreground,
            theme.muted_foreground,
            theme.primary,
            theme.success,
            theme.danger,
        );

        // ffmpeg banner
        let (icon, color, text) = match self.ffmpeg {
            ToolStatus::Ready => (IconName::CircleCheck, success, "ffmpeg ready".to_string()),
            ToolStatus::Missing => (
                IconName::CircleX,
                danger,
                "ffmpeg not found — install with: winget install Gyan.FFmpeg".to_string(),
            ),
        };
        let v_re = view.clone();
        let mut banner =
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(icon).text_color(color))
                .child(div().text_color(color).child(SharedString::from(text)))
                .child(Button::new("recheck").label("Re-check").small().on_click(
                    move |_ev, _w, cx| v_re.update(cx, |this, cx| this.recheck_ffmpeg(cx)),
                ));
        if self.ffmpeg == ToolStatus::Missing {
            let v_inst = view.clone();
            let installing = self.installing;
            banner = banner.child(
                Button::new("install-ffmpeg")
                    .label(if installing {
                        "Installing…"
                    } else {
                        "Install ffmpeg"
                    })
                    .small()
                    .disabled(installing)
                    .on_click(move |_ev, _w, cx| install_ffmpeg_action(&v_inst, cx)),
            );
        }

        // Pre-flight estimate (compress modes only)
        let preflight_line =
            self.mode
                .does_compress()
                .then_some(())
                .and(self.preflight.as_ref().map(|pf| {
                    let est = (pf.bytes_now as f64 / AVG_RATIO) as u64;
                    div().text_color(muted).child(SharedString::from(format!(
                        "Pre-flight: {} files · {} → ~{} (~{:.1}×)",
                        pf.files,
                        human_size(pf.bytes_now),
                        human_size(est),
                        AVG_RATIO
                    )))
                }));

        // Mode selector
        let mode_row = h_flex()
            .gap_1()
            .items_center()
            .child(div().w(px(110.)).child("Mode"))
            .child(mode_button(&view, Mode::Compress, self.mode))
            .child(mode_button(&view, Mode::Upload, self.mode))
            .child(mode_button(&view, Mode::Both, self.mode));

        // Start / Cancel
        let v_start = view.clone();
        let start = Button::new("start")
            .label("Start")
            .primary()
            .disabled(!self.can_start())
            .on_click(move |_ev, _w, cx| start_run(&v_start, cx));
        let cancel = self.running().then(|| {
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
        let actions = h_flex().gap_2().child(start).children(cancel);

        div().size_full().bg(bg).text_color(fg).p_4().child(
            v_flex()
                .gap_4()
                .child(div().text_xl().child("video-uploader"))
                .child(banner)
                .child(self.folder_row(
                    &view,
                    "Source folder",
                    self.input_dir.as_ref(),
                    Target::Input,
                    "browse-in",
                    muted,
                ))
                .child(self.folder_row(
                    &view,
                    "Output folder",
                    self.output_dir.as_ref(),
                    Target::Output,
                    "browse-out",
                    muted,
                ))
                .child(mode_row)
                .children(preflight_line)
                .child(actions)
                .children(self.run.as_ref().map(|r| run_panel(r, muted, accent, fg))),
        )
    }
}

impl AppView {
    fn folder_row(
        &self,
        view: &Entity<Self>,
        label: &'static str,
        value: Option<&PathBuf>,
        target: Target,
        browse_id: &'static str,
        muted: Hsla,
    ) -> impl IntoElement {
        let path_text = value
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string());
        let v = view.clone();
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w(px(110.)).child(label))
            .child(
                div()
                    .flex_1()
                    .text_color(muted)
                    .child(SharedString::from(path_text)),
            )
            .child(
                Button::new(browse_id)
                    .label("Browse…")
                    .small()
                    .on_click(move |_ev, _window, cx| pick_folder(&v, target, cx)),
            )
    }
}

/// A mode-selector button, highlighted when it's the active mode.
fn mode_button(view: &Entity<AppView>, mode: Mode, current: Mode) -> Button {
    let v = view.clone();
    let button = Button::new(mode.id())
        .label(mode.label())
        .small()
        .on_click(move |_ev, _w, cx| {
            v.update(cx, |this, c| {
                this.mode = mode;
                c.notify();
            })
        });
    if mode == current {
        button.primary()
    } else {
        button
    }
}

/// The live-progress panel shown while/after a run.
fn run_panel(r: &RunState, muted: Hsla, accent: Hsla, fg: Hsla) -> impl IntoElement {
    // Smoothly include the in-flight file's progress, not just completed files.
    let in_flight = if r.current_file.is_some() {
        r.current_fraction
    } else {
        0.0
    };
    let overall = if r.files_total > 0 {
        (r.files_done as f32 + in_flight) / r.files_total as f32
    } else if r.finished {
        1.0
    } else {
        0.0
    };
    let header = if r.finished {
        format!("Done — {}/{} files", r.files_done, r.files_total)
    } else {
        format!(
            "{}: {}/{} files · {} · {} failed · {} skipped",
            stage_label(r.stage),
            r.files_done,
            r.files_total,
            human_size(r.bytes_total),
            r.failed,
            r.skipped
        )
    };

    let current = r.current_file.as_ref().map(|f| {
        let mut line = format!("{} · {:.0}%", basename(f), r.current_fraction * 100.0);
        if let Some(s) = r.current_speed {
            line.push_str(&format!(" · {s:.1}x"));
        }
        if let Some(e) = r.current_eta {
            line.push_str(&format!(" · ETA {}", fmt_eta(e)));
        }
        div().text_color(muted).child(SharedString::from(line))
    });

    let log = r
        .log
        .iter()
        .cloned()
        .map(|l| div().text_color(muted).child(SharedString::from(l)));

    v_flex()
        .gap_2()
        .pt_2()
        .child(div().text_color(fg).child(SharedString::from(header)))
        .child(progress_bar(overall, muted, accent))
        .children(current)
        .child(v_flex().gap_0p5().children(log))
}

/// A simple two-div progress bar filled to `fraction` (0..=1).
fn progress_bar(fraction: f32, track: Hsla, fill: Hsla) -> impl IntoElement {
    div().w_full().h(px(8.)).rounded_full().bg(track).child(
        div()
            .h_full()
            .w(relative(fraction.clamp(0.0, 1.0)))
            .rounded_full()
            .bg(fill),
    )
}

/// Open the native OS folder picker for `target` and write the chosen path back
/// into the view. Runs asynchronously on gpui's executor so the UI never blocks.
fn pick_folder(view: &Entity<AppView>, target: Target, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    let view = view.clone();
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(picked))) = paths.await
            && let Some(path) = picked.into_iter().next()
        {
            let _ = cx.update(|app| {
                view.update(app, |this, c| {
                    this.set_dir(target, path);
                    c.notify();
                });
            });
        }
    })
    .detach();
}

/// Install ffmpeg via winget on a worker thread, then re-check status on the UI.
fn install_ffmpeg_action(view: &Entity<AppView>, cx: &mut App) {
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
                this.ffmpeg = status;
                c.notify();
            });
        });
    })
    .detach();
}

/// A [`ProgressSink`] that forwards core events to the UI over a channel.
struct ChannelSink(futures::channel::mpsc::UnboundedSender<Event>);

impl ProgressSink for ChannelSink {
    fn emit(&mut self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

/// Kick off a run: spawn the blocking pipeline on a worker thread and drain its
/// progress events into the view on gpui's foreground executor.
fn start_run(view: &Entity<AppView>, cx: &mut App) {
    let (mode, input, output) = {
        let v = view.read(cx);
        (v.mode, v.input_dir.clone(), v.output_dir.clone())
    };
    let Some(output) = output else { return };
    if mode.does_compress() && input.is_none() {
        return;
    }

    let cancel = CancelToken::new();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Event>();

    view.update(cx, |this, c| {
        this.run = Some(RunState::new(cancel.clone()));
        c.notify();
    });

    // Build core inputs. Manifest lives beside the output set.
    let manifest = output.join(".video-uploader").join("manifest.json");
    if let Some(dir) = manifest.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cfg = Config {
        source_dir: input.clone().unwrap_or_else(|| output.clone()),
        output_dir: output,
        manifest,
        encode: EncodeConfig::default(),
    };

    // Worker thread: blocking compress/upload, off the UI thread.
    let worker_cancel = cancel.clone();
    std::thread::spawn(move || {
        let mut sink = ChannelSink(tx);
        if mode.does_compress() && !worker_cancel.is_cancelled() {
            let ov = Overrides {
                jobs: Some(1),
                ..Default::default()
            };
            if let Err(e) = compress::run(&cfg, &ov, &mut sink, &worker_cancel) {
                sink.emit(Event::Log {
                    message: format!("compress error: {e:#}"),
                });
            }
        }
        if mode.does_upload() && !worker_cancel.is_cancelled() {
            let opts = vu_drive::Options::default();
            if let Err(e) = vu_drive::run(&cfg, &opts, &mut sink, &worker_cancel) {
                sink.emit(Event::Log {
                    message: format!("upload error: {e:#}"),
                });
            }
        }
        // `tx` drops here -> the foreground drain loop ends.
    });

    // Foreground: apply events to the view as they arrive.
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

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Compress => "Compressing",
        Stage::Upload => "Uploading",
    }
}

/// The last path component of a forward-slashed manifest key.
fn basename(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
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

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);

        let bounds = Bounds::centered(None, size(px(760.0), px(560.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        cx.open_window(options, |window, cx| {
            let view = cx.new(|_cx| AppView::new());
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open window");

        cx.activate(true);
    });
}
