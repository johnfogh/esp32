//! esp32-laser — firmware entry point (esp-hal no_std or esp-idf std).
#![cfg_attr(feature = "hal", no_std)]
#![cfg_attr(feature = "hal", no_main)]

mod lasers;

use log::info;
// The `#[esp_hal::main]` macro expects `esp_hal` in scope.
#[cfg(feature = "hal")]
use utils::esp_hal;

/// Project name advertised over BLE: `utils::bluetooth_announce(PROJECT)` → `laser-xxyyzz`.
const PROJECT: &str = "laser";

fn app() -> ! {
    let _board = utils::init();
    info!("esp32-laser starting ({} / {})", utils::CHIP, utils::FRAMEWORK);
    info!("chip: {}", utils::chip_info());
    // BLE announce (`utils::bluetooth_announce(PROJECT)`) runs in a background thread so the loop
    // below keeps ticking; on esp32s2 (no Bluetooth) it just logs and continues. `utils::init()`
    // mirrors all `log` output to the authenticated manager over BLE.
    match utils::bluetooth_announce_background(PROJECT) {
        Ok(()) => info!("ble: announcing as {PROJECT}-xxyyzz"),
        Err(e) => log::warn!("ble: not announcing: {e}"),
    }
    // Status screen (Waveshare ESP32-S3-Touch-LCD-2; on by default for esp32s3 builds).
    #[cfg(feature = "display")]
    if let Err(e) = utils::display::start(PROJECT) {
        log::warn!("display: {e}");
    }
    loop {
        // uptime_ms, time, battery, temperature, accel (whatever this build can read).
        utils::status::add_standard_readings();
        utils::send_buffer();
        utils::time::delay_ms(LOOP_MS);
    }
}

/// Main-loop sleep per pass (~20 passes/s → ~20 samples per 1 s snapshot).
const LOOP_MS: u32 = 50;

#[cfg(feature = "hal")]
#[esp_hal::main]

fn main() -> ! {
    app()
}

#[cfg(not(feature = "hal"))]
fn main() {
    app()
}
