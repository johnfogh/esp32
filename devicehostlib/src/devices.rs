//! Custom device names and device types, persisted to
//! `$XDG_CONFIG_HOME/esp32-controller/devices.json` (default `~/.config/...`).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Devices {
    /// Custom name per device, keyed by factory MAC (from the advert) or else the BLE address.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// Controller ingest filter. Keyed by factory MAC or BLE address. Missing means disabled.
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
    /// Device types stored in devices.json.
    #[serde(default = "default_types")]
    pub device_types: Vec<String>,
}

pub fn default_types() -> Vec<String> {
    vec!["laser".into(), "target sensor".into()]
}

impl Default for Devices {
    fn default() -> Self {
        Self {
            names: BTreeMap::new(),
            enabled: BTreeMap::new(),
            device_types: default_types(),
        }
    }
}

/// `$XDG_CONFIG_HOME/esp32-controller` (default `~/.config/esp32-controller`).
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("esp32-controller"))
}

pub fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("devices.json"))
}

/// Parse devices.json text and apply key/type migrations.
/// Returns `(devices, needs_save)` when the on-disk shape should be rewritten.
pub fn parse_with_migrate(text: &str) -> Result<(Devices, bool), String> {
    let mut v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| e.to_string())?;
    // Migrate the pre-0.8 `types` key to `device_types` (only when the file parses).
    let migrate = v
        .as_object()
        .is_some_and(|o| o.contains_key("types") && !o.contains_key("device_types"));
    if migrate && let Some(o) = v.as_object_mut() && let Some(t) = o.remove("types") {
        o.insert("device_types".into(), t);
    }
    let mut d: Devices = serde_json::from_value(v).map_err(|e| e.to_string())?;
    // Migrate the pre-0.11 `target` type to `target sensor` (order kept, duplicates dropped).
    let renamed = d.device_types.iter().any(|t| t == "target");
    if renamed {
        let mut out: Vec<String> = Vec::new();
        for t in d.device_types.drain(..) {
            let t = if t == "target" {
                "target sensor".to_string()
            } else {
                t
            };
            if !out.contains(&t) {
                out.push(t);
            }
        }
        d.device_types = out;
    }
    Ok((d, migrate || renamed))
}

/// Load devices.json, creating it with defaults if missing.
/// `Err` means the file exists but is unreadable/broken; it must not be overwritten.
pub fn load() -> Result<Devices, String> {
    let p = path().ok_or("no config dir ($HOME unset)")?;
    match std::fs::read_to_string(&p) {
        Ok(text) => {
            let (d, needs_save) =
                parse_with_migrate(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            if needs_save {
                save(&d)?;
            }
            Ok(d)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let d = Devices::default();
            save(&d)?;
            Ok(d)
        }
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

pub fn save(d: &Devices) -> Result<(), String> {
    let p = path().ok_or("no config dir ($HOME unset)")?;
    let json = serde_json::to_string_pretty(d).map_err(|e| e.to_string())?;
    write_atomic(&p, &json)
}

/// Write `text` (plus a newline) to `p` via a temp file + rename, creating parent dirs.
pub fn write_atomic(p: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut tmp = p.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, format!("{text}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_devices_include_laser_and_target_sensor() {
        assert_eq!(default_types(), ["laser", "target sensor"]);
        let d = Devices::default();
        assert!(d.names.is_empty());
        assert!(d.enabled.is_empty());
        assert_eq!(d.device_types, default_types());
    }

    #[test]
    fn write_atomic_creates_parent_and_file() {
        let dir = std::env::temp_dir().join(format!(
            "devicehostlib-devices-{}-{}",
            std::process::id(),
            "atomic"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("devices.json");
        write_atomic(&path, r#"{"ok":true}"#).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\"ok\":true}\n");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn devices_json_roundtrip_preserves_maps() {
        let mut d = Devices::default();
        d.names.insert("aa:bb".into(), "Gate".into());
        d.enabled.insert("aa:bb".into(), true);
        let json = serde_json::to_string_pretty(&d).unwrap();
        let back: Devices = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn parse_with_migrate_rewrites_types_key() {
        let (d, needs_save) = parse_with_migrate(
            r#"{"names":{},"enabled":{},"types":["laser","target sensor"]}"#,
        )
        .unwrap();
        assert!(needs_save);
        assert_eq!(d.device_types, ["laser", "target sensor"]);
    }

    #[test]
    fn parse_with_migrate_renames_target_and_dedupes() {
        let (d, needs_save) = parse_with_migrate(
            r#"{"device_types":["laser","target","target sensor","target"]}"#,
        )
        .unwrap();
        assert!(needs_save);
        assert_eq!(d.device_types, ["laser", "target sensor"]);
    }

    #[test]
    fn parse_with_migrate_noop_on_current_shape() {
        let (d, needs_save) = parse_with_migrate(
            r#"{"names":{},"enabled":{},"device_types":["laser","target sensor"]}"#,
        )
        .unwrap();
        assert!(!needs_save);
        assert_eq!(d.device_types, default_types());
    }

    #[test]
    fn parse_with_migrate_rejects_invalid_json() {
        assert!(parse_with_migrate("{not json").is_err());
    }
}
