//! Self-contained static HTML report builder.
//!
//! Generated reports are a single HTML file with inline CSS, inline JavaScript,
//! and images encoded as `data:` base64 URIs. Library markup does not reference
//! external files.

mod asset;
mod html;
mod report;

pub use asset::{data_uri, Mime};
pub use report::{Block, Report, TargetCard, TimelineEvent, TimelineKind};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tiny_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0xfe,
            0xd4, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn mime_as_str_and_data_uri() {
        assert_eq!(Mime::Png.as_str(), "image/png");
        assert_eq!(Mime::Jpeg.as_str(), "image/jpeg");
        assert_eq!(Mime::Gif.as_str(), "image/gif");
        assert_eq!(Mime::Webp.as_str(), "image/webp");
        assert_eq!(Mime::Svg.as_str(), "image/svg+xml");
        let uri = data_uri(Mime::Png.as_str(), b"abc");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(uri.ends_with("YWJj"));
    }

    #[test]
    fn report_render_escapes_and_covers_blocks() {
        let html = Report::new("A & B <C>")
            .meta("Key\"", "Val'")
            .heading(2, "Overview")
            .paragraph("Hello <world>")
            .html("<em>trusted</em>")
            .table(["H1", "H2"], [["a", "b"], ["c", "d"]])
            .list(false, ["one", "two"])
            .list(true, ["first"])
            .image_png(tiny_png(), "overlay")
            .image_jpeg(b"jpeg-bytes", "jpg alt")
            .style(".extra { color: red; }")
            .script("window.__ready = true;")
            .data_section(r#"{"hits":1}"#)
            .main_section(
                Some(1),
                Some(2),
                [
                    TargetCard::new()
                        .def("plate")
                        .zones(["A", "B"])
                        .at(0, 1)
                        .image_png(tiny_png()),
                    TargetCard::new().distance("10 yd"),
                ],
                [
                    TimelineEvent::log(1.0, "hit").detail("zone A"),
                    TimelineEvent::message(2.5, "msg"),
                ],
            )
            .render();

        assert!(html.contains("<title>A &amp; B &lt;C&gt;</title>"));
        assert!(html.contains("&quot;"));
        assert!(html.contains("&#39;"));
        assert!(html.contains("<em>trusted</em>"));
        assert!(html.contains("<h2>Overview</h2>"));
        assert!(html.contains("Hello &lt;world&gt;"));
        assert!(html.contains("<table>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("<ol>"));
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("data:image/jpeg;base64,"));
        assert!(html.contains(".extra { color: red; }"));
        assert!(html.contains("window.__ready = true;"));
        assert!(html.contains("{&quot;hits&quot;:1}"));
        assert!(html.contains("main-section"));
        assert!(html.contains("plate"));
        assert!(html.contains("10 yd"));
        assert!(html.contains("hit"));
        assert!(html.contains("zone A"));
        assert!(html.contains("msg"));
    }

    #[test]
    fn target_card_distance_only_and_write_to() {
        let distance_only = TargetCard::new().distance("5 m");
        assert!(distance_only.is_distance_only());
        let with_def = TargetCard::new().def("x").distance("5 m");
        assert!(!with_def.is_distance_only());

        let dir = std::env::temp_dir().join(format!("reportslib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.html");
        Report::new("Write me")
            .paragraph("body")
            .write_to(&path)
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("Write me"));
        assert!(written.contains("body"));
        let _ = std::fs::remove_dir_all(&dir);
        let _: PathBuf = path;
    }
}
