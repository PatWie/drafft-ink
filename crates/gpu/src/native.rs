//! Native platform pieces: clipboard (arboard), dropped files, cursor mapping.
#![cfg(not(target_arch = "wasm32"))]

use std::io::Cursor;

use crate::events::{ClipboardContent, DroppedFile, ImageBytes};

/// The system clipboard. Text and images (PNG on the way in and out). The private `custom`
/// payload does not exist on native clipboards; the shell keeps it in memory next to the text.
pub(crate) struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        let inner = match arboard::Clipboard::new() {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("no system clipboard: {e}");
                None
            }
        };
        Self { inner }
    }

    /// Read text and an image if the clipboard has them.
    pub(crate) fn read(&mut self) -> ClipboardContent {
        let Some(cb) = self.inner.as_mut() else { return ClipboardContent::default() };
        let text = cb.get_text().ok().filter(|t| !t.is_empty());
        let mut images = Vec::new();
        if let Ok(img) = cb.get_image() {
            match encode_png(img.width as u32, img.height as u32, &img.bytes) {
                Ok(bytes) => images.push(ImageBytes { mime: "image/png".into(), bytes }),
                Err(e) => log::warn!("clipboard image not usable: {e}"),
            }
        }
        ClipboardContent { text, images, custom: None }
    }

    /// Write text, or the first image when there is no text. (X11 and Windows hold one format per
    /// write through arboard.)
    pub(crate) fn write(&mut self, content: &ClipboardContent) {
        let Some(cb) = self.inner.as_mut() else { return };
        if let Some(text) = content.text.as_deref().filter(|t| !t.is_empty()) {
            if let Err(e) = cb.set_text(text.to_owned()) {
                log::warn!("clipboard write failed: {e}");
            }
            return;
        }
        if let Some(image) = content.images.first() {
            match decode_png(&image.bytes) {
                Ok((w, h, rgba)) => {
                    let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: rgba.into() };
                    if let Err(e) = cb.set_image(data) {
                        log::warn!("clipboard image write failed: {e}");
                    }
                }
                Err(e) => log::warn!("clipboard image not PNG: {e}"),
            }
        }
    }
}

pub(crate) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().map_err(|e| e.to_string())?.write_image_data(rgba).map_err(|e| e.to_string())?;
    Ok(out)
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    };
    Ok((info.width, info.height, rgba))
}

/// A guess from the extension, for dropped files (the browser tells the web host the real type).
pub(crate) fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("txt" | "md") => "text/plain",
        _ => "",
    }
}

/// Largest dropped file the shell reads (the web host uses the same cap).
pub(crate) const MAX_DROPPED_BYTES: u64 = 256 * 1024 * 1024;

/// Read one dropped file. Runs on a worker thread.
pub(crate) fn read_dropped(path: &std::path::Path) -> Result<DroppedFile, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_DROPPED_BYTES {
        return Err(format!("{} is larger than {} MB", path.display(), MAX_DROPPED_BYTES / 1024 / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(DroppedFile { name, mime: mime_for_path(path).to_owned(), bytes })
}

/// egui's cursor icon as a winit one.
pub(crate) fn cursor_icon(icon: egui::CursorIcon) -> Option<winit::window::CursorIcon> {
    use egui::CursorIcon as E;
    use winit::window::CursorIcon as W;
    Some(match icon {
        E::None => return None,
        E::Default => W::Default,
        E::ContextMenu => W::ContextMenu,
        E::Help => W::Help,
        E::PointingHand => W::Pointer,
        E::Progress => W::Progress,
        E::Wait => W::Wait,
        E::Cell => W::Cell,
        E::Crosshair => W::Crosshair,
        E::Text => W::Text,
        E::VerticalText => W::VerticalText,
        E::Alias => W::Alias,
        E::Copy => W::Copy,
        E::Move => W::Move,
        E::NoDrop => W::NoDrop,
        E::NotAllowed => W::NotAllowed,
        E::Grab => W::Grab,
        E::Grabbing => W::Grabbing,
        E::AllScroll => W::AllScroll,
        E::ResizeHorizontal => W::EwResize,
        E::ResizeNeSw => W::NeswResize,
        E::ResizeNwSe => W::NwseResize,
        E::ResizeVertical => W::NsResize,
        E::ResizeEast => W::EResize,
        E::ResizeSouthEast => W::SeResize,
        E::ResizeSouth => W::SResize,
        E::ResizeSouthWest => W::SwResize,
        E::ResizeWest => W::WResize,
        E::ResizeNorthWest => W::NwResize,
        E::ResizeNorth => W::NResize,
        E::ResizeNorthEast => W::NeResize,
        E::ResizeColumn => W::ColResize,
        E::ResizeRow => W::RowResize,
        E::ZoomIn => W::ZoomIn,
        E::ZoomOut => W::ZoomOut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 1, 2, 3, 4];
        let png = encode_png(2, 2, &rgba).unwrap();
        let (w, h, back) = decode_png(&png).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(back, rgba);
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn mime_guess() {
        assert_eq!(mime_for_path(std::path::Path::new("a/B.PNG")), "image/png");
        assert_eq!(mime_for_path(std::path::Path::new("x")), "");
    }

    #[test]
    fn dropped_file_over_cap_is_refused_and_missing_file_errors() {
        assert!(read_dropped(std::path::Path::new("/definitely/not/here.png")).is_err());
    }
}
