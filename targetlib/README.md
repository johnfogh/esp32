# targetlib

Rust library for `target_def` JSON files (zones over a target image).

```toml
targetlib = { path = "../targetlib" }
```

`dir()` is `~/dev/esp32/target_defs`.
`open_target_def(path)` / `save_target_def(path, doc)` read and write a `target_def` JSON file.
`save_target_def` pretty-prints with a trailing newline, appends `.json` if needed, creates the
parent folder, and embeds `image` as base64 when the field is still a relative path.
`load(stem)` / `zone_names(stem)` read `<dir>/<stem>.json`.
`Target::new(doc, display_px)` converts the file's cm size to pixels-per-cm from the
displayed image size. `Target::from_cm(doc)` is the hub/ADC convention: one display unit
equals one centimetre (same scale `resolve_hits` uses). `target.simulate_hit(x, y, f, decay)`
returns `[tl, tr, bl, br]`: `(x, y)` is cm from the image top-left; each reading is
`max(0, f - decay * pixel_distance)` on that displayed image. `target.locate_hit([tl, tr, bl, br])`
inverts that with `f = 4095` and `decay = (4095-2000)/diagonal`. `hit_zone(doc, x, y)` is
the smallest-area visible zone in cm. `create_target_overlay(doc, x, y)` returns a transparent PNG (native
image size) with a 5 px white circle and 2 px arrows toward the nearest plate
edge(s), plus that edge distance in mm. `target_to_json(target)` pretty-prints
`target.doc` with a trailing newline (display pixel fields are omitted).

## JSON format

All values are cm. Points are measured from the image's top-left corner: x along Length,
y along Width. `display_units` only remembers the UI unit. Pixel size is not stored; it
comes from the image as displayed.

```json
{
  "image": "<base64 of the PNG or JPEG bytes>",
  "length_cm": 60.96,
  "width_cm": 45.72,
  "display_units": "in",
  "zones": [
    { "name": "bullseye", "color": 0, "points": [[28.1, 20.3], [32.9, 20.3], [32.9, 25.4], [28.1, 25.4]] }
  ]
}
```

`image` is the standard-alphabet base64 encoding of the image file (no `data:` prefix). Older files
that store a relative path are still loaded; the next save embeds the bytes.
