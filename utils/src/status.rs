//! Status buffer: timestamped values queued on the board and shipped to the BLE manager.
//!
//! - [`buffer_add_sample`]`(time, fields)` stores one sample (one main-loop pass) as
//!   `"<ms>": {"field": value, …}`. The key is the controller-synced time ([`synced_time`], ms since
//!   the Unix epoch); before the first sync it is the board uptime in ms and the sample carries
//!   `"synced": false`. [`buffer_add`] is a one-field sample.
//! - Bounded: over [`MAX_BYTES`] (approx. serialized size) or [`MAX_ENTRIES`] the oldest entries are dropped.
//! - [`send_buffer`] (call it once per main-loop pass; non-blocking): while a manager is connected,
//!   a call with unsent samples, at least [`SNAPSHOT_INTERVAL_MS`] after the previous new snapshot,
//!   moves all of them (oldest first, ≤ ~[`SNAPSHOT_BYTES`]; any rest goes next time) into a new
//!   snapshot `{"seq":N,"buffer":{"<ms>":{…},…}}` with its own `seq` (u16, wrapping), without waiting
//!   for acks. Snapshots are kept until acknowledged (≤ [`MAX_INFLIGHT`]); one unacked for
//!   [`ACK_TIMEOUT_MS`] after its last frame is resent with the same `seq`; after a reconnect all
//!   unacked ones are resent. The BLE task sends them on the buffer notify characteristic
//!   (`c0de5e05`) back-to-back, each as contiguous frames `[seq & 0xff, flags, payload…]` (flags:
//!   bit 7 = last frame, bit 6 = first frame, bits 0–5 = frame index mod 64; payload ≤ 18 B).
//! - The manager acks with `{"action":"buffer_ack","params":[seq,seq,…]}` (batched); acked
//!   snapshots are dropped. When full, the oldest data goes first (unacked snapshots, then unsent
//!   samples).
//! - Time sync: command `{"action":"set_time","params":[<controller ms since epoch>]}` stores
//!   `offset = controller_time − uptime`.
//!
//! Thread-safe (critical section on `hal`, mutex on `idf`). On builds without a heap (hal without
//! `ble`, esp32s2) every function is a no-op and [`synced_time`] returns `None`.
use core::fmt::Write;

/// Byte cap for unsent samples plus unacked snapshot copies (each sample counted as its JSON + 40 B
/// overhead). 16 KB on S3/IDF (128 KiB heap or IDF heap); 8 KB on the other hal chips (96 KiB heap,
/// ~30 KiB taken by the BLE controller). Worst case incl. VecDeque slack and one snapshot being
/// serialized: ~16+12+3 KB on S3.
#[cfg(any(feature = "esp32s3", feature = "idf"))]
pub const MAX_BYTES: usize = 16 * 1024;
#[cfg(not(any(feature = "esp32s3", feature = "idf")))]
pub const MAX_BYTES: usize = 8 * 1024;
/// Sample-count cap (unsent + in unacked snapshots). In practice the byte cap binds first (~400 samples).
#[cfg(any(feature = "esp32s3", feature = "idf"))]
pub const MAX_ENTRIES: usize = 1000;
#[cfg(not(any(feature = "esp32s3", feature = "idf")))]
pub const MAX_ENTRIES: usize = 500;
/// Resend a snapshot if it is not acknowledged within this time.
pub const ACK_TIMEOUT_MS: u64 = 1000;
/// Minimum time between new snapshots.
pub const SNAPSHOT_INTERVAL_MS: u64 = 1000;
/// Max approximate size of one snapshot (oldest entries first); ≈ 85 frames.
pub const SNAPSHOT_BYTES: usize = 3072;
/// Frame flag: last frame of a snapshot.
pub const FRAME_LAST: u8 = 0x80;
/// Frame flag: first frame of a snapshot.
pub const FRAME_FIRST: u8 = 0x40;
/// Max unacknowledged snapshots kept (new snapshots wait while this many are in flight).
pub const MAX_INFLIGHT: usize = 16;
/// Payload bytes per buffer frame (`LOG_CHUNK` − 2 header bytes).
pub const FRAME_PAYLOAD: usize = crate::ble_proto::LOG_CHUNK - 2;

/// A value [`buffer_add`] can store: written as a JSON fragment.
pub trait BufValue {
    fn write_json(&self, out: &mut dyn Write) -> core::fmt::Result;
}
macro_rules! num_value { ($($t:ty),*) => { $(impl BufValue for $t {
    fn write_json(&self, out: &mut dyn Write) -> core::fmt::Result { write!(out, "{}", self) }
})* } }
num_value!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, bool);
impl BufValue for f32 {
    fn write_json(&self, out: &mut dyn Write) -> core::fmt::Result { (*self as f64).write_json(out) }
}
impl BufValue for f64 {
    fn write_json(&self, out: &mut dyn Write) -> core::fmt::Result {
        if self.is_finite() { write!(out, "{}", self) } else { out.write_str("null") }
    }
}
impl BufValue for &str {
    fn write_json(&self, out: &mut dyn Write) -> core::fmt::Result { write_json_str(out, self) }
}
/// A pre-serialized JSON fragment (object, array, …), stored verbatim.
pub struct Json<'a>(pub &'a str);
impl BufValue for Json<'_> {
    fn write_json(&self, out: &mut dyn Write) -> core::fmt::Result { out.write_str(self.0) }
}

fn write_json_str(out: &mut dyn Write, s: &str) -> core::fmt::Result {
    out.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

#[cfg(any(feature = "idf", all(feature = "hal", feature = "ble", not(feature = "esp32s2"))))]
mod imp {
    #[cfg(feature = "hal")]
    use alloc::{collections::VecDeque, string::String, vec::Vec};
    #[cfg(not(feature = "hal"))]
    use std::collections::VecDeque;
    use core::fmt::Write;

    /// One unsent sample (main-loop pass): `fields` is the JSON object body (`"k":v,…`, no braces).
    struct Entry { t: u64, synced: bool, fields: String }
    impl Entry { fn size(&self) -> usize { self.fields.len() + 40 } }

    /// A serialized snapshot that is sent (or queued) but not yet acknowledged.
    struct Snap { seq: u16, bytes: Vec<u8>, samples: usize, sent_at: u64, need_send: bool }

    /// Snapshot currently being framed out.
    struct Tx { seq: u16, pos: usize, frame: u8 }

    pub(super) struct State {
        entries: VecDeque<Entry>,
        bytes: usize,
        offset: Option<i64>,
        seq: u16,
        inflight: VecDeque<Snap>,
        tx: Option<Tx>,
        /// Uptime of the last new snapshot (`None` = none yet).
        last_snap: Option<u64>,
        /// Copy of the newest sample for the status screen (kept after it is sent).
        latest: Option<(u64, bool, String)>,
        /// Body of the newest sample with more than `uptime_ms` (kept for the status screen).
        latest_full: Option<String>,
        /// Key of the previous sample (keys are kept strictly increasing).
        last_t: Option<(u64, bool)>,
    }
    impl State {
        const fn new() -> Self {
            Self { entries: VecDeque::new(), bytes: 0, offset: None, seq: 0, inflight: VecDeque::new(), tx: None, last_snap: None, latest: None, latest_full: None, last_t: None }
        }
        fn total(&self) -> usize { self.bytes + self.inflight.iter().map(|s| s.bytes.len()).sum::<usize>() }
        /// Drop the oldest data (unacked snapshots first, then unsent samples) until under the caps.
        fn enforce_caps(&mut self) {
            while self.total() > super::MAX_BYTES || self.entries.len() + self.inflight.iter().map(|s| s.samples).sum::<usize>() > super::MAX_ENTRIES {
                let busy = self.tx.as_ref().map(|t| t.seq);
                if let Some(i) = self.inflight.iter().position(|s| Some(s.seq) != busy) {
                    self.inflight.remove(i);
                } else if self.entries.len() > 1 {
                    if let Some(e) = self.entries.pop_front() { self.bytes -= e.size(); }
                } else {
                    break;
                }
            }
        }
    }

    #[cfg(feature = "hal")]
    mod sync {
        use core::cell::RefCell;
        use critical_section::Mutex;
        static S: Mutex<RefCell<super::State>> = Mutex::new(RefCell::new(super::State::new()));
        pub fn with<R>(f: impl FnOnce(&mut super::State) -> R) -> R {
            critical_section::with(|cs| f(&mut S.borrow(cs).borrow_mut()))
        }
    }
    #[cfg(not(feature = "hal"))]
    mod sync {
        static S: std::sync::Mutex<super::State> = std::sync::Mutex::new(super::State::new());
        pub fn with<R>(f: impl FnOnce(&mut super::State) -> R) -> R {
            f(&mut S.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    fn now() -> u64 { crate::time::uptime_ms() }

    pub fn synced_time() -> Option<u64> {
        sync::with(|s| s.offset).map(|o| (now() as i64 + o).max(0) as u64)
    }

    pub fn set_time(controller_ms: u64) {
        let o = controller_ms as i64 - now() as i64;
        sync::with(|s| s.offset = Some(o));
    }

    pub fn add(time: Option<u64>, fields: &[(&str, &dyn super::BufValue)]) {
        let mut body = String::new();
        for (k, v) in fields {
            let start = body.len();
            if !body.is_empty() { body.push(','); }
            let ok = super::write_json_str(&mut body, k).is_ok() && body.write_char(':').is_ok();
            let before = body.len();
            if !ok || v.write_json(&mut body).is_err() || body.len() == before { body.truncate(start); }
        }
        if body.is_empty() { return; }
        let up = now();
        sync::with(|s| {
            let (t, synced) = match (time, s.offset) {
                (Some(t), _) => (t, true),
                (None, Some(o)) => ((up as i64 + o).max(0) as u64, true),
                (None, None) => (up, false),
            };
            // Unique, increasing keys: a key <= the previous one (same ms, or clock stepped back) becomes previous+1.
            let t = match s.last_t { Some((p, ps)) if ps == synced && t <= p => p + 1, _ => t };
            s.last_t = Some((t, synced));
            match &mut s.latest {
                Some(l) => { l.0 = t; l.1 = synced; l.2.clear(); l.2.push_str(&body); }
                None => s.latest = Some((t, synced, body.clone())),
            }
            if body.contains(',') {
                match &mut s.latest_full { Some(l) => { l.clear(); l.push_str(&body); } None => s.latest_full = Some(body.clone()) }
            }
            let e = Entry { t, synced, fields: body };
            s.bytes += e.size();
            s.entries.push_back(e);
            s.enforce_caps();
        })
    }

    pub fn send() {
        let connected = crate::blelog::connected();
        // A new snapshot is due: add one full reading first (polled outside the lock), so it is part of it.
        let due = connected && sync::with(|s| {
            !s.entries.is_empty() && s.inflight.len() < super::MAX_INFLIGHT
                && !s.last_snap.is_some_and(|l| now().saturating_sub(l) < super::SNAPSHOT_INTERVAL_MS)
        });
        if due { crate::readings::add_full_reading(); }
        let t = now();
        sync::with(|s| {
            if !connected {
                // Keep unacked snapshots; send them all again after reconnecting.
                s.tx = None;
                for sn in s.inflight.iter_mut() { sn.need_send = true; }
                return;
            }
            // Resend snapshots not acknowledged within ACK_TIMEOUT_MS (same seq).
            let busy = s.tx.as_ref().map(|x| x.seq);
            for sn in s.inflight.iter_mut() {
                if !sn.need_send && Some(sn.seq) != busy && t.saturating_sub(sn.sent_at) >= super::ACK_TIMEOUT_MS {
                    sn.need_send = true;
                }
            }
            if s.entries.is_empty() || s.inflight.len() >= super::MAX_INFLIGHT { return; }
            if s.last_snap.is_some_and(|l| t.saturating_sub(l) < super::SNAPSHOT_INTERVAL_MS) { return; }
            s.last_snap = Some(t);
            // New snapshot from the oldest unsent samples (≤ ~SNAPSHOT_BYTES), without waiting for acks.
            let mut take = 0;
            let mut size = 0;
            for e in s.entries.iter() {
                let sz = e.size();
                if take > 0 && size + sz > super::SNAPSHOT_BYTES { break; }
                size += sz;
                take += 1;
            }
            s.seq = s.seq.wrapping_add(1);
            let seq = s.seq;
            let mut out = String::with_capacity(size + 32);
            let _ = write!(out, "{{\"seq\":{seq},\"buffer\":{{");
            for i in 0..take {
                let e = s.entries.pop_front().unwrap();
                s.bytes -= e.size();
                if i > 0 { out.push(','); }
                let _ = write!(out, "\"{}\":{{{}", e.t, e.fields);
                if !e.synced { out.push_str(",\"synced\":false"); }
                out.push('}');
            }
            out.push_str("}}");
            s.inflight.push_back(Snap { seq, bytes: out.into_bytes(), samples: take, sent_at: t, need_send: true });
            s.enforce_caps();
        })
    }

    /// Remove acknowledged snapshots; returns how many were removed.
    pub fn ack(seqs: &[u16]) -> usize {
        sync::with(|s| {
            let before = s.inflight.len();
            s.inflight.retain(|sn| !seqs.contains(&sn.seq));
            if let Some(x) = &s.tx && seqs.contains(&x.seq) { s.tx = None; }
            before - s.inflight.len()
        })
    }

    pub fn latest(f: &mut dyn FnMut(u64, bool, &str)) -> usize {
        sync::with(|s| {
            if let Some((t, synced, fields)) = &s.latest {
                f(*t, *synced, fields);
                // Then the last full reading (the display skips its duplicate uptime_ms).
                if let Some(full) = &s.latest_full && full != fields { f(*t, *synced, full); }
            }
            s.entries.len() + s.inflight.iter().map(|x| x.samples).sum::<usize>()
        })
    }

    /// Next frame to notify (`[seq as u8, flags, payload…]`); 0 = nothing to send. Frames of one
    /// snapshot are contiguous; queued snapshots follow back-to-back.
    pub fn frame(buf: &mut [u8]) -> usize {
        sync::with(|s| {
            if s.tx.is_none() {
                let Some(sn) = s.inflight.iter_mut().find(|sn| sn.need_send) else { return 0 };
                sn.need_send = false;
                s.tx = Some(Tx { seq: sn.seq, pos: 0, frame: 0 });
            }
            let tx = s.tx.as_ref().unwrap();
            let Some(sn) = s.inflight.iter().find(|sn| sn.seq == tx.seq) else { s.tx = None; return 0 };
            let len = sn.bytes.len();
            let end = (tx.pos + super::FRAME_PAYLOAD).min(len);
            buf[0] = tx.seq as u8;
            buf[1] = (tx.frame & 0x3f)
                | if tx.pos == 0 { super::FRAME_FIRST } else { 0 }
                | if end == len { super::FRAME_LAST } else { 0 };
            buf[2..2 + end - tx.pos].copy_from_slice(&sn.bytes[tx.pos..end]);
            2 + end - tx.pos
        })
    }

    /// The frame returned by [`frame`] was sent.
    pub fn advance() {
        let t = now();
        sync::with(|s| {
            let Some(tx) = s.tx.as_mut() else { return };
            let seq = tx.seq;
            let Some(sn) = s.inflight.iter_mut().find(|sn| sn.seq == seq) else { s.tx = None; return };
            tx.pos = (tx.pos + super::FRAME_PAYLOAD).min(sn.bytes.len());
            tx.frame = tx.frame.wrapping_add(1);
            if tx.pos >= sn.bytes.len() {
                sn.sent_at = t; // ack timeout counts from the last frame
                s.tx = None;
            }
        })
    }
}

#[cfg(not(any(feature = "idf", all(feature = "hal", feature = "ble", not(feature = "esp32s2")))))]
mod imp {
    pub fn synced_time() -> Option<u64> { None }
    #[allow(dead_code)] pub fn set_time(_: u64) {}
    pub fn add(_: Option<u64>, _: &[(&str, &dyn super::BufValue)]) {}
    pub fn send() {}
    #[allow(dead_code)] pub fn ack(_: &[u16]) -> usize { 0 }
    #[allow(dead_code)] pub fn frame(_: &mut [u8]) -> usize { 0 }
    #[allow(dead_code)] pub fn advance() {}
    #[allow(dead_code)] pub fn latest(_: &mut dyn FnMut(u64, bool, &str)) -> usize { 0 }
}

/// Whether this build has a status buffer (heap): idf, or hal with `ble` (not esp32s2).
pub const ENABLED: bool = cfg!(any(feature = "idf", all(feature = "hal", feature = "ble", not(feature = "esp32s2"))));
pub use crate::readings::{add_standard_readings, battery, temperature};

/// Append one sample: `fields` become the entry `{"k":v,…}` under its time key. `time` = ms since
/// the epoch, or `None` for [`synced_time`] (uptime + `"synced":false` before the first sync).
/// A key <= the previous sample's (same domain) is bumped to previous+1 ms. Fields whose value fails to serialize are omitted; an empty sample is not stored. Over the caps
/// the oldest samples are dropped.
pub fn buffer_add_sample(time: Option<u64>, fields: &[(&str, &dyn BufValue)]) { imp::add(time, fields) }
/// One-field sample at the current time (`buffer_add_sample(None, &[(key, &value)])`).
pub fn buffer_add(key: &str, value: impl BufValue) { imp::add(None, &[(key, &value)]) }
/// Ship the buffer to the connected manager if possible (non-blocking; call once per loop pass).
pub fn send_buffer() { imp::send() }
/// Controller-synced time in ms since the Unix epoch, `None` before the first `set_time`.
pub fn synced_time() -> Option<u64> { imp::synced_time() }
#[allow(unused_imports)]
pub(crate) use imp::{ack, advance, frame, set_time};
/// For the status screen: the latest sample (no allocation, under the buffer lock); returns the
/// number of buffered samples.
#[allow(dead_code)]
pub(crate) fn latest(f: &mut dyn FnMut(u64, bool, &str)) -> usize { imp::latest(f) }
