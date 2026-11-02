# guilib

Rust library of GPUI layout components for desktop UIs.

## Components

| Component | Role |
|-----------|------|
| [`View`](src/view.rs) | Full-window root that contains panels |
| [`Panel`](src/panel.rs) | Flexbox region inside a view |
| [`DesignGrid`](src/design_grid.rs) | Card/connector lattice inside a panel (origin bottom-left) |
| [`Card`](src/card.rs) | Flexbox surface inside a panel or design grid |
| [`Connector`](src/connector.rs) | Uniform cell between cards in a design grid |
| [`TimeLine`](src/controls/timeline.rs) | Horizontal timeline with a playhead cursor |

Hierarchy: **View → Panel → Card** (or **Panel → DesignGrid → Card / Connector**). Controls such as `TimeLine` sit inside a card.

### DesignGrid

Interleaved integer grid: cards at even/even `(x, y)`, connectors where exactly one axis is odd, odd/odd junctions left empty. Origin `(0,0)` is bottom-left. All connectors share one `connector_size` (equal width and height).

### TimeLine

A capped horizontal track from `00:00.0000` to an editable end time (default `00:30.0000`). The playhead is two vertical lines with the current time above it. When focused, ← / → nudge the cursor (Home / End jump to the ends). Click the track to seek; click the end time to edit it (Enter applies, Esc cancels). Call [`TimeLine::bind_keys`](src/controls/timeline.rs) once at startup.

## Defaults

- **Colours** follow the current Omarchy theme (`~/.config/omarchy/current/theme` or `~/.local/state/omarchy/current/theme`: `colors.toml`, then `alacritty.toml`). Falls back to a dark palette when no theme is found.
- **Font** follows the system terminal / monospace setting (Alacritty, Ghostty, Kitty, then gsettings / fontconfig, then `monospace`).
- Call [`install_system_style`](src/system.rs) once at startup to live-reload theme and font when they change on disk.

## Box model

`View`, `Panel`, and `Card` each support:

| Builder | Meaning |
|---------|---------|
| `padding` | Inner padding |
| `margin` | Outer margin |
| `border` | Border width (`px(0.)` disables) |
| `border_color` | Optional colour override (defaults to `theme.border`) |

Defaults: View padding 8 / no border; Panel padding 8 / 1px border; Card padding 12 / 1px border. Margins default to 0.

## Build

```bash
cargo build
cargo build --examples
```

## Demo

```bash
cargo run --example demo
```

The demo opens a window with a top panel (status + action chips), a main panel (sidebar + content cards), and a timeline panel with a focusable `TimeLine` control, using the live Omarchy theme and terminal font.

## Usage

```rust
use guilib::{Card, Panel, View, install_system_style};
use gpui::{div, prelude::*, px, App, Application};

Application::new().run(|cx: &mut App| {
    install_system_style(cx);
    // open window…
});

View::new()
    .child(
        Panel::new()
            .title("Main")
            .padding(px(8.))
            .margin(px(4.))
            .border(px(1.))
            .child(
                Card::new()
                    .title("Hello")
                    .child(div().child("Content")),
            ),
    )
```

Override colours with `.theme(Theme::fallback())` or a custom `Theme` when you need a fixed palette.

## Notes

Uses `gpui` 0.2.2 with local patches for `xattr` and `proc-macro-error2` (same as `target_editor`).
