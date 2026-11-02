//! Omarchy theme colours.
//!
//! Source order: `<theme>/colors.toml` (Omarchy's canonical palette: `background`,
//! `foreground`, `accent`, `cursor`, `selection_*`), then `<theme>/alacritty.toml`
//! (`[colors.primary]`, `[colors.cursor]`, `[colors.selection]`), then dark defaults.
//! `<theme>` is `~/.config/omarchy/current/theme`, or `~/.local/state/omarchy/current/theme`
//! on newer Omarchy releases.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 0xRRGGBB colours used by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub accent: u32,
    /// Omarchy / terminal cursor.
    pub cursor: u32,
    pub selection_bg: u32,
    pub selection_fg: u32,
    /// Derived: subtle borders.
    pub border: u32,
    /// Derived: secondary text.
    pub muted: u32,
    /// Derived: raised surfaces (output panel, buttons).
    pub surface: u32,
    /// Derived: disabled text.
    pub disabled: u32,
    /// Derived: odd table-row background.
    pub row_alt: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_base(0x0d1117, 0xc9d1d9, 0x2f6feb, None, None, None)
    }
}

/// Mix `b` into `a` by `t` (0..=1).
pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let mix = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32).min(255) << s;
    mix(16) | mix(8) | mix(0)
}

/// Parse "#rrggbb", "0xrrggbb", "rrggbb" (optionally quoted, with alpha suffix ignored).
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
    pub fn from_base(
        bg: u32,
        fg: u32,
        accent: u32,
        sel_bg: Option<u32>,
        sel_fg: Option<u32>,
        cursor: Option<u32>,
    ) -> Self {
        let selection_bg = sel_bg.unwrap_or(accent);
        Self {
            background: bg,
            foreground: fg,
            accent,
            cursor: cursor.unwrap_or(selection_bg),
            selection_bg,
            selection_fg: sel_fg.unwrap_or(bg),
            border: blend(bg, fg, 0.22),
            muted: blend(bg, fg, 0.6),
            surface: blend(bg, fg, 0.06),
            disabled: blend(bg, fg, 0.35),
            row_alt: blend(bg, fg, 0.10),
        }
    }

    /// Omarchy `colors.toml`.
    pub fn from_colors_toml(text: &str) -> Option<Self> {
        let m = parse_flat_toml(text);
        let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
        let bg = g("background")?;
        let fg = g("foreground")?;
        let accent = g("accent").or(g("cursor")).or(g("color4")).unwrap_or(fg);
        let cursor = g("cursor");
        Some(Self::from_base(
            bg,
            fg,
            accent,
            g("selection_background").or(cursor),
            g("selection_foreground"),
            cursor,
        ))
    }

    /// Alacritty theme (`[colors.primary]` etc.).
    pub fn from_alacritty_toml(text: &str) -> Option<Self> {
        let m = parse_flat_toml(text);
        let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
        let bg = g("colors.primary.background")?;
        let fg = g("colors.primary.foreground")?;
        let accent = g("colors.cursor.cursor")
            .or(g("colors.normal.blue"))
            .unwrap_or(fg);
        let cursor = g("colors.cursor.cursor");
        Some(Self::from_base(
            bg,
            fg,
            accent,
            g("colors.selection.background").or(cursor),
            g("colors.selection.text"),
            cursor,
        ))
    }

    /// Load from a theme directory, trying colors.toml then alacritty.toml.
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

/// Current theme plus the file it came from (`None` = defaults).
pub fn load_current() -> (Theme, Option<PathBuf>) {
    theme_dirs()
        .iter()
        .find_map(|d| Theme::load_dir(d))
        .map_or((Theme::default(), None), |(t, p)| (t, Some(p)))
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

/// Desktop UI font (family + pixel size).
#[derive(Debug, Clone, PartialEq)]
pub struct UiFont {
    pub family: String,
    pub size_px: f32,
}

impl Default for UiFont {
    fn default() -> Self {
        Self {
            family: "JetBrainsMono Nerd Font".into(),
            size_px: 16.0,
        }
    }
}

/// Live terminal font from Alacritty / Ghostty / Kitty, then monospace gsettings / fontconfig.
pub fn load_ui_font() -> UiFont {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    for p in [
        home.join(".config/alacritty/alacritty.toml"),
        home.join(".config/ghostty/config"),
        home.join(".config/kitty/kitty.conf"),
        home.join(".local/state/omarchy/current/theme/alacritty.toml"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&p)
            && let Some(f) = parse_terminal_config(&text)
        {
            return f;
        }
    }
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "monospace-font-name"])
        .output()
        && out.status.success()
        && let Some(f) = parse_ui_font(&String::from_utf8_lossy(&out.stdout))
    {
        return f;
    }
    if let Ok(out) = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return UiFont {
                family: s,
                size_px: UiFont::default().size_px,
            };
        }
    }
    UiFont::default()
}

fn quoted_value(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches(',');
    if let Some(q) = s.find('"') {
        let rest = &s[q + 1..];
        return rest.find('"').map(|e| rest[..e].to_string());
    }
    let v = s.trim_matches('\'').trim();
    if v.is_empty() { None } else { Some(v.to_string()) }
}

fn parse_size_token(s: &str) -> Option<f32> {
    let n = s.trim().trim_end_matches(',').trim_end_matches("pt").trim_end_matches("px");
    n.parse::<f32>().ok().filter(|v| *v > 0.0)
}

fn parse_terminal_config(text: &str) -> Option<UiFont> {
    let mut family: Option<String> = None;
    let mut size_pt: Option<f32> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let lower = l.to_ascii_lowercase();
        if lower.starts_with("font-family") || lower.starts_with("font_family") || lower.contains("family")
        {
            if let Some((_, rhs)) = l.split_once('=') {
                if let Some(f) = quoted_value(rhs) {
                    family = Some(f);
                }
            } else if lower.starts_with("font_family") {
                let rest = l["font_family".len()..].trim();
                if !rest.is_empty() {
                    family = Some(rest.trim_matches('"').to_string());
                }
            }
        }
        if lower.starts_with("font-size") || lower.starts_with("font_size") || lower.starts_with("size")
        {
            let rhs = l.split_once('=').map(|(_, r)| r).unwrap_or_else(|| {
                l.split_whitespace().nth(1).unwrap_or("")
            });
            if let Some(s) = parse_size_token(rhs) {
                size_pt = Some(s);
            }
        }
    }
    family.map(|family| UiFont {
        family,
        size_px: size_pt.unwrap_or(12.0) * 96.0 / 72.0,
    })
}

fn parse_ui_font(raw: &str) -> Option<UiFont> {
    let s = raw.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    if s.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = s.split_whitespace().collect();
    let mut size_pt = 11.0_f32;
    if let Some(last) = parts.last() {
        let n = last.trim_end_matches("pt").trim_end_matches("px");
        if let Ok(v) = n.parse::<f32>()
            && v > 0.0
        {
            size_pt = v;
            parts.pop();
        }
    }
    let family = parts.join(" ");
    if family.is_empty() {
        None
    } else {
        Some(UiFont {
            family,
            size_px: size_pt * 96.0 / 72.0,
        })
    }
}


