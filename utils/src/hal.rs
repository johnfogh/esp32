//! esp-hal (no_std) backend.
use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{self, Level, OutputConfig},
    time::Instant,
};

// Panic handler (prints a backtrace over the console).
use esp_backtrace as _;

// App descriptor required by the ESP-IDF 2nd-stage bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

pub use esp_hal::peripherals::Peripherals;

static FLASH_SIZE: AtomicU32 = AtomicU32::new(0);

/// Initialised board: owns all peripherals not consumed by `init`.
pub struct Board {
    pub peripherals: Peripherals,
}

/// Board setup: logger, max CPU clock, flash detection. Call once.
pub fn init() -> Board {
    init_logger();
    let mut peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    {
        let flash = esp_storage::FlashStorage::new(peripherals.FLASH.reborrow());
        FLASH_SIZE.store(flash.capacity() as u32, Ordering::Relaxed);
    }
    Board { peripherals }
}

/// Install the esp-println logger (level from `ESP_LOG` at build time, default info).
/// With the `ble` feature the records are also mirrored to [`crate::blelog`] (BLE manager output).
#[cfg(not(feature = "ble"))]
pub fn init_logger() {
    esp_println::logger::init_logger_from_env();
}

#[cfg(feature = "ble")]
pub fn init_logger() {
    struct Mirror;
    impl log::Log for Mirror {
        fn enabled(&self, _: &log::Metadata) -> bool { true }
        fn log(&self, record: &log::Record) {
            esp_println::println!("{} - {}", record.level(), record.args());
            crate::blelog::mirror(record);
        }
        fn flush(&self) {}
    }
    static MIRROR: Mirror = Mirror;
    let env = option_env!("ESP_LOG").unwrap_or("info");
    let level = [
        ("off", log::LevelFilter::Off),
        ("error", log::LevelFilter::Error),
        ("warn", log::LevelFilter::Warn),
        ("debug", log::LevelFilter::Debug),
        ("trace", log::LevelFilter::Trace),
    ]
    .into_iter()
    .find(|(n, _)| env.eq_ignore_ascii_case(n))
    .map_or(log::LevelFilter::Info, |(_, l)| l);
    // SAFETY: called once from `init` before other threads exist.
    unsafe {
        let _ = log::set_logger_racy(&MIRROR);
        log::set_max_level_racy(level);
    }
}

pub(crate) fn flash_size() -> Option<u32> {
    match FLASH_SIZE.load(Ordering::Relaxed) { 0 => None, n => Some(n) }
}

pub(crate) fn mac() -> [u8; 6] {
    let mut out = [0u8; 6];
    out.copy_from_slice(&esp_hal::efuse::base_mac_address().as_bytes()[..6]);
    out
}

/// Blocking delay. Once the esp-rtos scheduler runs (BLE started) this sleeps the calling thread so
/// other threads (the BLE stack) get the CPU; before that it busy-waits.
pub fn delay_ms(ms: u32) {
    #[cfg(all(feature = "ble", not(feature = "esp32s2")))]
    if crate::ble::rtos_running() {
        esp_radio_rtos_driver::usleep(ms.saturating_mul(1000));
        return;
    }
    Delay::new().delay_millis(ms)
}
pub(crate) fn uptime_us() -> u64 { Instant::now().duration_since_epoch().as_micros() }

/// Push-pull output pin, starts low.
pub struct Output(gpio::Output<'static>);
impl Output {
    pub fn new(pin: impl gpio::OutputPin + 'static) -> Self {
        Self(gpio::Output::new(pin, Level::Low, OutputConfig::default()))
    }
    pub fn set_high(&mut self) { self.0.set_high() }
    pub fn set_low(&mut self) { self.0.set_low() }
    pub fn set(&mut self, high: bool) { if high { self.set_high() } else { self.set_low() } }
}

// ---------------------------------------------------------------- ADC
use esp_hal::analog::adc::{Adc, AdcChannel, AdcConfig, Attenuation};

fn adc1_read_pin<P: AdcChannel + gpio::AnalogPin>(pin: P) -> u16 {
    let mut cfg = AdcConfig::new();
    let mut p = cfg.enable_pin(pin, Attenuation::_11dB);
    // SAFETY: `utils::adc` documents that it owns ADC1; the driver is dropped after one read.
    let mut adc = Adc::new(unsafe { esp_hal::peripherals::ADC1::steal() }, cfg);
    loop {
        if let Ok(v) = adc.read_oneshot(&mut p) { return v; }
    }
}

macro_rules! adc1_dispatch {
    ($pin:expr; $($n:literal => $gpio:ident),* $(,)?) => {
        match $pin {
            // SAFETY: the pin is borrowed only for the duration of a single read;
            // callers must not use it concurrently as a digital pin.
            $( $n => Some(adc1_read_pin(unsafe { esp_hal::peripherals::$gpio::steal() })), )*
            _ => None,
        }
    };
}

/// One raw ADC1 sample of GPIO `pin` (channel is validated by the caller).
pub(crate) fn adc_read(pin: u8, _channel: u8) -> Option<u16> {
    #[cfg(feature = "esp32")]
    return adc1_dispatch!(pin; 36 => GPIO36, 37 => GPIO37, 38 => GPIO38, 39 => GPIO39, 32 => GPIO32, 33 => GPIO33, 34 => GPIO34, 35 => GPIO35);
    #[cfg(any(feature = "esp32s2", feature = "esp32s3"))]
    return adc1_dispatch!(pin; 1 => GPIO1, 2 => GPIO2, 3 => GPIO3, 4 => GPIO4, 5 => GPIO5, 6 => GPIO6, 7 => GPIO7, 8 => GPIO8, 9 => GPIO9, 10 => GPIO10);
    #[cfg(feature = "esp32c3")]
    return adc1_dispatch!(pin; 0 => GPIO0, 1 => GPIO1, 2 => GPIO2, 3 => GPIO3, 4 => GPIO4);
    #[cfg(feature = "esp32c6")]
    return adc1_dispatch!(pin; 0 => GPIO0, 1 => GPIO1, 2 => GPIO2, 3 => GPIO3, 4 => GPIO4, 5 => GPIO5, 6 => GPIO6);
    #[cfg(feature = "esp32h2")]
    return adc1_dispatch!(pin; 1 => GPIO1, 2 => GPIO2, 3 => GPIO3, 4 => GPIO4, 5 => GPIO5);
}
