//! OS-paired or OS-connected BlueZ devices. GATT only while the OS already has the device connected.
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bluer::{Adapter, AdapterEvent, Address, Device, DeviceEvent, DeviceProperty, Session};
use bluer::gatt::remote::Characteristic;
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use uuid::Uuid;

use utils::ble_proto::{
    self, AUTH_CHAR_UUID_U128, BUF_CHAR_UUID_U128, CMD_CHAR_UUID_U128, COMPANY_ID, LOG_CHAR_UUID_U128, LOG_CHUNK,
    MANAGER_TOKEN, SERVICE_UUID_U128, STATUS_CHAR_UUID_U128,
};

use crate::local::ClientEv;

type EvTx = mpsc::Sender<BleEvent>;

const TIME_SYNC: Duration = Duration::from_secs(30);
const ACK_BATCH: Duration = Duration::from_millis(200);
const ACK_BATCH_MAX: usize = 8;
const SEQ_DEDUP_WINDOW: Duration = Duration::from_secs(8);
const PAIR_POLL: Duration = Duration::from_secs(2);
const HUB_POLL: Duration = Duration::from_millis(200);

pub fn epoch() -> Instant {
    static E: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *E.get_or_init(Instant::now)
}

#[derive(Clone)]
pub enum BleCmd {
    Command(String, String),
    SetHub(std::sync::Arc<crate::sensor::Hub>),
}

impl std::fmt::Debug for BleCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(a, l) => write!(f, "Command({a}, {l})"),
            Self::SetHub(_) => write!(f, "SetHub"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub addr: String,
    pub name: String,
    pub project: String,
    pub mac: String,
    pub rssi: Option<i16>,
}

#[derive(Debug, Clone)]
pub enum BleEvent {
    Ready(String),
    Discovered(DeviceInfo),
    Connected(DeviceInfo),
    Disconnected(String),
    Removed(String),
    Rssi(String, i16),
    TimeOffset { addr: String, offset_ms: f64, rtt_ms: f64 },
    Error(String),
    Info(String),
    DeviceLine(String, String),
}

pub fn log_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".local/state")
        })
        .join("esp32-controller/events.log")
}

pub fn log_event(line: &str) {
    let p = log_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(
            f,
            "{} {line}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        );
    }
}

pub type Sender = UnboundedSender<BleCmd>;

pub fn wanted_adapter() -> Option<String> {
    std::env::var("ESP32_BLE_ADAPTER")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("VIRTUAL_TARGET_CENTRAL").ok().filter(|s| !s.is_empty()))
}

pub(crate) fn is_virtual(hci: &str) -> bool {
    if hci == "hci0" {
        return false;
    }
    let p = PathBuf::from("/sys/class/bluetooth").join(hci);
    std::fs::canonicalize(&p)
        .unwrap_or(p)
        .components()
        .any(|c| c.as_os_str() == "virtual")
}

pub fn spawn(
    want: Option<String>,
    hub: Option<std::sync::Arc<crate::sensor::Hub>>,
) -> (UnboundedSender<BleCmd>, Receiver<BleEvent>) {
    let (ctx, crx) = unbounded_channel();
    let (etx, erx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ble-central".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = etx.send(BleEvent::Error(format!("tokio runtime: {e}")));
                    return;
                }
            };
            rt.block_on(run(want, hub, crx, etx));
        })
        .expect("ble-central thread");
    (ctx, erx)
}

struct Sess {
    cmd: Option<Characteristic>,
    local: Option<tokio::sync::mpsc::UnboundedSender<String>>,
}

struct Inner {
    adapter: Option<Adapter>,
    sessions: HashMap<String, Sess>,
    info: HashMap<String, DeviceInfo>,
    watching: HashSet<String>,
    hub: Option<std::sync::Arc<crate::sensor::Hub>>,
    listen: bool,
}

async fn run(
    want: Option<String>,
    hub: Option<std::sync::Arc<crate::sensor::Hub>>,
    mut cmds: tokio::sync::mpsc::UnboundedReceiver<BleCmd>,
    tx: EvTx,
) {
    let inner = Arc::new(Mutex::new(Inner {
        adapter: None,
        sessions: HashMap::new(),
        info: HashMap::new(),
        watching: HashSet::new(),
        hub: hub.clone(),
        listen: false,
    }));
    if let Some(h) = hub {
        start_listen(&inner, h);
    }
    match open_adapter(want.or_else(wanted_adapter).as_deref()).await {
        Ok((adapter, name)) => {
            inner.lock().unwrap().adapter = Some(adapter.clone());
            let _ = tx.send(BleEvent::Ready(name));
            let i = inner.clone();
            let t = tx.clone();
            tokio::spawn(async move { watch_adapter(i, t).await });
            let i = inner.clone();
            let t = tx.clone();
            tokio::spawn(async move { poll_paired(i, t).await });
        }
        Err(e) => {
            let _ = tx.send(BleEvent::Error(e));
        }
    }
    {
        let i = inner.clone();
        let t = tx.clone();
        tokio::spawn(async move { poll_hub(i, t).await });
    }
    while let Some(cmd) = cmds.recv().await {
        match cmd {
            BleCmd::SetHub(h) => {
                start_listen(&inner, h.clone());
                inner.lock().unwrap().hub = Some(h);
            }
            BleCmd::Command(addr, line) => command(&inner, &tx, addr, line).await,
        }
    }
}

fn start_listen(inner: &Arc<Mutex<Inner>>, hub: std::sync::Arc<crate::sensor::Hub>) {
    let mut s = inner.lock().unwrap();
    if s.listen {
        return;
    }
    s.listen = true;
    tokio::spawn(crate::local::listen(hub));
}

async fn open_adapter(want: Option<&str>) -> Result<(Adapter, String), String> {
    let session = Session::new().await.map_err(|e| format!("bluetooth session: {e}"))?;
    let names = session.adapter_names().await.map_err(|e| format!("adapters: {e}"))?;
    if names.is_empty() {
        return Err("no Bluetooth adapter found".into());
    }
    let pick = if let Some(w) = want {
        names.iter().find(|n| n.eq_ignore_ascii_case(w)).cloned()
    } else {
        None
    };
    let name = pick
        .or_else(|| names.iter().find(|n| !is_virtual(n)).cloned())
        .unwrap_or_else(|| names[0].clone());
    let adapter = session.adapter(&name).map_err(|e| format!("{name}: {e}"))?;
    Ok((adapter, name))
}

async fn watch_adapter(inner: Arc<Mutex<Inner>>, tx: EvTx) {
    let adapter = inner.lock().unwrap().adapter.clone();
    let Some(adapter) = adapter else { return };
    if let Ok(addrs) = adapter.device_addresses().await {
        for a in addrs {
            consider(&inner, &tx, &adapter, a).await;
        }
    }
    let mut events = match adapter.events().await {
        Ok(e) => e,
        Err(e) => {
            let _ = tx.send(BleEvent::Error(format!("adapter events: {e}")));
            return;
        }
    };
    while let Some(ev) = events.next().await {
        match ev {
            AdapterEvent::DeviceAdded(a) => consider(&inner, &tx, &adapter, a).await,
            AdapterEvent::DeviceRemoved(a) => forget(&inner, &tx, &a.to_string()),
            _ => {}
        }
    }
}

async fn poll_paired(inner: Arc<Mutex<Inner>>, tx: EvTx) {
    loop {
        tokio::time::sleep(PAIR_POLL).await;
        let adapter = inner.lock().unwrap().adapter.clone();
        let Some(adapter) = adapter else { continue };
        let Ok(addrs) = adapter.device_addresses().await else { continue };
        let live: HashSet<String> = addrs.iter().map(|a| a.to_string()).collect();
        let known: Vec<String> = inner.lock().unwrap().info.keys().cloned().collect();
        for k in known {
            if !live.iter().any(|a| a.eq_ignore_ascii_case(&k))
                && inner.lock().unwrap().sessions.get(&k).is_none_or(|s| s.local.is_none())
            {
                forget(&inner, &tx, &k);
            }
        }
        for a in addrs {
            consider(&inner, &tx, &adapter, a).await;
        }
    }
}

async fn poll_hub(inner: Arc<Mutex<Inner>>, tx: EvTx) {
    loop {
        tokio::time::sleep(HUB_POLL).await;
        let hub = inner.lock().unwrap().hub.clone();
        let Some(hub) = hub else { continue };
        let mut boards = hub.manager_boards();
        if hub.hub_on() {
            boards.push(hub.hub.clone());
        }
        let mut live = HashSet::new();
        for b in boards {
            let s = b.snapshot();
            let addr = if s.mac.is_empty() { continue } else { s.mac.clone() };
            live.insert(addr.to_ascii_lowercase());
            if inner.lock().unwrap().sessions.contains_key(&addr)
                || inner
                    .lock()
                    .unwrap()
                    .sessions
                    .keys()
                    .any(|k| k.eq_ignore_ascii_case(&addr))
            {
                continue;
            }
            connect_local(&inner, &tx, addr, hub.clone()).await;
        }
        let drop: Vec<String> = inner
            .lock()
            .unwrap()
            .sessions
            .iter()
            .filter(|(k, s)| s.local.is_some() && !live.iter().any(|a| a.eq_ignore_ascii_case(k)))
            .map(|(k, _)| k.clone())
            .collect();
        for addr in drop {
            drop_session(&inner, &tx, &addr, true);
        }
    }
}

fn forget(inner: &Arc<Mutex<Inner>>, tx: &EvTx, addr: &str) {
    if inner.lock().unwrap().sessions.get(addr).is_some_and(|s| s.local.is_some()) {
        return;
    }
    let had = inner.lock().unwrap().info.remove(addr).is_some()
        || inner.lock().unwrap().sessions.contains_key(addr);
    drop_session(inner, tx, addr, false);
    inner.lock().unwrap().watching.remove(addr);
    if had {
        log_event(&format!("REMOVED {addr}"));
        let _ = tx.send(BleEvent::Removed(addr.to_string()));
    }
}

fn drop_session(inner: &Arc<Mutex<Inner>>, tx: &EvTx, addr: &str, removed: bool) {
    let sess = {
        let mut s = inner.lock().unwrap();
        s.sessions.remove(addr).or_else(|| {
            let k = s.sessions.keys().find(|k| k.eq_ignore_ascii_case(addr)).cloned()?;
            s.sessions.remove(&k)
        })
    };
    if sess.is_some() {
        log_event(&format!("DISCONNECTED {addr}"));
        let _ = tx.send(BleEvent::Disconnected(addr.to_string()));
    }
    if removed {
        inner.lock().unwrap().info.remove(addr);
        let _ = tx.send(BleEvent::Removed(addr.to_string()));
    }
}

async fn consider(inner: &Arc<Mutex<Inner>>, tx: &EvTx, adapter: &Adapter, addr: Address) {
    let key = addr.to_string();
    let Ok(dev) = adapter.device(addr) else { return };
    let paired = dev.is_paired().await.unwrap_or(false);
    let connected = dev.is_connected().await.unwrap_or(false);
    if !paired && !connected {
        if inner.lock().unwrap().info.contains_key(&key) {
            forget(inner, tx, &key);
        }
        return;
    }
    let Some(info) = ours(&dev, &key).await else { return };
    {
        let mut s = inner.lock().unwrap();
        s.info.insert(key.clone(), info.clone());
    }
    let _ = tx.send(BleEvent::Discovered(info.clone()));
    watch_device(inner, tx, adapter, addr);
    if connected {
        attach_gatt(inner, tx, adapter, addr, info).await;
    } else if inner.lock().unwrap().sessions.contains_key(&key) {
        drop_session(inner, tx, &key, false);
    }
}

fn watch_device(inner: &Arc<Mutex<Inner>>, tx: &EvTx, adapter: &Adapter, addr: Address) {
    let key = addr.to_string();
    {
        let mut s = inner.lock().unwrap();
        if !s.watching.insert(key.clone()) {
            return;
        }
    }
    let adapter = adapter.clone();
    let inner = inner.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let Ok(dev) = adapter.device(addr) else {
            inner.lock().unwrap().watching.remove(&key);
            return;
        };
        let Ok(mut events) = dev.events().await else {
            inner.lock().unwrap().watching.remove(&key);
            return;
        };
        while let Some(ev) = events.next().await {
            match ev {
                DeviceEvent::PropertyChanged(DeviceProperty::Paired(false)) => {
                    consider(&inner, &tx, &adapter, addr).await;
                }
                DeviceEvent::PropertyChanged(DeviceProperty::Connected(true)) => {
                    let info = inner.lock().unwrap().info.get(&key).cloned();
                    if let Some(info) = info {
                        attach_gatt(&inner, &tx, &adapter, addr, info).await;
                    } else {
                        consider(&inner, &tx, &adapter, addr).await;
                    }
                }
                DeviceEvent::PropertyChanged(DeviceProperty::Connected(false)) => {
                    drop_session(&inner, &tx, &key, false);
                }
                _ => {}
            }
        }
        inner.lock().unwrap().watching.remove(&key);
    });
}

async fn ours(dev: &Device, addr: &str) -> Option<DeviceInfo> {
    let name = match dev.alias().await {
        Ok(a) if !a.is_empty() => a,
        _ => dev.name().await.ok().flatten().unwrap_or_else(|| "?".into()),
    };
    let named = ble_proto::parse_name(&name).is_some();
    let uuids = dev.uuids().await.ok().flatten().unwrap_or_default();
    let has_svc = uuids.contains(&Uuid::from_u128(SERVICE_UUID_U128));
    let mfg = dev
        .manufacturer_data()
        .await
        .ok()
        .flatten()
        .and_then(|m| m.get(&COMPANY_ID).cloned())
        .and_then(|d| ble_proto::parse_manufacturer_mac(&d));
    if !has_svc && mfg.is_none() && !named {
        return None;
    }
    let project = ble_proto::parse_name(&name).map_or("?", |(p, _)| p).to_string();
    let rssi = dev.rssi().await.ok().flatten().map(|r| r as i16);
    Some(DeviceInfo {
        addr: addr.to_string(),
        name,
        project,
        mac: mfg.map_or_else(|| "?".into(), fmt_mac),
        rssi,
    })
}

fn fmt_mac(mac: [u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

async fn attach_gatt(
    inner: &Arc<Mutex<Inner>>,
    tx: &EvTx,
    adapter: &Adapter,
    addr: Address,
    info: DeviceInfo,
) {
    let key = addr.to_string();
    if inner.lock().unwrap().sessions.contains_key(&key) {
        return;
    }
    let Ok(dev) = adapter.device(addr) else { return };
    if !dev.is_connected().await.unwrap_or(false) {
        return;
    }
    log_event(&format!("GATT {key}"));
    let chars = match find_chars(&dev).await {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(BleEvent::Error(format!("{key}: {e}")));
            return;
        }
    };
    if let Some(auth) = &chars.auth
        && let Err(e) = auth.write(MANAGER_TOKEN).await
    {
        let _ = tx.send(BleEvent::Error(format!("{key}: auth: {e}")));
        return;
    }
    if let Some(c) = chars.cmd.clone() {
        if let Err(e) = write_cmd(&c, &set_time_line()).await {
            let _ = tx.send(BleEvent::Info(format!("{key}: set_time: {e}")));
        }
        tokio::spawn(time_sync(c.clone(), tx.clone(), key.clone(), dev.clone()));
    }
    inner.lock().unwrap().sessions.insert(
        key.clone(),
        Sess {
            cmd: chars.cmd.clone(),
            local: None,
        },
    );
    log_event(&format!("CONNECTED {key}"));
    let _ = tx.send(BleEvent::Connected(info));
    if let Some(st) = chars.status.clone() {
        tokio::spawn(poll_status(st, tx.clone(), key.clone()));
    }
    subscribe(&chars, tx, &key).await;
}

struct Chars {
    auth: Option<Characteristic>,
    cmd: Option<Characteristic>,
    log: Option<Characteristic>,
    buf: Option<Characteristic>,
    status: Option<Characteristic>,
}

async fn find_chars(dev: &Device) -> Result<Chars, String> {
    let svc_uuid = Uuid::from_u128(SERVICE_UUID_U128);
    let services = dev.services().await.map_err(|e| format!("services: {e}"))?;
    let mut svc = None;
    for s in services {
        if s.uuid().await.ok() == Some(svc_uuid) {
            svc = Some(s);
            break;
        }
    }
    let svc = svc.ok_or("service not found")?;
    let list = svc.characteristics().await.map_err(|e| format!("characteristics: {e}"))?;
    let mut out = Chars {
        auth: None,
        cmd: None,
        log: None,
        buf: None,
        status: None,
    };
    for c in list {
        let Ok(u) = c.uuid().await else { continue };
        if u == Uuid::from_u128(AUTH_CHAR_UUID_U128) {
            out.auth = Some(c);
        } else if u == Uuid::from_u128(CMD_CHAR_UUID_U128) {
            out.cmd = Some(c);
        } else if u == Uuid::from_u128(LOG_CHAR_UUID_U128) {
            out.log = Some(c);
        } else if u == Uuid::from_u128(BUF_CHAR_UUID_U128) {
            out.buf = Some(c);
        } else if u == Uuid::from_u128(STATUS_CHAR_UUID_U128) {
            out.status = Some(c);
        }
    }
    Ok(out)
}

async fn subscribe(chars: &Chars, tx: &EvTx, addr: &str) {
    let log_c = chars.log.clone();
    let buf_c = chars.buf.clone();
    let cmd = chars.cmd.clone();
    if log_c.is_none() && buf_c.is_none() {
        let _ = tx.send(BleEvent::Info(format!("{addr}: no log/buffer characteristic")));
        return;
    }
    let _ = tx.send(BleEvent::Info(format!("{addr}: subscribed to board output")));
    let (ack_tx, ack_rx) = unbounded_channel::<u16>();
    if let Some(c) = cmd.clone() {
        tokio::spawn(ack_batcher(c, tx.clone(), addr.to_string(), ack_rx));
    }
    if let Some(c) = log_c {
        let tx = tx.clone();
        let addr = addr.to_string();
        tokio::spawn(async move { pump_log(c, tx, addr).await });
    }
    if let Some(c) = buf_c {
        let tx = tx.clone();
        let addr = addr.to_string();
        tokio::spawn(async move { pump_buf(c, tx, addr, ack_tx).await });
    }
}

async fn pump_log(c: Characteristic, tx: EvTx, addr: String) {
    let n = match c.notify().await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(BleEvent::Info(format!("{addr}: log notify: {e}")));
            return;
        }
    };
    tokio::pin!(n);
    let mut buf: Vec<u8> = Vec::new();
    while let Some(value) = n.next().await {
        buf.extend_from_slice(&value);
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line[..i]).trim_end_matches('\r').to_string();
            let _ = tx.send(BleEvent::DeviceLine(addr.clone(), line));
        }
        if buf.len() > 4096 {
            buf.clear();
        }
    }
}

async fn pump_buf(
    c: Characteristic,
    tx: EvTx,
    addr: String,
    ack_tx: tokio::sync::mpsc::UnboundedSender<u16>,
) {
    let n = match c.notify().await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(BleEvent::Info(format!("{addr}: buffer notify: {e}")));
            return;
        }
    };
    tokio::pin!(n);
    let mut frames = Frames::default();
    let mut shown: VecDeque<(u16, Instant)> = VecDeque::new();
    while let Some(value) = n.next().await {
        let r = frames.push(&value);
        if let FrameResult::Error(e) = &r {
            let _ = tx.send(BleEvent::Info(format!("{addr}: {e}")));
        }
        if let FrameResult::Done(seq, lines) = r {
            while shown.front().is_some_and(|(_, t)| t.elapsed() > SEQ_DEDUP_WINDOW) {
                shown.pop_front();
            }
            if !shown.iter().any(|(s, _)| *s == seq) {
                shown.push_back((seq, Instant::now()));
                if shown.len() > 512 {
                    shown.pop_front();
                }
                for l in lines {
                    let _ = tx.send(BleEvent::DeviceLine(addr.clone(), l));
                }
            }
            let _ = ack_tx.send(seq);
        }
    }
}

async fn connect_local(
    inner: &Arc<Mutex<Inner>>,
    tx: &EvTx,
    addr: String,
    hub: std::sync::Arc<crate::sensor::Hub>,
) {
    if inner.lock().unwrap().sessions.keys().any(|k| k.eq_ignore_ascii_case(&addr)) {
        return;
    }
    let Some(board) = crate::local::find_board(&hub, &addr) else {
        return;
    };
    let snap = board.snapshot();
    match crate::local::attach(board) {
        Ok((io_tx, ev_rx)) => {
            let info = DeviceInfo {
                addr: addr.clone(),
                name: snap.name,
                project: "sensor".into(),
                mac: snap.mac,
                rssi: None,
            };
            inner.lock().unwrap().info.insert(addr.clone(), info.clone());
            inner.lock().unwrap().sessions.insert(
                addr.clone(),
                Sess {
                    cmd: None,
                    local: Some(io_tx.clone()),
                },
            );
            let _ = io_tx.send(set_time_line());
            log_event(&format!("CONNECTED {addr} (local)"));
            let _ = tx.send(BleEvent::Discovered(info.clone()));
            let _ = tx.send(BleEvent::Connected(info));
            tokio::spawn(forward_local(addr, ev_rx, tx.clone(), io_tx));
        }
        Err(e) => {
            let _ = tx.send(BleEvent::Error(format!("{addr}: {e}")));
        }
    }
}

async fn forward_local(
    addr: String,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ClientEv>,
    tx: EvTx,
    cmd: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut frames = Frames::default();
    let mut shown: VecDeque<(u16, Instant)> = VecDeque::new();
    while let Some(ev) = rx.recv().await {
        match ev {
            ClientEv::Log(line) => {
                let _ = tx.send(BleEvent::DeviceLine(addr.clone(), line));
            }
            ClientEv::Buf(data) => {
                if let FrameResult::Done(seq, lines) = frames.push(&data) {
                    while shown.front().is_some_and(|(_, t)| t.elapsed() > SEQ_DEDUP_WINDOW) {
                        shown.pop_front();
                    }
                    if !shown.iter().any(|(s, _)| *s == seq) {
                        shown.push_back((seq, Instant::now()));
                        if shown.len() > 512 {
                            shown.pop_front();
                        }
                        for l in lines {
                            let _ = tx.send(BleEvent::DeviceLine(addr.clone(), l));
                        }
                    }
                    let _ = cmd.send(buffer_ack_line(&[seq]));
                }
            }
            ClientEv::Status(v) => {
                if let Some((_, rssi)) = ble_proto::parse_status(&v)
                    && let Some(r) = rssi
                {
                    let _ = tx.send(BleEvent::Rssi(addr.clone(), r as i16));
                }
            }
        }
    }
    let _ = tx.send(BleEvent::Disconnected(addr));
}

async fn command(inner: &Arc<Mutex<Inner>>, tx: &EvTx, addr: String, line: String) {
    let sess = inner.lock().unwrap().sessions.iter().find(|(k, _)| k.eq_ignore_ascii_case(&addr)).map(|(_, s)| {
        (s.cmd.clone(), s.local.clone())
    });
    match sess {
        Some((_, Some(local))) => {
            if local.send(line.clone()).is_err() {
                let _ = tx.send(BleEvent::Info(format!("{addr}: not connected, command not sent")));
            } else {
                let _ = tx.send(BleEvent::Info(format!("{addr}: sent {line}")));
            }
        }
        Some((Some(c), _)) => {
            if let Err(e) = write_cmd(&c, &line).await {
                let _ = tx.send(BleEvent::Info(format!("{addr}: command write: {e}")));
            } else {
                let _ = tx.send(BleEvent::Info(format!("{addr}: sent {line}")));
            }
        }
        _ => {
            let _ = tx.send(BleEvent::Info(format!("{addr}: not connected, command not sent")));
        }
    }
}

#[derive(Default)]
struct Frames {
    seq: Option<u8>,
    next: u8,
    data: Vec<u8>,
}

enum FrameResult {
    Pending,
    Done(u16, Vec<String>),
    Error(String),
}

impl Frames {
    fn push(&mut self, f: &[u8]) -> FrameResult {
        use utils::ble_proto::{BUF_FRAME_FIRST, BUF_FRAME_INDEX, BUF_FRAME_LAST};
        let (Some(&seq), Some(&flags)) = (f.first(), f.get(1)) else {
            return FrameResult::Error("buffer: short frame".into());
        };
        let idx = flags & BUF_FRAME_INDEX;
        if flags & BUF_FRAME_FIRST != 0 {
            self.seq = Some(seq);
            self.next = 0;
            self.data.clear();
        } else if self.seq != Some(seq) {
            if self.seq.is_some() {
                let msg = format!("buffer: seq {seq} frame {idx} without first frame (had seq {:?}); dropped", self.seq);
                self.seq = None;
                self.data.clear();
                return FrameResult::Error(msg);
            }
            return FrameResult::Pending;
        }
        if idx != self.next {
            let msg = format!("buffer: seq {seq} out of order (frame {idx}, expected {}); transfer dropped", self.next);
            self.seq = None;
            self.data.clear();
            return FrameResult::Error(msg);
        }
        self.data.extend_from_slice(&f[2..]);
        self.next = (self.next + 1) & BUF_FRAME_INDEX;
        if flags & BUF_FRAME_LAST == 0 {
            return FrameResult::Pending;
        }
        self.seq = None;
        let data = std::mem::take(&mut self.data);
        match serde_json::from_slice::<serde_json::Value>(&data) {
            Ok(v) => {
                let s = v.get("seq").and_then(|x| x.as_u64()).map(|x| x as u16).unwrap_or(seq as u16);
                FrameResult::Done(s, vec![snapshot_line(s, v.get("buffer"))])
            }
            Err(e) => FrameResult::Error(format!("buffer: seq {seq}: invalid JSON ({e}, {} bytes)", data.len())),
        }
    }
}

fn buffer_ack_line(seqs: &[u16]) -> String {
    let list: Vec<String> = seqs.iter().map(|s| s.to_string()).collect();
    format!(r#"{{"action":"buffer_ack","params":[{}]}}"#, list.join(","))
}

fn snapshot_line(seq: u16, buf: Option<&serde_json::Value>) -> String {
    match buf {
        Some(b) if b.is_object() => format!("buffer seq {seq}: {b}"),
        _ => format!("buffer seq {seq}: (empty)"),
    }
}

async fn ack_batcher(
    c: Characteristic,
    tx: EvTx,
    addr: String,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<u16>,
) {
    let mut pending: Vec<u16> = Vec::new();
    loop {
        let first = if pending.is_empty() {
            match rx.recv().await {
                Some(s) => Some(s),
                None => return,
            }
        } else {
            None
        };
        if let Some(s) = first {
            pending.push(s);
        }
        let deadline = tokio::time::sleep(ACK_BATCH);
        tokio::pin!(deadline);
        let mut closed = false;
        while pending.len() < ACK_BATCH_MAX {
            tokio::select! {
                s = rx.recv() => match s {
                    Some(s) => { if !pending.contains(&s) { pending.push(s); } }
                    None => { closed = true; break }
                },
                _ = &mut deadline => break,
            }
        }
        let line = buffer_ack_line(&pending);
        pending.clear();
        if let Err(e) = write_cmd(&c, &line).await {
            let _ = tx.send(BleEvent::Info(format!("{addr}: buffer ack: {e}")));
        }
        if closed {
            return;
        }
    }
}

fn set_time_line() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!(r#"{{"action":"set_time","params":[{ms}]}}"#)
}

async fn time_sync(c: Characteristic, tx: EvTx, addr: String, dev: Device) {
    loop {
        tokio::time::sleep(TIME_SYNC).await;
        if !dev.is_connected().await.unwrap_or(false) {
            return;
        }
        if let Err(e) = write_cmd(&c, &set_time_line()).await {
            let _ = tx.send(BleEvent::Info(format!("{addr}: time sync: {e}")));
            return;
        }
    }
}

async fn write_cmd(c: &Characteristic, line: &str) -> Result<(), String> {
    let data = format!("{line}\n");
    for chunk in data.as_bytes().chunks(LOG_CHUNK) {
        c.write(chunk).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn poll_status(c: Characteristic, tx: EvTx, addr: String) {
    let t0 = Instant::now();
    let Ok(Ok(v)) = tokio::time::timeout(Duration::from_secs(2), c.read()).await else {
        return;
    };
    let t1 = Instant::now();
    let Some((up, rssi)) = ble_proto::parse_status(&v) else { return };
    if let Some(r) = rssi {
        let _ = tx.send(BleEvent::Rssi(addr.clone(), r as i16));
    }
    let rtt = t1 - t0;
    let mid = (t0 + rtt / 2).duration_since(epoch());
    let offset_ms = up as f64 - mid.as_secs_f64() * 1000.0;
    let _ = tx.send(BleEvent::TimeOffset {
        addr,
        offset_ms,
        rtt_ms: rtt.as_secs_f64() * 1000.0,
    });
}
