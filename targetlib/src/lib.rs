//! Load, save, and query `target_def` JSON files.
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CM_PER_IN: f64 = 2.54;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Cm,
    In,
}

impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Unit::Cm => "cm",
            Unit::In => "in",
        }
    }
    pub fn to_cm(self, v: f64) -> f64 {
        match self {
            Unit::Cm => v,
            Unit::In => v * CM_PER_IN,
        }
    }
    pub fn cm_to(self, v: f64) -> f64 {
        match self {
            Unit::Cm => v,
            Unit::In => v / CM_PER_IN,
        }
    }
}

/// A zone with points in image pixel coordinates.
#[derive(Clone, Debug)]
pub struct Zone {
    pub name: String,
    pub points: Vec<(f32, f32)>,
    /// Stable palette index, assigned at creation.
    pub color: usize,
    pub score: i32,
    pub hidden: bool,
}

/// Gold palette (index 0..9).
pub const PALETTE: [u32; 10] = [
    0xffe169, 0xfad643, 0xedc531, 0xdbb42c, 0xc9a227, 0xb69121, 0xa47e1b, 0x926c15, 0x805b10,
    0x76520e,
];

/// RGB for a stored colour index (wraps via `% PALETTE.len()`).
pub fn poly_color(i: usize) -> u32 {
    PALETTE[i % PALETTE.len()]
}

/// Colour for a new zone: the least-used slot (ties: lowest index).
pub fn next_color(used: impl IntoIterator<Item = usize>, n: usize) -> usize {
    let n = n.max(1);
    let mut count = vec![0usize; n];
    for c in used {
        count[c % n] += 1;
    }
    let min = *count.iter().min().unwrap_or(&0);
    count.iter().position(|&c| c == min).unwrap_or(0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonZone {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<usize>,
    #[serde(default)]
    pub score: i32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    pub points: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonDoc {
    pub image: String,
    pub length_cm: f64,
    pub width_cm: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_units: Option<Unit>,
    #[serde(alias = "polygons")]
    pub zones: Vec<JsonZone>,
}

/// Loaded `target_def` plus the pixel size of the image as displayed.
///
/// Pixel fields are not stored in the JSON: the same file can be shown at any scale.
/// `new` converts the file's cm length/width to pixels-per-cm from `display_px`.
#[derive(Clone, Debug)]
pub struct Target {
    pub doc: JsonDoc,
    /// Displayed image width in pixels (x / length).
    pub length_px: f32,
    /// Displayed image height in pixels (y / width).
    pub width_px: f32,
    pub px_per_cm_x: f32,
    pub px_per_cm_y: f32,
}

impl Target {
    /// `display_px` is `(length_px, width_px)` of the image on screen.
    pub fn new(doc: JsonDoc, display_px: (f32, f32)) -> Self {
        let length_px = display_px.0;
        let width_px = display_px.1;
        let px_per_cm_x = if doc.length_cm > 0.0 {
            length_px / doc.length_cm as f32
        } else {
            0.0
        };
        let px_per_cm_y = if doc.width_cm > 0.0 {
            width_px / doc.width_cm as f32
        } else {
            0.0
        };
        Self {
            doc,
            length_px,
            width_px,
            px_per_cm_x,
            px_per_cm_y,
        }
    }

    /// Hub / physical plate: one display unit = one centimetre.
    ///
    /// Use this for [`simulate_hit`] / [`locate_hit`] on sensor-hub ADC samples so decay
    /// matches plate geometry in cm (same convention as `resolve_hits`).
    pub fn from_cm(doc: JsonDoc) -> Self {
        let display = (doc.length_cm as f32, doc.width_cm as f32);
        Self::new(doc, display)
    }

    /// Linear decay that maps the plate diagonal to [`ADC_THRESHOLD`].
    pub fn decay(&self) -> f32 {
        let diag = self.length_px.hypot(self.width_px);
        if diag > 0.0 {
            (ADC_MAX - ADC_THRESHOLD) / diag
        } else {
            0.0
        }
    }

    /// Force at the four corner sensors (TL, TR, BL, BR) for a hit at `(x, y)` cm.
    ///
    /// `(x, y)` is the same space as `target_def` points (cm from the image top-left).
    /// Each sensor reads `max(0, f - decay * distance)`, where `distance` is Euclidean
    /// pixels on the displayed image.
    pub fn simulate_hit(&self, x: f32, y: f32, f: f32, decay: f32) -> [f32; 4] {
        let px = x * self.px_per_cm_x;
        let py = y * self.px_per_cm_y;
        let w = self.length_px;
        let h = self.width_px;
        let at = |cx: f32, cy: f32| (f - decay * (px - cx).hypot(py - cy)).max(0.0);
        [at(0.0, 0.0), at(w, 0.0), at(0.0, h), at(w, h)]
    }

    /// Recover a hit in cm from corner ADC readings `[tl, tr, bl, br]`.
    ///
    /// Inverse of [`simulate_hit`] with `f = ADC_MAX` and [`Self::decay`]. Missing or
    /// non-positive readings are unused. Needs at least one left/right pair and one
    /// top/bottom pair.
    pub fn locate_hit(&self, readings: [Option<f32>; 4]) -> Option<(f32, f32)> {
        let decay = self.decay();
        if decay <= 0.0 || self.length_px <= 0.0 || self.width_px <= 0.0 {
            return None;
        }
        let dist = |s: Option<f32>| {
            s.filter(|&v| v > 0.0)
                .map(|v| (ADC_MAX - v).max(0.0) / decay)
        };
        let d_tl = dist(readings[0]);
        let d_tr = dist(readings[1]);
        let d_bl = dist(readings[2]);
        let d_br = dist(readings[3]);
        let x = axis_from_pairs(d_tl, d_tr, d_bl, d_br, self.length_px)?;
        let y = axis_from_pairs(d_tl, d_bl, d_tr, d_br, self.width_px)?;
        let x_cm = if self.px_per_cm_x > 0.0 {
            x / self.px_per_cm_x
        } else {
            0.0
        };
        let y_cm = if self.px_per_cm_y > 0.0 {
            y / self.px_per_cm_y
        } else {
            0.0
        };
        Some((x_cm, y_cm))
    }
}

/// Hub ADC full scale (ESP32 12-bit).
pub const ADC_MAX: f32 = 4095.0;
/// Hub ADC threshold used by [`Target::decay`].
pub const ADC_THRESHOLD: f32 = 2000.0;

fn pair_coord(d0: Option<f32>, d1: Option<f32>, len: f32) -> Option<f32> {
    match (d0, d1) {
        (Some(a), Some(b)) if len > 0.0 => {
            Some(((len * len + a * a - b * b) / (2.0 * len)).clamp(0.0, len))
        }
        _ => None,
    }
}

fn axis_from_pairs(
    d00: Option<f32>,
    d01: Option<f32>,
    d10: Option<f32>,
    d11: Option<f32>,
    len: f32,
) -> Option<f32> {
    match (pair_coord(d00, d01, len), pair_coord(d10, d11, len)) {
        (Some(a), Some(b)) => Some((a + b) * 0.5),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

pub fn image_size_bytes(bytes: &[u8]) -> Result<(u32, u32), String> {
    image::load_from_memory(bytes)
        .map(|i| i.dimensions())
        .map_err(|e| format!("image: {e}"))
}

pub fn encode_image(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Bytes and a display name. `image` is either a relative path (legacy) or standard base64.
pub fn load_image_field(s: &str, json_dir: &Path) -> Result<(Vec<u8>, String), String> {
    let path = json_dir.join(s);
    if s.len() < 4096 && path.is_file() {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        return Ok((bytes, name));
    }
    let bytes = STANDARD
        .decode(s.trim())
        .map_err(|e| format!("image base64: {e}"))?;
    let _ = image_size_bytes(&bytes)?;
    Ok((bytes, "image".into()))
}

const OVERLAY_CIRCLE_R: i32 = 5;
const OVERLAY_ARROW_STROKE: i32 = 2;
const OVERLAY_ARROW_LEN: i32 = 16;
const OVERLAY_ARROW_HEAD: i32 = 6;

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

/// 1 px hollow circle (midpoint algorithm).
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

fn draw_thick_line(
    img: &mut image::RgbaImage,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    thickness: i32,
    c: image::Rgba<u8>,
) {
    let mut x = x0;
    let mut y = y0;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let r = (thickness / 2).max(0);
    loop {
        draw_disk(img, x, y, r, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Arrow from just outside the circle toward a plate edge. `(dx, dy)` is a unit axis step.
fn draw_edge_arrow(
    img: &mut image::RgbaImage,
    cx: i32,
    cy: i32,
    dx: i32,
    dy: i32,
    c: image::Rgba<u8>,
) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    if w <= 0 || h <= 0 {
        return;
    }
    let clamp = |v: i32, max: i32| v.clamp(0, max - 1);
    let gap = OVERLAY_CIRCLE_R + 2;
    let x0 = clamp(cx + dx * gap, w);
    let y0 = clamp(cy + dy * gap, h);
    let x1 = clamp(cx + dx * (gap + OVERLAY_ARROW_LEN), w);
    let y1 = clamp(cy + dy * (gap + OVERLAY_ARROW_LEN), h);
    draw_thick_line(img, x0, y0, x1, y1, OVERLAY_ARROW_STROKE, c);
    let head = OVERLAY_ARROW_HEAD;
    if dx != 0 {
        draw_thick_line(
            img,
            x1,
            y1,
            clamp(x1 - dx * head, w),
            clamp(y1 - head, h),
            OVERLAY_ARROW_STROKE,
            c,
        );
        draw_thick_line(
            img,
            x1,
            y1,
            clamp(x1 - dx * head, w),
            clamp(y1 + head, h),
            OVERLAY_ARROW_STROKE,
            c,
        );
    } else if dy != 0 {
        draw_thick_line(
            img,
            x1,
            y1,
            clamp(x1 - head, w),
            clamp(y1 - dy * head, h),
            OVERLAY_ARROW_STROKE,
            c,
        );
        draw_thick_line(
            img,
            x1,
            y1,
            clamp(x1 + head, w),
            clamp(y1 - dy * head, h),
            OVERLAY_ARROW_STROKE,
            c,
        );
    }
}

/// Transparent PNG (native image size) with a hit marker at `(x, y)` cm, and
/// distance in mm to the nearest plate edge (left / right / top / bottom).
///
/// Marker: 5 px radius circle, 1 px white stroke. 2 px white arrows point toward
/// the nearest edge(s).
pub fn create_target_overlay(doc: &JsonDoc, x: f32, y: f32) -> Result<(Vec<u8>, f32), String> {
    let (bytes, _) = load_image_field(&doc.image, &dir())?;
    let (w, h) = image_size_bytes(&bytes)?;
    let px_per_cm_x = if doc.length_cm > 0.0 {
        w as f32 / doc.length_cm as f32
    } else {
        0.0
    };
    let px_per_cm_y = if doc.width_cm > 0.0 {
        h as f32 / doc.width_cm as f32
    } else {
        0.0
    };
    let cx = (x * px_per_cm_x).round() as i32;
    let cy = (y * px_per_cm_y).round() as i32;

    let d_left = x;
    let d_right = doc.length_cm as f32 - x;
    let d_top = y;
    let d_bottom = doc.width_cm as f32 - y;
    let nearest_cm = d_left.min(d_right).min(d_top).min(d_bottom);
    let nearest_edge_mm = nearest_cm * 10.0;

    let mut img = image::RgbaImage::new(w, h);
    let white = image::Rgba([255, 255, 255, 255]);
    draw_circle_stroke(&mut img, cx, cy, OVERLAY_CIRCLE_R, white);

    let eps = 1e-4_f32;
    if (d_left - nearest_cm).abs() <= eps {
        draw_edge_arrow(&mut img, cx, cy, -1, 0, white);
    }
    if (d_right - nearest_cm).abs() <= eps {
        draw_edge_arrow(&mut img, cx, cy, 1, 0, white);
    }
    if (d_top - nearest_cm).abs() <= eps {
        draw_edge_arrow(&mut img, cx, cy, 0, -1, white);
    }
    if (d_bottom - nearest_cm).abs() <= eps {
        draw_edge_arrow(&mut img, cx, cy, 0, 1, white);
    }

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("png: {e}"))?;
    Ok((png, nearest_edge_mm))
}

pub fn json_path_for(image: &Path) -> PathBuf {
    let stem = image.file_stem().unwrap_or_default().to_string_lossy();
    image.with_file_name(format!("{stem}.json"))
}

fn expand_path(s: &str) -> PathBuf {
    if s == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(s));
    }
    PathBuf::from(s)
}

/// Folder of `target_def` JSON files: `~/dev/esp32/target_defs`.
pub fn dir() -> PathBuf {
    expand_path("~/dev/esp32/target_defs")
}

pub fn path_for(stem: &str) -> PathBuf {
    dir().join(format!("{stem}.json"))
}

pub fn open_target_def(path: impl AsRef<Path>) -> Result<JsonDoc, String> {
    let path = path.as_ref();
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save_target_def(path: impl AsRef<Path>, doc: &JsonDoc) -> Result<(), String> {
    let path = path.as_ref();
    let out = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        path.to_path_buf()
    } else {
        let mut s = path.as_os_str().to_os_string();
        s.push(".json");
        PathBuf::from(s)
    };
    let json_dir = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut doc = doc.clone();
    let (bytes, _) = load_image_field(&doc.image, json_dir)?;
    doc.image = encode_image(&bytes);
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let s = serde_json::to_string_pretty(&doc).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::write(&out, s + "\n").map_err(|e| format!("{}: {e}", out.display()))
}

pub fn load(stem: &str) -> Result<JsonDoc, String> {
    open_target_def(path_for(stem))
}

pub fn zone_names(stem: &str) -> Result<Vec<String>, String> {
    Ok(load(stem)?.zones.into_iter().map(|p| p.name).collect())
}

/// Pretty-print `target.doc` with a trailing newline. Display pixel fields are omitted.
pub fn target_to_json(target: &Target) -> Result<String, String> {
    let s = serde_json::to_string_pretty(&target.doc).map_err(|e| e.to_string())?;
    Ok(s + "\n")
}

/// Pixel zones -> cm document. `image` is base64 of the file bytes.
pub fn to_json(
    image: String,
    img: (u32, u32),
    length_cm: f64,
    width_cm: f64,
    unit: Unit,
    polys: &[Zone],
) -> JsonDoc {
    let sx = length_cm / img.0 as f64;
    let sy = width_cm / img.1 as f64;
    let r = |v: f64| (v * 1000.0).round() / 1000.0;
    JsonDoc {
        image,
        length_cm,
        width_cm,
        display_units: Some(unit),
        zones: polys
            .iter()
            .map(|p| JsonZone {
                name: p.name.clone(),
                color: Some(p.color),
                score: p.score,
                hidden: p.hidden,
                points: p
                    .points
                    .iter()
                    .map(|&(x, y)| [r(x as f64 * sx), r(y as f64 * sy)])
                    .collect(),
            })
            .collect(),
    }
}

/// cm document -> pixel zones.
pub fn from_json(doc: &JsonDoc, img: (u32, u32)) -> Vec<Zone> {
    let sx = if doc.length_cm > 0.0 {
        img.0 as f64 / doc.length_cm
    } else {
        0.0
    };
    let sy = if doc.width_cm > 0.0 {
        img.1 as f64 / doc.width_cm
    } else {
        0.0
    };
    let n = PALETTE.len();
    let mut out: Vec<Zone> = doc
        .zones
        .iter()
        .map(|p| Zone {
            name: p.name.clone(),
            points: p
                .points
                .iter()
                .map(|[x, y]| ((x * sx) as f32, (y * sy) as f32))
                .collect(),
            color: p.color.map_or(usize::MAX, |c| c % n),
            score: p.score,
            hidden: p.hidden,
        })
        .collect();
    // Zones saved without a colour get unused slots, in order.
    for i in 0..out.len() {
        if out[i].color == usize::MAX {
            let used: Vec<usize> = out
                .iter()
                .map(|p| p.color)
                .filter(|&c| c != usize::MAX)
                .collect();
            out[i].color = next_color(used, n);
        }
    }
    out
}

/// Shoelace area (absolute). Degenerate zones are 0.
pub fn area(pts: &[(f32, f32)]) -> f32 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0f32;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        s += pts[j].0 * pts[i].1 - pts[i].0 * pts[j].1;
        j = i;
    }
    s.abs() * 0.5
}

/// Even-odd ray cast in image pixel space.
pub fn contains(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    if pts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) {
            let den = yj - yi;
            if den.abs() > f32::EPSILON {
                let t = (y - yi) / den;
                if x < xi + t * (xj - xi) {
                    inside = !inside;
                }
            }
        }
        j = i;
    }
    inside
}

/// Topmost zone containing the image-space point.
pub fn hit_poly(polys: &[Zone], x: f32, y: f32) -> Option<usize> {
    polys.iter().rposition(|p| !p.hidden && contains(&p.points, x, y))
}

/// Smallest-area visible zone containing `(x, y)`. Ties keep file order.
pub fn hit_poly_smallest(polys: &[Zone], x: f32, y: f32) -> Option<usize> {
    polys
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.hidden && contains(&p.points, x, y))
        .min_by(|(i, a), (j, b)| {
            area(&a.points)
                .partial_cmp(&area(&b.points))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(i.cmp(j))
        })
        .map(|(i, _)| i)
}

/// Smallest-area visible JSON zone containing `(x_cm, y_cm)`. Ties keep file order.
pub fn hit_zone(doc: &JsonDoc, x_cm: f32, y_cm: f32) -> Option<String> {
    hit_zone_if(doc, x_cm, y_cm, |_| true)
}

/// Like [`hit_zone`], keeping only names for which `allow` is true.
pub fn hit_zone_if(
    doc: &JsonDoc,
    x_cm: f32,
    y_cm: f32,
    allow: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut best: Option<(f32, usize, String)> = None;
    for (i, p) in doc.zones.iter().enumerate() {
        if p.hidden || !allow(&p.name) {
            continue;
        }
        let pts: Vec<(f32, f32)> = p
            .points
            .iter()
            .map(|[x, y]| (*x as f32, *y as f32))
            .collect();
        if !contains(&pts, x_cm, y_cm) {
            continue;
        }
        let a = area(&pts);
        let better = match &best {
            None => true,
            Some((ba, bi, _)) => a < *ba || ((a - *ba).abs() <= f32::EPSILON && i < *bi),
        };
        if better {
            best = Some((a, i, p.name.clone()));
        }
    }
    best.map(|(_, _, n)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(name: &str, x0: f64, y0: f64, x1: f64, y1: f64, hidden: bool) -> JsonZone {
        JsonZone {
            name: name.into(),
            color: None,
            score: 0,
            hidden,
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        }
    }

    fn plate(polys: Vec<JsonZone>) -> Target {
        let doc = JsonDoc {
            image: String::new(),
            length_cm: 10.0,
            width_cm: 10.0,
            display_units: None,
            zones: polys,
        };
        Target::new(doc, (10.0, 10.0))
    }

    fn near(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn locate_hit_roundtrip_corner_and_center() {
        let t = plate(Vec::new());
        let decay = t.decay();
        let s = t.simulate_hit(0.0, 0.0, ADC_MAX, decay);
        let (x, y) = t.locate_hit(s.map(Some)).unwrap();
        near(x, 0.0);
        near(y, 0.0);
        let s = t.simulate_hit(5.0, 5.0, ADC_MAX, decay);
        let (x, y) = t.locate_hit(s.map(Some)).unwrap();
        near(x, 5.0);
        near(y, 5.0);
        assert!(t.locate_hit([None, None, None, None]).is_none());
    }

    #[test]
    fn from_cm_matches_hub_display_and_roundtrips() {
        let doc = JsonDoc {
            image: String::new(),
            length_cm: 20.0,
            width_cm: 10.0,
            display_units: None,
            zones: Vec::new(),
        };
        let t = Target::from_cm(doc);
        near(t.length_px, 20.0);
        near(t.width_px, 10.0);
        near(t.px_per_cm_x, 1.0);
        near(t.px_per_cm_y, 1.0);
        let s = t.simulate_hit(5.0, 2.0, ADC_MAX, t.decay());
        let (x, y) = t.locate_hit(s.map(Some)).unwrap();
        near(x, 5.0);
        near(y, 2.0);
    }

    #[test]
    fn hit_zone_smallest_nested() {
        let t = plate(vec![
            square("outer", 0.0, 0.0, 10.0, 10.0, false),
            square("inner", 3.0, 3.0, 7.0, 7.0, false),
        ]);
        assert_eq!(hit_zone(&t.doc, 5.0, 5.0).as_deref(), Some("inner"));
        assert_eq!(hit_zone(&t.doc, 1.0, 1.0).as_deref(), Some("outer"));
        assert_eq!(hit_zone(&t.doc, 20.0, 20.0), None);
        assert_eq!(
            hit_zone_if(&t.doc, 5.0, 5.0, |n| n == "outer").as_deref(),
            Some("outer")
        );
        assert_eq!(hit_zone_if(&t.doc, 5.0, 5.0, |_| false), None);
        let hidden_inner = plate(vec![
            square("outer", 0.0, 0.0, 10.0, 10.0, false),
            square("inner", 3.0, 3.0, 7.0, 7.0, true),
        ]);
        assert_eq!(hit_zone(&hidden_inner.doc, 5.0, 5.0).as_deref(), Some("outer"));
        let px = from_json(&t.doc, (10, 10));
        assert_eq!(hit_poly_smallest(&px, 5.0, 5.0), Some(1));
        assert_eq!(area(&px[1].points), 16.0);
    }

    fn tiny_png_doc(length_cm: f64, width_cm: f64, px: (u32, u32)) -> JsonDoc {
        let img = image::RgbaImage::from_pixel(px.0, px.1, image::Rgba([0, 0, 0, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                image::ImageFormat::Png,
            )
            .unwrap();
        JsonDoc {
            image: encode_image(&png),
            length_cm,
            width_cm,
            display_units: None,
            zones: Vec::new(),
        }
    }

    #[test]
    fn create_target_overlay_marker_and_distance() {
        let doc = tiny_png_doc(10.0, 8.0, (100, 80));
        let (out, mm) = create_target_overlay(&doc, 5.0, 4.0).unwrap();
        near(mm, 40.0);
        let decoded = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (100, 80));
        let ring = decoded.get_pixel(55, 40);
        assert_eq!(ring[0], 255);
        assert_eq!(ring[3], 255);
        let corner = decoded.get_pixel(0, 0);
        assert_eq!(corner[3], 0);
        let (_, mm_left) = create_target_overlay(&doc, 1.0, 4.0).unwrap();
        near(mm_left, 10.0);
    }
}
