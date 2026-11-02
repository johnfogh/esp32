//! Horizontal timeline control with a movable playhead cursor.

use gpui::{
    actions, canvas, div, prelude::*, px, relative, rgb, App, Bounds, Context, FocusHandle,
    Focusable, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, SharedString, Window,
};

use crate::Theme;
use crate::system::resolve_theme;

actions!(
    timeline,
    [
        MoveLeft,
        MoveRight,
        NudgeFineLeft,
        NudgeFineRight,
        NudgeCoarseLeft,
        NudgeCoarseRight,
        JumpStart,
        JumpEnd,
        AddEvent,
        RemoveEvent,
        ConfirmEdit,
        CancelEdit
    ]
);

/// One tick = 0.0001 seconds (four fractional digits in `MM:SS.ffff`).
pub type TimeTicks = u64;

/// Ticks in one second.
pub const TICKS_PER_SEC: TimeTicks = 10_000;

/// Default end time: 30 seconds.
pub const DEFAULT_DURATION: TimeTicks = 30 * TICKS_PER_SEC;

/// Legacy default step (0.1 s). Prefer [`FINE_STEP`] / [`COARSE_STEP`].
pub const DEFAULT_STEP: TimeTicks = TICKS_PER_SEC / 10;

/// Ctrl+arrow step: 0.01 seconds (`00:00.0100`).
pub const FINE_STEP: TimeTicks = 100;

/// Shift+arrow step: 1 second (`00:01.0000`).
pub const COARSE_STEP: TimeTicks = TICKS_PER_SEC;

/// Format ticks as `MM:SS.ffff`.
pub fn format_time(ticks: TimeTicks) -> String {
    let total_secs = ticks / TICKS_PER_SEC;
    let frac = ticks % TICKS_PER_SEC;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins:02}:{secs:02}.{frac:04}")
}

/// Compact display form: drop zero minutes, trim trailing fractional zeros.
///
/// Smallest value is `:00.0`. Examples: `:05.0`, `1:30.12`.
pub fn format_time_compact(ticks: TimeTicks) -> String {
    let total_secs = ticks / TICKS_PER_SEC;
    let frac = ticks % TICKS_PER_SEC;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    let frac_raw = format!("{frac:04}");
    let frac_trimmed = frac_raw.trim_end_matches('0');
    let frac_display = if frac_trimmed.is_empty() {
        "0"
    } else {
        frac_trimmed
    };
    if mins == 0 {
        format!(":{secs:02}.{frac_display}")
    } else {
        format!("{mins}:{secs:02}.{frac_display}")
    }
}

/// Parse `MM:SS.ffff`, `:SS.ffff`, `SS.ffff`, or bare seconds into ticks.
pub fn parse_time(s: &str) -> Option<TimeTicks> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (min_part, rest) = match s.split_once(':') {
        Some((m, r)) => (Some(m), r),
        None => (None, s),
    };
    let mins: u64 = match min_part {
        Some(m) if m.is_empty() => 0,
        Some(m) => m.parse().ok()?,
        None => 0,
    };
    let (secs_str, frac_str) = match rest.split_once('.') {
        Some((a, b)) => (a, b),
        None => (rest, "0"),
    };
    let secs: u64 = secs_str.parse().ok()?;
    if secs >= 60 && min_part.is_some() {
        return None;
    }
    // Pad / trim fractional part to 4 digits.
    let frac = if frac_str.is_empty() {
        0
    } else {
        let digits: String = frac_str.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        let padded = if digits.len() >= 4 {
            digits[..4].to_string()
        } else {
            format!("{digits:0<4}")
        };
        padded.parse::<u64>().ok()?
    };
    Some(mins * 60 * TICKS_PER_SEC + secs * TICKS_PER_SEC + frac)
}

/// Horizontal timeline: capped track, playhead cursor, editable end time, events.
pub struct TimeLine {
    focus_handle: FocusHandle,
    theme: Option<Theme>,
    /// End of the timeline (inclusive upper bound for the cursor).
    duration: TimeTicks,
    /// Playhead position in `[0, duration]`.
    cursor: TimeTicks,
    /// Arrow-key nudge amount (legacy; plain arrows jump events).
    step: TimeTicks,
    /// Sorted unique event positions in ticks.
    events: Vec<TimeTicks>,
    /// True while the user is dragging the playhead on the track.
    dragging: bool,
    /// When set, the end label is being edited.
    end_edit: Option<String>,
}

impl TimeLine {
    /// Create a timeline ending at [`DEFAULT_DURATION`] with the cursor at the start.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            theme: None,
            duration: DEFAULT_DURATION,
            cursor: 0,
            step: DEFAULT_STEP,
            events: Vec::new(),
            dragging: false,
            end_edit: None,
        }
    }

    /// Register key bindings (call once at app startup).
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("left", MoveLeft, Some("TimeLine")),
            KeyBinding::new("right", MoveRight, Some("TimeLine")),
            KeyBinding::new("ctrl-left", NudgeFineLeft, Some("TimeLine")),
            KeyBinding::new("ctrl-right", NudgeFineRight, Some("TimeLine")),
            KeyBinding::new("shift-left", NudgeCoarseLeft, Some("TimeLine")),
            KeyBinding::new("shift-right", NudgeCoarseRight, Some("TimeLine")),
            KeyBinding::new("home", JumpStart, Some("TimeLine")),
            KeyBinding::new("end", JumpEnd, Some("TimeLine")),
            KeyBinding::new("space", AddEvent, Some("TimeLine")),
            KeyBinding::new("delete", RemoveEvent, Some("TimeLine")),
            KeyBinding::new("enter", ConfirmEdit, Some("TimeLine")),
            KeyBinding::new("escape", CancelEdit, Some("TimeLine")),
        ]);
    }

    /// Override the colour palette.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Set the end time in ticks.
    pub fn duration(mut self, duration: TimeTicks) -> Self {
        self.duration = duration.max(1);
        self.cursor = self.cursor.min(self.duration);
        self.events.retain(|&t| t <= self.duration);
        self
    }

    /// Set the end time from a `MM:SS.ffff` string (ignores parse failures).
    pub fn duration_str(mut self, s: &str) -> Self {
        if let Some(t) = parse_time(s) {
            self.duration = t.max(1);
            self.cursor = self.cursor.min(self.duration);
            self.events.retain(|&e| e <= self.duration);
        }
        self
    }

    /// Set the playhead position in ticks (clamped to the duration).
    pub fn cursor(mut self, cursor: TimeTicks) -> Self {
        self.cursor = cursor.min(self.duration);
        self
    }

    /// Arrow-key step in ticks (legacy API).
    pub fn step(mut self, step: TimeTicks) -> Self {
        self.step = step.max(1);
        self
    }

    /// Current legacy step in ticks.
    pub fn step_ticks(&self) -> TimeTicks {
        self.step
    }

    /// Replace the event list (sorted + unique + clamped).
    pub fn events(mut self, events: impl IntoIterator<Item = TimeTicks>) -> Self {
        self.events = normalize_events(events, self.duration);
        self
    }

    /// Current end time in ticks.
    pub fn duration_ticks(&self) -> TimeTicks {
        self.duration
    }

    /// Current playhead position in ticks.
    pub fn cursor_ticks(&self) -> TimeTicks {
        self.cursor
    }

    /// Current events (sorted).
    pub fn events_ticks(&self) -> &[TimeTicks] {
        &self.events
    }

    /// Move the playhead to `cursor` (clamped) and redraw.
    pub fn set_cursor_ticks(&mut self, cursor: TimeTicks, cx: &mut Context<Self>) {
        self.set_cursor(cursor, cx);
    }

    /// Jump to the previous event, or to the start if none.
    pub fn nudge_left(&mut self, cx: &mut Context<Self>) {
        self.jump_prev_event(cx);
    }

    /// Jump to the next event, or to the end if none.
    pub fn nudge_right(&mut self, cx: &mut Context<Self>) {
        self.jump_next_event(cx);
    }

    /// Add an event at the current cursor (no-op if already present).
    pub fn add_event_at_cursor(&mut self, cx: &mut Context<Self>) {
        let t = self.cursor.min(self.duration);
        if let Err(idx) = self.events.binary_search(&t) {
            self.events.insert(idx, t);
            cx.notify();
        }
    }

    /// Remove the event exactly at the current cursor, if any.
    pub fn remove_event_at_cursor(&mut self, cx: &mut Context<Self>) {
        if let Ok(idx) = self.events.binary_search(&self.cursor) {
            self.events.remove(idx);
            cx.notify();
        }
    }

    fn set_cursor(&mut self, cursor: TimeTicks, cx: &mut Context<Self>) {
        let next = cursor.min(self.duration);
        if next != self.cursor {
            self.cursor = next;
            cx.notify();
        }
    }

    fn set_duration(&mut self, duration: TimeTicks, cx: &mut Context<Self>) {
        let duration = duration.max(1);
        if duration != self.duration {
            self.duration = duration;
            self.cursor = self.cursor.min(self.duration);
            self.events.retain(|&t| t <= self.duration);
            cx.notify();
        }
    }

    fn jump_prev_event(&mut self, cx: &mut Context<Self>) {
        let prev = self
            .events
            .iter()
            .rev()
            .copied()
            .find(|&t| t < self.cursor);
        self.set_cursor(prev.unwrap_or(0), cx);
    }

    fn jump_next_event(&mut self, cx: &mut Context<Self>) {
        let next = self.events.iter().copied().find(|&t| t > self.cursor);
        self.set_cursor(next.unwrap_or(self.duration), cx);
    }

    fn nudge_by(&mut self, delta: i64, cx: &mut Context<Self>) {
        let next = if delta < 0 {
            self.cursor.saturating_sub((-delta) as TimeTicks)
        } else {
            self.cursor.saturating_add(delta as TimeTicks)
        };
        self.set_cursor(next, cx);
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.jump_prev_event(cx);
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.jump_next_event(cx);
    }

    fn nudge_fine_left(&mut self, _: &NudgeFineLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.nudge_by(-(FINE_STEP as i64), cx);
    }

    fn nudge_fine_right(&mut self, _: &NudgeFineRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.nudge_by(FINE_STEP as i64, cx);
    }

    fn nudge_coarse_left(&mut self, _: &NudgeCoarseLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.nudge_by(-(COARSE_STEP as i64), cx);
    }

    fn nudge_coarse_right(&mut self, _: &NudgeCoarseRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.nudge_by(COARSE_STEP as i64, cx);
    }

    fn jump_start(&mut self, _: &JumpStart, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.set_cursor(0, cx);
    }

    fn jump_end(&mut self, _: &JumpEnd, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.set_cursor(self.duration, cx);
    }

    fn add_event_action(&mut self, _: &AddEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.add_event_at_cursor(cx);
    }

    fn remove_event_action(&mut self, _: &RemoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            return;
        }
        self.remove_event_at_cursor(cx);
    }

    fn confirm_edit(&mut self, _: &ConfirmEdit, _: &mut Window, cx: &mut Context<Self>) {
        self.commit_end_edit(cx);
    }

    fn cancel_edit(&mut self, _: &CancelEdit, _: &mut Window, cx: &mut Context<Self>) {
        if self.end_edit.take().is_some() {
            cx.notify();
        }
    }

    fn commit_end_edit(&mut self, cx: &mut Context<Self>) {
        let Some(buf) = self.end_edit.take() else {
            return;
        };
        if let Some(t) = parse_time(&buf) {
            self.set_duration(t, cx);
        }
        cx.notify();
    }

    fn begin_end_edit(&mut self, cx: &mut Context<Self>) {
        self.end_edit = Some(format_time(self.duration));
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let mods = &ev.keystroke.modifiers;

        if self.end_edit.is_some() {
            let buf = self.end_edit.as_mut().unwrap();
            if key == "backspace" {
                buf.pop();
                cx.notify();
                cx.stop_propagation();
                return;
            }
            if let Some(ch) = ev.keystroke.key_char.as_deref() {
                for c in ch.chars() {
                    if c.is_ascii_digit() || c == ':' || c == '.' {
                        buf.push(c);
                    }
                }
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        // Direct key handling so shortcuts work even if action bindings are missing.
        match key {
            "left" => {
                if mods.control {
                    self.nudge_by(-(FINE_STEP as i64), cx);
                } else if mods.shift {
                    self.nudge_by(-(COARSE_STEP as i64), cx);
                } else {
                    self.jump_prev_event(cx);
                }
                cx.stop_propagation();
            }
            "right" => {
                if mods.control {
                    self.nudge_by(FINE_STEP as i64, cx);
                } else if mods.shift {
                    self.nudge_by(COARSE_STEP as i64, cx);
                } else {
                    self.jump_next_event(cx);
                }
                cx.stop_propagation();
            }
            "home" => {
                self.set_cursor(0, cx);
                cx.stop_propagation();
            }
            "end" => {
                self.set_cursor(self.duration, cx);
                cx.stop_propagation();
            }
            "space" => {
                self.add_event_at_cursor(cx);
                cx.stop_propagation();
            }
            "delete" => {
                self.remove_event_at_cursor(cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn on_track_click(&mut self, fraction: f32, cx: &mut Context<Self>) {
        if self.end_edit.is_some() {
            self.commit_end_edit(cx);
        }
        let fraction = fraction.clamp(0.0, 1.0);
        let cursor = ((self.duration as f32) * fraction).round() as TimeTicks;
        self.set_cursor(cursor, cx);
    }

    fn cursor_fraction(&self) -> f32 {
        if self.duration == 0 {
            0.0
        } else {
            self.cursor as f32 / self.duration as f32
        }
    }

    fn event_fraction(&self, ticks: TimeTicks) -> f32 {
        if self.duration == 0 {
            0.0
        } else {
            ticks as f32 / self.duration as f32
        }
    }
}

fn normalize_events(
    events: impl IntoIterator<Item = TimeTicks>,
    duration: TimeTicks,
) -> Vec<TimeTicks> {
    let mut out: Vec<TimeTicks> = events.into_iter().map(|t| t.min(duration)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn track_fraction(bounds: Bounds<Pixels>, position_x: Pixels) -> f32 {
    let width = bounds.size.width.max(px(1.));
    let x = (position_x - bounds.origin.x).max(px(0.)).min(width);
    (x / width) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_default_duration() {
        assert_eq!(format_time(0), "00:00.0000");
        assert_eq!(format_time(DEFAULT_DURATION), "00:30.0000");
        assert_eq!(format_time(90 * TICKS_PER_SEC + 1234), "01:30.1234");
    }

    #[test]
    fn formats_compact() {
        assert_eq!(format_time_compact(0), ":00.0");
        assert_eq!(format_time_compact(5 * TICKS_PER_SEC), ":05.0");
        assert_eq!(format_time_compact(DEFAULT_DURATION), ":30.0");
        assert_eq!(
            format_time_compact(90 * TICKS_PER_SEC + 1200),
            "1:30.12"
        );
        assert_eq!(format_time_compact(100), ":00.01");
    }

    #[test]
    fn parses_time_strings() {
        assert_eq!(parse_time("00:30.0000"), Some(DEFAULT_DURATION));
        assert_eq!(parse_time("00:00.0000"), Some(0));
        assert_eq!(parse_time(":00.0"), Some(0));
        assert_eq!(parse_time(":05.0"), Some(5 * TICKS_PER_SEC));
        assert_eq!(parse_time("1:02.5"), Some(62 * TICKS_PER_SEC + 5000));
        assert_eq!(parse_time("45.25"), Some(45 * TICKS_PER_SEC + 2500));
        assert_eq!(parse_time("not-a-time"), None);
        assert_eq!(parse_time("00:60.0000"), None);
    }

    #[test]
    fn round_trips() {
        for ticks in [0, 1, 9999, DEFAULT_DURATION, 3_661_234] {
            assert_eq!(parse_time(&format_time(ticks)), Some(ticks));
        }
    }
}

impl Focusable for TimeLine {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TimeLine {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let focused = self.focus_handle.is_focused(window);
        let frac = self.cursor_fraction();
        let show_cursor_time = self.cursor > 0 && self.cursor < self.duration;
        let cursor_label = format_time_compact(self.cursor);
        let end_label = format_time_compact(self.duration);
        let editing = self.end_edit.clone();
        let is_editing = editing.is_some();
        let track_color = if focused { theme.accent } else { theme.muted };
        let cursor_color = theme.accent;
        let event_color = theme.foreground;
        let entity = cx.entity();
        let event_marks: Vec<(f32, TimeTicks)> = self
            .events
            .iter()
            .copied()
            .map(|t| (self.event_fraction(t), t))
            .collect();

        div()
            .id("timeline")
            .key_context("TimeLine")
            .track_focus(&self.focus_handle)
            .tab_index(0)
            .tab_stop(true)
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::nudge_fine_left))
            .on_action(cx.listener(Self::nudge_fine_right))
            .on_action(cx.listener(Self::nudge_coarse_left))
            .on_action(cx.listener(Self::nudge_coarse_right))
            .on_action(cx.listener(Self::jump_start))
            .on_action(cx.listener(Self::jump_end))
            .on_action(cx.listener(Self::add_event_action))
            .on_action(cx.listener(Self::remove_event_action))
            .on_action(cx.listener(Self::confirm_edit))
            .on_action(cx.listener(Self::cancel_edit))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window);
                    cx.notify();
                }),
            )
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(rgb(theme.surface))
            // Cursor time + track
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(56.))
                    // Cursor time label, centered above the playhead (hidden at ends)
                    .when(show_cursor_time, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .left(relative(frac))
                                .ml(px(-48.))
                                .w(px(96.))
                                .flex()
                                .justify_center()
                                .text_color(rgb(cursor_color))
                                .child(SharedString::from(cursor_label)),
                        )
                    })
                    // Track row (inset so end caps stay visible beside the playhead)
                    .child(
                        div()
                            .absolute()
                            .top(px(24.))
                            .left(px(4.))
                            .right(px(4.))
                            .h(px(28.))
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .h(px(28.))
                                    // Click / drag hit target (behind decorations)
                                    .child({
                                        let entity = entity.clone();
                                        canvas(
                                            |bounds, _, _| bounds,
                                            move |bounds: Bounds<Pixels>, _, window, _| {
                                                window.on_mouse_event({
                                                    let entity = entity.clone();
                                                    move |ev: &MouseDownEvent,
                                                          phase,
                                                          window,
                                                          cx| {
                                                        if phase != gpui::DispatchPhase::Bubble {
                                                            return;
                                                        }
                                                        if ev.button != MouseButton::Left {
                                                            return;
                                                        }
                                                        if !bounds.contains(&ev.position) {
                                                            return;
                                                        }
                                                        let fraction = track_fraction(
                                                            bounds,
                                                            ev.position.x,
                                                        );
                                                        entity.update(cx, |this, cx| {
                                                            this.focus_handle.focus(window);
                                                            this.dragging = true;
                                                            this.on_track_click(fraction, cx);
                                                        });
                                                    }
                                                });
                                                window.on_mouse_event({
                                                    let entity = entity.clone();
                                                    move |ev: &MouseMoveEvent, phase, _, cx| {
                                                        if phase != gpui::DispatchPhase::Bubble {
                                                            return;
                                                        }
                                                        if ev.pressed_button
                                                            != Some(MouseButton::Left)
                                                        {
                                                            return;
                                                        }
                                                        entity.update(cx, |this, cx| {
                                                            if !this.dragging {
                                                                return;
                                                            }
                                                            let fraction = track_fraction(
                                                                bounds,
                                                                ev.position.x,
                                                            );
                                                            this.on_track_click(fraction, cx);
                                                        });
                                                    }
                                                });
                                                window.on_mouse_event({
                                                    let entity = entity.clone();
                                                    move |ev: &MouseUpEvent, phase, _, cx| {
                                                        if phase != gpui::DispatchPhase::Bubble {
                                                            return;
                                                        }
                                                        if ev.button != MouseButton::Left {
                                                            return;
                                                        }
                                                        entity.update(cx, |this, cx| {
                                                            if this.dragging {
                                                                this.dragging = false;
                                                                cx.notify();
                                                            }
                                                        });
                                                    }
                                                });
                                            },
                                        )
                                        .absolute()
                                        .inset_0()
                                        .size_full()
                                    })
                                    // Left end cap
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top(px(4.))
                                            .w(px(3.))
                                            .h(px(20.))
                                            .bg(rgb(track_color)),
                                    )
                                    // Right end cap
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .top(px(4.))
                                            .w(px(3.))
                                            .h(px(20.))
                                            .bg(rgb(track_color)),
                                    )
                                    // Horizontal line
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .right_0()
                                            .top(px(13.))
                                            .h(px(2.))
                                            .bg(rgb(track_color)),
                                    )
                                    // Event markers below the line
                                    .children(event_marks.into_iter().map(|(efrac, ticks)| {
                                        div()
                                            .id(("timeline-event", ticks))
                                            .absolute()
                                            .top(px(18.))
                                            .left(relative(efrac))
                                            .ml(px(-3.))
                                            .w(px(6.))
                                            .h(px(6.))
                                            .rounded_full()
                                            .bg(rgb(event_color))
                                    }))
                                    // Playhead: two vertical lines
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(2.))
                                            .left(relative(frac))
                                            .ml(px(-4.))
                                            .flex()
                                            .flex_row()
                                            .gap(px(3.))
                                            .child(
                                                div()
                                                    .w(px(2.))
                                                    .h(px(24.))
                                                    .bg(rgb(cursor_color)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(2.))
                                                    .h(px(24.))
                                                    .bg(rgb(cursor_color)),
                                            ),
                                    ),
                            ),
                    ),
            )
            // End label only (start label removed)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .text_color(rgb(theme.muted))
                    .child(match editing {
                        Some(buf) => div()
                            .id("timeline-end-edit")
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.accent))
                            .bg(rgb(theme.background))
                            .text_color(rgb(theme.foreground))
                            .child(format!("{buf}|"))
                            .into_any_element(),
                        None => div()
                            .id("timeline-end")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(rgb(theme.border)))
                            .text_color(rgb(theme.foreground))
                            .child(SharedString::from(end_label))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    this.focus_handle.focus(window);
                                    this.begin_end_edit(cx);
                                    cx.stop_propagation();
                                    let _ = ev;
                                }),
                            )
                            .into_any_element(),
                    }),
            )
            .child(
                div()
                    .text_color(rgb(theme.muted))
                    .child(if focused {
                        if is_editing {
                            "Editing end time — Enter to apply, Esc to cancel"
                        } else {
                            "← → event · Ctrl±0.01s · Shift±1s · Space add · Delete remove · drag seek"
                        }
                    } else {
                        "Click to focus"
                    }),
            )
    }
}
