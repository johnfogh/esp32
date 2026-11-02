//! Clickable button control with optional toggle state.

use gpui::{
    div, prelude::*, px, rgb, App, Context, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    Pixels, SharedString, Window,
};

use crate::system::resolve_theme;
use crate::theme::blend;
use crate::Theme;

/// Glyph shown when a toggleable button is on.
const GLYPH_ON: &str = "●";
/// Glyph shown when a toggleable button is off.
const GLYPH_OFF: &str = "○";

/// Flexbox button that lives inside a layout surface (e.g. a [`crate::Card`]).
///
/// When [`Self::toggleable`] is true (default false), clicks flip an internal
/// toggled flag and a ●/○ glyph reflects the state.
pub struct Button {
    focus_handle: FocusHandle,
    label: SharedString,
    theme: Option<Theme>,
    toggleable: bool,
    toggled: bool,
    padding: Pixels,
    grow: bool,
}

impl Button {
    /// Create a non-toggleable button with the given label.
    pub fn new(label: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            label: label.into(),
            theme: None,
            toggleable: false,
            toggled: false,
            padding: px(8.),
            grow: false,
        }
    }

    /// Override the colour palette.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Enable toggle behaviour (default false).
    pub fn toggleable(mut self, toggleable: bool) -> Self {
        self.toggleable = toggleable;
        self
    }

    /// Initial toggled state (meaningful when toggleable).
    pub fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = toggled;
        self
    }

    /// Inner padding (default `px(8.)`).
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Whether the button grows to fill leftover flex space (default false).
    pub fn grow(mut self, grow: bool) -> Self {
        self.grow = grow;
        self
    }

    /// Current toggled state.
    pub fn is_toggled(&self) -> bool {
        self.toggled
    }

    /// Whether this button is toggleable.
    pub fn is_toggleable(&self) -> bool {
        self.toggleable
    }

    /// Set toggled state and notify.
    pub fn set_toggled(&mut self, toggled: bool, cx: &mut Context<Self>) {
        if self.toggled != toggled {
            self.toggled = toggled;
            cx.notify();
        }
    }

    /// Flip toggled state when toggleable.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.toggleable {
            self.toggled = !self.toggled;
            cx.notify();
        }
    }
}

impl Focusable for Button {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Button {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let toggleable = self.toggleable;
        let toggled = self.toggled;

        let (bg, fg, border) = if toggleable {
            if toggled {
                (theme.accent, 0xffffff, theme.accent)
            } else {
                (theme.surface, theme.foreground, theme.border)
            }
        } else {
            (theme.accent, 0xffffff, theme.accent)
        };
        let hover_bg = blend(bg, theme.foreground, 0.18);
        let pressed_bg = blend(bg, theme.background, 0.25);
        let focus_border = blend(border, theme.foreground, 0.45);

        let label = if toggleable {
            let glyph = if toggled { GLYPH_ON } else { GLYPH_OFF };
            format!("{glyph} {}", self.label)
        } else {
            self.label.to_string()
        };

        let id = SharedString::from(format!("button-{}", self.label));

        div()
            .id(id)
            .track_focus(&self.focus_handle)
            .tab_index(0)
            .tab_stop(true)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .p(self.padding)
            .rounded_md()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .hover(move |d| d.bg(rgb(hover_bg)))
            .active(move |d| d.bg(rgb(pressed_bg)))
            .focus(move |d| d.border_color(rgb(focus_border)).border_2())
            .when(self.grow, |d| d.flex_1())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window);
                    if this.toggleable {
                        this.toggled = !this.toggled;
                    }
                    cx.notify();
                }),
            )
            .child(label)
    }
}
