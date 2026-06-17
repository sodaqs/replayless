//! Custom window title bar.
//!
//! Replaces gpui-component's `TitleBar` so the window controls can match the
//! Windows-native caption-button width — the component hard-codes them to the
//! bar height (34px), with no way to resize. Behaviour is otherwise identical:
//! each control declares a [`WindowControlArea`], which the platform hit-tests
//! as the real caption button (drag / minimize / maximize / close). On Windows
//! the OS performs the action, so no click handlers are needed here.

use gpui::{
    App, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px,
};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex};

/// Title-bar height — close to the Windows 11 default.
const BAR_HEIGHT: Pixels = px(34.);
/// Caption-button width. Matches the Windows 11 native value: the buttons are
/// landscape rectangles (wider than the bar is tall), not squares like the
/// gpui-component default.
const BTN_WIDTH: Pixels = px(46.);

/// The three window-management controls, left to right.
#[derive(Clone, Copy)]
enum Control {
    Minimize,
    MaxRestore,
    Close,
}

impl Control {
    fn id(self) -> &'static str {
        match self {
            Control::Minimize => "window-minimize",
            Control::MaxRestore => "window-maximize",
            Control::Close => "window-close",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Control::Minimize => WindowControlArea::Min,
            Control::MaxRestore => WindowControlArea::Max,
            Control::Close => WindowControlArea::Close,
        }
    }
}

/// One caption button. The `WindowControlArea` is what makes the OS trigger the
/// action; the hover/active fills mirror gpui-component's own controls (red for
/// close, a subtle neutral for the rest).
fn control_button(control: Control, maximized: bool, cx: &App) -> impl IntoElement {
    let is_close = matches!(control, Control::Close);
    let icon = match control {
        Control::Minimize => IconName::WindowMinimize,
        Control::MaxRestore if maximized => IconName::WindowRestore,
        Control::MaxRestore => IconName::WindowMaximize,
        Control::Close => IconName::WindowClose,
    };
    let fg = cx.theme().foreground;
    let (hover_bg, hover_fg) = if is_close {
        (cx.theme().danger, cx.theme().danger_foreground)
    } else {
        (cx.theme().secondary_hover, cx.theme().secondary_foreground)
    };
    let active_bg = if is_close {
        cx.theme().danger_active
    } else {
        cx.theme().secondary_active
    };

    div()
        .id(control.id())
        .flex()
        .w(BTN_WIDTH)
        .h_full()
        .flex_shrink_0()
        .justify_center()
        .items_center()
        .text_color(fg)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .active(move |s| s.bg(active_bg).text_color(hover_fg))
        .window_control_area(control.area())
        .child(Icon::new(icon).small())
}

/// Title bar: a draggable brand region on the left, window controls on the right.
#[derive(IntoElement)]
pub struct WindowTitleBar;

impl RenderOnce for WindowTitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let bg = cx.theme().background;
        let fg = cx.theme().foreground;
        let maximized = window.is_maximized();

        h_flex()
            .id("title-bar")
            .h(BAR_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .bg(bg)
            .items_center()
            .justify_between()
            .child(
                // Whole left region is the OS drag area (move + double-click to
                // maximize), filled with the brand mark and name.
                h_flex()
                    .id("title-bar-drag")
                    .window_control_area(WindowControlArea::Drag)
                    .flex_1()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .pl(px(12.))
                    .child(Icon::empty().path("logo.svg").text_color(fg).size_5())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child("Replayless"),
                    ),
            )
            .child(
                h_flex()
                    .id("window-controls")
                    .h_full()
                    .flex_shrink_0()
                    .items_center()
                    .child(control_button(Control::Minimize, maximized, cx))
                    .child(control_button(Control::MaxRestore, maximized, cx))
                    .child(control_button(Control::Close, maximized, cx)),
            )
    }
}
