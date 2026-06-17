use gpui::prelude::FluentBuilder;
use gpui::{Div, Entity, Hsla, IntoElement, ParentElement, Styled, div, px};
use gpui_component::{
    Sizable,
    button::{Button, ButtonRounded, ButtonVariants},
    h_flex, v_flex,
};

use crate::app::AppView;

use super::Quality;

/// Wraps a group of segmented-control buttons in a bordered container.
fn seg_group(border: Hsla) -> Div {
    h_flex()
        .border_1()
        .border_color(border)
        .rounded_md()
        .overflow_hidden()
}

fn quality_btn(view: &Entity<AppView>, quality: Quality, current: Quality) -> Button {
    let v = view.clone();
    Button::new(quality.id())
        .label(quality.label())
        .small()
        .rounded(ButtonRounded::None)
        .when(quality == current, |b| b.primary())
        .on_click(move |_ev, _w, cx| {
            v.update(cx, |this, c| {
                this.quality = quality;
                c.notify();
            })
        })
}

fn row_label(text: &'static str, muted: Hsla) -> impl IntoElement {
    div()
        .text_sm()
        .text_color(muted)
        .w(px(72.))
        .flex_shrink_0()
        .child(text)
}

/// Renders the settings panel: Quality preset.
pub fn settings_panel(
    view: &Entity<AppView>,
    quality: Quality,
    muted: Hsla,
    border: Hsla,
) -> impl IntoElement {
    v_flex().gap_3().child(
        h_flex()
            .gap_3()
            .items_center()
            .child(row_label("Quality", muted))
            .child(
                seg_group(border)
                    .child(quality_btn(view, Quality::Balanced, quality))
                    .child(quality_btn(view, Quality::Smaller, quality))
                    .child(quality_btn(view, Quality::Higher, quality)),
            ),
    )
}
