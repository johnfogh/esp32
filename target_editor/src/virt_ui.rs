//! Shared formatters for the GPUI window.
use devicehostlib::sensor::{SENSOR_COUNT, ZoneHit};

pub fn uptime(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

pub fn accel(v: [f32; 3]) -> String {
    format!("{:.2}  {:.2}  {:.2}", v[0], v[1], v[2])
}

pub fn battery(v: f32, pct: u8) -> String {
    format!("{v:.2} V  {pct}%")
}

pub fn temp(v: f32) -> String {
    format!("{v:.1} °C")
}

pub fn zone_label(map: Option<ZoneHit>) -> String {
    match map {
        Some(ZoneHit::Sensor(i)) => format!("S{}", i + 1),
        Some(ZoneHit::Hub(i)) => format!("H{}", i + 1),
        None => "—".into(),
    }
}

pub fn cycle_zone(cur: Option<ZoneHit>, sensors: bool, hub: bool) -> Option<ZoneHit> {
    let mut opts: Vec<Option<ZoneHit>> = Vec::new();
    if sensors {
        for i in 0..SENSOR_COUNT {
            opts.push(Some(ZoneHit::Sensor(i)));
        }
    }
    if hub {
        for i in 0..SENSOR_COUNT {
            opts.push(Some(ZoneHit::Hub(i)));
        }
    }
    opts.push(None);
    if opts.len() == 1 {
        return None;
    }
    let i = opts.iter().position(|x| *x == cur).unwrap_or(opts.len() - 1);
    opts[(i + 1) % opts.len()]
}
