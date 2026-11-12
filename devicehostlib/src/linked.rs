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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::central::DeviceInfo;
    use serde_json::json;
    use std::cmp::Ordering;

    fn info(name: &str) -> DeviceInfo {
        DeviceInfo {
            addr: "AA:BB".into(),
            name: name.into(),
            project: "sensor".into(),
            mac: "aa:bb:cc:dd:ee:ff".into(),
            rssi: None,
        }
    }

    #[test]
    fn virt_ingest_default_matches_virtual_prefix() {
        assert!(virt_ingest_default(&info("virtual-1")));
        assert!(!virt_ingest_default(&info("sensor-1")));
    }

    #[test]
    fn parse_sensors_requires_four_u16() {
        assert_eq!(
            parse_sensors(&json!({"sensors": [1, 2, 3, 4]})),
            Some([1, 2, 3, 4])
        );
        assert_eq!(parse_sensors(&json!({"sensors": [1, 2, 3]})), None);
        assert_eq!(parse_sensors(&json!({"sensors": [1, 2, 3, -1]})), None);
        assert_eq!(parse_sensors(&json!({})), None);
    }

    #[test]
    fn cmp_ts_keys_orders_numeric_then_lexicographic() {
        assert_eq!(cmp_ts_keys("9", "10"), Ordering::Less);
        assert_eq!(cmp_ts_keys("10", "9"), Ordering::Greater);
        assert_eq!(cmp_ts_keys("10", "10"), Ordering::Equal);
        assert_eq!(cmp_ts_keys("10", "x"), Ordering::Greater);
        assert_eq!(cmp_ts_keys("x", "10"), Ordering::Less);
        assert_eq!(cmp_ts_keys("b", "a"), Ordering::Greater);
    }

    #[test]
    fn newest_sample_picks_highest_numeric_key() {
        let o = serde_json::Map::from_iter([
            ("9".into(), json!({"uptime_ms": 1})),
            ("10".into(), json!({"uptime_ms": 2})),
            ("8".into(), json!({"uptime_ms": 0})),
        ]);
        let (k, v) = newest_sample(&o).unwrap();
        assert_eq!(k, "10");
        assert_eq!(v.get("uptime_ms").and_then(|x| x.as_u64()), Some(2));
    }

    #[test]
    fn last_sample_from_line_buffer_and_plain() {
        let (ts, sample) = last_sample_from_line(
            r#"board buffer seq 3 {"9":{"uptime_ms":1},"11":{"uptime_ms":2}}"#,
        )
        .unwrap();
        assert_eq!(ts, "11");
        assert_eq!(sample.get("uptime_ms").and_then(|x| x.as_u64()), Some(2));

        let (ts, sample) = last_sample_from_line(r#"noise {"x":1.0,"y":2.0,"z":3.0}"#).unwrap();
        assert!(ts.is_empty());
        assert_eq!(sample.get("x").and_then(|x| x.as_f64()), Some(1.0));

        assert!(last_sample_from_line("no json here").is_none());
    }

    #[test]
    fn parse_accel_reads_xyz() {
        assert_eq!(
            parse_accel(&json!({"x": 1.5, "y": -0.5, "z": 9.8})),
            Some((1.5, -0.5, 9.8))
        );
        assert_eq!(parse_accel(&json!({"x": 1.0, "y": 2.0})), None);
    }

    #[test]
    fn ingest_sample_updates_fields_and_rejects_older_uptime() {
        let mut l = Linked::new(info("sensor-1"), None, None);
        ingest_sample(
            &mut l,
            &json!({
                "uptime_ms": 100,
                "temp": 21.5,
                "battery": {"v": 3.7, "pct": 88},
                "accel": {"x": 0.1, "y": 0.2, "z": 0.3},
                "sensors": [10, 20, 30, 40]
            }),
        );
        assert_eq!(l.uptime_ms.map(|(u, _)| u), Some(100));
        assert_eq!(l.temp, Some(21.5));
        assert_eq!(l.battery, Some((3.7, 88)));
        assert_eq!(l.accel, Some((0.1, 0.2, 0.3)));
        assert_eq!(l.sensors, Some([10, 20, 30, 40]));

        ingest_sample(&mut l, &json!({"uptime_ms": 50, "temp": 99.0}));
        assert_eq!(l.uptime_ms.map(|(u, _)| u), Some(100));
        assert_eq!(l.temp, Some(21.5));

        ingest_sample(&mut l, &json!({"uptime_ms": 150, "temp": 22.0}));
        assert_eq!(l.uptime_ms.map(|(u, _)| u), Some(150));
        assert_eq!(l.temp, Some(22.0));
    }

    #[test]
    fn apply_telemetry_buffer_and_accel_lines() {
        let mut l = Linked::new(info("sensor-1"), None, None);
        apply_telemetry(
            &mut l,
            r#"status buffer seq 1 {"5":{"uptime_ms":5,"temp":10.0},"6":{"uptime_ms":6,"temp":11.0}}"#,
        );
        assert_eq!(l.uptime_ms.map(|(u, _)| u), Some(6));
        assert_eq!(l.temp, Some(11.0));

        apply_telemetry(
            &mut l,
            r#"get_accelerometers ok {"x":1.0,"y":2.0,"z":3.0}"#,
        );
        assert_eq!(l.accel, Some((1.0, 2.0, 3.0)));

        apply_telemetry(&mut l, "ignored line");
        assert_eq!(l.accel, Some((1.0, 2.0, 3.0)));
    }
}
