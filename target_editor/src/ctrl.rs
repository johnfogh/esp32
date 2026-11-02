//! UI-independent application state and key handling.
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use utils::host::Chip;

use devicehostlib::central::{self, BleEvent, DeviceInfo};
use devicehostlib::devices::{self, Devices};
use devicehostlib::linked::{
    apply_telemetry, cmp_ts_keys, last_sample_from_line, virt_ingest_default, LastMsg, Linked,
};
use crate::key::{Code, Key};
use crate::theme::Theme;

/// Focusable areas, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Chips,
    Projects,
    /// Connection Manager device cards.
    Devices,
    Output,
    /// Stages events list.
    CourseEvents,
    /// Stages target cards.
    CourseTargets,
}

/// Top-level views, in tab-bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Flash,
    Manager,
    Course,
}

/// Inline rename of a Device Connections card.
#[derive(Debug, Clone)]
pub struct DevEdit {
    pub virt_mac: Option<String>,
    pub addr: Option<String>,
    pub buf: String,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Flash, Tab::Manager, Tab::Course];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
    /// Focusable controls of this view, in Tab order.
    pub fn order(self) -> &'static [Focus] {
        match self {
            Tab::Flash => &[Focus::Chips, Focus::Projects],
            Tab::Manager => &[Focus::Devices],
            Tab::Course => &[Focus::CourseEvents, Focus::CourseTargets],
        }
    }
}

impl Focus {
    /// Next focus in Tab order within `tab`'s view.
    fn step(self, d: isize, tab: Tab) -> Focus {
        let order = tab.order();
        let i = order.iter().position(|f| *f == self).unwrap_or(0) as isize;
        order[(i + d).rem_euclid(order.len() as isize) as usize]
    }
}

/// Maximum lines kept in the event log. Oldest lines are dropped when full.
pub const LOG_CAP: usize = 5000;

/// How long a disconnect alert stays in the status line.
#[cfg(test)]
pub const ALERT_TIME: std::time::Duration = std::time::Duration::from_secs(15);

/// What the event loop must do after a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Quit,
    Build,
    Flash,
    Refresh,
}

/// Messages from worker threads.
pub enum Msg {
    Line(String),
    /// Build/flash finished (`None` = could not start / killed).
    JobDone(Option<i32>),
    Boards(Vec<(String, Chip)>),
    Ble(BleEvent),
}

pub struct App {
    pub projects: Vec<PathBuf>,
    pub project: Option<usize>,
    pub project_cursor: usize,
    pub chip: Option<usize>,
    pub chip_cursor: usize,
    pub boards: Vec<(String, Chip)>,
    pub scanning: bool,
    pub running: Option<&'static str>,
    pub last_exit: Option<Option<i32>>,
    pub output: Vec<String>,
    /// Top line of the output view; `None` = follow the tail.
    pub scroll: Option<usize>,
    /// Output viewport height, updated on every draw.
    pub output_height: usize,
    pub focus: Focus,
    pub theme: Theme,
    /// Active view.
    pub tab: Tab,
    /// Remembered focus per tab (indexed by [`Tab::index`]).
    pub tab_focus: [Focus; 3],
    pub ble_status: String,
    /// Session device connections, keyed by lowercase BLE address.
    pub connections: HashMap<String, Linked>,
    /// Cursor in the merged Connection Manager device table.
    pub mgr_cursor: usize,
    /// Disconnect alert for the status line.
    pub alert: Option<(String, Instant)>,
    /// devices.json: custom names + device types.
    pub devices: Devices,
    /// Load/save error for devices.json.
    pub devices_err: Option<String>,
    /// `false` when devices.json exists but failed to load: never overwrite it.
    pub devices_writable: bool,
    /// Inline rename of a device card (`Some` blocks global keys).
    pub dev_edit: Option<DevEdit>,
    /// actions.json load error, shown in the status line.
    pub actions_err: Option<String>,
    /// Timestamped controller events + device output (capped at [`LOG_CAP`]).
    pub log: VecDeque<String>,
    /// Last DeviceLine wall clock + sample, keyed by BLE addr and MAC (lowercase).
    pub last_msg: HashMap<String, LastMsg>,
    /// Lua script runtime (linked-device map + BLE command waiters).
    pub script: std::sync::Arc<scriptlib::ScriptRt>,
}

impl App {
    pub fn new(projects: Vec<PathBuf>, theme: Theme) -> Self {
        Self {
            project: (!projects.is_empty()).then_some(0),
            projects,
            project_cursor: 0,
            chip: None,
            chip_cursor: 0,
            boards: Vec::new(),
            scanning: false,
            running: None,
            last_exit: None,
            output: vec!["Select a connected chip and a project, then Build or Flash.".into()],
            scroll: None,
            output_height: 10,
            focus: Focus::Chips,
            theme,
            tab: Tab::Flash,
            tab_focus: [Focus::Chips, Focus::Devices, Focus::CourseEvents],
            ble_status: String::new(),
            connections: HashMap::new(),
            mgr_cursor: 0,
            alert: None,
            devices: Devices::default(),
            devices_err: None,
            devices_writable: false,
            dev_edit: None,
            actions_err: None,
            log: VecDeque::new(),
            script: scriptlib::ScriptRt::new(),
            last_msg: HashMap::new(),
        }
    }

    /// Last DeviceLine for any of `keys` (addr or MAC).
    pub fn last_msg_at(&self, keys: impl IntoIterator<Item = impl AsRef<str>>) -> Option<&LastMsg> {
        for k in keys {
            let k = k.as_ref();
            if k.is_empty() {
                continue;
            }
            if let Some(t) = self.last_msg.get(&k.to_ascii_lowercase()) {
                return Some(t);
            }
        }
        None
    }

    /// Append a timestamped line to the event log. One raw line per event; JSON is not rewritten.
    pub fn log(&mut self, line: impl AsRef<str>) { self.log_tagged("", line.as_ref()) }

    /// Like [`App::log`], with a `tag` (device name) printed before `msg`.
    pub fn log_tagged(&mut self, tag: &str, msg: &str) {
        let ts = chrono::Local::now().format("%H:%M:%S%.3f");
        let line = if tag.is_empty() {
            format!("{ts} {msg}")
        } else {
            format!("{ts} {tag} {msg}")
        };
        self.push_log_line(line);
    }

    fn push_log_line(&mut self, l: String) {
        self.log.push_back(l);
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    fn conn_key(addr: &str) -> String {
        addr.to_ascii_lowercase()
    }

    pub fn conn(&self, addr: &str) -> Option<&Linked> {
        self.connections.get(&Self::conn_key(addr))
    }

    pub fn conn_mut(&mut self, addr: &str) -> Option<&mut Linked> {
        self.connections.get_mut(&Self::conn_key(addr))
    }

    pub fn enabled_for(&self, d: &DeviceInfo) -> bool {
        let k = Self::device_key(d);
        self.devices
            .enabled
            .get(&k)
            .or_else(|| self.devices.enabled.get(&d.addr))
            .copied()
            .unwrap_or(false)
    }

    /// Toggle the ingest filter. Persists in devices.json.
    pub fn set_enabled(&mut self, addr: &str, on: bool) -> bool {
        let Some(info) = self.conn(addr).map(|l| l.info.clone()) else {
            return false;
        };
        if let Some(l) = self.conn_mut(addr) {
            l.enabled = on;
        }
        self.devices.enabled.insert(Self::device_key(&info), on);
        self.script.set_enabled(&info.addr, on);
        self.save_devices();
        true
    }

    pub fn toggle_enabled(&mut self, addr: &str) -> Option<bool> {
        let on = !self.conn(addr).is_some_and(|l| l.enabled);
        self.set_enabled(addr, on).then_some(on)
    }

    /// Display name for a BLE address (session hash, else the address).
    fn name_of(&self, addr: &str) -> String {
        self.conn(addr)
            .map(|l| self.display_name(&l.info))
            .unwrap_or_else(|| addr.to_string())
    }

    /// Enabled first, then paired, sorted by BLE address.
    pub fn mgr_rows(&self) -> Vec<(&DeviceInfo, Option<&Linked>)> {
        let mut rows: Vec<(&DeviceInfo, Option<&Linked>)> =
            self.connections.values().map(|l| (&l.info, Some(l))).collect();
        rows.sort_by(|a, b| {
            let rank = |l: Option<&Linked>| l.is_some_and(|x| x.enabled);
            match (rank(a.1), rank(b.1)) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.0.addr.to_ascii_lowercase().cmp(&b.0.addr.to_ascii_lowercase()),
            }
        });
        rows
    }

    fn clamp_mgr_cursor(&mut self) {
        self.mgr_cursor = self.mgr_cursor.min(self.mgr_rows().len().saturating_sub(1));
    }

    fn selected_linked(&self) -> Option<&Linked> {
        self.mgr_rows().get(self.mgr_cursor).and_then(|(_, l)| *l)
    }

    pub fn chip_port(&self, i: usize) -> Option<&str> {
        let chip = Chip::ALL[i];
        self.boards
            .iter()
            .find(|(_, c)| *c == chip)
            .map(|(p, _)| p.as_str())
    }
    pub fn chip_enabled(&self, i: usize) -> bool {
        self.chip_port(i).is_some()
    }
    pub fn busy(&self) -> bool {
        self.running.is_some() || self.scanning
    }
    /// Build/Flash need a selected connected chip and a project.
    pub fn can_run(&self) -> bool {
        !self.busy() && self.project.is_some() && self.chip.is_some_and(|c| self.chip_enabled(c))
    }
    pub fn can_refresh(&self) -> bool {
        !self.busy()
    }

    /// Active disconnect alert, if any.
    #[cfg(test)]
    pub fn alert(&self) -> Option<&str> {
        self.alert
            .as_ref()
            .filter(|(_, t)| t.elapsed() < ALERT_TIME)
            .map(|(m, _)| m.as_str())
    }

    /// Switch view.
    pub fn set_tab(&mut self, tab: Tab) -> Option<Cmd> {
        if tab == self.tab {
            return None;
        }
        let old = self.tab;
        self.tab_focus[old.index()] = self.focus;
        self.tab = tab;
        self.focus = self.tab_focus[tab.index()];
        if old == Tab::Manager {
            self.clamp_mgr_cursor();
        }
        None
    }

    fn handle_ble(&mut self, ev: BleEvent) {
        match ev {
            BleEvent::Ready(a) => {
                self.ble_status = format!("paired/connected devices on {a}");
                self.log(format!("BLE: watching paired/connected devices on {a}"));
            }
            BleEvent::Error(e) => {
                self.ble_status = format!("BLE error: {e}");
                self.output.push(format!("BLE error: {e}"));
                self.log(format!("BLE error: {e}"));
            }
            BleEvent::Info(m) => self.log(format!("BLE: {m}")),
            BleEvent::DeviceLine(a, line) => {
                let enabled = self.conn(&a).is_some_and(|l| l.enabled);
                if !enabled {
                    return;
                }
                let (ts, sample) = last_sample_from_line(&line)
                    .unwrap_or_else(|| (String::new(), serde_json::Value::Null));
                let rec = LastMsg { ts, sample };
                let addr_k = a.to_ascii_lowercase();
                let newer = self
                    .last_msg
                    .get(&addr_k)
                    .is_none_or(|old| cmp_ts_keys(&rec.ts, &old.ts) != Ordering::Less);
                if newer {
                    self.last_msg.insert(addr_k, rec.clone());
                }
                if let Some(l) = self.connections.get_mut(&Self::conn_key(&a)) {
                    apply_telemetry(l, &line);
                    if newer && !l.info.mac.is_empty() {
                        self.last_msg.insert(l.info.mac.to_ascii_lowercase(), rec);
                    }
                }
                self.script.on_line(&a, &line);
                let name = self.name_of(&a);
                self.log_tagged(&name, &line);
            }
            BleEvent::Discovered(d) => {
                let k = Self::conn_key(&d.addr);
                if !self.connections.contains_key(&k) {
                    self.log(format!("BLE: paired {} ({}, mac {})", d.name, d.addr, d.mac));
                }
                match self.connections.get_mut(&k) {
                    Some(l) => {
                        if d.name != "?" {
                            l.info.name = d.name.clone();
                        }
                        if d.rssi.is_some() {
                            l.info.rssi = d.rssi;
                        }
                        if !d.project.is_empty() {
                            l.info.project = d.project.clone();
                        }
                        if !d.mac.is_empty() && d.mac != "?" {
                            l.info.mac = d.mac.clone();
                        }
                        l.info.addr = d.addr;
                    }
                    None => {
                        let enabled = virt_ingest_default(&d) || self.enabled_for(&d);
                        let mut l = Linked::new(d, None, None);
                        l.enabled = enabled;
                        self.script.set_enabled(&l.info.addr, enabled);
                        self.connections.insert(k, l);
                    }
                }
            }
            BleEvent::Connected(info) => {
                let enabled = self.conn(&info.addr).map(|l| l.enabled).unwrap_or_else(|| {
                    virt_ingest_default(&info) || self.enabled_for(&info)
                });
                let names = vec![
                    self.display_name(&info),
                    info.name.clone(),
                    info.addr.clone(),
                    info.mac.clone(),
                ];
                self.script.upsert_link(&info.addr, names, true);
                self.script.set_enabled(&info.addr, enabled);
                self.output.push(format!("BLE: connected {} ({})", info.name, info.addr));
                self.log(format!("BLE: connected {} ({})", info.name, info.addr));
                if self.alert.as_ref().is_some_and(|(m, _)| m.contains(&info.addr)) {
                    self.alert = None;
                }
                match self.conn_mut(&info.addr) {
                    Some(l) => {
                        l.since = Some(Instant::now());
                        l.retry = None;
                        l.last_err = None;
                        if info.name != "?" { l.info.name = info.name; }
                    }
                    None => {
                        let mut l = Linked::new(info, Some(Instant::now()), None);
                        l.enabled = enabled;
                        self.connections.insert(Self::conn_key(&l.info.addr), l);
                    }
                }
            }
            BleEvent::Disconnected(a) => {
                self.script.upsert_link(&a, None::<String>, false);
                let name = self.name_of(&a);
                if let Some(l) = self.conn_mut(&a) {
                    l.since = None;
                    l.offset = None;
                    l.retry = None;
                    self.log(format!("BLE: {name} ({a}) disconnected"));
                }
            }
            BleEvent::Removed(a) => {
                self.script.drop_link(&a);
                let name = self.name_of(&a);
                self.connections.remove(&Self::conn_key(&a));
                self.clamp_mgr_cursor();
                self.log(format!("BLE: unpaired {name} ({a})"));
            }
            BleEvent::TimeOffset { addr, offset_ms, rtt_ms } => {
                if let Some(l) = self.conn_mut(&addr) {
                    l.offset = Some((offset_ms, rtt_ms));
                }
            }
            BleEvent::Rssi(a, r) => {
                if let Some(l) = self.conn_mut(&a) {
                    l.info.rssi = Some(r);
                }
            }
        }
    }

    /// Key under which a device's custom name is stored: factory MAC if known, else BLE address.
    pub fn device_key(d: &DeviceInfo) -> String {
        if d.mac.is_empty() || d.mac == "?" { d.addr.clone() } else { d.mac.clone() }
    }

    /// Custom name if set, else the advertised name.
    pub fn display_name(&self, d: &DeviceInfo) -> String {
        self.devices.names.get(&Self::device_key(d)).cloned().unwrap_or_else(|| d.name.clone())
    }

    /// Load devices.json (creating it with defaults if missing).
    pub fn load_devices(&mut self) {
        match devices::load() {
            Ok(d) => {
                self.devices = d;
                self.devices_err = None;
                self.devices_writable = true;
            }
            Err(e) => {
                self.devices_err = Some(format!("devices.json not loaded (not overwriting): {e}"));
                self.devices_writable = false;
            }
        }
    }

    fn save_devices(&mut self) {
        if !self.devices_writable {
            return;
        }
        match devices::save(&self.devices) {
            Ok(()) => self.devices_err = None,
            Err(e) => self.devices_err = Some(format!("devices.json save failed: {e}")),
        }
    }

    fn device_info_by_addr(&self, addr: &str) -> Option<DeviceInfo> {
        self.conn(addr).map(|l| l.info.clone())
    }

    /// Persist a BLE display name. Empty or advertised name clears the custom name.
    pub fn apply_ble_rename(&mut self, addr: &str, new: String) {
        let Some(info) = self.device_info_by_addr(addr) else {
            return;
        };
        let k = Self::device_key(&info);
        let old = self.display_name(&info);
        if new.is_empty() || new == info.name {
            self.devices.names.remove(&k);
        } else {
            self.devices.names.insert(k.clone(), new);
        }
        let shown = self.display_name(&info);
        if shown != old {
            central::log_event(&format!(
                "RENAMED key={k} addr={} \"{old}\" -> \"{shown}\"",
                info.addr
            ));
            self.output.push(format!("BLE: renamed {old} → {shown}"));
            self.script.upsert_link(
                &info.addr,
                [shown.clone()],
                self.conn(&info.addr).is_some_and(|l| l.since.is_some()),
            );
            self.script.set_enabled(&info.addr, self.conn(&info.addr).is_some_and(|l| l.enabled));
        }
        self.save_devices();
    }

    /// Inline rename of a device card. Returns `true` when consumed.
    fn handle_rename_key(&mut self, key: Key) -> bool {
        let ctrl = key.ctrl;
        if let Some(edit) = self.dev_edit.as_mut() {
            match key.code {
                Code::Enter => {
                    let new = edit.buf.trim().to_string();
                    let addr = edit.addr.clone();
                    self.dev_edit = None;
                    if let Some(addr) = addr {
                        self.apply_ble_rename(&addr, new);
                    }
                }
                Code::Esc => self.dev_edit = None,
                Code::Backspace => {
                    edit.buf.pop();
                }
                Code::Char(c) if !ctrl => edit.buf.push(c),
                _ => {}
            }
            return true;
        }
        if key.code == Code::Char('r') && !ctrl {
            if let Some(info) = self.selected_linked().map(|l| l.info.clone()) {
                self.dev_edit = Some(DevEdit {
                    virt_mac: None,
                    addr: Some(info.addr.clone()),
                    buf: self.display_name(&info),
                });
                return true;
            }
        }
        false
    }

    #[cfg(test)]
    pub fn status(&self) -> String {
        let mut s = if let Some(e) = &self.actions_err {
            e.clone()
        } else if let Some(e) = &self.devices_err {
            e.clone()
        } else if let Some(what) = self.running {
            format!("{what} running…")
        } else {
            match self.last_exit {
                Some(Some(0)) => "Done: exit code 0".into(),
                Some(Some(c)) => format!("Failed: exit code {c}"),
                Some(None) => "Failed: no exit code".into(),
                None => "Ready".into(),
            }
        };
        if let Some(a) = self.alert() {
            s = format!("{s}  {a}");
        }
        s
    }

    #[cfg(test)]
    pub fn usb_status(&self) -> String {
        if self.scanning {
            "Scanning…".into()
        } else if self.boards.is_empty() {
            "—".into()
        } else {
            self.boards
                .iter()
                .map(|(_, c)| c.name())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    pub fn start_scan(&mut self) {
        self.scanning = true;
    }

    pub fn start_job(&mut self, what: &'static str) {
        self.running = Some(what);
        self.output.clear();
        self.scroll = None;
    }

    pub fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Ble(ev) => self.handle_ble(ev),
            Msg::Line(l) => {
                self.log(format!("{}: {l}", self.running.unwrap_or("job")));
                self.output.push(l);
            }
            Msg::JobDone(code) => {
                self.log(format!("{} finished: {}", self.running.unwrap_or("job"), code.map_or("no exit code".into(), |c| format!("exit code {c}"))));
                self.running = None;
                self.last_exit = Some(code);
            }
            Msg::Boards(b) => {
                self.scanning = false;
                self.output.push(if b.is_empty() {
                    "No ESP boards detected (check the cable and serial-port permissions, then Refresh).".into()
                } else {
                    format!("Detected: {}", b.iter().map(|(p, c)| format!("{} on {p}", c.name())).collect::<Vec<_>>().join(", "))
                });
                if let Some(l) = self.output.last().cloned() { self.log(l); }
                self.boards = b;
                if self.chip.is_some_and(|c| !self.chip_enabled(c)) {
                    self.chip = None;
                }
                if let Some(first) = (0..Chip::ALL.len()).find(|&i| self.chip_enabled(i)) {
                    if !self.chip_enabled(self.chip_cursor) {
                        self.chip_cursor = first;
                    }
                    if self.chip.is_none() {
                        self.chip = Some(first);
                    }
                }
            }
        }
    }

    fn move_chip(&mut self, d: isize) {
        let n = Chip::ALL.len() as isize;
        let mut i = self.chip_cursor as isize;
        for _ in 0..n {
            i += d;
            if !(0..n).contains(&i) {
                return;
            }
            if self.chip_enabled(i as usize) {
                self.chip_cursor = i as usize;
                return;
            }
        }
    }

    fn output_top(&self) -> usize {
        self.scroll
            .unwrap_or_else(|| self.output.len().saturating_sub(self.output_height))
    }
    pub fn scroll_output(&mut self, d: isize) {
        let max = self.output.len().saturating_sub(self.output_height);
        let top = (self.output_top() as isize + d).clamp(0, max as isize) as usize;
        self.scroll = (top < max).then_some(top);
    }

    pub fn handle_key(&mut self, key: Key) -> Option<Cmd> {
        let page = self.output_height.max(1) as isize;
        let ctrl = key.ctrl;
        if (self.focus == Focus::Devices || self.dev_edit.is_some()) && self.handle_rename_key(key) {
            return None;
        }
        match key.code {
            Code::Char('q') | Code::Esc => return Some(Cmd::Quit),
            Code::Char('c') if key.ctrl => {
                return Some(Cmd::Quit);
            }
            Code::Char('b') if self.tab == Tab::Flash && !ctrl => {
                return self.can_run().then_some(Cmd::Build);
            }
            Code::Char('f') if self.tab == Tab::Flash && !ctrl => {
                return self.can_run().then_some(Cmd::Flash);
            }
            Code::Char('r') if self.tab == Tab::Flash && !ctrl => {
                return self.can_refresh().then_some(Cmd::Refresh);
            }
            Code::Tab => {
                self.clamp_mgr_cursor();
                self.focus = self.focus.step(1, self.tab);
            }
            Code::BackTab => self.focus = self.focus.step(-1, self.tab),
            Code::PageUp => self.scroll_output(-page),
            Code::PageDown => self.scroll_output(page),
            Code::Up | Code::Down => {
                let d = if key.code == Code::Up { -1 } else { 1 };
                match self.focus {
                    Focus::Chips => self.move_chip(d),
                    Focus::Projects if !self.projects.is_empty() => {
                        self.project_cursor = (self.project_cursor as isize + d)
                            .clamp(0, self.projects.len() as isize - 1)
                            as usize;
                    }
                    Focus::Devices => {
                        let n = self.mgr_rows().len();
                        if n > 0 {
                            self.mgr_cursor = (self.mgr_cursor as isize + d).clamp(0, n as isize - 1) as usize;
                        }
                    }
                    _ => {}
                }
            }
            Code::Enter | Code::Char(' ') => match self.focus {
                Focus::Chips if !self.busy() && self.chip_enabled(self.chip_cursor) => {
                    self.chip = Some(self.chip_cursor)
                }
                Focus::Projects if !self.busy() && self.project_cursor < self.projects.len() => {
                    self.project = Some(self.project_cursor)
                }
                Focus::Devices => {
                    if let Some(addr) = self.mgr_rows().get(self.mgr_cursor).map(|(info, _)| info.addr.clone()) {
                        let _ = self.toggle_enabled(&addr);
                    }
                }
                _ => {}
            },
            Code::Delete | Code::Backspace | Code::Char('d') if self.focus == Focus::Devices => {
                if let Some(addr) = self
                    .mgr_rows()
                    .get(self.mgr_cursor)
                    .map(|(info, _)| info.addr.clone())
                {
                    let _ = self.set_enabled(&addr, false);
                }
            }
            _ => {}
        }
        None
    }
}
