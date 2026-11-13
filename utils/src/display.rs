//! Status screen for the **Waveshare ESP32-S3-Touch-LCD-2** (feature `display`, `hal` + esp32s3).
//!
//! ST7789T3 240x320 IPS over SPI2 at 40 MHz, pins from Waveshare's ESP32-S3-Touch-LCD-2-Demo
//! (`EXAMPLE_PIN_NUM_LCD_*`): SCLK 39, MOSI 38, MISO 40, DC 42, CS 45, RST none, backlight 1.
//! Init as in the demo: colour inversion on (IPS), RGB order, no offsets, portrait.
//!
//! [`start`] spawns an esp-rtos thread (priority 0, like main and BLE) that redraws about once a
//! second: device name `<project>-xxyyzz`, BLE state, uptime, an optional 2×2 sensor value grid
//! ([`set_grid_values`]), the last log lines (mirrored by the `ble` logger), then the latest
//! status-buffer sample ([`crate::status`]: up to 8 fields as `key=value`, plus pending sample
//! count and sync state). Only lines whose text changed are redrawn (text drawn with a background
//! colour, padded to the full width), and the thread sleeps between frames, so BLE is not starved.
//! The thread owns SPI2 and GPIO 1/38/39/40/42/45 (stolen): do not use them elsewhere.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Four corner ADC readings for the status-screen grid (TL, TR, BL, BR).
static GRID_VALUES: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static GRID_ON: AtomicBool = AtomicBool::new(false);

/// Publish last sensor readings for the 2×2 display grid (TL, TR, BL, BR).
/// No-op when this build has no status screen; still stores values so callers can be unconditional.
pub fn set_grid_values(values: [u16; 4]) {
    for (slot, v) in GRID_VALUES.iter().zip(values.iter()) {
        slot.store(*v as u32, Ordering::Relaxed);
    }
    GRID_ON.store(true, Ordering::Release);
}

#[cfg_attr(not(all(feature = "display", feature = "hal", feature = "esp32s3")), allow(dead_code))]
fn grid_values() -> Option<[u16; 4]> {
    if !GRID_ON.load(Ordering::Acquire) {
        return None;
    }
    let mut out = [0u16; 4];
    for (i, slot) in GRID_VALUES.iter().enumerate() {
        out[i] = slot.load(Ordering::Relaxed).min(u16::MAX as u32) as u16;
    }
    Some(out)
}

#[cfg(all(feature = "display", feature = "hal", feature = "esp32s3"))]
mod imp {
    use embedded_graphics::{
        mono_font::{MonoTextStyleBuilder, ascii::{FONT_6X10, FONT_10X20}},
        pixelcolor::Rgb565,
        prelude::*,
        text::{Baseline, Text},
    };
    use embedded_hal_bus::spi::ExclusiveDevice;
    use esp_hal::{
        delay::Delay,
        gpio::{Level, Output, OutputConfig},
        spi::{Mode, master::{Config, Spi}},
        time::Rate,
    };
    use mipidsi::{Builder, interface::SpiInterface, models::ST7789, options::{ColorInversion, ColorOrder}};
    use static_cell::StaticCell;

    use crate::blelog::{RECENT_LINES, RECENT_WIDTH};

    static PROJECT: StaticCell<&'static str> = StaticCell::new();
    const COLS_SMALL: usize = 40; // 240 px / 6 px
    const COLS_BIG: usize = 24; // 240 px / 10 px
    /// Header lines: name, BLE, uptime.
    const HDR: usize = 3;
    /// 2×2 piezo grid (two text rows).
    const GRID_LINES: usize = 2;
    const GRID_ROW: usize = HDR;
    /// Separator after grid.
    const SEP_ROW: usize = HDR + GRID_LINES;
    const LOG_ROW: usize = SEP_ROW + 1;
    /// Status-buffer rows (header + keys) below the log lines.
    const BUF_KEYS: usize = 8;
    const ROWS: usize = HDR + GRID_LINES + 1 + RECENT_LINES + 1 + BUF_KEYS;
    const BUF_ROW: usize = LOG_ROW + RECENT_LINES;

    pub fn start(project: &'static str) -> Result<(), &'static str> {
        let project: &'static &'static str = PROJECT.try_init(project).ok_or("display already started")?;
        crate::ble::start_rtos();
        extern "C" fn task(param: *mut core::ffi::c_void) {
            // SAFETY: points at the 'static PROJECT value.
            let project: &'static str = unsafe { *(param as *const &'static str) };
            run(project)
        }
        // SAFETY: the task never returns; its parameter is 'static.
        unsafe {
            esp_radio_rtos_driver::task_create("display", task, project as *const _ as *mut core::ffi::c_void, 0, None, 12 * 1024);
        }
        Ok(())
    }

    /// Fixed-width text line.
    #[derive(Clone, Copy, PartialEq)]
    struct Line { b: [u8; COLS_SMALL], n: usize }
    impl Line {
        const EMPTY: Self = Self { b: [b' '; COLS_SMALL], n: 0 };
        fn from_fmt(args: core::fmt::Arguments) -> Self {
            let mut l = Self::EMPTY;
            let _ = core::fmt::write(&mut l, args);
            l
        }
        fn padded(&self, cols: usize) -> &str {
            core::str::from_utf8(&self.b[..cols]).unwrap_or("")
        }
    }
    impl core::fmt::Write for Line {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for c in s.bytes() {
                if self.n == COLS_SMALL { break; }
                self.b[self.n] = if c.is_ascii_graphic() || c == b' ' { c } else { b'?' };
                self.n += 1;
            }
            Ok(())
        }
    }

    /// Split a JSON object body (`"a":1,"b":{"x":1,"y":2}`) at top-level commas (no allocation).
    fn split_top(s: &str) -> impl Iterator<Item = &str> {
        let b = s.as_bytes();
        let (mut start, mut depth, mut in_str, mut esc, mut i) = (0usize, 0i32, false, false, 0usize);
        core::iter::from_fn(move || {
            if start >= b.len() { return None; }
            while i < b.len() {
                let c = b[i];
                i += 1;
                if in_str {
                    if esc { esc = false } else if c == b'\\' { esc = true } else if c == b'"' { in_str = false }
                    continue;
                }
                match c {
                    b'"' => in_str = true,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => depth -= 1,
                    b',' if depth == 0 => { let f = &s[start..i - 1]; start = i; return Some(f); }
                    _ => {}
                }
            }
            let f = &s[start..];
            start = b.len();
            Some(f)
        })
    }

    fn run(project: &'static str) -> ! {
        let wait = || esp_radio_rtos_driver::usleep(1_000_000);
        match draw_loop(project) {
            Ok(never) => match never {},
            Err(e) => {
                log::warn!("display: {e}");
                loop { wait(); }
            }
        }
    }

    fn draw_loop(project: &'static str) -> Result<core::convert::Infallible, &'static str> {
        // SAFETY: the display thread owns these (documented in the module docs).
        let (spi2, sclk, mosi, miso, dc, cs, bl) = unsafe {
            use esp_hal::peripherals::*;
            (SPI2::steal(), GPIO39::steal(), GPIO38::steal(), GPIO40::steal(), GPIO42::steal(), GPIO45::steal(), GPIO1::steal())
        };
        let _backlight = Output::new(bl, Level::High, OutputConfig::default());
        let spi = Spi::new(spi2, Config::default().with_frequency(Rate::from_mhz(40)).with_mode(Mode::_0))
            .map_err(|_| "spi config")?
            .with_sck(sclk)
            .with_mosi(mosi)
            .with_miso(miso);
        let cs = Output::new(cs, Level::High, OutputConfig::default());
        let dc = Output::new(dc, Level::Low, OutputConfig::default());
        let dev = ExclusiveDevice::new(spi, cs, Delay::new()).map_err(|_| "spi device")?;
        static BUF: StaticCell<[u8; 512]> = StaticCell::new();
        let di = SpiInterface::new(dev, dc, BUF.init([0; 512]));
        let mut delay = Delay::new();
        let mut lcd = Builder::new(ST7789, di)
            .display_size(240, 320)
            .invert_colors(ColorInversion::Inverted)
            .color_order(ColorOrder::Rgb)
            .init(&mut delay)
            .map_err(|_| "st7789 init")?;
        lcd.clear(Rgb565::BLACK).map_err(|_| "clear")?;

        let mac = crate::backend::mac();
        let name = crate::ble_proto::device_name(project, mac);
        let big = MonoTextStyleBuilder::new().font(&FONT_10X20).text_color(Rgb565::CYAN).background_color(Rgb565::BLACK).build();
        let small = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::WHITE).background_color(Rgb565::BLACK).build();
        let dim = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::CSS_LIGHT_GRAY).background_color(Rgb565::BLACK).build();
        let mut shown = [Line { b: [0; COLS_SMALL], n: usize::MAX }; ROWS];
        let mut rec = [[0u8; RECENT_WIDTH]; RECENT_LINES];
        let mut lens = [0usize; RECENT_LINES];
        loop {
            let up = crate::time::uptime_ms() / 1000;
            let now = crate::time::uptime_ms();
            let since = crate::blelog::last_disconnect_ms().map(|d| now.saturating_sub(d));
            let p = crate::blelog::peer();
            let mut state = Line::EMPTY;
            let _ = if crate::blelog::connected() {
                match crate::blelog::rssi() {
                    Some(r) => core::fmt::write(&mut state, format_args!("connected {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} {r} dBm", p[0], p[1], p[2], p[3], p[4], p[5])),
                    None => core::fmt::write(&mut state, format_args!("connected {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", p[0], p[1], p[2], p[3], p[4], p[5])),
                }
            } else if since.is_some_and(|s| s < 1500) {
                core::fmt::write(&mut state, format_args!("disconnected"))
            } else if crate::blelog::advertising() {
                match since {
                    Some(s) => { let s = s / 1000; core::fmt::write(&mut state, format_args!("advertising, disc. {:02}:{:02} ago", s / 60, s % 60)) }
                    None => core::fmt::write(&mut state, format_args!("advertising (20-30 ms)")),
                }
            } else {
                core::fmt::write(&mut state, format_args!("starting"))
            };
            let state = core::str::from_utf8(&state.b[..state.n]).unwrap_or("?");
            let mut lines = [Line::EMPTY; ROWS];
            lines[0] = Line::from_fmt(format_args!("{}", name.as_str()));
            lines[1] = Line::from_fmt(format_args!("BLE {state}"));
            lines[2] = Line::from_fmt(format_args!("uptime {:02}:{:02}:{:02}", up / 3600, up / 60 % 60, up % 60));
            if let Some([tl, tr, bl, br]) = super::grid_values() {
                // 2×2 grid of last piezo ADC values (TL TR / BL BR).
                lines[GRID_ROW] = Line::from_fmt(format_args!("TL:{tl:<5}  TR:{tr:<5}"));
                lines[GRID_ROW + 1] = Line::from_fmt(format_args!("BL:{bl:<5}  BR:{br:<5}"));
            }
            lines[SEP_ROW] = Line::from_fmt(format_args!("{:-<40}", ""));
            let n = crate::blelog::recent(&mut rec, &mut lens);
            for i in 0..n {
                let s = core::str::from_utf8(&rec[i][..lens[i]]).unwrap_or("?");
                lines[LOG_ROW + i] = Line::from_fmt(format_args!("{s}"));
            }
            let mut k = 0;
            let total = crate::status::latest(&mut |_t, _synced, body| {
                // Top-level `"key":value` pairs of the latest sample → `key=value` rows.
                // Called with the latest sample, then the last full reading: skip its repeated uptime_ms.
                let again = k > 0;
                for field in split_top(body) {
                    if k == BUF_KEYS { break; }
                    let (key, v) = field.split_once(':').unwrap_or((field, ""));
                    if again && key.trim_matches('"') == "uptime_ms" { continue; }
                    lines[BUF_ROW + 1 + k] = Line::from_fmt(format_args!("{}={v}", key.trim_matches('"')));
                    k += 1;
                }
            });
            let sync = if crate::status::synced_time().is_some() { "synced" } else { "unsynced" };
            lines[BUF_ROW] = Line::from_fmt(format_args!("-- latest sample ({total} pending, {sync}) {:-<40}", ""));
            // y layout: name@4, BLE/uptime@30/44, grid@58/72, sep@86, logs from 98, status after logs.
            const Y_NAME: i32 = 4;
            const Y_HDR: i32 = 30;
            const Y_GRID: i32 = 58;
            const Y_SEP: i32 = 86;
            const Y_LOG: i32 = 98;
            for (i, l) in lines.iter().enumerate() {
                if shown[i] == *l { continue; }
                shown[i] = *l;
                let r = match i {
                    0 => Text::with_baseline(l.padded(COLS_BIG), Point::new(0, Y_NAME), big, Baseline::Top).draw(&mut lcd),
                    1 | 2 => Text::with_baseline(l.padded(COLS_SMALL), Point::new(0, Y_HDR + 14 * (i as i32 - 1)), small, Baseline::Top).draw(&mut lcd),
                    i if i == GRID_ROW || i == GRID_ROW + 1 => {
                        Text::with_baseline(l.padded(COLS_SMALL), Point::new(0, Y_GRID + 14 * (i as i32 - GRID_ROW as i32)), small, Baseline::Top).draw(&mut lcd)
                    }
                    i if i == SEP_ROW => Text::with_baseline(l.padded(COLS_SMALL), Point::new(0, Y_SEP), dim, Baseline::Top).draw(&mut lcd),
                    i if i >= BUF_ROW => Text::with_baseline(l.padded(COLS_SMALL), Point::new(0, Y_LOG + 12 * (i as i32 - LOG_ROW as i32) + 4), if i == BUF_ROW { dim } else { small }, Baseline::Top).draw(&mut lcd),
                    _ => Text::with_baseline(l.padded(COLS_SMALL), Point::new(0, Y_LOG + 12 * (i as i32 - LOG_ROW as i32)), dim, Baseline::Top).draw(&mut lcd),
                };
                if r.is_err() { return Err("draw"); }
            }
            esp_radio_rtos_driver::usleep(1_000_000);
        }
    }
}

#[cfg(not(all(feature = "display", feature = "hal", feature = "esp32s3")))]
mod imp {
    pub fn start(_project: &'static str) -> Result<(), &'static str> {
        Err("display not supported in this build (needs feature `display`, hal, esp32s3)")
    }
}

/// Start the status screen thread (see the module docs). `Err` when unsupported in this build.
pub fn start(project: &'static str) -> Result<(), &'static str> { imp::start(project) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn lock_grid() -> std::sync::MutexGuard<'static, ()> {
        static G: Mutex<()> = Mutex::new(());
        G.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_grid() {
        for slot in GRID_VALUES.iter() {
            slot.store(0, Ordering::Relaxed);
        }
        GRID_ON.store(false, Ordering::Release);
    }

    #[test]
    fn grid_values_none_until_set() {
        let _g = lock_grid();
        reset_grid();
        assert_eq!(grid_values(), None);
        set_grid_values([111, 222, 333, 444]);
        assert_eq!(grid_values(), Some([111, 222, 333, 444]));
    }

    #[test]
    fn set_grid_values_overwrites() {
        let _g = lock_grid();
        reset_grid();
        set_grid_values([1, 2, 3, 4]);
        set_grid_values([10, 20, 30, 40]);
        assert_eq!(grid_values(), Some([10, 20, 30, 40]));
    }

    #[test]
    fn start_unsupported_without_display_hal_s3() {
        // Host / non-display builds use the stub `imp`.
        assert!(start("sensor").is_err());
    }
}
