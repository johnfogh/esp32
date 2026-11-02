use gpui::{canvas, div, prelude::*, px, rgb};
use utils::host::Chip;

use crate::chrome;
use crate::ctrl::Focus;
use crate::host::{FlashDrop, Host};

fn drop_btn(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    kind: FlashDrop,
    title: &str,
    value: String,
) -> gpui::Div {
    let t = host.theme;
    let open = host.flash_drop == Some(kind);
    let focused = match kind {
        FlashDrop::Chip => host.ctrl.focus == Focus::Chips,
        FlashDrop::Project => host.ctrl.focus == Focus::Projects,
    };
    let cell = match kind {
        FlashDrop::Chip => host.flash_chip_b.clone(),
        FlashDrop::Project => host.flash_proj_b.clone(),
    };
    div()
        .flex_1()
        .relative()
        .child(
            div()
                .id(match kind {
                    FlashDrop::Chip => "chip-dd",
                    FlashDrop::Project => "proj-dd",
                })
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .px_2()
                .py_1()
                .border_1()
                .border_color(rgb(if open || focused { t.selection_bg } else { t.border }))
                .bg(rgb(t.background))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.ctrl.focus = match kind {
                        FlashDrop::Chip => Focus::Chips,
                        FlashDrop::Project => Focus::Projects,
                    };
                    this.flash_drop = if this.flash_drop == Some(kind) {
                        None
                    } else {
                        Some(kind)
                    };
                    cx.notify();
                }))
                .child(
                    canvas(
                        move |b, _, _| {
                            cell.set(b);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.))
                        .child(div().text_color(rgb(t.muted)).child(format!(" {title} ")))
                        .child(div().text_color(rgb(t.foreground)).child(value)),
                )
                .child(div().text_color(rgb(t.muted)).child(if open { "▴" } else { "▾" })),
        )
}

fn chip_items(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let a = &host.ctrl;
    let mut list = div().flex().flex_col().w_full();
    for (i, c) in Chip::ALL.iter().enumerate() {
        let port = a.chip_port(i);
        let selected = a.chip == Some(i);
        let disabled = port.is_none();
        let text = match port {
            Some(p) => format!("{} {p}", c.name()),
            None => format!("{} —", c.name()),
        };
        list = list.child(
            div()
                .id(("chip", i as u64))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.ctrl.focus = Focus::Chips;
                    this.ctrl.chip_cursor = i;
                    if this.ctrl.chip_enabled(i) && !this.ctrl.busy() {
                        this.ctrl.chip = Some(i);
                        this.flash_drop = None;
                    }
                    cx.notify();
                }))
                .child(chrome::row(t, selected, false, disabled, i % 2 == 1, text)),
        );
    }
    list
}

fn project_items(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let a = &host.ctrl;
    let mut list = div().flex().flex_col().w_full();
    if a.projects.is_empty() {
        return list.child(
            div()
                .px_2()
                .py_1()
                .text_color(rgb(t.muted))
                .child("no projects in ~/dev/esp32"),
        );
    }
    for (i, p) in a.projects.iter().enumerate() {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let selected = a.project == Some(i);
        list = list.child(
            div()
                .id(("proj", i as u64))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.ctrl.focus = Focus::Projects;
                    this.ctrl.project_cursor = i;
                    if !this.ctrl.busy() {
                        this.ctrl.project = Some(i);
                        this.flash_drop = None;
                    }
                    cx.notify();
                }))
                .child(chrome::row(t, selected, false, false, i % 2 == 1, name)),
        );
    }
    list
}

fn action_btn(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    id: &'static str,
    label: &'static str,
    enabled: bool,
    f: impl Fn(&mut Host, &mut gpui::Context<Host>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.surface))
        .text_color(rgb(if enabled { t.foreground } else { t.disabled }))
        .hover(|s| {
            if enabled {
                s.border_color(rgb(t.accent))
            } else {
                s
            }
        })
        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            f(this, cx);
            cx.notify();
        }))
        .child(label)
}

pub fn view(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let a = &host.ctrl;
    let chip_value = match a.chip.and_then(|i| Chip::ALL.get(i).copied().map(|c| (i, c))) {
        Some((i, c)) => match a.chip_port(i) {
            Some(p) => format!("{}  {p}", c.name()),
            None => c.name().to_string(),
        },
        None => "Select chip".into(),
    };
    let proj_value = a
        .project
        .and_then(|i| a.projects.get(i))
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Select project".into());
    let out_title = if a.scroll.is_some() {
        "Output (scrolled · PgDn to follow)"
    } else {
        "Output"
    };
    let out = chrome::output_text(&a.output, a.scroll, a.output_height.max(1));
    let can_run = a.can_run();
    let can_refresh = a.can_refresh();
    let drop = host.flash_drop;
    let view_b = host.flash_view_b.clone();
    let out_b = host.flash_out_b.clone();
    let out_wrap = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .child(chrome::panel(
            t,
            out_title,
            a.focus == Focus::Output,
            div()
                .id("flash-out")
                .relative()
                .size_full()
                .flex_1()
                .min_h(px(0.))
                .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.flash_drop = None;
                    this.ctrl.focus = Focus::Output;
                    cx.notify();
                }))
                .child(
                    canvas(
                        move |b, _, _| {
                            out_b.set(b);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .text_color(rgb(t.foreground))
                .child(out),
        ));
    let mut root = div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .gap_1()
        .child(
            canvas(
                move |b, _, _| {
                    view_b.set(b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_1()
                .child(drop_btn(host, cx, FlashDrop::Chip, "CHIP", chip_value))
                .child(drop_btn(host, cx, FlashDrop::Project, "PROJECT", proj_value))
                .child(action_btn(host, cx, "flash-build", "Build", can_run, |this, _| {
                    if this.ctrl.can_run() {
                        this.spawn_job(false);
                    }
                }))
                .child(action_btn(host, cx, "flash-flash", "Flash", can_run, |this, _| {
                    if this.ctrl.can_run() {
                        this.spawn_job(true);
                    }
                }))
                .child(action_btn(
                    host,
                    cx,
                    "flash-refresh",
                    "Refresh",
                    can_refresh,
                    |this, _| {
                        if this.ctrl.can_refresh() {
                            this.spawn_scan();
                        }
                    },
                )),
        )
        .child(out_wrap);
    if let Some(kind) = drop {
        let list = match kind {
            FlashDrop::Chip => chip_items(host, cx),
            FlashDrop::Project => project_items(host, cx),
        };
        let view = host.flash_view_b.get();
        let btn = match kind {
            FlashDrop::Chip => host.flash_chip_b.get(),
            FlashDrop::Project => host.flash_proj_b.get(),
        };
        let top = f32::from(btn.bottom() - view.origin.y);
        let left = f32::from(btn.origin.x - view.origin.x);
        let w = f32::from(btn.size.width).max(260.0);
        root = root.child(
            div()
                .absolute()
                .top(px(top.max(0.0)))
                .left(px(left.max(0.0)))
                .min_w(px(w))
                .max_w(px(w.max(420.0)))
                .border_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.background))
                .occlude()
                .child(list),
        );
    }
    root
}
