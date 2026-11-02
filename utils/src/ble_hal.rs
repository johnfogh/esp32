//! esp-hal BLE backend: esp-radio controller + trouble-host (via bt-hci `ExternalController`).
//!
//! Notes:
//! - Starts the esp-rtos scheduler (needed by esp-radio) on `TIMG0` timer 0 + `FROM_CPU_INTR0` and
//!   adds a 96 KiB esp-alloc heap (128 KiB on esp32s3); it **steals** `TIMG0`, `FROM_CPU_INTR0` and `BT`, so the
//!   application must not use them (nor start esp-rtos / define a global allocator itself).
//! - trouble-host is async; we run it with `embassy_futures::block_on` (busy polling; esp-rtos
//!   preempts it for the radio tasks). Its runner is kept in the returned `Inner`: once
//!   `bluetooth_announce` returns, the host is no longer polled, so the controller keeps the link
//!   alive but host-level traffic is not processed until future APIs drive the runner again.
use embassy_futures::select::{Either, select};
use embassy_time::Duration;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::controller::BleConnector;
use static_cell::StaticCell;
use trouble_host::prelude::*;

use super::{AUTH_TIMEOUT_MS, BleError, COMPANY_ID, LOG_CHUNK, MANAGER_TOKEN, SERVICE_UUID_U128, STATUS_LEN};

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

type Ctrl = ExternalController<BleConnector<'static>, 20>;
type Pool = DefaultPacketPool;

#[gatt_server]
struct Server {
    announce: AnnounceService,
}

// Must match super::SERVICE_UUID / super::AUTH_CHAR_UUID.
#[gatt_service(uuid = "c0de5e00-6d67-4e2a-9b1f-3a5d7e9c0b01")]
struct AnnounceService {
    #[characteristic(uuid = "c0de5e01-6d67-4e2a-9b1f-3a5d7e9c0b01", write)]
    auth: [u8; 20],
    /// Board log output (see `blelog`); must match super::LOG_CHAR_UUID.
    #[characteristic(uuid = "c0de5e02-6d67-4e2a-9b1f-3a5d7e9c0b01", notify)]
    log: [u8; LOG_CHUNK],
    /// uptime + RSSI (see super::STATUS_CHAR_UUID), refreshed on every read.
    #[characteristic(uuid = "c0de5e03-6d67-4e2a-9b1f-3a5d7e9c0b01", read)]
    status: [u8; STATUS_LEN],
    /// JSON command chunks from the manager (see super::CMD_CHAR_UUID / crate::commands).
    #[characteristic(uuid = "c0de5e04-6d67-4e2a-9b1f-3a5d7e9c0b01", write)]
    cmd: [u8; LOG_CHUNK],
    /// Status buffer frames (see super::BUF_CHAR_UUID / crate::status).
    #[characteristic(uuid = "c0de5e05-6d67-4e2a-9b1f-3a5d7e9c0b01", notify)]
    buf: [u8; LOG_CHUNK],
}

pub struct Inner {
    conn: Connection<'static, Pool>,
    _runner: Runner<'static, Ctrl, Pool>,
}

impl Inner {
    pub fn conn_handle(&self) -> u16 { self.conn.handle().raw() }
    pub fn peer_address(&self) -> [u8; 6] { self.conn.peer_address().addr.into_inner() }
    pub fn disconnect(&self) { self.conn.disconnect() }
    pub fn connection(&self) -> &Connection<'static, Pool> { &self.conn }
}

/// `StaticCell` instead of `AtomicBool::swap`: the esp32c3 (rv32imc) has no atomic RMW.
static STARTED: StaticCell<()> = StaticCell::new();
static RESOURCES: StaticCell<HostResources<Ctrl, Pool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>> = StaticCell::new();
static STACK: StaticCell<Stack<'static, Ctrl, Pool>> = StaticCell::new();
static SERVER: StaticCell<Server<'static>> = StaticCell::new();
static NAME: StaticCell<super::DeviceName> = StaticCell::new();

static RTOS: StaticCell<()> = StaticCell::new();
static RTOS_UP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();

/// `true` once the esp-rtos scheduler runs (then `delay_ms` sleeps instead of spinning).
pub(crate) fn rtos_running() -> bool { RTOS_UP.load(core::sync::atomic::Ordering::Acquire) }

#[embassy_executor::task]
async fn ble_task(project: &'static str) {
    let e = serve_async(project).await;
    log::warn!("ble: announce stopped: {e}");
}
static BG_PROJECT: StaticCell<&'static str> = StaticCell::new();

/// Heap + esp-rtos scheduler, once (steals `TIMG0` / `FROM_CPU_INTR0`).
pub(crate) fn start_rtos() {
    if RTOS.try_init(()).is_none() {
        return;
    }
    // Budget: BLE controller exchange memory (one 30 KiB alloc, `BLE assert emi.c 164 … 7800` when
    // it fails), task stacks from the heap (ble 16 KiB, ble_cmd 12 KiB, display 12 KiB), SMP pairing,
    // and the host. On esp32s3 (display + imu tasks) 72 KiB was exhausted at BLE init, so it gets 128 KiB, taken
    // from main DRAM (the S3 stack has ~190 KiB). Not the bootloader-"reclaimed" dram2 region: its top
    // (≥ 0x3FCE9710) overlaps memory the ROM (incl. the ROM BLE controller) uses at runtime, and with it
    // the board advertised only sporadically.
    #[cfg(feature = "esp32s3")]
    esp_alloc::heap_allocator!(size: 128 * 1024);
    #[cfg(not(feature = "esp32s3"))]
    esp_alloc::heap_allocator!(size: 96 * 1024);
    // SAFETY: documented in the module docs: utils owns these peripherals once BLE is used.
    let (timg0, intr0) = unsafe {
        use esp_hal::peripherals::{FROM_CPU_INTR0, TIMG0};
        (TIMG0::steal(), FROM_CPU_INTR0::steal())
    };
    esp_rtos::start(TimerGroup::new(timg0).timer0, intr0);
    RTOS_UP.store(true, core::sync::atomic::Ordering::Release);
}

/// Run [`announce`] in its own esp-rtos thread; the caller (main thread) keeps running.
pub fn announce_background(project: &'static str) -> Result<(), BleError> {
    let project: &'static &'static str = BG_PROJECT.try_init(project).ok_or(BleError::AlreadyStarted)?;
    start_rtos();
    // The BLE future runs on an esp-rtos embassy executor in its own thread: the thread sleeps
    // until a waker fires (radio event, timer) instead of busy-polling, so the main thread runs.
    extern "C" fn task(param: *mut core::ffi::c_void) {
        // SAFETY: `param` points at the 'static `BG_PROJECT` value.
        let project: &'static str = unsafe { *(param as *const &'static str) };
        let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
        executor.run(|spawner| spawner.spawn(ble_task(project).expect("ble task")))
    }
    extern "C" fn cmd_task(_: *mut core::ffi::c_void) {
        crate::commands::worker(|| esp_radio_rtos_driver::usleep(50_000))
    }
    // SAFETY: the task fns never return and their parameters are 'static.
    unsafe {
        esp_radio_rtos_driver::task_create("ble_cmd", cmd_task, core::ptr::null_mut(), 0, None, 12 * 1024);
        esp_radio_rtos_driver::task_create(
            "ble_announce",
            task,
            project as *const &'static str as *mut core::ffi::c_void,
            0, // same priority as the main thread: time-sliced, never starves it
            None,
            16 * 1024,
        );
    }
    Ok(())
}

struct Setup {
    runner: Runner<'static, Ctrl, Pool>,
    peripheral: Peripheral<'static, Ctrl, Pool>,
    server: &'static Server<'static>,
    mac: [u8; 6],
    name: &'static super::DeviceName,
    stack: &'static Stack<'static, Ctrl, Pool>,
}

fn setup() -> Result<(), BleError> {
    if STARTED.try_init(()).is_none() {
        return Err(BleError::AlreadyStarted);
    }
    Ok(())
}

fn init_stack(project: &str) -> Result<Setup, BleError> {
    setup()?;
    start_rtos();
    log::info!("ble: heap free {} bytes before controller init", esp_alloc::HEAP.free());
    // SAFETY: documented in the module docs: utils owns these peripherals once BLE is used.
    let bt = unsafe { esp_hal::peripherals::BT::steal() };
    let connector = BleConnector::new(bt, Default::default()).map_err(|_| BleError::Init)?;
    let controller: Ctrl = ExternalController::new(connector);

    let mac = crate::backend::mac();
    let name: &'static super::DeviceName = NAME.init(super::device_name(project, mac));
    let name_str: &'static str = core::str::from_utf8(name.as_bytes()).map_err(|_| BleError::Init)?;

    let resources = RESOURCES.init(HostResources::new());
    let stack: &'static Stack<'static, Ctrl, Pool> = STACK.init(trouble_host::new(controller, resources).build());
    let runner = stack.runner();
    let peripheral = stack.peripheral();
    let server: &'static Server<'static> = SERVER.init(
        Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
            name: name_str,
            appearance: &appearance::UNKNOWN,
        }))
        .map_err(|_| BleError::Init)?,
    );
    Ok(Setup { runner, peripheral, server, mac, name, stack })
}

pub fn announce(project: &str) -> Result<Inner, BleError> {
    let Setup { mut runner, mut peripheral, server, mac, name, .. } = init_stack(project)?;
    let conn = embassy_futures::block_on(async {
        match select(runner.run(), announce_loop(&mut peripheral, server, mac, name.as_bytes())).await {
            Either::First(_) => Err(BleError::Stack),
            Either::Second(r) => r.map(|g| g.raw().clone()),
        }
    })?;
    Ok(Inner { conn, _runner: runner })
}

/// Background mode: announce, serve the manager (log notifications, status reads), re-advertise
/// after it disconnects; returns only on a fatal error.
async fn serve_async(project: &str) -> BleError {
    let Setup { mut runner, mut peripheral, server, mac, name, stack } = match init_stack(project) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let serve = async {
        loop {
            let gatt = announce_loop(&mut peripheral, server, mac, name.as_bytes()).await?;
            // trouble/bt-hci addresses are little-endian; store most significant byte first.
            let mut peer = gatt.raw().peer_address().addr.into_inner();
            peer.reverse();
            crate::blelog::set_peer(peer);
            crate::blelog::set_connected(true);
            pump(stack, server, &gatt).await;
            crate::blelog::set_connected(false);
            log::info!("ble: manager disconnected, advertising again");
        }
    };
    match select(runner.run(), serve).await {
        Either::First(_) => BleError::Stack,
        Either::Second(r) => match r { Ok(()) => BleError::Stack, Err(e) => e },
    }
}

/// Serve the manager connection: answer GATT requests, flush `blelog` as notifications every 50 ms.
async fn pump(
    stack: &'static Stack<'static, Ctrl, Pool>,
    server: &'static Server<'static>,
    gatt: &GattConnection<'static, 'static, Pool>,
) {
    let mut chunk = [0u8; LOG_CHUNK];
    let status = server.announce.status.handle;
    let cmd = server.announce.cmd.handle;
    let mut ticks = 0u32;
    loop {
        match select(gatt.next(), embassy_time::Timer::after_millis(50)).await {
            Either::First(GattConnectionEvent::Disconnected { .. }) => return,
            Either::First(GattConnectionEvent::Gatt { event }) => {
                if let GattEvent::Read(r) = &event && r.handle() == status {
                    let rssi = gatt.raw().rssi(stack).await.ok();
                    let v = super::encode_status(crate::time::uptime_ms(), rssi);
                    let _ = server.announce.status.set(server, &v);
                }
                if let GattEvent::Write(w) = &event && w.handle() == cmd {
                    w.with_data(|_, d| crate::commands::submit(d));
                    if let GattEvent::Write(w) = event && let Ok(r) = w.accept_unprocessed() { r.send().await; }
                    continue;
                }
                if let Ok(r) = event.accept() { r.send().await; }
            }
            Either::First(_) => {}
            Either::Second(()) => {
                // Link RSSI for the status screen every ~2 s (40 × 50 ms).
                ticks = ticks.wrapping_add(1);
                if ticks % 40 == 1 { crate::blelog::set_rssi(gatt.raw().rssi(stack).await.ok()); }
                loop {
                let n = crate::blelog::pop(&mut chunk);
                if n == 0 || server.announce.log.notify_raw(gatt, &chunk[..n], false).await.is_err() { break; }
                }
            }
        }
        loop {
            let n = crate::status::frame(&mut chunk);
            if n == 0 || server.announce.buf.notify_raw(gatt, &chunk[..n], false).await.is_err() { break; }
            crate::status::advance();
        }
    }
}

async fn announce_loop(
    peripheral: &mut Peripheral<'static, Ctrl, Pool>,
    server: &'static Server<'static>,
    mac: [u8; 6],
    name: &[u8],
) -> Result<GattConnection<'static, 'static, Pool>, BleError> {
    let uuid = SERVICE_UUID_U128.to_le_bytes();
    let mut adv = [0u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[uuid]),
            AdStructure::ManufacturerSpecificData { company_identifier: COMPANY_ID, payload: &mac },
        ],
        &mut adv[..],
    )
    .map_err(|_| BleError::Init)?;
    let mut scan = [0u8; 31];
    let scan_len = AdStructure::encode_slice(&[AdStructure::CompleteLocalName(name)], &mut scan[..])
        .map_err(|_| BleError::Init)?;

    loop {
        let advertiser = peripheral
            .advertise(
                &AdvertisementParameters {
                    interval_min: Duration::from_millis(super::ADV_INTERVAL_MIN_MS),
                    interval_max: Duration::from_millis(super::ADV_INTERVAL_MAX_MS),
                    ..Default::default()
                },
                Advertisement::ConnectableScannableUndirected { adv_data: &adv[..adv_len], scan_data: &scan[..scan_len] },
            )
            .await
            .map_err(|_| BleError::Stack)?;
        crate::blelog::set_advertising(true);
        log::info!("ble: advertising");
        let conn = advertiser.accept().await.map_err(|_| BleError::Stack)?;
        let _ = conn.set_bondable(true);
        let gatt = match conn.clone().with_attribute_server(server) {
            Ok(g) => g,
            Err(_) => { conn.disconnect(); continue; }
        };
        log::info!("ble: central connected, waiting for manager token");
        if wait_manager(server, &gatt).await {
            crate::blelog::set_advertising(false);
            log::info!("ble: manager authenticated");
            return Ok(gatt);
        }
        log::info!("ble: central not authenticated, disconnecting");
        gatt.raw().disconnect();
    }
}

/// `true` once `MANAGER_TOKEN` is written; `false` on a wrong value or disconnect.
/// Unbonded centrals have [`AUTH_TIMEOUT_MS`]; bonded/encrypted links wait for the token.
async fn wait_manager(server: &Server<'_>, gatt: &GattConnection<'_, '_, Pool>) -> bool {
    let auth = server.announce.auth.handle;
    let conn = gatt.raw();
    let mut timeout = Some(embassy_time::Timer::after_millis(AUTH_TIMEOUT_MS));
    loop {
        let event = if let Some(t) = timeout.as_mut() {
            match select(gatt.next(), t).await {
                Either::First(e) => Some(e),
                Either::Second(()) => None,
            }
        } else {
            Some(gatt.next().await)
        };
        match event {
            None => {
                let held = conn.security_level().map(|l| l.encrypted()).unwrap_or(false) || conn.is_bonded_peer();
                if held {
                    log::info!("ble: bonded, waiting for manager token");
                    timeout = None;
                    continue;
                }
                return false;
            }
            Some(GattConnectionEvent::Disconnected { .. }) => return false,
            Some(GattConnectionEvent::PairingComplete { .. } | GattConnectionEvent::Encrypted { .. }) => {
                log::info!("ble: link encrypted, waiting for manager token");
                timeout = None;
            }
            Some(GattConnectionEvent::Gatt { event }) => {
                let verdict = match &event {
                    GattEvent::Write(w) if w.handle() == auth => Some(w.with_data(|off, d| off == 0 && d == MANAGER_TOKEN)),
                    _ => None,
                };
                let reply = match (&verdict, event) {
                    (Some(_), GattEvent::Write(w)) => w.accept_unprocessed(),
                    (_, e) => e.accept(),
                };
                if let Ok(r) = reply { r.send().await; }
                if let Some(ok) = verdict { return ok; }
            }
            _ => {}
        }
    }
}
