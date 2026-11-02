//! `$XDG_RUNTIME_DIR/esp32-virtual-target.json` / `.sock` paths; unlink on drop.
use std::path::PathBuf;

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn path() -> PathBuf {
    runtime_dir().join("esp32-virtual-target.json")
}

pub fn sock_path() -> PathBuf {
    runtime_dir().join("esp32-virtual-target.sock")
}

pub struct Guard;

impl Guard {
    pub fn new() -> Self {
        let _ = std::fs::remove_file(path());
        let _ = std::fs::remove_file(sock_path());
        Self
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(path());
        let _ = std::fs::remove_file(sock_path());
    }
}


