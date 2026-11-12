//! Pictures: adding an image from the bytes of a PNG, JPEG or WebP file.
//!
//! `drafftink-io` identifies the picture by its first bytes (never by a file name or a media type the page
//! claims), reads its size from the header and decodes it once, so a damaged file is refused now and not at every
//! frame. The bytes go into the scene's file table (one copy per content) and the new `image` shape gets the
//! natural size of the picture as its default box.

use drafftink_io::picture::{self, PictureError};
use drafftink_scene::{Element, ElementId, ElementKind, FileEntry, ImageData, SceneTx, Style, Transform};
use serde::Deserialize;

use crate::error::ApiError;
use crate::ops::OpCx;

/// Where and how big to put a picture (`stage.addImage(bytes, options)`). Every field is optional: without any,
/// the picture is at the origin in its natural size.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ImageSpec {
    /// Id of the new shape. Default: a fresh one.
    pub id: Option<String>,
    /// Left edge in world units. Default 0.
    pub x: Option<f64>,
    /// Top edge in world units. Default 0.
    pub y: Option<f64>,
    /// Width in world units. With only a width, the height follows the picture's proportions.
    pub width: Option<f64>,
    /// Height in world units. With only a height, the width follows the picture's proportions.
    pub height: Option<f64>,
    /// The longer side is at most this, in world units: a bigger picture is scaled down, a smaller one is left.
    pub max_side: Option<f64>,
    /// A name for the shape (shown by search and selectors, `#name`).
    pub name: Option<String>,
}

/// A picture that passed every check, ready to go into a transaction.
pub(crate) struct Prepared {
    entry: FileEntry,
    file_id: drafftink_scene::FileId,
    natural: (u32, u32),
    at: (f64, f64),
    size: (f64, f64),
    id: Option<ElementId>,
    name: Option<String>,
}

fn positive(what: &str, v: f64) -> Result<f64, ApiError> {
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(ApiError::invalid(format!("{what} must be a number above 0"))) }
}

fn picture_error(e: &PictureError) -> ApiError {
    ApiError::invalid(format!("{e}. Use a PNG, JPEG or WebP picture"))
}

/// The size a picture of `natural` pixels gets on the canvas.
fn shown_size(natural: (u32, u32), spec: &ImageSpec) -> Result<(f64, f64), ApiError> {
    let (nw, nh) = (f64::from(natural.0), f64::from(natural.1));
    let (w, h) = match (spec.width, spec.height) {
        (Some(w), Some(h)) => (positive("width", w)?, positive("height", h)?),
        (Some(w), None) => {
            let w = positive("width", w)?;
            (w, w * nh / nw)
        }
        (None, Some(h)) => {
            let h = positive("height", h)?;
            (h * nw / nh, h)
        }
        (None, None) => (nw, nh),
    };
    match spec.max_side {
        Some(limit) => {
            let limit = positive("maxSide", limit)?;
            let k = (limit / w.max(h)).min(1.0);
            Ok((w * k, h * k))
        }
        None => Ok((w, h)),
    }
}

/// Checks the bytes and the options.
pub(crate) fn prepare(bytes: &[u8], spec: &ImageSpec) -> Result<Prepared, ApiError> {
    let probe = picture::probe(bytes).map_err(|e| picture_error(&e))?;
    picture::validate_pixels(bytes, &probe).map_err(|e| picture_error(&e))?;
    let natural = (probe.width, probe.height);
    let at = (spec.x.unwrap_or(0.0), spec.y.unwrap_or(0.0));
    if !(at.0.is_finite() && at.1.is_finite()) {
        return Err(ApiError::invalid("x and y must be finite numbers"));
    }
    let size = shown_size(natural, spec)?;
    let id = spec.id.as_deref().map(ElementId::new).transpose()?;
    let (file_id, entry) = FileEntry::new(probe.mime, bytes);
    Ok(Prepared { entry, file_id, natural, at, size, id, name: spec.name.clone() })
}

/// Stores the picture and adds the shape on top of everything.
pub(crate) fn insert<T: SceneTx>(tx: &mut T, cx: &mut OpCx<'_>, picture: Prepared) -> Result<ElementId, ApiError> {
    let Prepared { entry, file_id, natural, at, size, id, name } = picture;
    let id = id.unwrap_or_else(|| cx.ids.element(&*tx));
    tx.put_file(entry)?;
    let z = tx.next_top_index().map_err(|e| ApiError::invalid(e.to_string()))?;
    let kind = ElementKind::Image(ImageData {
        file_id: Some(file_id),
        natural_width: natural.0,
        natural_height: natural.1,
        crop: None,
    });
    let mut element = Element::new(id.clone(), kind, z);
    element.transform = Transform::new(at.0, at.1, size.0, size.1);
    element.style = Style::for_image(cx.ids.seed());
    element.name = name;
    tx.insert(element)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_follows_the_picture_unless_the_options_say_otherwise() {
        let none = ImageSpec::default();
        assert_eq!(shown_size((400, 200), &none).unwrap(), (400.0, 200.0));
        let width = ImageSpec { width: Some(100.0), ..ImageSpec::default() };
        assert_eq!(shown_size((400, 200), &width).unwrap(), (100.0, 50.0));
        let height = ImageSpec { height: Some(100.0), ..ImageSpec::default() };
        assert_eq!(shown_size((400, 200), &height).unwrap(), (200.0, 100.0));
        let both = ImageSpec { width: Some(30.0), height: Some(70.0), ..ImageSpec::default() };
        assert_eq!(shown_size((400, 200), &both).unwrap(), (30.0, 70.0));
        let capped = ImageSpec { max_side: Some(100.0), ..ImageSpec::default() };
        assert_eq!(shown_size((400, 200), &capped).unwrap(), (100.0, 50.0));
        assert_eq!(shown_size((40, 20), &capped).unwrap(), (40.0, 20.0), "a small picture is not scaled up");
    }

    #[test]
    fn bad_numbers_are_refused_in_words() {
        for spec in [
            ImageSpec { width: Some(0.0), ..ImageSpec::default() },
            ImageSpec { height: Some(f64::NAN), ..ImageSpec::default() },
            ImageSpec { max_side: Some(-1.0), ..ImageSpec::default() },
        ] {
            let err = shown_size((10, 10), &spec).unwrap_err().to_string();
            assert!(err.contains("above 0"), "{err}");
        }
    }
}
