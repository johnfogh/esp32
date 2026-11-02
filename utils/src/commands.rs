//! BLE commands from the manager: JSON lines written to [`crate::ble_proto::CMD_CHAR_UUID`].
//!
//! Format: `{"action":"<name>","params":[p1,p2,…]}` + `\n` (params: strings or numbers), sent in
//! chunks of any size; the firmware reassembles until `\n`. Only the authenticated manager's writes
//! are accepted. A worker thread (started by `bluetooth_announce_background`) runs the method and
//! sends one result line on the log channel: `<action>: <result>` or `<action>: error: <msg>`.
//! Non-silent methods also store `{method, result, error}` in the status buffer.
//!
//! Methods (add new ones in [`dispatch`]):
//! - `get_accelerometers` `[unit, count, interval]`: unit `g` (default) / `m/s²` / `raw`, count
//!   (default 1), interval `MM:SS.mmm` or milliseconds (default 0). See [`crate::imu`].
//! - `ping` `[]`: replies `pong`.
//! - `set_time` `[ms]`: controller clock (ms since epoch) for [`crate::status::synced_time`]; silent.
//! - `buffer_ack` `[seq, …]`: acknowledges status-buffer snapshots ([`crate::status`]); silent.
//!
//! Projects can handle extra actions with [`set_handler`]. Unknown actions go there; if no handler
//! is set they error `unknown action`. `read_buffer` is silent in the status snapshot (result still
//! goes out on the log characteristic).
#[cfg(feature = "hal")]
use alloc::{format, string::String, vec::Vec};

use core::sync::atomic::{AtomicPtr, Ordering};

/// Project-specific BLE action: `(action, params) -> result`.
pub type Handler = fn(&str, &[&str]) -> Result<String, String>;

static HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Install the project command handler. Call once from `app` before [`crate::bluetooth_announce_background`].
pub fn set_handler(handler: Handler) {
    HANDLER.store(handler as *mut (), Ordering::Release);
}

fn project_handler() -> Option<Handler> {
    let p = HANDLER.load(Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute(p) })
    }
}

/// Longest accepted command line.
pub const MAX_LEN: usize = 256;

pub(crate) struct Pending {
    buf: [u8; MAX_LEN],
    len: usize,
    /// A complete line is waiting for the worker.
    ready: bool,
    overflow: bool,
}

impl Pending {
    const fn new() -> Self { Self { buf: [0; MAX_LEN], len: 0, ready: false, overflow: false } }
}

#[cfg(feature = "hal")]
mod sync {
    use core::cell::RefCell;
    use critical_section::Mutex;
    static P: Mutex<RefCell<super::Pending>> = Mutex::new(RefCell::new(super::Pending::new()));
    pub fn with<R>(f: impl FnOnce(&mut super::Pending) -> R) -> R {
        critical_section::with(|cs| f(&mut P.borrow(cs).borrow_mut()))
    }
}
#[cfg(not(feature = "hal"))]
mod sync {
    static P: std::sync::Mutex<super::Pending> = std::sync::Mutex::new(super::Pending::new());
    pub fn with<R>(f: impl FnOnce(&mut super::Pending) -> R) -> R {
        f(&mut P.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Append bytes written by the manager (non-blocking). Data arriving while a command is still
/// queued is dropped (one command at a time). Quick actions (`set_time`, `buffer_ack`) run inline
/// here, silently, so they are never stuck behind a long-running command.
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn submit(data: &[u8]) {
    let mut quick: Option<String> = None;
    sync::with(|p| {
        for &c in data {
            if c == b'\n' {
                if p.ready { continue; }
                if p.overflow { p.len = 0; p.overflow = false; }
                else if p.len > 0 {
                    let line = String::from_utf8_lossy(&p.buf[..p.len]);
                    if is_quick(&line) { quick = Some(line.into()); p.len = 0; } else { p.ready = true; }
                }
            } else if p.ready {
                continue;
            } else if p.len < MAX_LEN {
                p.buf[p.len] = c;
                p.len += 1;
            } else {
                p.overflow = true;
            }
        }
    });
    if let Some(l) = quick {
        let r = execute(&l);
        if r.contains(": error:") { log::warn!("cmd: {r}"); }
    }
}

fn is_quick(line: &str) -> bool {
    matches!(json_str_field(line, "action").as_deref(), Some("set_time" | "buffer_ack"))
}

fn take() -> Option<String> {
    sync::with(|p| {
        if !p.ready { return None; }
        let s = String::from_utf8_lossy(&p.buf[..p.len]).into();
        p.len = 0;
        p.ready = false;
        Some(s)
    })
}

/// Worker loop: run queued commands, send results; `sleep` waits ~50 ms.
#[cfg_attr(not(all(feature = "ble", not(feature = "esp32s2"))), allow(dead_code))]
pub(crate) fn worker(mut sleep: impl FnMut()) -> ! {
    loop {
        if let Some(cmd) = take() {
            let line = execute(&cmd);
            // Wait (bounded) for room in the BLE log buffer: results can be long.
            let mut sent = false;
            for _ in 0..100 {
                if !crate::blelog::connected() || crate::blelog::ble_log(&line) { sent = true; break; }
                sleep();
            }
            if !sent { log::warn!("cmd: result dropped ({} bytes)", line.len()); }
        }
        sleep();
    }
}

/// Parse and run one command line; returns the result line.
pub fn execute(line: &str) -> String {
    let Some(action) = json_str_field(line, "action") else { return format!("command: error: no \"action\" in {line}") };
    let params = json_params(line);
    let p: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
    match dispatch(&action, &p) {
        Ok(r) => {
            publish_result(&action, &r, false);
            format!("{action}: {r}")
        }
        Err(e) => {
            let msg = format!("error: {e}");
            publish_result(&action, &msg, true);
            format!("{action}: {msg}")
        }
    }
}

fn publish_result(method: &str, result: &str, error: bool) {
    if matches!(method, "set_time" | "buffer_ack" | "read_buffer") {
        return;
    }
    crate::status::buffer_add_sample(None, &[("method", &method), ("result", &result), ("error", &error)]);
}

/// Action table.
fn dispatch(action: &str, p: &[&str]) -> Result<String, String> {
    match action {
        "ping" => Ok("pong".into()),
        "set_time" => {
            let ms = p.first().and_then(|s| s.trim().parse::<u64>().ok()).ok_or("need [ms since epoch]")?;
            crate::status::set_time(ms);
            Ok("ok".into())
        }
        "buffer_ack" => {
            let mut seqs = [0u16; 32];
            let mut n = 0;
            for x in p.iter().take(seqs.len()) {
                seqs[n] = x.trim().parse::<u16>().map_err(|_| format!("bad seq {x:?}"))?;
                n += 1;
            }
            if n == 0 { return Err("need [seq, …]".into()); }
            crate::status::ack(&seqs[..n]);
            Ok("ok".into())
        }
        "get_accelerometers" => {
            let unit = crate::imu::Unit::parse(p.first().copied().unwrap_or("")).ok_or_else(|| format!("bad unit {:?}", p[0]))?;
            let count = match p.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                None => 1,
                Some(s) => s.parse::<u32>().map_err(|_| format!("bad count {s:?}"))?,
            };
            let interval = match p.get(2).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                None => 0,
                Some(s) => parse_duration_ms(s).ok_or_else(|| format!("bad interval {s:?} (MM:SS.mmm or ms)"))?,
            };
            crate::imu::get_accelerometers(unit, count, interval).map_err(|e| format!("{e}"))
        }
        other => match project_handler() {
            Some(h) => h(other, p),
            None => Err(format!("unknown action {other:?}")),
        },
    }
}

/// `MM:SS.mmm` (also `SS.mmm`, `MM:SS`) or plain milliseconds.
pub fn parse_duration_ms(s: &str) -> Option<u32> {
    if !s.contains(':') && !s.contains('.') { return s.parse().ok(); }
    let (m, rest) = match s.split_once(':') { Some((m, r)) => (m.parse::<u32>().ok()?, r), None => (0, s) };
    let (sec, ms) = match rest.split_once('.') {
        Some((a, b)) if !b.is_empty() && b.len() <= 3 => (a.parse::<u32>().ok()?, b.parse::<u32>().ok()? * 10u32.pow(3 - b.len() as u32)),
        Some(_) => return None,
        None => (rest.parse::<u32>().ok()?, 0),
    };
    if sec >= 60 && s.contains(':') { return None; }
    Some(m * 60_000 + sec * 1000 + ms)
}

/// Value of a top-level string field (minimal JSON; escapes `\"` and `\\` handled).
fn json_str_field(s: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\"");
    let i = s.find(&k)? + k.len();
    let rest = s[i..].trim_start().strip_prefix(':')?.trim_start();
    let (v, _) = json_string(rest)?;
    Some(v)
}

/// Parse a JSON string at the start of `s`; returns (value, rest).
fn json_string(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut it = s.strip_prefix('"')?.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some((out, &s[i + 2..])),
            '\\' => match it.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => { for _ in 0..4 { it.next()?; } out.push('?'); }
                e => out.push(e),
            },
            c => out.push(c),
        }
    }
    None
}

/// Items of the top-level `"params"` array as strings (strings unquoted, other scalars verbatim).
fn json_params(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let Some(i) = s.find("\"params\"") else { return v };
    let Some(mut rest) = s[i + 8..].trim_start().strip_prefix(':').and_then(|r| r.trim_start().strip_prefix('[')) else { return v };
    loop {
        rest = rest.trim_start();
        if rest.starts_with(']') || rest.is_empty() { break; }
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
        match rest.strip_prefix(',') { Some(r) => rest = r, None => break }
    }
    v
}
