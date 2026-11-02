//! Library part of esp32-sensor (host-testable logic).
#![cfg_attr(not(any(test, feature = "host")), no_std)]

extern crate alloc;

/// Project name advertised over BLE: `utils::bluetooth_announce(esp32_sensor::PROJECT)` → `sensor-xxyyzz`.
pub const PROJECT: &str = "sensor";

pub mod sensors;

use alloc::{format, string::String};

/// BLE command hook for [`utils::commands::set_handler`]: `set_threshold`, `set_name`, `read_buffer`.
#[cfg(any(test, feature = "host", feature = "idf", all(feature = "hal", not(feature = "esp32s2"))))]
pub fn dispatch(action: &str, params: &[&str]) -> Result<String, String> {
    match action {
        "set_threshold" => {
            let v = params.first().and_then(|s| s.trim().parse::<u16>().ok()).ok_or("need [u16]")?;
            sensors::set_threshold(v);
            Ok(format!("{v}"))
        }
        "set_name" => {
            let n = params.first().ok_or("need [name]")?;
            sensors::set_name(n).map_err(|e| format!("{e}"))?;
            Ok(sensors::name().as_str().into())
        }
        "read_buffer" => {
            let mut s = String::new();
            sensors::write_buffer_json(&mut s);
            Ok(s)
        }
        _ => Err(format!("unknown action {action:?}")),
    }
}
