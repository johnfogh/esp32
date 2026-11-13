//! `esp32-utils` — one small API over both ESP32 Rust frameworks.
//!
//! Select exactly one chip feature (`esp32`, `esp32s2`, `esp32s3`, `esp32c3`,
//! `esp32c6`, `esp32h2`) and exactly one framework feature (`hal` = no_std
//! esp-hal, `idf` = std esp-idf-svc/esp-idf-hal).
#![cfg_attr(feature = "hal", no_std)]

// Heap (esp-alloc) exists on hal only with `ble` (not esp32s2).
#[cfg(all(feature = "hal", feature = "ble", not(feature = "esp32s2")))]
extern crate alloc;

// ---------------------------------------------------------------- guards
#[cfg(not(any(
    feature = "host",
    feature = "esp32", feature = "esp32s2", feature = "esp32s3",
    feature = "esp32c3", feature = "esp32c6", feature = "esp32h2"
)))]
compile_error!("esp32-utils: enable exactly one chip feature: esp32, esp32s2, esp32s3, esp32c3, esp32c6 or esp32h2");

#[cfg(any(
    all(feature = "esp32", any(feature = "esp32s2", feature = "esp32s3", feature = "esp32c3", feature = "esp32c6", feature = "esp32h2")),
    all(feature = "esp32s2", any(feature = "esp32s3", feature = "esp32c3", feature = "esp32c6", feature = "esp32h2")),
    all(feature = "esp32s3", any(feature = "esp32c3", feature = "esp32c6", feature = "esp32h2")),
    all(feature = "esp32c3", any(feature = "esp32c6", feature = "esp32h2")),
    all(feature = "esp32c6", feature = "esp32h2"),
))]
compile_error!("esp32-utils: chip features are mutually exclusive; enable only one");

#[cfg(not(any(feature = "hal", feature = "idf", feature = "host")))]
compile_error!("esp32-utils: enable exactly one framework feature: `hal` (no_std), `idf` (std) or `host` (desktop tooling)");

#[cfg(all(feature = "host", any(feature = "hal", feature = "idf")))]
compile_error!("esp32-utils: `host` is mutually exclusive with `hal` and `idf`");

#[cfg(all(feature = "host", any(
    feature = "esp32", feature = "esp32s2", feature = "esp32s3",
    feature = "esp32c3", feature = "esp32c6", feature = "esp32h2"
)))]
compile_error!("esp32-utils: `host` does not take a chip feature (chips are runtime values: utils::host::Chip)");

/// Desktop-side helpers (chip table, build/flash command generation, serial ports).
#[cfg(feature = "host")]
pub mod host;

#[cfg(all(feature = "hal", feature = "idf"))]
compile_error!("esp32-utils: `hal` and `idf` are mutually exclusive");

// ---------------------------------------------------------------- backends
#[cfg(all(feature = "hal", not(feature = "idf")))]
#[path = "hal.rs"]
mod backend;
#[cfg(all(feature = "idf", not(feature = "hal")))]
#[path = "idf.rs"]
mod backend;

#[cfg(all(feature = "hal", not(feature = "idf")))]
pub use esp_hal;
#[cfg(all(feature = "idf", not(feature = "hal")))]
pub use {esp_idf_svc, esp_idf_svc::hal as esp_idf_hal};

#[cfg(any(feature = "hal", feature = "idf"))]
pub use backend::{Board, Peripherals, init};

pub use log;

/// Name of the selected chip.
#[cfg(not(feature = "host"))]
pub const CHIP: &str = if cfg!(feature = "esp32") { "esp32" }
    else if cfg!(feature = "esp32s2") { "esp32s2" }
    else if cfg!(feature = "esp32s3") { "esp32s3" }
    else if cfg!(feature = "esp32c3") { "esp32c3" }
    else if cfg!(feature = "esp32c6") { "esp32c6" }
    else { "esp32h2" };

/// Name of the selected framework.
#[cfg(not(feature = "host"))]
pub const FRAMEWORK: &str = if cfg!(feature = "idf") { "esp-idf" } else { "esp-hal" };

/// Basic facts about the running chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipInfo {
    pub model: &'static str,
    pub cores: u8,
    /// Detected flash size in bytes (`None` if unknown).
    pub flash_size: Option<u32>,
    /// Factory base MAC address.
    pub mac: [u8; 6],
}

impl core::fmt::Display for ChipInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let m = self.mac;
        write!(f, "{} ({} core{}), flash ", self.model, self.cores, if self.cores == 1 { "" } else { "s" })?;
        match self.flash_size {
            Some(b) => write!(f, "{} MiB", b / (1024 * 1024))?,
            None => write!(f, "unknown")?,
        }
        write!(f, ", MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
    }
}

/// Number of CPU cores of the selected chip.
#[cfg(not(feature = "host"))]
pub const CORES: u8 = if cfg!(any(feature = "esp32", feature = "esp32s3")) { 2 } else { 1 };

/// Return model, cores, flash size and MAC of the running chip.
#[cfg(any(feature = "hal", feature = "idf"))]
pub fn chip_info() -> ChipInfo {
    ChipInfo { model: CHIP, cores: CORES, flash_size: backend::flash_size(), mac: backend::mac() }
}

/// Blocking delays and a monotonic uptime clock.
#[cfg(any(feature = "hal", feature = "idf"))]
pub mod time {
    pub use crate::backend::delay_ms;
    /// Milliseconds since boot.
    pub fn uptime_ms() -> u64 { crate::backend::uptime_us() / 1000 }
}

/// GPIO output helpers.
#[cfg(any(feature = "hal", feature = "idf"))]
pub mod gpio {
    pub use crate::backend::Output;
}

/// BLE announce protocol constants and name/advert parsers, shared with desktop tooling (all features).
pub mod ble_proto;

/// BLE manager announcement ([`bluetooth_announce`]); needs the `ble` feature, unsupported on esp32s2.
#[cfg(any(feature = "hal", feature = "idf"))]
pub mod ble;
#[cfg(any(feature = "hal", feature = "idf"))]
pub use ble::{BleError, Manager, bluetooth_announce, bluetooth_announce_background};
/// Onboard QMI8658 IMU (Waveshare ESP32-S3-Touch-LCD-2), feature `imu`. Needs a heap: on `hal`
/// only with `ble` (not on esp32s2, which has no BLE/heap).
#[cfg(any(feature = "idf", all(feature = "hal", feature = "ble", not(feature = "esp32s2"))))]
pub mod imu;
#[cfg(any(feature = "idf", all(feature = "hal", feature = "ble", not(feature = "esp32s2"))))]
pub use imu::get_accelerometers;
/// JSON commands from the BLE manager (executed by the announce worker).
#[cfg(any(feature = "idf", all(feature = "hal", feature = "ble", not(feature = "esp32s2"))))]
pub mod commands;
/// Timestamped status buffer shipped to the BLE manager ([`buffer_add_sample`], [`send_buffer`]).
#[cfg(any(feature = "hal", feature = "idf"))]
pub mod status;
/// Standard readings for the status buffer (`status::add_standard_readings`).
#[cfg(any(feature = "hal", feature = "idf"))]
pub mod readings;
#[cfg(any(feature = "hal", feature = "idf"))]
pub use status::{buffer_add, buffer_add_sample, send_buffer, synced_time};
/// Status screen (Waveshare ESP32-S3-Touch-LCD-2), feature `display`.
/// Also compiled under `host` so [`display::set_grid_values`] and unit tests are available.
#[cfg(any(feature = "hal", feature = "idf", feature = "host"))]
pub mod display;
/// Board output over BLE ([`ble_log`], `log` mirroring).
#[cfg(any(feature = "hal", feature = "idf"))]
pub mod blelog;
#[cfg(any(feature = "hal", feature = "idf"))]
pub use blelog::ble_log;

/// Raw one-shot ADC1 readings by GPIO number (see module docs for pin maps).
pub mod adc;
