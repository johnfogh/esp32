//! GPUI image wrapper around `targetlib` documents.
use std::path::PathBuf;
use std::sync::Arc;

pub struct DocImage {
    pub bytes: Vec<u8>,
    pub gpui: Arc<gpui::Image>,
    pub name: String,
    pub source: Option<PathBuf>,
}

pub fn gpui_image(bytes: &[u8]) -> Result<Arc<gpui::Image>, String> {
    let fmt = image::guess_format(bytes).map_err(|e| format!("image: {e}"))?;
    let g = match fmt {
        image::ImageFormat::Png => gpui::ImageFormat::Png,
        image::ImageFormat::Jpeg => gpui::ImageFormat::Jpeg,
        image::ImageFormat::WebP => gpui::ImageFormat::Webp,
        image::ImageFormat::Gif => gpui::ImageFormat::Gif,
        image::ImageFormat::Bmp => gpui::ImageFormat::Bmp,
        image::ImageFormat::Tiff => gpui::ImageFormat::Tiff,
        _ => return Err(format!("unsupported image format {fmt:?}")),
    };
    Ok(Arc::new(gpui::Image::from_bytes(g, bytes.to_vec())))
}

pub fn doc_image(bytes: Vec<u8>, name: String, source: Option<PathBuf>) -> Result<DocImage, String> {
    let gpui = gpui_image(&bytes)?;
    let _ = targetlib::image_size_bytes(&bytes)?;
    Ok(DocImage { bytes, gpui, name, source })
}
