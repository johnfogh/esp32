use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Image MIME type for embedded assets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mime {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
}

impl Mime {
    pub fn as_str(self) -> &'static str {
        match self {
            Mime::Png => "image/png",
            Mime::Jpeg => "image/jpeg",
            Mime::Gif => "image/gif",
            Mime::Webp => "image/webp",
            Mime::Svg => "image/svg+xml",
        }
    }
}

/// Build a `data:<mime>;base64,...` URI from raw bytes.
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    let b64 = STANDARD.encode(bytes);
    format!("data:{mime};base64,{b64}")
}
