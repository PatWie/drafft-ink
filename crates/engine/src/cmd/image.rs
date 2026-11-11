//! `image`: insert pictures (docs/commands.md §2.13).
//!
//! `image.insert` replaces the three paths of the old app (paste, drop and the picker). The rules for pictures
//! (format sniffing, the size from the header, the pixel cap, the re-encoding of big pictures) are in
//! `drafftink_io::picture`; the rules for the elements are here because they are only sizes and places:
//!
//! * A picture whose longest side is over `fit` is scaled down to `fit`; a smaller one keeps its pixel size (the
//!   old 800 rule). The element is centred on `at`; each further picture moves by `offset` in x and in y.
//! * The bytes go into the document's file table by content hash. The same picture inserted twice is stored once.
//! * Everything is checked before the first change, so a bad third picture leaves the document alone.

use drafftink_geom::kurbo::Point;
use drafftink_io::picture::{self, MAX_PICTURE_BYTES, PictureError};
use drafftink_scene::{
    Element, ElementId, ElementKind, FileEntry, ImageData, SceneRead, SceneTx, StoreError, Style, Transform,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::bytes::{Bytes, BytesError, Loaded};
use super::clipboard::common::{At, edit_with, resolve_at, select_only};
use crate::args;
use crate::refusal::{Refusal, echo};
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// Most pictures one call inserts.
const MAX_IMAGES: usize = 20;
/// The longest side a picture is shown at, unless the call says otherwise.
pub(crate) const DEFAULT_FIT: f64 = 800.0;
/// The shift of each further picture.
pub(crate) const DEFAULT_OFFSET: f64 = 20.0;

/// Reads the bytes of one picture, refusing one over the picture limit before it is decoded.
pub(crate) fn load_picture(s: &Session, bytes: &Bytes, fallback: &str) -> Result<Loaded, Refusal> {
    bytes.load(s, fallback, MAX_PICTURE_BYTES).map_err(|e| bytes_refusal(s.cmd(), e))
}

/// A picture refused while its bytes were read.
fn bytes_refusal(cmd: &str, e: BytesError) -> Refusal {
    match e {
        BytesError::Base64 { name, source } => Refusal::custom(
            "parse_failed",
            cmd,
            format!("{} could not be read as an image: its base64 text is damaged ({source}). Try another file.", echo(&name)),
        ),
        BytesError::TooLarge { size } => Refusal::limit_exceeded(cmd, size, "bytes in one image", MAX_PICTURE_BYTES),
        BytesError::NoFiles => Refusal::port_unavailable(
            cmd,
            "file access",
            "this session cannot read files",
            "Pass the image as `base64` instead of `path`.",
        ),
        BytesError::Read { path, message } => Refusal::io_failed(cmd, "read", &path, &message),
    }
}

/// How pictures are placed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    pub at: Point,
    pub fit: f64,
    pub offset: f64,
    pub select: bool,
    pub downscale: bool,
}

/// One inserted picture.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Inserted {
    /// The element.
    pub id: String,
    /// The content hash under which the bytes are stored in the document.
    pub file_id: String,
    /// Width of the element in world units.
    pub width: f64,
    /// Height of the element in world units.
    pub height: f64,
    /// Width of the stored picture in pixels.
    pub natural_width: u32,
    /// Height of the stored picture in pixels.
    pub natural_height: u32,
    /// True when the document already held these bytes, so nothing new was stored.
    pub reused: bool,
    /// True when the picture was big and was stored smaller.
    pub shrunk: bool,
}

/// A picture that passed the checks and is ready to become an element.
struct Ready {
    entry: FileEntry,
    file_id: drafftink_scene::FileId,
    natural: (u32, u32),
    show: (f64, f64),
    shrunk: bool,
}

fn picture_refusal(cmd: &str, name: &str, e: &PictureError) -> Refusal {
    match e {
        PictureError::Unsupported { found } => Refusal::custom(
            "unsupported_format",
            cmd,
            format!("`{}` is not a supported image ({found}). Use PNG, JPEG or WebP.", echo(name)),
        ),
        PictureError::Unreadable(msg) => Refusal::custom(
            "parse_failed",
            cmd,
            format!("{} could not be read as an image: {}. Try another file.", echo(name), echo(msg)),
        ),
        PictureError::TooLarge { size } => Refusal::limit_exceeded(cmd, *size, "bytes in one image", MAX_PICTURE_BYTES),
        PictureError::TooManyPixels { .. } => Refusal::custom(
            "limit_exceeded",
            cmd,
            format!("{} is too large: {e}. Make it smaller and try again.", echo(name)),
        ),
    }
}

/// Inserts pictures. Used by `image.insert` and by `clipboard.paste`. Validates everything first.
pub(crate) fn insert_pictures(
    s: &mut Session,
    pics: Vec<Loaded>,
    place: &Placement,
) -> Result<Vec<Inserted>, Refusal> {
    let cmd = s.cmd();
    let mut ready: Vec<Ready> = Vec::with_capacity(pics.len());
    for p in pics {
        let probe = picture::probe(&p.bytes).map_err(|e| picture_refusal(cmd, &p.name, &e))?;
        let shrunk = if place.downscale {
            picture::shrink(&p.bytes, &probe).map_err(|e| picture_refusal(cmd, &p.name, &e))?
        } else {
            None
        };
        let (mime, natural, bytes) = match shrunk {
            Some(sh) => (sh.mime, (sh.width, sh.height), sh.bytes),
            None => (probe.mime, (probe.width, probe.height), p.bytes),
        };
        // The size on the canvas follows the picture as it was given, not the smaller copy we store.
        let (w, h) = (f64::from(probe.width), f64::from(probe.height));
        let k = if w.max(h) > place.fit { place.fit / w.max(h) } else { 1.0 };
        let (file_id, entry) = FileEntry::new(mime, bytes);
        ready.push(Ready { entry, file_id, natural, show: (w * k, h * k), shrunk: shrunk_flag(&probe, natural) });
    }
    // A picture is reused when the document has the bytes already, or an earlier picture of this call has them.
    let mut known: Vec<bool> = Vec::with_capacity(ready.len());
    for (i, r) in ready.iter().enumerate() {
        known.push(s.store().file(&r.file_id).is_some() || ready.iter().take(i).any(|q| q.file_id == r.file_id));
    }

    let (done, ids) = edit_with::<_, StoreError>(s, |tx, ids, _| {
        let mut out = Vec::with_capacity(ready.len());
        let mut created: Vec<ElementId> = Vec::with_capacity(ready.len());
        for (i, r) in ready.iter().enumerate() {
            tx.put_file(r.entry.clone())?;
            let id = drafftink_edit::unused_element_id(&*tx, ids);
            let z = tx.next_top_index().map_err(|e| StoreError::Backend(e.to_string()))?;
            let shift = place.offset * i as f64;
            let mut e = Element::new(
                id.clone(),
                ElementKind::Image(ImageData {
                    file_id: Some(r.file_id.clone()),
                    natural_width: r.natural.0,
                    natural_height: r.natural.1,
                    crop: None,
                }),
                z,
            );
            e.transform = Transform::new(
                place.at.x + shift - r.show.0 / 2.0,
                place.at.y + shift - r.show.1 / 2.0,
                r.show.0,
                r.show.1,
            );
            e.style = Style { seed: ids.seed(), ..Style::default() };
            tx.insert(e)?;
            created.push(id.clone());
            out.push(id);
        }
        Ok((out, created))
    })
    .map_err(|e| Refusal::internal(cmd, e.to_string()))?;

    if place.select {
        select_only(s, &ids);
    }
    Ok(done
        .iter()
        .zip(&ready)
        .zip(known)
        .map(|((id, r), reused)| Inserted {
            id: id.as_str().to_owned(),
            file_id: r.file_id.as_str().to_owned(),
            width: r.show.0,
            height: r.show.1,
            natural_width: r.natural.0,
            natural_height: r.natural.1,
            reused,
            shrunk: r.shrunk,
        })
        .collect())
}

fn shrunk_flag(probe: &picture::Probe, natural: (u32, u32)) -> bool {
    (probe.width, probe.height) != natural
}

// ---- image.insert ----

fn default_fit() -> f64 {
    DEFAULT_FIT
}
fn default_offset() -> f64 {
    DEFAULT_OFFSET
}
fn yes() -> bool {
    true
}

/// Arguments of `image.insert`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertArgs {
    /// The pictures: 1 to 20, each `{base64, name?, mime?}` or `{path}`. PNG, JPEG or WebP, at most 25 MiB each.
    pub images: Vec<Bytes>,
    /// Where the middle of the first picture goes, in world units. Default: the middle of the visible canvas.
    #[serde(default)]
    pub at: At,
    /// The longest side a picture is shown at. A larger picture is scaled down to this; a smaller one keeps its
    /// size.
    #[serde(default = "default_fit")]
    pub fit: f64,
    /// Each further picture is moved by this many units right and down, so a batch does not stack.
    #[serde(default = "default_offset")]
    pub offset: f64,
    /// Select the new elements.
    #[serde(default = "yes")]
    pub select: bool,
    /// Store a picture over 2 MiB and 2048 px on the long side scaled down to 2048 px.
    #[serde(default = "yes")]
    pub downscale: bool,
    /// A name for messages about pictures that carry no `name` of their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Output of `image.insert`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InsertOut {
    /// The new elements, in order.
    pub ids: Vec<String>,
    /// One entry per picture.
    pub images: Vec<Inserted>,
}

fn insert(s: &mut Session, a: InsertArgs) -> Result<InsertOut, Refusal> {
    let cmd = s.cmd();
    if a.images.is_empty() {
        return Err(Refusal::bad_arg(cmd, "images", "needs at least one picture"));
    }
    if a.images.len() > MAX_IMAGES {
        return Err(Refusal::limit_exceeded(cmd, a.images.len(), "images", MAX_IMAGES));
    }
    let fit = args::positive(cmd, "fit", a.fit, 100_000.0)?;
    let offset = args::world(cmd, "offset", a.offset)?;
    let at = resolve_at(s, &a.at)?;
    let mut loaded = Vec::with_capacity(a.images.len());
    for (i, b) in a.images.iter().enumerate() {
        let fallback = match (&a.name, a.images.len()) {
            (Some(n), 1) if !n.is_empty() => n.clone(),
            (Some(n), _) if !n.is_empty() => format!("{n} {}", i + 1),
            _ => format!("image {}", i + 1),
        };
        loaded.push(load_picture(s, b, &fallback)?);
    }
    let images = insert_pictures(s, loaded, &Placement { at, fit, offset, select: a.select, downscale: a.downscale })?;
    Ok(InsertOut { ids: images.iter().map(|i| i.id.clone()).collect(), images })
}

// ---- registration ----

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
};

/// The image family.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "image.insert",
    "image",
    "Insert image",
    "Inserts PNG, JPEG or WebP pictures as image elements, scaled to fit, stored once per content in the document.",
    &Typed::new(insert),
    DOC,
)
.refusals(&["unsupported_format"])
.example(r#"{"images":[{"base64":"iVBORw0KGgo=","name":"logo.png"}],"at":[0,0]}"#)];

#[cfg(test)]
#[path = "image_tests.rs"]
pub(crate) mod tests;
