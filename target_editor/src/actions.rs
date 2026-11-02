//! Action definitions from `$XDG_CONFIG_HOME/esp32-controller/actions.json`: an object mapping
//! action name -> array of supported device types (file order kept). Loaded once at startup;
//! the app only writes the file when it is missing or in the pre-0.12 format (hand-edited otherwise).

use std::path::{Path, PathBuf};

/// Built-in defaults (also used in memory when the file is broken).
pub const DEFAULTS: [(&str, &[&str]); 13] = [
    ("sleep", &["laser", "target sensor"]),
    ("set_time", &["laser", "target sensor"]),
    ("reboot", &["laser", "target sensor"]),
    ("signal", &["laser"]),
    ("set_score_zone", &[]),
    ("wait_for_hit", &[]),
    ("resolve_hits", &[]),
    ("process_targets", &[]),
    ("clear_score_zones", &[]),
    ("random_delay", &[]),
    ("start_timer", &[]),
    ("timer_expired", &[]),
    ("get_accelerometers", &["laser", "target sensor"]),
];

/// (action name, supported device types), in file order.
type Actions = Vec<(String, Vec<String>)>;

fn defaults() -> Actions {
    DEFAULTS.iter().map(|(n, t)| (n.to_string(), t.iter().map(|s| s.to_string()).collect())).collect()
}

/// Append built-in actions that a hand-edited file omitted, keeping file order.
/// Built-in controller methods (empty type list) stay device-less even if the file listed types.
fn merge_defaults(mut a: Actions) -> Actions {
    for (n, t) in defaults() {
        if let Some((_, types)) = a.iter_mut().find(|(k, _)| k == &n) {
            if t.is_empty() {
                types.clear();
            }
        } else {
            a.push((n, t));
        }
    }
    a
}

fn defaults_json() -> String {
    let mut m = serde_json::Map::new();
    for (n, t) in defaults() {
        m.insert(n, serde_json::Value::from(t));
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default()
}

pub fn path() -> Option<PathBuf> {
    devicehostlib::devices::config_dir().map(|d| d.join("actions.json"))
}

/// Old format: values are arrays of objects (keyed by device type, user actions).
fn is_old_format(v: &serde_json::Value) -> bool {
    v.as_object().is_some_and(|m| {
        !m.is_empty()
            && m.values().any(|x| x.as_array().is_some_and(|a| a.iter().any(|e| e.is_object())))
    })
}

fn parse(v: &serde_json::Value) -> Option<Actions> {
    v.as_object()?
        .iter()
        .map(|(k, x)| {
            let types = x.as_array()?.iter().map(|t| t.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
            Some((k.clone(), types))
        })
        .collect()
}

/// First free backup name: actions.json.bak, then .bak.1, .bak.2, …
fn backup_path(p: &Path) -> PathBuf {
    let base = |s: &str| {
        let mut o = p.as_os_str().to_owned();
        o.push(s);
        PathBuf::from(o)
    };
    let first = base(".bak");
    if !first.exists() {
        return first;
    }
    (1..).map(|i| base(&format!(".bak.{i}"))).find(|c| !c.exists()).unwrap_or(first)
}

fn load_inner() -> (Actions, Option<String>) {
    let Some(p) = path() else { return (defaults(), Some("actions.json: no config dir ($HOME unset)".into())) };
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let err = devicehostlib::devices::write_atomic(&p, &defaults_json()).err().map(|e| format!("actions.json: {e}"));
            return (defaults(), err);
        }
        Err(e) => return (defaults(), Some(format!("{}: {e} (using built-in actions)", p.display()))),
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (defaults(), Some(format!("{}: {e} (using built-in actions)", p.display()))),
    };
    if is_old_format(&v) {
        let bak = backup_path(&p);
        if let Err(e) = std::fs::copy(&p, &bak) {
            return (defaults(), Some(format!("{}: backup failed: {e} (using built-in actions)", bak.display())));
        }
        let err = devicehostlib::devices::write_atomic(&p, &defaults_json()).err().map(|e| format!("actions.json: {e}"));
        return (defaults(), err);
    }
    match parse(&v) {
        Some(a) => (merge_defaults(a), None),
        None => (
            defaults(),
            Some(format!("{}: expected {{\"action\": [\"type\", …]}} (using built-in actions)", p.display())),
        ),
    }
}

/// Load actions.json once at startup. Returns a status-line error, if any.
pub fn init() -> Option<String> {
    load_inner().1
}

