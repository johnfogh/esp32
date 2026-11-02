use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gpui::{
    canvas, div, fill, img, point, prelude::*, px, relative, rgb, size, Bounds, ClickEvent,
    ObjectFit, PathBuilder, Pixels, Window,
};

use crate::chrome;
use crate::ctrl::Focus;
use crate::host::Host;

const EVENT_CAP: usize = 2000;



#[derive(Clone)]
pub struct CourseTarget {
    pub name: String,
    pub def: Option<String>,
    pub image: Option<Arc<gpui::Image>>,
    pub img_size: Option<(u32, u32)>,
    pub img_w: Rc<Cell<f32>>,
    pub img_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub size_cm: Option<(f64, f64)>,
    pub zones: Vec<String>,
    pub geometry: Vec<targetlib::Zone>,
    pub err: Option<String>,
    pub hub: Option<String>,
    pub hub_label: Option<String>,
    /// Optional grid position from `define_grid` / `define_target`.
    pub row: Option<u32>,
    pub col: Option<u32>,
    /// Optional distance string from `define_distance`.
    pub distance: Option<String>,
    /// Image-pixel hit marks for the current running script.
    pub hits: Vec<(f32, f32)>,
}

impl CourseTarget {
    pub fn blank(name: String) -> Self {
        Self {
            name,
            def: None,
            image: None,
            img_size: None,
            img_w: Rc::new(Cell::new(0.0)),
            img_bounds: Rc::new(Cell::new(Bounds::default())),
            size_cm: None,
            zones: Vec::new(),
            geometry: Vec::new(),
            err: None,
            hub: None,
            hub_label: None,
            row: None,
            col: None,
            distance: None,
            hits: Vec::new(),
        }
    }

    pub fn set_def(&mut self, stem: String) {
        self.def = Some(stem.clone());
        let p = crate::targets::preview(&crate::targets::path_for(&stem));
        self.image = p.image;
        self.img_size = p.size;
        self.size_cm = p.size_cm;
        self.zones = p.zones;
        self.geometry = p.geometry;
        self.err = p.err;
    }
}

pub struct Course {
    pub scripts: Vec<String>,
    pub ix: usize,
    pub script_drop: bool,
    /// Index into today's roster names; session-only.
    pub roster_ix: Option<usize>,
    pub roster_drop: bool,
    /// JSON object keyed by local `HH:MM:SS`. Same-second messages become arrays.
    pub events: serde_json::Map<String, serde_json::Value>,
    pub running: bool,
    pub simulate: bool,
    pub stop: Arc<AtomicBool>,
    pub live: Arc<AtomicBool>,
    pub targets: Vec<CourseTarget>,
    /// From `define_grid`; when set, TARGETS lays out as a CSS grid.
    pub grid_rows: Option<u32>,
    pub grid_cols: Option<u32>,
    pub name_edit: Option<(usize, String)>,
    pub hub_drop: Option<usize>,
    /// Session SENSOR HUB by (script file name, card name). Not persisted.
    pub hub_by_card: HashMap<(String, String), (String, String)>,
    pub view_b: Rc<Cell<Bounds<Pixels>>>,
    pub hub_b: Rc<Cell<Bounds<Pixels>>>,
}

impl Course {
    pub fn new() -> Self {
        Self {
            scripts: Vec::new(),
            ix: 0,
            script_drop: false,
            roster_ix: None,
            roster_drop: false,
            events: serde_json::Map::new(),
            running: false,
            simulate: false,
            stop: Arc::new(AtomicBool::new(false)),
            live: Arc::new(AtomicBool::new(false)),
            targets: Vec::new(),
            grid_rows: None,
            grid_cols: None,
            name_edit: None,
            hub_drop: None,
            hub_by_card: HashMap::new(),
            view_b: Rc::new(Cell::new(Bounds::default())),
            hub_b: Rc::new(Cell::new(Bounds::default())),
        }
    }
}

fn stage_label(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

fn action_btn(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    enabled: bool,
    f: impl Fn(&mut Host, &mut gpui::Context<Host>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    div()
        .id(id)
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .justify_center()
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
            if enabled {
                f(this, cx);
                cx.notify();
            }
        }))
        .child(label.into())
}

pub fn view(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let c = &host.course;
    let view_b = c.view_b.clone();
    let hub_open = c.hub_drop;
    let mut root = div()
        .relative()
        .flex()
        .flex_row()
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
        .child(events_panel(host, cx))
        .child(targets_panel(host, cx));
    if let Some(i) = hub_open {
        root = root.child(hub_list(host, cx, i));
    }
    root
}

fn roster_control(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    value: String,
    open: bool,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    div()
        .id("course-roster")
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .py_1()
        .border_1()
        .border_color(rgb(if open { t.selection_bg } else { t.border }))
        .bg(rgb(t.background))
        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            this.ctrl.focus = Focus::CourseEvents;
            this.course.script_drop = false;
            this.course.hub_drop = None;
            this.course.roster_drop = !this.course.roster_drop;
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .child(div().text_color(rgb(t.muted)).child(" SHOOTER NAME "))
                .child(div().text_color(rgb(t.foreground)).child(value)),
        )
        .child(
            div()
                .text_color(rgb(t.muted))
                .child(if open { "▴" } else { "▾" }),
        )
}

fn script_control(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    value: String,
    open: bool,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    div()
        .id("course-script")
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .py_1()
        .border_1()
        .border_color(rgb(if open { t.selection_bg } else { t.border }))
        .bg(rgb(t.background))
        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            this.ctrl.focus = Focus::CourseEvents;
            this.course.roster_drop = false;
            this.course.hub_drop = None;
            this.course.script_drop = !this.course.script_drop;
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .child(div().text_color(rgb(t.muted)).child(" STAGE "))
                .child(div().text_color(rgb(t.foreground)).child(value)),
        )
        .child(
            div()
                .text_color(rgb(t.muted))
                .child(if open { "▴" } else { "▾" }),
        )
}

fn events_panel(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let c = &host.course;
    let roster_open = c.roster_drop;
    let script_open = c.script_drop;
    let roster_value = c
        .roster_ix
        .and_then(|i| host.roster.name(i))
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "Select shooter".into());
    let script_value = c
        .scripts
        .get(c.ix)
        .map(|n| stage_label(n))
        .unwrap_or_else(|| "Select stage".into());

    let body = if roster_open {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_1()
            .child(roster_control(host, cx, roster_value, true))
            .child(roster_list(host, cx))
    } else if script_open {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_1()
            .child(script_control(host, cx, script_value, true))
            .child(script_list(host, cx))
    } else {
        let can_start =
            !c.running && !c.scripts.is_empty() && host.course_start_errors().is_empty();
        let can_stop = c.running;
        let mut log = div().flex().flex_col().w_full();
        if c.events.is_empty() {
            log = log.child(
                div()
                    .text_color(rgb(t.muted))
                    .child("Start a script to see events"),
            );
        } else {
            let size_px = host.line_px();
            let mut logs = 0usize;
            for (ts, v) in c.events.iter() {
                log = log.child(time_sep(t, ts));
                let items: Vec<&serde_json::Value> = match v {
                    serde_json::Value::Array(a) => a.iter().collect(),
                    other => vec![other],
                };
                let mut i = 0;
                while i < items.len() {
                    match items[i] {
                        serde_json::Value::String(line) => {
                            log = log.child(chrome::row(
                                t,
                                false,
                                false,
                                false,
                                logs % 2 == 1,
                                line.clone(),
                            ));
                            logs += 1;
                            i += 1;
                        }
                        other if is_target_hit(other) => {
                            let mut hits = vec![other];
                            i += 1;
                            while i < items.len() && is_target_hit(items[i]) {
                                hits.push(items[i]);
                                i += 1;
                            }
                            log = log.child(target_hit_card(t, size_px, &hits));
                        }
                        other => {
                            log = log.child(message_card(t, size_px, other));
                            i += 1;
                        }
                    }
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_1()
            .child(roster_control(host, cx, roster_value, false))
            .child(script_control(host, cx, script_value, false))
            .child({
                let on = c.simulate;
                div()
                    .id("course-simulate")
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if on { t.accent } else { t.border }))
                    .bg(rgb(if on { t.selection_bg } else { t.surface }))
                    .text_color(rgb(if on { t.selection_fg } else { t.foreground }))
                    .hover(|s| s.border_color(rgb(t.accent)))
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        this.course.simulate = !this.course.simulate;
                        cx.notify();
                    }))
                    .child("Simulate")
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap_1()
                    .child(action_btn(host, cx, "course-start", "Start", can_start, |this, cx| {
                        this.start_course(cx);
                    }))
                    .child(action_btn(host, cx, "course-stop", "Stop", can_stop, |this, _| {
                        this.stop_course();
                    })),
            )
            .child(
                div()
                    .id("course-events")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        this.ctrl.focus = Focus::CourseEvents;
                        this.course.roster_drop = false;
                        this.course.script_drop = false;
                        this.course.hub_drop = None;
                        cx.notify();
                    }))
                    .child(log),
            )
    };

    div()
        .w(relative(0.25))
        .flex()
        .flex_col()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(chrome::panel(
            t,
            "Events",
            host.ctrl.focus == Focus::CourseEvents,
            body,
        ))
}

fn roster_list(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    let c = &host.course;
    let mut list = div().flex().flex_col().w_full();
    let mut rows = 0usize;
    for (i, entry) in host.roster.entries.iter().enumerate() {
        if entry.name.trim().is_empty() {
            continue;
        }
        let selected = c.roster_ix == Some(i);
        let stripe = rows % 2 == 1;
        rows += 1;
        list = list.child(
            div()
                .id(("course-roster-name", i as u64))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.select_course_roster(i);
                    this.ctrl.focus = Focus::CourseEvents;
                    cx.notify();
                }))
                .child(chrome::row(t, selected, false, false, stripe, entry.name.clone())),
        );
    }
    if rows == 0 {
        list = list.child(
            div()
                .px_2()
                .py_1()
                .text_color(rgb(t.muted))
                .child("No names in today's roster"),
        );
    }
    div()
        .id("course-roster-list")
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .overflow_y_scroll()
        .child(list)
}

fn script_list(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    let c = &host.course;
    let mut list = div().flex().flex_col().w_full();
    if c.scripts.is_empty() {
        list = list.child(
            div()
                .px_2()
                .py_1()
                .text_color(rgb(t.muted))
                .child("No stages"),
        );
    }
    for (i, n) in c.scripts.iter().enumerate() {
        let selected = i == c.ix;
        let label = stage_label(n);
        list = list.child(
            div()
                .id(("course-lua", i as u64))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.select_course_script(i);
                    this.ctrl.focus = Focus::CourseEvents;
                    cx.notify();
                }))
                .child(chrome::row(t, selected, false, false, i % 2 == 1, label)),
        );
    }
    div()
        .id("course-script-list")
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .overflow_y_scroll()
        .child(list)
}

enum GridCell {
    Empty,
    Distance(usize),
    Target(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DistOrient {
    Horizontal,
    Vertical,
}

const TRACK_TARGET: f32 = 4.0;
const TRACK_DISTANCE: f32 = 1.0;

fn grid_cell(targets: &[CourseTarget], row: u32, col: u32) -> GridCell {
    let mut distance = None;
    let mut target = None;
    for (i, c) in targets.iter().enumerate() {
        if c.row != Some(row) || c.col != Some(col) {
            continue;
        }
        if c.def.as_ref().is_some_and(|d| !d.is_empty()) {
            target = Some(i);
        } else if c.distance.is_some() {
            distance = Some(i);
        }
    }
    if let Some(i) = target {
        GridCell::Target(i)
    } else if let Some(i) = distance {
        GridCell::Distance(i)
    } else {
        GridCell::Empty
    }
}

fn is_target_cell(targets: &[CourseTarget], row: u32, col: u32) -> bool {
    matches!(grid_cell(targets, row, col), GridCell::Target(_))
}

fn col_grow(targets: &[CourseTarget], rows: u32, col: u32) -> f32 {
    if (1..=rows).any(|r| is_target_cell(targets, r, col)) {
        TRACK_TARGET
    } else {
        TRACK_DISTANCE
    }
}

fn row_grow(targets: &[CourseTarget], cols: u32, row: u32) -> f32 {
    if (1..=cols).any(|c| is_target_cell(targets, row, c)) {
        TRACK_TARGET
    } else {
        TRACK_DISTANCE
    }
}

fn distance_orient(targets: &[CourseTarget], rows: u32, cols: u32, row: u32, col: u32) -> DistOrient {
    let left = col > 1 && is_target_cell(targets, row, col - 1);
    let right = col < cols && is_target_cell(targets, row, col + 1);
    let above = row > 1 && is_target_cell(targets, row - 1, col);
    let below = row < rows && is_target_cell(targets, row + 1, col);
    let both_h = left && right;
    let both_v = above && below;
    if both_h {
        DistOrient::Horizontal
    } else if both_v {
        DistOrient::Vertical
    } else if left || right {
        DistOrient::Horizontal
    } else if above || below {
        DistOrient::Vertical
    } else {
        DistOrient::Horizontal
    }
}

fn flex_track(grow: f32) -> gpui::Div {
    let mut d = div().min_w(px(0.)).min_h(px(0.));
    d.style().flex_grow = Some(grow);
    d.style().flex_shrink = Some(1.0);
    d.style().flex_basis = Some(relative(0.).into());
    d
}

fn errors_table(t: crate::theme::Theme, size_px: f32, errors: &[String]) -> gpui::Div {
    let mut table = div()
        .flex()
        .flex_col()
        .w_full()
        .border_1()
        .border_color(rgb(t.border));
    for (i, msg) in errors.iter().enumerate() {
        let stripe = i % 2 == 1;
        let last = i + 1 == errors.len();
        let mut row = div()
            .w_full()
            .px_1()
            .py_0p5()
            .bg(rgb(chrome::row_bg(t, false, stripe)))
            .text_color(rgb(t.accent))
            .text_size(px(size_px * 1.25))
            .child(msg.clone());
        if !last {
            row = row.border_b_1().border_color(rgb(t.border));
        }
        table = table.child(row);
    }
    table
}

fn targets_panel(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let focused = host.ctrl.focus == Focus::CourseTargets;
    let errors = host.course_start_errors();
    let body = match (host.course.grid_rows, host.course.grid_cols) {
        (Some(rows), Some(cols)) if rows >= 1 && cols >= 1 => grid_targets(host, cx, rows, cols),
        _ => wrap_targets(host, cx),
    };
    let mut panel = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .border_1()
        .border_color(rgb(if focused { t.selection_bg } else { t.border }))
        .bg(rgb(t.background))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .py_0p5()
                .child(
                    div()
                        .text_color(rgb(if focused { t.accent } else { t.muted }))
                        .child(" TARGETS "),
                ),
        );
    if !errors.is_empty() {
        panel = panel.child(
            div()
                .w_full()
                .p_1()
                .child(errors_table(t, host.line_px(), &errors)),
        );
    }
    panel.child(body)
}

fn wrap_targets(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Stateful<gpui::Div> {
    let cards: Vec<_> = host
        .course
        .targets
        .iter()
        .enumerate()
        .map(|(i, card)| target_card(host, cx, i, card, false))
        .collect();
    div()
        .id("course-targets")
        .flex_1()
        .min_h(px(0.))
        .p_1()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_start()
        .overflow_y_scroll()
        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            this.ctrl.focus = Focus::CourseTargets;
            this.course.roster_drop = false;
            this.course.script_drop = false;
            this.course.hub_drop = None;
            cx.notify();
        }))
        .children(cards)
}

fn grid_targets(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    rows: u32,
    cols: u32,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    let targets = &host.course.targets;
    let col_grows: Vec<f32> = (1..=cols).map(|c| col_grow(targets, rows, c)).collect();
    let mut row_els = Vec::with_capacity(rows as usize);
    for row in 1..=rows {
        let rg = row_grow(targets, cols, row);
        let mut cells = Vec::with_capacity(cols as usize);
        for col in 1..=cols {
            let cg = col_grows[(col - 1) as usize];
            let inner = match grid_cell(targets, row, col) {
                GridCell::Empty => div()
                    .id(("course-cell", ((row as u64) << 32) | col as u64))
                    .size_full()
                    .into_any_element(),
                GridCell::Distance(i) => {
                    let text = targets[i].distance.clone().unwrap_or_default();
                    let orient = distance_orient(targets, rows, cols, row, col);
                    distance_card(t, i, text, orient).into_any_element()
                }
                GridCell::Target(i) => {
                    let card = &host.course.targets[i];
                    target_card(host, cx, i, card, true).into_any_element()
                }
            };
            cells.push(
                flex_track(cg)
                    .h_full()
                    .overflow_hidden()
                    .child(inner)
                    .into_any_element(),
            );
        }
        row_els.push(
            flex_track(rg)
                .w_full()
                .flex()
                .flex_row()
                .overflow_hidden()
                .children(cells)
                .into_any_element(),
        );
    }
    div()
        .id("course-targets")
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .p_1()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            this.ctrl.focus = Focus::CourseTargets;
            this.course.roster_drop = false;
            this.course.script_drop = false;
            this.course.hub_drop = None;
            cx.notify();
        }))
        .children(row_els)
}

fn paint_h_dim_arrow(bounds: Bounds<Pixels>, color: u32, window: &mut Window) {
    let pad = px(4.);
    let y = bounds.center().y;
    let x0 = bounds.left() + pad;
    let x1 = bounds.right() - pad;
    if x1 <= x0 {
        return;
    }
    let tick = px(6.);
    let head = px(8.);
    let mut st = PathBuilder::stroke(px(1.5));
    // left tick |
    st.move_to(point(x0, y - tick));
    st.line_to(point(x0, y + tick));
    // left arrow <-
    st.move_to(point(x0 + head, y - tick * 0.6));
    st.line_to(point(x0, y));
    st.line_to(point(x0 + head, y + tick * 0.6));
    // shaft
    st.move_to(point(x0, y));
    st.line_to(point(x1, y));
    // right arrow ->
    st.move_to(point(x1 - head, y - tick * 0.6));
    st.line_to(point(x1, y));
    st.line_to(point(x1 - head, y + tick * 0.6));
    // right tick |
    st.move_to(point(x1, y - tick));
    st.line_to(point(x1, y + tick));
    if let Ok(path) = st.build() {
        window.paint_path(path, rgb(color));
    }
}

fn paint_v_dim_arrow(bounds: Bounds<Pixels>, color: u32, window: &mut Window) {
    let pad = px(4.);
    let x = bounds.center().x;
    let y0 = bounds.top() + pad;
    let y1 = bounds.bottom() - pad;
    if y1 <= y0 {
        return;
    }
    let tick = px(6.);
    let head = px(8.);
    let mut st = PathBuilder::stroke(px(1.5));
    // top tick ─ (┬ bar)
    st.move_to(point(x - tick, y0));
    st.line_to(point(x + tick, y0));
    // top arrow ↑
    st.move_to(point(x - tick * 0.6, y0 + head));
    st.line_to(point(x, y0));
    st.line_to(point(x + tick * 0.6, y0 + head));
    // shaft
    st.move_to(point(x, y0));
    st.line_to(point(x, y1));
    // bottom arrow ↓
    st.move_to(point(x - tick * 0.6, y1 - head));
    st.line_to(point(x, y1));
    st.line_to(point(x + tick * 0.6, y1 - head));
    // bottom tick ─ (┴ bar)
    st.move_to(point(x - tick, y1));
    st.line_to(point(x + tick, y1));
    if let Ok(path) = st.build() {
        window.paint_path(path, rgb(color));
    }
}

fn distance_card(
    t: crate::theme::Theme,
    i: usize,
    text: String,
    orient: DistOrient,
) -> gpui::Stateful<gpui::Div> {
    let color = t.foreground;
    let arrow = match orient {
        DistOrient::Horizontal => canvas(
            |b, _, _| b,
            move |_, b, window, _| paint_h_dim_arrow(b, color, window),
        )
        .w_full()
        .h(px(16.))
        .into_any_element(),
        DistOrient::Vertical => canvas(
            |b, _, _| b,
            move |_, b, window, _| paint_v_dim_arrow(b, color, window),
        )
        .w(px(16.))
        .h_full()
        .into_any_element(),
    };
    let label = div()
        .text_color(rgb(t.foreground))
        .whitespace_nowrap()
        .child(text);
    let body = match orient {
        DistOrient::Horizontal => div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_0p5()
            .size_full()
            .child(label)
            .child(arrow),
        DistOrient::Vertical => div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .size_full()
            .child(arrow)
            .child(label),
    };
    div()
        .id(("course-dist", i as u64))
        .size_full()
        .overflow_hidden()
        .child(body)
}

fn target_card(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    i: usize,
    card: &CourseTarget,
    fill_cell: bool,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    let hub_b = host.course.hub_b.clone();
    let editing = host.course.name_edit.as_ref().is_some_and(|(j, _)| *j == i);
    let name = if editing {
        format!(
            "{}▏",
            host.course
                .name_edit
                .as_ref()
                .map(|(_, b)| b.as_str())
                .unwrap_or("")
        )
    } else {
        card.name.clone()
    };
    let def_label = card.def.clone().unwrap_or_else(|| "—".into());
    let hub_label = card
        .hub_label
        .clone()
        .unwrap_or_else(|| "Select sensor hub".into());
    let hub_open = host.course.hub_drop == Some(i);
    let measure_hub = hub_open;
    let mut d = div()
        .id(("course-tgt", i as u64))
        .flex()
        .flex_col()
        .gap_1()
        .p(px(4.))
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.surface));
    if fill_cell {
        d = d.size_full().min_w(px(0.)).overflow_hidden();
    } else {
        d = d.m(px(2.)).min_w(px(286.));
    }
    d.child(
        div()
            .flex()
            .flex_row()
            .gap_1()
            .items_center()
            .min_w(px(0.))
            .w_full()
            .overflow_hidden()
            .child(
                div()
                    .id(("course-nm", i as u64))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        this.start_course_rename(i);
                        this.ctrl.focus = Focus::CourseTargets;
                        this.course.hub_drop = None;
                        this.course.roster_drop = false;
                        this.course.script_drop = false;
                        cx.notify();
                    }))
                    .text_color(rgb(t.foreground))
                    .child(name),
            )
            .child(
                div()
                    .id(("course-def", i as u64))
                    .flex_none()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(t.muted))
                    .child(def_label),
            )
            .child(
                div()
                    .id(("course-hub", i as u64))
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w(px(80.))
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .px_1()
                    .border_1()
                    .border_color(rgb(if hub_open { t.selection_bg } else { t.border }))
                    .bg(rgb(t.background))
                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        this.ctrl.focus = Focus::CourseTargets;
                        this.course.roster_drop = false;
                        this.course.script_drop = false;
                        this.course.hub_drop = if this.course.hub_drop == Some(i) {
                            None
                        } else {
                            Some(i)
                        };
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .when(measure_hub, |d| {
                        d.child(
                            canvas(
                                move |b, _, _| {
                                    hub_b.set(b);
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(t.foreground))
                            .child(hub_label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(t.muted))
                            .child(if hub_open { "▴" } else { "▾" }),
                    ),
            ),
    )
    .child({
        let mut mid = div()
            .flex()
            .flex_row()
            .gap_1()
            .items_start()
            .min_w(px(0.))
            .w_full();
        if fill_cell {
            mid = mid.flex_1().min_h(px(0.)).overflow_hidden();
        }
        mid.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .when(fill_cell, |d| d.h_full())
                .child(preview_image(host, cx, i, card, fill_cell)),
        )
        .child(poly_list(t, card))
    })
}

fn preview_image(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    i: usize,
    card: &CourseTarget,
    fill_cell: bool,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    let img_w = card.img_w.clone();
    let img_bounds = card.img_bounds.clone();
    let measured = card.img_w.get();
    let width = if measured > 1.0 { measured } else { 286.0 };
    let h = match card.img_size {
        Some((iw, ih)) if iw > 0 => width * (ih as f32 / iw as f32),
        _ => 80.0,
    };
    let hits = if host.course.running {
        card.hits.clone()
    } else {
        Vec::new()
    };
    let bullet = host.settings.bullet_size_px();
    let img_size = card.img_size;
    let overlay = canvas(
        move |b, _, _| {
            img_w.set(f32::from(b.size.width));
            img_bounds.set(b);
            b
        },
        move |_, b, window, _| {
            let Some(img) = img_size else {
                return;
            };
            let (ox, oy, s) = crate::host::fit(b, img);
            if s <= 1e-6 {
                return;
            }
            let r = px(bullet / 2.0);
            let d = px(bullet);
            for &(x, y) in &hits {
                let c = point(px(ox + x * s), px(oy + y * s));
                window.paint_quad(
                    fill(
                        Bounds::new(point(c.x - r, c.y - r), size(d, d)),
                        rgb(0xff0000),
                    )
                    .corner_radii(r)
                    .border_widths(px(1.))
                    .border_color(rgb(0xffffff)),
                );
            }
        },
    )
    .absolute()
    .size_full();
    let mut box_ = div()
        .id(("course-preview", i as u64))
        .w_full()
        .overflow_hidden()
        .relative()
        .bg(rgb(t.background))
        .border_1()
        .border_color(rgb(t.border))
        .flex()
        .items_center()
        .justify_center()
        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
            this.ctrl.focus = Focus::CourseTargets;
            this.course.roster_drop = false;
            this.course.script_drop = false;
            this.course.hub_drop = None;
            if let Some(pos) = ev.mouse_position() {
                this.simulate_course_hit(i, pos);
            }
            cx.stop_propagation();
            cx.notify();
        }));
    box_ = if fill_cell {
        box_.h_full().min_h(px(0.))
    } else {
        box_.h(px(h.max(1.0)))
    };
    if let Some(im) = &card.image {
        box_.child(
            img(im.clone())
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .child(overlay)
    } else {
        let msg = card
            .err
            .clone()
            .unwrap_or_else(|| "No target selected".into());
        box_.child(div().p_1().text_color(rgb(t.muted)).child(msg))
            .child(overlay)
    }
}

fn poly_list(t: crate::theme::Theme, card: &CourseTarget) -> gpui::Div {
    let mut list = div().flex().flex_col().flex_none();
    if card.def.is_none() {
        return list;
    }
    if card.zones.is_empty() {
        return list.child(
            div()
                .px_1()
                .whitespace_nowrap()
                .text_color(rgb(t.muted))
                .child("No zones"),
        );
    }
    for (i, n) in card.zones.iter().enumerate() {
        let bg = chrome::row_bg(t, false, i % 2 == 1);
        list = list.child(
            div()
                .px_1()
                .whitespace_nowrap()
                .text_color(rgb(t.foreground))
                .bg(rgb(bg))
                .child(n.clone()),
        );
    }
    list
}

fn hub_list(host: &Host, cx: &mut gpui::Context<Host>, card: usize) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    let hubs = host.course_hubs();
    let mut list = div().flex().flex_col().w_full();
    if hubs.is_empty() {
        list = list.child(
            div()
                .px_2()
                .py_1()
                .text_color(rgb(t.muted))
                .child("No sensor hubs"),
        );
    }
    let selected = host.course.targets.get(card).and_then(|c| c.hub.clone());
    for (i, (label, key)) in hubs.into_iter().enumerate() {
        let on = selected.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&key));
        let key_c = key.clone();
        let label_c = label.clone();
        let (fg, bg) = if on {
            (t.background, t.cursor)
        } else {
            (t.foreground, t.background)
        };
        list = list.child(
            div()
                .id(("course-hub-item", i as u64))
                .w_full()
                .px_1()
                .text_color(rgb(fg))
                .bg(rgb(bg))
                .hover(|s| s.bg(rgb(t.cursor)).text_color(rgb(t.background)))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    let name = this.course.targets.get_mut(card).map(|t| {
                        t.hub = Some(key_c.clone());
                        t.hub_label = Some(label_c.clone());
                        t.name.clone()
                    });
                    if let Some(name) = name {
                        this.remember_course_hub(&name, key_c.clone(), label_c.clone());
                    }
                    this.course.hub_drop = None;
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(label),
        );
    }
    let view = host.course.view_b.get();
    let btn = host.course.hub_b.get();
    let top = f32::from(btn.bottom() - view.origin.y);
    let left = f32::from(btn.origin.x - view.origin.x);
    let w = f32::from(btn.size.width).max(200.0);
    div()
        .id("course-hub-list")
        .absolute()
        .top(px(top.max(0.0)))
        .left(px(left.max(0.0)))
        .min_w(px(w))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.background))
        .occlude()
        .child(list)
}

pub fn push_event(course: &mut Course, line: String) {
    let v = match serde_json::from_str::<serde_json::Value>(&line) {
        Ok(v) if v.is_object() || v.is_array() => v,
        _ if is_script_lifecycle(&line) => return,
        _ => serde_json::Value::String(line),
    };
    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
    match course.events.get_mut(&ts) {
        None => {
            course.events.insert(ts, v);
        }
        Some(serde_json::Value::Array(a)) => {
            a.push(v);
        }
        Some(existing) => {
            let prev = std::mem::replace(existing, serde_json::Value::Null);
            *existing = serde_json::Value::Array(vec![prev, v]);
        }
    }
    while course.events.len() > EVENT_CAP {
        if let Some(key) = course.events.keys().next().cloned() {
            course.events.shift_remove(&key);
        } else {
            break;
        }
    }
}

fn is_script_lifecycle(line: &str) -> bool {
    line.starts_with("script: running ")
        || line.starts_with("script: done ")
        || line.ends_with(": stopped")
}

fn time_sep(t: crate::theme::Theme, ts: &str) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .justify_center()
        .text_color(rgb(t.muted))
        .child(format!("--- {ts} ---"))
}

fn json_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "—".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn is_prim(v: &serde_json::Value) -> bool {
    !v.is_object() && !v.is_array()
}

struct KvSpec {
    label: String,
    value: String,
}

fn join_prims(a: &[serde_json::Value]) -> String {
    a.iter().map(json_cell).collect::<Vec<_>>().join(", ")
}

fn take_label(specs: &mut Vec<KvSpec>, key: &str) -> Option<KvSpec> {
    specs
        .iter()
        .position(|s| s.label.eq_ignore_ascii_case(key))
        .map(|i| specs.remove(i))
}

fn promote_name_title(mut specs: Vec<KvSpec>) -> Vec<KvSpec> {
    let name = take_label(&mut specs, "name");
    let title = take_label(&mut specs, "title");
    let rest = specs;
    let mut out = Vec::with_capacity(rest.len() + 2);
    out.extend(name);
    out.extend(title);
    out.extend(rest);
    out
}

fn specs_for_value(v: &serde_json::Value) -> Vec<KvSpec> {
    match v {
        serde_json::Value::Object(map) => {
            let mut out = Vec::new();
            for (k, val) in map {
                out.extend(specs_for_field(k, val));
            }
            promote_name_title(out)
        }
        serde_json::Value::Array(a) if a.iter().all(is_prim) => vec![KvSpec {
            label: "—".into(),
            value: join_prims(a),
        }],
        _ => vec![KvSpec {
            label: "—".into(),
            value: json_cell(v),
        }],
    }
}

fn specs_for_field(k: &str, v: &serde_json::Value) -> Vec<KvSpec> {
    match v {
        v if is_prim(v) => vec![KvSpec {
            label: k.to_string(),
            value: json_cell(v),
        }],
        serde_json::Value::Array(a) if a.iter().all(is_prim) => vec![KvSpec {
            label: k.to_string(),
            value: join_prims(a),
        }],
        serde_json::Value::Object(inner) if inner.values().all(is_prim) => inner
            .iter()
            .map(|(ck, cv)| KvSpec {
                label: format!("{k}.{ck}"),
                value: json_cell(cv),
            })
            .collect(),
        _ => vec![KvSpec {
            label: k.to_string(),
            value: v.to_string(),
        }],
    }
}

fn is_target_hit(v: &serde_json::Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("target_hit")
}

fn field_text(v: &serde_json::Value, key: &str) -> String {
    v.get(key).map(json_cell).unwrap_or_else(|| "—".into())
}

fn event_card_shell(t: crate::theme::Theme, inner: gpui::Div) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p(px(4.))
        .m(px(2.))
        .min_w(px(286.))
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.surface))
        .text_color(rgb(t.foreground))
        .child(inner)
}

fn ms_tail(v: &serde_json::Value) -> String {
    let raw = match v.get("ms") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => json_cell(other),
        None => return "—".into(),
    };
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return "—".into();
    }
    let start = digits.len().saturating_sub(5);
    digits[start..].to_string()
}

fn target_hit_card(t: crate::theme::Theme, size_px: f32, hits: &[&serde_json::Value]) -> gpui::Div {
    let n = hits.len();
    let mut inner = div().border_1().border_color(rgb(t.border)).flex().flex_col();
    for (i, v) in hits.iter().enumerate() {
        inner = inner.child(chrome::grid_row(
            t,
            t.foreground,
            i + 1 == n,
            false,
            size_px,
            [ms_tail(v), field_text(v, "target"), field_text(v, "zone")],
        ));
    }
    event_card_shell(t, inner)
}

fn message_card(t: crate::theme::Theme, size_px: f32, v: &serde_json::Value) -> gpui::Div {
    let specs = specs_for_value(v);
    let n = specs.len();
    let mut inner = div().border_1().border_color(rgb(t.border)).flex().flex_col();
    if specs.is_empty() {
        inner = inner.child(chrome::kv_row(
            t,
            t.foreground,
            true,
            size_px,
            "—".into(),
            "—".into(),
        ));
    } else {
        for (i, spec) in specs.into_iter().enumerate() {
            inner = inner.child(chrome::kv_row(
                t,
                t.foreground,
                i + 1 == n,
                size_px,
                spec.label,
                spec.value,
            ));
        }
    }
    event_card_shell(t, inner)
}
