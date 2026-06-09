//! video-uploader desktop GUI (gpui + gpui-component).
//!
//! M7: main window with source/output folder pickers (in-app `Dialog`) and an
//! ffmpeg readiness banner. The compress/upload run UI follows in M8/M9.

mod fsnav;

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, WindowBounds, WindowOptions, div, px,
    size,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::Dialog,
    h_flex,
    scroll::ScrollableElement,
    v_flex,
};
use vu_core::config::Config;
use vu_core::tooling::{self, ToolStatus};

/// Which folder a picker session is choosing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Input,
    Output,
}

/// Active folder-picker navigation state.
struct PickerState {
    target: Target,
    current: PathBuf,
}

/// Root application view + state.
struct AppView {
    input_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    ffmpeg: ToolStatus,
    picker: Option<PickerState>,
}

impl AppView {
    fn new() -> Self {
        let cfg = Config::default();
        Self {
            input_dir: Some(cfg.source_dir),
            output_dir: Some(cfg.output_dir),
            ffmpeg: tooling::ffmpeg_status(),
            picker: None,
        }
    }

    fn open_picker(&mut self, target: Target, cx: &mut Context<Self>) {
        let start = match target {
            Target::Input => self.input_dir.clone(),
            Target::Output => self.output_dir.clone(),
        }
        .filter(|p| p.exists())
        .or_else(|| fsnav::drive_roots().into_iter().next())
        .unwrap_or_else(|| PathBuf::from("."));
        self.picker = Some(PickerState {
            target,
            current: start,
        });
        cx.notify();
    }

    fn navigate(&mut self, to: PathBuf, cx: &mut Context<Self>) {
        if let Some(p) = self.picker.as_mut() {
            p.current = to;
        }
        cx.notify();
    }

    fn commit_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.picker.take() {
            match p.target {
                Target::Input => self.input_dir = Some(p.current),
                Target::Output => self.output_dir = Some(p.current),
            }
        }
        cx.notify();
    }

    fn cancel_picker(&mut self, cx: &mut Context<Self>) {
        self.picker = None;
        cx.notify();
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
            .child(Button::new(browse_id).label("Browse…").small().on_click(
                move |_ev, window, cx| {
                    v.update(cx, |this, cx| this.open_picker(target, cx));
                    let v2 = v.clone();
                    window.open_dialog(cx, move |dialog, window, cx| {
                        build_picker_dialog(dialog, &v2, window, cx)
                    });
                },
            ))
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
        let banner = h_flex()
            .gap_2()
            .items_center()
            .child(Icon::new(icon).text_color(color))
            .child(div().text_color(color).child(SharedString::from(text)))
            .child(
                Button::new("recheck")
                    .label("Re-check")
                    .small()
                    .on_click(move |_ev, _w, cx| {
                        v_re.update(cx, |this, cx| this.recheck_ffmpeg(cx))
                    }),
            );

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

/// Build the folder-picker dialog content from the current picker state.
fn build_picker_dialog(
    dialog: Dialog,
    view: &Entity<AppView>,
    _window: &mut Window,
    cx: &mut App,
) -> Dialog {
    let (target, current) = {
        let st = view.read(cx);
        match st.picker.as_ref() {
            Some(p) => (p.target, p.current.clone()),
            None => return dialog.title("Choose folder"),
        }
    };
    let title = match target {
        Target::Input => "Choose source folder",
        Target::Output => "Choose output folder",
    };
    let muted = cx.theme().muted_foreground;

    // Toolbar: Up + drive roots.
    let mut toolbar = h_flex().gap_1().flex_wrap();
    let up = fsnav::parent(&current);
    {
        let v = view.clone();
        let up2 = up.clone();
        toolbar = toolbar.child(
            Button::new("picker-up")
                .icon(IconName::ArrowUp)
                .label("Up")
                .small()
                .disabled(up.is_none())
                .on_click(move |_ev, _w, cx| {
                    if let Some(p) = up2.clone() {
                        v.update(cx, |this, cx| this.navigate(p, cx));
                    }
                }),
        );
    }
    for (i, drive) in fsnav::drive_roots().into_iter().enumerate() {
        let v = view.clone();
        let d = drive.clone();
        toolbar = toolbar.child(
            Button::new(SharedString::from(format!("drive-{i}")))
                .label(SharedString::from(fsnav::display_name(&drive)))
                .small()
                .on_click(move |_ev, _w, cx| {
                    let d = d.clone();
                    v.update(cx, |this, cx| this.navigate(d, cx));
                }),
        );
    }

    // Entry list (sub-folders).
    let entries = fsnav::subdirs(&current);
    let mut list = v_flex().gap_1();
    if entries.is_empty() {
        list = list.child(div().text_color(muted).child("(no sub-folders)"));
    }
    for (i, entry) in entries.into_iter().enumerate() {
        let v = view.clone();
        let e = entry.clone();
        list = list.child(
            Button::new(SharedString::from(format!("entry-{i}")))
                .icon(IconName::Folder)
                .label(SharedString::from(fsnav::display_name(&entry)))
                .ghost()
                .on_click(move |_ev, _w, cx| {
                    let e = e.clone();
                    v.update(cx, |this, cx| this.navigate(e, cx));
                }),
        );
    }

    let content = v_flex()
        .gap_2()
        .child(
            div()
                .text_color(muted)
                .child(SharedString::from(current.display().to_string())),
        )
        .child(toolbar)
        .child(
            div()
                .id("picker-list")
                .h(px(320.))
                .child(list)
                .overflow_y_scrollbar(),
        );

    let v_ok = view.clone();
    let v_cancel = view.clone();
    dialog
        .title(SharedString::from(title))
        .w(px(560.))
        .confirm()
        .child(content)
        .on_ok(move |_ev, _window, cx| {
            v_ok.update(cx, |this, cx| this.commit_picker(cx));
            true
        })
        .on_cancel(move |_ev, _window, cx| {
            v_cancel.update(cx, |this, cx| this.cancel_picker(cx));
            true
        })
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
