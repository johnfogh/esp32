//! Target definition files (`*.json` in `targets_folder`, written in Edit mode).
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Target definitions from `config.json` `targets_folder`.
pub fn dir() -> PathBuf {
    crate::config::targets_dir()
}

pub fn path_for(stem: &str) -> PathBuf {
    dir().join(format!("{stem}.json"))
}

/// Image, zone names, and pixel geometry from a target JSON (for Stages cards).
pub struct Preview {
    pub image: Option<Arc<gpui::Image>>,
    pub size: Option<(u32, u32)>,
    /// `(length_cm, width_cm)` from the JSON, when parsed.
    pub size_cm: Option<(f64, f64)>,
    pub zones: Vec<String>,
    pub geometry: Vec<targetlib::Zone>,
    pub err: Option<String>,
}

fn empty_preview(err: String) -> Preview {
    Preview {
        image: None,
        size: None,
        size_cm: None,
        zones: Vec::new(),
        geometry: Vec::new(),
        err: Some(err),
    }
}

pub fn preview(path: &Path) -> Preview {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return empty_preview(format!("{}: {e}", path.display())),
    };
    let doc: targetlib::JsonDoc = match serde_json::from_str(&s) {
        Ok(d) => d,
        Err(e) => return empty_preview(e.to_string()),
    };
    let json_dir = path.parent().unwrap_or(Path::new("."));
    let loaded = targetlib::load_image_field(&doc.image, json_dir);
    let (image, size, err) = match loaded {
        Ok((bytes, _)) => {
            let size = targetlib::image_size_bytes(&bytes).ok();
            match crate::model::gpui_image(&bytes) {
                Ok(img) => (Some(img), size, None),
                Err(e) => (None, size, Some(e)),
            }
        }
        Err(e) => (None, None, Some(e)),
    };
    let geometry = match size {
        Some((w, h)) if w > 0 && h > 0 => targetlib::from_json(&doc, (w, h)),
        _ => Vec::new(),
    };
    let mut zones: Vec<String> = geometry.iter().map(|p| p.name.clone()).collect();
    zones.sort_by(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
    let size_cm = (doc.length_cm > 0.0 && doc.width_cm > 0.0)
        .then_some((doc.length_cm, doc.width_cm));
    Preview {
        image,
        size,
        size_cm,
        zones,
        geometry,
        err,
    }
}
