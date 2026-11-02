use crate::asset::{data_uri, Mime};
use crate::report::{Block, Report, TargetCard, TimelineEvent, TimelineKind};

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const DEFAULT_CSS: &str = r#"
:root {
  --bg: #1b1b1b;
  --panel: #242424;
  --fg: #efebdc;
  --muted: #817f68;
  --accent: #e75a50;
  --border: #3a3a3a;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
  background: var(--bg);
  color: var(--fg);
  line-height: 1.5;
}
.report {
  max-width: 56rem;
  margin: 0 auto;
  padding: 1.5rem 1.25rem 3rem;
}
.report:has(.main-section),
.report:has(.data-section) {
  max-width: 72rem;
}
header.page {
  border-bottom: 1px solid var(--border);
  padding-bottom: 1rem;
  margin-bottom: 1.5rem;
}
header.page h1 {
  margin: 0 0 0.5rem;
  font-size: 1.6rem;
}
.meta {
  display: flex;
  flex-wrap: wrap;
  gap: 0.35rem 1.25rem;
  color: var(--muted);
  font-size: 0.9rem;
}
.meta dt { font-weight: 600; color: var(--accent); display: inline; }
.meta dd { display: inline; margin: 0; }
h1, h2, h3, h4, h5, h6 { line-height: 1.25; margin: 1.4rem 0 0.6rem; }
p { margin: 0.6rem 0; }
table {
  width: 100%;
  border-collapse: collapse;
  margin: 0.75rem 0 1.25rem;
  font-size: 0.95rem;
}
th, td {
  border: 1px solid var(--border);
  padding: 0.45rem 0.65rem;
  text-align: left;
  vertical-align: top;
}
th { background: var(--panel); }
tr:nth-child(even) td { background: #1f1f1f; }
img.report-img {
  display: block;
  max-width: 100%;
  height: auto;
  margin: 0.75rem 0 1.25rem;
  border: 1px solid var(--border);
  border-radius: 6px;
}
ul, ol { margin: 0.5rem 0 1rem; padding-left: 1.4rem; }
li { margin: 0.25rem 0; }
.block-html { margin: 0.75rem 0 1.25rem; }

.main-section { margin: 0.75rem 0 1.5rem; }
.main-section > h2 {
  margin: 0 0 0.75rem;
  font-size: 0.75rem;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
}
.tgt-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  align-items: stretch;
  margin-bottom: 1.5rem;
}
.tgt-grid.is-grid {
  display: grid;
  gap: 0.5rem;
}
.tgt-card {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  padding: 0.4rem;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--panel);
  min-width: 12rem;
  min-height: 0;
}
.tgt-card.distance {
  align-items: center;
  justify-content: center;
  min-width: 0;
  color: var(--fg);
  font-size: 0.95rem;
}
.tgt-card .def {
  color: var(--muted);
  font-size: 0.85rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tgt-card .body {
  display: flex;
  flex-direction: row;
  gap: 0.4rem;
  align-items: flex-start;
  min-width: 0;
}
.tgt-card .preview {
  flex: 1;
  min-width: 0;
}
.tgt-card .preview img {
  display: block;
  width: 100%;
  height: auto;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: #111;
}
.tgt-card .zones {
  flex: none;
  list-style: none;
  margin: 0;
  padding: 0;
  font-size: 0.8rem;
  color: var(--muted);
  max-height: 10rem;
  overflow: auto;
}
.tgt-card .zones li { margin: 0.1rem 0; white-space: nowrap; }
.tgt-card .empty {
  color: var(--muted);
  font-size: 0.85rem;
  padding: 1rem 0.5rem;
  text-align: center;
}

.timeline {
  margin-top: 0.5rem;
  padding: 0.75rem 0.25rem 0.25rem;
}
.timeline .track {
  position: relative;
  height: 4.5rem;
  margin: 0 0.75rem;
  border-bottom: 2px solid var(--border);
}
.timeline .track::before {
  content: "";
  position: absolute;
  left: 0; right: 0; bottom: -1px;
  height: 2px;
  background: var(--border);
}
.timeline .marker {
  position: absolute;
  bottom: 0;
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  align-items: center;
  width: max-content;
  max-width: 7rem;
}
.timeline .marker .dot {
  width: 0.7rem;
  height: 0.7rem;
  border-radius: 50%;
  border: 2px solid #fff;
  background: var(--muted);
  margin-top: 0.35rem;
  transform: translateY(50%);
  box-shadow: 0 0 0 1px var(--border);
}
.timeline .marker.kind-message .dot { background: var(--accent); }
.timeline .marker .lbl {
  font-size: 0.72rem;
  color: var(--fg);
  text-align: center;
  line-height: 1.2;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 7rem;
}
.timeline .marker .t {
  font-size: 0.65rem;
  color: var(--muted);
  margin-top: 0.15rem;
}
.timeline .empty {
  color: var(--muted);
  font-size: 0.9rem;
  padding: 0.5rem 0;
}

.data-section { margin: 0.75rem 0 1.5rem; }
.data-section > h2 {
  margin: 0 0 0.75rem;
  font-size: 0.75rem;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
}
.data-section .data-json {
  margin: 0;
  padding: 0.85rem 1rem;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--panel);
  color: var(--fg);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.82rem;
  line-height: 1.45;
  white-space: pre-wrap;
  overflow-x: auto;
}
.data-section .empty {
  color: var(--muted);
  font-size: 0.9rem;
  padding: 0.5rem 0;
}
"#;

pub(crate) fn render(report: &Report) -> String {
    let title = escape(&report.title);
    let mut meta_html = String::new();
    if !report.meta.is_empty() {
        meta_html.push_str("<dl class=\"meta\">\n");
        for (k, v) in &report.meta {
            meta_html.push_str(&format!(
                "  <div><dt>{}</dt> <dd>{}</dd></div>\n",
                escape(k),
                escape(v)
            ));
        }
        meta_html.push_str("</dl>\n");
    }

    let mut body = String::new();
    for block in &report.blocks {
        body.push_str(&render_block(block));
    }

    let mut styles = String::from(DEFAULT_CSS);
    for css in &report.styles {
        styles.push('\n');
        styles.push_str(css);
    }

    let mut scripts = String::new();
    for js in &report.scripts {
        if !scripts.is_empty() {
            scripts.push('\n');
        }
        scripts.push_str(js);
    }

    let script_tag = if scripts.is_empty() {
        String::new()
    } else {
        format!("<script>\n{scripts}\n</script>\n")
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{title}</title>
<style>
{styles}
</style>
</head>
<body>
<article class="report">
<header class="page">
  <h1>{title}</h1>
  {meta_html}</header>
{body}</article>
{script_tag}</body>
</html>
"#
    )
}

fn render_block(block: &Block) -> String {
    match block {
        Block::Heading { level, text } => {
            let level = (*level).clamp(1, 6);
            format!("<h{level}>{}</h{level}>\n", escape(text))
        }
        Block::Paragraph(text) => format!("<p>{}</p>\n", escape(text)),
        Block::Html(raw) => format!("<div class=\"block-html\">{raw}</div>\n"),
        Block::Table { headers, rows } => {
            let mut s = String::from("<table>\n<thead><tr>");
            for h in headers {
                s.push_str(&format!("<th>{}</th>", escape(h)));
            }
            s.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                s.push_str("<tr>");
                for cell in row {
                    s.push_str(&format!("<td>{}</td>", escape(cell)));
                }
                s.push_str("</tr>\n");
            }
            s.push_str("</tbody>\n</table>\n");
            s
        }
        Block::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            let mut s = format!("<{tag}>\n");
            for item in items {
                s.push_str(&format!("  <li>{}</li>\n", escape(item)));
            }
            s.push_str(&format!("</{tag}>\n"));
            s
        }
        Block::Image { mime, data, alt } => {
            let src = data_uri(mime, data);
            format!(
                "<img class=\"report-img\" src=\"{src}\" alt=\"{}\"/>\n",
                escape(alt)
            )
        }
        Block::MainSection {
            grid_rows,
            grid_cols,
            cards,
            events,
        } => render_main_section(*grid_rows, *grid_cols, cards, events),
        Block::DataSection { json } => render_data_section(json),
    }
}

fn render_data_section(json: &str) -> String {
    let mut s = String::from("<section class=\"data-section\">\n");
    s.push_str("<h2>Data</h2>\n");
    if json.is_empty() {
        s.push_str("<div class=\"empty\">No data</div>\n");
    } else {
        s.push_str(&format!(
            "<pre class=\"data-json\">{}</pre>\n",
            escape(json)
        ));
    }
    s.push_str("</section>\n");
    s
}

fn render_main_section(
    grid_rows: Option<u32>,
    grid_cols: Option<u32>,
    cards: &[TargetCard],
    events: &[TimelineEvent],
) -> String {
    let mut s = String::from("<section class=\"main-section\">\n");
    s.push_str("<h2>Main</h2>\n");
    s.push_str(&render_target_grid(grid_rows, grid_cols, cards));
    s.push_str(&render_timeline(events));
    s.push_str("</section>\n");
    s
}

fn render_target_grid(
    grid_rows: Option<u32>,
    grid_cols: Option<u32>,
    cards: &[TargetCard],
) -> String {
    let use_grid = matches!((grid_rows, grid_cols), (Some(r), Some(c)) if r > 0 && c > 0);
    let mut s = if use_grid {
        let cols = grid_cols.unwrap();
        let rows = grid_rows.unwrap();
        format!(
            "<div class=\"tgt-grid is-grid\" style=\"grid-template-columns:repeat({cols},minmax(0,1fr));grid-template-rows:repeat({rows},minmax(0,1fr));\">\n"
        )
    } else {
        String::from("<div class=\"tgt-grid\">\n")
    };

    if cards.is_empty() {
        s.push_str("  <div class=\"tgt-card empty\">No targets</div>\n");
    } else {
        for card in cards {
            s.push_str(&render_target_card(card, use_grid));
        }
    }
    s.push_str("</div>\n");
    s
}

fn render_target_card(card: &TargetCard, place: bool) -> String {
    let mut style = String::new();
    if place {
        if let (Some(r), Some(c)) = (card.row, card.col) {
            style = format!(" style=\"grid-row:{r};grid-column:{c};\"");
        }
    }

    if card.is_distance_only() {
        let text = card.distance.as_deref().unwrap_or("");
        return format!(
            "  <div class=\"tgt-card distance\"{style}><span>{}</span></div>\n",
            escape(text)
        );
    }

    let mut s = format!("  <div class=\"tgt-card\"{style}>\n");
    if let Some(def) = &card.def {
        s.push_str(&format!("    <div class=\"def\">{}</div>\n", escape(def)));
    }

    s.push_str("    <div class=\"body\">\n");
    s.push_str("      <div class=\"preview\">");
    if let Some(png) = &card.image_png {
        let src = data_uri(Mime::Png.as_str(), png);
        let alt = card.def.as_deref().unwrap_or("target");
        s.push_str(&format!(
            "<img src=\"{src}\" alt=\"{}\"/>",
            escape(alt)
        ));
    } else {
        s.push_str("<div class=\"empty\">No image</div>");
    }
    s.push_str("</div>\n");

    if !card.zones.is_empty() {
        s.push_str("      <ul class=\"zones\">\n");
        for z in &card.zones {
            s.push_str(&format!("        <li>{}</li>\n", escape(z)));
        }
        s.push_str("      </ul>\n");
    }
    s.push_str("    </div>\n");
    s.push_str("  </div>\n");
    s
}

fn render_timeline(events: &[TimelineEvent]) -> String {
    let mut s = String::from("<div class=\"timeline\">\n");
    if events.is_empty() {
        s.push_str("  <div class=\"empty\">No events</div>\n");
        s.push_str("</div>\n");
        return s;
    }

    let t_min = events.iter().map(|e| e.t).fold(f64::INFINITY, f64::min);
    let t_max = events.iter().map(|e| e.t).fold(f64::NEG_INFINITY, f64::max);
    let span = (t_max - t_min).max(1e-9);

    s.push_str("  <div class=\"track\">\n");
    for ev in events {
        let pct = ((ev.t - t_min) / span * 100.0).clamp(0.0, 100.0);
        let kind = match ev.kind {
            TimelineKind::Log => "kind-log",
            TimelineKind::Message => "kind-message",
        };
        let title = match &ev.detail {
            Some(d) => format!("{} — {}", ev.label, d),
            None => ev.label.clone(),
        };
        s.push_str(&format!(
            "    <div class=\"marker {kind}\" style=\"left:{pct:.3}%;\" title=\"{}\">\n",
            escape(&title)
        ));
        s.push_str(&format!(
            "      <span class=\"lbl\">{}</span>\n",
            escape(&ev.label)
        ));
        s.push_str(&format!(
            "      <span class=\"t\">{:.1}s</span>\n",
            ev.t
        ));
        s.push_str("      <span class=\"dot\"></span>\n");
        s.push_str("    </div>\n");
    }
    s.push_str("  </div>\n");
    s.push_str("</div>\n");
    s
}
