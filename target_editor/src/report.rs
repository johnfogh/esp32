//! Stage run HTML reports via `reportslib`.
use std::path::{Path, PathBuf};

use reportslib::Report;

use crate::ui_course::CourseTarget;

const MARK_R: i32 = 5;

/// Write `{reports_dir}/{shooter}/{stage}/{YYYYMMDD_HHMM}.html`.
pub fn write_stage_report(
    reports_dir: &Path,
    shooter: &str,
    stage: &str,
    events: &serde_json::Map<String, serde_json::Value>,
    targets: &[CourseTarget],
) -> Result<PathBuf, String> {
    let shooter_seg = sanitize_seg(shooter);
    let stage_seg = sanitize_seg(stage);
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M").to_string();
    let dir = reports_dir.join(&shooter_seg).join(&stage_seg);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{stamp}.html"));

    let generated = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut report = Report::new(format!("{stage_seg} — {shooter_seg}"))
        .meta("Shooter", &shooter_seg)
        .meta("Stage", &stage_seg)
        .meta("Generated", &generated);

    report = report.heading(2, "Targets");
    let rows: Vec<[String; 4]> = targets
        .iter()
        .map(|t| {
            [
                t.name.clone(),
                t.def.clone().unwrap_or_default(),
                t.hub_label
                    .clone()
                    .or_else(|| t.hub.clone())
                    .unwrap_or_default(),
                t.hits.len().to_string(),
            ]
        })
        .collect();
    report = report.table(["Name", "Definition", "Hub", "Hits"], rows);

    for t in targets {
        if t.hits.is_empty() && t.def.is_none() {
            continue;
        }
        report = report.heading(3, t.name.clone());
        if !t.hits.is_empty() {
            let items: Vec<String> = t
                .hits
                .iter()
                .enumerate()
                .map(|(i, (x, y))| format!("{i}: ({x:.1}, {y:.1})"))
                .collect();
            report = report.list(true, items);
        }
        if let Some(png) = target_overlay_png(t) {
            report = report.image_png(png, format!("{} hits", t.name));
        }
    }

    report = report.heading(2, "Events");
    if events.is_empty() {
        report = report.paragraph("No events.");
    } else {
        for (ts, v) in events {
            report = report.heading(3, format!("Message {ts}"));
            let items: Vec<&serde_json::Value> = match v {
                serde_json::Value::Array(a) => a.iter().collect(),
                other => vec![other],
            };
            let mut logs = Vec::new();
            for item in items {
                match item {
                    serde_json::Value::String(line) => logs.push(line.clone()),
                    other => {
                        if !logs.is_empty() {
                            report = report.list(false, std::mem::take(&mut logs));
                        }
                        let body =
                            serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string());
                        report = report.paragraph(body);
                    }
                }
            }
            if !logs.is_empty() {
                report = report.list(false, logs);
            }
        }
    }

    report
        .write_to(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn sanitize_seg(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "unknown".into();
    }
    let mut out = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

fn target_overlay_png(t: &CourseTarget) -> Option<Vec<u8>> {
    let stem = t.def.as_deref()?;
    let path = crate::targets::path_for(stem);
    let text = std::fs::read_to_string(&path).ok()?;
    let doc: targetlib::JsonDoc = serde_json::from_str(&text).ok()?;
    let json_dir = path.parent().unwrap_or(Path::new("."));
    let (bytes, _) = targetlib::load_image_field(&doc.image, json_dir).ok()?;
    let mut img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let red = image::Rgba([220, 38, 38, 255]);
    let white = image::Rgba([255, 255, 255, 255]);
    for &(x, y) in &t.hits {
        let cx = x.round() as i32;
        let cy = y.round() as i32;
        draw_disk(&mut img, cx, cy, MARK_R, red);
        draw_circle_stroke(&mut img, cx, cy, MARK_R, white);
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            image::ImageFormat::Png,
        )
        .ok()?;
    Some(png)
}

fn put_px(img: &mut image::RgbaImage, x: i32, y: i32, c: image::Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn draw_disk(img: &mut image::RgbaImage, cx: i32, cy: i32, r: i32, c: image::Rgba<u8>) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                put_px(img, cx + dx, cy + dy, c);
            }
        }
    }
}

fn draw_circle_stroke(img: &mut image::RgbaImage, cx: i32, cy: i32, r: i32, c: image::Rgba<u8>) {
    let mut x = r;
    let mut y = 0;
    let mut err = 1 - r;
    while x >= y {
        for (px, py) in [
            (cx + x, cy + y),
            (cx + y, cy + x),
            (cx - y, cy + x),
            (cx - x, cy + y),
            (cx - x, cy - y),
            (cx - y, cy - x),
            (cx + y, cy - x),
            (cx + x, cy - y),
        ] {
            put_px(img, px, py, c);
        }
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}
