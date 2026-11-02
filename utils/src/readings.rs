//! Standard status readings ([`add_standard_readings`]): one sample per call, keyed by the synced
//! time (uptime + `"synced":false` before the first sync). Per pass the sample is only
//! `{"uptime_ms":..}`; right before each new snapshot (~1/s) `send_buffer` adds one full sample via
//! [`add_full_reading`] with these fields (unavailable ones skipped):
//!
//! | key           | value                         | builds                                             |
//! |---------------|-------------------------------|----------------------------------------------------|
//! | `uptime_ms`   | board uptime (ms)             | all with a status buffer                           |
//! | `battery`     | `{"v":volts,"pct":percent}`   | esp32s3 + `display` (Waveshare ESP32-S3-Touch-LCD-2)|
//! | `temp`        | chip temperature (°C)         | hal: esp32c3/c6 (esp-hal `tsens`), esp32s3 (SENS registers); idf: all but esp32 |
//! | `accel`       | `{"x":..,"y":..,"z":..}` in g | hal + esp32s3 + `imu` (QMI8658)                     |
//!
//! Battery: GPIO5 = ADC1 channel 4 behind the board's 1:3 divider (Waveshare demos:
//! `04_lvgl_battery` `voltage = cal_mV / 1000 * 3.0`, Arduino `06_lvgl_battery`
//! `3.3 / 4096 * raw * 3`). We read the raw 12-bit code at 11 dB attenuation and use
//! `V = raw / 4095 * 3.1 V * 3` (uncalibrated, ±~0.1 V). Percent: linear 3.3 V → 0 %, 4.2 V → 100 %,
//! clamped. On USB without a battery the charger output is read (typically ~4.1–4.3 V → ~100 %).
//! ADC1 is opened per read (shared with `utils::adc`), the I²C bus and the temperature sensor once.
use crate::status::Json;

/// Battery divider ratio (Waveshare ESP32-S3-Touch-LCD-2).
pub const BATTERY_DIVIDER: f32 = 3.0;
/// ADC full scale at 11 dB attenuation used for the conversion (V).
pub const ADC_FULL_SCALE_V: f32 = 3.1;
/// Linear percent curve end points (V).
pub const BATTERY_EMPTY_V: f32 = 3.3;
pub const BATTERY_FULL_V: f32 = 4.2;

/// Battery voltage and percent, if this build has the board's battery divider.
pub fn battery() -> Option<(f32, u8)> {
    #[cfg(all(feature = "esp32s3", feature = "display"))]
    {
        let raw = crate::backend::adc_read(5, 4)? as f32;
        let v = raw / 4095.0 * ADC_FULL_SCALE_V * BATTERY_DIVIDER;
        let pct = ((v - BATTERY_EMPTY_V) / (BATTERY_FULL_V - BATTERY_EMPTY_V) * 100.0).clamp(0.0, 100.0);
        Some((v, (pct + 0.5) as u8))
    }
    #[cfg(not(all(feature = "esp32s3", feature = "display")))]
    { None }
}

/// Internal chip temperature (°C), where supported.
pub fn temperature() -> Option<f32> { temp::read() }

#[cfg(all(feature = "hal", any(feature = "esp32c3", feature = "esp32c6")))]
mod temp {
    use core::cell::RefCell;
    use critical_section::Mutex;
    use esp_hal::tsens::{Config, TemperatureSensor};
    static S: Mutex<RefCell<Option<TemperatureSensor<'static>>>> = Mutex::new(RefCell::new(None));
    static FAILED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    pub fn read() -> Option<f32> {
        if FAILED.load(core::sync::atomic::Ordering::Relaxed) { return None; }
        critical_section::with(|cs| {
            let mut s = S.borrow(cs).borrow_mut();
            if s.is_none() {
                // SAFETY: utils owns TSENS (documented in the README); created once.
                match TemperatureSensor::new(unsafe { esp_hal::peripherals::TSENS::steal() }, Config::default()) {
                    Ok(t) => *s = Some(t),
                    Err(_) => { FAILED.store(true, core::sync::atomic::Ordering::Relaxed); return None; }
                }
            }
            s.as_ref().map(|t| t.get_temperature().to_celsius())
        })
    }
}

/// esp32s3 hal: esp-hal 1.2 has no `tsens` driver for the S3, so drive the SENS temperature sensor
/// registers directly, as ESP-IDF's `hal/esp32s3/include/hal/temperature_sensor_ll.h` does:
/// clock gate on, power-up/XPD forced, clk_div 6, default range (−10…80 °C: DAC 15 via regi2c,
/// offset 0), then per read `dump_out` → wait `ready` → `out`. °C = 0.4386·raw − 27.88·offset − 20.52.
#[cfg(all(feature = "hal", feature = "esp32s3"))]
mod temp {
    use core::sync::atomic::{AtomicU8, Ordering};
    /// 0 = not initialised, 1 = ready, 2 = failed.
    static STATE: AtomicU8 = AtomicU8::new(0);
    /// Default range offset (ESP-IDF table: range −10…80 °C → reg_val 15, offset 0).
    const OFFSET: f32 = 0.0;
    const DAC_DEFAULT: u8 = 15;
    // regi2c SAR ADC block: I2C_SAR_ADC 0x69, host 1, I2C_SARADC_TSENS_DAC reg 0x6 bits 3:0.
    const I2C_SAR_ADC: u8 = 0x69;
    const I2C_SAR_ADC_HOSTID: u8 = 1;
    const I2C_SARADC_TSENS_DAC: u8 = 0x6;
    unsafe extern "C" {
        fn rom_i2c_writeReg_Mask(block: u8, host_id: u8, reg_add: u8, msb: u8, lsb: u8, data: u8);
    }

    fn init() {
        let sens = esp_hal::peripherals::SENS::regs();
        critical_section::with(|_| {
            sens.sar_peri_clk_gate_conf().modify(|_, w| w.tsens_clk_en().set_bit());
            sens.sar_tsens_ctrl().modify(|_, w| unsafe {
                w.sar_tsens_power_up_force().set_bit().sar_tsens_power_up().set_bit().sar_tsens_clk_div().bits(6)
            });
            sens.sar_tsens_ctrl2().modify(|_, w| unsafe {
                // 2-bit field (TRM): 0b11 = software force power-up (0b10 force down, 0b0x FSM).
                w.sar_tsens_xpd_force().bits(0b11).sar_tsens_clk_inv().set_bit()
            });
            // SAFETY: ROM regi2c helper (esp32s3.rom.ld), same call ESP-IDF's set_range makes.
            unsafe { rom_i2c_writeReg_Mask(I2C_SAR_ADC, I2C_SAR_ADC_HOSTID, I2C_SARADC_TSENS_DAC, 3, 0, DAC_DEFAULT) };
        });
        // Let the sensor settle (ESP-IDF recommends a few hundred µs after power-up).
        esp_hal::delay::Delay::new().delay_micros(300);
    }

    fn raw() -> Option<u8> {
        let sens = esp_hal::peripherals::SENS::regs();
        sens.sar_tsens_ctrl().modify(|_, w| w.sar_tsens_dump_out().set_bit());
        let mut ok = false;
        for _ in 0..10_000 {
            if sens.sar_tsens_ctrl().read().sar_tsens_ready().bit_is_set() { ok = true; break; }
        }
        let v = sens.sar_tsens_ctrl().read().sar_tsens_out().bits();
        sens.sar_tsens_ctrl().modify(|_, w| w.sar_tsens_dump_out().clear_bit());
        ok.then_some(v)
    }

    pub fn read() -> Option<f32> {
        match STATE.load(Ordering::Acquire) {
            2 => return None,
            0 => { init(); STATE.store(1, Ordering::Release); }
            _ => {}
        }
        let r = raw()?;
        if r == 0 || r == 0xff { return None; }
        let c = 0.4386 * r as f32 - 27.88 * OFFSET - 20.52;
        (-40.0..=125.0).contains(&c).then_some(c)
    }
}

#[cfg(all(feature = "idf", not(feature = "esp32")))]
mod temp {
    use esp_idf_svc::sys;
    /// Driver handle (as usize: raw pointers are not Send); 0 = not installed, 1 = failed.
    static H: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
    pub fn read() -> Option<f32> {
        let mut h = H.lock().ok()?;
        if *h == 1 { return None; }
        if *h == 0 {
            let cfg = sys::temperature_sensor_config_t { range_min: -10, range_max: 80, ..Default::default() };
            let mut handle: sys::temperature_sensor_handle_t = core::ptr::null_mut();
            // SAFETY: plain ESP-IDF driver calls with valid pointers; installed once.
            let ok = unsafe {
                sys::temperature_sensor_install(&cfg, &mut handle) == sys::ESP_OK
                    && sys::temperature_sensor_enable(handle) == sys::ESP_OK
            };
            *h = if ok { handle as usize } else { 1 };
            if !ok { return None; }
        }
        let mut c = 0f32;
        // SAFETY: handle installed and enabled above.
        let ok = unsafe { sys::temperature_sensor_get_celsius(*h as sys::temperature_sensor_handle_t, &mut c) == sys::ESP_OK };
        ok.then_some(c)
    }
}

#[cfg(not(any(all(feature = "hal", any(feature = "esp32c3", feature = "esp32c6", feature = "esp32s3")), all(feature = "idf", not(feature = "esp32")))))]
mod temp {
    pub fn read() -> Option<f32> { None }
}

/// Add one sample `{"uptime_ms":..}` per call (main-loop pass). The full readings are added by
/// [`add_full_reading`], which `send_buffer` calls right before it creates a snapshot (~1/s).
/// Call once per main-loop pass, before [`crate::send_buffer`].
pub fn add_standard_readings() {
    if !crate::status::ENABLED { return; }
    let up = crate::time::uptime_ms();
    crate::status::buffer_add_sample(None, &[("uptime_ms", &up)]);
}

/// Poll battery, temp and accel now and add one sample `{uptime_ms, battery{v,pct}, temp,
/// accel{x,y,z}}` (unavailable readings skipped). Called by `send_buffer` (outside the buffer lock)
/// just before a new snapshot is created, so each snapshot ends with one full sample.
pub fn add_full_reading() {
    if !crate::status::ENABLED { return; }
    let up = crate::time::uptime_ms();
    let mut bat = Fixed::<48>::new();
    let bat_ok = battery().is_some_and(|(v, pct)| core::fmt::write(&mut bat, format_args!("{{\"v\":{:.2},\"pct\":{}}}", v, pct)).is_ok());
    let mut tmp = Fixed::<16>::new();
    let tmp_ok = temperature().is_some_and(|c| core::fmt::write(&mut tmp, format_args!("{:.1}", c)).is_ok());
    #[cfg(all(feature = "imu", feature = "hal", feature = "esp32s3"))]
    let acc = crate::imu::get_accelerometers(crate::imu::Unit::G, 1, 0).ok();
    let (bat, tmp) = (Json(bat.as_str()), Json(tmp.as_str()));
    let mut fields: [(&str, &dyn crate::status::BufValue); 4] = [("uptime_ms", &up); 4];
    let mut n = 1;
    if bat_ok { fields[n] = ("battery", &bat); n += 1; }
    if tmp_ok { fields[n] = ("temp", &tmp); n += 1; }
    #[cfg(all(feature = "imu", feature = "hal", feature = "esp32s3"))]
    let acc_j = acc.as_deref().map(Json);
    #[cfg(all(feature = "imu", feature = "hal", feature = "esp32s3"))]
    if let Some(a) = &acc_j { fields[n] = ("accel", a); n += 1; }
    crate::status::buffer_add_sample(None, &fields[..n]);
}

/// Small stack string for formatting values without the heap.
struct Fixed<const N: usize> { b: [u8; N], n: usize }
impl<const N: usize> Fixed<N> {
    fn new() -> Self { Self { b: [0; N], n: 0 } }
    fn as_str(&self) -> &str { core::str::from_utf8(&self.b[..self.n]).unwrap_or("null") }
}
impl<const N: usize> core::fmt::Write for Fixed<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        if self.n + b.len() > N { return Err(core::fmt::Error); }
        self.b[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
        Ok(())
    }
}
