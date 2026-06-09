//! M7 build spike: prove `gpui` + `gpui-component` build and open a window on
//! this machine. Intentionally minimal — real UI lands in M8/M9.

use gpui::{
    App, AppContext, Application, Bounds, Context, IntoElement, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use gpui_component::Root;

struct HelloView;

impl Render for HelloView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child("video-uploader")
            .child("gpui + gpui-component window spike ✓")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);

        let bounds = Bounds::centered(None, size(px(640.0), px(420.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        cx.open_window(options, |window, cx| {
            let view = cx.new(|_cx| HelloView);
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open window");

        cx.activate(true);
    });
}
