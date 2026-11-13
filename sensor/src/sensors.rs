//! Four corner piezos (TL, TR, BL, BR): ADC pins, thresholded hit buffer.
//!
//! Logic lives in [`SensorArray`] / [`HitBuffer`] (validator/reader injected) so it can be
//! unit-tested on the host. Device globals: [`init_sensors`], [`poll_sensors`], [`last_values`],
//! [`push_hits`], [`set_threshold`], [`set_name`], [`write_buffer_json`].
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};

use heapless::{LinearMap, String as NameString, Vec};

/// Maximum number of sensors (one per corner).
pub const CAPACITY: usize = 4;
/// Hit-buffer entries (oldest dropped if the 10 s window is fuller than this).
pub const BUFFER_CAP: usize = 64;
/// Rolling window: entries older than this are dropped.
pub const WINDOW_MS: u64 = 10_000;
/// Default ADC threshold for storing a corner reading.
pub const DEFAULT_THRESHOLD: u16 = 2000;
/// Max bytes for [`set_name`] (hub name in each buffer entry). Default is [`crate::PROJECT`] until boot sets the advertised name.
pub const NAME_CAP: usize = 32;

/// Corner piezos, in pin-list order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

impl Corner {
    pub const ALL: [Self; CAPACITY] = [Self::Tl, Self::Tr, Self::Bl, Self::Br];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tl => "TL",
            Self::Tr => "TR",
            Self::Bl => "BL",
            Self::Br => "BR",
        }
    }
}

/// ADC1 GPIOs for TL, TR, BL, BR. Change this list to rewire.
#[cfg(feature = "esp32")]
pub const PIEZO_PINS: [u8; CAPACITY] = [36, 39, 34, 35];
#[cfg(feature = "esp32s2")]
pub const PIEZO_PINS: [u8; CAPACITY] = [1, 2, 3, 4];
#[cfg(feature = "esp32s3")]
pub const PIEZO_PINS: [u8; CAPACITY] = [2, 3, 4, 6];
#[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
pub const PIEZO_PINS: [u8; CAPACITY] = [0, 1, 2, 3];
#[cfg(feature = "esp32h2")]
pub const PIEZO_PINS: [u8; CAPACITY] = [1, 2, 3, 4];
#[cfg(not(any(
    feature = "esp32",
    feature = "esp32s2",
    feature = "esp32s3",
    feature = "esp32c3",
    feature = "esp32c6",
    feature = "esp32h2"
)))]
pub const PIEZO_PINS: [u8; CAPACITY] = [0, 1, 2, 3];

/// One timestamp: TL, TR, BL, BR. Stored only when all four corners are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    values: [u16; CAPACITY],
    complete: bool,
}

impl Sample {
    pub const fn new() -> Self {
        Self {
            values: [0; CAPACITY],
            complete: false,
        }
    }

    pub const fn from_all(values: [u16; CAPACITY]) -> Self {
        Self {
            values,
            complete: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.complete
    }

    pub fn get(&self, c: Corner) -> Option<u16> {
        self.complete.then_some(self.values[c as usize])
    }

    /// Comma-separated `tl,tr,bl,br`.
    fn write_values(&self, out: &mut impl Write) -> fmt::Result {
        for (i, v) in self.values.iter().enumerate() {
            if i > 0 {
                out.write_char(',')?;
            }
            write!(out, "{v}")?;
        }
        Ok(())
    }
}

impl Default for Sample {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorError {
    /// More than [`CAPACITY`] pins were given.
    TooMany { given: usize, capacity: usize },
    /// The pin is not an ADC pin on this chip.
    NotAdcPin(u8),
}

impl core::fmt::Display for SensorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooMany { given, capacity } => write!(f, "{given} sensor pins given, capacity is {capacity}"),
            Self::NotAdcPin(p) => write!(f, "GPIO{p} is not an ADC pin"),
        }
    }
}

/// Invalid [`set_name`] argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong { given: usize, max: usize },
    BadChar(char),
}

impl core::fmt::Display for NameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => f.write_str("empty name"),
            Self::TooLong { given, max } => write!(f, "name is {given} bytes, max is {max}"),
            Self::BadChar(c) => write!(f, "bad name char {c:?}"),
        }
    }
}

fn name_char_ok(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-')
}

/// Fixed-capacity list of sensor pins (index 0 = TL … 3 = BR).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensorArray {
    pins: Vec<u8, CAPACITY>,
}

impl SensorArray {
    pub const fn new() -> Self {
        Self { pins: Vec::new() }
    }

    /// Validate and store `pins` (all or nothing).
    pub fn from_pins(pins: &[u8], is_adc_pin: impl Fn(u8) -> bool) -> Result<Self, SensorError> {
        if pins.len() > CAPACITY {
            return Err(SensorError::TooMany { given: pins.len(), capacity: CAPACITY });
        }
        if let Some(&bad) = pins.iter().find(|&&p| !is_adc_pin(p)) {
            return Err(SensorError::NotAdcPin(bad));
        }
        Ok(Self { pins: Vec::from_slice(pins).expect("length checked") })
    }

    pub fn pins(&self) -> &[u8] {
        &self.pins
    }

    /// Read every configured pin into a fixed `[u16; CAPACITY]` (unset slots stay 0).
    pub fn read_all(&self, mut read: impl FnMut(u8) -> u16) -> [u16; CAPACITY] {
        let mut values = [0u16; CAPACITY];
        for (i, &p) in self.pins.iter().enumerate() {
            values[i] = read(p);
        }
        values
    }

    /// Hit sample from already-read values when all four corners are present and `>= threshold`.
    pub fn hits_from(values: &[u16; CAPACITY], n_pins: usize, threshold: u16) -> Sample {
        if n_pins < CAPACITY {
            return Sample::new();
        }
        if values.iter().any(|&v| v < threshold) {
            return Sample::new();
        }
        Sample::from_all(*values)
    }

    /// Read each configured pin; return a sample only when every corner is `>= threshold`.
    pub fn read_hits(&self, threshold: u16, read: impl FnMut(u8) -> u16) -> Sample {
        let values = self.read_all(read);
        Self::hits_from(&values, self.pins.len(), threshold)
    }
}

/// Timestamp → `[tl, tr, bl, br]`. Keys are unique and non-decreasing on insert.
#[derive(Debug, Clone)]
pub struct HitBuffer {
    map: LinearMap<u64, Sample, BUFFER_CAP>,
    last_t: Option<u64>,
}

impl HitBuffer {
    pub const fn new() -> Self {
        Self { map: LinearMap::new(), last_t: None }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn get(&self, t: u64) -> Option<&Sample> {
        self.map.get(&t)
    }

    fn prune(&mut self, now: u64) {
        let mut stale: Vec<u64, BUFFER_CAP> = Vec::new();
        for (&t, _) in self.map.iter() {
            if now.saturating_sub(t) > WINDOW_MS {
                let _ = stale.push(t);
            }
        }
        for t in stale {
            self.map.remove(&t);
        }
        if let Some(last) = self.last_t
            && self.map.get(&last).is_none()
        {
            self.last_t = self.map.iter().map(|(t, _)| *t).max();
        }
    }

    /// Insert `sample` at `now` (or `last+1` if `now` is not greater). Empty samples are ignored.
    /// Returns the stored timestamp, if any.
    pub fn push(&mut self, now: u64, sample: Sample) -> Option<u64> {
        if sample.is_empty() {
            return None;
        }
        self.prune(now);
        let mut t = now;
        if let Some(p) = self.last_t
            && t <= p
        {
            t = p + 1;
        }
        while self.map.len() >= BUFFER_CAP {
            let oldest = self.map.iter().next().map(|(k, _)| *k);
            match oldest {
                Some(k) => {
                    self.map.remove(&k);
                }
                None => break,
            }
        }
        let _ = self.map.insert(t, sample);
        self.last_t = Some(t);
        Some(t)
    }

    /// JSON map `{"name":[[ms,tl,tr,bl,br],…]}` with timestamps in insertion order.
    pub fn write_json(&mut self, name: &str, now: u64, out: &mut impl Write) -> fmt::Result {
        self.prune(now);
        write!(out, "{{\"{name}\":[")?;
        for (i, (t, sample)) in self.map.iter().enumerate() {
            if i > 0 {
                out.write_char(',')?;
            }
            write!(out, "[{t},")?;
            sample.write_values(out)?;
            out.write_char(']')?;
        }
        out.write_str("]}")
    }
}

impl Default for HitBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------ globals (device / tests)
fn stored() -> SensorArray {
    let n = SENSOR_COUNT.load(Ordering::Acquire).min(CAPACITY);
    let mut pins = Vec::<u8, CAPACITY>::new();
    for slot in &SENSOR_ARRAY[..n] {
        let _ = pins.push(slot.load(Ordering::Relaxed));
    }
    SensorArray { pins }
}

fn store(a: &SensorArray) {
    SENSOR_COUNT.store(0, Ordering::Release);
    for (slot, &p) in SENSOR_ARRAY.iter().zip(a.pins()) {
        slot.store(p, Ordering::Relaxed);
    }
    SENSOR_COUNT.store(a.pins().len(), Ordering::Release);
}

static SENSOR_ARRAY: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
static SENSOR_COUNT: AtomicUsize = AtomicUsize::new(0);
static THRESHOLD: AtomicU32 = AtomicU32::new(DEFAULT_THRESHOLD as u32);
static LAST_VALUES: [AtomicU32; CAPACITY] = [const { AtomicU32::new(0) }; CAPACITY];
static NAME_BUF: [AtomicU8; NAME_CAP] = [const { AtomicU8::new(0) }; NAME_CAP];
static NAME_LEN: AtomicUsize = AtomicUsize::new(0);

#[cfg(any(feature = "hal", feature = "idf"))]
mod buf_sync {
    use core::cell::RefCell;
    use critical_section::Mutex;
    static B: Mutex<RefCell<super::HitBuffer>> = Mutex::new(RefCell::new(super::HitBuffer::new()));
    pub fn with<R>(f: impl FnOnce(&mut super::HitBuffer) -> R) -> R {
        critical_section::with(|cs| f(&mut B.borrow(cs).borrow_mut()))
    }
}
#[cfg(not(any(feature = "hal", feature = "idf")))]
mod buf_sync {
    static B: std::sync::Mutex<super::HitBuffer> = std::sync::Mutex::new(super::HitBuffer::new());
    pub fn with<R>(f: impl FnOnce(&mut super::HitBuffer) -> R) -> R {
        f(&mut B.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Validate `pins` against the chip's ADC1 map and store them (replacing any previous set).
/// On error nothing is changed. Not meant to be called concurrently with itself.
pub fn init_sensors(pins: &[u8]) -> Result<(), SensorError> {
    let a = SensorArray::from_pins(pins, utils::adc::is_adc_pin)?;
    store(&a);
    Ok(())
}

/// Poll every connected piezo, store last ADC values, and return a hit sample when all
/// corners are present and `>= threshold`. One ADC pass per call.
pub fn poll_sensors(threshold: u16) -> Sample {
    let a = stored();
    let values = a.read_all(utils::adc::read);
    for (slot, &v) in LAST_VALUES.iter().zip(values.iter()) {
        slot.store(v as u32, Ordering::Relaxed);
    }
    SensorArray::hits_from(&values, a.pins().len(), threshold)
}

/// Last ADC reading per corner from the most recent [`poll_sensors`] (0 until first poll).
pub fn last_values() -> [u16; CAPACITY] {
    let mut out = [0u16; CAPACITY];
    for (i, slot) in LAST_VALUES.iter().enumerate() {
        out[i] = slot.load(Ordering::Relaxed).min(u16::MAX as u32) as u16;
    }
    out
}

/// Read each stored sensor; sample only when all corners are `>= threshold`.
/// Prefer [`poll_sensors`] in the main loop (also updates [`last_values`]).
pub fn scan_sensors(threshold: u16) -> Sample {
    stored().read_hits(threshold, utils::adc::read)
}

pub fn threshold() -> u16 {
    THRESHOLD.load(Ordering::Relaxed).min(u16::MAX as u32) as u16
}

pub fn set_threshold(v: u16) {
    THRESHOLD.store(v as u32, Ordering::Relaxed);
}

/// Hub name written into each [`write_buffer_json`] entry. Empty stored length means [`crate::PROJECT`]. Firmware boot sets this to the advertised `sensor-xxyyzz` name.
pub fn name() -> NameString<NAME_CAP> {
    let n = NAME_LEN.load(Ordering::Acquire).min(NAME_CAP);
    let mut s = NameString::new();
    if n == 0 {
        let _ = s.push_str(crate::PROJECT);
        return s;
    }
    let mut buf = [0u8; NAME_CAP];
    for (i, slot) in NAME_BUF.iter().take(n).enumerate() {
        buf[i] = slot.load(Ordering::Relaxed);
    }
    match core::str::from_utf8(&buf[..n]) {
        Ok(t) => {
            let _ = s.push_str(t);
        }
        Err(_) => {
            let _ = s.push_str(crate::PROJECT);
        }
    }
    s
}

/// Set the hub name used in buffer JSON. RAM only; BLE advertised name is unchanged.
pub fn set_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.len() > NAME_CAP {
        return Err(NameError::TooLong { given: name.len(), max: NAME_CAP });
    }
    if let Some(c) = name.chars().find(|c| !name_char_ok(*c)) {
        return Err(NameError::BadChar(c));
    }
    NAME_LEN.store(0, Ordering::Release);
    for (slot, &b) in NAME_BUF.iter().zip(name.as_bytes()) {
        slot.store(b, Ordering::Relaxed);
    }
    NAME_LEN.store(name.len(), Ordering::Release);
    Ok(())
}

/// Store a hit sample (empty samples are ignored). Prunes entries older than [`WINDOW_MS`].
pub fn push_hits(now_ms: u64, sample: Sample) {
    buf_sync::with(|b| {
        b.push(now_ms, sample);
    });
}

/// Write the current buffer as a JSON object (after pruning against synced time or uptime).
pub fn write_buffer_json(out: &mut impl Write) {
    let now = now_ms();
    buf_sync::with(|b| {
        let n = name();
        let _ = b.write_json(n.as_str(), now, out);
    });
}

fn now_ms() -> u64 {
    #[cfg(any(feature = "hal", feature = "idf"))]
    {
        utils::synced_time().unwrap_or_else(utils::time::uptime_ms)
    }
    #[cfg(not(any(feature = "hal", feature = "idf")))]
    {
        TEST_NOW.load(Ordering::Relaxed)
    }
}

#[cfg(not(any(feature = "hal", feature = "idf")))]
static TEST_NOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    fn even(p: u8) -> bool {
        p.is_multiple_of(2)
    }

    fn sample(values: [u16; CAPACITY]) -> Sample {
        Sample::from_all(values)
    }

    fn json_of(buf: &mut HitBuffer, now: u64) -> std::string::String {
        let mut s = std::string::String::new();
        buf.write_json(crate::PROJECT, now, &mut s).unwrap();
        s
    }

    #[test]
    fn rejects_over_capacity() {
        assert_eq!(
            SensorArray::from_pins(&[0, 2, 4, 6, 8], even),
            Err(SensorError::TooMany { given: 5, capacity: 4 })
        );
        assert!(SensorArray::from_pins(&[0, 2, 4, 6], even).is_ok());
    }

    #[test]
    fn rejects_non_adc() {
        assert_eq!(SensorArray::from_pins(&[0, 3], even), Err(SensorError::NotAdcPin(3)));
    }

    #[test]
    fn hits_require_all_corners() {
        let a = SensorArray::from_pins(&[0, 2, 4, 6], even).unwrap();
        let vals = |p: u8| [100u16, 0, 50, 0, 300, 0, 200][p as usize];
        // pin2=50 is below threshold 100 → empty
        assert!(a.read_hits(100, vals).is_empty());
        let all = |p: u8| [100u16, 0, 150, 0, 300, 0, 200][p as usize];
        let h = a.read_hits(100, all);
        assert_eq!(h.get(Corner::Tl), Some(100));
        assert_eq!(h.get(Corner::Tr), Some(150));
        assert_eq!(h.get(Corner::Bl), Some(300));
        assert_eq!(h.get(Corner::Br), Some(200));
        assert!(a.read_hits(301, all).is_empty());
        assert!(SensorArray::new().read_hits(0, all).is_empty());
    }

    #[test]
    fn read_all_keeps_every_corner() {
        let a = SensorArray::from_pins(&[0, 2, 4, 6], even).unwrap();
        let vals = |p: u8| [10u16, 0, 20, 0, 30, 0, 40][p as usize];
        // Below-threshold corners are still returned (unlike read_hits).
        assert_eq!(a.read_all(vals), [10, 20, 30, 40]);
        let partial = SensorArray::from_pins(&[0, 2], even).unwrap();
        assert_eq!(partial.read_all(vals), [10, 20, 0, 0]);
    }

    #[test]
    fn hits_from_uses_polled_values() {
        let values = [100u16, 150, 50, 200];
        assert!(SensorArray::hits_from(&values, 4, 100).is_empty());
        let values = [100u16, 150, 300, 200];
        let h = SensorArray::hits_from(&values, 4, 100);
        assert_eq!(h.get(Corner::Tl), Some(100));
        assert_eq!(h.get(Corner::Br), Some(200));
        assert!(SensorArray::hits_from(&values, 3, 0).is_empty());
    }

    #[test]
    fn buffer_skips_empty_and_prunes_window() {
        let mut b = HitBuffer::new();
        assert!(b.push(0, Sample::new()).is_none());
        let t = b.push(1_000, sample([2000, 2000, 2000, 2000])).unwrap();
        assert_eq!(t, 1_000);
        assert_eq!(b.len(), 1);
        b.push(1_000 + WINDOW_MS + 1, sample([2100, 2100, 2100, 2100]));
        assert!(b.get(1_000).is_none());
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn unique_timestamps() {
        let mut b = HitBuffer::new();
        let t0 = b.push(5, sample([1, 2, 3, 4])).unwrap();
        let t1 = b.push(5, sample([5, 6, 7, 8])).unwrap();
        assert_eq!(t0, 5);
        assert_eq!(t1, 6);
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn json_chronological() {
        let mut b = HitBuffer::new();
        b.push(20, sample([9, 9, 9, 9]));
        b.push(10, sample([1, 2, 3, 4]));
        // second insert at 10 becomes 21 because last_t is 20
        assert_eq!(
            json_of(&mut b, 20),
            r#"{"sensor":[[20,9,9,9,9],[21,1,2,3,4]]}"#
        );
    }

    #[test]
    fn json_empty() {
        let mut b = HitBuffer::new();
        assert_eq!(json_of(&mut b, 0), r#"{"sensor":[]}"#);
    }

    fn lock_globals() -> std::sync::MutexGuard<'static, ()> {
        static G: std::sync::Mutex<()> = std::sync::Mutex::new(());
        G.lock().unwrap_or_else(|e| e.into_inner())
    }

    // Global path using utils' host ADC mock (default chip esp32c3: ADC1 = GPIO0..=4).
    #[test]
    fn global_with_mock() {
        let _g = lock_globals();
        utils::adc::mock::reset();
        TEST_NOW.store(0, Ordering::Relaxed);
        buf_sync::with(|b| *b = HitBuffer::new());
        set_threshold(DEFAULT_THRESHOLD);
        set_name(crate::PROJECT).unwrap();

        assert_eq!(init_sensors(&[5]), Err(SensorError::NotAdcPin(5)));
        init_sensors(&[0, 1, 2, 3]).unwrap();
        for (p, v) in [(0, 2000), (1, 100), (2, 2100), (3, 1999)] {
            utils::adc::mock::set(p, v);
        }
        assert!(scan_sensors(2000).is_empty());

        for (p, v) in [(0, 2000), (1, 2100), (2, 2200), (3, 2300)] {
            utils::adc::mock::set(p, v);
        }
        let h = scan_sensors(2000);
        assert_eq!(h.get(Corner::Tl), Some(2000));
        assert_eq!(h.get(Corner::Tr), Some(2100));
        assert_eq!(h.get(Corner::Bl), Some(2200));
        assert_eq!(h.get(Corner::Br), Some(2300));

        TEST_NOW.store(100, Ordering::Relaxed);
        push_hits(100, h);
        let mut j = std::string::String::new();
        write_buffer_json(&mut j);
        assert_eq!(j, r#"{"sensor":[[100,2000,2100,2200,2300]]}"#);

        assert!(init_sensors(&[0, 1, 2, 3, 4]).is_err());
        assert_eq!(stored().pins(), &[0, 1, 2, 3]);
    }

    #[test]
    fn poll_sensors_updates_last_values() {
        let _g = lock_globals();
        utils::adc::mock::reset();
        buf_sync::with(|b| *b = HitBuffer::new());
        set_threshold(DEFAULT_THRESHOLD);
        init_sensors(&[0, 1, 2, 3]).unwrap();

        for (p, v) in [(0, 111), (1, 222), (2, 333), (3, 444)] {
            utils::adc::mock::set(p, v);
        }
        // Below threshold: no hit, but last values still stored.
        assert!(poll_sensors(2000).is_empty());
        assert_eq!(last_values(), [111, 222, 333, 444]);

        for (p, v) in [(0, 2000), (1, 2100), (2, 2200), (3, 2300)] {
            utils::adc::mock::set(p, v);
        }
        let h = poll_sensors(2000);
        assert_eq!(last_values(), [2000, 2100, 2200, 2300]);
        assert_eq!(h.get(Corner::Tl), Some(2000));
        assert_eq!(h.get(Corner::Br), Some(2300));
    }

    #[test]
    fn dispatch_set_threshold_and_read_buffer() {
        let _g = lock_globals();
        buf_sync::with(|b| *b = HitBuffer::new());
        TEST_NOW.store(50, Ordering::Relaxed);
        set_name(crate::PROJECT).unwrap();
        assert_eq!(crate::dispatch("set_threshold", &["1234"]).unwrap(), "1234");
        assert_eq!(threshold(), 1234);
        assert_eq!(crate::dispatch("set_threshold", &[]).unwrap_err(), "need [u16]");
        assert!(crate::dispatch("nope", &[]).unwrap_err().contains("unknown action"));
        push_hits(50, sample([2000, 2001, 2002, 2003]));
        let j = crate::dispatch("read_buffer", &[]).unwrap();
        assert_eq!(j, r#"{"sensor":[[50,2000,2001,2002,2003]]}"#);
        assert_eq!(crate::dispatch("set_name", &["front"]).unwrap(), "front");
        assert_eq!(name().as_str(), "front");
        let j = crate::dispatch("read_buffer", &[]).unwrap();
        assert_eq!(j, r#"{"front":[[50,2000,2001,2002,2003]]}"#);
        assert_eq!(crate::dispatch("set_name", &[]).unwrap_err(), "need [name]");
        assert_eq!(crate::dispatch("set_name", &[""]).unwrap_err(), "empty name");
        assert_eq!(crate::dispatch("set_name", &["bad name"]).unwrap_err(), r#"bad name char ' '"#);
        let long = "a".repeat(NAME_CAP + 1);
        assert_eq!(
            crate::dispatch("set_name", &[&long]).unwrap_err(),
            format!("name is {} bytes, max is {NAME_CAP}", NAME_CAP + 1)
        );
    }

    #[test]
    fn set_name_rejects() {
        let _g = lock_globals();
        set_name(crate::PROJECT).unwrap();
        assert_eq!(set_name(""), Err(NameError::Empty));
        assert_eq!(set_name("no.dots"), Err(NameError::BadChar('.')));
        assert_eq!(
            set_name(&"a".repeat(NAME_CAP + 1)),
            Err(NameError::TooLong { given: NAME_CAP + 1, max: NAME_CAP })
        );
        set_name("ok_1-2").unwrap();
        assert_eq!(name().as_str(), "ok_1-2");
    }
}
