# esp32-sensor

Firmware binary built on [esp32-utils](https://github.com/johnfogh/esp32-utils) (expected at `../utils`).
It initialises the board, logs the chip info and then logs a tick every second.

Default: **esp32 + esp-hal (no_std)**. Chip and framework are features forwarded to utils:
`esp32 esp32s2 esp32s3 esp32c3 esp32c6 esp32h2` × `hal | idf`.

## Setup

```sh
cargo binstall espup espflash ldproxy && espup install
rustup target add riscv32imc-unknown-none-elf riscv32imac-unknown-none-elf
. ~/export-esp.sh
git clone https://github.com/johnfogh/esp32-utils ../utils
```

## Build / flash

```sh
cargo build                  # esp32 (Xtensa) + hal
cargo run                    # flash + monitor via "espflash flash --monitor"
cargo build-s3               # other Xtensa chips: build-s2, build-s3
cargo build-c3               # RISC-V: build-c3, build-c6, build-h2
cargo run-c3                 # flash an ESP32-C3
cargo build-c3-idf           # std/ESP-IDF build (downloads ESP-IDF on first run)
cargo clippy-esp32; cargo clippy-c3
```

Use the aliases rather than plain `--features`, because they also switch `--target` and disable the
default `esp32,hal` features. RISC-V chips use the same pinned `esp` toolchain (rust-toolchain.toml),
building `core` from source; you could instead use `cargo +nightly build-c3`. The idf aliases add
`-Zbuild-std=std,panic_abort`.

## Sensors (`src/sensors.rs`)

Four corner piezos **TL, TR, BL, BR** on ADC1. Default GPIOs (change `PIEZO_PINS` to rewire):

| chip    | TL | TR | BL | BR |
|---------|----|----|----|----|
| esp32   | 36 | 39 | 34 | 35 |
| esp32s2 |  1 |  2 |  3 |  4 |
| esp32s3 |  2 |  3 |  4 |  6 |
| esp32c3 |  0 |  1 |  2 |  3 |
| esp32c6 |  0 |  1 |  2 |  3 |
| esp32h2 |  1 |  2 |  3 |  4 |

Each loop pass (~50 ms) reads all four. A sample is stored only if **all** corners are `>=` the threshold (default 2000). Otherwise the pass is skipped. Hits go in a hashmap keyed by synced time (ms), value `[tl,tr,bl,br]`. The map keeps the last 10 s (max 64 entries). `read_buffer` serializes as `{"<name>":[[ms,tl,tr,bl,br],…]}`.

BLE (after `utils::commands::set_handler` in `app`):

```json
{"action":"set_threshold","params":[2000]}
{"action":"set_name","params":["front"]}
{"action":"read_buffer","params":[]}
```

Replies: `set_threshold: 2000`, `set_name: front`, and `read_buffer: {"name":[[ms,tl,tr,bl,br],…]}` (default name is the advertised BLE name `sensor-xxyyzz`; empty `{"name":[]}`). `set_name` is RAM-only (ASCII `A–Z a–z 0–9 _ -`, 1..=32 bytes) and does not change the BLE advertised name.

```rust
use esp32_sensor::sensors::{init_sensors, scan_sensors};
init_sensors(&[0, 1, 2, 3])?;       // up to 4 ADC1 pins; Err(TooMany | NotAdcPin(p)), nothing stored on error
let v = scan_sensors(2000);         // Sample [tl,tr,bl,br] only when all channels >= 2000
```

Logic is in `SensorArray` / `HitBuffer` (validator/reader injected). Host tests:

```sh
cargo +stable test --target x86_64-unknown-linux-gnu --no-default-features --features host --lib
```

Extra idf aliases: `build-s2-idf`, `build-s3-idf`, `build-h2-idf`.

## BLE

`app()` calls `utils::bluetooth_announce_background(PROJECT)` (non-blocking; wraps
`utils::bluetooth_announce`), advertising as `sensor-xxyyzz` for the esp32-controller Connection Manager while the
main loop keeps running. Pair in the OS with Just Works (`bluetoothctl pair`, no PIN), then connect.
Once the manager authenticates, all `log` output is mirrored to it over BLE (Device Connections cards).
On esp32s2 (no Bluetooth) it logs a warning and continues.

## Display (esp32s3)

The `esp32s3` feature implies `display`, so every S3 build (the `build-s3` alias and the controller's
`--features esp32s3,hal` build) shows the utils status screen on the Waveshare ESP32-S3-Touch-LCD-2: device
name, BLE state, uptime, last log lines. Other chips are unaffected. On esp32s3 GPIO1 and GPIO5 are not accepted as sensor pins (backlight / battery divider).

## IMU (esp32s3)

`esp32s3` also implies `imu`: the manager can send `{"action":"get_accelerometers","params":[unit,count,interval]}`
to the BLE command characteristic and gets `get_accelerometers: {"x":..,"y":..,"z":..}` back as a log line.

## Status buffer

Each loop pass (1 s) calls `utils::status::add_standard_readings()` then `utils::send_buffer()`:
one sample per pass, keyed by the synced time, with only `uptime_ms`; since 0.10.0 each ~1 s snapshot also ends with one full sample
`{uptime_ms, battery{v,pct}, temp, accel{x,y,z}}` polled right before the snapshot (unavailable readings skipped); failed readings are
omitted. Device Connections cards show the newest sample; the S3 screen
shows the latest sample. No tick log line any more.

Since 0.9.1 the main loop runs every 50 ms (`LOOP_MS`), so a 1 s snapshot holds ~20 samples. Backlog while disconnected: ~13 s with the 16 KB cap (S3), ~6.5 s with 8 KB (other hal chips); older samples are dropped. Snapshots are `{"seq":N,"buffer":{"<ms>":{"uptime_ms":..},…,"<ms>":{"uptime_ms":..,"battery":{..},"temp":..,"accel":{..}}}}`. The display shows the latest uptime plus the last full readings. A sample is ~60 B, so a 1 s snapshot is ~1.2 KB and the buffer holds much longer backlogs. Since 0.8.2 the board records a sample every loop pass and sends at most one new snapshot per second
(all unsent samples, ≤ ~3 KB; no waiting for acks); unacked snapshots are resent after 1 s and the controller acks them in batches (see utils README).
