//! Board output over BLE: a bounded, non-blocking line buffer drained by the BLE announce thread
//! (see [`crate::bluetooth_announce_background`]) and sent as notifications on
//! [`crate::ble_proto::LOG_CHAR_UUID`] to the authenticated manager.
//!
//! - [`ble_log`] queues one line (a `\n` is appended). It never blocks: the line is dropped when no
//!   manager is connected, when the buffer ([`CAPACITY`] bytes) is full, or when the buffer is busy.
//! - With the `ble` feature, [`crate::init`] installs a logger that mirrors every `log` record
//!   (`log::info!` …) here as `<LEVEL> <target> - <message>`, in addition to the serial console.
use core::sync::atomic::{AtomicBool, Ordering};

/// Buffer size in bytes.
pub const CAPACITY: usize = 8192;
/// Longest mirrored line (longer lines are truncated).
pub const LINE_MAX: usize = 240;

static CONNECTED: AtomicBool = AtomicBool::new(false);
static ADVERTISING: AtomicBool = AtomicBool::new(false);
// Link info for the status screen (load/store only: esp32c3 has no atomic RMW).
static PEER_LO: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static PEER_HI: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// RSSI as `i8` bits; 127 = unknown.
static RSSI: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(127);
/// Uptime (ms) of the last manager disconnect + 1; 0 = never.
static LAST_DISC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Recent log lines kept for the status screen (`display` feature).
pub const RECENT_LINES: usize = 10;
/// Bytes kept per recent line.
pub const RECENT_WIDTH: usize = 40;

#[cfg_attr(not(all(feature = "display", feature = "hal")), allow(dead_code))]
pub(crate) struct Recent {
    lines: [[u8; RECENT_WIDTH]; RECENT_LINES],
    lens: [u8; RECENT_LINES],
    next: usize,
    count: usize,
}

#[cfg_attr(not(all(feature = "display", feature = "hal")), allow(dead_code))]
impl Recent {
    const fn new() -> Self { Self { lines: [[0; RECENT_WIDTH]; RECENT_LINES], lens: [0; RECENT_LINES], next: 0, count: 0 } }
    fn push(&mut self, l: &[u8]) {
        let n = l.len().min(RECENT_WIDTH);
        self.lines[self.next][..n].copy_from_slice(&l[..n]);
        self.lens[self.next] = n as u8;
        self.next = (self.next + 1) % RECENT_LINES;
        self.count = (self.count + 1).min(RECENT_LINES);
    }
}

/// Copy the recent lines, oldest first, into `out`; returns how many.
#[cfg(all(feature = "display", feature = "hal"))]
pub fn recent(out: &mut [[u8; RECENT_WIDTH]; RECENT_LINES], lens: &mut [usize; RECENT_LINES]) -> usize {
    sync::with_recent(|r| {
        let start = (r.next + RECENT_LINES - r.count) % RECENT_LINES;
        for i in 0..r.count {
            let j = (start + i) % RECENT_LINES;
            out[i] = r.lines[j];
            lens[i] = r.lens[j] as usize;
        }
        r.count
    })
    .unwrap_or(0)
}

/// BLE announce state for the status screen.
pub fn advertising() -> bool { ADVERTISING.load(Ordering::Relaxed) }
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn set_advertising(on: bool) { ADVERTISING.store(on, Ordering::Relaxed) }

pub(crate) struct Ring {
    buf: [u8; CAPACITY],
    head: usize,
    len: usize,
}

#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
impl Ring {
    const fn new() -> Self { Self { buf: [0; CAPACITY], head: 0, len: 0 } }
    fn push_line(&mut self, a: &[u8], b: &[u8]) -> bool {
        let n = a.len() + b.len() + 1;
        if n > CAPACITY - self.len { return false; }
        for &c in a.iter().chain(b).chain(b"\n") {
            self.buf[(self.head + self.len) % CAPACITY] = c;
            self.len += 1;
        }
        true
    }
    fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for o in out[..n].iter_mut() {
            *o = self.buf[self.head];
            self.head = (self.head + 1) % CAPACITY;
        }
        self.len -= n;
        n
    }
    fn clear(&mut self) { self.head = 0; self.len = 0; }
}

#[cfg(feature = "hal")]
mod sync {
    use core::cell::RefCell;
    use critical_section::Mutex;
    static RING: Mutex<RefCell<super::Ring>> = Mutex::new(RefCell::new(super::Ring::new()));
    static RECENT: Mutex<RefCell<super::Recent>> = Mutex::new(RefCell::new(super::Recent::new()));
    pub fn with<R>(f: impl FnOnce(&mut super::Ring) -> R) -> Option<R> {
        critical_section::with(|cs| RING.borrow(cs).try_borrow_mut().ok().map(|mut r| f(&mut r)))
    }
    #[cfg_attr(not(feature = "display"), allow(dead_code))]
    pub fn with_recent<R>(f: impl FnOnce(&mut super::Recent) -> R) -> Option<R> {
        critical_section::with(|cs| RECENT.borrow(cs).try_borrow_mut().ok().map(|mut r| f(&mut r)))
    }
}

#[cfg(not(feature = "hal"))]
mod sync {
    static RING: std::sync::Mutex<super::Ring> = std::sync::Mutex::new(super::Ring::new());
    pub fn with<R>(f: impl FnOnce(&mut super::Ring) -> R) -> Option<R> {
        RING.try_lock().ok().map(|mut r| f(&mut r))
    }
}

/// `true` while an authenticated manager is connected (lines are buffered only then).
pub fn connected() -> bool { CONNECTED.load(Ordering::Relaxed) }

/// Queue `line` for the manager; non-blocking, dropped if not connected / full. Returns `true` if queued.
pub fn ble_log(line: &str) -> bool {
    push(line.as_bytes(), b"")
}

fn push(a: &[u8], b: &[u8]) -> bool {
    connected() && sync::with(|r| r.push_line(a, b)).unwrap_or(false)
}

#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn set_connected(on: bool) {
    CONNECTED.store(on, Ordering::Relaxed);
    if !on {
        let _ = sync::with(Ring::clear);
        RSSI.store(127, Ordering::Relaxed);
        LAST_DISC.store((crate::time::uptime_ms() as u32).wrapping_add(1).max(1), Ordering::Relaxed);
    }
}

/// Address of the connected manager (as reported by the BLE stack, most significant byte first).
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn set_peer(addr: [u8; 6]) {
    PEER_LO.store(u32::from_le_bytes([addr[0], addr[1], addr[2], addr[3]]), Ordering::Relaxed);
    PEER_HI.store(u32::from_le_bytes([addr[4], addr[5], 0, 0]), Ordering::Relaxed);
}
/// Last known manager address (`[0; 6]` if none yet).
pub fn peer() -> [u8; 6] {
    let lo = PEER_LO.load(Ordering::Relaxed).to_le_bytes();
    let hi = PEER_HI.load(Ordering::Relaxed).to_le_bytes();
    [lo[0], lo[1], lo[2], lo[3], hi[0], hi[1]]
}
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn set_rssi(rssi: Option<i8>) { RSSI.store(rssi.unwrap_or(127) as u8 as u32, Ordering::Relaxed); }
/// Link RSSI (dBm) of the manager connection, refreshed every ~2 s while connected.
pub fn rssi() -> Option<i8> { let v = RSSI.load(Ordering::Relaxed) as u8 as i8; (v != 127).then_some(v) }
/// Uptime (ms) of the last manager disconnect, if any.
pub fn last_disconnect_ms() -> Option<u64> {
    match LAST_DISC.load(Ordering::Relaxed) { 0 => None, v => Some(v.wrapping_sub(1) as u64) }
}

/// Pop up to `out.len()` buffered bytes.
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn pop(out: &mut [u8]) -> usize {
    sync::with(|r| r.pop(out)).unwrap_or(0)
}

/// Format a `log` record into a fixed buffer and queue it.
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn mirror(record: &log::Record) {
    if !connected() && !cfg!(feature = "display") { return; }
    struct Buf { b: [u8; LINE_MAX], n: usize }
    impl core::fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &c in s.as_bytes() {
                if self.n == LINE_MAX { break; }
                self.b[self.n] = if c == b'\n' { b' ' } else { c };
                self.n += 1;
            }
            Ok(())
        }
    }
    let mut buf = Buf { b: [0; LINE_MAX], n: 0 };
    let _ = core::fmt::write(&mut buf, format_args!("{} {} - {}", record.level(), record.target(), record.args()));
    #[cfg(all(feature = "display", feature = "hal"))]
    let _ = sync::with_recent(|r| r.push(&buf.b[..buf.n]));
    push(&buf.b[..buf.n], b"");
}
