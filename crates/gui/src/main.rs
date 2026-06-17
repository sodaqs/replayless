// Don't pop a console window behind the GUI on Windows release builds. Debug
// builds keep the console so `tracing` output stays visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod features;
mod shared;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, rgb,
    size,
};
use gpui_component::{Root, Theme, TitleBar};

fn main() {
    Application::new()
        .with_assets(shared::assets::Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);

            // The bundled dark theme paints `success` as a very dark green
            // (#14532d), which is nearly unreadable on the near-black app
            // background. Brighten it to Tailwind green-400 so the
            // "ffmpeg ready" badge and the "Done" header read clearly. We never
            // re-sync the system appearance, so this override sticks.
            Theme::global_mut(cx).success = rgb(0x4ade80).into();

            // The custom title bar lives inside the client area (the native bar is
            // hidden via `appears_transparent`), so grow the window by the bar's
            // ~34px to keep the same usable content height.
            let bounds = Bounds::centered(None, size(px(800.0), px(800.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(800.0), px(800.0))),
                // The component's options leave `title` as `None`, so the window
                // had no native title — nothing showed in the taskbar tooltip or
                // Alt+Tab. Set it here; the visible in-client caption is still
                // drawn by our custom `WindowTitleBar`.
                titlebar: Some(TitlebarOptions {
                    title: Some("Replayless".into()),
                    ..TitleBar::title_bar_options()
                }),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let view = cx.new(|_cx| app::AppView::new());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");

            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use crate::features::progress::model::{RunState, fmt_eta};
    use crate::features::settings::Quality;
    use replayless_core::progress::{CancelToken, Event, Stage};

    #[test]
    fn quality_presets_map_to_cq_and_maxrate() {
        assert_eq!(Quality::Balanced.cq(), 30);
        assert_eq!(Quality::Smaller.cq(), 32);
        assert_eq!(Quality::Higher.cq(), 28);
        assert_eq!(Quality::Balanced.maxrate(), "12M");
        assert_eq!(Quality::Smaller.maxrate(), "8M");
        assert_eq!(Quality::Higher.maxrate(), "16M");
    }

    #[test]
    fn quality_est_ratio_orders_smaller_highest() {
        assert!(Quality::Smaller.est_ratio() > Quality::Balanced.est_ratio());
        assert!(Quality::Balanced.est_ratio() > Quality::Higher.est_ratio());
    }

    #[test]
    fn run_state_tracks_progress_and_completion() {
        let mut r = RunState::new(CancelToken::new());
        r.apply(Event::StageStarted {
            stage: Stage::Compress,
            files: 2,
            total_bytes: 100,
        });
        assert_eq!(r.files_total, 2);
        r.apply(Event::FileStarted {
            stage: Stage::Compress,
            key: "a".into(),
            index: 1,
            total: 2,
            bytes: 50,
        });
        assert_eq!(r.current_file.as_deref(), Some("a"));
        r.apply(Event::FileProgress {
            key: "a".into(),
            fraction: 0.5,
            speed: Some(4.0),
            eta_secs: Some(10),
        });
        assert_eq!(r.current_fraction, 0.5);
        r.apply(Event::FileFinished {
            stage: Stage::Compress,
            key: "a".into(),
            out_bytes: Some(10),
        });
        assert_eq!(r.files_done, 1);
        assert_eq!(r.bytes_out, 10);
        assert!(r.current_file.is_none());
        r.apply(Event::FileFailed {
            stage: Stage::Compress,
            key: "b".into(),
            error: "boom".into(),
        });
        assert_eq!(r.files_done, 2);
        assert_eq!(r.failed, 1);
    }

    #[test]
    fn fmt_eta_scales_units() {
        assert_eq!(fmt_eta(42), "42s");
        assert_eq!(fmt_eta(185), "3m05s");
        assert_eq!(fmt_eta(3725), "1h02m");
    }
}
