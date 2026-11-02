//! Laser outputs: laser `i` (L1..Ln in the controller's Signals table) is driven by GPIO
//! `LASER_PINS[i]` (active high, push-pull, starts low).
//!
//! Default pins (4 lasers) avoid strapping, flash/PSRAM, USB-JTAG and UART0 pins:
//!
//! | chip     | L1 | L2 | L3 | L4 |
//! |----------|----|----|----|----|
//! | esp32    | 25 | 26 | 27 | 32 |
//! | esp32s2  |  4 |  5 |  6 |  7 |
//! | esp32s3  |  4 | 18 |  6 |  7 |
//! | esp32c3  |  4 |  5 |  6 |  7 |
//! | esp32c6  | 18 | 19 | 20 | 21 |
//! | esp32h2  | 10 | 11 | 12 | 22 |
//!
//! Change the list (any length) to rewire; nothing else needs editing.
#![allow(dead_code)] // not yet used from the main loop

#[cfg(feature = "esp32")]
pub const LASER_PINS: [u8; 4] = [25, 26, 27, 32];
#[cfg(any(feature = "esp32s2", feature = "esp32c3"))]
pub const LASER_PINS: [u8; 4] = [4, 5, 6, 7];
/// esp32s3 (Waveshare ESP32-S3-Touch-LCD-2): GPIO5 is the on-board battery divider, so L2 is on 18.
#[cfg(feature = "esp32s3")]
pub const LASER_PINS: [u8; 4] = [4, 18, 6, 7];
#[cfg(feature = "esp32c6")]
pub const LASER_PINS: [u8; 4] = [18, 19, 20, 21];
#[cfg(feature = "esp32h2")]
pub const LASER_PINS: [u8; 4] = [10, 11, 12, 22];

pub const LASER_COUNT: usize = LASER_PINS.len();

use utils::gpio::Output;

/// Output drivers for all [`LASER_PINS`], all off.
pub struct Lasers {
    pins: [Output; LASER_COUNT],
}

impl Lasers {
    /// Take the laser GPIOs by number.
    ///
    /// # Safety
    /// The pins are stolen by number: nothing else may use the GPIOs in [`LASER_PINS`], and this
    /// must be called at most once.
    pub unsafe fn new() -> Self {
        Self { pins: LASER_PINS.map(|n| Output::new(unsafe { pin(n) })) }
    }

    /// Set laser `i` (0-based) on/off. Out-of-range indices are ignored.
    pub fn set(&mut self, i: usize, on: bool) {
        if let Some(p) = self.pins.get_mut(i) {
            p.set(on);
        }
    }

    /// Apply an on/off pattern (e.g. one Signals row): `pattern[i]` drives laser `i`.
    /// Lasers beyond the pattern's length are switched off; extra entries are ignored.
    pub fn set_pattern(&mut self, pattern: &[bool]) {
        for (i, p) in self.pins.iter_mut().enumerate() {
            p.set(pattern.get(i).copied().unwrap_or(false));
        }
    }
}

#[cfg(feature = "hal")]
unsafe fn pin(n: u8) -> utils::esp_hal::gpio::AnyPin<'static> {
    unsafe { utils::esp_hal::gpio::AnyPin::steal(n) }
}

#[cfg(not(feature = "hal"))]
unsafe fn pin(n: u8) -> utils::esp_idf_hal::gpio::AnyOutputPin<'static> {
    unsafe { utils::esp_idf_hal::gpio::AnyOutputPin::steal(n) }
}
