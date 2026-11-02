//! `$XDG_CONFIG_HOME/target-editor/config.json` (default `~/.config/target-editor/config.json`).
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

fn default_targets_folder() -> String {
    "~/dev/esp32/target_defs".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_targets_folder")]
    pub targets_folder: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            targets_folder: default_targets_folder(),
        }
    }
}

static LOADED: OnceLock<(Config, Option<String>)> = OnceLock::new();

/// `$XDG_CONFIG_HOME/target-editor` (default `~/.config/target-editor`).
fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("target-editor"))
}

fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

fn expand_path(s: &str) -> PathBuf {
    if s == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(s));
    }
    PathBuf::from(s)
}

/// Load config.json, creating it with defaults if missing.
/// `Err` means the file exists but is unreadable/broken; it must not be overwritten.
fn load() -> Result<Config, String> {
    let p = path().ok_or("no config dir ($HOME unset)")?;
    match std::fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let c = Config::default();
            save(&c)?;
            Ok(c)
        }
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn save(c: &Config) -> Result<(), String> {
    let p = path().ok_or("no config dir ($HOME unset)")?;
    let json = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    devicehostlib::devices::write_atomic(&p, &json)
}

fn loaded() -> &'static (Config, Option<String>) {
    LOADED.get_or_init(|| match load() {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e)),
    })
}

fn get() -> &'static Config {
    &loaded().0
}

pub fn load_err() -> Option<&'static str> {
    loaded().1.as_deref()
}

pub fn targets_dir() -> PathBuf {
    expand_path(&get().targets_folder)
}
