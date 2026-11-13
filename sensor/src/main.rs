//! esp32-sensor — firmware entry point (esp-hal no_std or esp-idf std).
#![cfg_attr(feature = "hal", no_std)]
#![cfg_attr(feature = "hal", no_main)]

use log::info;
// The `#[esp_hal::main]` macro expects `esp_hal` in scope.
#[cfg(feature = "hal")]
use utils::esp_hal;

#[cfg(any(feature = "hal", feature = "idf"))]
use esp32_sensor::PROJECT;

#[cfg(any(feature = "hal", feature = "idf"))]
fn app() -> ! {
    let _board = utils::init();
    info!("esp32-sensor starting ({} / {})", utils::CHIP, utils::FRAMEWORK);
    let chip = utils::chip_info();
    info!("chip: {}", chip);
    let dn = utils::ble_proto::device_name(PROJECT, chip.mac);
    if let Err(e) = esp32_sensor::sensors::set_name(dn.as_str()) {
        log::warn!("sensors: name: {e}");
    }
    // BLE announce (`utils::bluetooth_announce(PROJECT)`) runs in a background thread so the loop
    // below keeps ticking; on esp32s2 (no Bluetooth) it just logs and continues. `utils::init()`
    // mirrors all `log` output to the authenticated manager over BLE.
    if let Err(e) = esp32_sensor::sensors::init_sensors(&esp32_sensor::sensors::PIEZO_PINS) {
        log::warn!("sensors: {e}");
    }
    #[cfg(any(feature = "idf", all(feature = "hal", not(feature = "esp32s2"))))]
    utils::commands::set_handler(esp32_sensor::dispatch);
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
        // Always poll connected piezos; store last ADC values and record hits when all corners fire.
        let hits = esp32_sensor::sensors::poll_sensors(esp32_sensor::sensors::threshold());
        #[cfg(feature = "display")]
        utils::display::set_grid_values(esp32_sensor::sensors::last_values());
        if !hits.is_empty() {
            let t = utils::synced_time().unwrap_or_else(utils::time::uptime_ms);
            esp32_sensor::sensors::push_hits(t, hits);
        }
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

#[cfg(feature = "idf")]
fn main() {
    app()
}

#[cfg(feature = "host")]
fn main() {}
