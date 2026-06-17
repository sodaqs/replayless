use std::path::PathBuf;

use gpui::{
    App, Entity, Hsla, IntoElement, ParentElement, PathPromptOptions, SharedString, Styled, div, px,
};
use gpui_component::{Sizable, button::Button, h_flex};
use vu_core::scan::human_size;

use crate::app::AppView;
use crate::shared::components::stat::stat_chip;

use super::{Preflight, Target};

pub fn folder_row(
    view: &Entity<AppView>,
    label: &'static str,
    value: Option<&PathBuf>,
    target: Target,
    browse_id: &'static str,
    muted: Hsla,
    fg: Hsla,
) -> impl IntoElement {
    let path_text = value
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(not set)".to_string());
    let path_color = if value.is_some() { fg } else { muted };
    let v = view.clone();
    h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .w(px(80.))
                .flex_shrink_0()
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(path_color)
                .overflow_hidden()
                .child(SharedString::from(path_text)),
        )
        .child(
            Button::new(browse_id)
                .label("Browse…")
                .small()
                .on_click(move |_ev, _window, cx| pick_folder(&v, target, cx)),
        )
}

/// Pre-flight summary strip: 4 stat chips in a row.
pub fn preflight_strip(
    pf: &Preflight,
    ratio: f64,
    muted: Hsla,
    fg: Hsla,
    border: Hsla,
) -> impl IntoElement {
    let est = (pf.bytes_now as f64 / ratio) as u64;
    let vdiv = |b: Hsla| div().w(px(1.)).h(px(28.)).bg(b).flex_shrink_0();

    h_flex()
        .gap_5()
        .items_center()
        .child(stat_chip("Files", pf.files.to_string(), muted, fg))
        .child(vdiv(border))
        .child(stat_chip("Source", human_size(pf.bytes_now), muted, fg))
        .child(vdiv(border))
        .child(stat_chip("Est. output", human_size(est), muted, fg))
        .child(vdiv(border))
        .child(stat_chip("Ratio", format!("~{:.1}×", ratio), muted, fg))
}

/// Open the native OS folder picker and write the chosen path into the view.
pub fn pick_folder(view: &Entity<AppView>, target: Target, cx: &mut App) {
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
