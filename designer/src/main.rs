use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use gpui::{
    App, Application, Bounds, Context, Div, ElementId, FocusHandle, Image, ImageFormat,
    InteractiveElement, KeyBinding, KeyDownEvent, MouseButton, ObjectFit, PathBuilder,
    PathPromptOptions, Stateful, Window, WindowBounds, WindowOptions, actions, canvas, div, img,
    point, prelude::*, px, relative, rgb, size,
};
use serde::{Deserialize, Serialize};
use utils::host::{Chip, Framework, build_command, detect_boards, elf_path, flash_command};

actions!(
    timeline,
    [
        PrevEvent,
        NextEvent,
        NudgeCursorBack1s,
        NudgeCursorForward1s,
        NudgeCursorBack100ms,
        NudgeCursorForward100ms
    ]
);

actions!(file, [OpenFile, SaveFile, SaveFileAs]);

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CardType {
    Activate,
    Deactivate,
    StartTimer,
    StopTimer,
}

impl CardType {
    fn label(self) -> &'static str {
        match self {
            CardType::Activate => "Activate",
            CardType::Deactivate => "Deactivate",
            CardType::StartTimer => "Start Timer",
            CardType::StopTimer => "Stop Timer",
        }
    }

    fn all() -> [CardType; 4] {
        [
            CardType::Activate,
            CardType::Deactivate,
            CardType::StartTimer,
            CardType::StopTimer,
        ]
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct EventCard {
    card_type: CardType,
    /// Timer name, or Activate/Deactivate `Name - Zone` label.
    name: String,
}

struct Event {
    time: Duration,
    cards: Vec<EventCard>,
}

#[derive(Serialize, Deserialize)]
struct ProjectEvent {
    time_ms: u32,
    cards: Vec<EventCard>,
}

#[derive(Serialize, Deserialize)]
struct ProjectDocument {
    rows: usize,
    cols: usize,
    cells: Vec<CellKind>,
    duration_ms: u32,
    events: Vec<ProjectEvent>,
}

/// Stub device card for the Devices view (no real device logic yet).
struct DeviceCard {
    /// Selected layout Target cell name.
    target_name: String,
}

/// Firmware project flashed onto USB boards from the Devices view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlashProject {
    Laser,
    Sensor,
}

impl FlashProject {
    fn label(self) -> &'static str {
        match self {
            FlashProject::Laser => "Laser",
            FlashProject::Sensor => "Sensor",
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            FlashProject::Laser => "laser",
            FlashProject::Sensor => "sensor",
        }
    }

    fn all() -> [FlashProject; 2] {
        [FlashProject::Laser, FlashProject::Sensor]
    }
}

#[derive(Clone)]
struct FlashBoard {
    port: String,
    chip: Chip,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LengthUnit {
    Inches,
    Feet,
    Yards,
    Centimeters,
    Meters,
}

impl LengthUnit {
    fn label(self) -> &'static str {
        match self {
            LengthUnit::Inches => "inches",
            LengthUnit::Feet => "feet",
            LengthUnit::Yards => "yard",
            LengthUnit::Centimeters => "centimeters",
            LengthUnit::Meters => "meters",
        }
    }

    fn all() -> [LengthUnit; 5] {
        [
            LengthUnit::Inches,
            LengthUnit::Feet,
            LengthUnit::Yards,
            LengthUnit::Centimeters,
            LengthUnit::Meters,
        ]
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CellKind {
    Empty,
    Connector { distance: f64, units: LengthUnit },
    Target { def: String, name: String },
}

#[derive(Clone, Copy)]
enum GridSide {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppView {
    Layout,
    Events,
    Devices,
}

impl CellKind {
    fn label(&self) -> Option<&str> {
        match self {
            CellKind::Empty | CellKind::Connector { .. } => None,
            CellKind::Target { name, .. } => Some(name.as_str()),
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, CellKind::Empty)
    }

    fn default_connector() -> Self {
        CellKind::Connector {
            distance: 0.0,
            units: LengthUnit::Inches,
        }
    }
}

struct Designer {
    menu_open: bool,
    view: AppView,
    /// Path of the open layout file; `None` means unsaved / Untitled.
    layout_path: Option<PathBuf>,
    targets_sidebar: Option<(usize, usize)>,
    target_names: Vec<String>,
    target_images: HashMap<String, Arc<Image>>,
    /// Zone names from each target_def's `polygons[].name`, keyed by def stem.
    target_zones: HashMap<String, Vec<String>>,
    rows: usize,
    cols: usize,
    cells: Vec<CellKind>,
    duration_ms: u32,
    duration_edit: Option<String>,
    distance_edit: Option<String>,
    name_edit: Option<String>,
    event_time_edit: Option<(usize, String)>,
    /// Index of the event whose title is focused (Enter starts time edit).
    event_title_focused: Option<usize>,
    /// Focused stub card within an event pane: (event_idx, card_idx).
    event_card_focused: Option<(usize, usize)>,
    units_menu_open: bool,
    /// Open card-type select as (event_idx, card_idx), if any.
    card_type_menu_open: Option<(usize, usize)>,
    /// Inline name edit for a Start Timer card: (event_idx, card_idx, draft).
    event_card_name_edit: Option<(usize, usize, String)>,
    /// Open timer/target name select as (event_idx, card_idx), if any.
    timer_name_menu_open: Option<(usize, usize)>,
    /// Open device target-name select as device index, if any.
    device_target_menu_open: Option<usize>,
    /// USB boards detected for flashing.
    flash_boards: Vec<FlashBoard>,
    flash_project: FlashProject,
    flash_project_menu_open: bool,
    flash_status: String,
    flash_busy: bool,
    cursor_ms: u32,
    events: Vec<Event>,
    devices: Vec<DeviceCard>,
    focus_handle: FocusHandle,
}

impl Designer {
    fn layout_display_name(&self) -> String {
        self.layout_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    fn to_document(&self) -> ProjectDocument {
        ProjectDocument {
            rows: self.rows,
            cols: self.cols,
            cells: self.cells.clone(),
            duration_ms: self.duration_ms,
            events: self
                .events
                .iter()
                .map(|e| ProjectEvent {
                    time_ms: Self::event_ms(e),
                    cards: e.cards.clone(),
                })
                .collect(),
        }
    }

    fn apply_document(&mut self, doc: ProjectDocument) {
        self.rows = doc.rows.max(1);
        self.cols = doc.cols.max(1);
        let cell_count = self.rows.saturating_mul(self.cols);
        let mut cells = doc.cells;
        cells.resize(cell_count, CellKind::Empty);
        self.cells = cells;
        self.duration_ms = doc.duration_ms.max(1);
        self.events = doc
            .events
            .into_iter()
            .map(|e| Event {
                time: Duration::from_millis(e.time_ms.min(self.duration_ms) as u64),
                cards: e.cards,
            })
            .collect();
        self.events
            .sort_by_key(|e| Self::event_ms(e));
        self.cursor_ms = self.cursor_ms.min(self.duration_ms);
        self.targets_sidebar = None;
        self.duration_edit = None;
        self.distance_edit = None;
        self.name_edit = None;
        self.event_time_edit = None;
        self.event_title_focused = None;
        self.event_card_focused = None;
        self.units_menu_open = false;
        self.card_type_menu_open = None;
        self.event_card_name_edit = None;
        self.timer_name_menu_open = None;
        self.device_target_menu_open = None;
        self.flash_project_menu_open = false;
    }

    fn write_to_path(&self, path: &Path) -> bool {
        let Ok(json) = serde_json::to_string_pretty(&self.to_document()) else {
            return false;
        };
        fs::write(path, json).is_ok()
    }

    fn load_from_path(&mut self, path: &Path) -> bool {
        let Ok(text) = fs::read_to_string(path) else {
            return false;
        };
        let Ok(doc) = serde_json::from_str::<ProjectDocument>(&text) else {
            return false;
        };
        if doc.rows == 0 || doc.cols == 0 {
            return false;
        }
        self.apply_document(doc);
        self.layout_path = Some(path.to_path_buf());
        true
    }

    fn commit_all_edits(&mut self, cx: &mut Context<Self>) {
        self.commit_pending_prop_edits(cx);
        self.commit_event_card_name_edit(cx);
        self.commit_duration_edit(cx);
        self.commit_event_time_edit(cx);
    }

    fn default_save_dir(&self) -> PathBuf {
        if let Some(path) = self.layout_path.as_ref() {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    return parent.to_path_buf();
                }
            }
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn suggested_save_name(&self) -> String {
        self.layout_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "untitled.json".to_string())
    }

    fn open_file(&mut self, _: &OpenFile, _: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        cx.notify();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.load_from_path(&path);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_file(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_all_edits(cx);
        self.menu_open = false;
        if let Some(path) = self.layout_path.clone() {
            let _ = self.write_to_path(&path);
            cx.notify();
        } else {
            self.save_file_as(&SaveFileAs, window, cx);
        }
    }

    fn save_file_as(&mut self, _: &SaveFileAs, _: &mut Window, cx: &mut Context<Self>) {
        self.commit_all_edits(cx);
        self.menu_open = false;
        cx.notify();
        let dir = self.default_save_dir();
        let name = self.suggested_save_name();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.write_to_path(&path) {
                    this.layout_path = Some(path);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sniff_image_format(bytes: &[u8]) -> Option<ImageFormat> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageFormat::Jpeg)
        } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            Some(ImageFormat::Png)
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Some(ImageFormat::Webp)
        } else if bytes.starts_with(b"GIF8") {
            Some(ImageFormat::Gif)
        } else if bytes.starts_with(b"BM") {
            Some(ImageFormat::Bmp)
        } else {
            None
        }
    }

    fn load_targets() -> (
        Vec<String>,
        HashMap<String, Arc<Image>>,
        HashMap<String, Vec<String>>,
    ) {
        let mut dir = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        dir.push("dev/esp32/target_defs");
        let mut names = Vec::new();
        let mut images = HashMap::new();
        let mut zones_by_def = HashMap::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let name = stem.to_string();
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(b64) = value.get("image").and_then(|v| v.as_str()) {
                            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
                            {
                                if let Some(format) = Self::sniff_image_format(&bytes) {
                                    images.insert(name.clone(), Arc::new(Image::from_bytes(format, bytes)));
                                }
                            }
                        }
                        if let Some(polygons) = value.get("polygons").and_then(|v| v.as_array()) {
                            let mut zones = Vec::new();
                            for poly in polygons {
                                if let Some(zone) = poly.get("name").and_then(|v| v.as_str()) {
                                    let zone = zone.trim();
                                    if !zone.is_empty() && !zones.iter().any(|z| z == zone) {
                                        zones.push(zone.to_string());
                                    }
                                }
                            }
                            if !zones.is_empty() {
                                zones_by_def.insert(name.clone(), zones);
                            }
                        }
                    }
                }
                names.push(name);
            }
        }
        names.sort();
        (names, images, zones_by_def)
    }

    fn resize_on_side(&mut self, side: GridSide, grow: bool) {
        let old_rows = self.rows;
        let old_cols = self.cols;
        let (new_rows, new_cols) = match (side, grow) {
            (GridSide::Left | GridSide::Right, true) => (old_rows, old_cols + 1),
            (GridSide::Top | GridSide::Bottom, true) => (old_rows + 1, old_cols),
            (GridSide::Left | GridSide::Right, false) => {
                if old_cols <= 1 {
                    return;
                }
                (old_rows, old_cols - 1)
            }
            (GridSide::Top | GridSide::Bottom, false) => {
                if old_rows <= 1 {
                    return;
                }
                (old_rows - 1, old_cols)
            }
        };

        let mut next = vec![CellKind::Empty; new_rows * new_cols];
        for r in 0..old_rows {
            for c in 0..old_cols {
                let dest = match (side, grow) {
                    (GridSide::Left, true) => Some((r, c + 1)),
                    (GridSide::Right, true) => Some((r, c)),
                    (GridSide::Top, true) => Some((r + 1, c)),
                    (GridSide::Bottom, true) => Some((r, c)),
                    (GridSide::Left, false) => {
                        if c == 0 {
                            None
                        } else {
                            Some((r, c - 1))
                        }
                    }
                    (GridSide::Right, false) => {
                        if c >= new_cols {
                            None
                        } else {
                            Some((r, c))
                        }
                    }
                    (GridSide::Top, false) => {
                        if r == 0 {
                            None
                        } else {
                            Some((r - 1, c))
                        }
                    }
                    (GridSide::Bottom, false) => {
                        if r >= new_rows {
                            None
                        } else {
                            Some((r, c))
                        }
                    }
                };
                if let Some((nr, nc)) = dest {
                    next[nr * new_cols + nc] = self.cells[r * old_cols + c].clone();
                }
            }
        }

        if let Some((r, c)) = self.targets_sidebar {
            self.targets_sidebar = match (side, grow) {
                (GridSide::Left, true) => Some((r, c + 1)),
                (GridSide::Top, true) => Some((r + 1, c)),
                (GridSide::Left, false) if c == 0 => None,
                (GridSide::Left, false) => Some((r, c - 1)),
                (GridSide::Top, false) if r == 0 => None,
                (GridSide::Top, false) => Some((r - 1, c)),
                (GridSide::Right, false) if c >= new_cols => None,
                (GridSide::Bottom, false) if r >= new_rows => None,
                _ => Some((r, c)),
            };
        }

        self.rows = new_rows;
        self.cols = new_cols;
        self.cells = next;
    }

    fn cell_index(&self, row: usize, col: usize) -> usize {
        row * self.cols + col
    }

    fn assign_cell(&mut self, row: usize, col: usize, kind: CellKind) {
        // Drop drafts for the new type; prior write-through already updated cells.
        self.distance_edit = None;
        self.name_edit = None;
        self.units_menu_open = false;
        let idx = self.cell_index(row, col);
        if let Some(cell) = self.cells.get_mut(idx) {
            *cell = kind;
        }
        // Keep cell selection after type assign so properties stay editable.
        // Escape still clears selection via on_key_down.
    }

    fn selected_cell_mut(&mut self) -> Option<&mut CellKind> {
        let (row, col) = self.targets_sidebar?;
        let idx = self.cell_index(row, col);
        self.cells.get_mut(idx)
    }

    /// Write distance draft into the selected connector when parseable.
    /// Invalid / empty draft leaves the prior cell value unchanged.
    fn apply_distance_draft(&mut self) {
        let Some(draft) = self.distance_edit.as_deref() else {
            return;
        };
        let Ok(value) = draft.trim().parse::<f64>() else {
            return;
        };
        if let Some(CellKind::Connector { distance, .. }) = self.selected_cell_mut() {
            *distance = value;
        }
    }

    /// Write name draft into the selected target when non-empty.
    /// Empty draft leaves the prior cell value unchanged.
    fn apply_name_draft(&mut self) {
        let Some(draft) = self.name_edit.as_deref() else {
            return;
        };
        let name = draft.trim().to_string();
        if name.is_empty() {
            return;
        }
        if let Some(CellKind::Target { name: current, .. }) = self.selected_cell_mut() {
            *current = name;
        }
    }

    /// Commit distance + name drafts into the selected cell, then clear drafts.
    /// Invalid distance parse or empty name leaves the prior cell value.
    fn commit_pending_prop_edits(&mut self, cx: &mut Context<Self>) {
        self.apply_distance_draft();
        self.apply_name_draft();
        self.distance_edit = None;
        self.name_edit = None;
        cx.notify();
    }

    fn commit_distance_edit(&mut self, cx: &mut Context<Self>) {
        self.apply_distance_draft();
        self.distance_edit = None;
        cx.notify();
    }

    fn commit_name_edit(&mut self, cx: &mut Context<Self>) {
        self.apply_name_draft();
        self.name_edit = None;
        cx.notify();
    }

    fn set_connector_units(&mut self, units: LengthUnit, cx: &mut Context<Self>) {
        if let Some(CellKind::Connector { units: current, .. }) = self.selected_cell_mut() {
            *current = units;
            cx.notify();
        }
    }

    fn format_position(ms: u32) -> String {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1_000;
        let millis = ms % 1_000;
        format!("{minutes:02}:{seconds:02}.{millis:03}")
    }

    fn parse_position_ms(s: &str) -> Option<u32> {
        let s = s.trim();
        let (min_str, rest) = s.split_once(':')?;
        let minutes: u32 = min_str.parse().ok()?;
        let (sec_str, millis) = if let Some((sec, ms)) = rest.split_once('.') {
            let millis_str = if ms.len() >= 3 {
                &ms[..3]
            } else {
                ms
            };
            let mut millis: u32 = millis_str.parse().ok()?;
            for _ in millis_str.len()..3 {
                millis *= 10;
            }
            (sec, millis)
        } else {
            (rest, 0)
        };
        let seconds: u32 = sec_str.parse().ok()?;
        if seconds >= 60 || millis >= 1000 {
            return None;
        }
        Some(
            minutes
                .saturating_mul(60_000)
                .saturating_add(seconds.saturating_mul(1_000))
                .saturating_add(millis),
        )
    }

    fn parse_position(s: &str) -> Option<u32> {
        Self::parse_position_ms(s).map(|ms| ms.max(1))
    }

    fn any_text_edit_active(&self) -> bool {
        self.duration_edit.is_some()
            || self.distance_edit.is_some()
            || self.name_edit.is_some()
            || self.event_time_edit.is_some()
            || self.event_card_name_edit.is_some()
    }

    /// Unique non-empty names from all Start Timer cards (order of first appearance).
    fn start_timer_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for event in &self.events {
            for card in &event.cards {
                if card.card_type == CardType::StartTimer {
                    let name = card.name.trim();
                    if !name.is_empty() && !names.iter().any(|n| n == name) {
                        names.push(name.to_string());
                    }
                }
            }
        }
        names
    }

    /// Unique non-empty names from all Activate cards (order of first appearance).
    fn activate_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for event in &self.events {
            for card in &event.cards {
                if card.card_type == CardType::Activate {
                    let name = card.name.trim();
                    if !name.is_empty() && !names.iter().any(|n| n == name) {
                        names.push(name.to_string());
                    }
                }
            }
        }
        names
    }

    fn card_type_count(&self, card_type: CardType) -> usize {
        self.events
            .iter()
            .flat_map(|e| e.cards.iter())
            .filter(|c| c.card_type == card_type)
            .count()
    }

    fn has_card_type(&self, card_type: CardType) -> bool {
        self.card_type_count(card_type) > 0
    }

    /// Default name for a new Start Timer: `timer-{n}` with n = existing count + 1.
    fn next_timer_name(&self) -> String {
        format!("timer-{}", self.card_type_count(CardType::StartTimer) + 1)
    }

    /// Activate menu labels: unique non-empty layout target names.
    fn layout_target_names(&self) -> Vec<String> {
        unique_layout_target_names(&self.cells)
    }

    /// Activate menu labels: bare target names, then `{name} - {zone}`
    /// (same zone format as Deactivate). Deduped; names first.
    fn layout_activate_labels(&self) -> Vec<String> {
        let mut labels = self.layout_target_names();
        for zone_label in self.layout_target_zone_labels() {
            if !labels.iter().any(|l| l == &zone_label) {
                labels.push(zone_label);
            }
        }
        labels
    }

    /// Deactivate menu labels: `{target name} - {zone}` for each layout
    /// Target cell and each zone from that cell's target_def polygons.
    fn layout_target_zone_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        for cell in &self.cells {
            let CellKind::Target { def, name } = cell else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let Some(zones) = self.target_zones.get(def) else {
                continue;
            };
            for zone in zones {
                let zone = zone.trim();
                if zone.is_empty() {
                    continue;
                }
                let label = format!("{name} - {zone}");
                if !labels.iter().any(|l| l == &label) {
                    labels.push(label);
                }
            }
        }
        labels
    }

    fn apply_event_card_name_draft(&mut self) {
        let Some((event_idx, card_idx, draft)) = self.event_card_name_edit.clone() else {
            return;
        };
        let name = draft.trim().to_string();
        if name.is_empty() {
            return;
        }
        if let Some(card) = self
            .events
            .get_mut(event_idx)
            .and_then(|e| e.cards.get_mut(card_idx))
        {
            if card.card_type == CardType::StartTimer {
                card.name = name;
            }
        }
    }

    fn commit_event_card_name_edit(&mut self, cx: &mut Context<Self>) {
        self.apply_event_card_name_draft();
        self.event_card_name_edit = None;
        cx.notify();
    }

    fn event_ms(event: &Event) -> u32 {
        event.time.as_millis() as u32
    }

    fn commit_duration_edit(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.duration_edit.as_deref() else {
            return;
        };
        if let Some(ms) = Self::parse_position(draft) {
            self.duration_ms = ms;
            self.cursor_ms = self.cursor_ms.min(ms);
            self.duration_edit = None;
            cx.notify();
        }
    }

    fn commit_event_time_edit(&mut self, cx: &mut Context<Self>) {
        let Some((idx, draft)) = self.event_time_edit.clone() else {
            return;
        };
        let Some(parsed) = Self::parse_position_ms(&draft) else {
            return;
        };
        let ms = parsed.min(self.duration_ms);
        if idx >= self.events.len() {
            self.event_time_edit = None;
            cx.notify();
            return;
        }
        if self
            .events
            .iter()
            .enumerate()
            .any(|(i, e)| i != idx && Self::event_ms(e) == ms)
        {
            return;
        }
        let mut event = self.events.remove(idx);
        event.time = Duration::from_millis(ms as u64);
        let insert_at = self
            .events
            .iter()
            .position(|e| Self::event_ms(e) > ms)
            .unwrap_or(self.events.len());
        self.events.insert(insert_at, event);
        self.cursor_ms = ms;
        self.event_time_edit = None;
        self.event_title_focused = Some(insert_at);
        self.event_card_focused = None;
        self.card_type_menu_open = None;
        cx.notify();
    }

    fn create_event_at_cursor(&mut self, card_type: CardType, cx: &mut Context<Self>) {
        if self.any_text_edit_active() {
            return;
        }
        let name = if card_type == CardType::StartTimer {
            self.next_timer_name()
        } else {
            String::new()
        };
        let cursor_ms = self.cursor_ms;
        if let Some(idx) = self
            .events
            .iter()
            .position(|e| Self::event_ms(e) == cursor_ms)
        {
            let Some(event) = self.events.get_mut(idx) else {
                return;
            };
            event.cards.push(EventCard { card_type, name });
            let card_idx = event.cards.len() - 1;
            self.event_title_focused = None;
            self.event_card_focused = Some((idx, card_idx));
            self.card_type_menu_open = None;
            self.event_card_name_edit = None;
            self.timer_name_menu_open = None;
            self.device_target_menu_open = None;
            cx.notify();
            return;
        }
        let event = Event {
            time: Duration::from_millis(cursor_ms as u64),
            cards: vec![EventCard { card_type, name }],
        };
        let insert_at = self
            .events
            .iter()
            .position(|e| Self::event_ms(e) > cursor_ms)
            .unwrap_or(self.events.len());
        self.events.insert(insert_at, event);
        self.event_title_focused = None;
        self.event_card_focused = Some((insert_at, 0));
        self.card_type_menu_open = None;
        self.event_card_name_edit = None;
        self.timer_name_menu_open = None;
        self.device_target_menu_open = None;
        cx.notify();
    }

    fn remove_focused_event(&mut self, cx: &mut Context<Self>) {
        if self.any_text_edit_active() {
            return;
        }
        let Some(idx) = self
            .events
            .iter()
            .position(|e| Self::event_ms(e) == self.cursor_ms)
        else {
            return;
        };
        self.events.remove(idx);
        self.event_time_edit = None;
        self.event_title_focused = None;
        self.event_card_focused = None;
        self.card_type_menu_open = None;
        self.event_card_name_edit = None;
        self.timer_name_menu_open = None;
        self.device_target_menu_open = None;
        cx.notify();
    }

    fn remove_card_from_event(&mut self, idx: usize, card_idx: usize, cx: &mut Context<Self>) {
        if self.any_text_edit_active() {
            return;
        }
        let Some(event) = self.events.get_mut(idx) else {
            return;
        };
        if card_idx >= event.cards.len() {
            self.event_card_focused = None;
            self.card_type_menu_open = None;
            self.event_card_name_edit = None;
            self.timer_name_menu_open = None;
            self.device_target_menu_open = None;
            cx.notify();
            return;
        }
        event.cards.remove(card_idx);
        self.event_title_focused = None;
        self.event_card_focused = None;
        self.card_type_menu_open = None;
        self.event_card_name_edit = None;
        self.timer_name_menu_open = None;
        self.device_target_menu_open = None;
        cx.notify();
    }

    fn start_event_time_edit(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.events.len() {
            return;
        }
        let t = Self::event_ms(&self.events[idx]);
        self.cursor_ms = t;
        self.event_title_focused = None;
        self.event_card_focused = None;
        self.card_type_menu_open = None;
        self.event_card_name_edit = None;
        self.timer_name_menu_open = None;
        self.device_target_menu_open = None;
        self.event_time_edit = Some((idx, Self::format_position(t)));
        cx.notify();
    }

    fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();

        if self.distance_edit.is_some() {
            match key {
                "enter" => self.commit_distance_edit(cx),
                "escape" => {
                    // Preserve typed value, then end edit; sidebar stays open.
                    self.commit_pending_prop_edits(cx);
                }
                "backspace" => {
                    if let Some(draft) = self.distance_edit.as_mut() {
                        draft.pop();
                    }
                    self.apply_distance_draft();
                    cx.notify();
                }
                _ => {
                    if let Some(ch) = event.keystroke.key_char.as_deref() {
                        if ch.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') {
                            if let Some(draft) = self.distance_edit.as_mut() {
                                draft.push_str(ch);
                            }
                            self.apply_distance_draft();
                            cx.notify();
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        if self.name_edit.is_some() {
            match key {
                "enter" => self.commit_name_edit(cx),
                "escape" => {
                    // Preserve typed value, then end edit; sidebar stays open.
                    self.commit_pending_prop_edits(cx);
                }
                "backspace" => {
                    if let Some(draft) = self.name_edit.as_mut() {
                        draft.pop();
                    }
                    self.apply_name_draft();
                    cx.notify();
                }
                _ => {
                    if let Some(ch) = event.keystroke.key_char.as_deref() {
                        if ch.chars().any(|c| !c.is_control()) {
                            if let Some(draft) = self.name_edit.as_mut() {
                                draft.push_str(ch);
                            }
                            self.apply_name_draft();
                            cx.notify();
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        if self.event_card_name_edit.is_some() {
            match key {
                "enter" => self.commit_event_card_name_edit(cx),
                "escape" => {
                    self.commit_event_card_name_edit(cx);
                }
                "backspace" => {
                    if let Some((_, _, draft)) = self.event_card_name_edit.as_mut() {
                        draft.pop();
                    }
                    self.apply_event_card_name_draft();
                    cx.notify();
                }
                _ => {
                    if let Some(ch) = event.keystroke.key_char.as_deref() {
                        if ch.chars().any(|c| !c.is_control()) {
                            if let Some((_, _, draft)) = self.event_card_name_edit.as_mut() {
                                draft.push_str(ch);
                            }
                            self.apply_event_card_name_draft();
                            cx.notify();
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        if self.duration_edit.is_some() {
            match key {
                "enter" => self.commit_duration_edit(cx),
                "escape" => {
                    self.duration_edit = None;
                    cx.notify();
                }
                "backspace" => {
                    if let Some(draft) = self.duration_edit.as_mut() {
                        draft.pop();
                        cx.notify();
                    }
                }
                _ => {
                    if let Some(ch) = event.keystroke.key_char.as_deref() {
                        if ch.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '.') {
                            if let Some(draft) = self.duration_edit.as_mut() {
                                draft.push_str(ch);
                                cx.notify();
                            }
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        if self.event_time_edit.is_some() {
            match key {
                "enter" => self.commit_event_time_edit(cx),
                "escape" => {
                    self.event_time_edit = None;
                    cx.notify();
                }
                "backspace" => {
                    if let Some((_, draft)) = self.event_time_edit.as_mut() {
                        draft.pop();
                        cx.notify();
                    }
                }
                _ => {
                    if let Some(ch) = event.keystroke.key_char.as_deref() {
                        if ch.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '.') {
                            if let Some((_, draft)) = self.event_time_edit.as_mut() {
                                draft.push_str(ch);
                                cx.notify();
                            }
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        // Focused event title: Enter starts time edit.
        if let Some(idx) = self.event_title_focused {
            if key == "enter" {
                self.start_event_time_edit(idx, cx);
                cx.stop_propagation();
                return;
            }
            if key == "escape" {
                self.event_title_focused = None;
                self.event_card_focused = None;
                self.card_type_menu_open = None;
                self.timer_name_menu_open = None;
                self.device_target_menu_open = None;
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }

        if key == "escape" && self.timer_name_menu_open.is_some() {
            self.timer_name_menu_open = None;
            self.device_target_menu_open = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if key == "escape" && self.device_target_menu_open.is_some() {
            self.device_target_menu_open = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if key == "escape" && self.flash_project_menu_open {
            self.flash_project_menu_open = false;
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if key == "escape" && self.card_type_menu_open.is_some() {
            self.card_type_menu_open = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if key == "escape" && self.event_card_focused.is_some() {
            self.event_card_focused = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if key == "escape" && self.targets_sidebar.is_some() {
            self.commit_pending_prop_edits(cx);
            self.targets_sidebar = None;
            self.units_menu_open = false;
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn nudge_cursor(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.any_text_edit_active() {
            return;
        }
        let next = if delta < 0 {
            self.cursor_ms.saturating_sub((-delta) as u32)
        } else {
            self.cursor_ms.saturating_add(delta as u32)
        };
        self.cursor_ms = next.min(self.duration_ms);
        self.event_title_focused = None;
        self.event_card_focused = None;
        self.card_type_menu_open = None;
        self.event_card_name_edit = None;
        self.timer_name_menu_open = None;
        self.device_target_menu_open = None;
        cx.notify();
    }

    fn prev_event(&mut self, _: &PrevEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.any_text_edit_active() {
            return;
        }
        if let Some(t) = self
            .events
            .iter()
            .rev()
            .map(Self::event_ms)
            .find(|t| *t < self.cursor_ms)
        {
            self.cursor_ms = t;
            self.event_title_focused = None;
            self.event_card_focused = None;
            self.card_type_menu_open = None;
            self.timer_name_menu_open = None;
            self.device_target_menu_open = None;
            cx.notify();
        }
    }

    fn next_event(&mut self, _: &NextEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.any_text_edit_active() {
            return;
        }
        if let Some(t) = self
            .events
            .iter()
            .map(Self::event_ms)
            .find(|t| *t > self.cursor_ms)
        {
            self.cursor_ms = t;
            self.event_title_focused = None;
            self.event_card_focused = None;
            self.card_type_menu_open = None;
            self.timer_name_menu_open = None;
            self.device_target_menu_open = None;
            cx.notify();
        }
    }

    fn nudge_cursor_back_1s(
        &mut self,
        _: &NudgeCursorBack1s,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nudge_cursor(-1000, cx);
    }

    fn nudge_cursor_forward_1s(
        &mut self,
        _: &NudgeCursorForward1s,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nudge_cursor(1000, cx);
    }

    fn nudge_cursor_back_100ms(
        &mut self,
        _: &NudgeCursorBack100ms,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nudge_cursor(-100, cx);
    }

    fn nudge_cursor_forward_100ms(
        &mut self,
        _: &NudgeCursorForward100ms,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nudge_cursor(100, cx);
    }

    /// Side button. `cols` true → left/right (columns); false → top/bottom (rows).
    /// Left-click adds; right-click removes (minimum 1).
    fn edge(id: &'static str, side: GridSide, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .cursor_pointer()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x505050))
            .text_xs()
            .text_color(rgb(0xc0c0c0))
            .child("+")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.resize_on_side(side, true);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    this.resize_on_side(side, false);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    fn render_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows;
        let cols = self.cols;
        let border = rgb(0x505050);
        let gap = rgb(0x333334);
        let cell_bg = rgb(0x252526);
        let connector_color = rgb(0xc0c0c0);
        let cells = self.cells.clone();
        let target_images = self.target_images.clone();
        let selected = self.targets_sidebar;
        let distance_edit = self.distance_edit.clone();
        let name_edit = self.name_edit.clone();

        let cells_el = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .p(px(2.0))
            .bg(border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .bg(gap)
                    .gap(px(2.0))
                    .children((0..rows).map(|r| {
                        let cells = cells.clone();
                        let target_images = target_images.clone();
                        let distance_edit = distance_edit.clone();
                        let name_edit = name_edit.clone();
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.))
                            .gap(px(2.0))
                            .children((0..cols).map(|c| {
                                let kind = cells
                                    .get(r * cols + c)
                                    .cloned()
                                    .unwrap_or(CellKind::Empty);
                                let is_selected = selected == Some((r, c));
                                let target_image = match &kind {
                                    CellKind::Target { def, .. } => {
                                        target_images.get(def).cloned()
                                    }
                                    _ => None,
                                };
                                // Show live drafts on the selected cell while sidebar props edit.
                                let label = match (&kind, is_selected, &name_edit) {
                                    (CellKind::Target { .. }, true, Some(draft)) => {
                                        Some(draft.clone())
                                    }
                                    _ => kind.label().map(|s| s.to_string()),
                                };
                                let connector_caption = match &kind {
                                    CellKind::Connector { units, .. }
                                        if is_selected && distance_edit.is_some() =>
                                    {
                                        Some(format!(
                                            "{} {}",
                                            distance_edit.as_deref().unwrap_or(""),
                                            units.label()
                                        ))
                                    }
                                    CellKind::Connector { distance, units } => {
                                        Some(format!("{distance} {}", units.label()))
                                    }
                                    _ => None,
                                };
                                let is_connector = connector_caption.is_some();
                                let vertical_connector = is_connector && {
                                    let above_occupied = r > 0
                                        && !matches!(
                                            cells.get((r - 1) * cols + c),
                                            Some(CellKind::Empty) | None
                                        );
                                    let below_occupied = r + 1 < rows
                                        && !matches!(
                                            cells.get((r + 1) * cols + c),
                                            Some(CellKind::Empty) | None
                                        );
                                    above_occupied && below_occupied
                                };
                                div()
                                    .id(("grid-cell", r * cols + c))
                                    .relative()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .min_h(px(0.))
                                    .bg(if is_selected {
                                        rgb(0x505051)
                                    } else {
                                        cell_bg
                                    })
                                    .cursor_pointer()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .overflow_hidden()
                                    .text_xs()
                                    .text_color(rgb(0xc0c0c0))
                                    .when_some(target_image, |el, image| {
                                        el.child(
                                            div()
                                                .flex_1()
                                                .min_h(px(0.))
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    img(image)
                                                        .size_full()
                                                        .object_fit(ObjectFit::ScaleDown),
                                                ),
                                        )
                                    })
                                    .when_some(label, |el, text| {
                                        el.child(
                                            div()
                                                .id((
                                                    ElementId::from(("target-name", r * cols + c)),
                                                    text.clone(),
                                                ))
                                                .flex_shrink_0()
                                                .px_1()
                                                .pb_1()
                                                .text_xs()
                                                .text_color(rgb(0xc0c0c0))
                                                .child(text),
                                        )
                                    })
                                    .when_some(connector_caption, |el, caption| {
                                        let graphic = if vertical_connector {
                                            div()
                                                .h(relative(0.65))
                                                .w(px(16.0))
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .child(div().h(px(1.0)).w(px(16.0)).bg(connector_color))
                                                .child(div().flex_1().w(px(2.0)).bg(connector_color))
                                                .child(div().h(px(1.0)).w(px(16.0)).bg(connector_color))
                                        } else {
                                            div()
                                                .w(relative(0.65))
                                                .h(px(16.0))
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .child(div().w(px(1.0)).h(px(16.0)).bg(connector_color))
                                                .child(div().flex_1().h(px(2.0)).bg(connector_color))
                                                .child(div().w(px(1.0)).h(px(16.0)).bg(connector_color))
                                        };
                                        el.child(
                                            div()
                                                .id((ElementId::from(("connector", r * cols + c)), caption.clone()))
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .justify_center()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0xc0c0c0))
                                                        .child(caption),
                                                )
                                                .child(graphic),
                                        )
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.commit_pending_prop_edits(cx);
                                            this.units_menu_open = false;
                                            if this.targets_sidebar == Some((r, c)) {
                                                this.targets_sidebar = None;
                                            } else {
                                                this.targets_sidebar = Some((r, c));
                                            }
                                            cx.notify();
                                        }),
                                    )
                            }))
                    })),
            );

        div()
            .id("grid-pane")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .m_2()
            .overflow_hidden()
            .child(
                Self::edge("grid-top-edge", GridSide::Top, cx)
                    .w_full()
                    .h(px(22.0))
                    .mb(px(4.0)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        Self::edge("grid-left-edge", GridSide::Left, cx)
                            .w(px(22.0))
                            .h_full()
                            .mr(px(4.0)),
                    )
                    .child(cells_el)
                    .child(
                        Self::edge("grid-right-edge", GridSide::Right, cx)
                            .w(px(22.0))
                            .h_full()
                            .ml(px(4.0)),
                    ),
            )
            .child(
                Self::edge("grid-bottom-edge", GridSide::Bottom, cx)
                    .w_full()
                    .h(px(22.0))
                    .mt(px(4.0)),
            )
    }

    /// Horizontal button bar container shared by Layout and Events.
    fn render_button_bar(
        bar_id: &'static str,
        buttons: impl IntoIterator<Item = Stateful<Div>>,
    ) -> impl IntoElement {
        div()
            .id(bar_id)
            .w_full()
            .flex_none()
            .px_2()
            .py_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .children(buttons)
    }

    fn render_bar_button(
        id: impl Into<ElementId>,
        label: impl Into<String>,
    ) -> Stateful<Div> {
        let line = rgb(0x505050);
        div()
            .id(id)
            .cursor_pointer()
            .flex_none()
            .px_3()
            .py(px(4.0))
            .rounded_sm()
            .border_1()
            .border_color(line)
            .bg(rgb(0x2d2d2d))
            .hover(|s| s.bg(rgb(0x3a3a3a)))
            .text_xs()
            .text_color(rgb(0xc0c0c0))
            .child(label.into())
    }

    /// Layout tools: type button_bar below the grid, properties strip under the bar.
    /// Both strips always reserve space (invisible placeholders when hidden) so the
    /// design grid does not rescale when selection changes.
    fn render_layout_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target_names = self.target_names.clone();
        let line = rgb(0x505050);
        let hover = rgb(0x3a3a3a);
        let selected = rgb(0x4a4a4a);
        let selected_cell = self.targets_sidebar;
        let show_bar = selected_cell.is_some();
        let distance_edit = self.distance_edit.clone();
        let name_edit = self.name_edit.clone();
        let units_menu_open = self.units_menu_open;

        let cell = selected_cell.and_then(|(row, col)| {
            self.cells.get(self.cell_index(row, col)).cloned()
        });
        let show_properties = cell.as_ref().is_some_and(|c| !c.is_empty());

        let button_bar = if show_bar {
            let mut buttons = Vec::new();
            buttons.push(
                Self::render_bar_button("target-empty", "Empty").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some((row, col)) = this.targets_sidebar {
                            this.assign_cell(row, col, CellKind::Empty);
                            this.targets_sidebar = None;
                            this.distance_edit = None;
                            this.name_edit = None;
                            this.units_menu_open = false;
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                ),
            );
            buttons.push(
                Self::render_bar_button("target-connector", "Connector").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some((row, col)) = this.targets_sidebar {
                            this.assign_cell(row, col, CellKind::default_connector());
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                ),
            );
            for (i, name) in target_names.into_iter().enumerate() {
                let assign_name = name.clone();
                buttons.push(
                    Self::render_bar_button(("target-item", i as u64), name).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some((row, col)) = this.targets_sidebar {
                                let n = this
                                    .cells
                                    .iter()
                                    .filter(|c| matches!(c, CellKind::Target { .. }))
                                    .count()
                                    + 1;
                                this.assign_cell(
                                    row,
                                    col,
                                    CellKind::Target {
                                        def: assign_name.clone(),
                                        name: format!("{assign_name}-{n}"),
                                    },
                                );
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }),
                    ),
                );
            }
            Self::render_button_bar("layout-button-bar", buttons)
        } else {
            // Invisible spacer matching one bar button so height stays stable.
            Self::render_button_bar(
                "layout-button-bar",
                vec![Self::render_bar_button("layout-bar-spacer", "Empty")],
            )
        };

        let props = match cell.as_ref() {
            Some(CellKind::Connector { distance, units }) if show_properties => {
                let distance = *distance;
                let units = *units;
                let distance_label = distance_edit
                    .clone()
                    .unwrap_or_else(|| format!("{distance}"));
                let editing = distance_edit.is_some();
                div()
                    .id("properties-panel")
                    .px_2()
                    .pb_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .id("properties-title")
                            .text_sm()
                            .text_color(rgb(0xa0a0a0))
                            .child("Properties"),
                    )
                    .child(
                        div()
                            .id("prop-distance-row")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child("Distance"))
                            .child(
                                div()
                                    .id("prop-distance-value")
                                    .cursor_pointer()
                                    .px_2()
                                    .py_0()
                                    .rounded_md()
                                    .bg(if editing { selected } else { rgb(0x252526) })
                                    .min_w(px(64.0))
                                    .text_sm()
                                    .child(distance_label)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            if this.distance_edit.is_none() {
                                                this.commit_pending_prop_edits(cx);
                                                this.distance_edit = Some(format!("{distance}"));
                                                this.units_menu_open = false;
                                                this.focus_handle.focus(window);
                                                cx.notify();
                                            }
                                            cx.stop_propagation();
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("prop-units-row")
                            .relative()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child("Units"))
                            .child(
                                div()
                                    .id("prop-units-value")
                                    .cursor_pointer()
                                    .px_2()
                                    .py_0()
                                    .rounded_md()
                                    .bg(if units_menu_open {
                                        selected
                                    } else {
                                        rgb(0x252526)
                                    })
                                    .min_w(px(64.0))
                                    .text_sm()
                                    .child(units.label())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.units_menu_open = !this.units_menu_open;
                                            cx.stop_propagation();
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .when(units_menu_open, |row| {
                                row.child(
                                    div()
                                        .id("units-menu-popup")
                                        .absolute()
                                        .top(px(28.0))
                                        .left_0()
                                        .min_w(px(120.0))
                                        .bg(rgb(0x2d2d2d))
                                        .border_1()
                                        .border_color(line)
                                        .rounded_md()
                                        .shadow_lg()
                                        .py_1()
                                        .children(LengthUnit::all().into_iter().enumerate().map(
                                            move |(i, unit)| {
                                                let is_active = unit == units;
                                                div()
                                                    .id(("prop-unit", i))
                                                    .cursor_pointer()
                                                    .px_3()
                                                    .py_1()
                                                    .bg(if is_active {
                                                        selected
                                                    } else {
                                                        rgb(0x2d2d2d)
                                                    })
                                                    .hover(|s| s.bg(hover))
                                                    .text_sm()
                                                    .child(unit.label())
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.units_menu_open = false;
                                                            this.set_connector_units(unit, cx);
                                                            cx.stop_propagation();
                                                        }),
                                                    )
                                            },
                                        )),
                                )
                            }),
                    )
            }
            Some(CellKind::Target { name, .. }) if show_properties => {
                let name = name.clone();
                let name_label = name_edit.clone().unwrap_or_else(|| name.clone());
                let editing = name_edit.is_some();
                div()
                    .id("properties-panel")
                    .px_2()
                    .pb_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .id("properties-title")
                            .text_sm()
                            .text_color(rgb(0xa0a0a0))
                            .child("Properties"),
                    )
                    .child(
                        div()
                            .id("prop-name-row")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child("Name"))
                            .child(
                                div()
                                    .id("prop-name-value")
                                    .cursor_pointer()
                                    .px_2()
                                    .py_0()
                                    .rounded_md()
                                    .bg(if editing { selected } else { rgb(0x252526) })
                                    .min_w(px(64.0))
                                    .text_sm()
                                    .child(name_label)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            if this.name_edit.is_none() {
                                                this.commit_pending_prop_edits(cx);
                                                this.name_edit = Some(name.clone());
                                                this.units_menu_open = false;
                                                this.focus_handle.focus(window);
                                                cx.notify();
                                            }
                                            cx.stop_propagation();
                                        }),
                                    ),
                            ),
                    )
            }
            // Invisible placeholder matching the properties strip height.
            _ => div()
                .id("properties-panel")
                .px_2()
                .pb_1()
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .invisible()
                .child(
                    div()
                        .id("properties-title")
                        .text_sm()
                        .text_color(rgb(0xa0a0a0))
                        .child("Properties"),
                ),
        };

        div()
            .id("layout-tools")
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("layout-button-bar-slot")
                    .w_full()
                    .when(!show_bar, |slot| slot.invisible())
                    .child(button_bar),
            )
            .child(props)
    }

    fn render_events_button_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_activate = self.has_card_type(CardType::Activate);
        let has_start_timer = self.has_card_type(CardType::StartTimer);
        let mut buttons = Vec::new();
        for (opt_i, ty) in CardType::all().into_iter().enumerate() {
            let disabled = card_type_button_disabled(ty, has_activate, has_start_timer);
            let mut btn = Self::render_bar_button(("event-type-btn", opt_i as u64), ty.label());
            if disabled {
                // Do not call .hover() again: render_bar_button already sets hover.
                btn = btn
                    .cursor_default()
                    .text_color(rgb(0x606060))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    });
            } else {
                btn = btn.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.create_event_at_cursor(ty, cx);
                        cx.stop_propagation();
                    }),
                );
            }
            buttons.push(btn);
        }
        Self::render_button_bar("events-button-bar", buttons)
    }

    fn render_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let line = rgb(0x505050);
        let pane_bg = rgb(0x252526);
        let accent = rgb(0xc0c0c0);
        let selected = rgb(0xffffff);
        let duration = self.duration_ms.max(1) as f32;
        let cursor_ms = self.cursor_ms;
        let cursor_frac = (cursor_ms as f32 / duration).clamp(0.0, 1.0);
        let position = Self::format_position(cursor_ms);
        let event_times: Vec<u32> = self.events.iter().map(Self::event_ms).collect();
        let editing = self.duration_edit.is_some();
        let start_label = Self::format_position(0);
        let end_label = self
            .duration_edit
            .clone()
            .unwrap_or_else(|| Self::format_position(self.duration_ms));

        div()
            .id("timeline-pane")
            .flex_none()
            .w_full()
            .h(px(100.0))
            .m_2()
            .mt_0()
            .border_1()
            .border_color(line)
            .bg(pane_bg)
            .px_3()
            .py_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("timeline-track-area")
                    .w_full()
                    .h(px(56.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("timeline-start-time")
                            .flex_none()
                            .text_xs()
                            .text_color(rgb(0xffffff))
                            .whitespace_nowrap()
                            .child(start_label),
                    )
                    .child(
                        div()
                            .id("timeline-track-wrap")
                            .relative()
                            .flex_1()
                            .h(px(12.0))
                            // 1px track with end caps: |- -|
                            .child(
                                div()
                                    .id("timeline-track")
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .child(div().w(px(1.0)).h(px(12.0)).bg(accent))
                                    .child(div().flex_1().h(px(1.0)).bg(accent))
                                    .child(div().w(px(1.0)).h(px(12.0)).bg(accent)),
                            )
                            // cursor: single 1px bar; label centered below
                            .child(
                                div()
                                    .id("timeline-cursor")
                                    .absolute()
                                    .top(px(-6.0))
                                    .left(relative(cursor_frac))
                                    .ml(px(-0.5))
                                    .w(px(1.0))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .child(div().w(px(1.0)).h(px(24.0)).bg(rgb(0xffffff)))
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(rgb(0xffffff))
                                            .whitespace_nowrap()
                                            .child(position),
                                    ),
                            )
                            // event markers: filled upward triangles; accent when cursor is on that event
                            .children(event_times.into_iter().enumerate().map(move |(i, t)| {
                                let frac = (t as f32 / duration).clamp(0.0, 1.0);
                                let on_event = t == cursor_ms;
                                let fill = if on_event { selected } else { accent };
                                div()
                                    .id(("timeline-event", i))
                                    .absolute()
                                    .top(px(14.0))
                                    .left(relative(frac))
                                    .ml(px(-3.0))
                                    .w(px(6.0))
                                    .h(px(5.0))
                                    .child(
                                        canvas(
                                            |_, _, _| {},
                                            move |bounds, _, window, _| {
                                                let mut builder = PathBuilder::fill();
                                                let tip = point(
                                                    bounds.origin.x + bounds.size.width / 2.0,
                                                    bounds.origin.y,
                                                );
                                                let bl = point(
                                                    bounds.origin.x,
                                                    bounds.origin.y + bounds.size.height,
                                                );
                                                let br = point(
                                                    bounds.origin.x + bounds.size.width,
                                                    bounds.origin.y + bounds.size.height,
                                                );
                                                builder.move_to(tip);
                                                builder.line_to(bl);
                                                builder.line_to(br);
                                                builder.line_to(tip);
                                                if let Ok(path) = builder.build() {
                                                    window.paint_path(path, fill);
                                                }
                                            },
                                        )
                                        .size_full(),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .id("timeline-end-time")
                            .flex_none()
                            .px_1()
                            .py(px(1.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(if editing { selected } else { line })
                            .bg(if editing { rgb(0x3a3a3a) } else { pane_bg })
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(0xffffff))
                            .whitespace_nowrap()
                            .child(end_label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.duration_edit =
                                        Some(Self::format_position(this.duration_ms));
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }

    fn render_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let line = rgb(0x505050);
        let pane_bg = rgb(0x252526);
        let card_bg = rgb(0x2d2d2d);
        let nested_card_bg = rgb(0x1e1e1e);
        let highlight_bg = rgb(0x3a3a3a);
        let highlight_border = rgb(0xc0c0c0);
        let hover = rgb(0x3a3a3a);
        let selected_bg = rgb(0x4a4a4a);
        let cursor_ms = self.cursor_ms;
        let event_time_edit = self.event_time_edit.clone();
        let event_title_focused = self.event_title_focused;
        let event_card_focused = self.event_card_focused;
        let card_type_menu_open = self.card_type_menu_open;
        let event_card_name_edit = self.event_card_name_edit.clone();
        let timer_name_menu_open = self.timer_name_menu_open;
        let timer_names = self.start_timer_names();
        let activate_labels = self.layout_activate_labels();
        let deactivate_labels = self.activate_names();

        let mut event_els = Vec::new();
        for (i, event) in self.events.iter().enumerate() {
            let t = Self::event_ms(event);
            let selected = t == cursor_ms;
            let editing_time = matches!(
                event_time_edit.as_ref(),
                Some((idx, _)) if *idx == i
            );
            let title_focused = event_title_focused == Some(i) && !editing_time;
            let title_label = if let Some((idx, draft)) = event_time_edit.as_ref() {
                if *idx == i {
                    draft.clone()
                } else {
                    Self::format_position(t)
                }
            } else {
                Self::format_position(t)
            };

            let mut card_els = Vec::new();
            for (card_i, card) in event.cards.iter().enumerate() {
                let card_type = card.card_type;
                let card_name = card.name.clone();
                let card_focused = event_card_focused == Some((i, card_i));
                let menu_open = card_type_menu_open == Some((i, card_i));
                let timer_menu_open = timer_name_menu_open == Some((i, card_i));
                let name_editing = matches!(
                    event_card_name_edit.as_ref(),
                    Some((ei, ci, _)) if *ei == i && *ci == card_i
                );
                let name_label = if let Some((ei, ci, draft)) = event_card_name_edit.as_ref() {
                    if *ei == i && *ci == card_i {
                        draft.clone()
                    } else if card_name.is_empty() {
                        "Name".to_string()
                    } else {
                        card_name.clone()
                    }
                } else if card_name.is_empty() {
                    "Name".to_string()
                } else {
                    card_name.clone()
                };
                let mut type_opts = Vec::new();
                for (opt_i, ty) in CardType::all().into_iter().enumerate() {
                    let is_active = ty == card_type;
                    type_opts.push(
                        div()
                            .id((
                                "card-type-opt",
                                i as u64 * 1000 + card_i as u64 * 10 + opt_i as u64,
                            ))
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .bg(if is_active {
                                selected_bg
                            } else {
                                rgb(0x2d2d2d)
                            })
                            .hover(|s| s.bg(hover))
                            .text_xs()
                            .text_color(rgb(0xc0c0c0))
                            .child(ty.label())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let default_timer_name = if ty == CardType::StartTimer {
                                        Some(this.next_timer_name())
                                    } else {
                                        None
                                    };
                                    if let Some(card) = this
                                        .events
                                        .get_mut(i)
                                        .and_then(|e| e.cards.get_mut(card_i))
                                    {
                                        card.card_type = ty;
                                        if let Some(timer_name) = default_timer_name {
                                            if card.name.trim().is_empty() {
                                                card.name = timer_name;
                                            }
                                        }
                                    }
                                    this.card_type_menu_open = None;
                                    this.timer_name_menu_open = None;
                                    this.device_target_menu_open = None;
                                    if !matches!(ty, CardType::StartTimer)
                                        && matches!(
                                            this.event_card_name_edit.as_ref(),
                                            Some((ei, ci, _)) if *ei == i && *ci == card_i
                                        )
                                    {
                                        this.event_card_name_edit = None;
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    );
                }

                let mut timer_name_opts = Vec::new();
                for (opt_i, name) in timer_names.iter().cloned().enumerate() {
                    let is_active = name == card_name;
                    let name_for_set = name.clone();
                    timer_name_opts.push(
                        div()
                            .id((
                                "timer-name-opt",
                                i as u64 * 1000 + card_i as u64 * 10 + opt_i as u64,
                            ))
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .bg(if is_active {
                                selected_bg
                            } else {
                                rgb(0x2d2d2d)
                            })
                            .hover(|s| s.bg(hover))
                            .text_xs()
                            .text_color(rgb(0xc0c0c0))
                            .child(name)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(card) = this
                                        .events
                                        .get_mut(i)
                                        .and_then(|e| e.cards.get_mut(card_i))
                                    {
                                        card.name = name_for_set.clone();
                                    }
                                    this.timer_name_menu_open = None;
                                    this.device_target_menu_open = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    );
                }

                let mut activate_name_opts = Vec::new();
                for (opt_i, opt_name) in activate_labels.iter().cloned().enumerate() {
                    let is_active = opt_name == card_name;
                    let name_for_set = opt_name.clone();
                    activate_name_opts.push(
                        div()
                            .id((
                                "activate-name-opt",
                                i as u64 * 1000 + card_i as u64 * 10 + opt_i as u64,
                            ))
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .bg(if is_active {
                                selected_bg
                            } else {
                                rgb(0x2d2d2d)
                            })
                            .hover(|s| s.bg(hover))
                            .text_xs()
                            .text_color(rgb(0xc0c0c0))
                            .child(opt_name)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(card) = this
                                        .events
                                        .get_mut(i)
                                        .and_then(|e| e.cards.get_mut(card_i))
                                    {
                                        card.name = name_for_set.clone();
                                    }
                                    this.timer_name_menu_open = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    );
                }

                let mut deactivate_name_opts = Vec::new();
                for (opt_i, opt_name) in deactivate_labels.iter().cloned().enumerate() {
                    let is_active = opt_name == card_name;
                    let name_for_set = opt_name.clone();
                    deactivate_name_opts.push(
                        div()
                            .id((
                                "deactivate-name-opt",
                                i as u64 * 1000 + card_i as u64 * 10 + opt_i as u64,
                            ))
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .bg(if is_active {
                                selected_bg
                            } else {
                                rgb(0x2d2d2d)
                            })
                            .hover(|s| s.bg(hover))
                            .text_xs()
                            .text_color(rgb(0xc0c0c0))
                            .child(opt_name)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(card) = this
                                        .events
                                        .get_mut(i)
                                        .and_then(|e| e.cards.get_mut(card_i))
                                    {
                                        card.name = name_for_set.clone();
                                    }
                                    this.timer_name_menu_open = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    );
                }

                card_els.push(
                    div()
                        .id(("event-card-item", i as u64 * 1000 + card_i as u64))
                        .relative()
                        .flex_none()
                        .w_full()
                        .border_1()
                        .border_color(if card_focused {
                            rgb(0xffffff)
                        } else {
                            line
                        })
                        .bg(if card_focused {
                            rgb(0x2a2a2a)
                        } else {
                            nested_card_bg
                        })
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id(("event-card-type", i as u64 * 1000 + card_i as u64))
                                .cursor_pointer()
                                .w_full()
                                .px_2()
                                .pr(px(22.0))
                                .py(px(2.0))
                                .bg(if menu_open {
                                    selected_bg
                                } else {
                                    rgb(0x252526)
                                })
                                .text_xs()
                                .text_color(rgb(0xc0c0c0))
                                .child(card_type.label())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.commit_event_card_name_edit(cx);
                                        this.cursor_ms = t;
                                        this.event_time_edit = None;
                                        this.event_title_focused = None;
                                        this.event_card_focused = Some((i, card_i));
                                        this.timer_name_menu_open = None;
                                        this.device_target_menu_open = None;
                                        this.card_type_menu_open =
                                            if this.card_type_menu_open == Some((i, card_i))
                                            {
                                                None
                                            } else {
                                                Some((i, card_i))
                                            };
                                        this.focus_handle.focus(window);
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        )
                        .when(menu_open, |row| {
                            row.child(
                                div()
                                    .id((
                                        "card-type-select",
                                        i as u64 * 1000 + card_i as u64,
                                    ))
                                    .w_full()
                                    .border_t(px(1.0))
                                    .border_color(line)
                                    .bg(rgb(0x2d2d2d))
                                    .py_1()
                                    .children(type_opts),
                            )
                        })
                        .when(card_type == CardType::StartTimer, |row| {
                            let name_label = name_label.clone();
                            row.child(
                                div()
                                    .id(("event-card-name-row", i as u64 * 1000 + card_i as u64))
                                    .border_t(px(1.0))
                                    .border_color(line)
                                    .px_2()
                                    .py(px(2.0))
                                    .bg(if name_editing {
                                        selected_bg
                                    } else {
                                        rgb(0x252526)
                                    })
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .id((
                                                "event-card-name-value",
                                                i as u64 * 1000 + card_i as u64,
                                            ))
                                            .min_w(px(48.0))
                                            .text_xs()
                                            .text_color(if card_name.is_empty() && !name_editing {
                                                rgb(0x707070)
                                            } else {
                                                rgb(0xc0c0c0)
                                            })
                                            .child(name_label),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.cursor_ms = t;
                                            this.event_time_edit = None;
                                            this.event_title_focused = None;
                                            this.event_card_focused = Some((i, card_i));
                                            this.card_type_menu_open = None;
                                            this.timer_name_menu_open = None;
                                            this.device_target_menu_open = None;
                                            if this.event_card_name_edit.is_none()
                                                || !matches!(
                                                    this.event_card_name_edit.as_ref(),
                                                    Some((ei, ci, _))
                                                        if *ei == i && *ci == card_i
                                                )
                                            {
                                                this.commit_event_card_name_edit(cx);
                                                let current = this
                                                    .events
                                                    .get(i)
                                                    .and_then(|e| e.cards.get(card_i))
                                                    .map(|c| c.name.clone())
                                                    .unwrap_or_default();
                                                this.event_card_name_edit =
                                                    Some((i, card_i, current));
                                            }
                                            this.focus_handle.focus(window);
                                            cx.stop_propagation();
                                            cx.notify();
                                        }),
                                    ),
                            )
                        })
                        .when(card_type == CardType::StopTimer, |row| {
                            let name_label = name_label.clone();
                            row.child(
                                div()
                                    .id((
                                        "event-card-timer-name-row",
                                        i as u64 * 1000 + card_i as u64,
                                    ))
                                    .border_t(px(1.0))
                                    .border_color(line)
                                    .px_2()
                                    .py(px(2.0))
                                    .bg(if timer_menu_open {
                                        selected_bg
                                    } else {
                                        rgb(0x252526)
                                    })
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xa0a0a0))
                                            .child("Name"),
                                    )
                                    .child(
                                        div()
                                            .id((
                                                "event-card-timer-name-value",
                                                i as u64 * 1000 + card_i as u64,
                                            ))
                                            .cursor_pointer()
                                            .min_w(px(48.0))
                                            .text_xs()
                                            .text_color(if card_name.is_empty() {
                                                rgb(0x707070)
                                            } else {
                                                rgb(0xc0c0c0)
                                            })
                                            .child(name_label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.commit_event_card_name_edit(cx);
                                                    this.cursor_ms = t;
                                                    this.event_time_edit = None;
                                                    this.event_title_focused = None;
                                                    this.event_card_focused = Some((i, card_i));
                                                    this.card_type_menu_open = None;
                                                    this.timer_name_menu_open =
                                                        if this.timer_name_menu_open
                                                            == Some((i, card_i))
                                                        {
                                                            None
                                                        } else {
                                                            Some((i, card_i))
                                                        };
                                                    this.focus_handle.focus(window);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            )
                            .when(timer_menu_open, |row| {
                                row.child(
                                    div()
                                        .id((
                                            "timer-name-select",
                                            i as u64 * 1000 + card_i as u64,
                                        ))
                                        .w_full()
                                        .border_t(px(1.0))
                                        .border_color(line)
                                        .bg(rgb(0x2d2d2d))
                                        .py_1()
                                        .children(timer_name_opts),
                                )
                            })
                        })
                        .when(card_type == CardType::Activate, |row| {
                            let name_label = name_label.clone();
                            row.child(
                                div()
                                    .id((
                                        "event-card-activate-name-row",
                                        i as u64 * 1000 + card_i as u64,
                                    ))
                                    .border_t(px(1.0))
                                    .border_color(line)
                                    .px_2()
                                    .py(px(2.0))
                                    .bg(if timer_menu_open {
                                        selected_bg
                                    } else {
                                        rgb(0x252526)
                                    })
                                    .child(
                                        div()
                                            .id((
                                                "event-card-activate-name-value",
                                                i as u64 * 1000 + card_i as u64,
                                            ))
                                            .cursor_pointer()
                                            .min_w(px(48.0))
                                            .text_xs()
                                            .text_color(if card_name.is_empty() {
                                                rgb(0x707070)
                                            } else {
                                                rgb(0xc0c0c0)
                                            })
                                            .child(name_label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.commit_event_card_name_edit(cx);
                                                    this.cursor_ms = t;
                                                    this.event_time_edit = None;
                                                    this.event_title_focused = None;
                                                    this.event_card_focused = Some((i, card_i));
                                                    this.card_type_menu_open = None;
                                                    this.timer_name_menu_open =
                                                        if this.timer_name_menu_open
                                                            == Some((i, card_i))
                                                        {
                                                            None
                                                        } else {
                                                            Some((i, card_i))
                                                        };
                                                    this.focus_handle.focus(window);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            )
                            .when(timer_menu_open, |row| {
                                row.child(
                                    div()
                                        .id((
                                            "activate-name-select",
                                            i as u64 * 1000 + card_i as u64,
                                        ))
                                        .w_full()
                                        .border_t(px(1.0))
                                        .border_color(line)
                                        .bg(rgb(0x2d2d2d))
                                        .py_1()
                                        .children(activate_name_opts),
                                )
                            })
                        })
                        .when(card_type == CardType::Deactivate, |row| {
                            let name_label = name_label.clone();
                            row.child(
                                div()
                                    .id((
                                        "event-card-deactivate-name-row",
                                        i as u64 * 1000 + card_i as u64,
                                    ))
                                    .border_t(px(1.0))
                                    .border_color(line)
                                    .px_2()
                                    .py(px(2.0))
                                    .bg(if timer_menu_open {
                                        selected_bg
                                    } else {
                                        rgb(0x252526)
                                    })
                                    .child(
                                        div()
                                            .id((
                                                "event-card-deactivate-name-value",
                                                i as u64 * 1000 + card_i as u64,
                                            ))
                                            .cursor_pointer()
                                            .min_w(px(48.0))
                                            .text_xs()
                                            .text_color(if card_name.is_empty() {
                                                rgb(0x707070)
                                            } else {
                                                rgb(0xc0c0c0)
                                            })
                                            .child(name_label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.commit_event_card_name_edit(cx);
                                                    this.cursor_ms = t;
                                                    this.event_time_edit = None;
                                                    this.event_title_focused = None;
                                                    this.event_card_focused = Some((i, card_i));
                                                    this.card_type_menu_open = None;
                                                    this.timer_name_menu_open =
                                                        if this.timer_name_menu_open
                                                            == Some((i, card_i))
                                                        {
                                                            None
                                                        } else {
                                                            Some((i, card_i))
                                                        };
                                                    this.focus_handle.focus(window);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            )
                            .when(timer_menu_open, |row| {
                                row.child(
                                    div()
                                        .id((
                                            "deactivate-name-select",
                                            i as u64 * 1000 + card_i as u64,
                                        ))
                                        .w_full()
                                        .border_t(px(1.0))
                                        .border_color(line)
                                        .bg(rgb(0x2d2d2d))
                                        .py_1()
                                        .children(deactivate_name_opts),
                                )
                            })
                        })
                        .child(
                            div()
                                .id(("event-card-close", i as u64 * 1000 + card_i as u64))
                                .absolute()
                                .top(px(2.0))
                                .right(px(2.0))
                                .occlude()
                                .w(px(18.0))
                                .h(px(18.0))
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(highlight_bg))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(0xc0c0c0))
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.cursor_ms = t;
                                        this.remove_card_from_event(i, card_i, cx);
                                        cx.stop_propagation();
                                    }),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                // Do not clear a name edit started by a child on this click.
                                if !matches!(
                                    this.event_card_name_edit.as_ref(),
                                    Some((ei, ci, _)) if *ei == i && *ci == card_i
                                ) {
                                    this.commit_event_card_name_edit(cx);
                                }
                                this.cursor_ms = t;
                                this.event_time_edit = None;
                                this.event_title_focused = None;
                                this.event_card_focused = Some((i, card_i));
                                this.card_type_menu_open = None;
                                this.timer_name_menu_open = None;
                                this.device_target_menu_open = None;
                                this.focus_handle.focus(window);
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        ),
                );
            }

            event_els.push(
                div()
                    .id(("event-pane", i))
                    .flex_none()
                    .min_w(px(200.0))
                    .w(px(200.0))
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { highlight_border } else { line })
                    .bg(if selected { highlight_bg } else { card_bg })
                    .cursor_pointer()
                    .hover(|s| s.bg(highlight_bg))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id(("event-title", i))
                            .when(editing_time || title_focused, |s| {
                                s.px_1()
                                    .py(px(1.0))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(0xffffff))
                                    .bg(if editing_time {
                                        rgb(0x3a3a3a)
                                    } else {
                                        rgb(0x2a2a2a)
                                    })
                            })
                            .text_sm()
                            .text_color(rgb(0xffffff))
                            .child(title_label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.cursor_ms = t;
                                    this.event_time_edit = None;
                                    this.commit_event_card_name_edit(cx);
                                    this.event_title_focused = Some(i);
                                    this.event_card_focused = None;
                                    this.card_type_menu_open = None;
                                    this.timer_name_menu_open = None;
                                    this.device_target_menu_open = None;
                                    this.focus_handle.focus(window);
                                    cx.notify();
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id(("event-cards", i))
                            .flex_none()
                            .w_full()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_start()
                            .gap_1()
                            .children(card_els),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.cursor_ms = t;
                            this.commit_event_card_name_edit(cx);
                            this.event_title_focused = None;
                            this.event_card_focused = None;
                            this.event_time_edit = None;
                            this.card_type_menu_open = None;
                            this.timer_name_menu_open = None;
                            this.device_target_menu_open = None;
                            this.focus_handle.focus(window);
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.cursor_ms = t;
                            this.remove_focused_event(cx);
                        }),
                    ),
            );
        }

        div()
            .id("events-pane")
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .m_2()
            .mt_0()
            .border_1()
            .border_color(line)
            .bg(pane_bg)
            .flex()
            .flex_row()
            .flex_wrap()
            .items_start()
            .p_2()
            .gap_2()
            .overflow_x_hidden()
            .children(event_els)
    }

    fn refresh_flash_boards(&mut self, cx: &mut Context<Self>) {
        if self.flash_busy {
            return;
        }
        self.flash_busy = true;
        self.flash_project_menu_open = false;
        self.flash_status = "Scanning for boards…".to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let boards = cx
                .background_spawn(async move {
                    detect_boards(10)
                        .into_iter()
                        .map(|(port, chip)| FlashBoard { port, chip })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let n = boards.len();
                this.flash_boards = boards;
                this.flash_busy = false;
                this.flash_status = if n == 0 {
                    "No boards found. Plug in a device and Refresh.".to_string()
                } else {
                    format!("{n} board(s) found.")
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn flash_board_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.flash_busy {
            return;
        }
        let Some(board) = self.flash_boards.get(index).cloned() else {
            return;
        };
        let project = self.flash_project;
        self.flash_busy = true;
        self.flash_project_menu_open = false;
        self.flash_status = format!(
            "Flashing {} ({}) with {}…",
            board.port,
            board.chip.name(),
            project.label()
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_spawn(async move { flash_firmware(project, &board.port, board.chip) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flash_busy = false;
                this.flash_status = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn render_devices(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let line = rgb(0x505050);
        let pane_bg = rgb(0x252526);
        let card_bg = rgb(0x2d2d2d);
        let nested_card_bg = rgb(0x1e1e1e);
        let selected_bg = rgb(0x094771);
        let hover = rgb(0x3a3a3a);
        let target_names = self.layout_target_names();
        let menu_open_idx = self.device_target_menu_open;
        let flash_busy = self.flash_busy;
        let project_menu_open = self.flash_project_menu_open;
        let flash_project = self.flash_project;
        let flash_status = self.flash_status.clone();

        let mut project_opts = Vec::new();
        for (opt_i, opt) in FlashProject::all().into_iter().enumerate() {
            let is_active = opt == flash_project;
            project_opts.push(
                div()
                    .id(("flash-project-opt", opt_i))
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .bg(if is_active { selected_bg } else { rgb(0x2d2d2d) })
                    .hover(|s| s.bg(hover))
                    .text_xs()
                    .text_color(rgb(0xc0c0c0))
                    .child(opt.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.flash_project = opt;
                            this.flash_project_menu_open = false;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }

        let mut board_els = Vec::new();
        for (i, board) in self.flash_boards.iter().enumerate() {
            let port = board.port.clone();
            let chip_name = board.chip.name().to_string();
            board_els.push(
                div()
                    .id(("flash-board-card", i))
                    .flex_none()
                    .min_w(px(220.0))
                    .w(px(220.0))
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(line)
                    .bg(card_bg)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xffffff))
                            .child(chip_name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xa0a0a0))
                            .child(port),
                    )
                    .child(
                        div()
                            .id(("flash-board-btn", i))
                            .cursor_pointer()
                            .mt_1()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(if flash_busy {
                                rgb(0x3a3a3a)
                            } else {
                                selected_bg
                            })
                            .hover(|s| {
                                if flash_busy {
                                    s
                                } else {
                                    s.bg(rgb(0x0e639c))
                                }
                            })
                            .text_xs()
                            .text_color(rgb(0xffffff))
                            .child(if flash_busy { "Busy…" } else { "Flash" })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.flash_board_at(i, cx);
                                    cx.stop_propagation();
                                }),
                            ),
                    ),
            );
        }

        let mut card_els = Vec::new();
        for (i, device) in self.devices.iter().enumerate() {
            let target_name = device.target_name.clone();
            let name_label = if target_name.is_empty() {
                "—".to_string()
            } else {
                target_name.clone()
            };
            let menu_open = menu_open_idx == Some(i);

            let mut name_opts = Vec::new();
            for (opt_i, opt_name) in target_names.iter().cloned().enumerate() {
                let is_active = opt_name == target_name;
                let name_for_set = opt_name.clone();
                name_opts.push(
                    div()
                        .id(("device-target-name-opt", i as u64 * 100 + opt_i as u64))
                        .cursor_pointer()
                        .px_3()
                        .py_1()
                        .bg(if is_active { selected_bg } else { rgb(0x2d2d2d) })
                        .hover(|s| s.bg(hover))
                        .text_xs()
                        .text_color(rgb(0xc0c0c0))
                        .child(opt_name)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if let Some(device) = this.devices.get_mut(i) {
                                    device.target_name = name_for_set.clone();
                                }
                                this.device_target_menu_open = None;
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                );
            }

            card_els.push(
                div()
                    .id(("device-card", i))
                    .flex_none()
                    .min_w(px(200.0))
                    .w(px(200.0))
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(line)
                    .bg(card_bg)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xffffff))
                            .child(format!("Device {}", i + 1)),
                    )
                    .child(
                        div()
                            .id(("device-card-props", i))
                            .w_full()
                            .border_1()
                            .border_color(line)
                            .bg(nested_card_bg)
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id(("device-card-target-name-row", i))
                                    .px_2()
                                    .py(px(2.0))
                                    .bg(if menu_open {
                                        selected_bg
                                    } else {
                                        rgb(0x252526)
                                    })
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xa0a0a0))
                                            .child("Name"),
                                    )
                                    .child(
                                        div()
                                            .id(("device-card-target-name-value", i))
                                            .cursor_pointer()
                                            .min_w(px(48.0))
                                            .text_xs()
                                            .text_color(if target_name.is_empty() {
                                                rgb(0x707070)
                                            } else {
                                                rgb(0xc0c0c0)
                                            })
                                            .child(name_label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.flash_project_menu_open = false;
                                                    this.device_target_menu_open =
                                                        if this.device_target_menu_open == Some(i)
                                                        {
                                                            None
                                                        } else {
                                                            Some(i)
                                                        };
                                                    this.focus_handle.focus(window);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            )
                            .when(menu_open, |row| {
                                row.child(
                                    div()
                                        .id(("device-target-name-select", i))
                                        .w_full()
                                        .border_t(px(1.0))
                                        .border_color(line)
                                        .bg(rgb(0x2d2d2d))
                                        .py_1()
                                        .children(name_opts),
                                )
                            }),
                    ),
            );
        }

        div()
            .id("devices-view")
            .flex_1()
            .w_full()
            .m_2()
            .mt_0()
            .border_1()
            .border_color(line)
            .bg(pane_bg)
            .flex()
            .flex_col()
            .p_3()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xffffff))
                    .child("Flash Devices"),
            )
            .child(
                div()
                    .id("flash-toolbar")
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("flash-project-select")
                            .relative()
                            .child(
                                div()
                                    .id("flash-project-value")
                                    .cursor_pointer()
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(line)
                                    .bg(if project_menu_open {
                                        selected_bg
                                    } else {
                                        rgb(0x2d2d2d)
                                    })
                                    .text_xs()
                                    .text_color(rgb(0xc0c0c0))
                                    .child(format!("Project: {}", flash_project.label()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            if this.flash_busy {
                                                return;
                                            }
                                            this.device_target_menu_open = None;
                                            this.flash_project_menu_open =
                                                !this.flash_project_menu_open;
                                            this.focus_handle.focus(window);
                                            cx.stop_propagation();
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .when(project_menu_open, |row| {
                                row.child(
                                    div()
                                        .id("flash-project-menu")
                                        .absolute()
                                        .top_full()
                                        .left_0()
                                        .mt_1()
                                        .min_w(px(120.0))
                                        .border_1()
                                        .border_color(line)
                                        .bg(rgb(0x2d2d2d))
                                        .py_1()
                                        .children(project_opts),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("flash-refresh")
                            .cursor_pointer()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(if flash_busy {
                                rgb(0x3a3a3a)
                            } else {
                                rgb(0x3a3a3a)
                            })
                            .hover(|s| {
                                if flash_busy {
                                    s
                                } else {
                                    s.bg(rgb(0x505050))
                                }
                            })
                            .text_xs()
                            .text_color(rgb(0xffffff))
                            .child(if flash_busy { "Working…" } else { "Refresh" })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.refresh_flash_boards(cx);
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(0xa0a0a0))
                            .child(flash_status),
                    ),
            )
            .child(
                div()
                    .id("flash-board-cards")
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_start()
                    .gap_2()
                    .children(board_els),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(rgb(0xffffff))
                    .child("Devices"),
            )
            .child(
                div()
                    .id("devices-cards")
                    .flex_1()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_start()
                    .gap_2()
                    .children(card_els),
            )
    }
}

impl Render for Designer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("designer-root")
            .track_focus(&self.focus_handle)
            .key_context("Timeline")
            .on_action(cx.listener(Self::prev_event))
            .on_action(cx.listener(Self::next_event))
            .on_action(cx.listener(Self::nudge_cursor_back_1s))
            .on_action(cx.listener(Self::nudge_cursor_forward_1s))
            .on_action(cx.listener(Self::nudge_cursor_back_100ms))
            .on_action(cx.listener(Self::nudge_cursor_forward_100ms))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::save_file))
            .on_action(cx.listener(Self::save_file_as))
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e1e))
            .size_full()
            .font_family("JetBrainsMono Nerd Font")
            .text_color(rgb(0xffffff))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .p_2()
                    .gap_2()
                    .child(
                        div()
                            .id("hamburger")
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .hover(|s| s.bg(rgb(0x3a3a3a)))
                            .child("☰")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.menu_open = !this.menu_open;
                                    cx.notify();
                                }),
                            ),
                    )
                    .when(self.view == AppView::Layout, |this| {
                        this.child(
                            div()
                                .id("layout-title")
                                .px_1()
                                .py_1()
                                .text_sm()
                                .text_color(rgb(0xffffff))
                                .child(format!("Layout - {}", self.layout_display_name())),
                        )
                    })
                    .when(self.view == AppView::Events, |this| {
                        this.child(
                            div()
                                .id("events-title")
                                .px_1()
                                .py_1()
                                .text_sm()
                                .text_color(rgb(0xffffff))
                                .child(format!("Events - {}", self.layout_display_name())),
                        )
                    }),
            )
            .when(self.view == AppView::Layout, |this| {
                this.child(self.render_grid(cx))
                    .child(self.render_layout_tools(cx))
            })
            .when(self.view == AppView::Events, |this| {
                this.child(self.render_timeline(cx))
                    .child(self.render_events_button_bar(cx))
                    .child(self.render_events(cx))
            })
            .when(self.view == AppView::Devices, |this| {
                this.child(self.render_devices(cx))
            })
            .when(self.menu_open, |this| {
                let view = self.view;
                this.child(
                    div()
                        .id("menu-popup")
                        .absolute()
                        .top(px(40.0))
                        .left(px(8.0))
                        .min_w(px(220.0))
                        .bg(rgb(0x2d2d2d))
                        .border_1()
                        .border_color(rgb(0x505050))
                        .rounded_md()
                        .shadow_lg()
                        .py_1()
                        .child(
                            div()
                                .id("menu-open")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .flex()
                                .flex_row()
                                .justify_between()
                                .gap_6()
                                .child("Open")
                                .child(
                                    div()
                                        .text_color(rgb(0xa0a0a0))
                                        .child("Ctrl+O"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.open_file(&OpenFile, window, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("menu-save")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .flex()
                                .flex_row()
                                .justify_between()
                                .gap_6()
                                .child("Save")
                                .child(
                                    div()
                                        .text_color(rgb(0xa0a0a0))
                                        .child("Ctrl+S"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.save_file(&SaveFile, window, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("menu-save-as")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .flex()
                                .flex_row()
                                .justify_between()
                                .gap_6()
                                .child("Save As…")
                                .child(
                                    div()
                                        .text_color(rgb(0xa0a0a0))
                                        .child("Ctrl+Shift+S"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.save_file_as(&SaveFileAs, window, cx);
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("menu-separator")
                                .mx_2()
                                .my_1()
                                .h(px(1.0))
                                .bg(rgb(0x505050)),
                        )
                        .child(
                            div()
                                .id("menu-layout")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .when(view == AppView::Layout, |s| s.bg(rgb(0x3a3a3a)))
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .child("Layout")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.commit_event_card_name_edit(cx);
                                        this.event_time_edit = None;
                                        this.event_title_focused = None;
                                        this.event_card_focused = None;
                                        this.card_type_menu_open = None;
                                        this.timer_name_menu_open = None;
                                        this.device_target_menu_open = None;
                                        this.view = AppView::Layout;
                                        this.menu_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("menu-events")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .when(view == AppView::Events, |s| s.bg(rgb(0x3a3a3a)))
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .child("Events")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.commit_pending_prop_edits(cx);
                                        this.commit_event_card_name_edit(cx);
                                        this.device_target_menu_open = None;
                                        this.view = AppView::Events;
                                        this.menu_open = false;
                                        this.targets_sidebar = None;
                                        this.units_menu_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("menu-devices")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .when(view == AppView::Devices, |s| s.bg(rgb(0x3a3a3a)))
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .child("Devices")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.commit_pending_prop_edits(cx);
                                        this.commit_event_card_name_edit(cx);
                                        this.event_time_edit = None;
                                        this.event_title_focused = None;
                                        this.event_card_focused = None;
                                        this.card_type_menu_open = None;
                                        this.timer_name_menu_open = None;
                                        this.device_target_menu_open = None;
                                        this.flash_project_menu_open = false;
                                        this.view = AppView::Devices;
                                        this.menu_open = false;
                                        this.targets_sidebar = None;
                                        this.units_menu_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("quit")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .hover(|s| s.bg(rgb(0x3a3a3a)))
                                .child("Quit")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.quit();
                                }),
                        ),
                )
            })
    }
}

fn firmware_dir(project: FlashProject) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(project.dir_name())
}

/// Build (if needed) then flash release firmware for `project` onto `port`.
fn flash_firmware(project: FlashProject, port: &str, chip: Chip) -> String {
    flash_firmware_at(&firmware_dir(project), project, port, chip)
}

/// Same as [`flash_firmware`], but uses an explicit project directory (for tests).
fn flash_firmware_at(dir: &Path, project: FlashProject, port: &str, chip: Chip) -> String {
    if !dir.join("Cargo.toml").is_file() {
        return format!("Firmware project missing at {}.", dir.display());
    }
    let fw = Framework::Hal;
    let elf = elf_path(dir, chip, fw);
    if !elf.is_file() {
        let mut cmd = build_command(dir, chip, fw);
        cmd.stdin(std::process::Stdio::null());
        match cmd.output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let tail = err.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                return format!(
                    "Build failed for {} / {}: {}",
                    project.label(),
                    chip.name(),
                    if tail.is_empty() { out.status.to_string() } else { tail }
                );
            }
            Err(e) => return format!("Failed to start build: {e}"),
        }
        if !elf.is_file() {
            return format!("Build finished but ELF missing at {}.", elf.display());
        }
    }
    let mut cmd = flash_command(dir, chip, fw, Some(port));
    cmd.stdin(std::process::Stdio::null());
    match cmd.output() {
        Ok(out) if out.status.success() => {
            format!("Flashed {} ({}) with {}.", port, chip.name(), project.label())
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let out_s = String::from_utf8_lossy(&out.stdout);
            let text = if err.trim().is_empty() { out_s } else { err };
            let tail = text.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            format!(
                "Flash failed on {}: {}",
                port,
                if tail.is_empty() { out.status.to_string() } else { tail }
            )
        }
        Err(e) => format!("Failed to start espflash: {e}"),
    }
}

/// Whether an Events bar card-type button should be disabled.
/// Deactivate needs an Activate card; StopTimer needs a StartTimer card.
fn card_type_button_disabled(
    ty: CardType,
    has_activate: bool,
    has_start_timer: bool,
) -> bool {
    match ty {
        CardType::Deactivate => !has_activate,
        CardType::StopTimer => !has_start_timer,
        CardType::Activate | CardType::StartTimer => false,
    }
}

/// Unique non-empty Target cell names from the layout (order of first appearance).
fn unique_layout_target_names(cells: &[CellKind]) -> Vec<String> {
    let mut names = Vec::new();
    for cell in cells {
        let CellKind::Target { name, .. } = cell else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("left", PrevEvent, Some("Timeline")),
            KeyBinding::new("right", NextEvent, Some("Timeline")),
            KeyBinding::new("shift-left", NudgeCursorBack1s, Some("Timeline")),
            KeyBinding::new("shift-right", NudgeCursorForward1s, Some("Timeline")),
            KeyBinding::new("ctrl-left", NudgeCursorBack100ms, Some("Timeline")),
            KeyBinding::new("ctrl-right", NudgeCursorForward100ms, Some("Timeline")),
            KeyBinding::new("ctrl-o", OpenFile, None),
            KeyBinding::new("ctrl-s", SaveFile, None),
            KeyBinding::new("ctrl-shift-s", SaveFileAs, None),
        ]);

        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);
                    let rows = 3;
                    let cols = 3;
                    let (target_names, target_images, target_zones) = Designer::load_targets();
                    Designer {
                        menu_open: false,
                        view: AppView::Layout,
                        layout_path: None,
                        targets_sidebar: None,
                        target_names,
                        target_images,
                        target_zones,
                        rows,
                        cols,
                        cells: vec![CellKind::Empty; rows * cols],
                        duration_ms: 60_000,
                        duration_edit: None,
                        distance_edit: None,
                        name_edit: None,
                        event_time_edit: None,
                        event_title_focused: None,
                        event_card_focused: None,
                        units_menu_open: false,
                        card_type_menu_open: None,
                        event_card_name_edit: None,
                        timer_name_menu_open: None,
                        device_target_menu_open: None,
                        flash_boards: Vec::new(),
                        flash_project: FlashProject::Laser,
                        flash_project_menu_open: false,
                        flash_status: "Plug in a board and click Refresh.".to_string(),
                        flash_busy: false,
                        cursor_ms: 0,
                        events: vec![],
                        devices: vec![DeviceCard {
                            target_name: String::new(),
                        }],
                        focus_handle,
                    }
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(def: &str, name: &str) -> CellKind {
        CellKind::Target {
            def: def.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn unique_layout_target_names_skips_empty_and_non_targets() {
        let cells = vec![
            CellKind::Empty,
            CellKind::default_connector(),
            target("a", ""),
            target("a", "   "),
            target("a", "alpha"),
        ];
        assert_eq!(
            unique_layout_target_names(&cells),
            vec!["alpha".to_string()]
        );
    }

    #[test]
    fn unique_layout_target_names_preserves_first_appearance_order() {
        let cells = vec![
            target("a", "beta"),
            target("b", "alpha"),
            target("c", "beta"),
            target("d", "gamma"),
            target("e", "alpha"),
        ];
        assert_eq!(
            unique_layout_target_names(&cells),
            vec![
                "beta".to_string(),
                "alpha".to_string(),
                "gamma".to_string(),
            ]
        );
    }

    #[test]
    fn unique_layout_target_names_trims_whitespace() {
        let cells = vec![target("a", "  padded  "), target("b", "padded")];
        assert_eq!(
            unique_layout_target_names(&cells),
            vec!["padded".to_string()]
        );
    }

    #[test]
    fn card_type_button_disabled_gates_deactivate_and_stop_timer() {
        assert!(!card_type_button_disabled(CardType::Activate, false, false));
        assert!(!card_type_button_disabled(CardType::StartTimer, false, false));
        assert!(card_type_button_disabled(CardType::Deactivate, false, true));
        assert!(!card_type_button_disabled(CardType::Deactivate, true, false));
        assert!(card_type_button_disabled(CardType::StopTimer, true, false));
        assert!(!card_type_button_disabled(CardType::StopTimer, false, true));
    }

    #[test]
    fn flash_project_labels_dirs_and_all() {
        assert_eq!(FlashProject::Laser.label(), "Laser");
        assert_eq!(FlashProject::Sensor.label(), "Sensor");
        assert_eq!(FlashProject::Laser.dir_name(), "laser");
        assert_eq!(FlashProject::Sensor.dir_name(), "sensor");
        assert_eq!(
            FlashProject::all(),
            [FlashProject::Laser, FlashProject::Sensor]
        );
    }

    #[test]
    fn firmware_dir_points_at_monorepo_siblings_with_cargo_toml() {
        let designer_parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("designer has a parent")
            .to_path_buf();
        for project in FlashProject::all() {
            let dir = firmware_dir(project);
            assert_eq!(dir, designer_parent.join(project.dir_name()));
            assert!(
                dir.join("Cargo.toml").is_file(),
                "expected Cargo.toml under {}",
                dir.display()
            );
        }
    }

    #[test]
    fn flash_firmware_at_reports_missing_project() {
        let missing = std::env::temp_dir().join(format!(
            "designer-flash-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&missing);
        std::fs::create_dir_all(&missing).unwrap();
        let msg = flash_firmware_at(
            &missing,
            FlashProject::Laser,
            "/dev/ttyUSB0",
            Chip::Esp32c3,
        );
        assert_eq!(
            msg,
            format!("Firmware project missing at {}.", missing.display())
        );
        let _ = std::fs::remove_dir_all(&missing);
    }
}
