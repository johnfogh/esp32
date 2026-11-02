//! Roster mode: editable name/email table in `~/Documents/target_editor/rosters/roster_YYMMDD.json`.
use std::path::{Path, PathBuf};

use gpui::{div, prelude::*, px, rgb, ClickEvent};

use crate::chrome;
use crate::host::Host;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub email: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Email,
}

pub struct Roster {
    pub entries: Vec<Entry>,
    pub path: PathBuf,
    pub edit: Option<(usize, Field, String)>,
    pub focus: Option<usize>,
    pub load_err: Option<String>,
}

impl Roster {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            path: today_path(),
            edit: None,
            focus: None,
            load_err: None,
        }
    }

    pub fn load_today(&mut self) {
        self.path = today_path();
        self.edit = None;
        self.focus = None;
        self.load_err = None;
        match load(&self.path) {
            Ok(entries) => self.entries = entries,
            Err(e) => {
                self.entries = Vec::new();
                self.load_err = Some(e);
            }
        }
    }

    pub fn save(&mut self) -> Result<(), String> {
        save(&self.path, &self.entries)
    }

    pub fn name(&self, i: usize) -> Option<&str> {
        self.entries.get(i).map(|e| e.name.as_str())
    }
}

fn documents_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Documents")
}

pub fn rosters_dir() -> PathBuf {
    documents_dir().join("target_editor").join("rosters")
}

fn today_path() -> PathBuf {
    let stamp = chrono::Local::now().format("%y%m%d");
    rosters_dir().join(format!("roster_{stamp}.json"))
}

fn load(path: &Path) -> Result<Vec<Entry>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            save(path, &[])?;
            Ok(Vec::new())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn save(path: &Path, entries: &[Entry]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    devicehostlib::devices::write_atomic(path, &json)
}

fn btn(
    host: &Host,
    cx: &mut gpui::Context<Host>,
    id: &'static str,
    label: &'static str,
    f: impl Fn(&mut Host, &mut gpui::Context<Host>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let t = host.theme;
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_sm()
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .hover(|s| s.border_color(rgb(t.accent)))
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            f(this, cx);
            cx.notify();
        }))
}

fn cell_text(editing: bool, buf: &str, value: &str) -> String {
    if editing {
        format!("{buf}▏")
    } else if value.is_empty() {
        " ".into()
    } else {
        value.to_string()
    }
}

pub fn view(host: &Host, cx: &mut gpui::Context<Host>) -> gpui::Div {
    let t = host.theme;
    let r = &host.roster;
    let file = r
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "roster".into());
    let mut table = div()
        .flex()
        .flex_col()
        .w_full()
        .border_1()
        .border_color(rgb(t.border))
        .child(
            chrome::table_row()
                .text_color(rgb(t.muted))
                .border_b_1()
                .border_color(rgb(t.border))
                .bg(rgb(chrome::row_bg(t, false, false)))
                .child(chrome::cell(1.0, div().px_1().py_0p5().child("NAME")))
                .child(chrome::cell(1.0, div().px_1().py_0p5().child("EMAIL"))),
        );
    for (i, entry) in r.entries.iter().enumerate() {
        let edit_name = r.edit.as_ref().is_some_and(|(j, f, _)| *j == i && *f == Field::Name);
        let edit_email = r.edit.as_ref().is_some_and(|(j, f, _)| *j == i && *f == Field::Email);
        let focused = r.focus == Some(i);
        let buf = r.edit.as_ref().map(|(_, _, b)| b.as_str()).unwrap_or("");
        let name_text = cell_text(edit_name, buf, &entry.name);
        let email_text = cell_text(edit_email, buf, &entry.email);
        let stripe = i % 2 == 1;
        let editing = edit_name || edit_email;
        table = table.child(
            chrome::table_row()
                .id(("roster-row", i as u64))
                .border_b_1()
                .border_color(rgb(t.border))
                .bg(rgb(chrome::row_bg(t, focused && !editing, stripe)))
                .text_color(rgb(if focused && !editing {
                    t.selection_fg
                } else {
                    t.foreground
                }))
                .child(
                    chrome::cell(
                        1.0,
                        div()
                            .id(("roster-name", i as u64))
                            .w_full()
                            .px_1()
                            .py_0p5()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.roster_select(i);
                                this.start_roster_edit(i, Field::Name);
                                cx.notify();
                            }))
                            .child(name_text),
                    ),
                )
                .child(
                    chrome::cell(
                        1.0,
                        div()
                            .id(("roster-email", i as u64))
                            .w_full()
                            .px_1()
                            .py_0p5()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.roster_select(i);
                                this.start_roster_edit(i, Field::Email);
                                cx.notify();
                            }))
                            .child(email_text),
                    ),
                ),
        );
    }
    if r.entries.is_empty() {
        table = table.child(
            div()
                .px_1()
                .py_1()
                .text_color(rgb(t.muted))
                .child("Add a name to start today's roster"),
        );
    }

    let mut header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .child(div().flex_1().text_color(rgb(t.muted)).child(file))
        .child(btn(host, cx, "roster-add", "Add", |this, _| {
            this.roster_add();
        }))
        .child(btn(host, cx, "roster-del", "Delete", |this, _| {
            this.roster_delete_focused();
        }));
    if let Some(err) = &r.load_err {
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
                .id("roster-table")
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .overflow_y_scroll()
                .p_2()
                .child(table),
        )
}
