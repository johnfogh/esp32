//! Build script: ROM symbol fix-up for BLE on the original ESP32 (esp-hal backend).
//!
//! esp-wifi-sys-esp32's `libbtdm_app.a` references three ESP32 ROM callbacks that the published
//! esp-rom-sys 0.1.5 linker scripts do not PROVIDE (fixed on esp-hal main). We emit them as a
//! linker script disguised as a static library (the same trick esp-radio uses), because
//! `rustc-link-lib` propagates to the final binary while `rustc-link-arg` from a lib does not.
//! Addresses from ESP-IDF `components/esp_rom/esp32/ld/esp32.rom.ld`.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let ble = env::var_os("CARGO_FEATURE_BLE").is_some();
    let hal = env::var_os("CARGO_FEATURE_HAL").is_some();
    let target = env::var("TARGET").unwrap_or_default();
    if ble && hal && target == "xtensa-esp32-none-elf" {
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        fs::write(
            out.join("libutils-esp32-rom-bt.a"),
            "PROVIDE ( ld_pscan_evt_start_cbk = 0x4003e924 );\n\
             PROVIDE ( ld_iscan_evt_start_cbk = 0x4003b58c );\n\
             PROVIDE ( ld_page_evt_start_cbk = 0x4003cf40 );\n",
        )
        .expect("write rom linker script");
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rustc-link-lib=utils-esp32-rom-bt");
    }
}
