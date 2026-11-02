//! target-editor: flash, device manager, stages, target zones, virtual sensors.
mod actions;
mod chrome;
mod config;
mod ctrl;
mod host;
mod key;
mod model;
mod report;
mod targets;
mod theme;
mod ui_course;
mod ui_flash;
mod ui_manager;
mod ui_roster;
mod ui_settings;
mod virt_ui;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use gpui::{px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions};
use utils::host::{Chip, Framework};

use host::{Host, Mode};

const USAGE: &str = "usage: target-editor [--virtual] [--manager] [--flash] [--adapter hciN] [IMAGE|JSON]
       target-editor --build <project> <chip>
       target-editor --manager --headless [--adapter hciN] [--connect <name|mac>]...";

fn esp32_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("dev/esp32")
}

fn run_streaming(
    label: &str,
    mut cmd: std::process::Command,
    tx: &std::sync::mpsc::Sender<ctrl::Msg>,
) -> Option<i32> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    let _ = tx.send(ctrl::Msg::Line(format!("$ {label}")));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(ctrl::Msg::Line(format!("failed to start: {e}")));
            return None;
        }
    };
    fn pump(
        r: impl Read + Send + 'static,
        tx: std::sync::mpsc::Sender<ctrl::Msg>,
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
                        let _ = tx.send(ctrl::Msg::Line(part.to_string()));
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
            let _ = tx.send(ctrl::Msg::Line(match s.code() {
                Some(c) => format!("[exit code {c}]"),
                None => format!("[{s}]"),
            }));
            s.code()
        }
        Err(e) => {
            let _ = tx.send(ctrl::Msg::Line(format!("[wait failed: {e}]")));
            None
        }
    }
}

fn headless_build(project: &str, chip: &str) -> ! {
    let Some(chip) = Chip::from_name(chip) else {
        eprintln!(
            "unknown chip {chip}; expected one of: esp32 esp32s2 esp32s3 esp32c3 esp32c6 esp32h2"
        );
        std::process::exit(2);
    };
    let dir = esp32_root().join(project);
    let (tx, rx) = std::sync::mpsc::channel();
    let printer = std::thread::spawn(move || {
        for msg in rx {
            if let ctrl::Msg::Line(l) = msg {
                println!("{l}");
            }
        }
    });
    let code = run_streaming(
        &utils::host::build_shell(&dir, chip, Framework::Hal),
        utils::host::build_command(&dir, chip, Framework::Hal),
        &tx,
    );
    drop(tx);
    let _ = printer.join();
    if code == Some(0) {
        println!("ELF: {}", utils::host::elf_path(&dir, chip, Framework::Hal).display());
    }
    std::process::exit(code.unwrap_or(1));
}

fn headless_manager(targets: Vec<String>, adapter: Option<String>) -> ! {
    use devicehostlib::central::{self, BleEvent};

    let targets: Vec<String> = targets.into_iter().map(|t| t.to_lowercase()).collect();
    let (_cmd, events) = central::spawn(adapter, None);
    println!(
        "manager mode: OS-paired/connected devices (events log: {})",
        central::log_path().display()
    );
    let mut seen = std::collections::HashMap::<String, String>::new();
    let ts = || chrono::Local::now().format("%H:%M:%S");
    let wanted = |name: &str, mac: &str, addr: &str| {
        targets.is_empty()
            || [name, mac, addr]
                .iter()
                .any(|v| targets.contains(&v.to_lowercase()))
    };
    for ev in events {
        match ev {
            BleEvent::Ready(a) => println!("{} watching paired on {a}", ts()),
            BleEvent::Error(e) => eprintln!("{} error: {e}", ts()),
            BleEvent::Discovered(d) => {
                let key = format!("{}|{}", d.name, d.mac);
                if seen.get(&d.addr) != Some(&key) {
                    println!(
                        "{} paired name={} project={} mac={} addr={} rssi={}",
                        ts(),
                        d.name,
                        d.project,
                        d.mac,
                        d.addr,
                        d.rssi.map_or("?".into(), |r| r.to_string())
                    );
                    seen.insert(d.addr.clone(), key);
                }
            }
            BleEvent::Rssi(..) => {}
            BleEvent::Connected(d) => {
                println!("{} connected {} ({})", ts(), d.name, d.addr)
            }
            BleEvent::Disconnected(a) => println!("{} DISCONNECTED {a}", ts()),
            BleEvent::Removed(a) => println!("{} unpaired {a}", ts()),
            BleEvent::TimeOffset {
                addr,
                offset_ms,
                rtt_ms,
            } => {
                println!(
                    "{} {addr}: time offset {offset_ms:+.1} ms (rtt {rtt_ms:.1} ms)",
                    ts()
                )
            }
            BleEvent::Info(m) => println!("{} {m}", ts()),
            BleEvent::DeviceLine(a, l) => {
                if wanted(&a, &a, &a) {
                    println!("{} {a} {l}", ts());
                }
            }
        }
    }
    std::process::exit(1);
}

fn run_gpui(start: Mode, arg: Option<PathBuf>, adapter: Option<String>) {
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(840.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("target-editor".into()),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| Host::new(window, cx, arg.clone(), start, adapter.clone()))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    match args.as_slice() {
        [flag, project, chip] if flag == "--build" => headless_build(project, chip),
        rest => {
            let mut manager = false;
            let mut headless = false;
            let mut flash = false;
            let mut virtual_mode = false;
            let mut targets = Vec::new();
            let mut adapter: Option<String> = None;
            let mut file: Option<PathBuf> = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--manager" => manager = true,
                    "--headless" => headless = true,
                    "--flash" => flash = true,
                    "--virtual" => virtual_mode = true,
                    "--connect" => match it.next() {
                        Some(t) => targets.push(t.clone()),
                        None => {
                            eprintln!("--connect needs a value\n{USAGE}");
                            std::process::exit(2)
                        }
                    },
                    "--adapter" => match it.next() {
                        Some(t) => adapter = Some(t.clone()),
                        None => {
                            eprintln!("--adapter needs a value\n{USAGE}");
                            std::process::exit(2)
                        }
                    },
                    s if s.starts_with('-') => {
                        eprintln!("{USAGE}");
                        std::process::exit(2)
                    }
                    s => file = Some(PathBuf::from(s)),
                }
            }
            if !headless && !targets.is_empty() {
                headless = true;
            }
            if headless {
                headless_manager(targets, adapter)
            } else {
                let start = if virtual_mode {
                    Mode::Virtual
                } else if manager {
                    Mode::Manager
                } else if flash {
                    Mode::Flash
                } else {
                    Mode::Manager
                };
                let arg = file.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
                run_gpui(start, arg, adapter);
            }
        }
    }
}
