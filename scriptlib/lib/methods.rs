//! Controller-side methods (no device). Invoked from Lua; state is in-memory only.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One Stages card as it exists after `init()` (and UI edits).
#[derive(Clone, Debug, Default)]
pub struct TargetCard {
    pub name: String,
    pub def: String,
    pub zones: Vec<String>,
    pub hub: Option<String>,
    pub hub_name: Option<String>,
}

struct TargetRec {
    def: String,
    zones: Vec<String>,
    hub: Option<String>,
    hub_name: Option<String>,
    /// `target_def` JSON with `image` cleared (geometry only).
    doc: Option<targetlib::JsonDoc>,
    /// Hub samples already seen (`"<ms>"` → `[ms,TL,TR,BL,BR]`).
    events: serde_json::Map<String, serde_json::Value>,
}

/// One hub buffer row: `[ms, TL, TR, BL, BR]` under `sensor_hub_name`.
#[derive(Clone, Debug)]
pub struct HubEntry {
    pub ms: serde_json::Value,
    pub sensor_hub_name: String,
    pub tl: serde_json::Value,
    pub tr: serde_json::Value,
    pub bl: serde_json::Value,
    pub br: serde_json::Value,
}

impl HubEntry {
    fn ms_key(&self) -> String {
        match &self.ms {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Wire / events row: `[ms, TL, TR, BL, BR]`.
    fn to_row(&self) -> serde_json::Value {
        serde_json::json!([self.ms, self.tl, self.tr, self.bl, self.br])
    }

    fn corners(&self) -> [Option<f32>; 4] {
        [
            self.tl.as_f64().map(|v| v as f32),
            self.tr.as_f64().map(|v| v as f32),
            self.bl.as_f64().map(|v| v as f32),
            self.br.as_f64().map(|v| v as f32),
        ]
    }
}

#[derive(Default)]
pub struct State {
    /// Named deadlines for `start_timer` / `timer_expired`.
    deadlines: HashMap<String, Instant>,
    /// Pending Simulate hits for host `record_hit` (target, zone).
    hits: Vec<(String, String)>,
    targets: HashMap<String, TargetRec>,
    /// target name → zone name → active
    active: HashMap<String, HashMap<String, bool>>,
}

pub type Shared = Mutex<State>;

pub fn new_shared() -> Shared {
    Mutex::new(State::default())
}

fn lock(st: &Shared) -> std::sync::MutexGuard<'_, State> {
    st.lock().unwrap_or_else(|e| e.into_inner())
}

/// `MM:SS.mmm` → milliseconds. Minutes may have more than two digits; seconds must be < 60.
pub fn parse_time(s: &str) -> Option<u64> {
    let (m, rest) = s.trim().split_once(':')?;
    let (sec, ms) = rest.split_once('.')?;
    let digits = |x: &str, n: Option<usize>| {
        !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()) && n.is_none_or(|n| x.len() == n)
    };
    if !digits(m, None) || m.len() < 2 || !digits(sec, Some(2)) || !digits(ms, Some(3)) {
        return None;
    }
    let (m, sec, ms): (u64, u64, u64) = (m.parse().ok()?, sec.parse().ok()?, ms.parse().ok()?);
    (sec < 60).then(|| (m * 60 + sec) * 1000 + ms)
}

pub fn fmt_time(ms: u64) -> String {
    format!("{:02}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// Store a named deadline at now + `duration` (`MM:SS.mmm`). Restarts overwrite.
pub fn start_timer(st: &Shared, name: &str, duration: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("start_timer: NAME is empty".into());
    }
    let ms = parse_time(duration).ok_or_else(|| format!("start_timer: bad DURATION {duration:?}"))?;
    lock(st)
        .deadlines
        .insert(name.to_string(), Instant::now() + Duration::from_millis(ms));
    Ok(name.to_string())
}

/// `true` when now is at or past the named deadline.
pub fn timer_expired(st: &Shared, name: &str) -> Result<bool, String> {
    if name.is_empty() {
        return Err("timer_expired: NAME is empty".into());
    }
    match lock(st).deadlines.get(name) {
        Some(&deadline) => Ok(Instant::now() >= deadline),
        None => Err(format!("timer_expired: no timer {name:?}")),
    }
}

pub fn random_delay_stop(min: &str, max: &str, stop: Option<&AtomicBool>) -> Result<String, String> {
    let lo = parse_time(min).ok_or_else(|| format!("random_delay: bad MIN {min:?}"))?;
    let hi = parse_time(max).ok_or_else(|| format!("random_delay: bad MAX {max:?}"))?;
    if lo >= hi {
        return Err("random_delay: MIN must be strictly smaller than MAX".into());
    }
    let span = hi - lo;
    let mix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let ms = lo + mix % span;
    let mut left = ms;
    while left > 0 {
        if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            return Err("stopped".into());
        }
        let step = left.min(50);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    Ok(fmt_time(ms))
}

/// Replace the card list. Empty `zones` means the def was missing or had no names.
pub fn set_targets(st: &Shared, cards: impl IntoIterator<Item = TargetCard>) {
    let mut g = lock(st);
    g.targets.clear();
    g.active.clear();
    for c in cards {
        let flags: HashMap<String, bool> = c.zones.iter().map(|n| (n.clone(), false)).collect();
        g.active.insert(c.name.clone(), flags);
        g.targets.insert(
            c.name,
            TargetRec {
                doc: load_doc(&c.def),
                def: c.def,
                zones: c.zones,
                hub: c.hub,
                hub_name: c.hub_name,
                events: serde_json::Map::new(),
            },
        );
    }
}

fn load_doc(def: &str) -> Option<targetlib::JsonDoc> {
    if def.is_empty() {
        return None;
    }
    let mut doc = targetlib::load(def).ok()?;
    doc.image.clear();
    Some(doc)
}

#[cfg(test)]
pub fn set_target_doc(st: &Shared, name: &str, mut doc: targetlib::JsonDoc) {
    doc.image.clear();
    if let Some(rec) = lock(st).targets.get_mut(name) {
        rec.doc = Some(doc);
    }
}

pub fn get_target(st: &Shared, name: &str) -> Option<TargetCard> {
    let g = lock(st);
    let rec = g.targets.get(name)?;
    Some(TargetCard {
        name: name.to_string(),
        def: rec.def.clone(),
        zones: rec.zones.clone(),
        hub: rec.hub.clone(),
        hub_name: rec.hub_name.clone(),
    })
}

/// Device name/address for `target`'s SENSOR HUB (`hub_name`, else `hub`).
pub fn sensor_device(st: &Shared, method: &str, target: &str) -> Result<String, String> {
    if target.is_empty() {
        return Err(format!("{method}: TARGET is empty"));
    }
    let (hub, hub_name) = {
        let g = lock(st);
        let rec = g
            .targets
            .get(target)
            .ok_or_else(|| format!("{method}: unknown target {target:?}"))?;
        (rec.hub.clone(), rec.hub_name.clone())
    };
    hub_name
        .filter(|s| !s.is_empty())
        .or(hub.filter(|s| !s.is_empty()))
        .ok_or_else(|| format!("{method}: no SENSOR HUB assigned to {target:?}"))
}

/// Seen hub samples for `target` (`"<ms>"` → `[ms,TL,TR,BL,BR]`).
pub fn target_events(st: &Shared, target: &str) -> serde_json::Map<String, serde_json::Value> {
    lock(st)
        .targets
        .get(target)
        .map(|r| r.events.clone())
        .unwrap_or_default()
}

/// Diff hub `read_buffer` JSON against `target.events`. New `ms` keys are stored; returns those
/// rows as `{"name":[[ms,TL,TR,BL,BR],…]}` or `"{}"`.
pub fn merge_hub_events(st: &Shared, target: &str, json: &str) -> Result<String, String> {
    if target.is_empty() {
        return Err("merge_hub_events: TARGET is empty".into());
    }
    let v: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|e| format!("merge_hub_events: bad buffer JSON ({e})"))?;
    let items = parse_hub_entries(&v);
    let mut g = lock(st);
    let rec = g
        .targets
        .get_mut(target)
        .ok_or_else(|| format!("merge_hub_events: unknown target {target:?}"))?;
    let mut neu = Vec::new();
    for entry in items {
        let k = entry.ms_key();
        if rec.events.contains_key(&k) {
            continue;
        }
        let row = entry.to_row();
        rec.events.insert(k, row);
        neu.push(entry);
    }
    entries_to_map_json(&neu).map_err(|e| format!("merge_hub_events: {e}"))
}

fn entries_to_map_json(entries: &[HubEntry]) -> Result<String, serde_json::Error> {
    let mut map = serde_json::Map::new();
    for e in entries {
        let rows = map
            .entry(e.sensor_hub_name.clone())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if let Some(arr) = rows.as_array_mut() {
            arr.push(e.to_row());
        }
    }
    serde_json::to_string(&serde_json::Value::Object(map))
}

/// Parse `{"name":[[ms,TL,TR,BL,BR],…]}` (plus legacy wire forms) into [`HubEntry`] values.
fn parse_hub_entries(v: &serde_json::Value) -> Vec<HubEntry> {
    match v {
        serde_json::Value::Array(items) => items.iter().filter_map(parse_one_entry).collect(),
        serde_json::Value::Object(obj) => parse_name_map(obj),
        _ => Vec::new(),
    }
}

fn parse_one_entry(v: &serde_json::Value) -> Option<HubEntry> {
    if let Some(arr) = v.as_array() {
        // Legacy firmware row: [ms, "name", tl, tr, bl, br]
        if arr.len() < 6 {
            return None;
        }
        let name = arr[1].as_str()?.to_string();
        return Some(HubEntry {
            ms: arr[0].clone(),
            sensor_hub_name: name,
            tl: arr[2].clone(),
            tr: arr[3].clone(),
            bl: arr[4].clone(),
            br: arr[5].clone(),
        });
    }
    let obj = v.as_object()?;
    let ms = obj.get("ms")?.clone();
    let name = obj
        .get("sensor_hub_name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let corner = |upper: &str, lower: &str| {
        obj.get(upper)
            .or_else(|| obj.get(lower))
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    Some(HubEntry {
        ms,
        sensor_hub_name: name,
        tl: corner("TL", "tl"),
        tr: corner("TR", "tr"),
        bl: corner("BL", "bl"),
        br: corner("BR", "br"),
    })
}

/// `{"<hub>":[[ms,TL,TR,BL,BR],…]}` or legacy `{"<hub>":{"<ms>":[TL,TR,BL,BR]}}`.
fn parse_name_map(obj: &serde_json::Map<String, serde_json::Value>) -> Vec<HubEntry> {
    if obj.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (hub, inner) in obj {
        if let Some(rows) = inner.as_array() {
            for row in rows {
                if let Some(e) = entry_from_named_row(hub, row) {
                    out.push(e);
                }
            }
            continue;
        }
        if let Some(m) = inner.as_object() {
            for (ms, sample) in m {
                if let Some(e) = entry_from_legacy_sample(ms, hub, sample) {
                    out.push(e);
                }
            }
        }
    }
    out
}

fn entry_from_named_row(hub: &str, row: &serde_json::Value) -> Option<HubEntry> {
    let arr = row.as_array()?;
    if arr.len() < 5 {
        return None;
    }
    Some(HubEntry {
        ms: arr[0].clone(),
        sensor_hub_name: hub.to_string(),
        tl: arr[1].clone(),
        tr: arr[2].clone(),
        bl: arr[3].clone(),
        br: arr[4].clone(),
    })
}

fn entry_from_legacy_sample(ms: &str, hub: &str, sample: &serde_json::Value) -> Option<HubEntry> {
    let arr = sample.as_array()?;
    let get = |i: usize| arr.get(i).cloned().unwrap_or(serde_json::Value::Null);
    let ms_val = ms
        .parse::<u64>()
        .map(serde_json::Value::from)
        .unwrap_or_else(|_| serde_json::Value::String(ms.to_string()));
    Some(HubEntry {
        ms: ms_val,
        sensor_hub_name: hub.to_string(),
        tl: get(0),
        tr: get(1),
        bl: get(2),
        br: get(3),
    })
}

pub fn set_target_active(st: &Shared, target: &str, on: bool) -> Result<bool, String> {
    const METHOD: &str = "target:active";
    if target.is_empty() {
        return Err(format!("{METHOD}: TARGET is empty"));
    }
    let mut g = lock(st);
    let rec = g
        .targets
        .get(target)
        .ok_or_else(|| format!("{METHOD}: unknown target {target:?}"))?;
    if rec.zones.is_empty() {
        return Err(format!("{METHOD}: could not load target_def {:?}", rec.def));
    }
    let names = rec.zones.clone();
    let flags = g.active.entry(target.to_string()).or_default();
    for n in names {
        flags.insert(n, on);
    }
    Ok(on)
}

pub fn set_zone_active(st: &Shared, target: &str, zone: &str, on: bool) -> Result<bool, String> {
    const METHOD: &str = "zone:active";
    if target.is_empty() {
        return Err(format!("{METHOD}: TARGET is empty"));
    }
    if zone.is_empty() {
        return Err(format!("{METHOD}: ZONE is empty"));
    }
    let mut g = lock(st);
    let rec = g
        .targets
        .get(target)
        .ok_or_else(|| format!("{METHOD}: unknown target {target:?}"))?;
    if !rec.zones.iter().any(|n| n == zone) {
        return Err(format!(
            "{METHOD}: zone {zone:?} is not part of target_def {:?}",
            rec.def
        ));
    }
    g.active
        .entry(target.to_string())
        .or_default()
        .insert(zone.to_string(), on);
    Ok(on)
}

pub fn target_is_active(st: &Shared, target: &str) -> bool {
    lock(st)
        .active
        .get(target)
        .is_some_and(|m| m.values().any(|&v| v))
}

pub fn zone_is_active(st: &Shared, target: &str, zone: &str) -> bool {
    lock(st)
        .active
        .get(target)
        .and_then(|m| m.get(zone).copied())
        .unwrap_or(false)
}

/// Record a Simulate hit on `target`/`zone` for the host.
pub fn record_hit(st: &Shared, target: &str, zone: &str) {
    lock(st).hits.push((target.to_string(), zone.to_string()));
}

fn resolve_zone_for_target(g: &State, target: &str, entry: &HubEntry) -> Result<String, String> {
    let rec = g
        .targets
        .get(target)
        .ok_or_else(|| format!("hits_from_buffer: unknown target {target:?}"))?;
    let doc = rec
        .doc
        .as_ref()
        .ok_or_else(|| format!("hits_from_buffer: could not load target_def {:?}", rec.def))?;
    if doc.length_cm <= 0.0 || doc.width_cm <= 0.0 {
        return Err(format!("hits_from_buffer: target_def {:?} has no size", rec.def));
    }
    let plate = targetlib::Target::from_cm(doc.clone());
    let flags = g.active.get(target);
    let target_active = flags.is_some_and(|m| m.values().any(|&v| v));
    if !target_active {
        return Ok("Miss".into());
    }
    Ok(match plate.locate_hit(entry.corners()) {
        Some((x, y)) => targetlib::hit_zone_if(doc, x, y, |name| {
            flags.and_then(|m| m.get(name).copied()).unwrap_or(false)
        })
        .unwrap_or_else(|| "Miss".into()),
        None => "Miss".into(),
    })
}

/// Zones + corner ADC for samples in a hub hit buffer for one card (`hub_name` key only).
///
/// `buffer` is hub JSON from `read_buffer` (`{"<hub>":[[ms,TL,TR,BL,BR],…]}`).
/// Only the outer key equal to `target.hub_name` is used.
/// Returns `(ms, zone, [TL, TR, BL, BR])` in sample order
/// (`"Miss"` when inactive / unlocatable / no active zone). Empty / unmatched → `[]`.
pub fn hits_from_buffer(
    st: &Shared,
    target: &str,
    buffer: &str,
) -> Result<Vec<(serde_json::Value, String, [serde_json::Value; 4])>, String> {
    if target.is_empty() {
        return Err("hits_from_buffer: TARGET is empty".into());
    }
    let v: serde_json::Value = serde_json::from_str(buffer.trim())
        .map_err(|e| format!("hits_from_buffer: bad buffer JSON ({e})"))?;
    let items = parse_hub_entries(&v);
    let g = lock(st);
    let rec = g
        .targets
        .get(target)
        .ok_or_else(|| format!("hits_from_buffer: unknown target {target:?}"))?;
    let hub_name = rec
        .hub_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("hits_from_buffer: no hub_name on {target:?}"))?;
    let mut out = Vec::new();
    for entry in items {
        if entry.sensor_hub_name != hub_name {
            continue;
        }
        let zone = resolve_zone_for_target(&g, target, &entry)?;
        out.push((
            entry.ms.clone(),
            zone,
            [entry.tl.clone(), entry.tr.clone(), entry.bl.clone(), entry.br.clone()],
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_names(st: &Shared, target: &str) -> Vec<String> {
        let mut v: Vec<String> = lock(st)
            .active
            .get(target)
            .map(|m| m.iter().filter(|(_, on)| **on).map(|(n, _)| n.clone()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn target_and_zone_active() {
        let st = new_shared();
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec!["A Zone Head".into(), "C Zone".into()],
                hub: Some("aa:bb".into()),
                hub_name: Some("virtual-hub-sensor-01".into()),
            }],
        );
        assert_eq!(
            get_target(&st, "Target1").and_then(|t| t.hub),
            Some("aa:bb".into())
        );
        assert_eq!(
            get_target(&st, "Target1").and_then(|t| t.hub_name),
            Some("virtual-hub-sensor-01".into())
        );
        assert!(!target_is_active(&st, "Target1"));
        assert!(!zone_is_active(&st, "Target1", "A Zone Head"));
        assert!(set_target_active(&st, "", true).is_err());
        assert!(set_target_active(&st, "nope", true).is_err());
        assert!(set_zone_active(&st, "Target1", "nope", true).is_err());
        assert!(set_zone_active(&st, "Target1", "", true).is_err());
        assert!(set_target_active(&st, "Target1", true).unwrap());
        assert!(target_is_active(&st, "Target1"));
        assert_eq!(active_names(&st, "Target1"), ["A Zone Head", "C Zone"]);
        assert!(!set_zone_active(&st, "Target1", "C Zone", false).unwrap());
        assert_eq!(active_names(&st, "Target1"), ["A Zone Head"]);
        assert!(target_is_active(&st, "Target1"));
        assert!(set_zone_active(&st, "Target1", "C Zone", true).unwrap());
        assert!(!set_target_active(&st, "Target1", false).unwrap());
        assert!(!target_is_active(&st, "Target1"));
        assert!(active_names(&st, "Target1").is_empty());
        let missing = new_shared();
        set_targets(
            &missing,
            [TargetCard {
                name: "T".into(),
                def: "gone".into(),
                zones: vec![],
                hub: None,
                hub_name: None,
            }],
        );
        assert!(set_target_active(&missing, "T", true).is_err());
        assert!(set_zone_active(&missing, "T", "A", true).is_err());
    }

    #[test]
    fn sensor_device_hub_or_name() {
        let st = new_shared();
        assert_eq!(
            sensor_device(&st, "process_targets", "").unwrap_err(),
            "process_targets: TARGET is empty"
        );
        assert_eq!(
            sensor_device(&st, "process_targets", "Target1").unwrap_err(),
            r#"process_targets: unknown target "Target1""#
        );
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec![],
                hub: None,
                hub_name: None,
            }],
        );
        assert_eq!(
            sensor_device(&st, "process_targets", "Target1").unwrap_err(),
            r#"process_targets: no SENSOR HUB assigned to "Target1""#
        );
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec![],
                hub: None,
                hub_name: Some("virtual-hub-sensor-01".into()),
            }],
        );
        assert_eq!(
            sensor_device(&st, "process_targets", "Target1").unwrap(),
            "virtual-hub-sensor-01"
        );
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec![],
                hub: Some("aa:bb".into()),
                hub_name: None,
            }],
        );
        assert_eq!(sensor_device(&st, "process_targets", "Target1").unwrap(), "aa:bb");
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec![],
                hub: Some("aa:bb".into()),
                hub_name: Some("virtual-hub-sensor-01".into()),
            }],
        );
        assert_eq!(
            sensor_device(&st, "process_targets", "Target1").unwrap(),
            "virtual-hub-sensor-01"
        );
    }

    #[test]
    fn merge_hub_events_unwraps_and_diffs() {
        let st = new_shared();
        assert_eq!(
            merge_hub_events(&st, "", "{}").unwrap_err(),
            "merge_hub_events: TARGET is empty"
        );
        assert!(
            merge_hub_events(&st, "Target1", "{}")
                .unwrap_err()
                .contains("unknown target")
        );
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec![],
                hub: Some("aa:bb".into()),
                hub_name: Some("sensor-000001".into()),
            }],
        );
        assert!(target_events(&st, "Target1").is_empty());
        assert_eq!(
            merge_hub_events(&st, "Target1", r#"{"sensor-000001":[]}"#).unwrap(),
            "{}"
        );
        let first = merge_hub_events(
            &st,
            "Target1",
            r#"{"sensor-000001":[[100,2000,2100,2200,2300],[200,1,2,3,4]]}"#,
        )
        .unwrap();
        let first: serde_json::Value = serde_json::from_str(&first).unwrap();
        let rows = first["sensor-000001"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], 100);
        assert_eq!(rows[0][1], 2000);
        assert_eq!(rows[0][4], 2300);
        assert_eq!(rows[1][0], 200);
        assert_eq!(
            merge_hub_events(
                &st,
                "Target1",
                r#"{"sensor-000001":[[100,2000,2100,2200,2300],[200,1,2,3,4]]}"#
            )
            .unwrap(),
            "{}"
        );
        let third = merge_hub_events(
            &st,
            "Target1",
            r#"{"sensor-000001":{"100":[9,9,9,9],"300":[5,6,7,8]}}"#,
        )
        .unwrap();
        let third: serde_json::Value = serde_json::from_str(&third).unwrap();
        let rows = third["sensor-000001"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], 300);
        assert_eq!(rows[0][1], 5);
        let ev = target_events(&st, "Target1");
        assert_eq!(ev.len(), 3);
        assert_eq!(ev["100"][0], 100);
        assert_eq!(ev["100"][1], 2000);
        assert_eq!(ev["300"][4], 8);
        let plain = merge_hub_events(
            &st,
            "Target1",
            r#"{"hub":[[400,4,4,4,4]]}"#,
        )
        .unwrap();
        let plain: serde_json::Value = serde_json::from_str(&plain).unwrap();
        assert_eq!(plain["hub"][0][0], 400);
        assert!(merge_hub_events(&st, "Target1", "not-json").is_err());
    }

    fn plate_doc() -> targetlib::JsonDoc {
        let sq = |name: &str, x0: f64, y0: f64, x1: f64, y1: f64| targetlib::JsonZone {
            name: name.into(),
            color: None,
            score: 0,
            hidden: false,
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        };
        targetlib::JsonDoc {
            image: String::new(),
            length_cm: 10.0,
            width_cm: 10.0,
            display_units: None,
            zones: vec![sq("outer", 0.0, 0.0, 10.0, 10.0), sq("inner", 3.0, 3.0, 7.0, 7.0)],
        }
    }

    fn sample_buf(ms: u64, name: &str, s: [f32; 4]) -> String {
        serde_json::json!({ name: [[ms, s[0], s[1], s[2], s[3]]] }).to_string()
    }

    #[test]
    fn hits_from_buffer_zones() {
        let st = new_shared();
        assert_eq!(
            hits_from_buffer(&st, "", "{}").unwrap_err(),
            "hits_from_buffer: TARGET is empty"
        );
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec!["outer".into(), "inner".into()],
                hub: None,
                hub_name: Some("hub".into()),
            }],
        );
        set_target_doc(&st, "Target1", plate_doc());
        let plate = targetlib::Target::from_cm(plate_doc());
        let s = plate.simulate_hit(5.0, 5.0, targetlib::ADC_MAX, plate.decay());
        let miss = plate.simulate_hit(20.0, 20.0, targetlib::ADC_MAX, plate.decay());
        let buf = serde_json::json!({
            "hub": [
                [100, s[0], s[1], s[2], s[3]],
                [200, miss[0], miss[1], miss[2], miss[3]],
            ],
            "other": [[300, s[0], s[1], s[2], s[3]]],
        })
        .to_string();
        let miss_pair = |ms: i64, corners: [f32; 4]| {
            (
                serde_json::json!(ms),
                "Miss".to_string(),
                [
                    serde_json::json!(corners[0]),
                    serde_json::json!(corners[1]),
                    serde_json::json!(corners[2]),
                    serde_json::json!(corners[3]),
                ],
            )
        };
        assert_eq!(
            hits_from_buffer(&st, "Target1", &buf).unwrap(),
            vec![miss_pair(100, s), miss_pair(200, miss)]
        );
        set_zone_active(&st, "Target1", "inner", true).unwrap();
        assert_eq!(
            hits_from_buffer(&st, "Target1", &buf).unwrap(),
            vec![
                (
                    serde_json::json!(100),
                    "inner".to_string(),
                    [
                        serde_json::json!(s[0]),
                        serde_json::json!(s[1]),
                        serde_json::json!(s[2]),
                        serde_json::json!(s[3]),
                    ],
                ),
                miss_pair(200, miss),
            ]
        );
        assert!(hits_from_buffer(&st, "Target1", "{}").unwrap().is_empty());
        assert!(hits_from_buffer(&st, "Target1", &sample_buf(1, "other", s))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn hits_from_buffer_anisotropic_cm_plate() {
        // Non-square plate: hub ADC must use from_cm (not image pixels).
        let sq = |name: &str, x0: f64, y0: f64, x1: f64, y1: f64| targetlib::JsonZone {
            name: name.into(),
            color: None,
            score: 0,
            hidden: false,
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        };
        let doc = targetlib::JsonDoc {
            image: String::new(),
            length_cm: 20.0,
            width_cm: 10.0,
            display_units: None,
            zones: vec![
                sq("outer", 0.0, 0.0, 20.0, 10.0),
                sq("head", 8.0, 0.5, 12.0, 2.5),
            ],
        };
        let st = new_shared();
        set_targets(
            &st,
            [TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec!["outer".into(), "head".into()],
                hub: None,
                hub_name: Some("hub".into()),
            }],
        );
        set_target_doc(&st, "Target1", doc.clone());
        set_zone_active(&st, "Target1", "outer", true).unwrap();
        set_zone_active(&st, "Target1", "head", true).unwrap();
        let plate = targetlib::Target::from_cm(doc);
        let s = plate.simulate_hit(10.0, 1.5, targetlib::ADC_MAX, plate.decay());
        let hit = hits_from_buffer(&st, "Target1", &sample_buf(50, "hub", s)).unwrap();
        assert_eq!(hit[0].1, "head");
    }

    #[test]
    fn parse_time_mm_ss_mmm() {
        assert_eq!(parse_time("00:00.500"), Some(500));
        assert_eq!(parse_time("00:01.000"), Some(1000));
        assert_eq!(fmt_time(1500), "00:01.500");
        assert!(parse_time("0:00.000").is_none());
        assert!(parse_time("00:60.000").is_none());
    }

    #[test]
    fn start_timer_and_expired() {
        let st = new_shared();
        assert_eq!(start_timer(&st, "w", "00:05.000").unwrap(), "w");
        assert!(!timer_expired(&st, "w").unwrap());

        assert_eq!(start_timer(&st, "z", "00:00.000").unwrap(), "z");
        assert!(timer_expired(&st, "z").unwrap());

        assert!(start_timer(&st, "", "00:01.000").unwrap_err().contains("empty"));
        assert!(start_timer(&st, "bad", "1:00.000").unwrap_err().contains("DURATION"));
        assert!(timer_expired(&st, "").unwrap_err().contains("empty"));
        assert!(timer_expired(&st, "missing").unwrap_err().contains("no timer"));

        assert_eq!(start_timer(&st, "w", "00:00.000").unwrap(), "w");
        assert!(timer_expired(&st, "w").unwrap());
    }
}
