//! Cropping an image: the rules behind `image.crop`, `image.reset_crop` and the crop editor.
//!
//! A crop is non-destructive. The picture stays whole in the document; `ImageData::crop` names the part of it
//! that is shown, in pixels of the stored picture, and the element's box is where that part is drawn.
//!
//! The one question this module answers is how a crop and the box go together. [`Picture`] describes where the
//! whole picture lies in the element's local frame (origin at the top-left of the unrotated box, mirror flags not
//! applied, as `Transform::local_to_world`), at the scale the element shows it at. A crop is then a window onto
//! that picture:
//!
//! * [`BoxRule::FollowCrop`] (the default) keeps the scale and the position of the picture on the canvas and
//!   makes the box the new window: cropping shrinks the box around what stays visible, and uncropping grows it
//!   again, as Excalidraw does.
//! * [`BoxRule::Keep`] leaves the box alone and stretches the cropped part over it.
//! * [`BoxRule::Exactly`] puts the box where the caller says (the crop editor uses it to pan the picture under
//!   a window that does not move).
//!
//! Rotation and mirroring are handled in the local frame, so a rotated or mirrored image crops the same way as
//! an upright one.

use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_scene::{Crop, Element, ElementId, ElementKind, SceneRead, SceneTx, StoreError};

use crate::target::TargetError;

/// The smallest crop side, in pixels of the picture (a picture smaller than this can only be cropped to itself).
pub const MIN_CROP_PX: f64 = 1.0;

/// Slack for rounding when a crop is compared with the picture's edges, in pixels.
const EPS: f64 = 1.0e-6;

/// Why an image could not be cropped. The text says how to proceed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CropError {
    /// The element is missing, deleted or locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The element is not an image.
    #[error("Only images can be cropped, and {id} is a {kind}. Select an image.")]
    NotAnImage {
        /// The element.
        id: ElementId,
        /// Its kind, in words.
        kind: &'static str,
    },
    /// The picture's bytes are not in the document (yet), so its pixels cannot be named.
    #[error("The picture of {id} has not arrived yet, so it cannot be cropped. Wait for it to load, then try again.")]
    NotLoaded {
        /// The element.
        id: ElementId,
    },
    /// The image has no usable size (a box or a picture of zero width or height).
    #[error("{id} has no size to crop. Give it a width and height first.")]
    NoSize {
        /// The element.
        id: ElementId,
    },
    /// A number of the crop is not finite.
    #[error("The crop has a number that is not finite. Give x, y, w and h as plain numbers.")]
    NotFinite,
    /// The crop is thinner than one pixel on a side.
    #[error("The crop is {w} by {h} pixels, and a side must be at least {min} pixel. Make it larger.")]
    TooSmall {
        /// Width asked for.
        w: f64,
        /// Height asked for.
        h: f64,
        /// The smallest side allowed.
        min: f64,
    },
    /// The crop reaches past the picture.
    #[error(
        "The crop (x {x}, y {y}, {w} by {h}) is not inside the picture, which is {pw} by {ph} pixels. Keep x and y at 0 or more, and x + w and y + h within the picture."
    )]
    OutsidePicture {
        /// Left edge asked for.
        x: f64,
        /// Top edge asked for.
        y: f64,
        /// Width asked for.
        w: f64,
        /// Height asked for.
        h: f64,
        /// Width of the picture.
        pw: u32,
        /// Height of the picture.
        ph: u32,
    },
    /// The box asked for has no size or is not finite.
    #[error("The box for the cropped image must have a width and height above 0. Check `bounds`.")]
    BadBounds,
    /// The store refused the write.
    #[error("{0}")]
    Store(String),
}

impl From<StoreError> for CropError {
    fn from(e: StoreError) -> Self {
        Self::Store(e.to_string())
    }
}

/// Where the element's box goes when the crop changes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum BoxRule {
    /// The picture keeps its scale and its place on the canvas; the box becomes the cropped part.
    #[default]
    FollowCrop,
    /// The box stays as it is; the cropped part is stretched over it.
    Keep,
    /// The box is exactly this rectangle, in world units (the unrotated box, turning about its own centre).
    Exactly(Rect),
}

/// The whole picture of an image element, as it lies in the element's local frame.
///
/// The frame has its origin at the top-left of the unrotated box and no mirroring. The picture is mirrored inside
/// it when the element is flipped, which is what [`Picture::window_of`] and [`Picture::source_of`] account for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picture {
    natural: Size,
    natural_px: (u32, u32),
    whole: Rect,
    flip_x: bool,
    flip_y: bool,
}

impl Picture {
    /// The picture of an image element as it is shown now (its crop and box applied).
    pub fn of(e: &Element) -> Result<Self, CropError> {
        let ElementKind::Image(d) = &e.kind else {
            return Err(CropError::NotAnImage { id: e.id.clone(), kind: e.tag().name() });
        };
        let natural = Size::new(f64::from(d.natural_width), f64::from(d.natural_height));
        let t = &e.transform;
        let shown = d.crop.as_ref().map_or_else(|| Rect::from_origin_size(Point::ZERO, natural), Crop::rect);
        let usable = [natural.width, natural.height, shown.width(), shown.height(), t.w, t.h]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0);
        if !usable {
            return Err(CropError::NoSize { id: e.id.clone() });
        }
        // Local units per picture pixel.
        let (kx, ky) = (t.w / shown.width(), t.h / shown.height());
        // Without a flip the shown part starts at the left edge of the box; mirrored, it ends there.
        let (x0, x1) = if t.flip_x {
            (t.w - (natural.width - shown.x0) * kx, t.w + shown.x0 * kx)
        } else {
            (-shown.x0 * kx, (natural.width - shown.x0) * kx)
        };
        let (y0, y1) = if t.flip_y {
            (t.h - (natural.height - shown.y0) * ky, t.h + shown.y0 * ky)
        } else {
            (-shown.y0 * ky, (natural.height - shown.y0) * ky)
        };
        Ok(Self {
            natural,
            natural_px: (d.natural_width, d.natural_height),
            whole: Rect::new(x0, y0, x1, y1),
            flip_x: t.flip_x,
            flip_y: t.flip_y,
        })
    }

    /// Size of the stored picture in pixels.
    pub fn natural(&self) -> Size {
        self.natural
    }

    /// The whole picture in the local frame.
    pub fn whole(&self) -> Rect {
        self.whole
    }

    /// Local units per picture pixel, across and down.
    pub fn scale(&self) -> (f64, f64) {
        (self.whole.width() / self.natural.width, self.whole.height() / self.natural.height)
    }

    /// The same picture moved by `(dx, dy)` in local units.
    #[must_use]
    pub fn shifted(self, dx: f64, dy: f64) -> Self {
        Self { whole: self.whole + drafftink_geom::kurbo::Vec2::new(dx, dy), ..self }
    }

    /// The window (local frame) that shows `source` (picture pixels).
    pub fn window_of(&self, source: Rect) -> Rect {
        let (kx, ky) = self.scale();
        let w = self.whole;
        let (x0, x1) = if self.flip_x {
            (w.x1 - source.x1 * kx, w.x1 - source.x0 * kx)
        } else {
            (w.x0 + source.x0 * kx, w.x0 + source.x1 * kx)
        };
        let (y0, y1) = if self.flip_y {
            (w.y1 - source.y1 * ky, w.y1 - source.y0 * ky)
        } else {
            (w.y0 + source.y0 * ky, w.y0 + source.y1 * ky)
        };
        Rect::new(x0, y0, x1, y1)
    }

    /// The part of the picture (pixels) that a window (local frame) shows. The inverse of [`Picture::window_of`].
    pub fn source_of(&self, window: Rect) -> Rect {
        let (kx, ky) = self.scale();
        let w = self.whole;
        let (x0, x1) = if self.flip_x {
            ((w.x1 - window.x1) / kx, (w.x1 - window.x0) / kx)
        } else {
            ((window.x0 - w.x0) / kx, (window.x1 - w.x0) / kx)
        };
        let (y0, y1) = if self.flip_y {
            ((w.y1 - window.y1) / ky, (w.y1 - window.y0) / ky)
        } else {
            ((window.y0 - w.y0) / ky, (window.y1 - w.y0) / ky)
        };
        Rect::new(x0, y0, x1, y1)
    }

    /// The crop that shows `window` (local frame): the part of the picture it covers, clamped to the picture, or
    /// `None` when that is all of it. What the crop editor stores when it commits.
    pub fn crop_of(&self, window: Rect) -> Option<Crop> {
        self.normalise(self.source_of(window)).map(Crop::from_rect)
    }

    /// Clamps rounding noise at the picture's edges. Whole picture: `None`.
    fn normalise(&self, source: Rect) -> Option<Rect> {
        let (pw, ph) = (self.natural.width, self.natural.height);
        let clamped = Rect::new(source.x0.max(0.0), source.y0.max(0.0), source.x1.min(pw), source.y1.min(ph));
        let whole = clamped.x0 <= EPS && clamped.y0 <= EPS && clamped.x1 >= pw - EPS && clamped.y1 >= ph - EPS;
        (!whole).then_some(clamped)
    }

    /// Checks a crop against the picture and snaps rounding noise at its edges. Whole picture: `None`.
    fn check(&self, source: Rect) -> Result<Option<Rect>, CropError> {
        let (pw, ph) = (self.natural.width, self.natural.height);
        let numbers = [source.x0, source.y0, source.x1, source.y1];
        if !numbers.iter().all(|v| v.is_finite()) {
            return Err(CropError::NotFinite);
        }
        let (w, h) = (source.width(), source.height());
        let min_w = MIN_CROP_PX.min(pw);
        let min_h = MIN_CROP_PX.min(ph);
        if w < min_w - EPS || h < min_h - EPS {
            return Err(CropError::TooSmall { w, h, min: MIN_CROP_PX });
        }
        let inside = source.x0 >= -EPS && source.y0 >= -EPS && source.x1 <= pw + EPS && source.y1 <= ph + EPS;
        if !inside {
            return Err(CropError::OutsidePicture {
                x: source.x0,
                y: source.y0,
                w,
                h,
                pw: self.natural_px.0,
                ph: self.natural_px.1,
            });
        }
        Ok(self.normalise(source))
    }
}

/// What a crop did.
#[derive(Debug, Clone, PartialEq)]
pub struct Cropped {
    /// The crop now stored (`None`: the whole picture is shown).
    pub crop: Option<Crop>,
    /// The element's unrotated box in world units after the change.
    pub bounds: Rect,
    /// False when the image already looked like this and nothing was written.
    pub changed: bool,
}

/// Crops the image `id` to `source` (picture pixels; `None` shows the whole picture) and places its box by `rule`.
///
/// The image must be live, unlocked, and its picture must be in the document. Everything is checked before the
/// first write. A crop that covers the whole picture is stored as no crop.
pub fn crop_image<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    source: Option<Rect>,
    rule: BoxRule,
) -> Result<Cropped, CropError> {
    let e = tx.get(id).filter(|e| !e.deleted).ok_or_else(|| TargetError::NotFound(id.clone()))?;
    let ElementKind::Image(data) = &e.kind else {
        return Err(CropError::NotAnImage { id: id.clone(), kind: e.tag().name() });
    };
    if e.locked {
        return Err(TargetError::Locked(vec![id.clone()]).into());
    }
    if !data.file_id.as_ref().is_some_and(|f| tx.file(f).is_some()) {
        return Err(CropError::NotLoaded { id: id.clone() });
    }
    let picture = Picture::of(e)?;
    let crop = match source {
        Some(s) => picture.check(s)?.map(Crop::from_rect),
        None => None,
    };
    let shown = crop.as_ref().map_or_else(|| Rect::from_origin_size(Point::ZERO, picture.natural()), Crop::rect);
    let t = &e.transform;
    let bounds = match rule {
        BoxRule::Keep => t.rect(),
        BoxRule::FollowCrop => {
            let window = picture.window_of(shown);
            Rect::from_center_size(t.local_to_world(window.center()), window.size())
        }
        BoxRule::Exactly(b) => {
            let sane = [b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite()) && b.width() > 0.0 && b.height() > 0.0;
            if !sane {
                return Err(CropError::BadBounds);
            }
            b
        }
    };
    let unchanged = data.crop == crop && t.rect() == bounds;
    if unchanged {
        return Ok(Cropped { crop, bounds, changed: false });
    }
    tx.update(id, |e| {
        e.transform.x = bounds.x0;
        e.transform.y = bounds.y0;
        e.transform.w = bounds.width();
        e.transform.h = bounds.height();
        if let ElementKind::Image(d) = &mut e.kind {
            d.crop = crop.clone();
        }
    })?;
    Ok(Cropped { crop, bounds, changed: true })
}

/// Whether an element is an unlocked image whose picture is in the scene: what a crop needs. Used to offer the
/// crop commands; [`crop_image`] checks again.
pub fn can_crop<R: SceneRead>(scene: &R, e: &Element) -> bool {
    !e.deleted
        && !e.locked
        && e.image_data().is_some_and(|d| d.file_id.as_ref().is_some_and(|f| scene.file(f).is_some()))
}

#[cfg(test)]
#[path = "crop_tests.rs"]
mod tests;
