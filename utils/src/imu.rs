//! Onboard QMI8658 IMU of the **Waveshare ESP32-S3-Touch-LCD-2** (feature `imu`, `hal` + esp32s3).
//!
//! I2C0 on SDA 48 / SCL 47 at 400 kHz, address `0x6B` (from Waveshare's ESP32-S3-Touch-LCD-2-Demo:
//! `EXAMPLE_PIN_NUM_I2C_SDA/SCL`, `QMI8658_SENSOR_ADDR` / `IMU_ADDRESS`). The bus is shared with the
//! CST816D touch controller (unused). Accelerometer configured as in the demo (±4 g, ~235 Hz ODR),
//! but without the demo's self-test bit. Other builds return [`ImuError::Unsupported`].
#[cfg(feature = "hal")]
use alloc::string::String;
#[cfg(not(feature = "hal"))]
use std::string::String;

/// I2C pins and address.
pub mod pins {
    pub const SDA: u8 = 48;
    pub const SCL: u8 = 47;
    pub const ADDR: u8 = 0x6B;
}

/// Output unit of [`get_accelerometers`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unit {
    /// Standard gravity (default).
    #[default]
    G,
    /// m/s² (g × 9.80665).
    Ms2,
    /// Raw signed 16-bit counts (±4 g range: 8192 counts/g).
    Raw,
}

impl Unit {
    /// `g`, `m/s²` / `m/s2` / `ms2`, `raw` (case-insensitive); empty = default.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("g") { Some(Self::G) }
        else if ["m/s²", "m/s2", "ms2", "m/s^2"].iter().any(|u| s.eq_ignore_ascii_case(u)) { Some(Self::Ms2) }
        else if s.eq_ignore_ascii_case("raw") { Some(Self::Raw) }
        else { None }
    }
}

/// Maximum samples per call (the result must fit the BLE log buffer).
pub const MAX_COUNT: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImuError {
    /// No IMU in this build (needs feature `imu`, hal, esp32s3).
    Unsupported,
    /// No QMI8658 answered (WHO_AM_I != 0x05).
    NotFound,
    /// I2C error or no data.
    Bus,
    /// count is 0 or above [`MAX_COUNT`].
    BadCount,
}

impl core::fmt::Display for ImuError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "no IMU in this build (needs feature `imu` on esp32s3 hal)",
            Self::NotFound => "QMI8658 not found",
            Self::Bus => "I2C error",
            Self::BadCount => "count must be 1..=100",
        })
    }
}

/// Read `count` accelerometer samples, `interval_ms` apart (the calling thread sleeps in between).
/// Returns JSON: `{"x":..,"y":..,"z":..}` for one sample, an array of them for `count > 1`.
pub fn get_accelerometers(unit: Unit, count: u32, interval_ms: u32) -> Result<String, ImuError> {
    if count == 0 || count > MAX_COUNT { return Err(ImuError::BadCount); }
    // hal: the heap (and scheduler, so the interval sleeps) come up with BLE; make sure they exist.
    #[cfg(feature = "hal")]
    crate::ble::start_rtos();
    let mut out = String::new();
    use core::fmt::Write;
    if count > 1 { out.push('['); }
    for i in 0..count {
        if i > 0 {
            out.push(',');
            if interval_ms > 0 { crate::time::delay_ms(interval_ms); }
        }
        let [x, y, z] = imp::read_raw()?;
        let _ = match unit {
            Unit::Raw => write!(out, "{{\"x\":{x},\"y\":{y},\"z\":{z}}}"),
            _ => {
                let k = if unit == Unit::G { 1.0 / 8192.0 } else { 9.80665 / 8192.0 };
                write!(out, "{{\"x\":{:.4},\"y\":{:.4},\"z\":{:.4}}}", x as f32 * k, y as f32 * k, z as f32 * k)
            }
        };
    }
    if count > 1 { out.push(']'); }
    Ok(out)
}

#[cfg(all(feature = "imu", feature = "hal", feature = "esp32s3"))]
mod imp {
    use core::cell::RefCell;
    use critical_section::Mutex;
    use esp_hal::{Blocking, i2c::master::{Config, I2c}, time::Rate};

    use super::{ImuError, pins::ADDR};

    // Only moved in/out inside the critical section; transfers run outside it.
    static BUS: Mutex<RefCell<Option<I2c<'static, Blocking>>>> = Mutex::new(RefCell::new(None));

    fn init() -> Result<I2c<'static, Blocking>, ImuError> {
        // SAFETY: the IMU code owns I2C0 and GPIO 47/48 (documented in the module docs).
        let (i2c0, sda, scl) = unsafe {
            use esp_hal::peripherals::*;
            (I2C0::steal(), GPIO48::steal(), GPIO47::steal())
        };
        let mut bus = I2c::new(i2c0, Config::default().with_frequency(Rate::from_khz(400)))
            .map_err(|_| ImuError::Bus)?
            .with_sda(sda)
            .with_scl(scl);
        let mut id = [0u8];
        bus.write_read(ADDR, &[0x00], &mut id).map_err(|_| ImuError::NotFound)?;
        if id[0] != 0x05 { return Err(ImuError::NotFound); }
        let w = |bus: &mut I2c<'static, Blocking>, r: u8, v: u8| bus.write(ADDR, &[r, v]).map_err(|_| ImuError::Bus);
        w(&mut bus, 0x60, 0xB0)?; // RESET
        crate::time::delay_ms(10);
        w(&mut bus, 0x02, 0x40)?; // CTRL1: address auto-increment
        w(&mut bus, 0x03, 0x15)?; // CTRL2: accel ±4 g, ODR ~235 Hz (demo uses 0x95 = same + self-test)
        w(&mut bus, 0x08, 0x01)?; // CTRL7: accelerometer enable
        crate::time::delay_ms(20);
        Ok(bus)
    }

    pub fn read_raw() -> Result<[i16; 3], ImuError> {
        let taken = critical_section::with(|cs| BUS.borrow(cs).borrow_mut().take());
        let mut bus = match taken { Some(b) => b, None => init()? };
        let r = (|| {
            for _ in 0..20 {
                let mut st = [0u8];
                bus.write_read(ADDR, &[0x2E], &mut st).map_err(|_| ImuError::Bus)?; // STATUS0
                if st[0] & 0x01 != 0 {
                    let mut b = [0u8; 6];
                    bus.write_read(ADDR, &[0x35], &mut b).map_err(|_| ImuError::Bus)?; // AX_L..AZ_H
                    return Ok([0, 2, 4].map(|i| i16::from_le_bytes([b[i], b[i + 1]])));
                }
                crate::time::delay_ms(2);
            }
            Err(ImuError::Bus)
        })();
        critical_section::with(|cs| *BUS.borrow(cs).borrow_mut() = Some(bus));
        r
    }
}

#[cfg(not(all(feature = "imu", feature = "hal", feature = "esp32s3")))]
mod imp {
    pub fn read_raw() -> Result<[i16; 3], super::ImuError> { Err(super::ImuError::Unsupported) }
}
