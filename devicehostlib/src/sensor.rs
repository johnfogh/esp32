//! Virtual BLE boards: four standalone sensors and one four-channel hub.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SENSOR_COUNT: usize = 4;
pub const CHANNELS: usize = 4;
pub const PROJECT: &str = "sensor";
pub const NAME_CAP: usize = 32;
pub const THRESHOLD: u16 = 2000;
const HIT_MS: u64 = 250;
const REST: u16 = 0;
const HIT_ADC: u16 = 3000;
const HIT_WINDOW_MS: u64 = 10_000;
const HIT_BUFFER_CAP: usize = 64;
const G: f32 = 9.80665;
const REST_ACCEL: [f32; 3] = [0.0, 0.0, 1.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Standalone,
    Hub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneHit {
    Sensor(usize),
    Hub(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Off,
    NoAdapter,
    Advertising,
    Connected,
    Authenticated,
}

impl Link {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::NoAdapter => "no adapter",
            Self::Advertising => "advertising",
            Self::Connected => "connecting…",
            Self::Authenticated => "connected",
        }
    }

    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected | Self::Authenticated)
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub index: usize,
    pub kind: Kind,
    pub name: String,
    pub mac: String,
    pub adapter: Option<String>,
    pub adapter_addr: Option<String>,
    pub advertise: bool,
    pub link: Link,
    pub uptime_ms: u64,
    pub temp: f32,
    pub battery_v: f32,
    pub battery_pct: u8,
    pub accel: [f32; 3],
    pub sensors: [u16; CHANNELS],
    pub hit: bool,
    pub error: String,
}

pub fn virt_name(mac: [u8; 6]) -> String {
    utils::ble_proto::device_name(PROJECT, mac).as_str().to_string()
}

fn name_char_ok(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-')
}

impl Snapshot {
    fn new(index: usize, kind: Kind) -> Self {
        let mac = factory_mac(index);
        Self {
            index,
            kind,
            name: virt_name(mac),
            mac: fmt_mac(mac),
            adapter: None,
            adapter_addr: None,
            advertise: true,
            link: Link::Advertising,
            uptime_ms: 0,
            temp: 24.0,
            battery_v: 4.10,
            battery_pct: 89,
            accel: REST_ACCEL,
            sensors: [REST; CHANNELS],
            hit: false,
            error: String::new(),
        }
    }
}

pub struct Board {
    pub started: Instant,
    pub advertise: AtomicBool,
    kind: Kind,
    local: AtomicBool,
    inner: Mutex<Inner>,
}

struct Inner {
    snap: Snapshot,
    rest_accel: [f32; 3],
    hit_accel_until: Option<Instant>,
    hit_until: [Option<Instant>; CHANNELS],
    offset: Option<i64>,
    threshold: u16,
    sensor_name: String,
    hits: HitBuffer,
}

/// Firmware-shaped 10 s corner hit buffer (`{"<name>":[[ms,tl,tr,bl,br],…]}`).
struct HitBuffer {
    map: HashMap<u64, [u16; CHANNELS]>,
    last_t: Option<u64>,
}

impl HitBuffer {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            last_t: None,
        }
    }

    fn prune(&mut self, now: u64) {
        self.map.retain(|&t, _| now.saturating_sub(t) <= HIT_WINDOW_MS);
        if let Some(last) = self.last_t
            && !self.map.contains_key(&last)
        {
            self.last_t = self.map.keys().copied().max();
        }
    }

    fn push(&mut self, now: u64, sample: [u16; CHANNELS]) -> Option<u64> {
        self.prune(now);
        let mut t = now;
        if let Some(p) = self.last_t
            && t <= p
        {
            t = p + 1;
        }
        while self.map.len() >= HIT_BUFFER_CAP {
            match self.map.keys().copied().min() {
                Some(oldest) => {
                    self.map.remove(&oldest);
                }
                None => break,
            }
        }
        self.map.insert(t, sample);
        self.last_t = Some(t);
        Some(t)
    }

    fn write_rows(&mut self, now: u64, out: &mut String) {
        self.prune(now);
        out.push('[');
        let mut keys: Vec<u64> = self.map.keys().copied().collect();
        keys.sort_unstable();
        for (i, t) in keys.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let sample = &self.map[t];
            out.push_str(&format!(
                "[{t},{},{},{},{}]",
                sample[0], sample[1], sample[2], sample[3]
            ));
        }
        out.push(']');
    }

    fn write_json(&mut self, name: &str, now: u64, out: &mut String) {
        out.push_str("{\"");
        out.push_str(name);
        out.push_str("\":");
        self.write_rows(now, out);
        out.push('}');
    }
}

impl Board {
    fn new(index: usize, kind: Kind) -> Arc<Self> {
        let snap = Snapshot::new(index, kind);
        let sensor_name = snap.name.clone();
        Arc::new(Self {
            started: Instant::now(),
            advertise: AtomicBool::new(true),
            kind,
            local: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                snap,
                rest_accel: REST_ACCEL,
                hit_accel_until: None,
                hit_until: [None; CHANNELS],
                offset: None,
                threshold: THRESHOLD,
                sensor_name,
                hits: HitBuffer::new(),
            }),
        })
    }

    pub fn is_hub(&self) -> bool {
        self.kind == Kind::Hub
    }

    pub fn name(&self) -> String {
        self.with(|i| i.snap.name.clone())
    }

    pub fn set_name(&self, name: String) {
        self.with(|i| {
            i.sensor_name = name.clone();
            i.snap.name = name;
        });
    }

    pub fn sensor_name(&self) -> String {
        self.with(|i| i.sensor_name.clone())
    }

    /// JSON outer key for [`read_buffer_json`]. RAM only; advertised name is unchanged.
    pub fn set_sensor_name(&self, name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("empty name".into());
        }
        if name.len() > NAME_CAP {
            return Err(format!("name is {} bytes, max is {NAME_CAP}", name.len()));
        }
        if let Some(c) = name.chars().find(|c| !name_char_ok(*c)) {
            return Err(format!("bad name char {c:?}"));
        }
        self.with(|i| i.sensor_name = name.to_string());
        Ok(())
    }

    pub fn advertise(&self) -> bool {
        self.advertise.load(Ordering::Relaxed)
    }

    pub fn set_advertise(&self, on: bool) {
        self.advertise.store(on, Ordering::Relaxed);
        self.with(|i| {
            i.snap.advertise = on;
            if !on && i.snap.link != Link::NoAdapter {
                i.snap.link = Link::Off;
            }
        });
    }

    pub fn snapshot(&self) -> Snapshot {
        self.with(|i| {
            expire_hits(i);
            i.snap.uptime_ms = self.started.elapsed().as_millis() as u64;
            i.snap.clone()
        })
    }

    pub fn set_adapter(&self, name: Option<String>, addr: Option<String>) {
        let local = self.local.load(Ordering::Relaxed);
        self.with(|i| {
            i.snap.adapter = name;
            i.snap.adapter_addr = addr;
            if local {
                if i.snap.adapter.is_some()
                    && matches!(i.snap.link, Link::NoAdapter | Link::Advertising | Link::Off)
                {
                    i.snap.link = Link::Authenticated;
                }
                return;
            }
            if i.snap.adapter.is_none() {
                if i.snap.link == Link::NoAdapter {
                    i.snap.link = if i.snap.advertise { Link::Advertising } else { Link::Off };
                }
            } else if i.snap.link == Link::NoAdapter {
                i.snap.link = if i.snap.advertise { Link::Advertising } else { Link::Off };
            }
        });
    }

    pub fn set_link(&self, link: Link) {
        let local = self.local.load(Ordering::Relaxed);
        self.with(|i| {
            if local {
                if matches!(link, Link::Authenticated | Link::Connected) {
                    i.snap.link = link;
                }
                return;
            }
            if i.snap.adapter.is_none() {
                if !matches!(i.snap.link, Link::Advertising | Link::Off) {
                    i.snap.link = Link::NoAdapter;
                }
                return;
            }
            i.snap.link = link;
        });
    }

    pub fn set_error(&self, e: impl Into<String>) {
        self.with(|i| i.snap.error = e.into());
    }

    pub fn try_local(&self) -> bool {
        !self.local.swap(true, Ordering::SeqCst)
    }

    pub fn end_local(&self) {
        self.local.store(false, Ordering::SeqCst);
        self.with(|i| {
            if matches!(i.snap.link, Link::Connected | Link::Authenticated) {
                i.snap.link = if i.snap.advertise {
                    Link::Advertising
                } else {
                    Link::Off
                };
            }
        });
    }

    pub fn hit_accel(&self) {
        if self.kind != Kind::Standalone {
            return;
        }
        self.with(|i| {
            i.hit_accel_until = Some(Instant::now() + Duration::from_millis(HIT_MS));
            i.snap.accel = [
                i.rest_accel[0],
                i.rest_accel[1],
                (i.rest_accel[2] + 3.0).clamp(-8.0, 8.0),
            ];
            i.snap.hit = true;
        });
    }

    pub fn hit_channel(&self, channel: usize) {
        if self.kind != Kind::Hub || channel >= CHANNELS {
            return;
        }
        self.with(|i| {
            i.hit_until[channel] = Some(Instant::now() + Duration::from_millis(HIT_MS));
            i.snap.sensors[channel] = HIT_ADC;
            i.snap.hit = true;
            // Single-corner pulse does not meet the all-four threshold gate; UI only.
        });
    }

    pub fn hit_sensors(&self, readings: [u16; CHANNELS]) {
        if self.kind != Kind::Hub {
            return;
        }
        self.with(|i| {
            let until = Instant::now() + Duration::from_millis(HIT_MS);
            for ch in 0..CHANNELS {
                i.hit_until[ch] = Some(until);
                i.snap.sensors[ch] = readings[ch];
            }
            i.snap.hit = true;
            if readings.iter().all(|&v| v >= i.threshold) {
                record_hit(i, self.now_ms_locked(i), readings);
            }
        });
    }

    pub fn set_threshold(&self, v: u16) {
        self.with(|i| i.threshold = v);
    }

    pub fn read_buffer_json(&self) -> String {
        self.with(|i| {
            let now = self.now_ms_locked(i);
            let mut s = String::new();
            if self.kind == Kind::Hub {
                i.hits.write_json(&i.sensor_name, now, &mut s);
            } else {
                s.push_str("{}");
            }
            s
        })
    }

    fn now_ms_locked(&self, i: &Inner) -> u64 {
        let up = self.started.elapsed().as_millis() as u64;
        match i.offset {
            Some(o) => (up as i64 + o).max(0) as u64,
            None => up,
        }
    }

    pub fn telemetry(&self) -> Telemetry {
        self.with(|i| {
            expire_hits(i);
            Telemetry {
                uptime_ms: self.started.elapsed().as_millis() as u64,
                temp: i.snap.temp,
                battery_v: i.snap.battery_v,
                battery_pct: i.snap.battery_pct,
                accel: i.snap.accel,
                offset: i.offset,
            }
        })
    }

    pub fn set_time(&self, controller_ms: u64) {
        let up = self.started.elapsed().as_millis() as i64;
        self.with(|i| i.offset = Some(controller_ms as i64 - up));
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

fn record_hit(i: &mut Inner, now: u64, sample: [u16; CHANNELS]) {
    i.hits.push(now, sample);
}

fn expire_hits(i: &mut Inner) {
    let now = Instant::now();
    if i.hit_accel_until.is_some_and(|t| now >= t) {
        i.hit_accel_until = None;
        i.snap.accel = i.rest_accel;
    }
    for ch in 0..CHANNELS {
        if i.hit_until[ch].is_some_and(|t| now >= t) {
            i.hit_until[ch] = None;
            i.snap.sensors[ch] = REST;
        }
    }
    i.snap.hit = i.hit_accel_until.is_some() || i.hit_until.iter().any(|t| t.is_some());
}

#[derive(Clone, Copy)]
pub struct Telemetry {
    pub uptime_ms: u64,
    pub temp: f32,
    pub battery_v: f32,
    pub battery_pct: u8,
    pub accel: [f32; 3],
    pub offset: Option<i64>,
}

impl Telemetry {
    pub fn accel_unit(&self, unit: AccelUnit) -> [f32; 3] {
        match unit {
            AccelUnit::G => self.accel,
            AccelUnit::Ms2 => self.accel.map(|v| v * G),
            AccelUnit::Raw => self.accel.map(|v| (v * 8192.0).round()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccelUnit {
    G,
    Ms2,
    Raw,
}

impl AccelUnit {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "" | "g" => Some(Self::G),
            "m/s²" | "m/s2" | "ms2" => Some(Self::Ms2),
            "raw" => Some(Self::Raw),
            _ => None,
        }
    }
}

pub struct Hub {
    pub sensors: [Arc<Board>; SENSOR_COUNT],
    pub hub: Arc<Board>,
    extras: Mutex<Vec<Arc<Board>>>,
    sensors_on: AtomicBool,
    hub_on: AtomicBool,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sensors: std::array::from_fn(|i| Board::new(i + 1, Kind::Standalone)),
            hub: Board::new(0, Kind::Hub),
            extras: Mutex::new(Vec::new()),
            sensors_on: AtomicBool::new(true),
            hub_on: AtomicBool::new(true),
        })
    }

    pub fn sensors_on(&self) -> bool {
        self.sensors_on.load(Ordering::Relaxed)
    }

    pub fn hub_on(&self) -> bool {
        self.hub_on.load(Ordering::Relaxed)
    }

    pub fn set_sensors(&self, on: bool) {
        if on && self.extra_owns_factory_sensors() {
            return;
        }
        self.sensors_on.store(on, Ordering::Relaxed);
        for b in &self.sensors {
            b.set_advertise(on);
            if !on {
                b.set_adapter(None, None);
            }
        }
    }

    pub fn set_hub(&self, on: bool) {
        if on && self.extra_owns_index(0) {
            return;
        }
        self.hub_on.store(on, Ordering::Relaxed);
        self.hub.set_advertise(on);
        if !on {
            self.hub.set_adapter(None, None);
        }
    }

    pub fn live_boards(&self) -> Vec<Arc<Board>> {
        let mut v = Vec::new();
        if self.sensors_on() {
            v.extend(self.sensors.iter().cloned());
        }
        if self.hub_on() {
            v.push(self.hub.clone());
        }
        v.extend(
            self.extras
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned(),
        );
        v
    }

    pub fn all_boards(&self) -> Vec<Arc<Board>> {
        let mut v: Vec<_> = self.sensors.iter().cloned().collect();
        v.push(self.hub.clone());
        v.extend(
            self.extras
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned(),
        );
        v
    }

    fn extras_locked(&self) -> std::sync::MutexGuard<'_, Vec<Arc<Board>>> {
        self.extras.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn extra_owns_index(&self, index: usize) -> bool {
        self.extras_locked()
            .iter()
            .any(|b| b.snapshot().index == index)
    }

    fn extra_owns_factory_sensors(&self) -> bool {
        (1..=SENSOR_COUNT).any(|i| self.extra_owns_index(i))
    }

    fn next_free_index(&self) -> usize {
        let mut used = HashSet::new();
        if self.hub_on() {
            used.insert(0);
        }
        if self.sensors_on() {
            used.extend(1..=SENSOR_COUNT);
        }
        used.extend(self.extras_locked().iter().map(|b| b.snapshot().index));
        (0..).find(|i| !used.contains(i)).expect("index")
    }

    pub fn add(&self, kind: Kind) -> Arc<Board> {
        let b = Board::new(self.next_free_index(), kind);
        self.extras_locked().push(b.clone());
        b
    }

    pub fn remove(&self, mac: &str) -> bool {
        let mut extras = self.extras.lock().unwrap_or_else(|e| e.into_inner());
        let n = extras.len();
        extras.retain(|b| !b.snapshot().mac.eq_ignore_ascii_case(mac));
        extras.len() != n
    }

    pub fn manager_boards(&self) -> Vec<Arc<Board>> {
        self.extras
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn board_by_mac(&self, mac: &str) -> Option<Arc<Board>> {
        let hit = |b: &Arc<Board>| b.snapshot().mac.eq_ignore_ascii_case(mac);
        self.manager_boards()
            .into_iter()
            .find(hit)
            .or_else(|| self.live_boards().into_iter().find(hit))
            .or_else(|| self.all_boards().into_iter().find(hit))
    }
}

/// Locally administered factory MACs `02:00:00:00:00:0N` (1-based).
pub fn factory_mac(index: usize) -> [u8; 6] {
    [0x02, 0x00, 0x00, 0x00, 0x00, (index as u8) + 1]
}

pub fn fmt_mac(m: [u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_sensors_sets_channels() {
        let hub = Hub::new();
        hub.hub.hit_sensors([4095, 3000, 2500, 2000]);
        let s = hub.hub.snapshot();
        assert_eq!(s.sensors, [4095, 3000, 2500, 2000]);
        assert!(s.hit);
        let j = hub.hub.read_buffer_json();
        assert!(j.starts_with(r#"{"sensor-000001":["#), "{j}");
        // row is [ms,TL,TR,BL,BR]
        assert!(j.contains(",4095,3000,2500,2000]"), "{j}");
    }

    #[test]
    fn hit_sensors_skips_standalone() {
        let hub = Hub::new();
        hub.sensors[0].hit_sensors([1, 2, 3, 4]);
        let s = hub.sensors[0].snapshot();
        assert_eq!(s.sensors, [REST; CHANNELS]);
        assert!(!s.hit);
        assert_eq!(hub.sensors[0].read_buffer_json(), "{}");
    }

    #[test]
    fn hit_buffer_skips_partial_threshold() {
        let hub = Hub::new();
        hub.hub.set_threshold(3000);
        hub.hub.hit_sensors([4095, 3000, 2500, 2000]);
        assert_eq!(hub.hub.read_buffer_json(), r#"{"sensor-000001":[]}"#);
        hub.hub.hit_sensors([4095, 3000, 3100, 3200]);
        let j = hub.hub.read_buffer_json();
        assert!(j.starts_with(r#"{"sensor-000001":["#), "{j}");
        assert!(j.contains(",4095,3000,3100,3200]"), "{j}");
    }

    #[test]
    fn hit_buffer_skips_empty() {
        let hub = Hub::new();
        hub.hub.hit_sensors([0, 0, 0, 0]);
        assert_eq!(hub.hub.read_buffer_json(), r#"{"sensor-000001":[]}"#);
    }

    #[test]
    fn set_sensor_name_wraps_buffer() {
        let hub = Hub::new();
        hub.hub.set_sensor_name("front").unwrap();
        hub.hub.hit_sensors([2000, 2000, 2000, 2000]);
        let j = hub.hub.read_buffer_json();
        assert!(j.starts_with(r#"{"front":["#), "{j}");
        assert!(j.contains(",2000,2000,2000,2000]"), "{j}");
        assert_eq!(hub.hub.set_sensor_name("").unwrap_err(), "empty name");
        assert_eq!(
            hub.hub.set_sensor_name("bad name").unwrap_err(),
            r#"bad name char ' '"#
        );
        let long = "a".repeat(NAME_CAP + 1);
        assert_eq!(
            hub.hub.set_sensor_name(&long).unwrap_err(),
            format!("name is {} bytes, max is {NAME_CAP}", NAME_CAP + 1)
        );
    }

    #[test]
    fn board_name_matches_firmware_device_name() {
        let hub = Hub::new();
        let mac = factory_mac(0);
        assert_eq!(hub.hub.name(), virt_name(mac));
        assert_eq!(hub.hub.name(), "sensor-000001");
        assert_eq!(hub.hub.sensor_name(), "sensor-000001");
        assert_eq!(hub.sensors[0].name(), "sensor-000002");
        let extra = hub.add(Kind::Hub);
        assert_eq!(extra.name(), "sensor-000006");
        let extra_s = hub.add(Kind::Standalone);
        assert_eq!(extra_s.name(), "sensor-000007");
        hub.hub.set_name("front".into());
        assert_eq!(hub.hub.name(), "front");
        assert_eq!(hub.hub.sensor_name(), "front");
        assert_eq!(hub.hub.read_buffer_json(), r#"{"front":[]}"#);
    }

    #[test]
    fn extras_start_at_one_when_factory_off() {
        let hub = Hub::new();
        hub.set_sensors(false);
        hub.set_hub(false);
        let a = hub.add(Kind::Hub);
        assert_eq!(a.name(), "sensor-000001");
        let b = hub.add(Kind::Standalone);
        assert_eq!(b.name(), "sensor-000002");
        hub.set_hub(true);
        assert!(!hub.hub_on());
        hub.set_sensors(true);
        assert!(!hub.sensors_on());
        assert_eq!(hub.board_by_mac(&a.snapshot().mac).unwrap().name(), a.name());
    }

    #[test]
    fn default_hubs_from_settings_start_at_one() {
        // Mirrors Host::ensure_hub when default_virtual_sensor_hubs > 0.
        let hub = Hub::new();
        hub.set_sensors(false);
        hub.set_hub(false);
        let a = hub.add(Kind::Hub);
        let b = hub.add(Kind::Hub);
        assert_eq!(a.name(), "sensor-000001");
        assert_eq!(b.name(), "sensor-000002");
    }
}
