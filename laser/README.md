# esp32-laser

Firmware binary built on [esp32-utils](https://github.com/johnfogh/esp32-utils) (expected at `../utils`).
It initialises the board, logs the chip info and then logs a tick every second.

Default: **esp32 + esp-hal (no_std)**. Chip and framework are features forwarded to utils:
`esp32 esp32s2 esp32s3 esp32c3 esp32c6 esp32h2` × `hal | idf`.

## Laser pins

`src/lasers.rs` maps laser `i` (L1..Ln in the controller's Signals table) to GPIO `LASER_PINS[i]`
(active high, push-pull, start low). Edit the list to rewire; any length works. Defaults avoid
strapping, flash/PSRAM, USB-JTAG and UART0 pins:

| chip | L1 | L2 | L3 | L4 |
|------|----|----|----|----|
| esp32 | 25 | 26 | 27 | 32 |
| esp32s2 / s3 / c3 | 4 | 5 | 6 | 7 |
| esp32c6 | 18 | 19 | 20 | 21 |
| esp32h2 | 10 | 11 | 12 | 22 |

`unsafe { Lasers::new() }` takes the pins (hal and idf); `set_pattern(&[bool])` and `set(i, on)`
drive them. Not yet called from the main loop.

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

## BLE

`app()` calls `utils::bluetooth_announce_background(PROJECT)` (non-blocking; wraps
`utils::bluetooth_announce`), advertising as `laser-xxyyzz` for the esp32-controller Connection Manager while the
main loop keeps running. Pair in the OS with Just Works (`bluetoothctl pair`, no PIN), then connect.
Once the manager authenticates, all `log` output is mirrored to it over BLE (Device Connections cards).
On esp32s2 (no Bluetooth) it logs a warning and continues.

## Display (esp32s3)

The `esp32s3` feature implies `display`, so every S3 build (the `build-s3` alias and the controller's
`--features esp32s3,hal` build) shows the utils status screen on the Waveshare ESP32-S3-Touch-LCD-2: device
name, BLE state, uptime, last log lines. Other chips are unaffected. On esp32s3 `LASER_PINS` is `[4, 18, 6, 7]` (GPIO5 is the board's battery divider).

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
