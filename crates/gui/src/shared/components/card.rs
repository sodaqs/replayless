use gpui::{Div, Hsla, Styled, div};

/// Returns a styled card container (border + rounded corners + bg + padding).
/// Chain `.child(...)` on the result to add content.
pub fn card(border_color: Hsla, bg: Hsla) -> Div {
    div()
        .rounded_lg()
        .border_1()
        .border_color(border_color)
        .bg(bg)
        .p_4()
}
