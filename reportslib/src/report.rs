use std::fs;
use std::io;
use std::path::Path;

use crate::asset::Mime;
use crate::html;

/// Stages-like target card for [`Report::main_section`].
///
/// Shows definition label, image, and zones. Omits name and sensor hub.
#[derive(Clone, Debug, Default)]
pub struct TargetCard {
    pub def: Option<String>,
    pub image_png: Option<Vec<u8>>,
    pub zones: Vec<String>,
    pub row: Option<u32>,
    pub col: Option<u32>,
    pub distance: Option<String>,
}

impl TargetCard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn def(mut self, def: impl Into<String>) -> Self {
        self.def = Some(def.into());
        self
    }

    pub fn image_png(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.image_png = Some(bytes.into());
        self
    }

    pub fn zones<I, S>(mut self, zones: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.zones = zones.into_iter().map(Into::into).collect();
        self
    }

    pub fn at(mut self, row: u32, col: u32) -> Self {
        self.row = Some(row);
        self.col = Some(col);
        self
    }

    pub fn distance(mut self, text: impl Into<String>) -> Self {
        self.distance = Some(text.into());
        self
    }

    pub(crate) fn is_distance_only(&self) -> bool {
        self.distance.is_some()
            && self.def.is_none()
            && self.image_png.is_none()
            && self.zones.is_empty()
    }
}

/// Visual kind for a timeline marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineKind {
    Log,
    Message,
}

/// Event marker on the Main Section timeline.
#[derive(Clone, Debug)]
pub struct TimelineEvent {
    /// Time in a consistent unit (e.g. seconds from run start).
    pub t: f64,
    pub label: String,
    pub kind: TimelineKind,
    pub detail: Option<String>,
}

impl TimelineEvent {
    pub fn log(t: f64, label: impl Into<String>) -> Self {
        Self {
            t,
            label: label.into(),
            kind: TimelineKind::Log,
            detail: None,
        }
    }

    pub fn message(t: f64, label: impl Into<String>) -> Self {
        Self {
            t,
            label: label.into(),
            kind: TimelineKind::Message,
            detail: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A content block in a report.
#[derive(Clone, Debug)]
pub enum Block {
    Heading { level: u8, text: String },
    Paragraph(String),
    /// Trusted HTML fragment; inserted without escaping.
    Html(String),
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    List { ordered: bool, items: Vec<String> },
    Image {
        mime: String,
        data: Vec<u8>,
        alt: String,
    },
    /// Stages-like target grid on top, horizontal event timeline below.
    MainSection {
        grid_rows: Option<u32>,
        grid_cols: Option<u32>,
        cards: Vec<TargetCard>,
        events: Vec<TimelineEvent>,
    },
    /// Raw JSON dump (caller-formatted); rendered in a Data section `<pre>`.
    DataSection { json: String },
}

/// Builder for a self-contained HTML report.
#[derive(Clone, Debug)]
pub struct Report {
    pub(crate) title: String,
    pub(crate) meta: Vec<(String, String)>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) styles: Vec<String>,
    pub(crate) scripts: Vec<String>,
}

impl Report {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            meta: Vec::new(),
            blocks: Vec::new(),
            styles: Vec::new(),
            scripts: Vec::new(),
        }
    }

    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.meta.push((key.into(), value.into()));
        self
    }

    pub fn heading(mut self, level: u8, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Heading {
            level: level.clamp(1, 6),
            text: text.into(),
        });
        self
    }

    pub fn paragraph(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Paragraph(text.into()));
        self
    }

    /// Append a trusted HTML fragment (not escaped).
    pub fn html(mut self, raw: impl Into<String>) -> Self {
        self.blocks.push(Block::Html(raw.into()));
        self
    }

    pub fn table<H, R, C>(mut self, headers: H, rows: R) -> Self
    where
        H: IntoIterator,
        H::Item: Into<String>,
        R: IntoIterator,
        R::Item: IntoIterator<Item = C>,
        C: Into<String>,
    {
        let headers: Vec<String> = headers.into_iter().map(Into::into).collect();
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|row| row.into_iter().map(Into::into).collect())
            .collect();
        self.blocks.push(Block::Table { headers, rows });
        self
    }

    pub fn list<I, S>(mut self, ordered: bool, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.blocks.push(Block::List {
            ordered,
            items: items.into_iter().map(Into::into).collect(),
        });
        self
    }

    pub fn image(mut self, bytes: impl Into<Vec<u8>>, mime: &str, alt: impl Into<String>) -> Self {
        self.blocks.push(Block::Image {
            mime: mime.to_string(),
            data: bytes.into(),
            alt: alt.into(),
        });
        self
    }

    pub fn image_png(self, bytes: impl Into<Vec<u8>>, alt: impl Into<String>) -> Self {
        self.image(bytes, Mime::Png.as_str(), alt)
    }

    pub fn image_jpeg(self, bytes: impl Into<Vec<u8>>, alt: impl Into<String>) -> Self {
        self.image(bytes, Mime::Jpeg.as_str(), alt)
    }

    /// Extra CSS appended after the default theme.
    pub fn style(mut self, css: impl Into<String>) -> Self {
        self.styles.push(css.into());
        self
    }

    /// Extra JavaScript appended into an inline `<script>` at the end of `<body>`.
    pub fn script(mut self, js: impl Into<String>) -> Self {
        self.scripts.push(js.into());
        self
    }

    /// Main section: Stages-like target cards (no name / hub) above a horizontal timeline.
    pub fn main_section(
        mut self,
        grid_rows: Option<u32>,
        grid_cols: Option<u32>,
        cards: impl IntoIterator<Item = TargetCard>,
        events: impl IntoIterator<Item = TimelineEvent>,
    ) -> Self {
        self.blocks.push(Block::MainSection {
            grid_rows,
            grid_cols,
            cards: cards.into_iter().collect(),
            events: events.into_iter().collect(),
        });
        self
    }

    /// Data section: embed events (or any payload) as raw JSON text.
    ///
    /// The library does not parse or pretty-print; pass already-formatted JSON.
    pub fn data_section(mut self, json: impl Into<String>) -> Self {
        self.blocks.push(Block::DataSection {
            json: json.into(),
        });
        self
    }

    /// Render a complete self-contained HTML document.
    pub fn render(&self) -> String {
        html::render(self)
    }

    /// Write the report to `path` as UTF-8 HTML.
    pub fn write_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.render())
    }
}
