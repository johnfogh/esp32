//! esp-idf (std) backend.
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{self, PinDriver},
};
use esp_idf_svc::sys;

pub use esp_idf_svc::hal::peripherals::Peripherals;

/// Initialised board: owns all peripherals.
pub struct Board {
    pub peripherals: Peripherals,
}

/// Board setup: runtime patches, EspLogger, peripherals. Call once.
pub fn init() -> Board {
    sys::link_patches();
    init_logger();
    Board { peripherals: Peripherals::take().expect("peripherals already taken") }
}

/// Install the ESP-IDF logger bridge for the `log` crate.
/// With the `ble` feature the records are also mirrored to [`crate::blelog`] (BLE manager output).
#[cfg(not(feature = "ble"))]
pub fn init_logger() {
    esp_idf_svc::log::EspLogger::initialize_default();
}

#[cfg(feature = "ble")]
pub fn init_logger() {
    use esp_idf_svc::log::{EspIdfLogFilter, EspIdfLogger};
    struct Mirror(EspIdfLogger<EspIdfLogFilter>);
    impl log::Log for Mirror {
        fn enabled(&self, m: &log::Metadata) -> bool { log::Log::enabled(&self.0, m) }
        fn log(&self, record: &log::Record) {
            if log::Log::enabled(&self.0, record.metadata()) {
                log::Log::log(&self.0, record);
                crate::blelog::mirror(record);
            }
        }
        fn flush(&self) { log::Log::flush(&self.0) }
    }
    static MIRROR: Mirror = Mirror(EspIdfLogger::new(EspIdfLogFilter::new()));
    if log::set_logger(&MIRROR).is_ok() {
        MIRROR.0.filter().initialize();
    }
}

pub(crate) fn flash_size() -> Option<u32> {
    let mut size = 0u32;
    let rc = unsafe { sys::esp_flash_get_size(core::ptr::null_mut(), &mut size) };
    (rc == sys::ESP_OK && size > 0).then_some(size)
}

pub(crate) fn mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    unsafe { sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    mac
}

pub fn delay_ms(ms: u32) { FreeRtos::delay_ms(ms) }
pub(crate) fn uptime_us() -> u64 { unsafe { sys::esp_timer_get_time() as u64 } }

/// Push-pull output pin, starts low.
pub struct Output(PinDriver<'static, gpio::Output>);
impl Output {
    pub fn new(pin: impl gpio::OutputPin + 'static) -> Self {
        let mut p = PinDriver::output(pin).expect("gpio output");
        let _ = p.set_low();
        Self(p)
    }
    pub fn set_high(&mut self) { let _ = self.0.set_high(); }
    pub fn set_low(&mut self) { let _ = self.0.set_low(); }
    pub fn set(&mut self, high: bool) { if high { self.set_high() } else { self.set_low() } }
}

// ---------------------------------------------------------------- ADC
/// ADC1 oneshot unit handle, created lazily (raw esp-idf `adc_oneshot` API).
struct AdcUnit(sys::adc_oneshot_unit_handle_t);
// SAFETY: the handle is only used while holding the mutex below.
unsafe impl Send for AdcUnit {}
static ADC1: std::sync::Mutex<Option<AdcUnit>> = std::sync::Mutex::new(None);

/// One raw ADC1 sample of `channel` (GPIO `_pin`), 12 dB attenuation, default bit width.
pub(crate) fn adc_read(_pin: u8, channel: u8) -> Option<u16> {
    let mut guard = ADC1.lock().ok()?;
    if guard.is_none() {
        let cfg = sys::adc_oneshot_unit_init_cfg_t {
            unit_id: sys::adc_unit_t_ADC_UNIT_1,
            ulp_mode: sys::adc_ulp_mode_t_ADC_ULP_MODE_DISABLE,
            ..Default::default()
        };
        let mut h: sys::adc_oneshot_unit_handle_t = core::ptr::null_mut();
        if unsafe { sys::adc_oneshot_new_unit(&cfg, &mut h) } != sys::ESP_OK { return None; }
        *guard = Some(AdcUnit(h));
    }
    let h = guard.as_ref()?.0;
    let ch_cfg = sys::adc_oneshot_chan_cfg_t {
        atten: sys::adc_atten_t_ADC_ATTEN_DB_12,
        bitwidth: sys::adc_bitwidth_t_ADC_BITWIDTH_DEFAULT,
    };
    let ch = channel as sys::adc_channel_t;
    if unsafe { sys::adc_oneshot_config_channel(h, ch, &ch_cfg) } != sys::ESP_OK { return None; }
    let mut raw: core::ffi::c_int = 0;
    if unsafe { sys::adc_oneshot_read(h, ch, &mut raw) } != sys::ESP_OK { return None; }
    Some(raw as u16)
}
