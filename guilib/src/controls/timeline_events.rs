//! Panel of timed event cards bound to a [`super::TimeLine`].

use gpui::{
    AbsoluteLength, App, Div, Entity, FlexDirection, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, RenderOnce, SharedString, Window, div, prelude::*, px, rgb,
};

use super::{TimeLine, TimeTicks, format_time_compact, parse_time};
use crate::Theme;
use crate::system::resolve_theme;

/// Card for a single timeline event: stores a time, shows only a label.
#[derive(IntoElement, Clone)]
pub struct TimelineEvent {
    time: TimeTicks,
    label: SharedString,
    theme: Option<Theme>,
    highlighted: bool,
    /// When set, a click seeks this timeline to [`Self::time`].
    seek: Option<Entity<TimeLine>>,
}

impl TimelineEvent {
    /// Create an event at `time` with the given label.
    pub fn new(time: TimeTicks, label: impl Into<SharedString>) -> Self {
        Self {
            time,
            label: label.into(),
            theme: None,
            highlighted: false,
            seek: None,
        }
    }

    /// Override the event time in ticks.
    pub fn time(mut self, time: TimeTicks) -> Self {
        self.time = time;
        self
    }

    /// Set time from `MM:SS.ffff` (or other [`parse_time`] forms). Invalid input is ignored.
    pub fn time_str(mut self, s: &str) -> Self {
        if let Some(t) = parse_time(s) {
            self.time = t;
        }
        self
    }

    /// Label drawn on the card (time is never drawn).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Override the colour palette.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Accent styling when the bound timeline cursor matches this event's time.
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    /// Stored event time in ticks.
    pub fn time_ticks(&self) -> TimeTicks {
        self.time
    }

    /// Event label.
    pub fn label_str(&self) -> &SharedString {
        &self.label
    }
}

impl RenderOnce for TimelineEvent {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let border = if self.highlighted {
            theme.accent
        } else {
            theme.border
        };
        let text = if self.highlighted {
            theme.accent
        } else {
            theme.foreground
        };
        let time = self.time;
        let seek = self.seek;

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .rounded_md()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(theme.surface))
            .px_3()
            .py_2()
            .text_color(rgb(text))
            .when(seek.is_some(), |d| d.cursor_pointer())
            .when_some(seek, |d, timeline| {
                d.on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, _, cx| {
                    timeline.update(cx, |tl, cx| tl.set_cursor_ticks(time, cx));
                })
            })
            .child(self.label)
    }
}

/// Panel of [`TimelineEvent`] cards connected to a [`TimeLine`] entity.
#[derive(IntoElement)]
pub struct TimelineEvents {
    timeline: Entity<TimeLine>,
    events: Vec<TimelineEvent>,
    theme: Option<Theme>,
    direction: FlexDirection,
    gap: Pixels,
    padding: Pixels,
    margin: Pixels,
    border: Pixels,
    border_color: Option<u32>,
    title: Option<SharedString>,
    grow: bool,
}

impl TimelineEvents {
    /// Create an events panel bound to `timeline`.
    pub fn new(timeline: Entity<TimeLine>) -> Self {
        Self {
            timeline,
            events: Vec::new(),
            theme: None,
            direction: FlexDirection::Row,
            gap: px(8.),
            padding: px(8.),
            margin: px(0.),
            border: px(1.),
            border_color: None,
            title: None,
            grow: true,
        }
    }

    /// Bound timeline control.
    pub fn timeline_entity(&self) -> &Entity<TimeLine> {
        &self.timeline
    }

    /// Override the colour palette.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Optional header label.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Lay out events in a row.
    pub fn flex_row(mut self) -> Self {
        self.direction = FlexDirection::Row;
        self
    }

    /// Lay out events in a column.
    pub fn flex_col(mut self) -> Self {
        self.direction = FlexDirection::Column;
        self
    }

    /// Gap between event cards.
    pub fn gap(mut self, gap: impl Into<Pixels>) -> Self {
        self.gap = gap.into();
        self
    }

    /// Inner padding.
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Outer margin.
    pub fn margin(mut self, margin: impl Into<Pixels>) -> Self {
        self.margin = margin.into();
        self
    }

    /// Border width (`px(0.)` disables the border).
    pub fn border(mut self, border: impl Into<Pixels>) -> Self {
        self.border = border.into();
        self
    }

    /// Border colour override (defaults to `theme.border`).
    pub fn border_color(mut self, color: u32) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Whether the panel grows to fill leftover space (default true).
    pub fn grow(mut self, grow: bool) -> Self {
        self.grow = grow;
        self
    }

    /// Append one event card.
    pub fn event(mut self, event: TimelineEvent) -> Self {
        self.events.push(event);
        self
    }

    /// Append several event cards.
    pub fn events(mut self, events: impl IntoIterator<Item = TimelineEvent>) -> Self {
        self.events.extend(events);
        self
    }
}

impl RenderOnce for TimelineEvents {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let border_color = self.border_color.unwrap_or(theme.border);

        let timeline_state = self.timeline.read(cx);
        let cursor = timeline_state.cursor_ticks();

        // Prefer marks from the bound TimeLine; fall back to builder-supplied events.
        let mut events: Vec<TimelineEvent> = if self.events.is_empty() {
            timeline_state
                .events_ticks()
                .iter()
                .copied()
                .map(|ticks| TimelineEvent::new(ticks, format_time_compact(ticks)))
                .collect()
        } else {
            self.events
        };

        events.sort_by_key(|e| e.time_ticks());
        for event in &mut events {
            event.highlighted = event.time_ticks() == cursor;
            event.seek = Some(self.timeline.clone());
        }

        let mut body: Div = div()
            .flex()
            .gap(self.gap)
            .p(self.padding)
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .children(events);
        body.style().flex_direction = Some(self.direction);

        let mut root: Div = div()
            .flex()
            .flex_col()
            .w_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .m(self.margin)
            .bg(rgb(theme.background))
            .when(self.grow, |d| d.flex_1())
            .when_some(self.title, |d, title| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(border_color))
                        .text_color(rgb(theme.muted))
                        .child(title),
                )
            })
            .child(body);

        if self.border > px(0.) {
            root = root.border_color(rgb(border_color));
            let w: AbsoluteLength = self.border.into();
            let style = root.style();
            style.border_widths.top = Some(w);
            style.border_widths.right = Some(w);
            style.border_widths.bottom = Some(w);
            style.border_widths.left = Some(w);
        }

        root
    }
}
