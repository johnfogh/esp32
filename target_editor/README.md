# target-editor

Standalone GPUI host for ESP32 shooting-range firmware. Menu:

**Device Connections | Stages | Roster | Target Editor | Flash Device**

Ctrl+Tab / Ctrl+Shift+Tab cycles those plus Virtual. Target definitions come from
`~/.config/target-editor/config.json` (`targets_folder`). Stages load from the
`default_stages_directory` setting (default `~/dev/esp32/course_defs`). Device config and logs
stay under `esp32-controller` (`~/.config/esp32-controller/`, `~/.local/state/esp32-controller/events.log`).

```
target-editor [--virtual] [--manager] [--flash] [--adapter hciN] [IMAGE|JSON]
target-editor --build <project> <chip>
target-editor --manager --headless [--adapter hciN] [--connect <name|mac>]...
```

Default is **Edit**. `--virtual` starts Virtual (Hub immediately). `--manager` starts
Device Connections (OS-paired or OS-connected boards + in-process extras). `--flash` starts Device Flash. A file argument opens in Edit.

Host BLE, virtual sensors, and `devices.json` live in sibling crate `devicehostlib`
(`../devicehostlib`, https://github.com/johnfogh/esp32-devicehostlib.git).

The former `esp32-controller` TUI lives in these modes (GPUI). Unlink `~/.local/bin/esp32-controller`
after install.

Colours follow the current Omarchy theme (`~/.config/omarchy/current/theme` or
`~/.local/state/omarchy/current/theme`, `colors.toml` then `alacritty.toml`), live-reloaded.

## Flash

Device Flash: pick a connected chip and a firmware project (`~/dev/esp32`, skips `utils`,
`target_editor`, `scriptlib`, `targetlib`, `devicehostlib`, `devicelib`, and `reportslib`). Output fills the remaining window. `b` build, `f` flash, `r` refresh USB. PgUp/PgDn scrolls output.
`--build <project> <chip>` runs the same build with no window.

## Manager

Device Connections lists BlueZ-paired boards that look like ours (name `<project>-xxyyzz`, service
UUID, or manufacturer MAC), and matching boards the OS already has connected. Pairing is Just
Works (no PIN): Settings / `bluetoothctl pair` then connect, or `bluetoothctl connect` alone. The host
opens GATT only while BlueZ already has the device connected. It does not scan, pair, or
connect/disconnect.

`●`/`○` is an ingest filter (persisted in `devices.json`). Disabled physical rows stay listed; their
messages are ignored. Space or the dot toggles that filter. Virtual Hub extras attach in-process,
appear only while connected (status `connected HH:MM:SS`), and ○ removes the extra and its card.
`--adapter hciN` pins which adapter’s paired list to watch.

## Roster

Menu **Roster**. Editable Name | Email table for today's file
`~/Documents/target_editor/rosters/roster_YYMMDD.json` (created empty if missing; JSON array of
`{"name","email"}` objects). Add / Delete rows; click a cell to edit. Saves on commit, add, and
delete. Old string-array files are not migrated.

## Stages

Menu **Stages**. Left Events sidebar order: Shooter Name (today's roster, session pick),
Stage (`.lua` from setting `default_stages_directory`,
default `~/dev/esp32/course_defs`), Simulate, then Start and Stop. Selecting a stage
runs `init()`. `define_grid(rows, columns)` creates `"col,row"` cards; `define_distance(distance, row, col)` stores a string on a cell; `define_target("Target1", "ipsc"[, row, col])` names a card and loads `<targets_folder>/ipsc.json`. Start calls
`main()` (not `init`) and injects cards with a non-empty def as Lua `global_targets` (`target_name`, `target_def`, `zones`, `:active`; empty grid and distance cells are omitted). Stop aborts the run. `message()` JSON
objects and arrays show as cards in Events (same grid layout as Device Connections); other log lines stay as rows.

Target cards shrink to content height. Editable name, full-width image (aspect kept), definition
popup, sensor-hub popup (each unassigned card gets the next unused hub; list has no stripes; focused row uses theme
cursor; the pick is session-only and kept across Stages re-enter / script re-init, not written to Lua or JSON). Virtual Hub Sensor boards stay until Hub is toggled off or an extra is ○-removed, plus
connected Device Connections rows whose PROJECT is `sensor`. Zone list sorted A–Z. Stages
starts the virtual hub. Add still creates a blank card. Cards are session-only.
With **Simulate** on and a stage running, a click on a card preview records the zone for
`wait_for_hit`, pulses the assigned virtual hub’s four corner sensors (max force at the click;
ADC decay in plate cm, same model as `resolve_hits`), and marks the point with a 5px red dot
(1px white stroke) until Stop.

When a stage run ends (Stop or `main()` finishes), an HTML report is written under
`reports_path` (setting, default `~/Documents/target_editor_reports`) as
`{shooter}/{stage}/{YYYYMMDD_HHMM}.html` via `reportslib` (events + targets/hits).

App settings (gear): `virtual_bullet_size`, `default_virtual_sensor_hubs` (extras created with the
virtual hub on first use; factory hub stays on), `reports_path`, `default_stages_directory`.

## Edit

| Input | Action |
|---|---|
| Ctrl+O | Open image or .json (file dialog; falls back to a path field if no portal) |
| Ctrl+S | Save (requires Length and Width > 0). First save of a document opens a Save As dialog in the image's folder with `<image stem>.json` suggested; after that (or after opening a .json) it saves silently to the same file |
| Ctrl+Shift+S | Save As: always opens the dialog. `.json` is appended if missing; falls back to a typed path field if no dialog/portal is available |
| Add | Side panel: new 20×20 px rectangle at the image centre, named `Unamed-N` |
| Click a handle | Select that zone and drag the handle |
| Double-click a handle | Delete that handle (min 3) |
| Drag fill or edge | Move the whole zone |
| Double-click an edge | Insert a draggable handle at the hit |
| Click empty image | Deselect |
| Esc | Deselect the handle, then the zone |
| Delete / Backspace | Delete the selected handle (min 3 points) |
| Delete (no handle selected) | Delete the selected zone |
| Side panel | Length / Width / Unit table. Zones table is front-to-back (top row is the top layer); drag a row to reorder. Click a row to select, click it again to deselect. Double-click a name to rename. Click the swatch to cycle colour, click Score to type an integer. Right-click a row: Show/Hide, Duplicate, Delete. Name and Score fields select-all on edit (black background, white text). Below the list, TL/TR/BL/BR are pixel distances from the selected zone’s bounding-box corners to the canvas mouse |

Zones use a 10-colour gold palette (`#ffe169` … `#76520e`), filled at 75% opacity (25% transparent)
with a 1px `#777` stroke. Handles (5×5 px, 1px `#777` stroke) show only on the selected zone. A 5×5 px plus with a 1px
red stroke marks the image centre and cannot be edited.
A new zone takes the least-used slot (ties: lowest index) and is named `Unamed-N` until renamed.
The colour is saved as `"color": <index>` (wraps `% 10`). Click the swatch to cycle. Score is an
integer (`"score"`, default 0), right-aligned in the list. `"hidden": true` omits the zone from
the canvas and Virtual zone chips. Duplicate offsets points by 8px. The right sidebar is 25% of the
window width. UI chrome still follows the Omarchy theme.

Length = horizontal (image width direction), Width = vertical. The title and status bar show
`●` when there are unsaved changes.

## Virtual

In-process sensors and hub. No host advertise or virtual HCI. Toolbar **Sensors** and **Hub** are
independent (default both on). Click a zone to fire the mapped board or hub channel. Zone chips
cycle S1–S4, H1–H4, and — (skipping a kind that is off). Keys `1`–`4` hit S1–S4 when Sensors is on,
otherwise hub channels H1–H4. The process Hub starts the first time you enter Virtual (or at launch
with `--virtual`) and stays running if you switch back to Edit.

Factory MACs:

- Hub `02:00:00:00:00:01` → `sensor-000001`
- S1 `02:00:00:00:00:02` → `sensor-000002`
- S2 `02:00:00:00:00:03` → `sensor-000003`
- S3 `02:00:00:00:00:04` → `sensor-000004`
- S4 `02:00:00:00:00:05` → `sensor-000005`

Each card shows a status dot: accent while a local manager link is up. Temp, voltage, and accel are
read-only telemetry. Standalone Hit is an accel impulse. Hub H1–H4 analog values rest at `0`; a hit
pulses `3000` for 250 ms (threshold `2000`) and stores above-threshold corners in a 10 s buffer, same
as firmware `esp32-sensor`. Device Connections extras attach in-process, ack buffer snapshots, and show the newest sample key. The Unix
roster/socket is still published. Firmware GATT (`utils::ble_proto`) is unchanged for physical
boards the OS has paired. A sample is stored only when all four corners are `>=` threshold.
`process_targets()` polls every card’s SENSOR HUB, resolves new samples, and logs `{"type":"target_hit","ms":…,"target":…,"zone":…,"sensors":[TL,TR,BL,BR]}` (including `"Miss"`) to the Events sidebar. Hit cards show last-5-digit `ms`, target, and zone as unlabeled columns; consecutive same-second hits share one card.
Controller Lua `wait_for_hit` is host-local and does not complete from these hits.

## JSON format

Target definition files are documented in [`targetlib`](../targetlib).

## Build

gpui 0.2.2 needs the bundled `patches/xattr` (`[patch.crates-io]`; libc no longer exports
`ENOATTR` on Linux).

```
cargo install --path . && ln -sfn ~/.cargo/bin/target-editor ~/.local/bin/target-editor
```

The old `virtual-target` and `esp32-controller` binaries are gone; unlink
`~/.local/bin/virtual-target` and `~/.local/bin/esp32-controller` if they are still there.

Lua: Stages files in `default_stages_directory` (`../scriptlib/docs/scripting_guide.md`). The runtime is the `scriptlib` crate.

## Changelog

0.5.0: merge esp32-controller as Flash / Manager / Course / Edit (GPUI). In-process virtual
connect. `target-editor --build` / `--manager --headless`. Archive johnfogh/esp32-controller.
