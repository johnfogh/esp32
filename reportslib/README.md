# reportslib

Rust library for generating **self-contained** static HTML reports.

Each report is a single `.html` file with inline CSS, inline JavaScript, and
images encoded as `data:` base64 URIs. Library markup does not reference
external files.

```toml
reportslib = { path = "../reportslib" }
```

```rust
use reportslib::Report;

Report::new("Session summary")
    .meta("Generated", "2026-10-18")
    .heading(2, "Overview")
    .paragraph("Hits recorded during the session.")
    .table(["Device", "Hits"], [["sensor-01", "12"]])
    .image_png(png_bytes, "overlay")
    .write_to("report.html")?;
```

### Main Section

Stages-like target grid (definition, image, zones — no name or sensor hub) above a horizontal event timeline:

```rust
use reportslib::{Report, TargetCard, TimelineEvent};

Report::new("Classifier — Alice")
    .main_section(
        Some(2),
        Some(2),
        [
            TargetCard::new().def("ipsc").image_png(png).zones(["A", "C"]).at(1, 1),
            TargetCard::new().distance("10 yd").at(2, 1),
        ],
        [
            TimelineEvent::log(0.0, "Start"),
            TimelineEvent::message(5.1, "Hit").detail("T1"),
            TimelineEvent::log(14.2, "Stop"),
        ],
    )
    .write_to("main_section.html")?;
```

### Data Section

Embed events (or any payload) as raw JSON. The library does not parse or pretty-print; pass already-formatted text:

```rust
Report::new("Classifier — Alice")
    .main_section(/* ... */)
    .data_section(r#"{ "events": [ { "t": 0.0, "label": "Start" } ] }"#)
    .write_to("main_section.html")?;
```

See `examples/basic.rs` and `examples/main_section.rs`.
