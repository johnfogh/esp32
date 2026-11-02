//! Grid panel that places cards and connectors on an interleaved integer lattice.
//!
//! Origin is the bottom-left corner. Cards occupy even/even cells; connectors
//! occupy cells where exactly one axis is odd. Odd/odd cells are empty junctions.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Div, IntoElement, ParentElement, Pixels, RenderOnce, Window, div, prelude::*,
    px, rgb,
};

use crate::Theme;
use crate::card::Card;
use crate::connector::Connector;
use crate::system::resolve_theme;

/// One placed cell in the grid.
enum Cell {
    Card(Card),
    Connector(Connector),
}

/// Layout surface inside a [`crate::Panel`] that holds [`Card`]s and [`Connector`]s.
///
/// # Grid rules
/// - `(x, y)` with origin at bottom-left; no negative positions.
/// - Cards only at even `x` and even `y`.
/// - Connectors only when exactly one of `x`, `y` is odd.
/// - All connectors share one uniform size (`connector_size`).
#[derive(IntoElement)]
pub struct DesignGrid {
    cells: HashMap<(u32, u32), Cell>,
    theme: Option<Theme>,
    connector_size: Pixels,
    padding: Pixels,
    grow: bool,
}

impl Default for DesignGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl DesignGrid {
    /// Empty grid with a 24px connector size.
    pub fn new() -> Self {
        Self {
            cells: HashMap::new(),
            theme: None,
            connector_size: px(24.),
            padding: px(0.),
            grow: true,
        }
    }

    /// Override the colour palette (disables live system theme for empty cells).
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Uniform connector width and height (also sizes odd/odd junctions).
    pub fn connector_size(mut self, size: impl Into<Pixels>) -> Self {
        self.connector_size = size.into();
        self
    }

    /// Inner padding around the grid.
    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Whether the grid grows to fill leftover space in the panel (default true).
    pub fn grow(mut self, grow: bool) -> Self {
        self.grow = grow;
        self
    }

    /// Place a card at even/even `(x, y)`. Invalid positions are ignored (debug assert).
    pub fn card(mut self, x: u32, y: u32, card: Card) -> Self {
        if !is_card_cell(x, y) {
            debug_assert!(
                false,
                "DesignGrid::card: ({x},{y}) must be even/even"
            );
            return self;
        }
        self.cells.insert((x, y), Cell::Card(card));
        self
    }

    /// Place a connector where exactly one of `x`, `y` is odd. Invalid positions ignored.
    pub fn connector(mut self, x: u32, y: u32, connector: Connector) -> Self {
        if !is_connector_cell(x, y) {
            debug_assert!(
                false,
                "DesignGrid::connector: ({x},{y}) must have exactly one odd axis"
            );
            return self;
        }
        self.cells.insert((x, y), Cell::Connector(connector));
        self
    }
}

fn is_card_cell(x: u32, y: u32) -> bool {
    x % 2 == 0 && y % 2 == 0
}

fn is_connector_cell(x: u32, y: u32) -> bool {
    (x % 2 == 0) != (y % 2 == 0)
}

fn is_junction_cell(x: u32, y: u32) -> bool {
    x % 2 == 1 && y % 2 == 1
}

impl RenderOnce for DesignGrid {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = resolve_theme(self.theme, cx);
        let cs = self.connector_size;
        let mut cells = self.cells;

        let max_x = cells.keys().map(|(x, _)| *x).max().unwrap_or(0);
        let max_y = cells.keys().map(|(_, y)| *y).max().unwrap_or(0);

        // High Y first so origin (0,0) sits at the visual bottom-left.
        let mut rows: Vec<AnyElement> = Vec::new();
        for y in (0..=max_y).rev() {
            let row_is_connector = y % 2 == 1;
            let mut row_children: Vec<AnyElement> = Vec::new();

            for x in 0..=max_x {
                let child: AnyElement = if is_junction_cell(x, y) {
                    div().flex_none().w(cs).h(cs).into_any_element()
                } else if let Some(cell) = cells.remove(&(x, y)) {
                    match cell {
                        Cell::Card(card) => div()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .w_full()
                            .h_full()
                            .child(card)
                            .into_any_element(),
                        Cell::Connector(connector) => {
                            connector.size(cs).into_any_element()
                        }
                    }
                } else if is_connector_cell(x, y) {
                    div().flex_none().w(cs).h(cs).into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .into_any_element()
                };
                row_children.push(child);
            }

            let mut row: Div = div()
                .flex()
                .flex_row()
                .w_full()
                .children(row_children);

            if row_is_connector {
                row = row.h(cs).flex_none();
            } else {
                row = row.flex_1().min_h(px(0.));
            }

            rows.push(row.into_any_element());
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .p(self.padding)
            .bg(rgb(theme.background))
            .when(self.grow, |d| d.flex_1())
            .children(rows)
    }
}
