//! Load and run Lua scripts that call controller and device methods.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, StdLib, UserData, UserDataFields, UserDataMethods,
    Value, Variadic, VmState,
};

use crate::methods;

const LINK_WAIT: Duration = Duration::from_secs(30);
const RESULT_WAIT: Duration = Duration::from_secs(30);

/// Device methods sent on the command characteristic.
pub const DEVICE_METHODS: &[&str] = &[
    "ping",
    "get_accelerometers",
    "sleep",
    "reboot",
    "signal",
    "read_buffer",
    "set_threshold",
    "set_name",
];

/// Send a JSON command line to a BLE address.
pub type BleSend = Arc<dyn Fn(String, String) -> Result<(), String> + Send + Sync>;

/// BLE command line for a device method: `{"action":"<name>","params":[…]}`.
pub fn command_json(action: &str, params: &[String]) -> String {
    serde_json::json!({ "action": action, "params": params }).to_string()
}

#[derive(Clone)]
struct Link {
    addr: String,
    names: Vec<String>,
    connected: bool,
    enabled: bool,
}

struct Waiter {
    addr: String,
    method: String,
    tx: Sender<Result<String, String>>,
}

/// Shared with the UI thread: linked devices, BLE command sender, result waiters.
pub struct ScriptRt {
    pub ble: Mutex<Option<BleSend>>,
    links: Mutex<Vec<Link>>,
    waits: Mutex<Vec<Waiter>>,
    pub host: methods::Shared,
}

impl ScriptRt {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ble: Mutex::new(None),
            links: Mutex::new(Vec::new()),
            waits: Mutex::new(Vec::new()),
            host: methods::new_shared(),
        })
    }

    pub fn set_ble(&self, send: BleSend) {
        *self.ble.lock().unwrap_or_else(|e| e.into_inner()) = Some(send);
    }

    pub fn upsert_link(&self, addr: &str, names: impl IntoIterator<Item = String>, connected: bool) {
        let names: Vec<String> = names.into_iter().filter(|n| !n.is_empty()).collect();
        let mut v = self.links.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = v.iter_mut().find(|l| l.addr == addr) {
            for n in names {
                if !l.names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                    l.names.push(n);
                }
            }
            l.connected = connected;
        } else {
            v.push(Link { addr: addr.to_string(), names, connected, enabled: false });
        }
    }

    pub fn set_enabled(&self, addr: &str, enabled: bool) {
        let mut v = self.links.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = v.iter_mut().find(|l| l.addr == addr) {
            l.enabled = enabled;
        } else {
            v.push(Link {
                addr: addr.to_string(),
                names: Vec::new(),
                connected: false,
                enabled,
            });
        }
    }

    pub fn drop_link(&self, addr: &str) {
        self.links.lock().unwrap_or_else(|e| e.into_inner()).retain(|l| l.addr != addr);
    }

    /// Feed a DeviceLine; complete waiters whose method result is in a buffer snapshot.
    pub fn on_line(&self, addr: &str, line: &str) {
        for (method, result, error) in method_results(line) {
            let mut waits = self.waits.lock().unwrap_or_else(|e| e.into_inner());
            let mut i = 0;
            while i < waits.len() {
                if waits[i].addr == addr && waits[i].method == method {
                    let w = waits.remove(i);
                    let _ = w.tx.send(if error { Err(result.clone()) } else { Ok(result.clone()) });
                } else {
                    i += 1;
                }
            }
        }
    }
}

fn method_results(line: &str) -> Vec<(String, String, bool)> {
    if let Some(i) = line.find("buffer seq ") {
        let Some(brace) = line[i..].find('{') else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line[i + brace..].trim_end()) else {
            return Vec::new();
        };
        let Some(o) = v.as_object() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for sample in o.values() {
            let Some(m) = sample.get("method").and_then(|x| x.as_str()) else { continue };
            let result = match sample.get("result") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(v) => serde_json::to_string(v).unwrap_or_default(),
                None => String::new(),
            };
            let error = sample.get("error").and_then(|x| x.as_bool()).unwrap_or(false)
                || result.starts_with("error:");
            out.push((m.to_string(), result, error));
        }
        return out;
    }
    log_result(line).into_iter().collect()
}

fn log_result(line: &str) -> Option<(String, String, bool)> {
    let (action, rest) = line.split_once(": ")?;
    if action.is_empty()
        || !action
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let error = rest.starts_with("error:");
    Some((action.to_string(), rest.to_string(), error))
}

/// `.lua` file names in `d`, sorted.
pub fn list_in(d: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(rd) = std::fs::read_dir(d) else {
        return names;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lua"))
            && let Some(n) = p.file_name()
        {
            names.push(n.to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

/// Stage Lua files: `~/dev/esp32/course_defs`, then `fallback` if that is a directory.
pub fn course_dir(fallback: impl AsRef<Path>) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let preferred = home.join("dev/esp32/course_defs");
    if preferred.is_dir() {
        return preferred;
    }
    let crate_dir = fallback.as_ref();
    if crate_dir.is_dir() {
        return crate_dir.to_path_buf();
    }
    preferred
}

fn new_lua(script: Option<&Path>, stop: Option<Arc<AtomicBool>>) -> Result<Lua, String> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::new()).map_err(|e| e.to_string())?;
    set_package_path(&lua, script).map_err(|e| e.to_string())?;
    preload_libs(&lua, stop).map_err(|e| e.to_string())?;
    Ok(lua)
}

fn set_package_path(lua: &Lua, script: Option<&Path>) -> mlua::Result<()> {
    let dir = script
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let dir = dir.to_string_lossy().replace('\\', "/");
    let extra = format!("{dir}/?.lua;{dir}/?/init.lua;{dir}/lib/?.lua;{dir}/lib/?/init.lua");
    let pkg: mlua::Table = lua.globals().get("package")?;
    let path: String = pkg.get("path")?;
    pkg.set("path", format!("{extra};{path}"))
}

fn preload_libs(lua: &Lua, stop: Option<Arc<AtomicBool>>) -> mlua::Result<()> {
    let g = lua.globals();
    let preload: mlua::Table = g.get::<mlua::Table>("package")?.get("preload")?;
    preload.set(
        "dkjson",
        lua.create_function(|lua, _name: String| {
            const DKJSON: &str = include_str!("dkjson.lua");
            lua.load(DKJSON).set_name("dkjson.lua").eval::<Value>()
        })?,
    )?;
    let sleep = lua.create_function(move |_, n: f64| {
        socket_sleep(n, stop.as_deref()).map_err(mlua::Error::external)
    })?;
    let gettime = lua.create_function(|_, ()| Ok(socket_gettime()))?;
    preload.set(
        "socket",
        lua.create_function(move |lua, _name: String| {
            const SOCKET: &str = include_str!("socket.lua");
            lua.load(SOCKET)
                .set_name("socket.lua")
                .call::<Value>((sleep.clone(), gettime.clone()))
        })?,
    )?;
    Ok(())
}

fn socket_sleep(seconds: f64, stop: Option<&AtomicBool>) -> Result<(), String> {
    if seconds <= 0.0 {
        return Ok(());
    }
    let ms = (seconds * 1000.0).ceil() as u64;
    let mut left = ms;
    while left > 0 {
        if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            return Err("stopped".into());
        }
        let step = left.min(50);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    Ok(())
}

fn socket_gettime() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One Stages card collected during `init`.
#[derive(Clone, Debug, Default)]
pub struct InitCard {
    pub name: String,
    /// TARGET_DEFS stem; empty for grid placeholders.
    pub def: String,
    pub row: Option<u32>,
    pub col: Option<u32>,
    pub distance: Option<String>,
}

/// Roster returned by [`run_init`]: optional grid size plus cards.
#[derive(Clone, Debug, Default)]
pub struct InitRoster {
    pub rows: Option<u32>,
    pub columns: Option<u32>,
    pub cards: Vec<InitCard>,
}

struct InitState {
    rows: Option<u32>,
    columns: Option<u32>,
    cards: Vec<InitCard>,
}

fn push_index(lua: &Lua, name: &str) -> mlua::Result<()> {
    let global_targets: mlua::Table = lua.globals().get("global_targets")?;
    let index: mlua::Table = global_targets.get("index")?;
    index.raw_push(name.to_string())
}

fn remove_index(lua: &Lua, name: &str) -> mlua::Result<()> {
    let global_targets: mlua::Table = lua.globals().get("global_targets")?;
    let index: mlua::Table = global_targets.get("index")?;
    let next = lua.create_table()?;
    for entry in index.sequence_values::<String>() {
        let n = entry?;
        if n != name {
            next.raw_push(n)?;
        }
    }
    global_targets.set("index", next)
}

/// Load a course script and call `init()`. Collects grid metadata and cards from
/// `define_grid`, `define_distance`, and `define_target`.
pub fn run_init(path: &Path) -> Result<InitRoster, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lua = new_lua(Some(path), None)?;
    let state = Arc::new(Mutex::new(InitState {
        rows: None,
        columns: None,
        cards: Vec::new(),
    }));
    {
        let global_targets = lua.create_table().map_err(|e| e.to_string())?;
        let index = lua.create_table().map_err(|e| e.to_string())?;
        global_targets
            .set("index", index)
            .map_err(|e| e.to_string())?;
        lua.globals()
            .set("global_targets", global_targets)
            .map_err(|e| e.to_string())?;
    }
    {
        let state = state.clone();
        let f = lua
            .create_function(move |lua, (rows, columns): (u32, u32)| {
                if rows < 1 || columns < 1 {
                    return Err(mlua::Error::external(
                        "define_grid: rows and columns must be >= 1",
                    ));
                }
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if st.rows.is_some() {
                    return Err(mlua::Error::external(
                        "define_grid: grid already defined",
                    ));
                }
                st.rows = Some(rows);
                st.columns = Some(columns);
                for row in 1..=rows {
                    for col in 1..=columns {
                        let name = format!("{col},{row}");
                        push_index(lua, &name)?;
                        st.cards.push(InitCard {
                            name,
                            def: String::new(),
                            row: Some(row),
                            col: Some(col),
                            distance: None,
                        });
                    }
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        lua.globals()
            .set("define_grid", f)
            .map_err(|e| e.to_string())?;
    }
    {
        let state = state.clone();
        let f = lua
            .create_function(move |lua, (distance, row, col): (String, u32, u32)| {
                let name = {
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    let card = st
                        .cards
                        .iter_mut()
                        .find(|c| c.row == Some(row) && c.col == Some(col))
                        .ok_or_else(|| {
                            mlua::Error::external(format!(
                                "define_distance: no card at row={row} col={col}"
                            ))
                        })?;
                    card.distance = Some(distance);
                    card.name.clone()
                };
                remove_index(lua, &name)?;
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        lua.globals()
            .set("define_distance", f)
            .map_err(|e| e.to_string())?;
    }
    {
        let state = state.clone();
        let f = lua
            .create_function(
                move |lua, (name, def, row, col): (String, String, Option<u32>, Option<u32>)| {
                    if name.trim().is_empty() {
                        return Err(mlua::Error::external("define_target: name is empty"));
                    }
                    if def.trim().is_empty() {
                        return Err(mlua::Error::external("define_target: def is empty"));
                    }
                    match (row, col) {
                        (None, None) | (Some(_), Some(_)) => {}
                        _ => {
                            return Err(mlua::Error::external(
                                "define_target: row and col must both be set or both omitted",
                            ));
                        }
                    }
                    push_index(lua, &name)?;
                    {
                        let global_targets: mlua::Table = lua.globals().get("global_targets")?;
                        let stub = lua.create_table()?;
                        stub.set("target_name", name.as_str())?;
                        stub.set("target_def", def.as_str())?;
                        global_targets.set(name.as_str(), stub)?;
                    }
                    state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .cards
                        .push(InitCard {
                            name,
                            def,
                            row,
                            col,
                            distance: None,
                        });
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
        lua.globals()
            .set("define_target", f)
            .map_err(|e| e.to_string())?;
    }
    for name in ["process_targets"] {
        let msg = format!("{name}: cannot be called from init");
        let f = lua
            .create_function(move |_, _: Variadic<Value>| {
                Err::<(), _>(mlua::Error::external(msg.clone()))
            })
            .map_err(|e| e.to_string())?;
        lua.globals().set(name, f).map_err(|e| e.to_string())?;
    }
    lua.load(&src)
        .set_name(path.to_string_lossy().as_ref())
        .exec()
        .map_err(|e| e.to_string())?;
    let init: Function = lua
        .globals()
        .get("init")
        .map_err(|_| "no init function".to_string())?;
    init.call::<()>(()).map_err(|e| e.to_string())?;
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    Ok(InitRoster {
        rows: st.rows,
        columns: st.columns,
        cards: st.cards.clone(),
    })
}

pub fn spawn_stop(
    path: PathBuf,
    rt: Arc<ScriptRt>,
    log: Sender<String>,
    stop: Arc<AtomicBool>,
    done: Option<Arc<AtomicBool>>,
    cards: Vec<methods::TargetCard>,
) {
    std::thread::Builder::new()
        .name("lua-script".into())
        .spawn(move || {
            let name = path.display().to_string();
            let _ = log.send(format!("script: running {name}"));
            match run(&path, &rt, stop, log.clone(), cards) {
                Ok(()) => {
                    let _ = log.send(format!("script: done {name}"));
                }
                Err(e) => {
                    let _ = log.send(format!("script: {name}: {e}"));
                }
            }
            if let Some(d) = done {
                d.store(false, Ordering::Relaxed);
            }
        })
        .expect("lua-script thread");
}

fn run(
    path: &Path,
    rt: &Arc<ScriptRt>,
    stop: Arc<AtomicBool>,
    log: Sender<String>,
    cards: Vec<methods::TargetCard>,
) -> Result<(), String> {
    methods::set_targets(&rt.host, cards.clone());
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lua = new_lua(Some(path), Some(stop.clone()))?;
    bind(&lua, rt.clone(), stop.clone(), log, &cards).map_err(|e| e.to_string())?;
    let hook_stop = stop.clone();
    lua.set_hook(HookTriggers::EVERY_LINE, move |_lua, _debug| {
        if hook_stop.load(Ordering::Relaxed) {
            Err(mlua::Error::external("stopped"))
        } else {
            Ok(VmState::Continue)
        }
    })
    .map_err(|e| e.to_string())?;
    lua.load(&src)
        .set_name(path.to_string_lossy().as_ref())
        .exec()
        .map_err(|e| e.to_string())?;
    let main: Function = lua
        .globals()
        .get("main")
        .map_err(|_| "no main function".to_string())?;
    main.call::<()>(()).map_err(|e| e.to_string())?;
    Ok(())
}

fn bind(
    lua: &Lua,
    rt: Arc<ScriptRt>,
    stop: Arc<AtomicBool>,
    log: Sender<String>,
    cards: &[methods::TargetCard],
) -> mlua::Result<()> {
    let g = lua.globals();
    let host = {
        let rt = rt.clone();
        lua.create_function(move |_, (name, duration): (String, String)| {
            methods::start_timer(&rt.host, &name, &duration).map_err(mlua::Error::external)
        })?
    };
    g.set("start_timer", host)?;
    let host = {
        let rt = rt.clone();
        lua.create_function(move |_, name: String| {
            methods::timer_expired(&rt.host, &name).map_err(mlua::Error::external)
        })?
    };
    g.set("timer_expired", host)?;
    g.set(
        "random_delay",
        {
            let stop = stop.clone();
            lua.create_function(move |_, (min, max): (String, String)| {
                methods::random_delay_stop(&min, &max, Some(&stop)).map_err(mlua::Error::external)
            })?
        },
    )?;
    let host = {
        let rt = rt.clone();
        let stop = stop.clone();
        let log = log.clone();
        let names: Vec<String> = cards.iter().map(|c| c.name.clone()).collect();
        lua.create_function(move |_, ()| {
            for name in &names {
                let device = methods::sensor_device(&rt.host, "process_targets", name)
                    .map_err(mlua::Error::external)?;
                let json = call_device(&rt, &device, "read_buffer", Vec::new(), &stop)
                    .map_err(mlua::Error::external)?;
                let neu = methods::merge_hub_events(&rt.host, name, &json)
                    .map_err(mlua::Error::external)?;
                if neu == "{}" {
                    continue;
                }
                let hits = methods::hits_from_buffer(&rt.host, name, &neu)
                    .map_err(mlua::Error::external)?;
                for (ms, zone, sensors) in hits {
                    let line = serde_json::json!({
                        "type": "target_hit",
                        "ms": ms,
                        "target": name,
                        "zone": zone,
                        "sensors": sensors,
                    })
                    .to_string();
                    log.send(line).map_err(|e| mlua::Error::external(e.to_string()))?;
                }
            }
            Ok(())
        })?
    };
    g.set("process_targets", host)?;
    g.set(
        "text",
        lua.create_function(move |_, txt: Value| {
            let msg = serde_json::json!({ "text": lua_to_string(&txt) }).to_string();
            log.send(msg).map_err(|e| mlua::Error::external(e.to_string()))
        })?,
    )?;
    g.set(
        "global_targets",
        global_targets_table(lua, &rt, cards)?,
    )?;
    Ok(())
}

fn global_targets_table(
    lua: &Lua,
    rt: &Arc<ScriptRt>,
    cards: &[methods::TargetCard],
) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    let name_index = lua.create_table()?;
    for (i, c) in cards.iter().enumerate() {
        let obj = LuaTarget {
            name: c.name.clone(),
            rt: rt.clone(),
        };
        t.set(i + 1, obj.clone())?;
        name_index.set(i + 1, c.name.as_str())?;
        if !c.name.is_empty() {
            t.set(c.name.as_str(), obj)?;
        }
    }
    t.set("index", name_index)?;
    Ok(t)
}

#[derive(Clone)]
struct LuaTarget {
    name: String,
    rt: Arc<ScriptRt>,
}

impl UserData for LuaTarget {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("target_name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("target_def", |_, this| {
            Ok(methods::get_target(&this.rt.host, &this.name)
                .map(|c| c.def)
                .unwrap_or_default())
        });
        fields.add_field_method_get("zones", |lua, this| {
            let names = methods::get_target(&this.rt.host, &this.name)
                .map(|c| c.zones)
                .unwrap_or_default();
            let t = lua.create_table()?;
            for name in names {
                t.set(
                    name.as_str(),
                    LuaZone {
                        target: this.name.clone(),
                        name: name.clone(),
                        rt: this.rt.clone(),
                    },
                )?;
            }
            Ok(t)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("active", |_, this, on: Option<bool>| match on {
            Some(on) => crate::methods::set_target_active(&this.rt.host, &this.name, on)
                .map_err(mlua::Error::external),
            None => Ok(crate::methods::target_is_active(&this.rt.host, &this.name)),
        });
    }
}

#[derive(Clone)]
struct LuaZone {
    target: String,
    name: String,
    rt: Arc<ScriptRt>,
}

impl UserData for LuaZone {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("active", |_, this, on: Option<bool>| match on {
            Some(on) => crate::methods::set_zone_active(&this.rt.host, &this.target, &this.name, on)
                .map_err(mlua::Error::external),
            None => Ok(crate::methods::zone_is_active(&this.rt.host, &this.target, &this.name)),
        });
    }
}

fn call_device(
    rt: &ScriptRt,
    name: &str,
    method: &str,
    params: Vec<String>,
    stop: &AtomicBool,
) -> Result<String, String> {
    let addr = wait_linked(rt, name, stop)?;
    let (tx, rx) = std::sync::mpsc::channel();
    rt.waits.lock().unwrap_or_else(|e| e.into_inner()).push(Waiter {
        addr: addr.clone(),
        method: method.to_string(),
        tx,
    });
    let ble = rt.ble.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(ble) = ble else {
        return Err("device link is not running".into());
    };
    let line = command_json(method, &params);
    ble(addr.clone(), line)?;
    let start = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(r) => return r,
            Err(_) if start.elapsed() >= RESULT_WAIT => {
                return Err(format!("{method}: timed out waiting for {addr} buffer"));
            }
            Err(_) => {}
        }
    }
}

fn lua_to_string(v: &Value) -> String {
    match v {
        Value::Nil => String::new(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        other => format!("{other:?}"),
    }
}

fn wait_linked(rt: &ScriptRt, name: &str, stop: &AtomicBool) -> Result<String, String> {
    let start = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        {
            let links = rt.links.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(l) = links.iter().find(|l| {
                l.names.iter().any(|n| n.eq_ignore_ascii_case(name)) || l.addr.eq_ignore_ascii_case(name)
            }) {
                if l.connected && l.enabled {
                    return Ok(l.addr.clone());
                }
                if l.connected && !l.enabled {
                    return Err(format!("device {name:?} is not enabled"));
                }
            }
        }
        if start.elapsed() >= LINK_WAIT {
            return Err(format!("device {name:?} is not linked"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::TargetCard;

    fn bind_with_log(cards: Vec<TargetCard>) -> (Lua, Arc<ScriptRt>, std::sync::mpsc::Receiver<String>) {
        let rt = ScriptRt::new();
        methods::set_targets(&rt.host, cards.clone());
        let lua = new_lua(None, None).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        bind(
            &lua,
            rt.clone(),
            Arc::new(AtomicBool::new(false)),
            tx,
            &cards,
        )
        .unwrap();
        (lua, rt, rx)
    }

    fn bind_cards(cards: Vec<TargetCard>) -> (Lua, Arc<ScriptRt>) {
        let (lua, rt, _rx) = bind_with_log(cards);
        (lua, rt)
    }

    #[test]
    fn targets_named_and_index() {
        let (lua, _) = bind_cards(vec![
            TargetCard {
                name: "Target1".into(),
                def: "ipsc".into(),
                zones: vec!["A Zone Head".into(), "C Zone".into()],
                hub: Some("aa:bb".into()),
                hub_name: Some("virtual-hub-sensor-01".into()),
            },
            TargetCard {
                name: "Target-2".into(),
                def: String::new(),
                zones: vec![],
                hub: None,
                hub_name: None,
            },
        ]);
        let name: String = lua
            .load("return global_targets.Target1.target_name")
            .eval()
            .unwrap();
        assert_eq!(name, "Target1");
        let name: String = lua.load("return global_targets[1].target_name").eval().unwrap();
        assert_eq!(name, "Target1");
        let def: String = lua
            .load("return global_targets.Target1.target_def")
            .eval()
            .unwrap();
        assert_eq!(def, "ipsc");
        let n: usize = lua
            .load("local n=0; for _ in pairs(global_targets.Target1.zones) do n=n+1 end; return n")
            .eval()
            .unwrap();
        assert_eq!(n, 2);
        let on: bool = lua.load("return global_targets.Target1:active(true)").eval().unwrap();
        assert!(on);
        let active: bool = lua.load("return global_targets.Target1:active()").eval().unwrap();
        assert!(active);
        let off: bool = lua
            .load(r#"return global_targets.Target1.zones["A Zone Head"]:active(false)"#)
            .eval()
            .unwrap();
        assert!(!off);
        let still: bool = lua.load("return global_targets.Target1:active()").eval().unwrap();
        assert!(still);
        lua.load("global_targets[1]:active(false)").exec().unwrap();
        let active: bool = lua.load("return global_targets.Target1:active()").eval().unwrap();
        assert!(!active);
    }

    #[test]
    fn method_results_stringifies_json() {
        let v = method_results(
            r#"buffer seq 1: {"1":{"method":"read_sensor","result":[0,1,2,3],"error":false}}"#,
        );
        assert_eq!(v, vec![("read_sensor".into(), "[0,1,2,3]".into(), false)]);

        let v = method_results(
            r#"buffer seq 1: {"1":{"method":"get_accelerometers","result":{"x":1.0,"y":2.0,"z":3.0},"error":false}}"#,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "get_accelerometers");
        assert_eq!(v[0].1, r#"{"x":1.0,"y":2.0,"z":3.0}"#);
        assert!(!v[0].2);

        let v = method_results(
            r#"buffer seq 1: {"1":{"method":"ping","result":"pong","error":false}}"#,
        );
        assert_eq!(v, vec![("ping".into(), "pong".into(), false)]);

        let v = method_results(r#"read_buffer: {"sensor-000001":[[100,2000,2100,2200,2300]]}"#);
        assert_eq!(
            v,
            vec![(
                "read_buffer".into(),
                r#"{"sensor-000001":[[100,2000,2100,2200,2300]]}"#.into(),
                false
            )]
        );
        let v = method_results(r#"read_buffer: error: not a hub"#);
        assert_eq!(
            v,
            vec![("read_buffer".into(), "error: not a hub".into(), true)]
        );
    }

    #[test]
    fn on_line_completes_read_buffer_log() {
        let rt = ScriptRt::new();
        let (tx, rx) = std::sync::mpsc::channel();
        rt.waits.lock().unwrap_or_else(|e| e.into_inner()).push(Waiter {
            addr: "aa:bb".into(),
            method: "read_buffer".into(),
            tx,
        });
        rt.on_line("aa:bb", r#"read_buffer: {"n":[[100,2000,2100,2200,2300]]}"#);
        assert_eq!(
            rx.try_recv().unwrap(),
            Ok(r#"{"n":[[100,2000,2100,2200,2300]]}"#.into())
        );
    }

    #[test]
    fn process_targets_requires_hub() {
        let (lua, _) = bind_cards(vec![TargetCard {
            name: "Target1".into(),
            def: "ipsc".into(),
            zones: vec!["outer".into()],
            hub: None,
            hub_name: None,
        }]);
        let err = lua.load("process_targets()").exec().unwrap_err().to_string();
        assert!(
            err.contains("no SENSOR HUB"),
            "{err}"
        );
    }

    #[test]
    fn process_targets_blocked_in_init() {
        let dir = std::env::temp_dir().join(format!("scriptlib-pt-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("stage.lua");
        std::fs::write(
            &script,
            "function init() process_targets() end\nfunction main() end\n",
        )
        .unwrap();
        let err = run_init(&script).unwrap_err();
        assert!(
            err.contains("process_targets: cannot be called from init"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_socket_sleep() {
        let lua = new_lua(None, None).unwrap();
        lua.load(r#"local socket = require("socket"); socket.sleep(0)"#)
            .exec()
            .unwrap();
        let t: f64 = lua.load(r#"return require("socket").gettime()"#).eval().unwrap();
        assert!(t > 0.0);
    }

    #[test]
    fn require_sibling_lua() {
        let dir = std::env::temp_dir().join(format!("scriptlib-require-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("helpers.lua"), "return { ok = true }").unwrap();
        std::fs::write(dir.join("lib").join("extra.lua"), "return { from = 'lib' }").unwrap();
        let script = dir.join("stage.lua");
        std::fs::write(&script, "function init() end\nfunction main() end").unwrap();
        let lua = new_lua(Some(&script), None).unwrap();
        let ok: bool = lua.load(r#"return require("helpers").ok"#).eval().unwrap();
        assert!(ok);
        let from: String = lua.load(r#"return require("extra").from"#).eval().unwrap();
        assert_eq!(from, "lib");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_distance_not_in_global_targets_index() {
        let dir = std::env::temp_dir().join(format!(
            "scriptlib-init-distance-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("stage.lua");
        std::fs::write(
            &script,
            r#"
function init()
  define_grid(2, 2)
  define_distance("7y", 1, 2)
  define_target("Target1", "ipsc", 1, 1)
  for _, n in ipairs(global_targets.index) do
    if n == "2,1" then error("distance still in index") end
  end
end
"#,
        )
        .unwrap();
        let roster = run_init(&script).expect("run_init");
        assert!(
            roster
                .cards
                .iter()
                .any(|c| c.name == "2,1" && c.distance.as_deref() == Some("7y")),
            "distance card kept in roster for layout"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
