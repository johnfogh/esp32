//! Flexbox region inside a view.

use gpui::{
    AbsoluteLength, AnyElement, App, Div, FlexDirection, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, Window, div, prelude::*, px, rgb,
};

use crate::Theme;
use crate::system::resolve_theme;

/// Flexbox panel that lives inside a [`crate::View`] and holds [`crate::Card`]s.
#[derive(IntoElement)]
pub struct Panel {
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
    equal_children: bool,
    wrap: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    /// Create an empty panel (row layout by default).
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            theme: None,
            direction: FlexDirection::Row,
            gap: px(8.),
            padding: px(8.),
            margin: px(0.),
            border: px(1.),
            border_color: None,
            title: None,
            grow: true,
            equal_children: true,
            wrap: false,
        }
    }

    /// Override the colour palette (disables live system theme for this panel).
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Optional header label drawn above the flex children.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Lay out cards in a row (default).
    pub fn flex_row(mut self) -> Self {
        self.direction = FlexDirection::Row;
        self
    }

    /// Lay out cards in a column.
    pub fn flex_col(mut self) -> Self {
        self.direction = FlexDirection::Column;
        self
    }

    /// Gap between cards.
    pub fn gap(mut self, gap: impl Into<Pixels>) -> Self {
        self.gap = gap.into();
        self
    }

    /// Inner padding around the card stack.
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Outer margin around the panel.
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

    /// Whether the panel grows to fill leftover space in the view (default true).
    pub fn grow(mut self, grow: bool) -> Self {
        self.grow = grow;
        self
    }

    /// Whether children share the main axis equally (default true).
    ///
    /// In the default row layout this splits width; in a column it splits height.
    pub fn equal_children(mut self, equal: bool) -> Self {
        self.equal_children = equal;
        self
    }

    /// Whether children wrap onto additional lines when they overflow (default false).
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Add a card (or any element) as a child.
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

impl RenderOnce for Panel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let border_color = self.border_color.unwrap_or(theme.border);

        let children: Vec<AnyElement> = if self.equal_children {
            self.children
                .into_iter()
                .map(|child| {
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .child(child)
                        .into_any_element()
                })
                .collect()
        } else {
            self.children
        };

        let mut body: Div = div()
            .flex()
            .gap(self.gap)
            .p(self.padding)
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .when(self.wrap, |d| d.flex_wrap())
            .children(children);
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
