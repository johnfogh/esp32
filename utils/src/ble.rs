//! BLE "manager" announcement: [`bluetooth_announce`].
//!
//! The device advertises (connectable, undirected) with
//! - **advertising data**: flags, the 128-bit service UUID [`SERVICE_UUID`] and manufacturer-specific
//!   data `[COMPANY_ID (LE, 2 bytes)] ++ MAC (6 bytes, display order)`, where MAC is the factory base
//!   MAC from [`crate::chip_info`] (exactly 31 bytes);
//! - **scan response**: complete local name `<project>-xxyyzz` (project passed to [`bluetooth_announce`],
//!   truncated to [`PROJECT_MAX`] bytes, plus the last three MAC bytes; see [`device_name`] / [`parse_name`]).
//!
//! OS pairing is Just Works (no PIN): the peripheral is bondable. A connecting central must write
//! exactly [`MANAGER_TOKEN`] (`b"manager-device"`) to [`AUTH_CHAR_UUID`] within [`AUTH_TIMEOUT_MS`]
//! unless the link is already bonded/encrypted, in which case the token may arrive later. A wrong
//! value or no write in time on an unbonded link gets it disconnected and advertising resumes. The
//! first central that authenticates is returned as a [`Manager`] and advertising stops.
//!
//! The call is **blocking** on both frameworks and waits indefinitely:
//! - `hal`: drives the async trouble-host stack with `embassy_futures::block_on` on top of the
//!   esp-rtos scheduler (which `bluetooth_announce` starts itself). See the `ble_hal` notes.
//! - `idf`: NimBLE via `esp32-nimble`; the NimBLE host runs in its own FreeRTOS task.
//!
//! Requires the `ble` cargo feature (and, on `idf`, NimBLE enabled in `sdkconfig.defaults`).
//! Without it, and always on esp32s2 (no Bluetooth), it returns `Err(BleError::Unsupported)`.

pub use crate::ble_proto::*;

/// BLE errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleError {
    /// No Bluetooth on this chip (esp32s2) or the `ble` feature is disabled.
    Unsupported,
    /// `bluetooth_announce` was already called (the stack can be brought up only once).
    AlreadyStarted,
    /// Controller / host initialisation failed.
    Init,
    /// The BLE host stack reported an error while advertising or connecting.
    Stack,
}

impl core::fmt::Display for BleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "Bluetooth not supported (esp32s2 or `ble` feature disabled)",
            Self::AlreadyStarted => "BLE already started",
            Self::Init => "BLE initialisation failed",
            Self::Stack => "BLE host stack error",
        })
    }
}

impl core::error::Error for BleError {}

#[cfg(all(feature = "ble", feature = "hal", not(feature = "esp32s2")))]
#[path = "ble_hal.rs"]
mod imp;
#[cfg(all(feature = "ble", feature = "hal", not(feature = "esp32s2")))]
pub(crate) use imp::rtos_running;
#[cfg(all(feature = "ble", feature = "hal", not(feature = "esp32s2")))]
pub(crate) use imp::start_rtos;
#[cfg(all(feature = "ble", feature = "idf", not(feature = "esp32s2")))]
#[path = "ble_idf.rs"]
mod imp;

#[cfg(not(all(feature = "ble", not(feature = "esp32s2"))))]
mod imp {
    use super::BleError;
    pub struct Inner(core::convert::Infallible);
    impl Inner {
        pub fn conn_handle(&self) -> u16 { match self.0 {} }
        pub fn peer_address(&self) -> [u8; 6] { match self.0 {} }
        pub fn disconnect(&self) { match self.0 {} }
    }
    pub fn announce(_project: &str) -> Result<Inner, BleError> { Err(BleError::Unsupported) }
    pub fn announce_background(_project: &'static str) -> Result<(), BleError> { Err(BleError::Unsupported) }
}

/// An authenticated manager connection, returned by [`bluetooth_announce`].
pub struct Manager {
    inner: imp::Inner,
}

impl Manager {
    /// HCI connection handle of the link.
    pub fn conn_handle(&self) -> u16 { self.inner.conn_handle() }
    /// Peer (manager) Bluetooth device address, as reported by the stack (`hal`: little-endian
    /// HCI order; `idf`: NimBLE little-endian order).
    pub fn peer_address(&self) -> [u8; 6] { self.inner.peer_address() }
    /// Request disconnection of the manager.
    pub fn disconnect(&self) { self.inner.disconnect() }
    /// The underlying trouble-host connection (`hal` only).
    #[cfg(all(feature = "ble", feature = "hal", not(feature = "esp32s2")))]
    pub fn connection(&self) -> &trouble_host::prelude::Connection<'static, trouble_host::prelude::DefaultPacketPool> {
        self.inner.connection()
    }
}

/// Advertise over BLE and block until a manager authenticates (see the [module docs](self)).
///
/// `project` is advertised in the scan-response name `<project>-xxyyzz` (e.g. `"laser"` →
/// `laser-a1b2c3`); use the firmware's `PROJECT` constant. Call at most once per boot. Returns `Err(BleError::Unsupported)` on esp32s2 or without the `ble`
/// feature, `Err(BleError::AlreadyStarted)` on a second call.
pub fn bluetooth_announce(project: &str) -> Result<Manager, BleError> {
    imp::announce(project).map(|inner| Manager { inner })
}

/// Like [`bluetooth_announce`], but **non-blocking**: runs the announcement in a background thread
/// (`hal`: esp-rtos thread, started here; `idf`: std thread) so the caller's main loop keeps running.
/// That thread keeps serving the authenticated manager: buffered [`crate::blelog`] lines (all `log`
/// output) are sent as notifications on [`LOG_CHAR_UUID`], and when the manager disconnects the
/// device advertises again (so the manager can reconnect). Errors that are known up front
/// (`Unsupported` on esp32s2 / without `ble`, `AlreadyStarted`) are returned; later stack errors
/// are logged from the background thread.
pub fn bluetooth_announce_background(project: &'static str) -> Result<(), BleError> {
    imp::announce_background(project)
}

/// Background thread body: serve announce + manager log output forever (re-advertising after the
/// manager disconnects); on a fatal stack error log it and park.
#[cfg(all(feature = "ble", feature = "idf", not(feature = "esp32s2")))]
fn run_background(project: &'static str, mut sleep: impl FnMut()) -> ! {
    let e = imp::serve(project);
    log::warn!("ble: announce stopped: {e}");
    loop { sleep(); }
}
