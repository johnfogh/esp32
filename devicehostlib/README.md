# devicehostlib

Host-side device stack for `target-editor`: OS-paired BlueZ GATT (`central`),
`devices.json`, virtual Hub/Board sensors, in-process/Unix attach, and status-buffer protocol.

Origin: https://github.com/johnfogh/esp32-devicehostlib.git

```toml
devicehostlib = { path = "../devicehostlib" }
```

## Modules

- `central` — BlueZ-paired/connected GATT host
- `devices` — `~/.config/esp32-controller/devices.json`
- `linked` — session `Linked` / `LastMsg` + telemetry ingest
- `sensor` — virtual Hub/Board
- `local` — in-process attach + Unix socket
- `protocol` — status buffer + command dispatch
- `roster` — runtime roster/sock paths