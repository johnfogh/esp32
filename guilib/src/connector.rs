//! Connector cell that sits between cards in a [`crate::DesignGrid`].

use gpui::{
    AbsoluteLength, App, Div, IntoElement, Pixels, RenderOnce, Window, div, prelude::*, px, rgb,
};

use crate::Theme;
use crate::system::resolve_theme;

/// Visual link between cards on a [`crate::DesignGrid`].
///
/// Fills a uniform connector cell (equal width and height set by the grid).
#[derive(IntoElement)]
pub struct Connector {
    theme: Option<Theme>,
    border: Pixels,
    border_color: Option<u32>,
    /// When set by [`crate::DesignGrid`], forces the cell size.
    size: Option<Pixels>,
}

impl Default for Connector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector {
    /// Create a connector with default border styling.
    pub fn new() -> Self {
        Self {
            theme: None,
            border: px(1.),
            border_color: None,
            size: None,
        }
    }

    /// Override the colour palette (disables live system theme for this connector).
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
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

    /// Force width and height (used by [`crate::DesignGrid`]).
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = Some(size.into());
        self
    }
}

impl RenderOnce for Connector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let border_color = self.border_color.unwrap_or(theme.border);

        let mut root: Div = div()
            .flex()
            .flex_none()
            .rounded_sm()
            .bg(rgb(theme.muted))
            .when_some(self.size, |d, s| d.w(s).h(s));

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
