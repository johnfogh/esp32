//! GPUI layout components for desktop UIs.
//!
//! Hierarchy:
//! - [`View`] — full-window root that contains [`Panel`]s
//! - [`Panel`] — flexbox region inside a view
//! - [`DesignGrid`] — card/connector lattice inside a panel
//! - [`Card`] — flexbox surface inside a panel or design grid
//! - [`Connector`] — uniform cell between cards in a design grid
//!
//! Controls:
//! - [`Button`] — flexbox button with optional toggle glyph
//! - [`TimeLine`] — horizontal timeline with a playhead cursor
//! - [`TimelineEvents`] — panel of timed event cards bound to a [`TimeLine`]
//!
//! By default, colours come from the current Omarchy theme and the UI font
//! from the system terminal / monospace setting. Call [`install_system_style`]
//! once at startup for live reload when the theme or font changes.

mod card;
mod connector;
mod controls;
mod design_grid;
mod font;
mod panel;
mod system;
mod theme;
mod view;

pub use card::Card;
pub use connector::Connector;
pub use controls::{
    format_time, format_time_compact, parse_time, Button, TimeLine, TimeTicks, TimelineEvent,
    TimelineEvents, COARSE_STEP, DEFAULT_DURATION, DEFAULT_STEP, FINE_STEP, TICKS_PER_SEC,
};
pub use design_grid::DesignGrid;
pub use font::UiFont;
pub use panel::Panel;
pub use system::{install_system_style, resolve_font, resolve_theme, system_style, SystemStyle};
pub use theme::Theme;
pub use view::View;

pub use gpui;
