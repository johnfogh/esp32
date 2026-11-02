# esp32-utils

Shared Rust helpers for ESP32-family firmware. One API, two frameworks:

| framework feature | runtime | crates |
|---|---|---|
| `hal` | `no_std`, bare metal | `esp-hal` 1.2, `esp-println`, `esp-backtrace`, `esp-bootloader-esp-idf`, `esp-storage` |
| `idf` | `std` on ESP-IDF | `esp-idf-svc` 0.53 (re-exports `esp-idf-hal` 0.47) |

| `host` | `std`, desktop (x86_64 etc.) | none: chip table + build/flash command helpers for tooling such as esp32-controller |

Chip features (exactly one, firmware only): `esp32`, `esp32s2`, `esp32s3` (Xtensa) and `esp32c3`, `esp32c6`, `esp32h2` (RISC-V).
Selecting none or more than one chip, or not exactly one framework, fails with `compile_error!`.
`host` takes **no** chip feature (and rejects one, and rejects `hal`/`idf`).
(The `idf` backend is written for esp32/s2/s3/c3/c6; esp32h2 + idf builds but is untested.)

```toml
[dependencies]
utils = { package = "esp32-utils", path = "../utils", features = ["esp32c3", "hal"] }
```

## API

```rust
let board = utils::init();                 // logger + chip setup, returns Board { peripherals }
log::info!("{}", utils::chip_info());      // ChipInfo { model, cores, flash_size, mac }
utils::time::delay_ms(500); utils::time::uptime_ms();
let mut led = utils::gpio::Output::new(pin);           // set_high / set_low / set
```

Pins are the framework's own types
(`board.peripherals.GPIO2` with `hal`, `board.peripherals.pins.gpio2` with `idf`).
`init()` installs the logger (esp-println + `log` on hal, `EspLogger` on idf).
On `hal`, `utils` also provides the panic handler and the ESP-IDF app descriptor, so binaries need nothing else.

### ADC (`utils::adc`, hal / idf / host)

```rust
if utils::adc::is_adc_pin(2) { let raw: u16 = utils::adc::read(2); }  // read() panics on a non-ADC pin
utils::adc::try_read(2);        // Option<u16>
utils::adc::channel(2);         // ADC1 channel, Option<u8>; utils::adc::PINS = (gpio, channel) map
```

Raw, uncalibrated one-shot samples (12-bit, ~11/12 dB attenuation). **ADC1 only**: ADC2 conflicts with
Wi-Fi/BT on ESP32/S2/S3 and is unreliable on the C3, so its pins are rejected.
hal: steals `ADC1` + the GPIO and builds an `esp_hal` `Adc` per read; idf: raw `adc_oneshot_*` ADC1 unit
(created lazily, mutex-guarded). Don't also use the framework's own ADC1 driver in the same program.

| chip | ADC1 GPIOs (channel 0..) |
|---|---|
| esp32 | 36, 37, 38, 39, 32, 33, 34, 35 |
| esp32s2 / esp32s3 | 1–10 |
| esp32c3 | 0–4 |
| esp32c6 | 0–6 |
| esp32h2 | 1–5 |

On `host`, `utils::adc::pins_for(chip)` gives the map and `utils::adc::mock::{set_chip, set, reset}`
drives `is_adc_pin`/`read` in tests (per thread, default chip esp32c3).

### BLE manager announce (`utils::bluetooth_announce`, feature `ble`)

```rust
let manager: utils::Manager = utils::bluetooth_announce("laser")?;   // Result<Manager, utils::BleError>, blocks
manager.conn_handle(); manager.peer_address(); manager.disconnect();
```

Advertises (connectable) the 128-bit service `utils::ble::SERVICE_UUID` = `c0de5e00-6d67-4e2a-9b1f-3a5d7e9c0b01`,
manufacturer data `0xFFFF` (LE) + the 6-byte base MAC, and, in the scan response (the advert is full at 31 bytes), the Complete Local Name `<project>-xxyyzz`
(project truncated to 22 bytes + last 3 MAC bytes, e.g. `laser-a1b2c3`). Constants, `device_name()`, `parse_name()` and
`parse_manufacturer_mac()` live in `utils::ble_proto`, which is compiled with every feature (incl. `host`) for desktop tools.
OS pairing is Just Works (no PIN). A central must write `manager-device` to characteristic
`c0de5e01-6d67-4e2a-9b1f-3a5d7e9c0b01` within 5 s unless the link is already bonded/encrypted.
Unbonded centrals that miss the token are disconnected and advertising resumes. Waits forever;
returns the first authenticated connection.

- `hal`: esp-radio 1.0.0-beta.1 + trouble-host 0.7 (`ExternalController`, `security` + `legacy-pairing`), run with `embassy_futures::block_on`.
  It starts esp-rtos itself and adds a 96 KiB esp-alloc global heap (128 KiB on esp32s3), and steals `TIMG0`, `FROM_CPU_INTR0`, `BT`:
  don't use those, start esp-rtos or define a global allocator in the app. After return the host stack is not polled
  (the link stays up at controller level). `build.rs` supplies 3 ESP32 ROM symbols missing from esp-rom-sys 0.1.5.
- `idf`: esp32-nimble 0.13. The consumer's `sdkconfig.defaults` needs `CONFIG_BT_ENABLED=y`,
  `CONFIG_BT_BLUEDROID_ENABLED=n`, `CONFIG_BT_NIMBLE_ENABLED=y` (see esp32-sensor / esp32-laser).
- esp32s2 (no Bluetooth) or `ble` off: `Err(BleError::Unsupported)`. A second call: `Err(BleError::AlreadyStarted)`.

#### Non-blocking announce + board output (`bluetooth_announce_background`, `ble_log`)

```rust
match utils::bluetooth_announce_background(PROJECT) {           // returns immediately
    Ok(()) => {}
    Err(e) => log::warn!("ble: not announcing: {e}"),           // e.g. Unsupported on esp32s2
}
```

Runs the announce in its own thread (hal: an esp-rtos thread, 16 KiB stack; idf: a std thread) and keeps
serving the authenticated manager; when it disconnects the device advertises again. While a manager is
connected, buffered output is sent as notifications on the log characteristic `ble_proto::LOG_CHAR_UUID` =
`c0de5e02-6d67-4e2a-9b1f-3a5d7e9c0b01` in chunks of `LOG_CHUNK` (20) bytes; lines end with `\n`, the central
reassembles them. With `ble`, `utils::init()` installs a logger that prints to the console as before **and**
mirrors every `log` record (`<LEVEL> <target> - <msg>`) into that buffer; `utils::ble_log(line)` queues a line
directly. Queuing never blocks: a 4 KiB ring buffer, lines are dropped when no manager is connected or it is
full. (hal: the mirroring logger replaces esp-println's, honouring the `ESP_LOG` level but not per-target filters.)

The GATT service also has a read characteristic `ble_proto::STATUS_CHAR_UUID` (`c0de5e03-…`): 9 bytes =
`uptime_ms` (u64 LE, at read time) ++ connection RSSI measured by the board (i8, 127 = unknown); see
`encode_status` / `parse_status`. The controller uses it for the signal and the time offset.

hal threading: the background announce runs on an esp-rtos **embassy executor** in its own thread at priority
0 (same as main), so it sleeps until woken instead of spinning; once the scheduler runs, `utils::time::delay_ms`
sleeps (`esp_radio_rtos_driver::usleep`) so other threads get the CPU. (The blocking `bluetooth_announce` still
uses `block_on` in the caller's thread.)

## Host feature

```toml
utils = { package = "esp32-utils", path = "../utils", features = ["host"] }
```

```rust
use utils::host::{Chip, Framework, build_command, flash_command, elf_path, list_serial_ports, list_projects, detect_boards, parse_board_info};
for c in Chip::ALL { c.name(); c.feature(); c.arch(); c.target_triple(Framework::Hal); }
let mut cmd = build_command(dir, Chip::Esp32c3, Framework::Hal); // bash -c '. ~/export-esp.sh && ... cargo build --release --target ... --features esp32c3,hal'
let elf = elf_path(dir, Chip::Esp32c3, Framework::Hal);          // <dir>/target/<triple>/release/<package name>
let mut f = flash_command(dir, Chip::Esp32c3, Framework::Hal, Some("/dev/ttyUSB0")); // espflash flash --non-interactive --chip ... <elf>
list_serial_ports();                                              // /dev/ttyUSB*, /dev/ttyACM*
detect_boards(10);    // Vec<(port, Chip)> via `timeout 10 espflash board-info --non-interactive --port <p>`, in parallel
parse_board_info(out); Chip::from_name("ESP32-C3");
```

The commands return `std::process::Command`s that clear `RUSTUP_TOOLCHAIN`/`CARGO_*` overrides, so each project's own
`rust-toolchain.toml` (`esp`) applies. `build_shell`/`flash_shell` give the same command as text.
Unit tests (the default target here is Xtensa, so pass the host target explicitly):

```sh
cargo +stable test   --target x86_64-unknown-linux-gnu --features host
cargo +stable clippy --target x86_64-unknown-linux-gnu --features host --all-targets
```

## Toolchain

```sh
cargo install cargo-binstall && cargo binstall espup espflash ldproxy
espup install                  # Xtensa "esp" toolchain, writes ~/export-esp.sh
. ~/export-esp.sh              # in every shell
rustup target add riscv32imc-unknown-none-elf riscv32imac-unknown-none-elf
```

`rust-toolchain.toml` pins the `esp` channel. It is used for every chip; RISC-V targets build their
`core`/`alloc` from source (`[unstable] build-std` in `.cargo/config.toml`), so the same toolchain covers everything.
The `idf` builds additionally need `python3` (with venv), `git`, and network access: on first build `esp-idf-sys`
downloads ESP-IDF and its tools (~1 GB) into `.embuild/`.

## Build (library check)

```sh
cargo build-esp32   cargo build-s2   cargo build-s3      # Xtensa + hal
cargo build-c3      cargo build-c6   cargo build-h2      # RISC-V + hal
cargo build-c3-idf  cargo build-esp32-idf  cargo build-c6-idf
cargo clippy-esp32  cargo clippy-c3
```

### Status screen (`utils::display::start`, feature `display`)

For the **Waveshare ESP32-S3-Touch-LCD-2** (ST7789T3 240x320 IPS, SPI). hal + esp32s3 only; on other builds
(incl. idf) `start` returns `Err` (documented no-op; idf support not implemented). Implies `ble`.

```rust
#[cfg(feature = "display")]
if let Err(e) = utils::display::start(PROJECT) { log::warn!("display: {e}"); }
```

Pins (from Waveshare's ESP32-S3-Touch-LCD-2-Demo, `EXAMPLE_PIN_NUM_LCD_*`): SCLK 39, MOSI 38, MISO 40, DC 42,
CS 45, RST none (software reset), backlight 1 (driven high, no PWM). Init as the demo: colour inversion on,
RGB order, no offsets, portrait. SPI2 at 40 MHz via mipidsi 0.10 + embedded-graphics 0.8.

An esp-rtos thread (priority 0) redraws about once a second: `<project>-xxyyzz`, BLE state
(starting / advertising / connected), uptime and the last 10 log lines (from the `ble` logger mirror). Only
changed lines are redrawn and the thread sleeps between frames. It owns SPI2 and GPIO 1/38/39/40/42/45.
Other on-board pins: I2C 47/48 (CST816D touch + QMI8658, not used), SD CS 41, battery ADC 5, camera socket
2,4,6–17,21. With `display`, the esp32s3 ADC1 map excludes GPIO1 (backlight) and GPIO5 (battery divider).


## IMU (`imu` feature, esp32s3 hal)

`get_accelerometers(unit, count, interval_ms)` reads the onboard QMI8658 (I2C0, SDA 48, SCL 47, address
0x6B, 400 kHz, ±4 g) and returns `{"x":..,"y":..,"z":..}` or a JSON array when `count > 1` (max 100).
Units: `g` (default), `m/s²`, `raw`. It sleeps `interval_ms` between samples. Other chips/builds (incl. idf)
return an error.

## BLE commands

Characteristic `c0de5e04-…` (write, manager connection only) takes `\n`-terminated JSON, sent in ≤20-byte
chunks: `{"action":"get_accelerometers","params":["g",1,"00:00.000"]}`. A worker thread executes it. The
result is a log notify line `<action>: <json>` or `<action>: error: <msg>`, and a status-buffer sample
`{"method":"<name>","result":"<text>","error":bool}` (`set_time` / `buffer_ack` / `read_buffer` stay silent).
Methods: `ping`, `get_accelerometers`. Add more in `src/commands.rs` (`dispatch`), or from the firmware
with `utils::commands::set_handler`.

## Status buffer (`utils::status`)

- `buffer_add_sample(time, &[(key, &value), …])` stores one sample (one main-loop pass) as
  `"<ms>": {"key": value, …}`; `time = None` uses the controller-synced clock (ms since epoch), or the
  uptime plus `"synced": false` inside the sample before the first `set_time`. Values: numbers, bools,
  `&str`, or `status::Json("{…}")`; fields that fail to serialize are omitted. `buffer_add(key, v)` is a
  one-field sample. Caps: ~16 KB / 1000 samples (S3, IDF; ~8 KB / 500 on other hal chips), oldest samples dropped.
- `send_buffer()` (every main-loop pass, non-blocking): while a manager is connected, at most once per
  second all unsent samples go out as a new snapshot `{"seq":N,"buffer":{"<ms>":{…},…}}` (oldest first,
  ≤ ~3 KB, own u16 `seq`) without waiting for earlier acks. Up to 16 unacked snapshots are kept; one
  unacked 1 s after
  its last frame is resent with the same `seq`, and all unacked ones are resent after a reconnect. When
  the byte/sample cap is hit the oldest data goes first (unacked snapshots, then unsent samples).
- Frames on notify characteristic `c0de5e05-…`: `[seq & 0xff, flags, ≤18 B payload]` (flags bit 7 =
  last, bit 6 = first, bits 0–5 = index mod 64); snapshots are queued back-to-back, each one contiguous.
- The manager acks in batches: `{"action":"buffer_ack","params":[seq,seq,…]}` (≤ 32 per command).
- `{"action":"set_time","params":[ms]}`: offset = controller_time − uptime; `synced_time()` = uptime + offset.
  `set_time`/`buffer_ack` run inline in the BLE write path (silent, never queued behind other commands).
- hal (with `ble`, not esp32s2) and idf. Elsewhere all calls are no-ops.

Heap (hal + ble): 96 KiB, 128 KiB on esp32s3 (main DRAM; the reclaimed dram2 region is not used, it overlaps ROM/BLE data). The BLE
controller allocates 30 KiB at init; if the heap is short it aborts with `BLE assert emi.c 164, param
00000000 00007800`. The free heap is logged as `ble: heap free … bytes before controller init`.

Status screen: below the log lines it shows the status buffer: `-- status buffer: N pending, synced|unsynced`
and up to 8 keys (newest first) as `key=latest (entries)`, read under the buffer lock without allocating.

## Standard readings (`utils::status::add_standard_readings`)

Adds one sample per call, i.e. every main-loop pass (keyed by synced time; a key <= the previous one is bumped to previous+1 ms so keys stay unique) with only `uptime_ms`. Since 0.11.0 `send_buffer`, right before it creates a new snapshot (~1/s), adds one full sample (`add_full_reading`, keyed by the current time, so it is the last entry of that snapshot) with `uptime_ms` plus: `battery()` `{"v","pct"}` on
esp32s3+`display` (GPIO5/ADC1 ch4, 1:3 divider per Waveshare demos 04/06_lvgl_battery,
`V = raw/4095 × 3.1 V × 3`, uncalibrated; pct linear 3.3 V→0 %, 4.2 V→100 %; on USB without battery it
reads the charger output); `temp` °C (hal: esp32c3/c6 via esp-hal `tsens`; esp32s3 via direct SENS
register access per ESP-IDF temperature_sensor_ll, default range, `0.4386·raw − 20.52`; not on hal S2/esp32; idf: all chips but esp32 via the IDF temperature_sensor driver); `accel` `{x,y,z}` g
(hal + esp32s3 + `imu`). Unavailable readings are skipped; no-op where there is no status buffer.

Advertising: interval 20–30 ms (`ble_proto::ADV_INTERVAL_MIN_MS`/`MAX_MS`; hal trouble
`AdvertisementParameters`, idf NimBLE 32/48 × 0.625 ms), restarted immediately after any disconnect.

Status screen BLE line: `starting`, `advertising (20-30 ms)` / `advertising, disc. MM:SS ago`,
`disconnected` (~1.5 s after a manager disconnect), `connected AA:BB:CC:DD:EE:FF -NN dBm` (manager address
and link RSSI, refreshed every ~2 s; `blelog::peer()`, `rssi()`, `last_disconnect_ms()`).
