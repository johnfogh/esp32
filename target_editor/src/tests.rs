//! Controller state tests (no GPUI).
use std::path::PathBuf;
use std::time::Instant;

use utils::host::Chip;

use devicehostlib::central::{BleEvent, DeviceInfo};
use crate::ctrl::{App, Cmd, Focus, Msg, Tab, LOG_CAP};
use devicehostlib::Linked;
use crate::key::{Code, Key};
use crate::theme::Theme;

fn app() -> App {
    let mut a = App::new(
        vec![PathBuf::from("/x/laser"), PathBuf::from("/x/sensor")],
        Theme::default(),
    );
    a.handle_msg(Msg::Boards(vec![
        ("/dev/ttyUSB0".into(), Chip::Esp32),
        ("/dev/ttyACM0".into(), Chip::Esp32c3),
    ]));
    a
}
fn key(c: Code) -> Key {
    Key::new(c)
}

#[test]
fn focus_cycling() {
    let mut a = app();
    let order: Vec<Focus> = (0..2)
        .map(|_| {
            a.handle_key(key(Code::Tab));
            a.focus
        })
        .collect();
    assert_eq!(order, [Focus::Projects, Focus::Chips]);
    a.handle_key(key(Code::BackTab));
    assert_eq!(a.focus, Focus::Projects);
}

#[test]
fn chip_navigation_skips_disabled() {
    let mut a = app();
    assert_eq!((a.chip, a.chip_cursor), (Some(0), 0));
    a.handle_key(key(Code::Down));
    assert_eq!(a.chip_cursor, 3);
    a.handle_key(key(Code::Down));
    assert_eq!(a.chip_cursor, 3);
    a.handle_key(key(Code::Enter));
    assert_eq!(a.chip, Some(3));
    a.handle_key(key(Code::Up));
    assert_eq!(a.chip_cursor, 0);
    a.chip_cursor = 1;
    a.handle_key(key(Code::Char(' ')));
    assert_eq!(a.chip, Some(3));
}

#[test]
fn projects_and_hotkeys() {
    let mut a = app();
    a.focus = Focus::Projects;
    a.handle_key(key(Code::Down));
    a.handle_key(key(Code::Down));
    assert_eq!(a.project_cursor, 1);
    a.handle_key(key(Code::Enter));
    assert_eq!(a.project, Some(1));
    assert_eq!(a.handle_key(key(Code::Char('b'))), Some(Cmd::Build));
    assert_eq!(a.handle_key(key(Code::Char('f'))), Some(Cmd::Flash));
    assert_eq!(a.handle_key(key(Code::Char('r'))), Some(Cmd::Refresh));
    a.start_job("Build");
    assert_eq!(a.status(), "Build running…");
    assert_eq!(a.handle_key(key(Code::Char('b'))), None);
    assert_eq!(a.handle_key(key(Code::Char('f'))), None);
    assert_eq!(a.handle_key(key(Code::Char('r'))), None);
    a.handle_msg(Msg::JobDone(Some(101)));
    assert!(a.status().starts_with("Failed: exit code 101"));
    assert_eq!(a.handle_key(key(Code::Char('q'))), Some(Cmd::Quit));
    assert_eq!(a.handle_key(key(Code::Esc)), Some(Cmd::Quit));
}

#[test]
fn no_boards_means_no_build() {
    let mut a = App::new(vec![PathBuf::from("/x/laser")], Theme::default());
    a.start_scan();
    assert_eq!(a.usb_status(), "Scanning…");
    a.handle_msg(Msg::Boards(vec![]));
    assert_eq!(a.chip, None);
    assert!(!a.can_run());
    assert_eq!(a.handle_key(key(Code::Char('b'))), None);
    assert_eq!(a.handle_key(key(Code::Char('r'))), Some(Cmd::Refresh));
}

#[test]
fn output_scrolling() {
    let mut a = app();
    a.output_height = 10;
    for i in 0..100 {
        a.handle_msg(Msg::Line(format!("line {i}")));
    }
    a.handle_key(key(Code::PageUp));
    assert!(a.scroll.is_some());
    a.handle_key(key(Code::PageDown));
    a.handle_key(key(Code::PageDown));
    assert_eq!(a.scroll, None);
}

fn sample_device() -> DeviceInfo {
    DeviceInfo {
        addr: "aa:bb:cc:dd:ee:ff".into(),
        name: "laser-aabb".into(),
        project: "laser".into(),
        mac: "aabbccddeeff".into(),
        rssi: Some(-42),
    }
}

fn linked(since: Option<Instant>, enabled: bool) -> Linked {
    let mut l = Linked::new(sample_device(), since, None);
    l.enabled = enabled;
    l
}

#[test]
fn manager_focus_and_rows() {
    let mut a = app();
    a.tab = Tab::Manager;
    a.focus = Focus::Devices;
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), true),
    );
    a.log.push_back("hello log".into());
    assert_eq!(a.mgr_rows().len(), 1);
    a.handle_key(key(Code::Tab));
    assert_eq!(a.focus, Focus::Devices);
}

#[test]
fn log_drops_oldest_at_cap() {
    let mut a = app();
    for i in 0..(LOG_CAP + 2) {
        a.log(format!("n{i}"));
    }
    assert_eq!(a.log.len(), LOG_CAP);
    let front = a.log.front().unwrap();
    assert!(front.contains("n2"), "{front}");
    let back = a.log.back().unwrap();
    assert!(back.contains(&format!("n{}", LOG_CAP + 1)), "{back}");
}

#[test]
fn output_log_is_unfiltered() {
    let mut a = app();
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), true),
    );
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"buffer seq 1: {"1":{"uptime_ms":10},"2":{"temp":41.2}}"#.into(),
    )));
    let line = a.log.back().unwrap();
    assert!(
        line.contains(r#"buffer seq 1: {"1":{"uptime_ms":10},"2":{"temp":41.2}}"#),
        "{line}"
    );
    assert!(!line.contains('\n'), "{line}");
    assert_eq!(
        a.log.iter().filter(|l| l.contains("buffer seq")).count(),
        1
    );
}

#[test]
fn buffer_line_updates_telemetry() {
    let mut a = app();
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), true),
    );
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"buffer seq 1: {"1":{"uptime_ms":10},"2":{"temp":41.2,"battery":{"v":3.91,"pct":80},"accel":{"x":0.0,"y":0.1,"z":0.98}}}"#.into(),
    )));
    let l = a.connections.get("aa:bb:cc:dd:ee:ff").unwrap();
    assert_eq!(l.uptime_ms.map(|(u, _)| u), Some(10));
    assert_eq!(l.temp, Some(41.2));
    assert_eq!(l.battery, Some((3.91, 80)));
    assert_eq!(l.accel, Some((0.0, 0.1, 0.98)));
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"get_accelerometers: {"x":1.0,"y":2.0,"z":3.0}"#.into(),
    )));
    assert_eq!(
        a.connections.get("aa:bb:cc:dd:ee:ff").unwrap().accel,
        Some((1.0, 2.0, 3.0))
    );
}

#[test]
fn disconnect_keeps_enabled() {
    let mut a = app();
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), true),
    );
    a.handle_msg(Msg::Ble(BleEvent::Disconnected("aa:bb:cc:dd:ee:ff".into())));
    assert_eq!(a.mgr_rows().len(), 1);
    assert!(a.conn("aa:bb:cc:dd:ee:ff").unwrap().enabled);
    assert!(a.conn("aa:bb:cc:dd:ee:ff").unwrap().since.is_none());
}

#[test]
fn unpaired_drops_row() {
    let mut a = app();
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), true),
    );
    a.handle_msg(Msg::Ble(BleEvent::Removed("aa:bb:cc:dd:ee:ff".into())));
    assert!(a.connections.is_empty());
    assert_eq!(a.mgr_rows().len(), 0);
}

#[test]
fn ignores_disabled_device_lines() {
    let mut a = app();
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), false),
    );
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"buffer seq 1: {"1":{"uptime_ms":10,"temp":41.2}}"#.into(),
    )));
    let l = a.connections.get("aa:bb:cc:dd:ee:ff").unwrap();
    assert!(l.temp.is_none());
    assert!(a.last_msg.is_empty());
    assert_eq!(a.toggle_enabled("aa:bb:cc:dd:ee:ff"), Some(true));
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"buffer seq 1: {"1":{"uptime_ms":10,"temp":41.2}}"#.into(),
    )));
    assert_eq!(a.connections.get("aa:bb:cc:dd:ee:ff").unwrap().temp, Some(41.2));
}

#[test]
fn buffer_line_uses_numeric_newest_key() {
    let mut a = app();
    a.connections.insert(
        "aa:bb:cc:dd:ee:ff".into(),
        linked(Some(Instant::now()), true),
    );
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"buffer seq 1: {"999":{"uptime_ms":999,"temp":1.0,"sensors":[9,9,9,9]},"1000":{"uptime_ms":1000,"temp":42.0,"sensors":[1,2,3,4]}}"#.into(),
    )));
    let l = a.connections.get("aa:bb:cc:dd:ee:ff").unwrap();
    assert_eq!(l.temp, Some(42.0));
    assert_eq!(l.sensors, Some([1, 2, 3, 4]));
    assert_eq!(
        a.last_msg_at(["aa:bb:cc:dd:ee:ff"]).map(|m| m.ts.as_str()),
        Some("1000")
    );
    a.handle_msg(Msg::Ble(BleEvent::DeviceLine(
        "aa:bb:cc:dd:ee:ff".into(),
        r#"buffer seq 1: {"500":{"uptime_ms":500,"temp":1.0,"sensors":[0,0,0,0]}}"#.into(),
    )));
    let l = a.connections.get("aa:bb:cc:dd:ee:ff").unwrap();
    assert_eq!(l.temp, Some(42.0));
    assert_eq!(l.sensors, Some([1, 2, 3, 4]));
    assert_eq!(
        a.last_msg_at(["aa:bb:cc:dd:ee:ff"]).map(|m| m.ts.as_str()),
        Some("1000")
    );
}

#[test]
fn virtual_discovered_enables_ingest() {
    let mut a = app();
    a.handle_msg(Msg::Ble(BleEvent::Discovered(DeviceInfo {
        addr: "02:00:00:00:00:06".into(),
        name: "sensor-000006".into(),
        project: "sensor".into(),
        mac: "02:00:00:00:00:06".into(),
        rssi: None,
    })));
    assert!(a.conn("02:00:00:00:00:06").unwrap().enabled);
    a.handle_msg(Msg::Ble(BleEvent::Discovered(sample_device())));
    assert!(!a.conn("aa:bb:cc:dd:ee:ff").unwrap().enabled);
}

#[test]
fn course_hit_adc_corner() {
    let v = crate::host::course_hit_adc(10.0, 10.0, (100, 100), 0.0, 0.0);
    assert_eq!(v[0], 4095);
    assert_eq!(v[3], devicehostlib::sensor::THRESHOLD);
    assert_eq!(v[1], v[2]);
    assert!(v[1] > devicehostlib::sensor::THRESHOLD && v[1] < 4095);
}

#[test]
fn course_hit_adc_center() {
    let v = crate::host::course_hit_adc(10.0, 10.0, (100, 100), 50.0, 50.0);
    assert!(v.iter().all(|&x| x >= devicehostlib::sensor::THRESHOLD));
    assert_eq!(v[0], v[1]);
    assert_eq!(v[0], v[2]);
    assert_eq!(v[0], v[3]);
}

#[test]
fn course_hit_adc_anisotropic_matches_from_cm_locate() {
    // Portrait image on a landscape plate — old pixel-decay path misplaced hits.
    let length_cm = 20.0;
    let width_cm = 10.0;
    let img = (100u32, 200u32);
    let (x_px, y_px) = (25.0_f32, 40.0_f32);
    let adc = crate::host::course_hit_adc(length_cm, width_cm, img, x_px, y_px);
    let doc = targetlib::JsonDoc {
        image: String::new(),
        length_cm,
        width_cm,
        display_units: None,
        zones: Vec::new(),
    };
    let plate = targetlib::Target::from_cm(doc);
    let (x, y) = plate
        .locate_hit(adc.map(|v| Some(v as f32)))
        .expect("locate");
    let x_cm = x_px * length_cm as f32 / img.0 as f32;
    let y_cm = y_px * width_cm as f32 / img.1 as f32;
    assert!((x - x_cm).abs() < 0.05, "{x} vs {x_cm}");
    assert!((y - y_cm).abs() < 0.05, "{y} vs {y_cm}");
}
