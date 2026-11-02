//! Combined GPUI host: Manager, Course, Edit, Flash, Virtual.
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    Bounds, ClickEvent, Context, DispatchPhase, FocusHandle, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, PathBuilder,
    Pixels, Point, SharedString, Window, canvas, div, fill, img, point, prelude::*, px, relative,
    rgb, rgba, size,
};

use crate::chrome;
use crate::model::{self, DocImage};
use targetlib::{Unit, Zone};
use devicehostlib::roster;
use devicehostlib::sensor::{CHANNELS, Hub, Kind, Snapshot, THRESHOLD, ZoneHit};
use crate::theme::{self, Theme};
use crate::virt_ui;

const THEME_POLL: Duration = Duration::from_secs(2);
const SNAP_POLL: Duration = Duration::from_millis(100);
const HIT: f32 = 7.0;
const POLY_STROKE: u32 = 0x777777;
const POLY_FILL_A: u8 = 0xbf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Flash,
    Manager,
    Course,
    Roster,
    Edit,
    Virtual,
    Settings,
}

impl Mode {
    pub const ALL: [Mode; 6] = [
        Mode::Manager,
        Mode::Course,
        Mode::Roster,
        Mode::Edit,
        Mode::Flash,
        Mode::Virtual,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
    fn bar_title(self) -> Option<&'static str> {
        match self {
            Mode::Course => Some("Stages"),
            Mode::Roster => Some("Roster"),
            Mode::Edit => Some("Target Editor"),
            Mode::Manager => Some("Device Connections"),
            Mode::Flash => Some("Flash Device"),
            Mode::Virtual => None,
            Mode::Settings => Some("Settings"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FlashDrop {
    Chip,
    Project,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Length,
    Width,
    Name(usize),
    Score(usize),
    Path,
    /// Typed save path (fallback when no save dialog).
    SavePath,
}

pub struct Host {
    focus: FocusHandle,
    pub theme: Theme,
    theme_sig: Vec<(PathBuf, Option<SystemTime>)>,
    ui_font: theme::UiFont,
    menu_open: bool,
    pub flash_drop: Option<FlashDrop>,
    pub course: crate::ui_course::Course,
    pub roster: crate::ui_roster::Roster,
    pub settings: crate::ui_settings::Settings,
    /// Mode to restore when leaving Settings.
    settings_return: Mode,
    pub mode: Mode,
    image: Option<DocImage>,
    img_size: (u32, u32),
    length_cm: f64,
    width_cm: f64,
    unit: Unit,
    polys: Vec<Zone>,
    selected: Option<usize>,
    /// Selected vertex index within the selected polygon.
    sel_point: Option<usize>,
    edit: Option<(Field, String)>,
    /// Replace the buffer on the next printable key / Backspace (`Name` / `Score`).
    edit_sel_all: bool,
    poly_menu: Option<(usize, Point<Pixels>)>,
    drag: Option<(usize, usize)>,
    /// Whole-polygon drag: `(index, last image-space point)`.
    poly_move: Option<(usize, (f32, f32))>,
    /// Last canvas mouse in image pixels (Edit).
    hover_img: Option<(f32, f32)>,
    list_drag: Option<usize>,
    list_drag_moved: bool,
    list_down_sel: bool,
    dirty: bool,
    /// Where Ctrl+S saves silently; `None` = ask first.
    save_path: Option<PathBuf>,
    status: String,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    pub flash_view_b: Rc<Cell<Bounds<Pixels>>>,
    pub flash_out_b: Rc<Cell<Bounds<Pixels>>>,
    pub flash_chip_b: Rc<Cell<Bounds<Pixels>>>,
    pub flash_proj_b: Rc<Cell<Bounds<Pixels>>>,
    hub: Option<Arc<Hub>>,
    _roster: Option<roster::Guard>,
    snaps: Vec<Snapshot>,
    /// Polygon index → S1–S4 / H1–H4.
    zone_map: Vec<Option<ZoneHit>>,
    pub ctrl: crate::ctrl::App,
    ble: Option<(
        devicehostlib::central::Sender,
        std::sync::Mutex<std::sync::mpsc::Receiver<devicehostlib::central::BleEvent>>,
    )>,
    job_tx: std::sync::mpsc::Sender<crate::ctrl::Msg>,
    job_rx: std::sync::Mutex<std::sync::mpsc::Receiver<crate::ctrl::Msg>>,
    course_tx: std::sync::mpsc::Sender<String>,
    course_rx: std::sync::Mutex<std::sync::mpsc::Receiver<String>>,
    adapter: Option<String>,
}

/// Image-fit transform: (origin x, origin y, scale).
pub(crate) fn fit(b: Bounds<Pixels>, img: (u32, u32)) -> (f32, f32, f32) {
    let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
    let (w, h) = (img.0.max(1) as f32, img.1.max(1) as f32);
    let s = (bw / w).min(bh / h);
    (
        f32::from(b.origin.x) + (bw - w * s) / 2.0,
        f32::from(b.origin.y) + (bh - h * s) / 2.0,
        s,
    )
}

const ADC_MAX: f32 = 4095.0;

/// Four-corner hub ADC for a max-force hit at image pixel `(x, y)`.
///
/// Click pixels are converted to cm from the plate size; decay uses the same
/// centimetre display scale as [`targetlib::Target::from_cm`] / `resolve_hits`.
pub(crate) fn course_hit_adc(
    length_cm: f64,
    width_cm: f64,
    img: (u32, u32),
    x: f32,
    y: f32,
) -> [u16; 4] {
    let doc = targetlib::JsonDoc {
        image: String::new(),
        length_cm,
        width_cm,
        display_units: None,
        zones: Vec::new(),
    };
    let x_cm = if img.0 > 0 && length_cm > 0.0 {
        x * length_cm as f32 / img.0 as f32
    } else {
        0.0
    };
    let y_cm = if img.1 > 0 && width_cm > 0.0 {
        y * width_cm as f32 / img.1 as f32
    } else {
        0.0
    };
    let target = targetlib::Target::from_cm(doc);
    target
        .simulate_hit(x_cm, y_cm, ADC_MAX, target.decay())
        .map(|v| v.round().clamp(0.0, ADC_MAX) as u16)
}

fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn rect_pts(a: (f32, f32), b: (f32, f32)) -> Vec<(f32, f32)> {
    let (x0, x1) = if a.0 <= b.0 { (a.0, b.0) } else { (b.0, a.0) };
    let (y0, y1) = if a.1 <= b.1 { (a.1, b.1) } else { (b.1, a.1) };
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

enum Pick {
    File(PathBuf),
    Cancel,
    Unavailable,
}

fn pick_open_file(dir: &Path) -> Pick {
    let hint = format!("{}/", dir.display());
    if let Some(p) = run_picker("zenity", &["--file-selection", "--title=Open image or target JSON", "--filename", &hint]) {
        return p;
    }
    if let Some(p) = run_picker("kdialog", &["--getopenfilename", hint.trim_end_matches('/')]) {
        return p;
    }
    Pick::Unavailable
}

fn run_picker(bin: &str, args: &[&str]) -> Option<Pick> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return Some(Pick::Cancel);
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        Some(Pick::Cancel)
    } else {
        Some(Pick::File(PathBuf::from(s)))
    }
}

fn alpha(c: u32, a: u8) -> Hsla {
    rgba((c << 8) | a as u32).into()
}

fn run_streaming(
    label: &str,
    mut cmd: std::process::Command,
    tx: &std::sync::mpsc::Sender<crate::ctrl::Msg>,
) -> Option<i32> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    let _ = tx.send(crate::ctrl::Msg::Line(format!("$ {label}")));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(crate::ctrl::Msg::Line(format!("failed to start: {e}")));
            return None;
        }
    };
    fn pump(
        r: impl Read + Send + 'static,
        tx: std::sync::mpsc::Sender<crate::ctrl::Msg>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut r = BufReader::new(r);
            let mut buf = Vec::new();
            while let Ok(n) = r.read_until(b'\n', &mut buf) {
                if n == 0 {
                    break;
                }
                for part in String::from_utf8_lossy(&buf).split(['\r', '\n']) {
                    if !part.trim().is_empty() {
                        let _ = tx.send(crate::ctrl::Msg::Line(part.to_string()));
                    }
                }
                buf.clear();
            }
        })
    }
    let a = pump(child.stdout.take().unwrap(), tx.clone());
    let b = pump(child.stderr.take().unwrap(), tx.clone());
    let status = child.wait();
    let _ = (a.join(), b.join());
    match status {
        Ok(s) => {
            let _ = tx.send(crate::ctrl::Msg::Line(match s.code() {
                Some(c) => format!("[exit code {c}]"),
                None => format!("[{s}]"),
            }));
            s.code()
        }
        Err(e) => {
            let _ = tx.send(crate::ctrl::Msg::Line(format!("[wait failed: {e}]")));
            None
        }
    }
}

fn esp32_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("dev/esp32")
}

impl Host {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        arg: Option<PathBuf>,
        start: Mode,
        adapter: Option<String>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let theme = theme::load_current().0;
        let projects = utils::host::list_projects(
            &esp32_root(),
            &["utils", "target_editor", "scriptlib", "targetlib", "devicehostlib", "devicelib", "reportslib"],
        );
        let mut ctrl = crate::ctrl::App::new(projects, theme);
        ctrl.actions_err = crate::actions::init();
        ctrl.load_devices();
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let job_rx = std::sync::Mutex::new(job_rx);
        let (course_tx, course_rx) = std::sync::mpsc::channel();
        let course_rx = std::sync::Mutex::new(course_rx);
        let mut this = Self {
            focus,
            theme,
            theme_sig: theme::signature(),
            ui_font: theme::load_ui_font(),
            menu_open: false,
            flash_drop: None,
            course: crate::ui_course::Course::new(),
            roster: crate::ui_roster::Roster::empty(),
            settings: crate::ui_settings::Settings::load(),
            settings_return: Mode::Edit,
            mode: Mode::Edit,
            image: None,
            img_size: (1, 1),
            length_cm: 0.0,
            width_cm: 0.0,
            unit: Unit::Cm,
            polys: Vec::new(),
            selected: None,
            sel_point: None,
            edit: None,
            edit_sel_all: false,
            poly_menu: None,
            drag: None,
            poly_move: None,
            hover_img: None,
            list_drag: None,
            list_drag_moved: false,
            list_down_sel: false,
            dirty: false,
            save_path: None,
            status: "Ctrl+O to open an image or .json".into(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            flash_view_b: Rc::new(Cell::new(Bounds::default())),
            flash_out_b: Rc::new(Cell::new(Bounds::default())),
            flash_chip_b: Rc::new(Cell::new(Bounds::default())),
            flash_proj_b: Rc::new(Cell::new(Bounds::default())),
            hub: None,
            _roster: None,
            snaps: Vec::new(),
            zone_map: Vec::new(),
            ctrl,
            ble: None,
            job_tx,
            job_rx,
            course_tx,
            course_rx,
            adapter,
        };
        if let Some(e) = crate::config::load_err() {
            this.status = format!("config.json: {e}");
        }
        if let Some(p) = arg {
            this.open_path(&p);
        }
        this.watch_theme(cx);
        this.watch_jobs(cx);
        this.set_mode(start, cx);
        if start == Mode::Flash {
            this.spawn_scan();
        }
        this
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.flash_drop = None;
        self.course.roster_drop = false;
        self.course.script_drop = false;
        self.course.hub_drop = None;
        if mode == Mode::Settings {
            self.toggle_settings(cx);
            return;
        }
        if self.mode == Mode::Roster && mode != Mode::Roster {
            self.commit_roster_edit();
        }
        if self.mode == Mode::Settings && mode != Mode::Settings {
            self.settings.commit_value();
            if let Err(e) = self.settings.save() {
                self.status = e;
            }
        }
        let old = self.mode;
        if old == mode {
            match mode {
                Mode::Virtual => self.enter_virtual(cx),
                Mode::Manager => {
                    self.ensure_hub(cx);
                    self.start_ble();
                }
                Mode::Course => self.sync_course_roster(),
                Mode::Roster => {
                    self.roster.load_today();
                }
                Mode::Settings => {}
                _ => {}
            }
            return;
        }
        self.mode = mode;
        match mode {
            Mode::Edit => self.enter_edit(),
            Mode::Virtual => self.enter_virtual(cx),
            Mode::Flash => {
                let _ = self.ctrl.set_tab(crate::ctrl::Tab::Flash);
                if self.ctrl.boards.is_empty() && !self.ctrl.scanning {
                    self.spawn_scan();
                }
            }
            Mode::Manager => {
                let _ = self.ctrl.set_tab(crate::ctrl::Tab::Manager);
                self.ctrl.focus = crate::ctrl::Focus::Devices;
                self.ensure_hub(cx);
                self.start_ble();
            }
            Mode::Course => {
                let _ = self.ctrl.set_tab(crate::ctrl::Tab::Course);
                self.ensure_hub(cx);
                self.start_ble();
                self.refresh_course();
                self.sync_course_roster();
            }
            Mode::Roster => {
                self.roster.load_today();
                if let Some(e) = &self.roster.load_err {
                    self.status = e.clone();
                } else {
                    self.status = format!(
                        "Roster {}",
                        self.roster
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    );
                }
            }
            Mode::Settings => {}
        }
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Settings {
            self.close_settings(cx);
            return;
        }
        self.commit_edit();
        self.commit_roster_edit();
        if self.course.name_edit.is_some() {
            self.course.name_edit = None;
        }
        self.settings_return = self.mode;
        self.settings.reload();
        if let Some(e) = &self.settings.load_err {
            self.status = e.clone();
        } else {
            self.status = "Settings".into();
        }
        self.mode = Mode::Settings;
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.commit_value();
        match self.settings.save() {
            Ok(()) => self.status = "Settings saved".into(),
            Err(e) => self.status = e,
        }
        let ret = self.settings_return;
        self.mode = ret;
        match ret {
            Mode::Virtual => self.enter_virtual(cx),
            Mode::Manager => {
                self.ensure_hub(cx);
                self.start_ble();
            }
            Mode::Course => {
                self.ensure_hub(cx);
                self.start_ble();
                self.refresh_course();
                self.sync_course_roster();
            }
            Mode::Roster => self.roster.load_today(),
            Mode::Flash => {
                if self.ctrl.boards.is_empty() && !self.ctrl.scanning {
                    self.spawn_scan();
                }
            }
            Mode::Edit => self.enter_edit(),
            Mode::Settings => {}
        }
        cx.notify();
    }

    pub(crate) fn settings_select(&mut self, i: usize) {
        self.settings.focus = Some(i);
    }

    pub(crate) fn start_settings_value_edit(&mut self, i: usize) {
        self.settings.start_value_edit(i);
    }

    fn cycle_mode(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = Mode::ALL.len() as isize;
        let i = (self.mode.index() as isize + d).rem_euclid(n) as usize;
        self.set_mode(Mode::ALL[i], cx);
    }

    fn start_ble(&mut self) {
        let hub = self.hub.clone();
        let adapter = self.adapter.clone();
        let (cmd, _) = self.ble.get_or_insert_with(|| {
            let (cmd, rx) = devicehostlib::central::spawn(adapter, hub.clone());
            (cmd, std::sync::Mutex::new(rx))
        });
        if let Some(h) = &hub {
            let _ = cmd.send(devicehostlib::central::BleCmd::SetHub(h.clone()));
        }
        let send = cmd.clone();
        self.ctrl.script.set_ble(Arc::new(move |addr, line| {
            send.send(devicehostlib::central::BleCmd::Command(addr, line))
                .map_err(|e| format!("BLE command: {e}"))
        }));
    }

    fn watch_jobs(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                if this
                    .update(cx, |this, cx| {
                        let mut msgs = Vec::new();
                        if let Ok(rx) = this.job_rx.lock() {
                            while let Ok(msg) = rx.try_recv() {
                                msgs.push(msg);
                            }
                        }
                        let mut evs = Vec::new();
                        if let Some((_, erx)) = &this.ble
                            && let Ok(rx) = erx.lock()
                        {
                            while let Ok(ev) = rx.try_recv() {
                                evs.push(ev);
                            }
                        }
                        let mut course_msgs = Vec::new();
                        if let Ok(rx) = this.course_rx.lock() {
                            while let Ok(msg) = rx.try_recv() {
                                course_msgs.push(msg);
                            }
                        }
                        for msg in msgs {
                            this.ctrl.handle_msg(msg);
                        }
                        for ev in evs {
                            this.ctrl.handle_msg(crate::ctrl::Msg::Ble(ev));
                        }
                        for line in course_msgs {
                            crate::ui_course::push_event(&mut this.course, line);
                        }
                        let was_running = this.course.running;
                        this.course.running = this.course.live.load(Ordering::Relaxed);
                        if was_running && !this.course.running {
                            this.write_course_report();
                            for t in &mut this.course.targets {
                                t.hits.clear();
                            }
                        }
                        this.ctrl.theme = this.theme;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn spawn_scan(&mut self) {
        self.ctrl.start_scan();
        let tx = self.job_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::ctrl::Msg::Boards(utils::host::detect_boards(10)));
        });
    }

    pub(crate) fn spawn_job(&mut self, flash: bool) {
        use utils::host::{self, Chip, Framework};
        let (Some(ci), Some(pi)) = (self.ctrl.chip, self.ctrl.project) else {
            return;
        };
        let chip = Chip::ALL[ci];
        let dir = self.ctrl.projects[pi].clone();
        let port = self.ctrl.chip_port(ci).map(str::to_string);
        self.ctrl.start_job(if flash { "Flash" } else { "Build" });
        let tx = self.job_tx.clone();
        std::thread::spawn(move || {
            let mut code = run_streaming(
                &host::build_shell(&dir, chip, Framework::Hal),
                host::build_command(&dir, chip, Framework::Hal),
                &tx,
            );
            if flash && code == Some(0) {
                let p = port.as_deref();
                let _ = tx.send(crate::ctrl::Msg::Line(format!(
                    "Flashing {} on {}",
                    chip.name(),
                    p.unwrap_or("auto-detected port")
                )));
                code = run_streaming(
                    &host::flash_shell(&dir, chip, Framework::Hal, p),
                    host::flash_command(&dir, chip, Framework::Hal, p),
                    &tx,
                );
            }
            let _ = tx.send(crate::ctrl::Msg::JobDone(code));
        });
    }

    fn dispatch(&mut self, cmd: crate::ctrl::Cmd, cx: &mut Context<Self>) {
        match cmd {
            crate::ctrl::Cmd::Quit => cx.quit(),
            crate::ctrl::Cmd::Refresh => self.spawn_scan(),
            crate::ctrl::Cmd::Build => self.spawn_job(false),
            crate::ctrl::Cmd::Flash => self.spawn_job(true),
        }
    }

    pub(crate) fn add_virtual(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let fresh = self.hub.is_none();
        self.ensure_hub(cx);
        if fresh && let Some(hub) = &self.hub {
            hub.set_sensors(false);
            hub.set_hub(false);
        }
        if let Some(hub) = &self.hub {
            let b = hub.add(kind);
            let s = b.snapshot();
            let what = match kind {
                Kind::Hub => "Hub Sensor",
                Kind::Standalone => "Sensor",
            };
            self.status = format!("Added virtual {what} {}", s.name);
            self.ctrl.log(format!("virtual: added {what} {} ({})", s.name, s.mac));
        }
        self.start_ble();
        self.sync_virt_links(cx);
    }

    pub(crate) fn start_mgr_rename(
        &mut self,
        virt_mac: Option<String>,
        addr: Option<String>,
        name: String,
    ) {
        if self.ctrl.dev_edit.is_some() {
            self.commit_mgr_rename();
        }
        self.ctrl.dev_edit = Some(crate::ctrl::DevEdit {
            virt_mac,
            addr,
            buf: name,
        });
    }

    fn commit_mgr_rename(&mut self) {
        let Some(edit) = self.ctrl.dev_edit.take() else {
            return;
        };
        let new = edit.buf.trim().to_string();
        if let Some(mac) = edit.virt_mac {
            if !new.is_empty()
                && let Some(hub) = &self.hub
                && let Some(b) = hub.board_by_mac(&mac)
            {
                b.set_name(new);
            }
            return;
        }
        if let Some(addr) = edit.addr {
            self.ctrl.apply_ble_rename(&addr, new);
        }
    }

    pub(crate) fn toggle_mgr_enabled(
        &mut self,
        virt_mac: Option<String>,
        addr: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(mac) = virt_mac {
            self.ensure_hub(cx);
            if let Some(hub) = &self.hub
                && let Some(b) = hub.board_by_mac(&mac)
            {
                let s = b.snapshot();
                let keys = Self::virt_link_keys(&s);
                let enabled = keys.iter().any(|k| self.ctrl.conn(k).is_some_and(|l| l.enabled));
                if enabled {
                    hub.remove(&mac);
                    for k in keys {
                        self.ctrl.script.drop_link(&k);
                        self.ctrl.connections.remove(&k.to_ascii_lowercase());
                    }
                    self.ctrl.mgr_cursor = self
                        .ctrl
                        .mgr_cursor
                        .min(self.ctrl.mgr_rows().len().saturating_sub(1));
                    cx.notify();
                    return;
                }
                let Some(addr) = keys.first().cloned() else {
                    return;
                };
                self.ensure_virt_row(&s);
                let _ = self.ctrl.set_enabled(&addr, true);
            }
            cx.notify();
            return;
        }
        let Some(addr) = addr else {
            return;
        };
        let _ = self.ctrl.toggle_enabled(&addr);
        cx.notify();
    }

    fn virt_link_key(s: &Snapshot) -> Option<String> {
        s.adapter_addr
            .clone()
            .filter(|a| !a.is_empty())
            .or_else(|| {
                if s.mac.is_empty() {
                    None
                } else {
                    Some(s.mac.clone())
                }
            })
    }

    fn virt_link_keys(s: &Snapshot) -> Vec<String> {
        let mut keys = Vec::new();
        let push = |keys: &mut Vec<String>, v: &str| {
            if v.is_empty() {
                return;
            }
            if keys.iter().any(|k| k.eq_ignore_ascii_case(v)) {
                return;
            }
            keys.push(v.to_string());
        };
        if let Some(a) = Self::virt_link_key(s) {
            push(&mut keys, &a);
        }
        push(&mut keys, &s.mac);
        if let Some(a) = &s.adapter_addr {
            push(&mut keys, a);
        }
        keys
    }

    fn ensure_virt_row(&mut self, s: &Snapshot) {
        let Some(addr) = Self::virt_link_keys(s).into_iter().next() else {
            return;
        };
        let k = addr.to_ascii_lowercase();
        if let Some(l) = self.ctrl.connections.get_mut(&k) {
            l.enabled = true;
            self.ctrl.script.set_enabled(&addr, true);
            return;
        }
        let info = devicehostlib::central::DeviceInfo {
            addr: addr.clone(),
            name: s.name.clone(),
            project: devicehostlib::sensor::PROJECT.into(),
            mac: s.mac.clone(),
            rssi: None,
        };
        let mut l = devicehostlib::Linked::new(info, None, None);
        l.enabled = true;
        self.ctrl.script.set_enabled(&addr, true);
        self.ctrl.connections.insert(k, l);
    }

    fn sync_virt_links(&mut self, _cx: &mut Context<Self>) {
        for b in self.virtual_boards() {
            let s = b.snapshot();
            if !s.link.is_connected() {
                continue;
            }
            let keys = Self::virt_link_keys(&s);
            self.ensure_virt_row(&s);
            for k in keys {
                if let Some(l) = self.ctrl.conn_mut(&k) {
                    if l.since.is_none() {
                        l.since = Some(Instant::now());
                    }
                }
            }
        }
        self.ctrl.mgr_cursor = self
            .ctrl
            .mgr_cursor
            .min(self.ctrl.mgr_rows().len().saturating_sub(1));
    }

    fn ensure_hub(&mut self, cx: &mut Context<Self>) {
        if self.hub.is_some() {
            return;
        }
        let hub = Hub::new();
        let n = self.settings.default_virtual_sensor_hubs();
        if n > 0 {
            hub.set_sensors(false);
            hub.set_hub(false);
            for _ in 0..n {
                hub.add(Kind::Hub);
            }
        }
        self._roster = Some(roster::Guard::new());
        self.snaps = hub.live_boards().iter().map(|b| b.snapshot()).collect();
        self.hub = Some(hub);
        self.start_ble();
        self.watch_snaps(cx);
    }

    fn write_course_report(&mut self) {
        let shooter = self
            .course
            .roster_ix
            .and_then(|i| self.roster.name(i))
            .unwrap_or("");
        let stage = self
            .course
            .scripts
            .get(self.course.ix)
            .map(|s| {
                PathBuf::from(s)
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| s.clone())
            })
            .unwrap_or_else(|| "unknown".into());
        match crate::report::write_stage_report(
            &self.settings.reports_dir(),
            shooter,
            &stage,
            &self.course.events,
            &self.course.targets,
        ) {
            Ok(path) => {
                self.status = format!("Report {}", path.display());
                self.ctrl.log(format!("report: {}", path.display()));
            }
            Err(e) => {
                self.status = format!("Report failed: {e}");
                self.ctrl.log(format!("report: {e}"));
            }
        }
    }

    fn watch_snaps(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SNAP_POLL).await;
                if this
                    .update(cx, |this, cx| {
                        if let Some(hub) = &this.hub {
                            this.snaps = hub.live_boards().iter().map(|b| b.snapshot()).collect();
                            this.sync_virt_links(cx);
                            if this.mode == Mode::Course {
                                this.assign_default_hubs();
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn line_px(&self) -> f32 {
        self.ui_font.size_px
    }

    fn flash_output_lines(&self, window: &Window) -> usize {
        let line = self.line_px().max(1.0);
        let inner = f32::from(self.flash_out_b.get().size.height);
        let h = if inner > 1.0 {
            inner
        } else {
            let win = f32::from(window.bounds().size.height);
            (win - 36.0 - line * 4.0).max(line)
        };
        (h / line).floor().max(1.0) as usize
    }

    pub(crate) fn virtual_boards(&self) -> Vec<Arc<devicehostlib::sensor::Board>> {
        self.hub
            .as_ref()
            .map(|h| h.manager_boards())
            .unwrap_or_default()
    }

    pub(crate) fn virtual_macs(&self) -> std::collections::HashSet<String> {
        self.hub
            .as_ref()
            .map(|h| {
                h.all_boards()
                    .into_iter()
                    .map(|b| b.snapshot().mac.to_ascii_uppercase())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn sensors_on(&self) -> bool {
        self.hub.as_ref().is_some_and(|h| h.sensors_on())
    }

    fn hub_on(&self) -> bool {
        self.hub.as_ref().is_some_and(|h| h.hub_on())
    }

    fn sync_zone_map(&mut self) {
        let n = self.polys.len();
        if self.zone_map.is_empty() && n > 0 {
            let sensors = self.sensors_on();
            let hub = self.hub_on();
            self.zone_map = (0..n)
                .map(|i| {
                    if i >= CHANNELS {
                        None
                    } else if sensors {
                        Some(ZoneHit::Sensor(i))
                    } else if hub {
                        Some(ZoneHit::Hub(i))
                    } else {
                        None
                    }
                })
                .collect();
            return;
        }
        self.zone_map.resize(n, None);
        let sensors = self.sensors_on();
        let hub = self.hub_on();
        for slot in &mut self.zone_map {
            match *slot {
                Some(ZoneHit::Sensor(_)) if !sensors => *slot = None,
                Some(ZoneHit::Hub(_)) if !hub => *slot = None,
                _ => {}
            }
        }
    }

    fn enter_virtual(&mut self, cx: &mut Context<Self>) {
        self.commit_edit();
        self.drag = None;
        self.poly_move = None;
        self.hover_img = None;
        self.list_drag = None;
        self.list_drag_moved = false;
        self.mode = Mode::Virtual;
        self.sync_zone_map();
        self.ensure_hub(cx);
        if self.status.starts_with("Ctrl+O") {
            self.status = "Sensors / Hub · 1–4 hit · click a zone · scripts/vhci.sh".into();
        }
    }

    fn enter_edit(&mut self) {
        self.mode = Mode::Edit;
    }

    fn fire(&mut self, hit: ZoneHit, zone: &str) {
        let Some(hub) = &self.hub else {
            return;
        };
        match hit {
            ZoneHit::Sensor(i) if hub.sensors_on() => {
                if let Some(b) = hub.sensors.get(i) {
                    b.hit_accel();
                    self.status = format!("S{} {zone}", i + 1);
                }
            }
            ZoneHit::Hub(i) if hub.hub_on() => {
                hub.hub.hit_channel(i);
                self.status = format!("H{} {zone}", i + 1);
            }
            _ => {}
        }
    }

    fn watch_theme(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(THEME_POLL).await;
                let sig = theme::signature();
                let font = theme::load_ui_font();
                let alive = this.update(cx, |this, cx| {
                    let theme_changed = sig != this.theme_sig;
                    let font_changed = font != this.ui_font;
                    if theme_changed {
                        this.theme_sig = sig;
                        this.theme = theme::load_current().0;
                    }
                    if font_changed {
                        this.ui_font = font;
                    }
                    if theme_changed || font_changed {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn reset_doc(&mut self) {
        self.polys.clear();
        self.selected = None;
        self.sel_point = None;
        self.edit = None;
        self.edit_sel_all = false;
        self.poly_menu = None;
        self.drag = None;
        self.poly_move = None;
        self.hover_img = None;
        self.list_drag = None;
        self.list_drag_moved = false;
        self.dirty = false;
        self.save_path = None;
    }

    fn close_doc(&mut self) {
        self.reset_doc();
        self.image = None;
        self.img_size = (1, 1);
        self.length_cm = 0.0;
        self.width_cm = 0.0;
        self.zone_map.clear();
        self.status = "Closed".into();
    }

    fn open_path(&mut self, p: &Path) {
        let p = p.to_path_buf();
        let is_json = p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        let r = if is_json {
            self.open_json(&p)
        } else {
            let j = targetlib::json_path_for(&p);
            if j.exists() {
                self.open_json(&j)
            } else {
                self.open_image(&p)
            }
        };
        if let Err(e) = r {
            self.status = format!("Open failed: {e}");
        }
    }

    fn open_image(&mut self, p: &Path) -> Result<(), String> {
        let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        let doc = model::doc_image(bytes, name, Some(p.to_path_buf()))?;
        let sz = targetlib::image_size_bytes(&doc.bytes)?;
        self.reset_doc();
        self.image = Some(doc);
        self.img_size = sz;
        self.status = format!("Opened {} ({}x{} px)", p.display(), sz.0, sz.1);
        Ok(())
    }

    fn open_json(&mut self, p: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let doc: targetlib::JsonDoc =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        let dir = p.parent().unwrap_or(Path::new("."));
        let (bytes, name) = targetlib::load_image_field(&doc.image, dir)?;
        let img = model::doc_image(bytes, name, None)?;
        let sz = targetlib::image_size_bytes(&img.bytes)?;
        self.reset_doc();
        self.image = Some(img);
        self.img_size = sz;
        self.length_cm = doc.length_cm;
        self.width_cm = doc.width_cm;
        self.unit = doc.display_units.unwrap_or_default();
        self.polys = targetlib::from_json(&doc, sz);
        self.save_path = Some(p.to_path_buf());
        self.status = format!("Loaded {} ({} zones)", p.display(), self.polys.len());
        Ok(())
    }

    /// Ready to save? Sets status and returns the original image name if so.
    fn can_save(&mut self) -> Option<String> {
        let Some(image) = &self.image else {
            self.status = "Nothing to save: open an image first".into();
            return None;
        };
        if self.length_cm <= 0.0 || self.width_cm <= 0.0 {
            self.status = "Set Length and Width (> 0) before saving".into();
            return None;
        }
        Some(image.name.clone())
    }

    /// Ctrl+S: silent save to the known path, else Save As.
    fn save(&mut self, cx: &mut Context<Self>) {
        match self.save_path.clone() {
            Some(p) => self.save_to(&p),
            None => self.save_as(cx),
        }
    }

    pub(crate) fn refresh_course(&mut self) {
        self.course.scripts = scriptlib::list_in(&self.settings.stages_dir());
        if self.course.ix >= self.course.scripts.len() {
            self.course.ix = 0;
        }
        self.apply_course_init();
    }

    pub(crate) fn select_course_script(&mut self, i: usize) {
        if i >= self.course.scripts.len() {
            return;
        }
        self.course.ix = i;
        self.course.script_drop = false;
        self.apply_course_init();
    }

    pub(crate) fn select_course_roster(&mut self, i: usize) {
        if self.roster.name(i).map_or(true, |n| n.trim().is_empty()) {
            return;
        }
        self.course.roster_ix = Some(i);
        self.course.roster_drop = false;
    }

    fn sync_course_roster(&mut self) {
        self.roster.load_today();
        if let Some(i) = self.course.roster_ix {
            let ok = self.roster.name(i).is_some_and(|n| !n.trim().is_empty());
            if !ok {
                self.course.roster_ix = None;
            }
        }
    }

    fn apply_course_init(&mut self) {
        self.course.targets.clear();
        self.course.grid_rows = None;
        self.course.grid_cols = None;
        self.course.events.clear();
        self.course.name_edit = None;
        self.course.roster_drop = false;
        self.course.hub_drop = None;
        let Some(name) = self.course.scripts.get(self.course.ix).cloned() else {
            return;
        };
        let path = self.settings.stages_dir().join(&name);
        match scriptlib::run_init(&path) {
            Ok(roster) => {
                self.course.grid_rows = roster.rows;
                self.course.grid_cols = roster.columns;
                for card in roster.cards {
                    let mut t = crate::ui_course::CourseTarget::blank(card.name);
                    t.row = card.row;
                    t.col = card.col;
                    t.distance = card.distance;
                    if !card.def.is_empty() {
                        t.set_def(card.def);
                    }
                    self.course.targets.push(t);
                }
                self.restore_course_hubs();
                self.assign_default_hubs();
            }
            Err(e) => {
                crate::ui_course::push_event(&mut self.course, format!("init: {e}"));
            }
        }
    }

    pub(crate) fn course_hubs(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let virt_macs = self.virtual_macs();
        if let Some(hub) = &self.hub {
            for b in hub.live_boards() {
                let s = b.snapshot();
                if s.kind != devicehostlib::sensor::Kind::Hub {
                    continue;
                }
                let key = s
                    .adapter_addr
                    .clone()
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| s.mac.clone());
                if key.is_empty() {
                    continue;
                }
                let k = key.to_ascii_lowercase();
                if seen.insert(k) {
                    out.push((s.name, key));
                }
            }
        }
        for (info, linked) in self.ctrl.mgr_rows() {
            if virt_macs.contains(&info.mac.to_ascii_uppercase()) {
                continue;
            }
            let Some(l) = linked else {
                continue;
            };
            if l.since.is_none() || !info.project.eq_ignore_ascii_case(devicehostlib::sensor::PROJECT) {
                continue;
            }
            let key = info.addr.clone();
            let k = key.to_ascii_lowercase();
            if seen.insert(k) {
                out.push((self.ctrl.display_name(info), key));
            }
        }
        out
    }

    pub(crate) fn start_course(&mut self, _cx: &mut Context<Self>) {
        if self.course.running
            || self.course.scripts.is_empty()
            || !self.course_start_errors().is_empty()
        {
            return;
        }
        let name = self.course.scripts[self.course.ix].clone();
        let path = self.settings.stages_dir().join(&name);
        self.course.stop.store(false, Ordering::Relaxed);
        self.course.live.store(true, Ordering::Relaxed);
        self.course.events.clear();
        for t in &mut self.course.targets {
            t.hits.clear();
        }
        self.course.running = true;
        let cards: Vec<scriptlib::TargetCard> = self
            .course
            .targets
            .iter()
            .filter(|t| t.def.as_ref().is_some_and(|d| !d.is_empty()))
            .map(|t| scriptlib::TargetCard {
                name: t.name.clone(),
                def: t.def.clone().unwrap_or_default(),
                zones: t.zones.clone(),
                hub: t.hub.clone(),
                hub_name: t.hub_label.clone(),
            })
            .collect();
        scriptlib::spawn_stop(
            path,
            self.ctrl.script.clone(),
            self.course_tx.clone(),
            self.course.stop.clone(),
            Some(self.course.live.clone()),
            cards,
        );
    }

    pub(crate) fn stop_course(&mut self) {
        self.course.stop.store(true, Ordering::Relaxed);
    }

    pub(crate) fn simulate_course_hit(&mut self, i: usize, pos: Point<Pixels>) {
        if !self.course.simulate || !self.course.running {
            return;
        }
        let Some(t) = self.course.targets.get(i) else {
            return;
        };
        let Some(size) = t.img_size else {
            return;
        };
        let b = t.img_bounds.get();
        let (ox, oy, s) = fit(b, size);
        if s <= 1e-6 {
            return;
        }
        let (px, py) = (f32::from(pos.x), f32::from(pos.y));
        let (iw, ih) = (size.0 as f32, size.1 as f32);
        if px < ox || py < oy || px > ox + iw * s || py > oy + ih * s {
            return;
        }
        let x = ((px - ox) / s).clamp(0.0, iw);
        let y = ((py - oy) / s).clamp(0.0, ih);
        let size_cm = t.size_cm;
        let hub_key = t.hub.clone();
        let geometry = t.geometry.clone();
        let name = t.name.clone();
        self.course.targets[i].hits.push((x, y));
        if let Some((length_cm, width_cm)) = size_cm {
            let adc = course_hit_adc(length_cm, width_cm, size, x, y);
            if let Some(key) = hub_key.as_deref()
                && let Some(b) = self.virtual_hub_board(key)
            {
                b.hit_sensors(adc);
            }
        }
        if let Some(pi) = targetlib::hit_poly(&geometry, x, y) {
            let zone = geometry[pi].name.clone();
            scriptlib::methods::record_hit(&self.ctrl.script.host, &name, &zone);
        }
    }

    fn virtual_hub_board(&self, key: &str) -> Option<Arc<devicehostlib::sensor::Board>> {
        let hub = self.hub.as_ref()?;
        devicehostlib::local::find_board(hub, key).filter(|b| b.is_hub())
    }

    fn is_real_course_target(t: &crate::ui_course::CourseTarget) -> bool {
        t.def.as_ref().is_some_and(|d| !d.is_empty())
    }

    pub(crate) fn course_start_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let shooter_ok = self
            .course
            .roster_ix
            .and_then(|i| self.roster.name(i))
            .is_some_and(|n| !n.trim().is_empty());
        if !shooter_ok {
            errors.push("Shooter name not selected".into());
        }
        for t in &self.course.targets {
            if Self::is_real_course_target(t) && t.hub.is_none() {
                errors.push(format!("Target '{}' has no sensor hub", t.name));
            }
        }
        errors
    }

    fn assign_default_hubs(&mut self) {
        let hubs = self.course_hubs();
        if hubs.is_empty() {
            return;
        }
        let mut used: std::collections::HashSet<String> = self
            .course
            .targets
            .iter()
            .filter(|t| Self::is_real_course_target(t))
            .filter_map(|t| t.hub.as_ref().map(|k| k.to_ascii_lowercase()))
            .collect();
        let mut assigned = Vec::new();
        for t in &mut self.course.targets {
            if !Self::is_real_course_target(t) || t.hub.is_some() {
                continue;
            }
            let Some((label, key)) = hubs
                .iter()
                .find(|(_, k)| !used.contains(&k.to_ascii_lowercase()))
                .cloned()
            else {
                break;
            };
            used.insert(key.to_ascii_lowercase());
            t.hub = Some(key.clone());
            t.hub_label = Some(label.clone());
            assigned.push((t.name.clone(), key, label));
        }
        for (name, key, label) in assigned {
            self.remember_course_hub(&name, key, label);
        }
    }

    pub(crate) fn remember_course_hub(&mut self, name: &str, key: String, label: String) {
        let Some(script) = self.course.scripts.get(self.course.ix).cloned() else {
            return;
        };
        self.course
            .hub_by_card
            .insert((script, name.to_string()), (key, label));
    }

    fn restore_course_hubs(&mut self) {
        let Some(script) = self.course.scripts.get(self.course.ix).cloned() else {
            return;
        };
        let stored: Vec<Option<(String, String)>> = self
            .course
            .targets
            .iter()
            .map(|t| {
                self.course
                    .hub_by_card
                    .get(&(script.clone(), t.name.clone()))
                    .cloned()
            })
            .collect();
        let hubs = self.course_hubs();
        for (t, stored) in self.course.targets.iter_mut().zip(stored) {
            if !Self::is_real_course_target(t) {
                continue;
            }
            let Some((key, _label)) = stored else {
                continue;
            };
            // Only restore hubs that are still up/online in course_hubs().
            if let Some((l, k)) = hubs.iter().find(|(_, k)| k.eq_ignore_ascii_case(&key)) {
                t.hub = Some(k.clone());
                t.hub_label = Some(l.clone());
            }
        }
    }

    pub(crate) fn start_course_rename(&mut self, i: usize) {
        let Some(name) = self.course.targets.get(i).map(|t| t.name.clone()) else {
            return;
        };
        self.commit_course_rename();
        self.course.name_edit = Some((i, name));
    }

    fn commit_course_rename(&mut self) {
        let Some((i, buf)) = self.course.name_edit.take() else {
            return;
        };
        let name = buf.trim();
        if name.is_empty() {
            return;
        }
        let old = self.course.targets.get(i).map(|t| t.name.clone());
        if let Some(t) = self.course.targets.get_mut(i) {
            t.name = name.to_string();
        }
        if let (Some(old), Some(script)) = (old, self.course.scripts.get(self.course.ix).cloned())
            && old != name
        {
            if let Some(v) = self.course.hub_by_card.remove(&(script.clone(), old)) {
                self.course.hub_by_card.insert((script, name.to_string()), v);
            }
        }
    }

    pub(crate) fn roster_select(&mut self, i: usize) {
        if i < self.roster.entries.len() {
            self.roster.focus = Some(i);
        }
    }

    pub(crate) fn start_roster_edit(&mut self, i: usize, field: crate::ui_roster::Field) {
        let Some(entry) = self.roster.entries.get(i) else {
            return;
        };
        let buf = match field {
            crate::ui_roster::Field::Name => entry.name.clone(),
            crate::ui_roster::Field::Email => entry.email.clone(),
        };
        self.commit_roster_edit();
        self.roster.focus = Some(i);
        self.roster.edit = Some((i, field, buf));
    }

    fn commit_roster_edit(&mut self) {
        let Some((i, field, buf)) = self.roster.edit.take() else {
            return;
        };
        if let Some(entry) = self.roster.entries.get_mut(i) {
            let trimmed = buf.trim().to_string();
            match field {
                crate::ui_roster::Field::Name => entry.name = trimmed,
                crate::ui_roster::Field::Email => entry.email = trimmed,
            }
        }
        self.persist_roster();
    }

    pub(crate) fn roster_add(&mut self) {
        self.commit_roster_edit();
        self.roster.entries.push(crate::ui_roster::Entry {
            name: String::new(),
            email: String::new(),
        });
        let i = self.roster.entries.len() - 1;
        self.roster.focus = Some(i);
        self.roster.edit = Some((i, crate::ui_roster::Field::Name, String::new()));
        self.persist_roster();
    }

    pub(crate) fn roster_delete_focused(&mut self) {
        self.commit_roster_edit();
        let Some(i) = self.roster.focus else {
            return;
        };
        if i >= self.roster.entries.len() {
            return;
        }
        self.roster.entries.remove(i);
        self.roster.focus = if self.roster.entries.is_empty() {
            None
        } else {
            Some(i.min(self.roster.entries.len() - 1))
        };
        self.persist_roster();
    }

    fn persist_roster(&mut self) {
        match self.roster.save() {
            Ok(()) => {
                self.roster.load_err = None;
                self.status = format!(
                    "Saved {}",
                    self.roster
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
            }
            Err(e) => {
                self.status = e.clone();
                self.roster.load_err = Some(e);
            }
        }
    }

    /// Save As dialog, starting in the image's folder with `<stem>.json` suggested.
    fn save_as(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.can_save() else {
            return;
        };
        let start = self.save_path.clone().unwrap_or_else(|| {
            self.image
                .as_ref()
                .and_then(|i| i.source.as_ref())
                .map(|p| targetlib::json_path_for(p))
                .unwrap_or_else(|| targetlib::json_path_for(Path::new(&name)))
        });
        let dir = start
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = start
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(Ok(Some(p))) => this.save_to(&p),
                    Ok(Ok(None)) => this.status = "Save cancelled".into(),
                    _ => {
                        this.status = "No save dialog available: type a path, Enter to save".into();
                        this.edit = Some((Field::SavePath, start.to_string_lossy().into_owned()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_to(&mut self, out: &Path) {
        if self.can_save().is_none() {
            return;
        };
        let Some(img) = &self.image else {
            return;
        };
        let out = if out
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            out.to_path_buf()
        } else {
            let mut s = out.as_os_str().to_os_string();
            s.push(".json");
            PathBuf::from(s)
        };
        let doc = targetlib::to_json(
            targetlib::encode_image(&img.bytes),
            self.img_size,
            self.length_cm,
            self.width_cm,
            self.unit,
            &self.polys,
        );
        match serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())
            .and_then(|s| std::fs::write(&out, s + "\n").map_err(|e| e.to_string()))
        {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("Saved {}", out.display());
                self.save_path = Some(out);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let dir = crate::targets::dir();
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move { pick_open_file(&dir) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match picked {
                    Pick::File(p) => this.open_path(&p),
                    Pick::Cancel => {}
                    Pick::Unavailable => {
                        this.status =
                            "No file dialog available: type a path, Enter to open".into();
                        this.edit = Some((Field::Path, format!("{}/", crate::targets::dir().display())));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_edit(&mut self, f: Field) {
        let v = match f {
            Field::Length => self.fmt_dim(self.length_cm),
            Field::Width => self.fmt_dim(self.width_cm),
            Field::Name(i) => self
                .polys
                .get(i)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            Field::Score(i) => self
                .polys
                .get(i)
                .map(|p| p.score.to_string())
                .unwrap_or_default(),
            Field::Path | Field::SavePath => String::new(),
        };
        self.edit = Some((f, v));
        self.edit_sel_all = matches!(f, Field::Name(_) | Field::Score(_));
    }

    fn fmt_dim(&self, cm: f64) -> String {
        if cm > 0.0 {
            fmt_num(self.unit.cm_to(cm))
        } else {
            String::new()
        }
    }

    fn commit_edit(&mut self) {
        let Some((f, buf)) = self.edit.take() else {
            return;
        };
        self.edit_sel_all = false;
        let buf = buf.trim().to_string();
        match f {
            Field::Length | Field::Width => match buf.replace(',', ".").parse::<f64>() {
                Ok(v) if v > 0.0 && v.is_finite() => {
                    let cm = self.unit.to_cm(v);
                    let slot = if f == Field::Length {
                        &mut self.length_cm
                    } else {
                        &mut self.width_cm
                    };
                    if (*slot - cm).abs() > 1e-9 {
                        *slot = cm;
                        self.dirty = true;
                    }
                }
                _ if buf.is_empty() => {}
                _ => self.status = format!("Invalid number: {buf}"),
            },
            Field::Name(i) => {
                if let Some(p) = self.polys.get_mut(i)
                    && !buf.is_empty()
                    && p.name != buf
                {
                    p.name = buf;
                    self.dirty = true;
                }
            }
            Field::Score(i) => match buf.parse::<i32>() {
                Ok(v) => {
                    if let Some(p) = self.polys.get_mut(i)
                        && p.score != v
                    {
                        p.score = v;
                        self.dirty = true;
                    }
                }
                _ if buf.is_empty() => {}
                _ => self.status = format!("Invalid score: {buf}"),
            },
            Field::Path | Field::SavePath => {
                if !buf.is_empty() {
                    let p = if let Some(rest) = buf.strip_prefix("~/") {
                        std::env::var_os("HOME")
                            .map(PathBuf::from)
                            .unwrap_or_default()
                            .join(rest)
                    } else {
                        PathBuf::from(buf)
                    };
                    if f == Field::Path {
                        self.open_path(&p);
                    } else {
                        self.save_to(&p);
                    }
                }
            }
        }
    }

    fn cancel_edit(&mut self) {
        self.edit = None;
        self.edit_sel_all = false;
    }

    fn next_unamed(&self) -> String {
        let mut n = 1u32;
        loop {
            let s = format!("Unamed-{n}");
            if !self.polys.iter().any(|p| p.name == s) {
                return s;
            }
            n += 1;
        }
    }

    fn add_polygon(&mut self) {
        if self.image.is_none() {
            self.status = "Open an image first".into();
            return;
        }
        let (w, h) = self.img_size;
        let cx = w as f32 / 2.0;
        let cy = h as f32 / 2.0;
        let points = rect_pts((cx - 10.0, cy - 10.0), (cx + 10.0, cy + 10.0));
        let color = targetlib::next_color(self.polys.iter().map(|p| p.color), targetlib::PALETTE.len());
        self.polys.push(Zone {
            name: self.next_unamed(),
            points,
            color,
            score: 0,
            hidden: false,
        });
        if !self.zone_map.is_empty() {
            self.zone_map.push(None);
        }
        self.selected = Some(self.polys.len() - 1);
        self.sel_point = None;
        self.dirty = true;
    }

    fn corner_px(pts: &[(f32, f32)], cx: f32, cy: f32) -> Option<[u32; 4]> {
        if pts.is_empty() {
            return None;
        }
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for &(x, y) in pts {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let d = |x: f32, y: f32| ((x - cx).hypot(y - cy)).round() as u32;
        Some([
            d(min_x, min_y),
            d(max_x, min_y),
            d(min_x, max_y),
            d(max_x, max_y),
        ])
    }

    fn finish_list_drag(&mut self) -> bool {
        if self.list_drag.take().is_none() {
            return false;
        }
        if !self.list_drag_moved && self.list_down_sel {
            self.selected = None;
            self.sel_point = None;
        }
        self.list_drag_moved = false;
        true
    }

    fn reorder_poly(&mut self, from: usize, to: usize) {
        let n = self.polys.len();
        if from == to || from >= n || to >= n {
            return;
        }
        let p = self.polys.remove(from);
        self.polys.insert(to, p);
        if from < self.zone_map.len() {
            let z = self.zone_map.remove(from);
            self.zone_map.insert(to.min(self.zone_map.len()), z);
        }
        self.selected = Some(to);
        self.sel_point = None;
        if let Some((Field::Name(i) | Field::Score(i), _)) = self.edit {
            if i == from {
                self.edit = None;
            }
        }
        self.dirty = true;
    }

    fn delete_poly(&mut self, i: usize) {
        if i < self.polys.len() {
            self.polys.remove(i);
            if i < self.zone_map.len() {
                self.zone_map.remove(i);
            }
            self.sel_point = None;
            self.selected = match self.selected {
                Some(s) if s == i => None,
                Some(s) if s > i => Some(s - 1),
                s => s,
            };
            if let Some((Field::Name(_) | Field::Score(_), _)) = self.edit {
                self.edit = None;
            }
            self.poly_menu = None;
            self.dirty = true;
        }
    }

    fn unique_copy_name(&self, base: &str) -> String {
        let stem = format!("{base} copy");
        if !self.polys.iter().any(|p| p.name == stem) {
            return stem;
        }
        let mut n = 2u32;
        loop {
            let s = format!("{stem} {n}");
            if !self.polys.iter().any(|p| p.name == s) {
                return s;
            }
            n += 1;
        }
    }

    fn duplicate_poly(&mut self, i: usize) {
        let Some(src) = self.polys.get(i).cloned() else {
            return;
        };
        let name = self.unique_copy_name(&src.name);
        let color = targetlib::next_color(self.polys.iter().map(|p| p.color), targetlib::PALETTE.len());
        let dup = Zone {
            name,
            points: src.points.iter().map(|&(x, y)| (x + 8.0, y + 8.0)).collect(),
            color,
            score: src.score,
            hidden: false,
        };
        let j = i + 1;
        self.polys.insert(j, dup);
        if !self.zone_map.is_empty() {
            self.zone_map.insert(j.min(self.zone_map.len()), None);
        }
        self.selected = Some(j);
        self.sel_point = None;
        self.poly_menu = None;
        self.dirty = true;
    }

    fn toggle_hidden(&mut self, i: usize) {
        if let Some(p) = self.polys.get_mut(i) {
            p.hidden = !p.hidden;
            self.dirty = true;
        }
        self.poly_menu = None;
    }

    fn delete_point(&mut self) {
        let (Some(pi), Some(vi)) = (self.selected, self.sel_point) else {
            return;
        };
        let Some(p) = self.polys.get_mut(pi) else {
            self.sel_point = None;
            return;
        };
        if vi >= p.points.len() {
            self.sel_point = None;
        } else if p.points.len() > 3 {
            p.points.remove(vi);
            self.sel_point = None;
            self.dirty = true;
        } else {
            self.status = "A zone needs at least 3 points".into();
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let ctrl = k.modifiers.control || k.modifiers.platform;
        if ctrl && k.key == "o" {
            self.menu_open = false;
            self.commit_edit();
            self.prompt_open(cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if ctrl && k.key == "s" {
            self.menu_open = false;
            self.flash_drop = None;
            self.commit_edit();
            if k.modifiers.shift {
                self.save_as(cx);
            } else {
                self.save(cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if ctrl && k.key == "w" {
            self.menu_open = false;
            self.close_doc();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if ctrl && k.key == "q" {
            cx.quit();
            cx.stop_propagation();
            return;
        }
        if self.menu_open {
            if k.key == "escape" {
                self.menu_open = false;
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.poly_menu.is_some() && k.key == "escape" {
            self.poly_menu = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.flash_drop.is_some() && k.key == "escape" {
            self.flash_drop = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if (self.course.roster_drop
            || self.course.script_drop
            || self.course.hub_drop.is_some())
            && k.key == "escape"
        {
            self.course.roster_drop = false;
            self.course.script_drop = false;
            self.course.hub_drop = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.mode == Mode::Settings {
            if let Some(key) = crate::key::Key::from_gpui(
                &k.key,
                k.key_char.as_deref(),
                ctrl,
                k.modifiers.shift,
                k.modifiers.alt,
            ) {
                if self.settings.value_edit.is_some() {
                    match key.code {
                        crate::key::Code::Enter => self.settings.commit_value(),
                        crate::key::Code::Esc => self.settings.value_edit = None,
                        crate::key::Code::Backspace if !key.ctrl => {
                            if let Some((_, buf)) = &mut self.settings.value_edit {
                                buf.pop();
                            }
                        }
                        crate::key::Code::Char(c) if !key.ctrl => {
                            if let Some((_, buf)) = &mut self.settings.value_edit {
                                buf.push(c);
                            }
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        crate::key::Code::Esc => {
                            self.close_settings(cx);
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        crate::key::Code::Up => {
                            if let Some(i) = self.settings.focus {
                                self.settings.focus = Some(i.saturating_sub(1));
                            } else if !self.settings.items.is_empty() {
                                self.settings.focus = Some(self.settings.items.len() - 1);
                            }
                        }
                        crate::key::Code::Down => {
                            let n = self.settings.items.len();
                            if n == 0 {
                            } else if let Some(i) = self.settings.focus {
                                self.settings.focus = Some((i + 1).min(n - 1));
                            } else {
                                self.settings.focus = Some(0);
                            }
                        }
                        crate::key::Code::Enter => {
                            if let Some(i) = self.settings.focus {
                                self.start_settings_value_edit(i);
                            }
                        }
                        _ => {}
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if ctrl && (k.key == "tab" || k.key == "pagedown" || k.key == "pageup") {
            let back = k.key == "pageup" || k.modifiers.shift;
            self.cycle_mode(if back { -1 } else { 1 }, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !matches!(self.mode, Mode::Edit | Mode::Virtual) {
            if self.mode == Mode::Course && self.course.name_edit.is_some() {
                if let Some(key) = crate::key::Key::from_gpui(
                    &k.key,
                    k.key_char.as_deref(),
                    ctrl,
                    k.modifiers.shift,
                    k.modifiers.alt,
                ) {
                    match key.code {
                        crate::key::Code::Enter => self.commit_course_rename(),
                        crate::key::Code::Esc => self.course.name_edit = None,
                        crate::key::Code::Backspace if !key.ctrl => {
                            if let Some((_, buf)) = &mut self.course.name_edit {
                                buf.pop();
                            }
                        }
                        crate::key::Code::Char(c) if !key.ctrl => {
                            if let Some((_, buf)) = &mut self.course.name_edit {
                                buf.push(c);
                            }
                        }
                        _ => {}
                    }
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if self.mode == Mode::Roster {
                if self.roster.edit.is_some() {
                    if let Some(key) = crate::key::Key::from_gpui(
                        &k.key,
                        k.key_char.as_deref(),
                        ctrl,
                        k.modifiers.shift,
                        k.modifiers.alt,
                    ) {
                        match key.code {
                            crate::key::Code::Enter => self.commit_roster_edit(),
                            crate::key::Code::Esc => self.roster.edit = None,
                            crate::key::Code::Backspace if !key.ctrl => {
                                if let Some((_, _, buf)) = &mut self.roster.edit {
                                    buf.pop();
                                }
                            }
                            crate::key::Code::Char(c) if !key.ctrl => {
                                if let Some((_, _, buf)) = &mut self.roster.edit {
                                    buf.push(c);
                                }
                            }
                            _ => {}
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if let Some(key) = crate::key::Key::from_gpui(
                    &k.key,
                    k.key_char.as_deref(),
                    ctrl,
                    k.modifiers.shift,
                    k.modifiers.alt,
                ) {
                    match key.code {
                        crate::key::Code::Delete | crate::key::Code::Backspace => {
                            self.roster_delete_focused();
                        }
                        crate::key::Code::Up => {
                            if let Some(i) = self.roster.focus {
                                self.roster.focus = Some(i.saturating_sub(1));
                            } else if !self.roster.entries.is_empty() {
                                self.roster.focus = Some(self.roster.entries.len() - 1);
                            }
                        }
                        crate::key::Code::Down => {
                            let n = self.roster.entries.len();
                            if n == 0 {
                            } else if let Some(i) = self.roster.focus {
                                self.roster.focus = Some((i + 1).min(n - 1));
                            } else {
                                self.roster.focus = Some(0);
                            }
                        }
                        crate::key::Code::Enter => {
                            if let Some(i) = self.roster.focus {
                                self.start_roster_edit(i, crate::ui_roster::Field::Name);
                            }
                        }
                        _ => {}
                    }
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if self.mode == Mode::Manager && self.ctrl.dev_edit.is_some() {
                if let Some(key) = crate::key::Key::from_gpui(
                    &k.key,
                    k.key_char.as_deref(),
                    ctrl,
                    k.modifiers.shift,
                    k.modifiers.alt,
                ) {
                    match key.code {
                        crate::key::Code::Enter => self.commit_mgr_rename(),
                        crate::key::Code::Esc => self.ctrl.dev_edit = None,
                        crate::key::Code::Backspace if !key.ctrl => {
                            if let Some(e) = &mut self.ctrl.dev_edit {
                                e.buf.pop();
                            }
                        }
                        crate::key::Code::Char(c) if !key.ctrl => {
                            if let Some(e) = &mut self.ctrl.dev_edit {
                                e.buf.push(c);
                            }
                        }
                        _ => {}
                    }
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if let Some(key) = crate::key::Key::from_gpui(
                &k.key,
                k.key_char.as_deref(),
                ctrl,
                k.modifiers.shift,
                k.modifiers.alt,
            ) {
                if let Some(cmd) = self.ctrl.handle_key(key) {
                    self.dispatch(cmd, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if let Some((_, buf)) = &mut self.edit {
            match k.key.as_str() {
                "enter" | "tab" => self.commit_edit(),
                "escape" => self.cancel_edit(),
                "backspace" => {
                    if self.edit_sel_all {
                        buf.clear();
                        self.edit_sel_all = false;
                    } else {
                        buf.pop();
                    }
                }
                _ if !ctrl && !k.modifiers.alt => {
                    if let Some(c) = &k.key_char {
                        if self.edit_sel_all {
                            buf.clear();
                            self.edit_sel_all = false;
                        }
                        buf.push_str(c);
                    }
                }
                _ => return,
            }
        } else if self.mode == Mode::Virtual {
            if let Some(i) = match k.key.as_str() {
                "1" => Some(0),
                "2" => Some(1),
                "3" => Some(2),
                "4" => Some(3),
                _ => None,
            } {
                let hit = if self.sensors_on() {
                    ZoneHit::Sensor(i)
                } else {
                    ZoneHit::Hub(i)
                };
                self.fire(hit, "manual");
            } else {
                return;
            }
        } else {
            match k.key.as_str() {
                "escape" => {
                    if self.sel_point.is_some() {
                        self.sel_point = None;
                    } else {
                        self.selected = None;
                    }
                }
                "delete" | "backspace" => {
                    if self.sel_point.is_some() {
                        self.delete_point();
                    } else if k.key == "delete"
                        && let Some(i) = self.selected
                    {
                        self.delete_poly(i);
                    }
                }
                _ => return,
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn to_img(&self, p: Point<Pixels>) -> (f32, f32) {
        let (ox, oy, s) = fit(self.bounds.get(), self.img_size);
        let s = s.max(1e-6);
        (
            ((f32::from(p.x) - ox) / s).clamp(0.0, self.img_size.0 as f32),
            ((f32::from(p.y) - oy) / s).clamp(0.0, self.img_size.1 as f32),
        )
    }

    fn inside_img(&self, p: Point<Pixels>) -> bool {
        let (ox, oy, s) = fit(self.bounds.get(), self.img_size);
        let (x, y) = (f32::from(p.x), f32::from(p.y));
        x >= ox
            && y >= oy
            && x <= ox + self.img_size.0 as f32 * s
            && y <= oy + self.img_size.1 as f32 * s
    }

    fn to_screen(&self, p: (f32, f32)) -> (f32, f32) {
        let (ox, oy, s) = fit(self.bounds.get(), self.img_size);
        (ox + p.0 * s, oy + p.1 * s)
    }

    /// Polygon order for hit tests: selected first.
    fn hit_order(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.polys.len())
            .filter(|&i| !self.polys[i].hidden)
            .collect();
        if let Some(s) = self.selected {
            v.retain(|&i| i != s);
            if !self.polys.get(s).is_some_and(|p| p.hidden) {
                v.insert(0, s);
            }
        }
        v
    }

    fn hit_fill(&self, m: Point<Pixels>) -> Option<usize> {
        let (x, y) = self.to_img(m);
        self.hit_order()
            .into_iter()
            .find(|&pi| targetlib::contains(&self.polys[pi].points, x, y))
    }

    fn hit_point(&self, m: Point<Pixels>) -> Option<(usize, usize)> {
        let m = (f32::from(m.x), f32::from(m.y));
        for pi in self.hit_order() {
            for (vi, &p) in self.polys[pi].points.iter().enumerate() {
                let s = self.to_screen(p);
                if ((s.0 - m.0).powi(2) + (s.1 - m.1).powi(2)).sqrt() <= HIT {
                    return Some((pi, vi));
                }
            }
        }
        None
    }

    /// (polygon, index of the edge's first vertex).
    fn hit_edge(&self, m: Point<Pixels>) -> Option<(usize, usize)> {
        let m = (f32::from(m.x), f32::from(m.y));
        for pi in self.hit_order() {
            let pts = &self.polys[pi].points;
            for vi in 0..pts.len() {
                let a = self.to_screen(pts[vi]);
                let b = self.to_screen(pts[(vi + 1) % pts.len()]);
                if seg_dist(m, a, b) <= HIT {
                    return Some((pi, vi));
                }
            }
        }
        None
    }

    fn on_virtual_canvas_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.image.is_none() {
            return;
        }
        let (x, y) = self.to_img(ev.position);
        if let Some(pi) = targetlib::hit_poly(&self.polys, x, y)
            && let Some(s) = self.zone_map.get(pi).copied().flatten()
        {
            let name = self.polys[pi].name.clone();
            self.fire(s, &name);
        }
        cx.notify();
    }

    fn on_left_down(&mut self, ev: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(w);
        if self.mode == Mode::Virtual {
            self.on_virtual_canvas_down(ev, cx);
            return;
        }
        if self.image.is_none() {
            return;
        }
        self.commit_edit();
        let m = ev.position;
        if let Some((pi, vi)) = self.hit_point(m) {
            self.selected = Some(pi);
            self.sel_point = Some(vi);
            if ev.click_count >= 2 {
                self.delete_point();
            } else {
                self.drag = Some((pi, vi));
            }
        } else if let Some((pi, vi)) = self.hit_edge(m) {
            if ev.click_count >= 2 {
                let p = self.to_img(m);
                self.polys[pi].points.insert(vi + 1, p);
                self.selected = Some(pi);
                self.sel_point = Some(vi + 1);
                self.drag = Some((pi, vi + 1));
                self.dirty = true;
            } else {
                self.selected = Some(pi);
                self.sel_point = None;
                self.poly_move = Some((pi, self.to_img(m)));
            }
        } else if let Some(pi) = self.hit_fill(m) {
            self.selected = Some(pi);
            self.sel_point = None;
            self.poly_move = Some((pi, self.to_img(m)));
        } else if self.inside_img(m) {
            self.selected = None;
            self.sel_point = None;
        }
        cx.notify();
    }

    fn on_right_down(&mut self, _ev: &MouseDownEvent, _w: &mut Window, _cx: &mut Context<Self>) {}

    fn on_move(&mut self, ev: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Virtual || self.image.is_none() {
            return;
        }
        let p = self.to_img(ev.position);
        self.hover_img = Some(p);
        if let Some((pi, vi)) = self.drag {
            if ev.pressed_button == Some(MouseButton::Left) {
                if let Some(pt) = self.polys.get_mut(pi).and_then(|q| q.points.get_mut(vi))
                    && *pt != p
                {
                    *pt = p;
                    self.dirty = true;
                }
            } else {
                self.drag = None;
            }
        } else if let Some((pi, last)) = self.poly_move {
            if ev.pressed_button == Some(MouseButton::Left) {
                let (dx, dy) = (p.0 - last.0, p.1 - last.1);
                if (dx != 0.0 || dy != 0.0)
                    && let Some(poly) = self.polys.get_mut(pi)
                {
                    for pt in &mut poly.points {
                        pt.0 += dx;
                        pt.1 += dy;
                    }
                    self.poly_move = Some((pi, p));
                    self.dirty = true;
                }
            } else {
                self.poly_move = None;
            }
        }
        cx.notify();
    }

    fn on_left_up(&mut self, _ev: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let mut n = false;
        if self.drag.take().is_some() {
            n = true;
        }
        if self.finish_list_drag() {
            n = true;
        }
        if self.poly_move.take().is_some() {
            n = true;
        }
        if n {
            cx.notify();
        }
    }

    fn field(&self, f: Field, value: String, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let editing = matches!(&self.edit, Some((ef, _)) if *ef == f);
        let list_edit = editing && matches!(f, Field::Name(_) | Field::Score(_));
        let text = match &self.edit {
            Some((ef, b)) if *ef == f => format!("{b}▏"),
            _ => value,
        };
        let id: SharedString = match f {
            Field::Length => "f-length".into(),
            Field::Width => "f-width".into(),
            Field::Name(i) => format!("f-name-{i}").into(),
            Field::Score(i) => format!("f-score-{i}").into(),
            Field::Path => "f-path".into(),
            Field::SavePath => "f-save-path".into(),
        };
        div()
            .id(id)
            .min_w(px(80.))
            .px_2()
            .py_0p5()
            .rounded_sm()
            .border_1()
            .border_color(rgb(if editing { t.accent } else { t.border }))
            .bg(rgb(if list_edit { 0x000000 } else { t.background }))
            .text_color(rgb(if list_edit { 0xffffff } else { t.foreground }))
            .child(text)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.commit_edit();
                this.start_edit(f);
                cx.notify();
            }))
    }

    fn button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        div()
            .id(id.into())
            .px_2()
            .py_0p5()
            .rounded_sm()
            .bg(rgb(t.surface))
            .border_1()
            .border_color(rgb(t.border))
            .hover(|s| s.border_color(rgb(t.accent)))
            .child(label.into())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                f(this, cx);
                cx.notify();
            }))
    }

    fn title(&self) -> String {
        let name = self
            .image
            .as_ref()
            .map(|p| p.name.clone())
            .or_else(|| {
                self.save_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "no file".into());
        let dirty = if self.dirty { "● " } else { "" };
        match self.mode {
            Mode::Virtual => format!("{dirty}virtual — {name} - target-editor"),
            Mode::Edit => format!("{dirty}{name} - target-editor"),
            Mode::Flash => "flash — target-editor".into(),
            Mode::Manager => "manager — target-editor".into(),
            Mode::Course => "stages — target-editor".into(),
            Mode::Roster => "roster — target-editor".into(),
            Mode::Settings => "settings — target-editor".into(),
        }
    }

    fn menu_item(
        &self,
        id: &'static str,
        label: &'static str,
        shortcut: Option<&'static str>,
        checked: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        let text = if checked {
            format!("✓  {label}")
        } else {
            format!("    {label}")
        };
        div()
            .id(id)
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_6()
            .px_3()
            .py_1()
            .text_color(rgb(if checked { t.selection_fg } else { t.foreground }))
            .bg(rgb(if checked { t.selection_bg } else { t.background }))
            .hover(|s| s.bg(rgb(t.row_alt)))
            .child(text)
            .child(
                div()
                    .text_color(rgb(if checked { t.selection_fg } else { t.muted }))
                    .child(shortcut.unwrap_or("").to_string()),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.menu_open = false;
                this.flash_drop = None;
                this.course.roster_drop = false;
                this.course.script_drop = false;
                this.course.hub_drop = None;
                f(this, cx);
                cx.notify();
            }))
    }

    fn poly_ctx_item(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        div()
            .id(id.into())
            .w_full()
            .px_3()
            .py_1()
            .hover(|s| s.bg(rgb(t.row_alt)))
            .child(label.into())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                f(this, cx);
                this.poly_menu = None;
                cx.notify();
            }))
    }

    fn poly_ctx_menu(&self, i: usize, cx: &mut Context<Self>) -> gpui::Div {
        let t = self.theme;
        let hidden = self.polys.get(i).is_some_and(|p| p.hidden);
        div()
            .flex()
            .flex_col()
            .min_w(px(140.))
            .bg(rgb(t.background))
            .border_1()
            .border_color(rgb(t.border))
            .occlude()
            .child(self.poly_ctx_item(
                format!("ctx-hide-{i}"),
                if hidden { "Show" } else { "Hide" },
                cx,
                move |this, _| this.toggle_hidden(i),
            ))
            .child(self.poly_ctx_item(
                format!("ctx-dup-{i}"),
                "Duplicate",
                cx,
                move |this, _| this.duplicate_poly(i),
            ))
            .child(self.poly_ctx_item(
                format!("ctx-del-{i}"),
                "Delete",
                cx,
                move |this, _| this.delete_poly(i),
            ))
    }

    fn app_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = self.theme;
        let mode = self.mode;
        let group = |first: bool| {
            let d = div().flex().flex_col();
            if first {
                d
            } else {
                d.border_t_1().border_color(rgb(t.border))
            }
        };
        div()
            .flex()
            .flex_col()
            .min_w(px(280.))
            .bg(rgb(t.background))
            .border_1()
            .border_color(rgb(t.border))
            .occlude()
            .child(
                group(true)
                    .child(self.menu_item("m-open", "Open", Some("Ctrl+O"), false, cx, |this, cx| {
                        this.prompt_open(cx);
                    }))
                    .child(self.menu_item("m-save", "Save", Some("Ctrl+S"), false, cx, |this, cx| {
                        this.save(cx);
                    }))
                    .child(self.menu_item(
                        "m-save-as",
                        "Save As",
                        Some("Ctrl+Shift+S"),
                        false,
                        cx,
                        |this, cx| this.save_as(cx),
                    ))
                    .child(self.menu_item("m-close", "Close", Some("Ctrl+W"), false, cx, |this, _| {
                        this.close_doc();
                    })),
            )
            .child(
                group(false)
                    .child(self.menu_item(
                        "m-mgr",
                        "Device Connections",
                        None,
                        mode == Mode::Manager,
                        cx,
                        |this, cx| this.set_mode(Mode::Manager, cx),
                    ))
                    .child(self.menu_item(
                        "m-course",
                        "Stages",
                        None,
                        mode == Mode::Course,
                        cx,
                        |this, cx| this.set_mode(Mode::Course, cx),
                    ))
                    .child(self.menu_item(
                        "m-roster",
                        "Roster",
                        None,
                        mode == Mode::Roster,
                        cx,
                        |this, cx| this.set_mode(Mode::Roster, cx),
                    ))
                    .child(self.menu_item(
                        "m-edit",
                        "Target Editor",
                        None,
                        mode == Mode::Edit,
                        cx,
                        |this, cx| this.set_mode(Mode::Edit, cx),
                    ))
                    .child(self.menu_item(
                        "m-flash",
                        "Flash Device",
                        None,
                        mode == Mode::Flash,
                        cx,
                        |this, cx| this.set_mode(Mode::Flash, cx),
                    ))
                    .child(self.menu_item(
                        "m-settings",
                        "Settings",
                        None,
                        mode == Mode::Settings,
                        cx,
                        |this, cx| this.set_mode(Mode::Settings, cx),
                    )),
            )
            .child(
                group(false).child(self.menu_item("m-quit", "Quit", Some("Ctrl+Q"), false, cx, |_this, cx| {
                    cx.quit();
                })),
            )
    }

    fn toggle_btn(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded_sm()
            .bg(rgb(if on { t.selection_bg } else { t.surface }))
            .text_color(rgb(if on { t.selection_fg } else { t.foreground }))
            .border_1()
            .border_color(rgb(if on { t.accent } else { t.border }))
            .hover(|s| s.border_color(rgb(t.accent)))
            .child(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                f(this, cx);
                cx.notify();
            }))
    }

    fn card_chrome(&self, s: &Snapshot) -> (u32, u32) {
        let t = self.theme;
        let connected = s.link.is_connected();
        let border = if s.hit {
            t.selection_bg
        } else if connected {
            t.accent
        } else {
            t.border
        };
        let status_color = if connected { t.accent } else { t.muted };
        (border, status_color)
    }

    fn card_header(&self, s: &Snapshot, status_color: u32) -> gpui::Div {
        let t = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_1()
            .child(div().text_color(rgb(t.accent)).child(s.name.clone()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(div().size(px(8.)).rounded_full().bg(rgb(status_color)))
                    .child(
                        div()
                            .text_color(rgb(status_color))
                            .child(s.link.label().to_string()),
                    ),
            )
    }

    fn card(&self, s: &Snapshot, cx: &mut Context<Self>) -> gpui::Div {
        let t = self.theme;
        let i = s.index;
        let kind = s.kind;
        let (border, status_color) = self.card_chrome(s);
        let mut body = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(t.surface))
            .child(self.card_header(s, status_color))
            .child(
                div()
                    .text_color(rgb(t.muted))
                    .child(format!(
                        "{}  {}",
                        s.adapter.as_deref().unwrap_or("—"),
                        s.adapter_addr.as_deref().unwrap_or("")
                    )),
            )
            .child(div().child(format!("uptime {}", virt_ui::uptime(s.uptime_ms))))
            .child(
                div()
                    .text_color(rgb(t.muted))
                    .child(format!("temp {}", virt_ui::temp(s.temp))),
            )
            .child(
                div()
                    .text_color(rgb(t.muted))
                    .child(virt_ui::battery(s.battery_v, s.battery_pct)),
            )
            .child(
                div()
                    .text_color(rgb(t.muted))
                    .child(format!("accel {}", virt_ui::accel(s.accel))),
            );
        if kind == Kind::Hub {
            let mut channels = div().flex().flex_col().gap_1();
            for ch in 0..CHANNELS {
                let v = s.sensors[ch];
                let hot = v >= THRESHOLD;
                channels = channels.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .text_color(rgb(if hot { t.accent } else { t.muted }))
                                .child(format!("H{}  {v}", ch + 1)),
                        )
                        .child(self.button(format!("hit-h-{ch}"), "Hit", cx, move |this, _| {
                            this.fire(ZoneHit::Hub(ch), "manual");
                        })),
                );
            }
            body = body.child(channels);
        } else {
            body = body.child(self.button(format!("hit-s-{i}"), "Hit", cx, move |this, _| {
                this.fire(ZoneHit::Sensor(i), "manual");
            }));
        }
        body.child(self.button(
            format!("adv-{i}"),
            if s.advertise { "Advertising" } else { "Advertise" },
            cx,
            move |this, _| {
                if let Some(hub) = &this.hub {
                    let b = if kind == Kind::Hub {
                        &hub.hub
                    } else {
                        &hub.sensors[i]
                    };
                    let on = !b.advertise();
                    b.set_advertise(on);
                }
            },
        ))
        .when(!s.error.is_empty(), |d| {
            d.child(div().text_color(rgb(t.accent)).child(s.error.clone()))
        })
    }
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(&self.title());
        if self.mode == Mode::Flash {
            let n = self.flash_output_lines(window);
            if self.ctrl.output_height != n {
                self.ctrl.output_height = n;
                cx.notify();
            }
        }
        let t = self.theme;
        let u = self.unit;

        // Canvas: fitted image + overlay.
        let bounds_cell = self.bounds.clone();
        let polys = self.polys.clone();
        let selected = self.selected;
        let img_size = self.img_size;
        let virt = self.mode == Mode::Virtual;
        let host = cx.entity();
        let overlay = canvas(
            move |b, _, _| {
                bounds_cell.set(b);
                b
            },
            move |_, b, window, _| {
                let (ox, oy, s) = fit(b, img_size);
                let sp = |p: &(f32, f32)| point(px(ox + p.0 * s), px(oy + p.1 * s));
                {
                    let mut draw = |pts: &[(f32, f32)], fill_col: u32| {
                        if pts.is_empty() {
                            return;
                        }
                        let screen: Vec<Point<Pixels>> = pts.iter().map(sp).collect();
                        if pts.len() >= 3 {
                            let mut f = PathBuilder::fill();
                            f.add_polygon(&screen, true);
                            if let Ok(path) = f.build() {
                                window.paint_path(path, alpha(fill_col, POLY_FILL_A));
                            }
                        }
                        if pts.len() >= 2 {
                            let mut st = PathBuilder::stroke(px(1.));
                            st.add_polygon(&screen, true);
                            if let Ok(path) = st.build() {
                                window.paint_path(path, rgb(POLY_STROKE));
                            }
                        }
                    };
                    for p in &polys {
                        if p.hidden {
                            continue;
                        }
                        draw(&p.points, targetlib::poly_color(p.color));
                    }
                }
                if !virt
                    && let Some(i) = selected
                    && let Some(p) = polys.get(i)
                    && !p.hidden
                {
                    let fill_col = targetlib::poly_color(p.color);
                    let hs = px(5.);
                    let h = px(2.5);
                    for pt in &p.points {
                        let p = sp(pt);
                        window.paint_quad(fill(
                            Bounds::new(point(p.x - h, p.y - h), size(hs, hs)),
                            rgb(fill_col),
                        ));
                        let box_pts = [
                            point(p.x - h, p.y - h),
                            point(p.x + h, p.y - h),
                            point(p.x + h, p.y + h),
                            point(p.x - h, p.y + h),
                        ];
                        let mut st = PathBuilder::stroke(px(1.));
                        st.add_polygon(&box_pts, true);
                        if let Ok(path) = st.build() {
                            window.paint_path(path, rgb(POLY_STROKE));
                        }
                    }
                }
                let c = sp(&(img_size.0 as f32 / 2.0, img_size.1 as f32 / 2.0));
                let arm = px(2.5);
                for seg in [
                    [point(c.x - arm, c.y), point(c.x + arm, c.y)],
                    [point(c.x, c.y - arm), point(c.x, c.y + arm)],
                ] {
                    let mut plus = PathBuilder::stroke(px(1.));
                    plus.add_polygon(&seg, false);
                    if let Ok(path) = plus.build() {
                        window.paint_path(path, rgb(0xff0000));
                    }
                }
                let host = host.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || !b.contains(&ev.position) {
                        return;
                    }
                    host.update(cx, |this, cx| {
                        if this.mode == Mode::Virtual || this.image.is_none() {
                            return;
                        }
                        let p = this.to_img(ev.position);
                        if this.hover_img != Some(p) {
                            this.hover_img = Some(p);
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full();

        let mut area = div()
            .id("canvas")
            .flex_1()
            .relative()
            .overflow_hidden()
            .bg(rgb(t.background))
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_left_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_left_up))
            .on_mouse_move(cx.listener(Self::on_move));
        if let Some(p) = &self.image {
            area = area.child(
                img(p.gpui.clone())
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
        } else {
            area = area.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(t.muted))
                    .child("Ctrl+O to open a PNG/JPEG or a saved .json"),
            );
        }
        area = area.child(overlay);

        let body = if self.mode == Mode::Virtual {
            let sensors_on = self.sensors_on();
            let hub_on = self.hub_on();
            let mut zones = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .p_2()
                .child(self.toggle_btn("virt-sensors", "Sensors", sensors_on, cx, |this, _| {
                    if let Some(hub) = &this.hub {
                        hub.set_sensors(!hub.sensors_on());
                    }
                    this.sync_zone_map();
                }))
                .child(self.toggle_btn("virt-hub", "Hub", hub_on, cx, |this, _| {
                    if let Some(hub) = &this.hub {
                        hub.set_hub(!hub.hub_on());
                    }
                    this.sync_zone_map();
                }));
            for (i, p) in self.polys.iter().enumerate() {
                if p.hidden {
                    continue;
                }
                let map = self.zone_map.get(i).copied().flatten();
                zones = zones.child(self.button(
                    format!("z-{i}"),
                    format!("{} → {}", p.name, virt_ui::zone_label(map)),
                    cx,
                    move |this, _| {
                        let sensors = this.sensors_on();
                        let hub = this.hub_on();
                        if let Some(slot) = this.zone_map.get_mut(i) {
                            *slot = virt_ui::cycle_zone(*slot, sensors, hub);
                        }
                    },
                ));
            }
            let mut cards = div()
                .id("cards")
                .w(relative(0.25))
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .overflow_y_scroll();
            for s in self.snaps.clone() {
                cards = cards.child(self.card(&s, cx));
            }
            div()
                .flex_1()
                .flex()
                .flex_row()
                .child(div().flex_1().flex().flex_col().child(area).child(zones))
                .child(cards)
        } else {
            // Side panel.
            let mut list = div()
                .id("polys")
                .flex()
                .flex_col()
                .gap_1()
                .overflow_y_scroll()
                .flex_1()
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if this.finish_list_drag() {
                            cx.notify();
                        }
                    }),
                );
            list = list.child(
                chrome::table_row()
                    .child(chrome::cell(1.0, div().text_color(rgb(t.muted)).child("Color")))
                    .child(chrome::cell(3.0, div().text_color(rgb(t.muted)).child("Name")))
                    .child(chrome::cell(
                        1.5,
                        div()
                            .w_full()
                            .flex()
                            .justify_end()
                            .text_right()
                            .text_color(rgb(t.muted))
                            .child("Score"),
                    )),
            );
            let n = self.polys.len();
            for i in (0..n).rev() {
                let p = &self.polys[i];
                let sel = Some(i) == self.selected;
                let renaming = matches!(self.edit, Some((Field::Name(j), _)) if j == i);
                let scoring = matches!(self.edit, Some((Field::Score(j), _)) if j == i);
                let name: gpui::AnyElement = if renaming {
                    self.field(Field::Name(i), String::new(), cx)
                        .into_any_element()
                } else {
                    div()
                        .id(("sel", i))
                        .w_full()
                        .overflow_hidden()
                        .child(p.name.clone())
                        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                            if ev.click_count() >= 2 {
                                this.commit_edit();
                                this.selected = Some(i);
                                this.sel_point = None;
                                this.start_edit(Field::Name(i));
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                };
                let score: gpui::AnyElement = if scoring {
                    self.field(Field::Score(i), String::new(), cx)
                        .into_any_element()
                } else {
                    div()
                        .id(("score", i))
                        .w_full()
                        .child(p.score.to_string())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.commit_edit();
                            this.selected = Some(i);
                            this.start_edit(Field::Score(i));
                            cx.notify();
                        }))
                        .into_any_element()
                };
                list = list.child(
                    chrome::table_row()
                        .id(("poly-row", i as u64))
                        .p_1()
                        .rounded_sm()
                        .when(sel, |d| {
                            d.bg(rgb(t.selection_bg)).text_color(rgb(t.selection_fg))
                        })
                        .when(p.hidden && !sel, |d| d.text_color(rgb(t.muted)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.commit_edit();
                                this.list_down_sel = this.selected == Some(i);
                                this.list_drag_moved = false;
                                if this.selected != Some(i) {
                                    this.selected = Some(i);
                                    this.sel_point = None;
                                }
                                this.list_drag = Some(i);
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                this.commit_edit();
                                this.selected = Some(i);
                                this.poly_menu = Some((i, ev.position));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered
                                && let Some(from) = this.list_drag
                                && from != i
                            {
                                this.list_drag_moved = true;
                                this.reorder_poly(from, i);
                                this.list_drag = Some(i);
                                cx.notify();
                            }
                        }))
                        .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            if let Some(from) = this.list_drag
                                && from != i
                            {
                                this.list_drag_moved = true;
                                this.reorder_poly(from, i);
                                this.list_drag = Some(i);
                                cx.notify();
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                this.finish_list_drag();
                                cx.notify();
                            }),
                        )
                        .child(chrome::cell(
                            1.0,
                            div()
                                .id(("sw", i))
                                .size(px(12.))
                                .flex_none()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(t.border))
                                .bg(rgba((targetlib::poly_color(p.color) << 8) | 0xbf))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.commit_edit();
                                    if let Some(p) = this.polys.get_mut(i) {
                                        p.color = (p.color + 1) % targetlib::PALETTE.len();
                                        this.dirty = true;
                                    }
                                    this.selected = Some(i);
                                    cx.notify();
                                })),
                        ))
                        .child(chrome::cell(3.0, name))
                        .child(chrome::cell(
                            1.5,
                            div().w_full().flex().justify_end().text_right().child(score),
                        )),
                );
            }
            let panel = div()
                .w(relative(0.25))
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .bg(rgb(t.surface))
                .border_l_1()
                .border_color(rgb(t.border))
                .child(
                    chrome::table_row()
                        .child(chrome::cell(1.0, div().text_color(rgb(t.muted)).child("Length")))
                        .child(chrome::cell(1.0, div().text_color(rgb(t.muted)).child("Width")))
                        .child(chrome::cell(1.0, div().text_color(rgb(t.muted)).child("Unit"))),
                )
                .child(
                    chrome::table_row()
                        .child(chrome::cell(
                            1.0,
                            self.field(Field::Length, self.fmt_dim(self.length_cm), cx),
                        ))
                        .child(chrome::cell(
                            1.0,
                            self.field(Field::Width, self.fmt_dim(self.width_cm), cx),
                        ))
                        .child(chrome::cell(
                            1.0,
                            self.button("unit", u.label().to_string(), cx, |this, _| {
                                this.commit_edit();
                                this.unit = match this.unit {
                                    Unit::Cm => Unit::In,
                                    Unit::In => Unit::Cm,
                                };
                                this.dirty = true;
                            }),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(div().text_color(rgb(t.muted)).child("Zones"))
                        .child(self.button("add-poly", "Add", cx, |this, _| {
                            this.commit_edit();
                            this.add_polygon();
                        })),
                )
                .child(list)
                .child({
                    let vals = self.hover_img.and_then(|(mx, my)| {
                        if let Some(p) = self
                            .selected
                            .and_then(|i| self.polys.get(i))
                            .filter(|p| !p.hidden)
                        {
                            Self::corner_px(&p.points, mx, my)
                        } else {
                            let (w, h) = (self.img_size.0 as f32, self.img_size.1 as f32);
                            Self::corner_px(&[(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)], mx, my)
                        }
                    });
                    let cell = |s: String| {
                        chrome::cell(
                            1.0,
                            div()
                                .w_full()
                                .flex()
                                .justify_center()
                                .text_color(rgb(t.muted))
                                .child(s),
                        )
                    };
                    let lab = |s: &str| cell(s.to_string());
                    let num = |i: usize| {
                        cell(vals.map(|v| v[i].to_string()).unwrap_or_else(|| "—".into()))
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_1()
                        .border_t_1()
                        .border_color(rgb(t.border))
                        .child(chrome::table_row().child(lab("TL")).child(lab("TR")).child(lab("BL")).child(lab("BR")))
                        .child(chrome::table_row().child(num(0)).child(num(1)).child(num(2)).child(num(3)))
                });
            div().flex_1().flex().flex_row().child(area).child(panel)
        };

        let body = match self.mode {
            Mode::Flash => crate::ui_flash::view(self, cx),
            Mode::Manager => crate::ui_manager::view(self, cx),
            Mode::Course => crate::ui_course::view(self, cx),
            Mode::Roster => crate::ui_roster::view(self, cx),
            Mode::Settings => crate::ui_settings::view(self, cx),
            Mode::Edit | Mode::Virtual => body,
        };

        let menu_on = self.menu_open;
        let chrome_btn = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .w(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if on { t.accent } else { t.border }))
                .bg(rgb(if on { t.selection_bg } else { t.surface }))
                .text_color(rgb(if on { t.selection_fg } else { t.foreground }))
                .hover(|s| s.border_color(rgb(t.accent)))
                .child(label)
        };
        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .px_2()
            .py_1()
            .bg(rgb(t.surface))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div().w(px(36.)).flex().items_center().justify_start().child(
                    chrome_btn("menu-btn", "☰", menu_on).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.menu_open = !this.menu_open;
                            this.flash_drop = None;
                            this.course.roster_drop = false;
                            this.course.script_drop = false;
                            this.course.hub_drop = None;
                            cx.notify();
                        },
                    )),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.mode.bar_title().unwrap_or("").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(
                        chrome_btn("win-settings", "⚙", self.mode == Mode::Settings).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.set_mode(Mode::Settings, cx);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(chrome_btn("win-close", "×", false).on_click(cx.listener(
                        |_this, _: &ClickEvent, _, cx| {
                            cx.quit();
                        },
                    ))),
            );

        let mut root = div()
            .key_context("TargetEditor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(t.background))
            .text_color(rgb(t.foreground))
            .font_family(self.ui_font.family.clone())
            .text_size(px(self.ui_font.size_px))
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .child(body),
            );
        if let Some((f @ (Field::Path | Field::SavePath), _)) = self.edit {
            let label = if f == Field::Path {
                "Open path"
            } else {
                "Save path"
            };
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .bg(rgb(t.surface))
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .child(div().text_color(rgb(t.muted)).child(label))
                    .child(div().flex_1().child(self.field(f, String::new(), cx))),
            );
        }
        if self.menu_open {
            root = root
                .child(
                    div()
                        .id("menu-scrim")
                        .absolute()
                        .top(px(36.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.menu_open = false;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(36.))
                        .left(px(8.))
                        .font_family(self.ui_font.family.clone())
                        .text_size(px(self.ui_font.size_px))
                        .child(self.app_menu(cx)),
                );
        }
        if let Some((i, pos)) = self.poly_menu {
            root = root
                .child(
                    div()
                        .id("poly-menu-scrim")
                        .absolute()
                        .size_full()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.poly_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.poly_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(pos.x)
                        .top(pos.y)
                        .font_family(self.ui_font.family.clone())
                        .text_size(px(self.ui_font.size_px))
                        .child(self.poly_ctx_menu(i, cx)),
                );
        }
        root
    }
}

