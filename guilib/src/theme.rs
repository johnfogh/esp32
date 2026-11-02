//! Omarchy / system theme colours for guilib components.
//!
//! Source order: `<theme>/colors.toml` (Omarchy palette), then
//! `<theme>/alacritty.toml`, then [`Theme::fallback`].
//! `<theme>` is `~/.config/omarchy/current/theme` or
//! `~/.local/state/omarchy/current/theme`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 0xRRGGBB colours used by View, Panel, and Card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub background: u32,
    pub surface: u32,
    pub foreground: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::system()
    }
}

/// Mix `b` into `a` by `t` (0..=1).
pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let mix = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32).min(255) << s;
    mix(16) | mix(8) | mix(0)
}

/// Parse `#rrggbb`, `0xrrggbb`, or `rrggbb` (optional quotes; alpha suffix ignored).
pub fn parse_hex(v: &str) -> Option<u32> {
    let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
    let v = v
        .strip_prefix('#')
        .or_else(|| v.strip_prefix("0x"))
        .unwrap_or(v);
    if v.len() != 6 && v.len() != 8 {
        return None;
    }
    u32::from_str_radix(&v[..6], 16).ok()
}

/// Flat `section.key -> value` map of simple TOML (`key = value` lines).
fn parse_flat_toml(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l.starts_with('[') {
            section = l.trim_matches(|c| c == '[' || c == ']').trim().to_string();
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let v = v.trim();
            let v = if let Some(q) = v.strip_prefix('"') {
                q.split('"').next().unwrap_or("")
            } else {
                v.split_whitespace().next().unwrap_or("")
            };
            let key = if section.is_empty() {
                k.trim().to_string()
            } else {
                format!("{section}.{}", k.trim())
            };
            out.insert(key, v.to_string());
        }
    }
    out
}

impl Theme {
    /// Hardcoded dark palette used when no Omarchy theme is available.
    pub fn fallback() -> Self {
        Self {
            background: 0x0d_11_17,
            surface: 0x16_1b_22,
            foreground: 0xc9_d1_d9,
            muted: 0x8b_94_9e,
            border: 0x30_36_3d,
            accent: 0x2f_6f_eb,
        }
    }

    /// Load the current Omarchy theme, or [`Theme::fallback`].
    pub fn system() -> Self {
        load_current().0
    }

    /// Build a theme from base colours, deriving surface / muted / border.
    pub fn from_base(bg: u32, fg: u32, accent: u32) -> Self {
        Self {
            background: bg,
            foreground: fg,
            accent,
            border: blend(bg, fg, 0.22),
            muted: blend(bg, fg, 0.6),
            surface: blend(bg, fg, 0.06),
        }
    }

    /// Omarchy `colors.toml` (supports both older and newer key sets).
    pub fn from_colors_toml(text: &str) -> Option<Self> {
        let m = parse_flat_toml(text);
        let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
        let bg = g("background")?;
        let fg = g("foreground")?;
        let accent = g("accent")
            .or_else(|| g("cursor"))
            .or_else(|| g("blue"))
            .unwrap_or(fg);
        let mut theme = Self::from_base(bg, fg, accent);
        if let Some(muted) = g("muted") {
            theme.muted = muted;
        }
        if let Some(surface) = g("lighter_background").or_else(|| g("surface")) {
            theme.surface = surface;
        }
        if let Some(border) = g("border").or_else(|| g("selection")) {
            theme.border = border;
        }
        Some(theme)
    }

    /// Alacritty theme (`[colors.primary]` etc.).
    pub fn from_alacritty_toml(text: &str) -> Option<Self> {
        let m = parse_flat_toml(text);
        let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
        let bg = g("colors.primary.background")?;
        let fg = g("colors.primary.foreground")?;
        let accent = g("colors.cursor.cursor")
            .or_else(|| g("colors.normal.blue"))
            .unwrap_or(fg);
        let mut theme = Self::from_base(bg, fg, accent);
        if let Some(surface) = g("colors.selection.background") {
            theme.border = blend(bg, surface, 0.5);
        }
        Some(theme)
    }

    /// Load from a theme directory, trying `colors.toml` then `alacritty.toml`.
    pub fn load_dir(dir: &Path) -> Option<(Self, PathBuf)> {
        let c = dir.join("colors.toml");
        if let Some(t) = std::fs::read_to_string(&c)
            .ok()
            .and_then(|s| Self::from_colors_toml(&s))
        {
            return Some((t, c));
        }
        let a = dir.join("alacritty.toml");
        std::fs::read_to_string(&a)
            .ok()
            .and_then(|s| Self::from_alacritty_toml(&s))
            .map(|t| (t, a))
    }
}

/// Candidate Omarchy theme directories, in priority order.
pub fn theme_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    vec![
        home.join(".config/omarchy/current/theme"),
        home.join(".local/state/omarchy/current/theme"),
    ]
}

/// Current theme plus the file it came from (`None` = fallback).
pub fn load_current() -> (Theme, Option<PathBuf>) {
    theme_dirs()
        .iter()
        .find_map(|d| Theme::load_dir(d))
        .map_or((Theme::fallback(), None), |(t, p)| (t, Some(p)))
}

/// Cheap change signature: resolved theme dirs + mtimes of the files we read.
pub fn signature() -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut sig = Vec::new();
    for d in theme_dirs() {
        let resolved = std::fs::canonicalize(&d).unwrap_or(d.clone());
        for f in [Some("colors.toml"), Some("alacritty.toml"), None] {
            let p = match f {
                Some(f) => resolved.join(f),
                None => resolved.clone(),
            };
            sig.push((
                p.clone(),
                std::fs::metadata(&p).and_then(|m| m.modified()).ok(),
            ));
        }
        if let Some(parent) = d.parent() {
            let n = parent.join("theme.name");
            sig.push((
                n.clone(),
                std::fs::metadata(&n).and_then(|m| m.modified()).ok(),
            ));
        }
    }
    sig
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_theme_loads_omarchy_or_fallback() {
        let t = Theme::system();
        assert_ne!(t.foreground, t.background);
        assert_ne!(t.accent, 0);
        // When an Omarchy theme dir is present, load_current reports its path.
        if load_current().1.is_some() {
            assert_ne!(t, Theme::fallback());
        }
    }

    #[test]
    fn parses_new_omarchy_colors_toml() {
        let text = r##"
mode = "dark"
accent = "#509475"
selection = "#32473B"
muted = "#53685B"
background = "#111c18"
lighter_background = "#23372B"
foreground = "#C1C497"
"##;
        let t = Theme::from_colors_toml(text).expect("parse");
        assert_eq!(t.background, 0x111c18);
        assert_eq!(t.foreground, 0xC1C497);
        assert_eq!(t.accent, 0x509475);
        assert_eq!(t.muted, 0x53685B);
        assert_eq!(t.surface, 0x23372B);
        assert_eq!(t.border, 0x32473B);
    }
}
