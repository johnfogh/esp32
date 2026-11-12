//! Host (desktop) helpers: the chip table and the exact cargo/espflash
//! commands used to build and flash firmware projects.
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// All supported ESP32-family chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chip { Esp32, Esp32s2, Esp32s3, Esp32c3, Esp32c6, Esp32h2 }

impl Chip {
    pub const ALL: [Chip; 6] = [Chip::Esp32, Chip::Esp32s2, Chip::Esp32s3, Chip::Esp32c3, Chip::Esp32c6, Chip::Esp32h2];

    /// Display name, e.g. "ESP32-C3".
    pub fn name(self) -> &'static str {
        match self {
            Chip::Esp32 => "ESP32", Chip::Esp32s2 => "ESP32-S2", Chip::Esp32s3 => "ESP32-S3",
            Chip::Esp32c3 => "ESP32-C3", Chip::Esp32c6 => "ESP32-C6", Chip::Esp32h2 => "ESP32-H2",
        }
    }
    /// Cargo feature name; also the `espflash --chip` value.
    pub fn feature(self) -> &'static str {
        match self {
            Chip::Esp32 => "esp32", Chip::Esp32s2 => "esp32s2", Chip::Esp32s3 => "esp32s3",
            Chip::Esp32c3 => "esp32c3", Chip::Esp32c6 => "esp32c6", Chip::Esp32h2 => "esp32h2",
        }
    }
    /// Rust target triple for the given framework.
    pub fn target_triple(self, fw: Framework) -> &'static str {
        match (fw, self) {
            (Framework::Hal, Chip::Esp32) => "xtensa-esp32-none-elf",
            (Framework::Hal, Chip::Esp32s2) => "xtensa-esp32s2-none-elf",
            (Framework::Hal, Chip::Esp32s3) => "xtensa-esp32s3-none-elf",
            (Framework::Hal, Chip::Esp32c3) => "riscv32imc-unknown-none-elf",
            (Framework::Hal, Chip::Esp32c6 | Chip::Esp32h2) => "riscv32imac-unknown-none-elf",
            (Framework::Idf, Chip::Esp32) => "xtensa-esp32-espidf",
            (Framework::Idf, Chip::Esp32s2) => "xtensa-esp32s2-espidf",
            (Framework::Idf, Chip::Esp32s3) => "xtensa-esp32s3-espidf",
            (Framework::Idf, Chip::Esp32c3) => "riscv32imc-esp-espidf",
            (Framework::Idf, Chip::Esp32c6 | Chip::Esp32h2) => "riscv32imac-esp-espidf",
        }
    }
}

impl fmt::Display for Chip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.name()) }
}

/// Firmware framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework { Hal, Idf }
impl Framework {
    pub fn feature(self) -> &'static str { match self { Framework::Hal => "hal", Framework::Idf => "idf" } }
}

/// Quote a string for POSIX shells.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./,=:+@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

const PRELUDE: &str = r#". "$HOME/export-esp.sh" && export PATH="$HOME/.cargo/bin:$PATH""#;

/// The cargo invocation (shell text) that builds a project for chip/framework in release mode.
pub fn build_shell(project_dir: &Path, chip: Chip, fw: Framework) -> String {
    let mut s = format!(
        "{PRELUDE} && cd {} && cargo build --release --target {} --no-default-features --features {},{}",
        shell_quote(&project_dir.to_string_lossy()), chip.target_triple(fw), chip.feature(), fw.feature()
    );
    if fw == Framework::Idf {
        s.push_str(" -Zbuild-std=std,panic_abort");
    }
    s
}

fn bash(script: String, project_dir: &Path) -> Command {
    let mut c = Command::new("bash");
    c.arg("-c").arg(script).current_dir(project_dir);
    // Don't leak the caller's toolchain/target overrides (e.g. when run via `cargo run`),
    // so the project's own rust-toolchain.toml (`esp`) applies.
    for v in ["RUSTUP_TOOLCHAIN", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "RUSTC", "RUSTC_WRAPPER", "CARGO"] {
        c.env_remove(v);
    }
    c
}

/// `bash -c '. ~/export-esp.sh && cargo build --release ...'` for the project.
pub fn build_command(project_dir: &Path, chip: Chip, fw: Framework) -> Command {
    bash(build_shell(project_dir, chip, fw), project_dir)
}

/// Shell text for flashing the release ELF (`port: None` lets espflash auto-detect).
pub fn flash_shell(project_dir: &Path, chip: Chip, fw: Framework, port: Option<&str>) -> String {
    let mut s = format!("{PRELUDE} && espflash flash --non-interactive --chip {}", chip.feature());
    if let Some(p) = port {
        s.push_str(" --port ");
        s.push_str(&shell_quote(p));
    }
    s.push(' ');
    s.push_str(&shell_quote(&elf_path(project_dir, chip, fw).to_string_lossy()));
    s
}

/// `bash -c '... espflash flash --chip <chip> [--port P] <elf>'` (does not build).
pub fn flash_command(project_dir: &Path, chip: Chip, fw: Framework, port: Option<&str>) -> Command {
    bash(flash_shell(project_dir, chip, fw, port), project_dir)
}

/// `[package] name` from a Cargo.toml (falls back to the directory name).
pub fn package_name(project_dir: &Path) -> String {
    let text = std::fs::read_to_string(project_dir.join("Cargo.toml")).unwrap_or_default();
    let mut in_pkg = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_pkg = l == "[package]";
        } else if in_pkg
            && let Some(rest) = l.strip_prefix("name")
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            return v.trim().trim_matches('"').to_string();
        }
    }
    project_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Release ELF produced by [`build_command`].
pub fn elf_path(project_dir: &Path, chip: Chip, fw: Framework) -> PathBuf {
    project_dir.join("target").join(chip.target_triple(fw)).join("release").join(package_name(project_dir))
}

/// Serial ports that look like ESP dev boards (`/dev/ttyUSB*`, `/dev/ttyACM*`), sorted.
pub fn list_serial_ports() -> Vec<String> {
    list_serial_ports_in(Path::new("/dev"))
}

/// Like [`list_serial_ports`] but scanning `dir` (for tests).
pub fn list_serial_ports_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("ttyUSB") || n.starts_with("ttyACM")
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Subdirectories of `root` containing a Cargo.toml, minus `exclude`, sorted.
pub fn list_projects(root: &Path, exclude: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("Cargo.toml").is_file())
        .filter(|p| !p.file_name().is_some_and(|n| exclude.iter().any(|x| n == *x)))
        .collect();
    v.sort();
    v
}

impl Chip {
    /// Parse a chip name as printed by espflash/esptool ("esp32c3", "ESP32-C3", "esp32-s3", ...).
    pub fn from_name(name: &str) -> Option<Chip> {
        let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Chip::ALL.into_iter().find(|c| c.feature() == n)
    }
}

/// Extract the chip from `espflash board-info` output (the `Chip type:` line).
pub fn parse_board_info(output: &str) -> Option<Chip> {
    output.lines().find_map(|line| {
        // Strip ANSI colour codes espflash may emit.
        let mut clean = String::with_capacity(line.len());
        let mut esc = false;
        for ch in line.chars() {
            match (esc, ch) {
                (false, '\x1b') => esc = true,
                (true, c) if c.is_ascii_alphabetic() => esc = false,
                (true, _) => {}
                (false, c) => clean.push(c),
            }
        }
        let rest = clean.trim().strip_prefix("Chip type:")?;
        Chip::from_name(rest.split_whitespace().next()?)
    })
}

/// Shell text that queries one port: `timeout <secs> espflash board-info --non-interactive --port <p>`.
pub fn board_info_shell(port: &str, timeout_secs: u32) -> String {
    format!("{PRELUDE} && timeout {timeout_secs} espflash board-info --non-interactive --port {}", shell_quote(port))
}

/// Probe every serial port (in parallel) with `espflash board-info` and return the
/// ports where a supported chip answered. Blocking; takes up to `timeout_secs`.
pub fn detect_boards(timeout_secs: u32) -> Vec<(String, Chip)> {
    let handles: Vec<_> = list_serial_ports()
        .into_iter()
        .map(|port| {
            std::thread::spawn(move || {
                let mut c = bash(board_info_shell(&port, timeout_secs), Path::new("/"));
                c.stdin(std::process::Stdio::null());
                let out = c.output().ok()?;
                let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                parse_board_info(&text).map(|chip| (port, chip))
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("esp32-utils-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn chip_table() {
        assert_eq!(Chip::ALL.len(), 6);
        assert_eq!(Chip::Esp32c3.feature(), "esp32c3");
        assert_eq!(Chip::Esp32s3.name(), "ESP32-S3");
        assert_eq!(format!("{}", Chip::Esp32c6), "ESP32-C6");
        assert_eq!(Chip::Esp32.target_triple(Framework::Hal), "xtensa-esp32-none-elf");
        assert_eq!(Chip::Esp32c3.target_triple(Framework::Hal), "riscv32imc-unknown-none-elf");
        assert_eq!(Chip::Esp32c6.target_triple(Framework::Hal), "riscv32imac-unknown-none-elf");
        assert_eq!(Chip::Esp32c3.target_triple(Framework::Idf), "riscv32imc-esp-espidf");
        assert_eq!(Chip::Esp32.target_triple(Framework::Idf), "xtensa-esp32-espidf");
        assert_eq!(Chip::Esp32s2.target_triple(Framework::Idf), "xtensa-esp32s2-espidf");
        assert_eq!(Chip::Esp32s3.target_triple(Framework::Idf), "xtensa-esp32s3-espidf");
        assert_eq!(Chip::Esp32h2.target_triple(Framework::Idf), "riscv32imac-esp-espidf");
        for c in Chip::ALL {
            let xtensa = matches!(c, Chip::Esp32 | Chip::Esp32s2 | Chip::Esp32s3);
            assert_eq!(xtensa, c.target_triple(Framework::Hal).starts_with("xtensa"));
        }
    }

    #[test]
    fn build_cmd() {
        let d = Path::new("/home/x/dev/esp32/sensor");
        let c = build_command(d, Chip::Esp32c3, Framework::Hal);
        assert_eq!(c.get_program(), "bash");
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with(". \"$HOME/export-esp.sh\" && export PATH=\"$HOME/.cargo/bin:$PATH\""));
        assert!(args[1].ends_with("cd /home/x/dev/esp32/sensor && cargo build --release --target riscv32imc-unknown-none-elf --no-default-features --features esp32c3,hal"));
        assert_eq!(c.get_current_dir(), Some(d));
        assert!(c.get_envs().any(|(k, v)| k == "RUSTUP_TOOLCHAIN" && v.is_none()));
        assert!(build_shell(d, Chip::Esp32, Framework::Idf).ends_with("--features esp32,idf -Zbuild-std=std,panic_abort"));
        assert!(build_shell(Path::new("/a b/it's"), Chip::Esp32, Framework::Hal).contains("cd '/a b/it'\\''s' &&"));
    }

    #[test]
    fn elf_and_flash() {
        let d = tmp("elf");
        std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"esp32-demo\"\n[dependencies]\nname = \"x\"\n").unwrap();
        assert_eq!(elf_path(&d, Chip::Esp32s3, Framework::Hal), d.join("target/xtensa-esp32s3-none-elf/release/esp32-demo"));
        let s = flash_shell(&d, Chip::Esp32c3, Framework::Hal, Some("/dev/ttyUSB0"));
        assert!(s.contains("espflash flash --non-interactive --chip esp32c3 --port /dev/ttyUSB0 "));
        assert!(s.ends_with("target/riscv32imc-unknown-none-elf/release/esp32-demo"));
        assert!(!flash_shell(&d, Chip::Esp32, Framework::Hal, None).contains("--port"));
        let c = flash_command(&d, Chip::Esp32, Framework::Hal, None);
        assert_eq!(c.get_program(), "bash");
        assert_eq!(package_name(Path::new("/nonexistent/foo")), "foo");
    }

    #[test]
    fn serial_ports_and_projects() {
        let d = tmp("dev");
        for n in ["ttyUSB1", "ttyACM0", "ttyS0", "tty1", "ttyUSB0"] {
            std::fs::write(d.join(n), "").unwrap();
        }
        let ports = list_serial_ports_in(&d);
        let names: Vec<_> = ports.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
        assert_eq!(names, ["ttyACM0", "ttyUSB0", "ttyUSB1"]);
        let _ = list_serial_ports();

        let r = tmp("root");
        for n in ["laser", "sensor", "utils", "controller", "docs"] {
            std::fs::create_dir_all(r.join(n)).unwrap();
            if n != "docs" { std::fs::write(r.join(n).join("Cargo.toml"), "").unwrap(); }
        }
        let p = list_projects(&r, &["utils", "controller"]);
        assert_eq!(p, [r.join("laser"), r.join("sensor")]);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("abc/def"), "abc/def");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn board_info_parsing() {
        let c3 = "[2026-10-12T22:00:00Z INFO ] Serial port: '/dev/ttyACM0'\n[2026-10-12T22:00:00Z INFO ] Connecting...\n[2026-10-12T22:00:01Z INFO ] Using flash stub\nChip type:         esp32c3 (revision v0.4)\nCrystal frequency: 40 MHz\nFlash size:        4MB\nFeatures:          WiFi, BLE\nMAC address:       a0:76:4e:00:11:22\n";
        assert_eq!(parse_board_info(c3), Some(Chip::Esp32c3));
        let esp32 = "Chip type:         esp32 (revision v3.0)\nCrystal frequency: 40 MHz\nFeatures:          WiFi, BT, Dual Core, 240MHz\n";
        assert_eq!(parse_board_info(esp32), Some(Chip::Esp32));
        assert_eq!(parse_board_info("\x1b[1mChip type:\x1b[0m   esp32s3 (revision v0.2)"), Some(Chip::Esp32s3));
        assert_eq!(parse_board_info("Chip type: ESP32-H2"), Some(Chip::Esp32h2));
        assert_eq!(parse_board_info("Chip type: esp32p4 (revision v1.0)"), None);
        assert_eq!(parse_board_info("Error: espflash::connection_failed\n  x Error while connecting to device"), None);
        assert_eq!(parse_board_info(""), None);
        assert!(board_info_shell("/dev/ttyUSB0", 10).ends_with("timeout 10 espflash board-info --non-interactive --port /dev/ttyUSB0"));
        assert_eq!(Chip::from_name("esp32-c6"), Some(Chip::Esp32c6));
        // No serial ports here -> empty, and returns quickly.
        if list_serial_ports().is_empty() { assert!(detect_boards(5).is_empty()); }
    }
}
