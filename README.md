# ESP32 shooting-range monorepo

Master project for ESP32 range firmware and host-side libraries/apps that work with it.

Sub-projects live as sibling directories under this root. Path dependencies use `../<crate>`. Defaults such as `~/dev/esp32/target_defs` point at this tree.

## Layout

| Path | Role | Toolchain |
|------|------|-----------|
| **Firmware** | | |
| [`utils/`](utils/) | Shared board helpers (`esp32-utils`): init, GPIO, ADC, BLE, display, IMU; `hal` / `idf` / `host` features | `esp` |
| [`sensor/`](sensor/) | Piezo hit-sensor firmware (`esp32-sensor`) | `esp` |
| [`laser/`](laser/) | Laser-output firmware (`esp32-laser`) | `esp` |
| **Host apps** | | |
| [`designer/`](designer/) | GPUI design / timeline experiment | `stable` |
| **Host libraries** | | |
| [`devicehostlib/`](devicehostlib/) | BlueZ GATT, `devices.json`, virtual sensors | `stable` |
| [`targetlib/`](targetlib/) | Load / save / query `target_def` JSON | `stable` |
| [`reportslib/`](reportslib/) | Self-contained HTML session reports | `stable` |
| **Data** | | |
| [`target_defs/`](target_defs/) | Target definition JSON (zones + image) | — |
| [`esp32_board_versions.txt`](esp32_board_versions.txt) | Chip / board capability notes | — |

```
  devicehostlib ──► utils (host)
  targetlib
  reportslib
  designer (standalone GPUI app)

  sensor ──► utils (hal|idf)     laser ──► utils (hal|idf)
```

## Build notes

- **Firmware** (`sensor`, `laser`, and `utils` without `host`): use the `esp` toolchain in each crate’s `rust-toolchain.toml`. Build from that crate directory (`cd sensor && cargo build`). Prefer the crate’s cargo aliases (`build-s3`, `build-c3`, …) so `--target` and features stay aligned.
- **Host** (`designer` and the `*lib` crates): `stable`. Build from the crate directory, e.g. `cd designer && cargo run`.
- There is **no root Cargo workspace**. Mixing `esp` and `stable` members under one workspace fights toolchains; keep builds per crate.
- First-time ESP setup is documented in [`utils/README.md`](utils/README.md) and the firmware READMEs (`espup`, `espflash`, `export-esp.sh`).

## Config / runtime paths

| What | Where |
|------|--------|
| Target definitions | `~/dev/esp32/target_defs` |
| Devices / controller state | `~/.config/esp32-controller/`, `~/.local/state/esp32-controller/` |
