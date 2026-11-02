use gpui::{div, prelude::*, px, rgb};

use crate::chrome;
use crate::ctrl::Focus;
use crate::host::Host;
use devicehostlib::sensor::Kind;

fn fmt_dur(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn dash() -> String {
    "—".into()
}

fn sensor_dash() -> String {
    "-".into()
}

fn fmt_sensor(v: &serde_json::Value) -> String {
    if let Some(n) = v.as_u64() {
        return n.to_string();
    }
    if let Some(n) = v.as_i64() {
        return n.to_string();
    }
    if let Some(n) = v.as_f64() {
        if n.fract() == 0.0 {
            return (n as i64).to_string();
        }
        return n.to_string();
    }
    sensor_dash()
}

fn ms_str(ms: Option<u64>) -> String {
    ms.map(|m| m.to_string()).unwrap_or_else(dash)
}

struct Row {
    virt_mac: Option<String>,
    addr: Option<String>,
    enabled: bool,
    stale: bool,
    name: String,
    connected: bool,
    busy: bool,
    status: String,
    project: String,
    temp: String,
    battery: String,
    accel: [String; 3],
    sensors: [String; 4],
    uptime_ms: String,
}

fn rows(host: &Host) -> Vec<Row> {
    let a = &host.ctrl;
    let mut out = Vec::new();
    let virt_macs = host.virtual_macs();
    if !host.virtual_boards().is_empty() {
        for b in host.virtual_boards() {
            let s = b.snapshot();
            let linked = a.connections.values().find(|l| {
                l.info.mac.eq_ignore_ascii_case(&s.mac)
                    || s.adapter_addr.as_deref().is_some_and(|ad| l.info.addr.eq_ignore_ascii_case(ad))
            });
            let enabled = linked.is_some_and(|l| l.enabled);
            let since = linked.and_then(|l| l.since);
            let connected = since.is_some() || s.link.is_connected();
            if !connected {
                continue;
            }
            let busy = false;
            let status = match since {
                Some(t) => format!("connected {}", fmt_dur(t.elapsed())),
                None => "connected".into(),
            };
            let (x, y, z) = (
                format!("{:.2}", s.accel[0]),
                format!("{:.2}", s.accel[1]),
                format!("{:.2}", s.accel[2]),
            );
            let sensors = if s.kind == Kind::Hub {
                s.sensors.map(|v| v.to_string())
            } else {
                [sensor_dash(), sensor_dash(), sensor_dash(), sensor_dash()]
            };
            out.push(Row {
                virt_mac: Some(s.mac.clone()),
                addr: linked.map(|l| l.info.addr.clone()),
                enabled,
                stale: false,
                name: s.name,
                connected,
                busy,
                status,
                project: match s.kind {
                    Kind::Hub => devicehostlib::sensor::PROJECT.into(),
                    Kind::Standalone => "Sensor".into(),
                },
                temp: format!("{:.1} °C", s.temp),
                battery: format!("{:.2} V {}%", s.battery_v, s.battery_pct),
                accel: [x, y, z],
                sensors,
                uptime_ms: s.uptime_ms.to_string(),
            });
        }
    }
    let snap: Vec<(devicehostlib::central::DeviceInfo, Option<devicehostlib::Linked>)> = a
        .mgr_rows()
        .into_iter()
        .map(|(info, l)| (info.clone(), l.cloned()))
        .collect();
    for (info, linked) in snap.iter() {
        if virt_macs.contains(&info.mac.to_ascii_uppercase()) {
            continue;
        }
        let busy = false;
        let connected = linked.as_ref().is_some_and(|l| l.since.is_some());
        let name = a.display_name(info);
        let status: String = if connected {
            format!("connected {}", fmt_dur(linked.as_ref().unwrap().since.unwrap().elapsed()))
        } else {
            "paired".into()
        };
        let temp = linked
            .as_ref()
            .and_then(|l| l.temp)
            .map(|c| format!("{c:.1} °C"))
            .unwrap_or_else(dash);
        let bat = linked
            .as_ref()
            .and_then(|l| l.battery)
            .map(|(v, p)| format!("{v:.2} V {p}%"))
            .unwrap_or_else(dash);
        let accel = match linked.as_ref().and_then(|l| l.accel) {
            Some((x, y, z)) => [format!("{x:.2}"), format!("{y:.2}"), format!("{z:.2}")],
            None => [dash(), dash(), dash()],
        };
        let sensors = match linked.as_ref().and_then(|l| l.sensors) {
            Some(s) => s.map(|v| v.to_string()),
            None => [sensor_dash(), sensor_dash(), sensor_dash(), sensor_dash()],
        };
        let stale = linked.as_ref().is_some_and(|l| l.since.is_none());
        out.push(Row {
            virt_mac: None,
            addr: Some(info.addr.clone()),
            enabled: linked.as_ref().is_some_and(|l| l.enabled),
            stale,
            name,
            connected,
            busy,
            status,
            project: info.project.clone(),
            temp,
            battery: bat,
            accel,
            sensors,
            uptime_ms: ms_str(linked.as_ref().and_then(|l| l.uptime_ms.map(|(ms, _)| ms))),
        });
    }
    out
}

fn action_btn(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    enabled: bool,
    f: impl Fn(&mut Host, &mut gpui::Context<Host>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.surface))
        .text_color(rgb(if enabled { t.foreground } else { t.disabled }))
        .hover(|s| {
            if enabled {
                s.border_color(rgb(t.accent))
            } else {
                s
            }
        })
        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            if enabled {
                f(this, cx);
                cx.notify();
            }
        }))
        .child(label.into())
}

pub fn view(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let a = &host.ctrl;
    let list = rows(host);
    let n = list.len();
    let title = format!("Devices ({n})");
    let cards = device_cards(host, cx, &list);
    let half = px(host.line_px() / 2.0);
    let pad = || div().h(half).w_full();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .child(pad())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .px_1()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(rgb(t.muted))
                        .child(a.ble_status.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(div().text_color(rgb(t.muted)).child("Virtual Sensors:"))
                        .child(action_btn(host, cx, "virt-hub", "Hub Sensor", true, |this, cx| {
                            this.add_virtual(Kind::Hub, cx);
                        }))
                        .child(action_btn(host, cx, "virt-sensor", "Sensor", true, |this, cx| {
                            this.add_virtual(Kind::Standalone, cx);
                        })),
                ),
        )
        .child(pad())
        .child(chrome::panel(
            t,
            &title,
            a.focus == Focus::Devices,
            div()
                .id("mgr-msgs")
                .flex()
                .flex_row()
                .flex_wrap()
                .overflow_y_scroll()
                .children(cards),
        ))
}

fn sample_version(v: &serde_json::Value) -> Option<String> {
    v.get("project_version")
        .or_else(|| v.get("version"))
        .and_then(|x| {
            x.as_str()
                .map(str::to_string)
                .or_else(|| x.as_u64().map(|n| n.to_string()))
                .or_else(|| x.as_f64().map(|n| n.to_string()))
        })
        .filter(|s| !s.is_empty())
}

fn sample_uptime(v: &serde_json::Value) -> Option<String> {
    v.get("uptime_ms").and_then(|u| {
        u.as_u64()
            .map(|n| n.to_string())
            .or_else(|| u.as_i64().map(|n| n.to_string()))
            .or_else(|| Some(u.to_string()))
    })
}

fn sample_temp(v: &serde_json::Value) -> Option<String> {
    v.get("temp").and_then(|x| x.as_f64()).map(|t| format!("{t:.1} °C"))
}

fn sample_battery(v: &serde_json::Value) -> Option<String> {
    let b = v.get("battery")?;
    let volts = b.get("v").and_then(|x| x.as_f64())?;
    let pct = b.get("pct").and_then(|x| x.as_u64())?;
    Some(format!("{volts:.2} V {pct}%"))
}

fn sample_accel(v: &serde_json::Value) -> Option<[String; 3]> {
    let a = v.get("accel").unwrap_or(v);
    let x = a.get("x").and_then(|x| x.as_f64())?;
    let y = a.get("y").and_then(|x| x.as_f64())?;
    let z = a.get("z").and_then(|x| x.as_f64())?;
    Some([format!("{x:.2}"), format!("{y:.2}"), format!("{z:.2}")])
}

fn sample_sensors(v: &serde_json::Value) -> Option<[String; 4]> {
    let a = v.get("sensors")?.as_array().filter(|a| a.len() == 4)?;
    Some(std::array::from_fn(|i| fmt_sensor(&a[i])))
}



fn device_cards(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    list: &[Row],
) -> Vec<gpui::Stateful<gpui::Div>> {
    let t = host.theme;
    list.iter()
        .enumerate()
        .map(|(i, r)| {
            let virt_mac = r.virt_mac.clone();
            let addr = r.addr.clone();
            let mark = if r.enabled { "●" } else { "○" };
            let last = host
                .ctrl
                .last_msg_at([r.addr.as_deref().unwrap_or(""), r.virt_mac.as_deref().unwrap_or("")]);
            let sample = last.and_then(|m| (!m.sample.is_null()).then_some(&m.sample));
            let msg = last
                .map(|m| if m.ts.is_empty() { dash() } else { m.ts.clone() })
                .unwrap_or_else(dash);
            let version = sample.and_then(|s| sample_version(s)).unwrap_or_else(dash);
            let uptime = sample.and_then(|s| sample_uptime(s)).unwrap_or_else(|| r.uptime_ms.clone());
            let temp = sample.and_then(|s| sample_temp(s)).unwrap_or_else(|| r.temp.clone());
            let battery = sample.and_then(|s| sample_battery(s)).unwrap_or_else(|| r.battery.clone());
            let accel = sample.and_then(|s| sample_accel(s)).unwrap_or_else(|| r.accel.clone());
            let sensors = sample.and_then(|s| sample_sensors(s)).unwrap_or_else(|| r.sensors.clone());
            let fg = if r.stale { t.accent } else { t.foreground };
            let size_px = host.line_px();
            let row = |last: bool, label: bool, vals: Vec<String>| {
                chrome::grid_row(t, fg, last, label, size_px, vals)
            };
            let editing = host.ctrl.dev_edit.as_ref().is_some_and(|e| {
                match (&virt_mac, &e.virt_mac) {
                    (Some(_), Some(_)) => e.virt_mac == virt_mac,
                    (None, None) => addr.is_some() && e.addr == addr,
                    _ => false,
                }
            });
            let name = if editing {
                format!(
                    "{}▏",
                    host.ctrl.dev_edit.as_ref().map(|e| e.buf.as_str()).unwrap_or("")
                )
            } else {
                r.name.clone()
            };
            let mut title = div().flex().flex_row().items_center().gap_2().child(
                div()
                    .id(("en", i as u64))
                    .when(!r.busy, |d| {
                        d.on_click(cx.listener({
                            let virt_mac = virt_mac.clone();
                            let addr = addr.clone();
                            move |this, _: &gpui::ClickEvent, _, cx| {
                                this.toggle_mgr_enabled(virt_mac.clone(), addr.clone(), cx);
                                cx.notify();
                            }
                        }))
                    })
                    .text_color(rgb(if r.busy { t.disabled } else { t.foreground }))
                    .child(mark.to_string()),
            );
            if r.connected {
                title = title.child("✓".to_string());
            }
            let rename_name = r.name.clone();
            title = title.child(
                div()
                    .id(("nm", i as u64))
                    .on_click(cx.listener({
                        let virt_mac = virt_mac.clone();
                        let addr = addr.clone();
                        move |this, _: &gpui::ClickEvent, _, cx| {
                            this.start_mgr_rename(virt_mac.clone(), addr.clone(), rename_name.clone());
                            cx.notify();
                        }
                    }))
                    .child(name),
            );
            if r.busy {
                title = title.child("…".to_string());
            }
            div()
                .id(("mgr", i as u64))
                .flex()
                .flex_col()
                .gap_1()
                .p(px(4.))
                .m(px(2.))
                .min_w(px(286.))
                .rounded_md()
                .border_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.surface))
                .text_color(rgb(fg))
                .child(title)
                .child(
                    div()
                        .border_1()
                        .border_color(rgb(t.border))
                        .flex()
                        .flex_col()
                        .child(row(false, false, vec![r.status.clone()]))
                        .child(row(false, false, vec![r.project.clone(), version]))
                        .child(row(false, true, vec!["Message ID".into(), "Uptime (MS)".into()]))
                        .child(row(false, false, vec![msg, uptime]))
                        .child(row(false, true, vec!["Temp".into(), "Battery".into()]))
                        .child(row(false, false, vec![temp, battery]))
                        .child(row(false, true, vec!["X".into(), "Y".into(), "Z".into()]))
                        .child(row(
                            false,
                            false,
                            vec![accel[0].clone(), accel[1].clone(), accel[2].clone()],
                        ))
                        .child(row(false, true, vec!["Sensors".into()]))
                        .child(row(
                            false,
                            true,
                            vec!["TL".into(), "TR".into(), "BL".into(), "BR".into()],
                        ))
                        .child(row(
                            true,
                            false,
                            vec![
                                sensors[0].clone(),
                                sensors[1].clone(),
                                sensors[2].clone(),
                                sensors[3].clone(),
                            ],
                        )),
                )
        })
        .collect()
}
