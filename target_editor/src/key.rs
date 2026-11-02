//! Crate-local keys (GPUI `KeyDownEvent` mapped in `host`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: Code,
    pub ctrl: bool,
    pub shift: bool,
}

impl Key {
    #[cfg(test)]
    pub fn new(code: Code) -> Self {
        Self { code, ctrl: false, shift: false }
    }

    pub fn from_gpui(key: &str, key_char: Option<&str>, ctrl: bool, shift: bool, alt: bool) -> Option<Self> {
        if alt {
            return None;
        }
        let code = match key {
            "enter" => Code::Enter,
            "escape" => Code::Esc,
            "tab" if shift => Code::BackTab,
            "tab" => Code::Tab,
            "backspace" => Code::Backspace,
            "delete" => Code::Delete,
            "up" => Code::Up,
            "down" => Code::Down,
            "left" => Code::Left,
            "right" => Code::Right,
            "home" => Code::Home,
            "end" => Code::End,
            "pageup" => Code::PageUp,
            "pagedown" => Code::PageDown,
            "space" => Code::Char(' '),
            k if k.len() == 1 => {
                let c = key_char.and_then(|s| s.chars().next()).unwrap_or_else(|| k.chars().next().unwrap());
                Code::Char(c)
            }
            _ => {
                if let Some(c) = key_char.and_then(|s| s.chars().next()) {
                    Code::Char(c)
                } else {
                    return None;
                }
            }
        };
        Some(Self { code, ctrl, shift })
    }
}
