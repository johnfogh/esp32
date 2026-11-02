//! Live Omarchy theme + terminal font as a GPUI global.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui::{App, Global};

use crate::font::{self, UiFont};
use crate::theme::{self, Theme};

const STYLE_POLL: Duration = Duration::from_secs(2);

/// Combined theme + font state shared across the app.
#[derive(Debug, Clone)]
pub struct SystemStyle {
    pub theme: Theme,
    pub font: UiFont,
    theme_sig: Vec<(PathBuf, Option<SystemTime>)>,
    font_sig: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Global for SystemStyle {}

/// Marker so `install_system_style` only spawns one poller.
struct SystemStyleInstalled;
impl Global for SystemStyleInstalled {}

impl SystemStyle {
    fn load() -> Self {
        Self {
            theme: Theme::system(),
            font: UiFont::system(),
            theme_sig: theme::signature(),
            font_sig: font::signature(),
        }
    }
}

/// Read the installed [`SystemStyle`], or load theme/font ad hoc.
pub fn system_style(cx: &App) -> SystemStyle {
    cx.try_global::<SystemStyle>()
        .cloned()
        .unwrap_or_else(SystemStyle::load)
}

/// Resolve theme: explicit override → global → fresh system load.
pub fn resolve_theme(override_theme: Option<Theme>, cx: &App) -> Theme {
    override_theme
        .or_else(|| cx.try_global::<SystemStyle>().map(|s| s.theme))
        .unwrap_or_else(Theme::system)
}

/// Resolve font: explicit override → global → fresh system load.
pub fn resolve_font(override_font: Option<UiFont>, cx: &App) -> UiFont {
    override_font
        .or_else(|| cx.try_global::<SystemStyle>().map(|s| s.font.clone()))
        .unwrap_or_else(UiFont::system)
}

/// Install Omarchy theme + terminal font as a global and start live reload.
///
/// Safe to call more than once: the poller is only spawned on the first call.
/// On change (every ~2s poll), updates the global and refreshes all windows.
pub fn install_system_style(cx: &mut App) {
    if !cx.has_global::<SystemStyle>() {
        cx.set_global(SystemStyle::load());
    }
    if cx.has_global::<SystemStyleInstalled>() {
        return;
    }
    cx.set_global(SystemStyleInstalled);

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(STYLE_POLL).await;
            let theme_sig = theme::signature();
            let font_sig = font::signature();
            let font = UiFont::system();
            let ok = cx.update(|cx| {
                let Some(current) = cx.try_global::<SystemStyle>() else {
                    return;
                };
                let theme_changed = theme_sig != current.theme_sig;
                let font_changed = font_sig != current.font_sig || font != current.font;
                if !theme_changed && !font_changed {
                    return;
                }
                let mut next = current.clone();
                if theme_changed {
                    next.theme_sig = theme_sig;
                    next.theme = Theme::system();
                }
                if font_changed {
                    next.font_sig = font_sig;
                    next.font = font;
                }
                cx.set_global(next);
                cx.refresh_windows();
            });
            if ok.is_err() {
                break;
            }
        }
    })
    .detach();
}
