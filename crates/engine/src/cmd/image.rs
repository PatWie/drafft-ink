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
//!
//! The same family crops pictures without changing them (`image.crop`, `image.reset_crop`) and opens and closes
//! the crop editor (`image.crop_begin`, `image.crop_end`). The rules for crops are in `drafftink_edit::crop`; the
//! pointer side of the editor is `interact::crop`.

use drafftink_edit::TargetError;
use drafftink_edit::crop::{self, BoxRule, CropError, Cropped};
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_io::picture::{self, MAX_PICTURE_BYTES, PictureError};
use drafftink_scene::{
    Element, ElementId, ElementKind, FileEntry, ImageData, SceneRead, SceneTx, StoreError, Style, Transform,
};
use serde_json::json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::bytes::{Bytes, BytesError, Loaded};
use super::clipboard::common::{At, edit_with, resolve_at, select_only};
use crate::args;
use super::element_common::target_refusal;
use super::view::RectArg;
use crate::refusal::{Refusal, echo};
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};
use crate::state::Tool;

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
        if shrunk.is_none() {
            // Only the header was read so far: damaged pixel data must fail here and not at every draw.
            picture::validate_pixels(&p.bytes, &probe).map_err(|e| picture_refusal(cmd, &p.name, &e))?;
        }
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
            let z = tx.next_top_index().map_err(StoreError::backend)?;
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
            e.style = Style::for_image(ids.seed());
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

// ---- cropping ----

/// What a crop does to the element's box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ElementBox {
    /// The picture keeps its scale and its place on the canvas, and the box becomes the cropped part: cropping
    /// shrinks the box, uncropping grows it (Excalidraw's rule).
    #[default]
    FollowCrop,
    /// The box keeps its size and place, and the cropped part is stretched over it.
    Keep,
}

impl From<ElementBox> for BoxRule {
    fn from(b: ElementBox) -> Self {
        match b {
            ElementBox::FollowCrop => BoxRule::FollowCrop,
            ElementBox::Keep => BoxRule::Keep,
        }
    }
}

/// Arguments of `image.crop`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CropArgs {
    /// The image: an id or `@name`. Default: the single selected element.
    pub id: Option<Ref>,
    /// The part of the picture to show, in pixels of the stored picture: `{x, y, w, h}` from the top-left corner
    /// (the size of the picture is `natural_width` by `natural_height`, see `element.query`). `null` or absent
    /// shows the whole picture. A crop that covers the whole picture is the same as `null`.
    pub crop: Option<RectArg>,
    /// What happens to the box: `follow_crop` (default) shrinks or grows it around the cropped part so the pixels
    /// keep their scale and place; `keep` leaves it and stretches the crop over it.
    pub element_box: ElementBox,
    /// Puts the box exactly here (world units, the unrotated box) and overrides `element_box`. The crop editor
    /// uses it to slide the picture under a window that stays where it is.
    pub bounds: Option<RectArg>,
}

/// Result of `image.crop` and `image.reset_crop`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CropOut {
    /// The image.
    pub id: String,
    /// The crop now shown, in pixels of the picture; null when the whole picture is shown.
    pub crop: Option<RectArg>,
    /// The unrotated box of the image in world units after the change.
    pub bounds: RectArg,
    /// Width of the stored picture in pixels.
    pub natural_width: u32,
    /// Height of the stored picture in pixels.
    pub natural_height: u32,
    /// False when the image already looked like this and nothing was written.
    pub changed: bool,
}

/// The refusal code for a crop that is too small, outside the picture or not a number.
const BAD_CROP: &str = "bad_crop";
/// The refusal code for a picture whose bytes are not in the document.
const PICTURE_NOT_LOADED: &str = "picture_not_loaded";

fn crop_refusal(cmd: &str, e: CropError) -> Refusal {
    match e {
        CropError::Target(t) => target_refusal(cmd, t),
        CropError::NotAnImage { id, kind } => {
            Refusal::wrong_kind(cmd, kind, "images").with_details(json!({ "ids": [id.as_str()] }))
        }
        CropError::NotLoaded { .. } => Refusal::custom(PICTURE_NOT_LOADED, cmd, e.to_string()),
        CropError::BadBounds => Refusal::bad_arg(cmd, "bounds", "needs a width and height above 0"),
        CropError::Store(m) => Refusal::internal(cmd, m),
        CropError::NoSize { .. } | CropError::NotFinite | CropError::TooSmall { .. } | CropError::OutsidePicture { .. } => {
            Refusal::custom(BAD_CROP, cmd, e.to_string())
        }
    }
}

/// The one image a command acts on: the given reference, else the single selected element.
fn one_image(s: &Session, id: Option<&Ref>) -> Result<ElementId, Refusal> {
    if let Some(r) = id {
        return s.resolve_ref(r);
    }
    let live: Vec<ElementId> = s.selection().iter().filter(|i| s.element(i).is_some()).cloned().collect();
    match live.as_slice() {
        [one] => Ok(one.clone()),
        other => Err(Refusal::needs_single(s.cmd(), "image", other.len())),
    }
}

fn rect_arg(r: Rect) -> RectArg {
    RectArg { x: r.x0, y: r.y0, w: r.width(), h: r.height() }
}

fn world_rect(cmd: &str, key: &str, r: RectArg) -> Result<Rect, Refusal> {
    let x = args::world(cmd, &format!("{key}.x"), r.x)?;
    let y = args::world(cmd, &format!("{key}.y"), r.y)?;
    let w = args::world(cmd, &format!("{key}.w"), r.w)?;
    let h = args::world(cmd, &format!("{key}.h"), r.h)?;
    Ok(Rect::new(x, y, x + w, y + h))
}

/// Crops `id` and closes the crop editor when it was open on that image. Shared by `image.crop` and
/// `image.reset_crop`.
fn apply_crop(
    s: &mut Session,
    id: &ElementId,
    source: Option<Rect>,
    rule: BoxRule,
) -> Result<CropOut, Refusal> {
    let cmd = s.cmd();
    let (done, _) = s
        .tab_mut()
        .doc
        .edit::<Cropped, CropError>(|tx| crop::crop_image(tx, id, source, rule))
        .map_err(|e| crop_refusal(cmd, e))?;
    let obs = &mut s.tab_mut().observed;
    if obs.crop_editor.as_ref() == Some(id) {
        obs.crop_editor = None;
    }
    let (natural_width, natural_height) =
        s.element(id).and_then(|e| e.image_data()).map_or((0, 0), |d| (d.natural_width, d.natural_height));
    Ok(CropOut {
        id: id.as_str().to_owned(),
        crop: done.crop.as_ref().map(|c| rect_arg(c.rect())),
        bounds: rect_arg(done.bounds),
        natural_width,
        natural_height,
        changed: done.changed,
    })
}

fn crop_image(s: &mut Session, a: CropArgs) -> Result<CropOut, Refusal> {
    let cmd = s.cmd();
    let id = one_image(s, a.id.as_ref())?;
    let source = a.crop.map(|r| Rect::new(r.x, r.y, r.x + r.w, r.y + r.h));
    let rule = match a.bounds {
        Some(b) => BoxRule::Exactly(world_rect(cmd, "bounds", b)?),
        None => a.element_box.into(),
    };
    apply_crop(s, &id, source, rule)
}

/// Arguments of `image.reset_crop`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ResetCropArgs {
    /// The image: an id or `@name`. Default: the single selected element.
    pub id: Option<Ref>,
    /// What happens to the box, as for `image.crop`: `follow_crop` (default) grows it back to the whole picture
    /// at the scale it has, `keep` leaves it and shows the whole picture in it.
    pub element_box: ElementBox,
}

fn reset_crop(s: &mut Session, a: ResetCropArgs) -> Result<CropOut, Refusal> {
    let id = one_image(s, a.id.as_ref())?;
    apply_crop(s, &id, None, a.element_box.into())
}

/// `image.crop` and `image.reset_crop` are offered for one selected image whose picture is loaded.
fn single_croppable(s: &Session) -> Result<(), Refusal> {
    let cmd = s.cmd();
    let id = one_image(s, None)?;
    let e = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
    if e.image_data().is_none() {
        return Err(Refusal::wrong_kind(cmd, e.tag().name(), "images"));
    }
    if e.locked {
        return Err(target_refusal(cmd, TargetError::Locked(vec![id])));
    }
    if !crop::can_crop(s.store(), e) {
        return Err(Refusal::custom(
            PICTURE_NOT_LOADED,
            cmd,
            "The picture has not arrived yet, so it cannot be cropped. Wait for it to load, then try again.",
        ));
    }
    Ok(())
}

/// `image.reset_crop` is offered for one selected image that is cropped.
fn single_cropped(s: &Session) -> Result<(), Refusal> {
    single_croppable(s)?;
    let cropped = one_image(s, None)
        .ok()
        .and_then(|id| s.element(&id).and_then(|e| e.image_data()).map(|d| d.crop.is_some()))
        .unwrap_or(false);
    if cropped {
        Ok(())
    } else {
        Err(Refusal::nothing_to_do(s.cmd(), "reset", "the image is not cropped"))
    }
}

/// Arguments of `image.crop_begin`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CropBeginArgs {
    /// The image: an id or `@name`. Default: the single selected element. It becomes the selection.
    pub id: Option<Ref>,
}

/// Result of `image.crop_begin` and `image.crop_end`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CropEditorOut {
    /// The image whose crop is being edited now; null when the editor is closed.
    pub id: Option<String>,
    /// False when the editor was in this state already.
    pub changed: bool,
}

fn crop_begin(s: &mut Session, a: CropBeginArgs) -> Result<CropEditorOut, Refusal> {
    let cmd = s.cmd();
    let id = one_image(s, a.id.as_ref())?;
    let e = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
    if e.image_data().is_none() {
        return Err(Refusal::wrong_kind(cmd, e.tag().name(), "images"));
    }
    if e.locked {
        return Err(target_refusal(cmd, TargetError::Locked(vec![id])));
    }
    if !crop::can_crop(s.store(), e) {
        return Err(Refusal::custom(PICTURE_NOT_LOADED, cmd, "The picture has not arrived yet, so it cannot be cropped. Wait for it to load, then try again."));
    }
    let already = s.tab().observed.crop_editor.as_ref() == Some(&id);
    if already {
        return Ok(CropEditorOut { id: Some(id.as_str().to_owned()), changed: false });
    }
    // The editor is a mode of the select tool, and it belongs to one image: select that image alone.
    if s.tool.active != Tool::Select {
        s.run_nested("tool.set", json!({ "tool": "select" }))?;
    }
    s.run_nested("selection.select", json!({ "ids": [id.as_str()] }))?;
    s.tab_mut().observed.crop_editor = Some(id.clone());
    Ok(CropEditorOut { id: Some(id.as_str().to_owned()), changed: true })
}

/// Arguments of `image.crop_end`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CropEndArgs {}

fn crop_end(s: &mut Session, _: CropEndArgs) -> Result<CropEditorOut, Refusal> {
    let was_open = s.tab_mut().observed.crop_editor.take().is_some();
    Ok(CropEditorOut { id: None, changed: was_open })
}

fn crop_editor_open(s: &Session) -> Result<(), Refusal> {
    if s.tab().observed.crop_editor.is_some() {
        Ok(())
    } else {
        Err(Refusal::nothing_to_do(s.cmd(), "end", "the crop editor is not open. Open it with `image.crop_begin`"))
    }
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
    doc_write: crate::spec::DocWrite::Edit,
};

/// The editor commands change no document: they open and close a mode.
const EDITOR: Facts = Facts {
    undo: Undo::NoUndo,
    mutates: Mutates::OBSERVED,
    needs_selection: Selection::Soft,
    redraw: Redraw::Overlay,
    palette: true,
    doc_write: crate::spec::DocWrite::NotDocument,
    ..DOC
};

/// `image.crop` and `image.reset_crop` change the document, and close the crop editor of the image.
const CROP: Facts = Facts {
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::Soft,
    palette: true,
    ..DOC
};

/// The image family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "image.insert",
        "image",
        "Insert image",
        "Inserts PNG, JPEG or WebP pictures as image elements, scaled to fit, stored once per content in the document.",
        &Typed::new(insert),
        DOC,
    )
    .refusals(&["unsupported_format"])
    .example(r#"{"images":[{"base64":"iVBORw0KGgo=","name":"logo.png"}],"at":[0,0]}"#),
    CommandSpec::new(
        "image.crop",
        "image",
        "Crop",
        "Shows only a part of an image, given in pixels of the picture. The picture is kept whole, so the crop can be changed or reset later.",
        &Typed::new(crop_image),
        CROP,
    )
    .default_targets(single_croppable)
    .refusals(&[BAD_CROP, PICTURE_NOT_LOADED])
    .example(r#"{"id":"e1","crop":{"x":40,"y":20,"w":80,"h":40}}"#),
    CommandSpec::new(
        "image.reset_crop",
        "image",
        "Reset crop",
        "Shows the whole picture of a cropped image again.",
        &Typed::new(reset_crop),
        CROP,
    )
    .default_targets(single_cropped)
    .refusals(&[BAD_CROP, PICTURE_NOT_LOADED])
    .example(r#"{"id":"e1"}"#),
    CommandSpec::new(
        "image.crop_begin",
        "image",
        "Crop image",
        "Opens the crop editor on an image: the whole picture is shown faded, with handles on the crop.",
        &Typed::new(crop_begin),
        Facts { mutates: Mutates::OBSERVED.with(Mutates::TOOL), ..EDITOR },
    )
    .default_targets(single_croppable)
    .refusals(&[PICTURE_NOT_LOADED])
    .example(r#"{"id":"e1"}"#),
    CommandSpec::new(
        "image.crop_end",
        "image",
        "Done cropping",
        "Closes the crop editor and keeps the crop as it is in the document.",
        &Typed::new(crop_end),
        Facts { needs_selection: Selection::No, agent: false, ..EDITOR },
    )
    .available(crop_editor_open),
];

#[cfg(test)]
#[path = "image_tests.rs"]
pub(crate) mod tests;
