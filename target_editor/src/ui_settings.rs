//! App settings: `~/.config/target-editor/settings.json`.
use std::path::PathBuf;

use gpui::{div, prelude::*, px, relative, rgb, ClickEvent};
use serde::{Deserialize, Serialize};

use crate::chrome;
use crate::host::Host;

#[derive(Debug, Clone)]
pub struct Setting {
    pub title: String,
    pub description: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct File {
    settings: Vec<[String; 3]>,
}

pub struct Settings {
    pub items: Vec<Setting>,
    pub value_edit: Option<(usize, String)>,
    pub focus: Option<usize>,
    pub load_err: Option<String>,
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self {
            items: defaults(),
            value_edit: None,
            focus: None,
            load_err: None,
        };
        s.reload();
        s
    }

    pub fn reload(&mut self) {
        self.value_edit = None;
        self.focus = None;
        self.load_err = None;
        match load_file() {
            Ok(items) => self.items = merge_defaults(items),
            Err(e) => {
                self.items = defaults();
                self.load_err = Some(e);
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        save_file(&self.items)
    }

    pub fn bullet_size_px(&self) -> f32 {
        let raw = self
            .items
            .iter()
            .find(|s| s.title == "virtual_bullet_size")
            .map(|s| s.value.as_str())
            .unwrap_or("10px");
        parse_px(raw).unwrap_or(10.0).max(1.0)
    }

    pub fn default_virtual_sensor_hubs(&self) -> usize {
        self.items
            .iter()
            .find(|s| s.title == "default_virtual_sensor_hubs")
            .and_then(|s| s.value.trim().parse().ok())
            .unwrap_or(2)
    }

    pub fn reports_dir(&self) -> PathBuf {
        let raw = self
            .items
            .iter()
            .find(|s| s.title == "reports_path")
            .map(|s| s.value.as_str())
            .unwrap_or("~/Documents/target_editor_reports");
        expand_path(raw)
    }

    pub fn stages_dir(&self) -> PathBuf {
        let raw = self
            .items
            .iter()
            .find(|s| s.title == "default_stages_directory")
            .map(|s| s.value.as_str())
            .unwrap_or("~/dev/esp32/course_defs");
        expand_path(raw)
    }

    pub fn commit_value(&mut self) {
        let Some((i, buf)) = self.value_edit.take() else {
            return;
        };
        if let Some(item) = self.items.get_mut(i) {
            item.value = buf;
        }
    }

    pub fn start_value_edit(&mut self, i: usize) {
        self.commit_value();
        let Some(item) = self.items.get(i) else {
            return;
        };
        self.focus = Some(i);
        self.value_edit = Some((i, item.value.clone()));
    }
}

fn defaults() -> Vec<Setting> {
    vec![
        Setting {
            title: "virtual_bullet_size".into(),
            description: "size in pixels of virtual bullets in sumlated runs.".into(),
            value: "10px".into(),
        },
        Setting {
            title: "default_virtual_sensor_hubs".into(),
            description: "The number of virtual sensor hubs that should be created on startup"
                .into(),
            value: "2".into(),
        },
        Setting {
            title: "reports_path".into(),
            description: "The directory to store reports in.".into(),
            value: "~/Documents/target_editor_reports".into(),
        },
        Setting {
            title: "default_stages_directory".into(),
            description: "the directory where stages are loaded from".into(),
            value: "~/dev/esp32/course_defs".into(),
        },
    ]
}

fn expand_path(s: &str) -> PathBuf {
    if s == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(s));
    }
    PathBuf::from(s)
}

fn merge_defaults(mut items: Vec<Setting>) -> Vec<Setting> {
    for d in defaults() {
        if !items.iter().any(|s| s.title == d.title) {
            items.push(d);
        }
    }
    items
}

fn parse_px(s: &str) -> Option<f32> {
    let t = s.trim();
    let t = t.strip_suffix("px").unwrap_or(t).trim();
    t.parse().ok()
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("target-editor"))
}

fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

fn load_file() -> Result<Vec<Setting>, String> {
    let p = path().ok_or("no config dir ($HOME unset)")?;
    match std::fs::read_to_string(&p) {
        Ok(text) => {
            let f: File = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            Ok(f.settings
                .into_iter()
                .map(|[title, description, value]| Setting {
                    title,
                    description,
                    value,
                })
                .collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let items = defaults();
            save_file(&items)?;
            Ok(items)
        }
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn save_file(items: &[Setting]) -> Result<(), String> {
    let p = path().ok_or("no config dir ($HOME unset)")?;
    let f = File {
        settings: items
            .iter()
            .map(|s| [s.title.clone(), s.description.clone(), s.value.clone()])
            .collect(),
    };
    let json = serde_json::to_string_pretty(&f).map_err(|e| e.to_string())?;
    devicehostlib::devices::write_atomic(&p, &json)
}

pub fn view(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let s = &host.settings;
    let size_px = host.line_px();
    let mut table = div()
        .flex()
        .flex_col()
        .w_full()
        .border_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex()
                .flex_row()
                .w_full()
                .border_b_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.surface))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .px_2()
                        .py_0p5()
                        .text_color(rgb(t.muted))
                        .child("SETTING"),
                )
                .child(
                    div()
                        .w(relative(0.35))
                        .min_w(px(80.))
                        .px_2()
                        .py_0p5()
                        .text_color(rgb(t.muted))
                        .border_l_1()
                        .border_color(rgb(t.border))
                        .child("VALUE"),
                ),
        );

    for (i, item) in s.items.iter().enumerate() {
        let editing = s.value_edit.as_ref().is_some_and(|(j, _)| *j == i);
        let focused = s.focus == Some(i);
        let value_text = if editing {
            format!(
                "{}▏",
                s.value_edit.as_ref().map(|(_, b)| b.as_str()).unwrap_or("")
            )
        } else if item.value.is_empty() {
            " ".into()
        } else {
            item.value.clone()
        };
        let stripe = i % 2 == 1;
        let bg = chrome::row_bg(t, focused && !editing, stripe);
        let fg = if focused && !editing {
            t.selection_fg
        } else {
            t.foreground
        };
        table = table.child(
            div()
                .id(("settings-row", i as u64))
                .flex()
                .flex_row()
                .w_full()
                .items_start()
                .border_b_1()
                .border_color(rgb(t.border))
                .bg(rgb(bg))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .px_2()
                        .py_1()
                        .child(
                            div()
                                .text_color(rgb(fg))
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_color(rgb(t.muted))
                                .text_size(px((size_px - 2.0 * 96.0 / 72.0).max(1.0)))
                                .whitespace_normal()
                                .child(item.description.clone()),
                        ),
                )
                .child(
                    div()
                        .id(("settings-value", i as u64))
                        .w(relative(0.35))
                        .min_w(px(80.))
                        .px_2()
                        .py_1()
                        .border_l_1()
                        .border_color(rgb(t.border))
                        .text_color(rgb(fg))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.settings_select(i);
                            this.start_settings_value_edit(i);
                            cx.notify();
                        }))
                        .child(value_text),
                ),
        );
    }

    let mut header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .child(
            div()
                .flex_1()
                .text_color(rgb(t.muted))
                .child(
                    path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "settings.json".into()),
                ),
        );
    if let Some(err) = &s.load_err {
        header = header.child(div().text_color(rgb(t.accent)).child(err.clone()));
    }

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .child(header)
        .child(
            div()
                .id("settings-table")
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .overflow_y_scroll()
                .p_2()
                .child(table),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_px_values() {
        assert_eq!(parse_px("10px"), Some(10.0));
        assert_eq!(parse_px(" 12 "), Some(12.0));
        assert_eq!(parse_px("bad"), None);
    }
}
