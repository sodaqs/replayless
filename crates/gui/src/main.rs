//! video-uploader desktop GUI (gpui + gpui-component).
//!
//! M7: main window with source/output folder pickers (native OS dialog) and an
//! ffmpeg readiness banner. The compress/upload run UI follows in M8/M9.

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, Hsla, IntoElement, ParentElement,
    PathPromptOptions, Render, SharedString, Styled, Window, WindowBounds, WindowOptions, div, px,
    size,
};
use gpui_component::{ActiveTheme, Icon, IconName, Root, Sizable, button::Button, h_flex, v_flex};
use vu_core::config::Config;
use vu_core::tooling::{self, ToolStatus};

/// Which folder a picker session is choosing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Input,
    Output,
}

/// Root application view + state.
struct AppView {
    input_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    ffmpeg: ToolStatus,
}

impl AppView {
    fn new() -> Self {
        let cfg = Config::default();
        Self {
            input_dir: Some(cfg.source_dir),
            output_dir: Some(cfg.output_dir),
            ffmpeg: tooling::ffmpeg_status(),
        }
    }

    fn set_dir(&mut self, target: Target, path: PathBuf) {
        match target {
            Target::Input => self.input_dir = Some(path),
            Target::Output => self.output_dir = Some(path),
        }
    }

    fn recheck_ffmpeg(&mut self, cx: &mut Context<Self>) {
        self.ffmpeg = tooling::ffmpeg_status();
        cx.notify();
    }

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

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = cx.theme();
        let (bg, fg, muted, success, danger) = (
            theme.background,
            theme.foreground,
            theme.muted_foreground,
            theme.success,
            theme.danger,
        );

        let (icon, color, text) = match self.ffmpeg {
            ToolStatus::Ready => (IconName::CircleCheck, success, "ffmpeg ready".to_string()),
            ToolStatus::Missing => (
                IconName::CircleX,
                danger,
                "ffmpeg not found — install with: winget install Gyan.FFmpeg".to_string(),
            ),
        };
        let v_re = view.clone();
        let banner =
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(icon).text_color(color))
                .child(div().text_color(color).child(SharedString::from(text)))
                .child(Button::new("recheck").label("Re-check").small().on_click(
                    move |_ev, _w, cx| v_re.update(cx, |this, cx| this.recheck_ffmpeg(cx)),
                ));

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
                )),
        )
    }
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
        // Ok(Ok(Some(_))) = a folder was chosen; cancelled -> None; Err -> failed.
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

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);

        let bounds = Bounds::centered(None, size(px(760.0), px(520.0)), cx);
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
