//! Full-window root that contains panels.

use gpui::{
    AbsoluteLength, AnyElement, App, Div, FlexDirection, IntoElement, ParentElement, Pixels,
    RenderOnce, Window, div, prelude::*, px, rgb,
};

use crate::Theme;
use crate::font::UiFont;
use crate::system::{resolve_font, resolve_theme};

/// Full-window component that lays out [`crate::Panel`] children with flexbox.
#[derive(IntoElement)]
pub struct View {
    children: Vec<AnyElement>,
    theme: Option<Theme>,
    font: Option<UiFont>,
    direction: FlexDirection,
    gap: Pixels,
    padding: Pixels,
    margin: Pixels,
    border: Pixels,
    border_color: Option<u32>,
}

impl Default for View {
    fn default() -> Self {
        Self::new()
    }
}

impl View {
    /// Create an empty full-window view (column layout by default).
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            theme: None,
            font: None,
            direction: FlexDirection::Column,
            gap: px(8.),
            padding: px(8.),
            margin: px(0.),
            border: px(0.),
            border_color: None,
        }
    }

    /// Override the colour palette (disables live system theme for this view).
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Override the UI font (disables live system font for this view).
    pub fn font(mut self, font: UiFont) -> Self {
        self.font = Some(font);
        self
    }

    /// Lay out panels in a row.
    pub fn flex_row(mut self) -> Self {
        self.direction = FlexDirection::Row;
        self
    }

    /// Lay out panels in a column (default).
    pub fn flex_col(mut self) -> Self {
        self.direction = FlexDirection::Column;
        self
    }

    /// Gap between panels.
    pub fn gap(mut self, gap: impl Into<Pixels>) -> Self {
        self.gap = gap.into();
        self
    }

    /// Outer padding around the panel stack.
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Outer margin around the view.
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

    /// Add a panel (or any element) as a child.
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    /// Add several children at once.
    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(|c| c.into_any_element()));
        self
    }
}

impl RenderOnce for View {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let font = resolve_font(self.font, cx);
        let border_color = self.border_color.unwrap_or(theme.border);

        let mut root: Div = div()
            .size_full()
            .flex()
            .gap(self.gap)
            .p(self.padding)
            .m(self.margin)
            .bg(rgb(theme.background))
            .text_color(rgb(theme.foreground))
            .font_family(font.family)
            .text_size(px(font.size_px))
            .children(self.children);

        if self.border > px(0.) {
            root = root.border_color(rgb(border_color));
            let w: AbsoluteLength = self.border.into();
            let style = root.style();
            style.border_widths.top = Some(w);
            style.border_widths.right = Some(w);
            style.border_widths.bottom = Some(w);
            style.border_widths.left = Some(w);
        }

        root.style().flex_direction = Some(self.direction);
        root
    }
}
