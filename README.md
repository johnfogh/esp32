# ESP32 shooting-range monorepo

Master project for ESP32 range firmware and the desktop host that builds, flashes, and runs stages against it.

Sub-projects live as sibling directories under this root. Path dependencies use `../<crate>`. Defaults such as `~/dev/esp32/course_defs` and `~/dev/esp32/target_defs` point at this tree.

## Layout

| Path | Role | Toolchain |
|------|------|-----------|
| **Firmware** | | |
| [`utils/`](utils/) | Shared board helpers (`esp32-utils`): init, GPIO, ADC, BLE, display, IMU; `hal` / `idf` / `host` features | `esp` |
| [`sensor/`](sensor/) | Piezo hit-sensor firmware (`esp32-sensor`) | `esp` |
| [`laser/`](laser/) | Laser-output firmware (`esp32-laser`) | `esp` |
| **Host apps** | | |
| [`target_editor/`](target_editor/) | GPUI host: devices, stages, roster, target editor, flash | `stable` |
| [`designer/`](designer/) | GPUI design / timeline experiment | `stable` |
| **Host libraries** | | |
| [`devicehostlib/`](devicehostlib/) | BlueZ GATT, `devices.json`, virtual sensors | `stable` |
| [`scriptlib/`](scriptlib/) | Lua 5.4 Stages runtime for `target-editor` | `stable` |
| [`targetlib/`](targetlib/) | Load / save / query `target_def` JSON | `stable` |
| [`reportslib/`](reportslib/) | Self-contained HTML session reports | `stable` |
| **Data** | | |
| [`course_defs/`](course_defs/) | Stage / course Lua scripts | — |
| [`target_defs/`](target_defs/) | Target definition JSON (zones + image) | — |
| [`esp32_board_versions.txt`](esp32_board_versions.txt) | Chip / board capability notes | — |

```
                    ┌─────────────────┐
                    │  target_editor  │
                    └────────┬────────┘
         ┌───────────┬───────┼───────┬──────────┐
         ▼           ▼       ▼       ▼          ▼
  devicehostlib  scriptlib targetlib reportslib  utils (host)
         │           │
         │           ▼
         │       targetlib
         ▼
      utils (host)

  sensor ──► utils (hal|idf)     laser ──► utils (hal|idf)
  designer (standalone GPUI app)
```

## Build notes

- **Firmware** (`sensor`, `laser`, and `utils` without `host`): use the `esp` toolchain in each crate’s `rust-toolchain.toml`. Build from that crate directory (`cd sensor && cargo build`). Prefer the crate’s cargo aliases (`build-s3`, `build-c3`, …) so `--target` and features stay aligned.
- **Host** (`target_editor` and the `*lib` crates): `stable`. Build from the crate directory, e.g. `cd target_editor && cargo run`.
- There is **no root Cargo workspace**. Mixing `esp` and `stable` members under one workspace fights toolchains; keep builds per crate.
- First-time ESP setup is documented in [`utils/README.md`](utils/README.md) and the firmware READMEs (`espup`, `espflash`, `export-esp.sh`).

## Config / runtime paths

| What | Where |
|------|--------|
| Target definitions | `~/.config/target-editor/config.json` → `targets_folder`, else `~/dev/esp32/target_defs` |
| Stages | `default_stages_directory` (default `~/dev/esp32/course_defs`) |
| Devices / controller state | `~/.config/esp32-controller/`, `~/.local/state/esp32-controller/` |

## Flash from the host

Device Flash in `target-editor` lists firmware projects under this root and skips host-only crates (`utils`, `target_editor`, `designer`, `scriptlib`, `targetlib`, `devicehostlib`, `reportslib`).
