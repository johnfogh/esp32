//! Demo application for guilib: View → Panel → Card, plus Button, TimeLine, TimelineEvents.
use gpui::{
    App, Application, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use guilib::{
    format_time_compact, Button, Card, Connector, DesignGrid, Panel, Theme, TimeLine,
    TimelineEvents, View, install_system_style, system_style, TICKS_PER_SEC,
};

actions!(demo, [TimelineLeft, TimelineRight, TimelineHome, TimelineEnd]);

struct Demo {
    timeline: Entity<TimeLine>,
    btn_build: Entity<Button>,
    btn_flash: Entity<Button>,
    btn_run: Entity<Button>,
    btn_armed: Entity<Button>,
    focus_handle: FocusHandle,
}

impl Demo {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Start the playhead at 10s so the twin-line cursor is visible mid-track.
        // Seed event marks every 5s across the default 30s duration.
        let event_ticks: Vec<_> = (0..=6).map(|i| i * 5 * TICKS_PER_SEC).collect();
        let timeline = cx.new(|cx| {
            TimeLine::new(cx)
                .cursor(10 * TICKS_PER_SEC)
                .events(event_ticks)
        });
        cx.observe(&timeline, |_, _, cx| cx.notify()).detach();
        let btn_build = cx.new(|cx| Button::new("Build", cx));
        let btn_flash = cx.new(|cx| Button::new("Flash", cx));
        let btn_run = cx.new(|cx| Button::new("Run", cx));
        let btn_armed = cx.new(|cx| Button::new("Armed", cx).toggleable(true));
        let focus_handle = cx.focus_handle();
        let tl_handle = timeline.read(cx).focus_handle(cx);
        // Focus after paint: timeline first (own key_context), demo root as fallback.
        window.on_next_frame({
            let tl_handle = tl_handle.clone();
            let focus_handle = focus_handle.clone();
            move |window, _cx| {
                if !tl_handle.is_focused(window) {
                    window.focus(&tl_handle);
                }
                if !tl_handle.is_focused(window) {
                    window.focus(&focus_handle);
                }
            }
        });
        Self {
            timeline,
            btn_build,
            btn_flash,
            btn_run,
            btn_armed,
            focus_handle,
        }
    }

    fn timeline_left(&mut self, _: &TimelineLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.timeline.update(cx, |tl, cx| tl.nudge_left(cx));
    }

    fn timeline_right(&mut self, _: &TimelineRight, _: &mut Window, cx: &mut Context<Self>) {
        self.timeline.update(cx, |tl, cx| tl.nudge_right(cx));
    }

    fn timeline_home(&mut self, _: &TimelineHome, _: &mut Window, cx: &mut Context<Self>) {
        self.timeline
            .update(cx, |tl, cx| tl.set_cursor_ticks(0, cx));
    }

    fn timeline_end(&mut self, _: &TimelineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.timeline.update(cx, |tl, cx| {
            let end = tl.duration_ticks();
            tl.set_cursor_ticks(end, cx);
        });
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let mods = &ev.keystroke.modifiers;
        match key {
            "left" | "right" | "home" | "end" | "space" | "delete" => {
                // Prefer TimeLine's own handlers when it has focus; these are a fallback.
                self.timeline.update(cx, |tl, cx| match key {
                    "left" if mods.control => {
                        tl.set_cursor_ticks(
                            tl.cursor_ticks().saturating_sub(guilib::FINE_STEP),
                            cx,
                        )
                    }
                    "right" if mods.control => {
                        tl.set_cursor_ticks(
                            tl.cursor_ticks().saturating_add(guilib::FINE_STEP),
                            cx,
                        )
                    }
                    "left" if mods.shift => {
                        tl.set_cursor_ticks(
                            tl.cursor_ticks().saturating_sub(guilib::COARSE_STEP),
                            cx,
                        )
                    }
                    "right" if mods.shift => {
                        tl.set_cursor_ticks(
                            tl.cursor_ticks().saturating_add(guilib::COARSE_STEP),
                            cx,
                        )
                    }
                    "left" => tl.nudge_left(cx),
                    "right" => tl.nudge_right(cx),
                    "home" => tl.set_cursor_ticks(0, cx),
                    "end" => {
                        let end = tl.duration_ticks();
                        tl.set_cursor_ticks(end, cx);
                    }
                    "space" => tl.add_event_at_cursor(cx),
                    "delete" => tl.remove_event_at_cursor(cx),
                    _ => {}
                });
            }
            _ => {}
        }
    }
}

impl Focusable for Demo {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title("guilib demo");
        let t = system_style(cx).theme;

        let cursor_label = format_time_compact(self.timeline.read(cx).cursor_ticks());
        let end_label = format_time_compact(self.timeline.read(cx).duration_ticks());

        div()
            .id("demo")
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::timeline_left))
            .on_action(cx.listener(Self::timeline_right))
            .on_action(cx.listener(Self::timeline_home))
            .on_action(cx.listener(Self::timeline_end))
            .on_key_down(cx.listener(Self::on_key))
            .child(
                View::new()
                    .gap(px(12.))
                    .padding(px(12.))
                    .child(
                        Panel::new()
                            .title("Top panel")
                            .grow(false)
                            .child(
                                Card::new()
                                    .title("Status")
                                    .grow(false)
                                    .child(
                                        div()
                                            .text_color(rgb(t.foreground))
                                            .child("View → Panel → Card hierarchy"),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(t.muted))
                                            .child(
                                                "Theme and font follow the Omarchy system defaults.",
                                            ),
                                    ),
                            )
                            .child(
                                Card::new()
                                    .title("Actions")
                                    .grow(false)
                                    .flex_row()
                                    .child(self.btn_build.clone())
                                    .child(self.btn_flash.clone())
                                    .child(self.btn_run.clone())
                                    .child(self.btn_armed.clone()),
                            ),
                    )
                    .child(
                        Panel::new().title("Design grid").child(
                            DesignGrid::new()
                                .card(
                                    0,
                                    0,
                                    Card::new().title("Sidebar").child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(row_item(t, "Devices"))
                                            .child(row_item(t, "Targets"))
                                            .child(row_item(t, "Reports")),
                                    ),
                                )
                                .connector(1, 0, Connector::new())
                                .card(
                                    2,
                                    0,
                                    Card::new()
                                        .title("Content")
                                        .child(
                                            div()
                                                .text_color(rgb(t.foreground))
                                                .child("Main content area"),
                                        )
                                        .child(
                                            div().mt_2().text_color(rgb(t.muted)).child(
                                                "Panels are flexboxes inside a View. Cards are flexboxes inside a Panel.",
                                            ),
                                        )
                                        .child(
                                            div().mt_2().text_color(rgb(t.accent)).child(format!(
                                                "TimeLine cursor {cursor_label} / {end_label}"
                                            )),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        Panel::new()
                            .title("Timeline panel")
                            .flex_col()
                            .equal_children(false)
                            .grow(false)
                            .child(
                                TimelineEvents::new(self.timeline.clone())
                                    .title("Timeline events")
                                    .border(px(0.))
                                    .grow(false),
                            )
                            .child(
                                Card::new()
                                    .grow(false)
                                    .child(self.timeline.clone()),
                            ),
                    ),
            )
    }
}

fn row_item(t: Theme, label: &str) -> impl IntoElement {
    div()
        .w_full()
        .px_2()
        .py_1()
        .rounded_sm()
        .hover(|d| d.bg(rgb(t.border)))
        .text_color(rgb(t.foreground))
        .child(label.to_string())
}

fn main() {
    Application::new().run(|cx: &mut App| {
        install_system_style(cx);
        TimeLine::bind_keys(cx);
        // Demo-level bindings (no key context) so arrows work while the window is focused.
        cx.bind_keys([
            KeyBinding::new("left", TimelineLeft, None),
            KeyBinding::new("right", TimelineRight, None),
            KeyBinding::new("home", TimelineHome, None),
            KeyBinding::new("end", TimelineEnd, None),
        ]);
        let bounds = Bounds::centered(None, size(px(960.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("guilib-demo".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Demo::new(window, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
