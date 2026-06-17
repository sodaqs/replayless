use gpui::{FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled};
use gpui_component::v_flex;

/// A label-above / value-below metric chip used in the pre-flight strip and
/// the run-progress stats row.
pub fn stat_chip(
    label: &'static str,
    value: String,
    label_color: Hsla,
    value_color: Hsla,
) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(gpui::div().text_xs().text_color(label_color).child(label))
        .child(
            gpui::div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(value_color)
                .child(SharedString::from(value)),
        )
}
