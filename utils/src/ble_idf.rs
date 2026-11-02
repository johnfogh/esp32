//! esp-idf BLE backend: NimBLE via the `esp32-nimble` crate.
//!
//! Needs `CONFIG_BT_ENABLED=y`, `CONFIG_BT_BLUEDROID_ENABLED=n`, `CONFIG_BT_NIMBLE_ENABLED=y` in the
//! consumer's `sdkconfig.defaults`. The NimBLE host runs in its own task; callbacks forward events to
//! the blocking caller over a channel, which enforces the 5 s authentication deadline on unbonded links.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use esp32_nimble::enums::{AuthReq, PairKeyDist, SecurityIOCap};
use esp32_nimble::{BLEDevice, BLEServer, NimbleProperties, utilities::BleUuid};

use super::{AUTH_CHAR_UUID, AUTH_TIMEOUT_MS, BleError, LOG_CHAR_UUID, LOG_CHUNK, MANAGER_TOKEN, SERVICE_UUID};

pub struct Inner {
    handle: u16,
    peer: [u8; 6],
}

impl Inner {
    pub fn conn_handle(&self) -> u16 { self.handle }
    pub fn peer_address(&self) -> [u8; 6] { self.peer }
    pub fn disconnect(&self) { let _ = BLEDevice::take().get_server().disconnect(self.handle); }
}

enum Ev {
    Connected(u16, [u8; 6]),
    Write(u16, bool),
    Cmd(u16, Vec<u8>),
    Disconnected(u16),
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Run [`announce`] in its own std thread; the caller keeps running.
pub fn announce_background(project: &'static str) -> Result<(), BleError> {
    std::thread::Builder::new()
        .name("ble_cmd".into())
        .stack_size(12 * 1024)
        .spawn(|| crate::commands::worker(|| std::thread::sleep(Duration::from_millis(50))))
        .map_err(|_| BleError::Init)?;
    std::thread::Builder::new()
        .name("ble_announce".into())
        .stack_size(16 * 1024)
        .spawn(move || super::run_background(project, || std::thread::sleep(Duration::from_secs(1))))
        .map(|_| ())
        .map_err(|_| BleError::Init)
}

pub fn announce(project: &str) -> Result<Inner, BleError> {
    run(project, false)
}

/// Background mode: never returns `Ok`; after the manager disconnects, advertising resumes.
pub fn serve(project: &str) -> BleError {
    match run(project, true) { Ok(_) => BleError::Stack, Err(e) => e }
}

fn run(project: &str, background: bool) -> Result<Inner, BleError> {
    if STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err(BleError::AlreadyStarted);
    }
    let svc_uuid = BleUuid::from_uuid128_string(SERVICE_UUID).map_err(|_| BleError::Init)?;
    let chr_uuid = BleUuid::from_uuid128_string(AUTH_CHAR_UUID).map_err(|_| BleError::Init)?;
    let log_uuid = BleUuid::from_uuid128_string(LOG_CHAR_UUID).map_err(|_| BleError::Init)?;
    let mac = crate::backend::mac();
    let dn = super::device_name(project, mac);
    let name_bytes = dn.as_bytes();
    let name = core::str::from_utf8(name_bytes).map_err(|_| BleError::Init)?;

    let device = BLEDevice::take();
    device
        .security()
        .set_auth(AuthReq::Bond | AuthReq::Sc)
        .set_io_cap(SecurityIOCap::NoInputNoOutput)
        .set_security_init_key(PairKeyDist::ENC | PairKeyDist::ID)
        .set_security_resp_key(PairKeyDist::ENC | PairKeyDist::ID);
    BLEDevice::set_device_name(name).map_err(|_| BleError::Init)?;
    let server = device.get_server();
    let (tx, rx) = mpsc::channel::<Ev>();
    let t = tx.clone();
    server.on_connect(move |_, desc| { let _ = t.send(Ev::Connected(desc.conn_handle(), desc.address().as_le_bytes())); });
    let t = tx.clone();
    server.on_disconnect(move |desc, _| { let _ = t.send(Ev::Disconnected(desc.conn_handle())); });
    server.advertise_on_disconnect(true);

    let service = server.create_service(svc_uuid);
    let chr = service.lock().create_characteristic(chr_uuid, NimbleProperties::WRITE);
    let cmd_uuid = BleUuid::from_uuid128_string(super::CMD_CHAR_UUID).map_err(|_| BleError::Init)?;
    let t = tx.clone();
    service.lock().create_characteristic(cmd_uuid, NimbleProperties::WRITE).lock().on_write(move |args| {
        let _ = t.send(Ev::Cmd(args.desc().conn_handle(), args.recv_data().to_vec()));
    });
    let log_chr = service.lock().create_characteristic(log_uuid, NimbleProperties::NOTIFY);
    let buf_uuid = BleUuid::from_uuid128_string(super::BUF_CHAR_UUID).map_err(|_| BleError::Init)?;
    let buf_chr = service.lock().create_characteristic(buf_uuid, NimbleProperties::NOTIFY);
    let status_uuid = BleUuid::from_uuid128_string(super::STATUS_CHAR_UUID).map_err(|_| BleError::Init)?;
    service.lock().create_characteristic(status_uuid, NimbleProperties::READ).lock().on_read(|chr, desc| {
        chr.set_value(&super::encode_status(crate::time::uptime_ms(), desc.get_rssi().ok()));
    });
    let t = tx;
    chr.lock().on_write(move |args| {
        let _ = t.send(Ev::Write(args.desc().conn_handle(), args.recv_data() == MANAGER_TOKEN));
    });

    let advertising = device.get_advertising();
    let mut adv = [0u8; 31];
    let raw = adv_data(&mut adv, mac);
    {
        let mut a = advertising.lock();
        a.set_raw_data(raw).map_err(|_| BleError::Init)?;
        // Units of 0.625 ms: 20 ms = 32, 30 ms = 48.
        a.min_interval((super::ADV_INTERVAL_MIN_MS * 1000 / 625) as u16)
            .max_interval((super::ADV_INTERVAL_MAX_MS * 1000 / 625) as u16);
        let mut scan = [0u8; 31];
        scan[0] = name_bytes.len() as u8 + 1;
        scan[1] = 0x09; // Complete Local Name
        scan[2..2 + name_bytes.len()].copy_from_slice(name_bytes);
        a.set_raw_scan_response_data(&scan[..2 + name_bytes.len()]).map_err(|_| BleError::Init)?;
        a.start().map_err(|_| BleError::Init)?;
    }
    crate::blelog::set_advertising(true);
    log::info!("ble: advertising as {name}");

    // Pending (connected, not yet authenticated) centrals. `None` deadline = bonded, wait for token.
    let mut pending: Vec<(u16, [u8; 6], Option<Instant>)> = Vec::new();
    // Background mode: the authenticated manager connection receiving log notifications.
    let mut manager: Option<u16> = None;
    let mut chunk = [0u8; LOG_CHUNK];
    let mut rssi_at = Instant::now();
    loop {
        if let Some(h) = manager && rssi_at.elapsed() >= Duration::from_secs(2) {
            rssi_at = Instant::now();
            let r = server.connections().find(|c| c.conn_handle() == h).and_then(|c| c.get_rssi().ok());
            crate::blelog::set_rssi(r);
        }
        if let Some(h) = manager {
            loop {
                let n = crate::blelog::pop(&mut chunk);
                if n == 0 || log_chr.lock().notify_with(&chunk[..n], h).is_err() { break; }
            }
            loop {
                let n = crate::status::frame(&mut chunk);
                if n == 0 || buf_chr.lock().notify_with(&chunk[..n], h).is_err() { break; }
                crate::status::advance();
            }
        }
        let now = Instant::now();
        let mut drop_h = Vec::new();
        for (h, _, deadline) in pending.iter_mut() {
            let Some(d) = *deadline else { continue };
            if d > now {
                continue;
            }
            if conn_held(server, *h) {
                log::info!("ble: conn {h} bonded, waiting for manager token");
                *deadline = None;
            } else {
                drop_h.push(*h);
            }
        }
        for h in drop_h {
            log::info!("ble: conn {h} did not authenticate in time, disconnecting");
            pending.retain(|p| p.0 != h);
            let _ = server.disconnect(h);
        }
        let wait = pending
            .iter()
            .filter_map(|p| p.2.map(|d| d.saturating_duration_since(now)))
            .min()
            .unwrap_or(Duration::from_secs(3600));
        let wait = if manager.is_some() { wait.min(Duration::from_millis(50)) } else { wait };
        match rx.recv_timeout(wait) {
            Ok(Ev::Connected(h, peer)) => {
                log::info!("ble: central connected (conn {h}), waiting for manager token");
                pending.push((h, peer, Some(Instant::now() + Duration::from_millis(AUTH_TIMEOUT_MS))));
                // Keep advertising for other centrals while this one authenticates.
                if manager.is_none() { let _ = advertising.lock().start(); } else { let _ = server.disconnect(h); }
            }
            Ok(Ev::Write(h, true)) => {
                if let Some(&(_, peer, _)) = pending.iter().find(|p| p.0 == h) {
                    log::info!("ble: manager authenticated (conn {h})");
                    crate::blelog::set_advertising(false);
                    let _ = advertising.lock().stop();
                    for &(o, _, _) in pending.iter().filter(|p| p.0 != h) { let _ = server.disconnect(o); }
                    if !background {
                        server.advertise_on_disconnect(false);
                        return Ok(Inner { handle: h, peer });
                    }
                    pending.clear();
                    manager = Some(h);
                    // NimBLE gives the address little-endian; store most significant byte first.
                    let mut p = peer;
                    p.reverse();
                    crate::blelog::set_peer(p);
                    crate::blelog::set_connected(true);
                }
            }
            Ok(Ev::Write(h, false)) => {
                if pending.iter().any(|p| p.0 == h) {
                    log::info!("ble: wrong token from conn {h}, disconnecting");
                    pending.retain(|p| p.0 != h);
                    let _ = server.disconnect(h);
                }
            }
            Ok(Ev::Cmd(h, data)) => {
                if manager == Some(h) { crate::commands::submit(&data); }
            }
            Ok(Ev::Disconnected(h)) => {
                pending.retain(|p| p.0 != h);
                if manager == Some(h) {
                    manager = None;
                    crate::blelog::set_connected(false);
                    log::info!("ble: manager disconnected, advertising again");
                    crate::blelog::set_advertising(true);
                    let _ = advertising.lock().start();
                } else if manager.is_none() {
                    // A pending (unauthenticated) central left: make sure we are advertising.
                    let _ = advertising.lock().start();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(BleError::Stack),
        }
    }
}

fn conn_held(server: &BLEServer, h: u16) -> bool {
    server
        .connections()
        .any(|c| c.conn_handle() == h && (c.bonded() || c.encrypted()))
}

/// Flags + complete 128-bit service UUID + manufacturer data (company id + MAC): 31 bytes.
fn adv_data(buf: &mut [u8; 31], mac: [u8; 6]) -> &[u8] {
    let uuid = super::SERVICE_UUID_U128.to_le_bytes();
    buf[..3].copy_from_slice(&[2, 0x01, 0x06]);
    buf[3] = 17;
    buf[4] = 0x07; // Complete List of 128-bit Service UUIDs
    buf[5..21].copy_from_slice(&uuid);
    buf[21] = 9;
    buf[22] = 0xFF; // Manufacturer Specific Data
    buf[23..31].copy_from_slice(&super::manufacturer_payload(mac));
    &buf[..]
}
