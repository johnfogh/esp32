//! Main Section demo: Stages-like target grid + horizontal event timeline.
use reportslib::{Report, TargetCard, TimelineEvent};

fn main() -> std::io::Result<()> {
    // 1x1 gray PNG placeholder (stand-in for a hit overlay).
    let png = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0xfe, 0xd4, 0xef, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    let cards = [
        TargetCard::new()
            .def("ipsc")
            .image_png(png)
            .zones(["A", "C", "D"])
            .at(1, 1),
        TargetCard::new()
            .def("mini")
            .image_png(png)
            .zones(["A", "B"])
            .at(1, 2),
        TargetCard::new().distance("10 yd").at(2, 1),
        TargetCard::new()
            .def("popper")
            .image_png(png)
            .zones(["hit"])
            .at(2, 2),
    ];

    let events = [
        TimelineEvent::log(0.0, "Start"),
        TimelineEvent::message(2.4, "Ready").detail(r#"{"status":"ready"}"#),
        TimelineEvent::log(5.1, "Hit T1"),
        TimelineEvent::log(7.8, "Hit T2"),
        TimelineEvent::message(11.0, "Score").detail(r#"{"hits":4}"#),
        TimelineEvent::log(14.2, "Stop"),
    ];

    let events_json = r#"{
  "events": [
    { "t": 0.0, "kind": "log", "label": "Start" },
    { "t": 2.4, "kind": "message", "label": "Ready", "detail": { "status": "ready" } },
    { "t": 5.1, "kind": "log", "label": "Hit T1" },
    { "t": 7.8, "kind": "log", "label": "Hit T2" },
    { "t": 11.0, "kind": "message", "label": "Score", "detail": { "hits": 4 } },
    { "t": 14.2, "kind": "log", "label": "Stop" }
  ]
}"#;

    Report::new("Classifier — Alice")
        .meta("Shooter", "Alice")
        .meta("Stage", "Classifier")
        .meta("Generated", "2026-10-18 14:30:00")
        .main_section(Some(2), Some(2), cards, events)
        .data_section(events_json)
        .write_to("main_section.html")?;

    println!("wrote main_section.html");
    Ok(())
}
