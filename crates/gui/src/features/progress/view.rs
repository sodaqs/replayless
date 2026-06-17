use gpui::{FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div, px};
use gpui_component::{h_flex, progress::Progress, v_flex};
use vu_core::progress::Stage;
use vu_core::scan::human_size;

use crate::shared::components::stat::stat_chip;

use super::model::{RunState, basename, fmt_eta};

pub fn run_panel(
    r: &RunState,
    muted: Hsla,
    accent: Hsla,
    fg: Hsla,
    danger: Hsla,
    success: Hsla,
) -> impl IntoElement {
    let in_flight = r.current_file.as_ref().map_or(0.0, |_| r.current_fraction);
    let overall = if r.files_total > 0 {
        (r.files_done as f32 + in_flight) / r.files_total as f32
    } else if r.finished {
        1.0
    } else {
        0.0
    };

    let header_color = if r.finished { success } else { fg };
    let header_text = if r.finished {
        format!("Done — {}/{} files", r.files_done, r.files_total)
    } else {
        format!("{}: {}/{}", stage_str(r.stage), r.files_done, r.files_total)
    };

    // Stats row
    let speed_str = r
        .current_speed
        .map(|s| format!("{s:.1}×"))
        .unwrap_or_else(|| "—".to_string());
    let eta_str = r
        .current_eta
        .map(fmt_eta)
        .unwrap_or_else(|| "—".to_string());
    let out_str = if r.bytes_out > 0 {
        human_size(r.bytes_out)
    } else {
        human_size(r.bytes_total)
    };

    let stats_row = h_flex()
        .gap_6()
        .child(stat_chip(
            "Files",
            format!("{}/{}", r.files_done, r.files_total),
            muted,
            fg,
        ))
        .child(stat_chip(
            if r.bytes_out > 0 {
                "Compressed"
            } else {
                "Total"
            },
            out_str,
            muted,
            fg,
        ))
        .child(stat_chip("Speed", speed_str, muted, fg))
        .child(stat_chip("ETA", eta_str, muted, fg))
        .children((r.failed > 0).then(|| stat_chip("Failed", r.failed.to_string(), danger, danger)))
        .children(
            (r.skipped > 0).then(|| stat_chip("Skipped", r.skipped.to_string(), muted, muted)),
        );

    // Current file block
    let current_block = r.current_file.as_ref().map(|f| {
        let name = basename(f);
        let detail = {
            let mut parts = vec![format!("{:.0}%", r.current_fraction * 100.)];
            if let Some(s) = r.current_speed {
                parts.push(format!("{s:.1}×"));
            }
            if let Some(e) = r.current_eta {
                parts.push(fmt_eta(e));
            }
            parts.join(" · ")
        };
        v_flex()
            .gap_1()
            .pt_2()
            .child(
                div()
                    .text_sm()
                    .text_color(fg)
                    .child(SharedString::from(format!("▶  {name}"))),
            )
            .child(
                Progress::new()
                    .value(r.current_fraction * 100.)
                    .h(px(6.))
                    .bg(accent),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(detail)),
            )
    });

    // Log
    let log_section = (!r.log.is_empty()).then(|| {
        v_flex().gap_0p5().pt_2().children(r.log.iter().map(|line| {
            div()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(line.clone()))
        }))
    });

    v_flex()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(header_color)
                .child(SharedString::from(header_text)),
        )
        .child(Progress::new().value(overall * 100.).h(px(16.)))
        .child(stats_row)
        .children(current_block)
        .children(log_section)
}

fn stage_str(stage: Stage) -> &'static str {
    match stage {
        Stage::Compress => "Compressing",
    }
}
