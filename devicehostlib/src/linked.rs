//! Session link state and status-buffer telemetry ingest.
use std::cmp::Ordering;
use std::time::Instant;

use crate::central::DeviceInfo;

/// A device in the Connection Manager session hash (paired and/or enabled).
#[derive(Clone)]
pub struct Linked {
    pub info: DeviceInfo,
    /// Controller ingest filter (card ●). Independent of the OS connection.
    pub enabled: bool,
    /// `Some(since)` while the OS has a GATT session.
    pub since: Option<Instant>,
    /// Last status text while not connected.
    pub retry: Option<String>,
    /// Latest connect failure reason (kept while connecting/waiting).
    pub last_err: Option<String>,
    /// Board clock offset (ms, board uptime − controller clock) and the RTT of that measurement.
    pub offset: Option<(f64, f64)>,
    /// Last board `uptime_ms` from a status-buffer sample, and when it was ingested.
    pub uptime_ms: Option<(u64, Instant)>,
    /// Last chip temperature (°C) from a status-buffer sample.
    pub temp: Option<f32>,
    /// Last battery voltage and percent.
    pub battery: Option<(f32, u8)>,
    /// Last accelerometer reading (g).
    pub accel: Option<(f32, f32, f32)>,
    /// Last hub ADC sample.
    pub sensors: Option<[u16; 4]>,
}

impl Linked {
    pub fn new(info: DeviceInfo, since: Option<Instant>, retry: Option<String>) -> Self {
        Self {
            info,
            enabled: false,
            since,
            retry,
            last_err: None,
            offset: None,
            uptime_ms: None,
            temp: None,
            battery: None,
            accel: None,
            sensors: None,
        }
    }
}

/// Last DeviceLine for a device (raw buffer timestamp key + newest JSON sample).
#[derive(Clone)]
pub struct LastMsg {
    pub ts: String,
    pub sample: serde_json::Value,
}

pub fn virt_ingest_default(d: &DeviceInfo) -> bool {
    d.name.starts_with("virtual-")
}

pub fn parse_sensors(v: &serde_json::Value) -> Option<[u16; 4]> {
    let a = v.get("sensors")?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    Some([
        a[0].as_u64()? as u16,
        a[1].as_u64()? as u16,
        a[2].as_u64()? as u16,
        a[3].as_u64()? as u16,
    ])
}

pub fn cmp_ts_keys(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Greater,
        (Err(_), Ok(_)) => Ordering::Less,
        _ => a.cmp(b),
    }
}

pub fn newest_sample(o: &serde_json::Map<String, serde_json::Value>) -> Option<(&String, &serde_json::Value)> {
    o.iter().max_by(|(a, _), (b, _)| cmp_ts_keys(a, b))
}

pub fn last_sample_from_line(line: &str) -> Option<(String, serde_json::Value)> {
    let (i, v) = line.match_indices(['{', '[']).find_map(|(i, _)| {
        serde_json::from_str::<serde_json::Value>(line[i..].trim_end())
            .ok()
            .filter(|v| v.is_object() || v.is_array())
            .map(|v| (i, v))
    })?;
    let head = &line[..i];
    if head.contains("buffer seq ") {
        let o = v.as_object()?;
        let (k, sample) = newest_sample(o)?;
        Some((k.clone(), sample.clone()))
    } else {
        Some((String::new(), v))
    }
}

pub fn parse_accel(v: &serde_json::Value) -> Option<(f32, f32, f32)> {
    Some((
        v.get("x")?.as_f64()? as f32,
        v.get("y")?.as_f64()? as f32,
        v.get("z")?.as_f64()? as f32,
    ))
}

pub fn ingest_sample(l: &mut Linked, v: &serde_json::Value) {
    if let Some(u) = v.get("uptime_ms").and_then(|x| x.as_u64()) {
        if l.uptime_ms.is_some_and(|(prev, _)| u < prev) {
            return;
        }
        l.uptime_ms = Some((u, Instant::now()));
    }
    if let Some(t) = v.get("temp").and_then(|x| x.as_f64()) {
        l.temp = Some(t as f32);
    }
    if let Some(b) = v.get("battery") {
        let volts = b.get("v").and_then(|x| x.as_f64()).map(|x| x as f32);
        let pct = b.get("pct").and_then(|x| x.as_u64()).map(|x| x as u8);
        if let (Some(volts), Some(pct)) = (volts, pct) {
            l.battery = Some((volts, pct));
        }
    }
    if let Some(a) = v.get("accel").and_then(parse_accel) {
        l.accel = Some(a);
    }
    if let Some(s) = parse_sensors(v) {
        l.sensors = Some(s);
    }
}

/// Pull uptime / temp / battery / accel out of a board log line (status-buffer snapshot or get_accelerometers).
pub fn apply_telemetry(l: &mut Linked, line: &str) {
    let Some((i, v)) = line.match_indices(['{', '[']).find_map(|(i, _)| {
        serde_json::from_str::<serde_json::Value>(line[i..].trim_end())
            .ok()
            .filter(|v| v.is_object() || v.is_array())
            .map(|v| (i, v))
    }) else {
        return;
    };
    let head = &line[..i];
    if head.contains("buffer seq ") {
        if let Some(o) = v.as_object() {
            let mut samples: Vec<_> = o.iter().collect();
            samples.sort_by(|(a, _), (b, _)| cmp_ts_keys(a, b));
            for (_, sample) in samples {
                ingest_sample(l, sample);
            }
        }
    } else if head.contains("get_accelerometers")
        && let Some(a) = parse_accel(&v)
    {
        l.accel = Some(a);
    }
}
