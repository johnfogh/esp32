//! System terminal / monospace font detection.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::theme::theme_dirs;

/// Desktop UI font (family + pixel size).
#[derive(Debug, Clone, PartialEq)]
pub struct UiFont {
    pub family: String,
    pub size_px: f32,
}

impl Default for UiFont {
    fn default() -> Self {
        Self::system()
    }
}

impl UiFont {
    /// Hardcoded monospace fallback when no system font is found.
    pub fn fallback() -> Self {
        Self {
            family: "monospace".into(),
            size_px: 16.0,
        }
    }

    /// Resolve the live terminal / monospace font.
    pub fn system() -> Self {
        load_ui_font()
    }
}

/// Live terminal font from Alacritty / Ghostty / Kitty, then monospace
/// gsettings / fontconfig, then [`UiFont::fallback`].
pub fn load_ui_font() -> UiFont {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut paths = vec![
        home.join(".config/alacritty/alacritty.toml"),
        home.join(".config/ghostty/config"),
        home.join(".config/kitty/kitty.conf"),
    ];
    for d in theme_dirs() {
        paths.push(d.join("alacritty.toml"));
    }
    for p in paths {
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
                size_px: UiFont::fallback().size_px,
            };
        }
    }
    UiFont::fallback()
}

/// Paths consulted for font changes (for live-reload signatures).
pub fn font_paths() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut paths = vec![
        home.join(".config/alacritty/alacritty.toml"),
        home.join(".config/ghostty/config"),
        home.join(".config/kitty/kitty.conf"),
    ];
    for d in theme_dirs() {
        paths.push(d.join("alacritty.toml"));
    }
    paths
}

/// Cheap change signature for terminal font config files.
pub fn signature() -> Vec<(PathBuf, Option<SystemTime>)> {
    font_paths()
        .into_iter()
        .map(|p| {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            (p, mtime)
        })
        .collect()
}

fn quoted_value(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches(',');
    if let Some(q) = s.find('"') {
        let rest = &s[q + 1..];
        return rest.find('"').map(|e| rest[..e].to_string());
    }
    let v = s.trim_matches('\'').trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn parse_size_token(s: &str) -> Option<f32> {
    let n = s
        .trim()
        .trim_end_matches(',')
        .trim_end_matches("pt")
        .trim_end_matches("px");
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
        if lower.starts_with("font-family")
            || lower.starts_with("font_family")
            || lower.contains("family")
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
        if lower.starts_with("font-size")
            || lower.starts_with("font_size")
            || lower.starts_with("size")
        {
            let rhs = l
                .split_once('=')
                .map(|(_, r)| r)
                .unwrap_or_else(|| l.split_whitespace().nth(1).unwrap_or(""));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_resolves_nonempty_family() {
        let f = UiFont::system();
        assert!(!f.family.is_empty());
        assert!(f.size_px > 0.0);
    }

    #[test]
    fn parses_ghostty_font_config() {
        let text = r##"
font-family = "JetBrainsMono Nerd Font"
font-size = 12
"##;
        let f = parse_terminal_config(text).expect("parse");
        assert_eq!(f.family, "JetBrainsMono Nerd Font");
        assert!((f.size_px - 16.0).abs() < 0.01); // 12pt * 96/72
    }
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
