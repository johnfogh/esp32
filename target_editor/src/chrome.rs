//! Shared GPUI chrome for controller modes.
use gpui::{div, prelude::*, px, relative, rgb, Div};

use crate::theme::Theme;

pub fn panel(t: Theme, title: &str, focused: bool, child: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h(px(0.))
        .border_1()
        .border_color(rgb(if focused { t.selection_bg } else { t.border }))
        .bg(rgb(t.background))
        .child(
            div()
                .px_2()
                .py_0p5()
                .text_color(rgb(if focused { t.accent } else { t.muted }))
                .child(format!(" {} ", title.to_uppercase())),
        )
        .child(div().flex_1().min_h(px(0.)).w_full().p_1().child(child))
}

pub fn row_bg(t: Theme, selected: bool, stripe: bool) -> u32 {
    if selected {
        t.selection_bg
    } else if stripe {
        t.row_alt
    } else {
        t.background
    }
}

pub fn row(t: Theme, selected: bool, cursor: bool, disabled: bool, stripe: bool, text: String) -> Div {
    let on = selected || cursor;
    let (fg, bg) = if disabled {
        (t.disabled, row_bg(t, false, stripe))
    } else if on {
        (t.selection_fg, t.selection_bg)
    } else {
        (t.foreground, row_bg(t, false, stripe))
    };
    div()
        .w_full()
        .px_1()
        .text_color(rgb(fg))
        .bg(rgb(bg))
        .child(text)
}

pub fn cell(weight: f32, child: impl IntoElement) -> Div {
    let mut d = div()
        .min_w(px(0.))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_start()
        .text_left();
    d.style().flex_grow = Some(weight.max(1.0));
    d.style().flex_shrink = Some(1.0);
    d.style().flex_basis = Some(relative(0.).into());
    d.child(div().w_full().text_left().child(child))
}

pub fn table_row() -> Div {
    div().flex().flex_row().w_full().gap_2().items_center()
}

/// Two-column key/value row. Label is 15% width, right-justified; value takes the rest, left-justified.
pub fn kv_row(t: Theme, fg: u32, last_row: bool, size_px: f32, label: String, value: String) -> Div {
    let mut row = div().flex().flex_row().w_full().items_center();
    if !last_row {
        row = row.border_b_1().border_color(rgb(t.border));
    }
    row.child(
        div()
            .w(relative(0.15))
            .flex()
            .flex_row()
            .justify_end()
            .items_center()
            .px_1()
            .py_0p5()
            .text_right()
            .text_color(rgb(t.muted))
            .text_size(px((size_px - 2.0 * 96.0 / 72.0).max(1.0)))
            .border_r_1()
            .border_color(rgb(t.border))
            .child(label.to_ascii_uppercase()),
    )
    .child(
        div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .flex()
            .flex_row()
            .justify_start()
            .items_center()
            .px_1()
            .py_0p5()
            .text_left()
            .text_color(rgb(fg))
            .child(value),
    )
}

/// Equal-width cells with inner borders. Label rows are uppercase, muted, and smaller.
pub fn grid_row(
    t: Theme,
    fg: u32,
    last_row: bool,
    label: bool,
    size_px: f32,
    vals: impl IntoIterator<Item = String>,
) -> Div {
    let vals: Vec<String> = vals.into_iter().collect();
    let n = vals.len();
    let color = if label { t.muted } else { fg };
    let mut row = div().flex().flex_row().w_full().items_center();
    if !last_row {
        row = row.border_b_1().border_color(rgb(t.border));
    }
    for (i, v) in vals.into_iter().enumerate() {
        let text = if label { v.to_ascii_uppercase() } else { v };
        let mut c = div()
            .flex()
            .flex_row()
            .justify_center()
            .items_center()
            .px_1()
            .py_0p5()
            .text_color(rgb(color))
            .child(text);
        if label {
            c = c.text_size(px((size_px - 2.0 * 96.0 / 72.0).max(1.0)));
        }
        if i + 1 < n {
            c = c.border_r_1().border_color(rgb(t.border));
        }
        c.style().flex_grow = Some(1.0);
        c.style().flex_shrink = Some(1.0);
        c.style().flex_basis = Some(relative(0.).into());
        row = row.child(c);
    }
    row
}

pub fn output_text(lines: &[String], scroll: Option<usize>, height: usize) -> String {
    let max_top = lines.len().saturating_sub(height.max(1));
    let top = scroll.map_or(max_top, |s| s.min(max_top));
    lines.iter().skip(top).take(height.max(1)).cloned().collect::<Vec<_>>().join("\n")
}
