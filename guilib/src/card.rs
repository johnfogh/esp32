//! Flexbox surface inside a panel.

use gpui::{
    AbsoluteLength, AnyElement, App, Div, FlexDirection, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, Window, div, prelude::*, px, rgb,
};

use crate::Theme;
use crate::system::resolve_theme;

/// Flexbox card that lives inside a [`crate::Panel`].
#[derive(IntoElement)]
pub struct Card {
    children: Vec<AnyElement>,
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

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

impl Card {
    /// Create an empty card (column layout by default).
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            theme: None,
            direction: FlexDirection::Column,
            gap: px(6.),
            padding: px(12.),
            margin: px(0.),
            border: px(1.),
            border_color: None,
            title: None,
            grow: true,
        }
    }

    /// Override the colour palette (disables live system theme for this card).
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Optional header label drawn above the card body.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Lay out content in a row.
    pub fn flex_row(mut self) -> Self {
        self.direction = FlexDirection::Row;
        self
    }

    /// Lay out content in a column (default).
    pub fn flex_col(mut self) -> Self {
        self.direction = FlexDirection::Column;
        self
    }

    /// Gap between children.
    pub fn gap(mut self, gap: impl Into<Pixels>) -> Self {
        self.gap = gap.into();
        self
    }

    /// Inner padding.
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Outer margin around the card.
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

    /// Whether the card grows to fill leftover space in the panel (default true).
    pub fn grow(mut self, grow: bool) -> Self {
        self.grow = grow;
        self
    }

    /// Add content as a child.
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

impl RenderOnce for Card {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let border_color = self.border_color.unwrap_or(theme.border);

        let mut body: Div = div()
            .flex()
            .gap(self.gap)
            .p(self.padding)
            .when(self.grow, |d| d.w_full().flex_1().min_h(px(0.)).min_w(px(0.)))
            .children(self.children);
        body.style().flex_direction = Some(self.direction);

        let mut root: Div = div()
            .flex()
            .flex_col()
            .when(self.grow, |d| d.w_full().min_h(px(0.)).min_w(px(0.)).flex_1())
            .m(self.margin)
            .rounded_md()
            .bg(rgb(theme.surface))
            .when_some(self.title, |d, title| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(border_color))
                        .text_color(rgb(theme.accent))
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
