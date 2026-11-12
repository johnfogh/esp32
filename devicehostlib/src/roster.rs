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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    fn lock_env() -> MutexGuard<'static, ()> {
        static G: Mutex<()> = Mutex::new(());
        G.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn path_and_sock_use_xdg_runtime_dir() {
        let _g = lock_env();
        let dir = std::env::temp_dir().join(format!(
            "devicehostlib-roster-{}-{}",
            std::process::id(),
            "xdg"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: serialized by lock_env; restored before unlock.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
        assert_eq!(path(), dir.join("esp32-virtual-target.json"));
        assert_eq!(sock_path(), dir.join("esp32-virtual-target.sock"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guard_removes_json_and_sock_on_drop() {
        let _g = lock_env();
        let dir = std::env::temp_dir().join(format!(
            "devicehostlib-roster-{}-{}",
            std::process::id(),
            "guard"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };

        let json = path();
        let sock = sock_path();
        std::fs::write(&json, "{}").unwrap();
        std::fs::write(&sock, "").unwrap();
        {
            let _guard = Guard::new();
            assert!(!json.exists());
            assert!(!sock.exists());
            std::fs::write(&json, "{}").unwrap();
            std::fs::write(&sock, "").unwrap();
            assert!(json.exists());
            assert!(sock.exists());
        }
        assert!(!json.exists());
        assert!(!sock.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}


