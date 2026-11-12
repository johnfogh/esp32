//! Host-side clone of firmware status-buffer framing and BLE command dispatch.
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use utils::ble_proto::{BUF_FRAME_FIRST, BUF_FRAME_INDEX, BUF_FRAME_LAST, LOG_CHUNK};

use crate::sensor::{AccelUnit, Board};

const MAX_BYTES: usize = 8 * 1024;
const MAX_ENTRIES: usize = 500;
const ACK_TIMEOUT: Duration = Duration::from_millis(1000);
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(1000);
const SNAPSHOT_BYTES: usize = 3072;
const FRAME_PAYLOAD: usize = LOG_CHUNK - 2;
const MAX_INFLIGHT: usize = 16;

struct Entry {
    t: u64,
    synced: bool,
    fields: String,
}
impl Entry {
    fn size(&self) -> usize {
        self.fields.len() + 40
    }
}

struct Snap {
    seq: u16,
    bytes: Vec<u8>,
    samples: usize,
    sent_at: Instant,
    need_send: bool,
}

struct Tx {
    seq: u16,
    pos: usize,
    frame: u8,
}

pub struct Buffer {
    entries: VecDeque<Entry>,
    bytes: usize,
    seq: u16,
    inflight: VecDeque<Snap>,
    tx: Option<Tx>,
    last_snap: Option<Instant>,
    last_t: Option<(u64, bool)>,
    last_full: Instant,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            seq: 0,
            inflight: VecDeque::new(),
            tx: None,
            last_snap: None,
            last_t: None,
            last_full: Instant::now() - SNAPSHOT_INTERVAL,
        }
    }

    fn total(&self) -> usize {
        self.bytes + self.inflight.iter().map(|s| s.bytes.len()).sum::<usize>()
    }

    fn enforce_caps(&mut self) {
        while self.total() > MAX_BYTES
            || self.entries.len() + self.inflight.iter().map(|s| s.samples).sum::<usize>() > MAX_ENTRIES
        {
            let busy = self.tx.as_ref().map(|t| t.seq);
            if let Some(i) = self.inflight.iter().position(|s| Some(s.seq) != busy) {
                self.inflight.remove(i);
            } else if self.entries.len() > 1 {
                if let Some(e) = self.entries.pop_front() {
                    self.bytes -= e.size();
                }
            } else {
                break;
            }
        }
    }

    pub fn add_sample(&mut self, t: u64, synced: bool, fields: String) {
        if fields.is_empty() {
            return;
        }
        let t = match self.last_t {
            Some((p, ps)) if ps == synced && t <= p => p + 1,
            _ => t,
        };
        self.last_t = Some((t, synced));
        let e = Entry { t, synced, fields };
        self.bytes += e.size();
        self.entries.push_back(e);
        self.enforce_caps();
    }

    /// Uptime sample; a full reading is folded in when a snapshot is due.
    pub fn tick(&mut self, board: &Board, connected: bool) {
        let tel = board.telemetry();
        let (t, synced) = match tel.offset {
            Some(o) => ((tel.uptime_ms as i64 + o).max(0) as u64, true),
            None => (tel.uptime_ms, false),
        };
        self.add_sample(t, synced, format!("\"uptime_ms\":{}", tel.uptime_ms));
        if connected && self.last_full.elapsed() >= SNAPSHOT_INTERVAL {
            self.last_full = Instant::now();
            let mut fields = String::new();
            let _ = write!(
                fields,
                "\"uptime_ms\":{},\"temp\":{:.2},\"battery\":{{\"v\":{:.3},\"pct\":{}}},\"accel\":{{\"x\":{:.4},\"y\":{:.4},\"z\":{:.4}}}",
                tel.uptime_ms, tel.temp, tel.battery_v, tel.battery_pct, tel.accel[0], tel.accel[1], tel.accel[2]
            );
            self.add_sample(t.saturating_add(1), synced, fields);
        }
        if connected {
            self.maybe_snapshot();
        } else {
            self.tx = None;
            for sn in self.inflight.iter_mut() {
                sn.need_send = true;
            }
        }
    }

    fn maybe_snapshot(&mut self) {
        let now = Instant::now();
        let busy = self.tx.as_ref().map(|x| x.seq);
        for sn in self.inflight.iter_mut() {
            if !sn.need_send && Some(sn.seq) != busy && now.saturating_duration_since(sn.sent_at) >= ACK_TIMEOUT {
                sn.need_send = true;
            }
        }
        if self.entries.is_empty() || self.inflight.len() >= MAX_INFLIGHT {
            return;
        }
        if self.last_snap.is_some_and(|l| now.saturating_duration_since(l) < SNAPSHOT_INTERVAL) {
            return;
        }
        self.last_snap = Some(now);
        let mut take = 0;
        let mut size = 0;
        for e in self.entries.iter() {
            let sz = e.size();
            if take > 0 && size + sz > SNAPSHOT_BYTES {
                break;
            }
            size += sz;
            take += 1;
        }
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        let mut out = String::with_capacity(size + 32);
        let _ = write!(out, "{{\"seq\":{seq},\"buffer\":{{");
        for i in 0..take {
            let e = self.entries.pop_front().unwrap();
            self.bytes -= e.size();
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "\"{}\":{{{}", e.t, e.fields);
            if !e.synced {
                out.push_str(",\"synced\":false");
            }
            out.push('}');
        }
        out.push_str("}}");
        self.inflight.push_back(Snap {
            seq,
            bytes: out.into_bytes(),
            samples: take,
            sent_at: now,
            need_send: true,
        });
        self.enforce_caps();
    }

    pub fn ack(&mut self, seqs: &[u16]) {
        self.inflight.retain(|sn| !seqs.contains(&sn.seq));
        if let Some(x) = &self.tx
            && seqs.contains(&x.seq)
        {
            self.tx = None;
        }
    }

    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.tx.is_none() {
            let sn = self.inflight.iter_mut().find(|sn| sn.need_send)?;
            sn.need_send = false;
            self.tx = Some(Tx { seq: sn.seq, pos: 0, frame: 0 });
        }
        let tx = self.tx.as_ref()?;
        let sn = match self.inflight.iter().find(|sn| sn.seq == tx.seq) {
            Some(s) => s,
            None => {
                self.tx = None;
                return None;
            }
        };
        let len = sn.bytes.len();
        let end = (tx.pos + FRAME_PAYLOAD).min(len);
        let mut buf = vec![0u8; 2 + end - tx.pos];
        buf[0] = tx.seq as u8;
        buf[1] = (tx.frame & BUF_FRAME_INDEX)
            | if tx.pos == 0 { BUF_FRAME_FIRST } else { 0 }
            | if end == len { BUF_FRAME_LAST } else { 0 };
        buf[2..].copy_from_slice(&sn.bytes[tx.pos..end]);
        Some(buf)
    }

    pub fn advance(&mut self) {
        let now = Instant::now();
        let Some(tx) = self.tx.as_mut() else {
            return;
        };
        let seq = tx.seq;
        let Some(sn) = self.inflight.iter_mut().find(|sn| sn.seq == seq) else {
            self.tx = None;
            return;
        };
        tx.pos = (tx.pos + FRAME_PAYLOAD).min(sn.bytes.len());
        tx.frame = tx.frame.wrapping_add(1);
        if tx.pos >= sn.bytes.len() {
            sn.sent_at = now;
            self.tx = None;
        }
    }

    pub fn add_method(&mut self, board: &Board, method: &str, result: &str, error: bool) {
        let tel = board.telemetry();
        let (t, synced) = match tel.offset {
            Some(o) => ((tel.uptime_ms as i64 + o).max(0) as u64, true),
            None => (tel.uptime_ms, false),
        };
        let fields = format!(
            "\"method\":\"{}\",\"result\":{},\"error\":{}",
            json_escape(method),
            json_str(result),
            error
        );
        self.add_sample(t, synced, fields);
    }
}

pub fn execute(board: &Arc<Board>, line: &str) -> String {
    let Some(action) = json_str_field(line, "action") else {
        return format!("command: error: no \"action\" in {line}");
    };
    let params = json_params(line);
    let p: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
    match dispatch(board, &action, &p) {
        Ok(r) => format!("{action}: {r}"),
        Err(e) => format!("{action}: error: {e}"),
    }
}

pub fn is_silent(action: &str) -> bool {
    matches!(action, "set_time" | "buffer_ack" | "read_buffer")
}

pub fn logs_result(action: &str) -> bool {
    !matches!(action, "set_time" | "buffer_ack")
}

pub fn action_name(line: &str) -> Option<String> {
    json_str_field(line, "action")
}

fn dispatch(board: &Arc<Board>, action: &str, p: &[&str]) -> Result<String, String> {
    match action {
        "ping" => Ok("pong".into()),
        "set_time" => {
            let ms = p.first().and_then(|s| s.trim().parse::<u64>().ok()).ok_or("need [ms since epoch]")?;
            board.set_time(ms);
            Ok("ok".into())
        }
        "buffer_ack" => {
            if p.is_empty() {
                return Err("need [seq, …]".into());
            }
            // parsed by the BLE task which owns the buffer
            Ok(p.join(","))
        }
        "get_accelerometers" => {
            let unit = AccelUnit::parse(p.first().copied().unwrap_or("")).ok_or_else(|| format!("bad unit {:?}", p[0]))?;
            let count = match p.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                None => 1,
                Some(s) => s.parse::<u32>().map_err(|_| format!("bad count {s:?}"))?,
            };
            if count == 0 || count > 100 {
                return Err("count must be 1..=100".into());
            }
            let interval = match p.get(2).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                None => 0,
                Some(s) => parse_duration_ms(s).ok_or_else(|| format!("bad interval {s:?} (MM:SS.mmm or ms)"))?,
            };
            let mut samples = Vec::new();
            for i in 0..count {
                if i > 0 && interval > 0 {
                    std::thread::sleep(Duration::from_millis(interval as u64));
                }
                let a = board.telemetry().accel_unit(unit);
                samples.push(format!("{{\"x\":{},\"y\":{},\"z\":{}}}", a[0], a[1], a[2]));
            }
            if samples.len() == 1 {
                Ok(samples.pop().unwrap())
            } else {
                Ok(format!("[{}]", samples.join(",")))
            }
        }
        "set_threshold" => {
            if !board.is_hub() {
                return Err("not a hub".into());
            }
            let v = p
                .first()
                .and_then(|s| s.trim().parse::<u16>().ok())
                .ok_or("need [u16]")?;
            board.set_threshold(v);
            Ok(format!("{v}"))
        }
        "set_name" => {
            if !board.is_hub() {
                return Err("not a hub".into());
            }
            let n = p.first().ok_or("need [name]")?;
            board.set_sensor_name(n)?;
            Ok(board.sensor_name())
        }
        "read_buffer" => {
            if !board.is_hub() {
                return Err("not a hub".into());
            }
            Ok(board.read_buffer_json())
        }
        _ => Err(format!("unknown action {action:?}")),
    }
}

pub fn parse_ack_seqs(result_ok: &str) -> Result<Vec<u16>, String> {
    let mut seqs = Vec::new();
    for x in result_ok.split(',') {
        let x = x.trim();
        if x.is_empty() {
            continue;
        }
        seqs.push(x.parse::<u16>().map_err(|_| format!("bad seq {x:?}"))?);
    }
    if seqs.is_empty() {
        Err("need [seq, …]".into())
    } else {
        Ok(seqs)
    }
}

pub fn parse_duration_ms(s: &str) -> Option<u32> {
    if !s.contains(':') && !s.contains('.') {
        return s.parse().ok();
    }
    let (m, rest) = match s.split_once(':') {
        Some((m, r)) => (m.parse::<u32>().ok()?, r),
        None => (0, s),
    };
    let (sec, ms) = match rest.split_once('.') {
        Some((a, b)) if !b.is_empty() && b.len() <= 3 => {
            (a.parse::<u32>().ok()?, b.parse::<u32>().ok()? * 10u32.pow(3 - b.len() as u32))
        }
        Some(_) => return None,
        None => (rest.parse::<u32>().ok()?, 0),
    };
    if sec >= 60 && s.contains(':') {
        return None;
    }
    Some(m * 60_000 + sec * 1000 + ms)
}

fn json_str_field(s: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\"");
    let i = s.find(&k)? + k.len();
    let rest = s[i..].trim_start().strip_prefix(':')?.trim_start();
    let (v, _) = json_string(rest)?;
    Some(v)
}

fn json_string(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut it = s.strip_prefix('"')?.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some((out, &s[i + 2..])),
            '\\' => match it.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    for _ in 0..4 {
                        it.next()?;
                    }
                    out.push('?');
                }
                e => out.push(e),
            },
            c => out.push(c),
        }
    }
    None
}

fn json_params(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let Some(i) = s.find("\"params\"") else {
        return v;
    };
    let Some(mut rest) = s[i + 8..]
        .trim_start()
        .strip_prefix(':')
        .and_then(|r| r.trim_start().strip_prefix('['))
    else {
        return v;
    };
    loop {
        rest = rest.trim_start();
        if rest.starts_with(']') || rest.is_empty() {
            break;
        }
        if rest.starts_with('"') {
            let Some((x, r)) = json_string(rest) else { break };
            v.push(x);
            rest = r;
        } else {
            let end = rest.find([',', ']']).unwrap_or(rest.len());
            v.push(rest[..end].trim().into());
            rest = &rest[end..];
        }
        rest = rest.trim_start();
        match rest.strip_prefix(',') {
            Some(r) => rest = r,
            None => break,
        }
    }
    v
}

fn json_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o
}

fn json_str(s: &str) -> String {
    if s.starts_with('{') || s.starts_with('[') {
        s.to_string()
    } else {
        format!("\"{}\"", json_escape(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensor::Hub;

    #[test]
    fn read_buffer_hub_rest() {
        let hub = Hub::new();
        let r = execute(&hub.hub, r#"{"action":"read_buffer","params":[]}"#);
        assert_eq!(r, r#"read_buffer: {"sensor-000001":[]}"#);
    }

    #[test]
    fn read_buffer_hub_hit() {
        let hub = Hub::new();
        hub.hub.hit_sensors([4095, 3000, 2500, 2000]);
        let r = execute(&hub.hub, r#"{"action":"read_buffer","params":[]}"#);
        assert!(r.starts_with(r#"read_buffer: {"sensor-000001":["#), "{r}");
        assert!(r.contains(",4095,3000,2500,2000]"), "{r}");
    }

    #[test]
    fn set_threshold_and_omit() {
        let hub = Hub::new();
        let r = execute(&hub.hub, r#"{"action":"set_threshold","params":[3000]}"#);
        assert_eq!(r, "set_threshold: 3000");
        hub.hub.hit_sensors([4095, 3000, 2500, 2000]);
        let r = execute(&hub.hub, r#"{"action":"read_buffer","params":[]}"#);
        assert_eq!(r, r#"read_buffer: {"sensor-000001":[]}"#);
        assert_eq!(
            execute(&hub.hub, r#"{"action":"set_threshold","params":[]}"#),
            "set_threshold: error: need [u16]"
        );
    }

    #[test]
    fn read_buffer_standalone_errors() {
        let hub = Hub::new();
        let r = execute(&hub.sensors[0], r#"{"action":"read_buffer","params":[]}"#);
        assert_eq!(r, "read_buffer: error: not a hub");
        let r = execute(&hub.sensors[0], r#"{"action":"read_sensor","params":[]}"#);
        assert_eq!(r, r#"read_sensor: error: unknown action "read_sensor""#);
    }

    #[test]
    fn set_name_wraps_buffer() {
        let hub = Hub::new();
        assert_eq!(
            execute(&hub.hub, r#"{"action":"set_name","params":["front"]}"#),
            "set_name: front"
        );
        hub.hub.hit_sensors([2000, 2000, 2000, 2000]);
        let r = execute(&hub.hub, r#"{"action":"read_buffer","params":[]}"#);
        assert!(r.starts_with(r#"read_buffer: {"front":["#), "{r}");
        assert!(r.contains(",2000,2000,2000,2000]"), "{r}");
        assert_eq!(
            execute(&hub.hub, r#"{"action":"set_name","params":[]}"#),
            "set_name: error: need [name]"
        );
        assert_eq!(
            execute(&hub.hub, r#"{"action":"set_name","params":[""]}"#),
            "set_name: error: empty name"
        );
        assert_eq!(
            execute(&hub.sensors[0], r#"{"action":"set_name","params":["front"]}"#),
            "set_name: error: not a hub"
        );
    }

    #[test]
    fn silent_and_log_result_actions() {
        assert!(is_silent("set_time"));
        assert!(is_silent("buffer_ack"));
        assert!(is_silent("read_buffer"));
        assert!(!is_silent("ping"));
        assert!(!logs_result("set_time"));
        assert!(!logs_result("buffer_ack"));
        assert!(logs_result("read_buffer"));
        assert!(logs_result("ping"));
    }

    #[test]
    fn action_name_reads_json_field() {
        assert_eq!(
            action_name(r#"{"action":"ping","params":[]}"#).as_deref(),
            Some("ping")
        );
        assert_eq!(action_name(r#"{"params":[]}"#), None);
        assert_eq!(
            action_name(r#"{"action":"set_name","params":["a"]}"#).as_deref(),
            Some("set_name")
        );
    }

    #[test]
    fn parse_ack_seqs_splits_and_rejects_empty() {
        assert_eq!(parse_ack_seqs("1, 2,3").unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_ack_seqs("42").unwrap(), vec![42]);
        assert!(parse_ack_seqs("").is_err());
        assert!(parse_ack_seqs("1,x").is_err());
    }

    #[test]
    fn parse_duration_ms_plain_and_clock() {
        assert_eq!(parse_duration_ms("1500"), Some(1500));
        assert_eq!(parse_duration_ms("1:02.5"), Some(62_500));
        assert_eq!(parse_duration_ms("0:00.250"), Some(250));
        assert_eq!(parse_duration_ms("2.5"), Some(2500));
        assert_eq!(parse_duration_ms("1:60"), None);
        assert_eq!(parse_duration_ms("1:02.9999"), None);
        assert_eq!(parse_duration_ms("nope"), None);
    }

    #[test]
    fn execute_ping_and_missing_action() {
        let hub = Hub::new();
        assert_eq!(
            execute(&hub.hub, r#"{"action":"ping","params":[]}"#),
            "ping: pong"
        );
        assert!(execute(&hub.hub, r#"{"params":[]}"#).starts_with("command: error:"));
    }
}
