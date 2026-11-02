//! One-shot raw ADC readings by GPIO number.
//!
//! Only **ADC1** pins are supported on every chip: ADC2 is shared with the
//! Wi-Fi/BT radio on ESP32/S2/S3 (reads fail or block while the radio is on),
//! and ADC2 on the ESP32-C3 is documented by Espressif as unreliable. Pins on
//! ADC2 are therefore rejected by [`is_adc_pin`].
//!
//! Readings are raw (uncalibrated) 12-bit codes with ~11/12 dB attenuation
//! (full range roughly 0..3.1 V, chip dependent). No calibration is applied.
//!
//! On `hal` and `idf`, `read` drives ADC1 directly, so do not also own ADC1
//! through the framework's own ADC driver in the same program.
//! On `host`, `read` returns mock values set with [`mock::set`].

/// ADC1 pin map: (GPIO number, ADC1 channel).
pub mod pins {
    pub const ESP32: &[(u8, u8)] = &[(36, 0), (37, 1), (38, 2), (39, 3), (32, 4), (33, 5), (34, 6), (35, 7)];
    pub const ESP32S2: &[(u8, u8)] = &[(1, 0), (2, 1), (3, 2), (4, 3), (5, 4), (6, 5), (7, 6), (8, 7), (9, 8), (10, 9)];
    #[cfg(not(feature = "display"))]
    pub const ESP32S3: &[(u8, u8)] = ESP32S2;
    /// With `display` (Waveshare ESP32-S3-Touch-LCD-2): GPIO1 is the LCD backlight and GPIO5 the
    /// on-board battery divider, so both are excluded.
    #[cfg(feature = "display")]
    pub const ESP32S3: &[(u8, u8)] = &[(2, 1), (3, 2), (4, 3), (6, 5), (7, 6), (8, 7), (9, 8), (10, 9)];
    pub const ESP32C3: &[(u8, u8)] = &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)];
    pub const ESP32C6: &[(u8, u8)] = &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6)];
    pub const ESP32H2: &[(u8, u8)] = &[(1, 0), (2, 1), (3, 2), (4, 3), (5, 4)];

    /// ADC1 channel of `pin` in `map`, if any.
    pub const fn channel_in(map: &[(u8, u8)], pin: u8) -> Option<u8> {
        let mut i = 0;
        while i < map.len() {
            if map[i].0 == pin { return Some(map[i].1); }
            i += 1;
        }
        None
    }
}

/// ADC1 pin map of the selected chip.
#[cfg(not(feature = "host"))]
pub const PINS: &[(u8, u8)] = if cfg!(feature = "esp32") { pins::ESP32 }
    else if cfg!(feature = "esp32s2") { pins::ESP32S2 }
    else if cfg!(feature = "esp32s3") { pins::ESP32S3 }
    else if cfg!(feature = "esp32c3") { pins::ESP32C3 }
    else if cfg!(feature = "esp32c6") { pins::ESP32C6 }
    else { pins::ESP32H2 };

/// ADC1 pin map for a runtime chip (host tooling).
#[cfg(feature = "host")]
pub const fn pins_for(chip: crate::host::Chip) -> &'static [(u8, u8)] {
    use crate::host::Chip;
    match chip {
        Chip::Esp32 => pins::ESP32, Chip::Esp32s2 => pins::ESP32S2, Chip::Esp32s3 => pins::ESP32S3,
        Chip::Esp32c3 => pins::ESP32C3, Chip::Esp32c6 => pins::ESP32C6, Chip::Esp32h2 => pins::ESP32H2,
    }
}

/// ADC1 channel for `pin` on the selected chip (host: the mock chip).
pub fn channel(pin: u8) -> Option<u8> {
    #[cfg(not(feature = "host"))]
    { pins::channel_in(PINS, pin) }
    #[cfg(feature = "host")]
    { pins::channel_in(pins_for(mock::chip()), pin) }
}

/// Whether `pin` is a supported (ADC1) analog pin on the selected chip.
pub fn is_adc_pin(pin: u8) -> bool { channel(pin).is_some() }

/// Raw reading of `pin`, or `None` if it is not an ADC1 pin (or the driver failed).
pub fn try_read(pin: u8) -> Option<u16> {
    let ch = channel(pin)?;
    #[cfg(not(feature = "host"))]
    { crate::backend::adc_read(pin, ch) }
    #[cfg(feature = "host")]
    { let _ = ch; Some(mock::get(pin)) }
}

/// Raw reading of `pin`. Panics if `pin` is not an ADC pin (check with [`is_adc_pin`]).
pub fn read(pin: u8) -> u16 {
    match try_read(pin) {
        Some(v) => v,
        None => panic!("adc::read: GPIO{} is not a usable ADC1 pin on {}", pin, chip_name()),
    }
}

fn chip_name() -> &'static str {
    #[cfg(not(feature = "host"))]
    { crate::CHIP }
    #[cfg(feature = "host")]
    { mock::chip().name() }
}

/// Host mock: per-thread chip selection and pin values (default chip esp32c3, value 0).
#[cfg(feature = "host")]
pub mod mock {
    use crate::host::Chip;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static STATE: RefCell<(Chip, HashMap<u8, u16>)> = RefCell::new((Chip::Esp32c3, HashMap::new()));
    }
    /// Chip whose pin map `is_adc_pin`/`read` use.
    pub fn set_chip(chip: Chip) { STATE.with(|s| s.borrow_mut().0 = chip) }
    pub fn chip() -> Chip { STATE.with(|s| s.borrow().0) }
    /// Value returned by `read(pin)`.
    pub fn set(pin: u8, value: u16) { STATE.with(|s| { s.borrow_mut().1.insert(pin, value); }) }
    pub fn get(pin: u8) -> u16 { STATE.with(|s| s.borrow().1.get(&pin).copied().unwrap_or(0)) }
    /// Clear values and reset chip to esp32c3.
    pub fn reset() { STATE.with(|s| *s.borrow_mut() = (Chip::Esp32c3, HashMap::new())) }
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use super::*;
    use crate::host::Chip;

    #[test]
    fn maps() {
        mock::reset();
        for (chip, ok, bad) in [
            (Chip::Esp32, 36, 4), (Chip::Esp32s2, 1, 11), (Chip::Esp32s3, 10, 0),
            (Chip::Esp32c3, 4, 5), (Chip::Esp32c6, 6, 7), (Chip::Esp32h2, 1, 0),
        ] {
            mock::set_chip(chip);
            assert!(is_adc_pin(ok), "{chip:?} {ok}");
            assert!(!is_adc_pin(bad), "{chip:?} {bad}");
        }
        assert_eq!(pins::channel_in(pins::ESP32, 32), Some(4));
    }

    #[test]
    fn mock_read() {
        mock::reset();
        mock::set(2, 1234);
        assert_eq!(read(2), 1234);
        assert_eq!(read(3), 0);
        assert_eq!(try_read(9), None);
    }

    #[test]
    #[should_panic]
    fn read_bad_pin_panics() { mock::reset(); read(20); }
}
