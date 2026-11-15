//! `image` (paint): painting on pictures (docs/commands.md §2.13).
//!
//! The picture stays whole and untouched; the strokes live on the element, in pixels of the stored picture, and
//! the renderer and the exporters bake them over it (`drafftink_scene::paint::bake`). The rules (what a valid
//! stroke is, the limits, the brush, where a canvas point lies on the picture) are in `drafftink_edit::retouch`;
//! the commands here read their arguments, fill in what the session brush supplies, and delegate.
//!
//! * `image.paint` adds strokes (one undo step per call), `image.reset_paint` takes them all off.
//! * `image.paint_begin` and `image.paint_end` open and close the paint editor, a mode of the select tool like the
//!   crop editor (`interact::paint` is its pointer side). The two editors are never open together.
//! * `image.paint_brush` sets the session brush: what a stroke gets for the settings it does not name.

use drafftink_edit::retouch::{self, MAX_STROKES_PER_CALL, PaintBrush, PaintError};
use drafftink_edit::style::ColorArg;
use drafftink_edit::TargetError;
use drafftink_geom::kurbo::Point;
use drafftink_scene::paint::{PaintMode, PaintStroke};
use drafftink_scene::ElementId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::clipboard::common::edit_with;
use super::element_common::target_refusal;
use super::image::{PICTURE_NOT_LOADED, one_image};
use crate::args::Pt;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};
use crate::state::Tool;

/// The refusal code for a stroke or a brush with a number out of range.
const BAD_PAINT: &str = "bad_paint";

fn paint_refusal(cmd: &str, e: PaintError) -> Refusal {
    match e {
        PaintError::Target(t) => target_refusal(cmd, t),
        PaintError::NotAnImage { id, kind } => {
            Refusal::wrong_kind(cmd, kind, "images").with_details(json!({ "ids": [id.as_str()] }))
        }
        PaintError::NotLoaded { .. } => Refusal::custom(PICTURE_NOT_LOADED, cmd, e.to_string()),
        PaintError::NoStrokes => Refusal::bad_arg(cmd, "strokes", "needs at least one stroke"),
        PaintError::Limit { .. } => Refusal::custom("limit_exceeded", cmd, e.to_string()),
        PaintError::Store(m) => Refusal::internal(cmd, m),
        PaintError::NoSize { .. } | PaintError::BadStroke { .. } | PaintError::NoStrokeId => {
            Refusal::custom(BAD_PAINT, cmd, e.to_string())
        }
    }
}

// ---- image.paint ----

/// One brush stroke of `image.paint`. Coordinates and sizes are in pixels of the stored picture.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaintStrokeArg {
    /// `paint` lays the colour over the picture; `erase` gives the original pixels back where the brush passes.
    /// Default: the mode of the session brush (`image.paint_brush`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PaintMode>,
    /// The colour: `#rrggbb`, `#rrggbbaa` or a palette name such as `red-600`. Its alpha multiplies `opacity`.
    /// An erase stroke ignores it. Default: the colour of the session brush.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorArg>,
    /// The brush diameter at full pressure, 0.5 to 4096 picture pixels. Default: the size of the session brush
    /// (until a size is set, 1.5 % of the picture's longer side, from 4 to 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// 1 is a hard edge, towards 0 the edge fades over more of the radius. Default: the session brush.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardness: Option<f64>,
    /// How much of the picture the stroke covers, 0 (nothing) to 1 (all). The whole stroke has this opacity, so
    /// where it crosses itself it does not get darker. Default: the session brush.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Where the brush went, in pixels of the stored picture from its top-left corner (`natural_width` by
    /// `natural_height` of `element.query`). 1 to 10000 points; a single point is a dab.
    pub points: Vec<Pt>,
    /// Pen pressure per point, 0 to 1: none, or one per point. The brush is narrower where the pressure is low.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pressures: Vec<f64>,
}

/// Arguments of `image.paint`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PaintArgs {
    /// The image: an id or `@name`. Default: the single selected element.
    pub id: Option<Ref>,
    /// The strokes to add, 1 to 64, applied in this order after the strokes the picture has.
    pub strokes: Vec<PaintStrokeArg>,
}

/// Result of `image.paint`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PaintOut {
    /// The image.
    pub id: String,
    /// How many strokes the image carries now.
    pub strokes: usize,
    /// The ids of the strokes this call added, in order.
    pub added: Vec<String>,
    /// True when the document changed.
    pub changed: bool,
}

/// The strokes of a call as the model has them: what the call leaves out comes from the session brush.
fn resolve_strokes(brush: &PaintBrush, natural: (u32, u32), given: Vec<PaintStrokeArg>) -> Vec<PaintStroke> {
    given
        .into_iter()
        .map(|g| {
            let points = g.points.iter().map(|[x, y]| Point::new(*x, *y)).collect();
            let looks = PaintBrush {
                mode: g.mode.unwrap_or(brush.mode),
                color: g.color.map_or(brush.color, |c| c.0),
                hardness: g.hardness.unwrap_or(brush.hardness),
                opacity: g.opacity.unwrap_or(brush.opacity),
                ..*brush
            };
            looks.stroke(g.size.unwrap_or_else(|| brush.size_for(natural)), points, g.pressures)
        })
        .collect()
}

fn paint(s: &mut Session, a: PaintArgs) -> Result<PaintOut, Refusal> {
    let cmd = s.cmd();
    if a.strokes.is_empty() {
        return Err(Refusal::bad_arg(cmd, "strokes", "needs at least one stroke"));
    }
    if a.strokes.len() > MAX_STROKES_PER_CALL {
        return Err(Refusal::limit_exceeded(cmd, a.strokes.len(), "strokes", MAX_STROKES_PER_CALL));
    }
    let id = one_image(s, a.id.as_ref())?;
    let natural = s.element(&id).and_then(|e| e.image_data()).map_or((0, 0), |d| (d.natural_width, d.natural_height));
    let strokes = resolve_strokes(&s.tool.paint_brush, natural, a.strokes);
    let painted = edit_with::<_, PaintError>(s, |tx, ids, _| {
        let tag = format!("{:08x}", ids.seed());
        retouch::paint_image(tx, &id, strokes, &tag)
    })
    .map_err(|e| paint_refusal(cmd, e))?;
    Ok(PaintOut {
        id: id.as_str().to_owned(),
        strokes: painted.total,
        added: painted.added.iter().map(|i| i.as_str().to_owned()).collect(),
        changed: !painted.added.is_empty(),
    })
}

// ---- image.reset_paint ----

/// Arguments of `image.reset_paint`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ResetPaintArgs {
    /// The image: an id or `@name`. Default: the single selected element.
    pub id: Option<Ref>,
}

/// Result of `image.reset_paint`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ResetPaintOut {
    /// The image.
    pub id: String,
    /// How many strokes were taken off.
    pub removed: usize,
}

fn reset_paint(s: &mut Session, a: ResetPaintArgs) -> Result<ResetPaintOut, Refusal> {
    let cmd = s.cmd();
    let id = one_image(s, a.id.as_ref())?;
    let (removed, _) = s
        .tab_mut()
        .doc
        .edit::<usize, PaintError>(|tx| retouch::reset_paint(tx, &id))
        .map_err(|e| paint_refusal(cmd, e))?;
    Ok(ResetPaintOut { id: id.as_str().to_owned(), removed })
}

/// The one selected image whose picture is loaded and which is not locked: what the paint commands are offered
/// for.
fn single_paintable(s: &Session) -> Result<ElementId, Refusal> {
    let cmd = s.cmd();
    let id = one_image(s, None)?;
    let e = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
    if e.image_data().is_none() {
        return Err(Refusal::wrong_kind(cmd, e.tag().name(), "images"));
    }
    if e.locked {
        return Err(target_refusal(cmd, TargetError::Locked(vec![id])));
    }
    if !retouch::can_paint(s.store(), e) {
        return Err(Refusal::custom(
            PICTURE_NOT_LOADED,
            cmd,
            "The picture has not arrived yet, so it cannot be painted on. Wait for it to load, then try again.",
        ));
    }
    Ok(id)
}

fn available_to_paint(s: &Session) -> Result<(), Refusal> {
    single_paintable(s).map(drop)
}

/// `image.reset_paint` is offered for one selected image that has paint.
fn available_to_reset(s: &Session) -> Result<(), Refusal> {
    let id = single_paintable(s)?;
    let painted = s.element(&id).and_then(|e| e.image_data()).is_some_and(|d| !d.paint.is_empty());
    if painted { Ok(()) } else { Err(Refusal::nothing_to_do(s.cmd(), "reset", "the image has no paint")) }
}

// ---- the editor ----

/// Arguments of `image.paint_begin`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PaintBeginArgs {
    /// The image: an id or `@name`. Default: the single selected element. It becomes the selection.
    pub id: Option<Ref>,
}

/// Result of `image.paint_begin` and `image.paint_end`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PaintEditorOut {
    /// The image that is being painted on now; null when the editor is closed.
    pub id: Option<String>,
    /// False when the editor was in this state already.
    pub changed: bool,
}

fn paint_begin(s: &mut Session, a: PaintBeginArgs) -> Result<PaintEditorOut, Refusal> {
    let cmd = s.cmd();
    let id = one_image(s, a.id.as_ref())?;
    let e = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
    let Some(d) = e.image_data() else {
        return Err(Refusal::wrong_kind(cmd, e.tag().name(), "images"));
    };
    let natural = (d.natural_width, d.natural_height);
    if e.locked {
        return Err(target_refusal(cmd, TargetError::Locked(vec![id])));
    }
    if !retouch::can_paint(s.store(), e) {
        return Err(Refusal::custom(
            PICTURE_NOT_LOADED,
            cmd,
            "The picture has not arrived yet, so it cannot be painted on. Wait for it to load, then try again.",
        ));
    }
    // Until a size is chosen the brush follows the picture it is about to paint on.
    if !s.tool.paint_brush.size_chosen {
        s.tool.paint_brush.size = retouch::default_brush_size(natural);
    }
    if s.tab().observed.paint_editor.as_ref() == Some(&id) {
        return Ok(PaintEditorOut { id: Some(id.as_str().to_owned()), changed: false });
    }
    // The editor is a mode of the select tool, and it belongs to one image: select that image alone.
    if s.tool.active != Tool::Select {
        s.run_nested("tool.set", json!({ "tool": "select" }))?;
    }
    s.run_nested("selection.select", json!({ "ids": [id.as_str()] }))?;
    let obs = &mut s.tab_mut().observed;
    obs.crop_editor = None;
    obs.paint_editor = Some(id.clone());
    Ok(PaintEditorOut { id: Some(id.as_str().to_owned()), changed: true })
}

/// Arguments of `image.paint_end`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PaintEndArgs {}

fn paint_end(s: &mut Session, _: PaintEndArgs) -> Result<PaintEditorOut, Refusal> {
    let was_open = s.tab_mut().observed.paint_editor.take().is_some();
    Ok(PaintEditorOut { id: None, changed: was_open })
}

fn paint_editor_open(s: &Session) -> Result<(), Refusal> {
    if s.tab().observed.paint_editor.is_some() {
        Ok(())
    } else {
        Err(Refusal::nothing_to_do(s.cmd(), "end", "the paint editor is not open. Open it with `image.paint_begin`"))
    }
}

// ---- the brush ----

/// Arguments of `image.paint_brush`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PaintBrushArgs {
    /// The colour: `#rrggbb`, `#rrggbbaa` or a palette name such as `red-600`.
    pub color: Option<ColorArg>,
    /// The diameter at full pressure, 0.5 to 4096 picture pixels. Once set it stays, whatever the picture.
    pub size: Option<f64>,
    /// 1 is a hard edge, 0 the softest.
    pub hardness: Option<f64>,
    /// 0 (clear) to 1 (solid).
    pub opacity: Option<f64>,
    /// `paint` or `erase`.
    pub mode: Option<PaintMode>,
}

/// The session brush, as `image.paint_brush` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PaintBrushOut {
    /// The colour as `#rrggbb` (or `#rrggbbaa`).
    pub color: String,
    /// The diameter at full pressure in picture pixels. Until a size is set it is the default of the last picture
    /// the paint editor opened on.
    pub size: f64,
    /// 1 is a hard edge, 0 the softest.
    pub hardness: f64,
    /// 0 (clear) to 1 (solid).
    pub opacity: f64,
    /// `paint` or `erase`.
    pub mode: PaintMode,
}

impl From<&PaintBrush> for PaintBrushOut {
    fn from(b: &PaintBrush) -> Self {
        Self { color: b.color.to_hex(), size: b.size, hardness: b.hardness, opacity: b.opacity, mode: b.mode }
    }
}

fn paint_brush(s: &mut Session, a: PaintBrushArgs) -> Result<PaintBrushOut, Refusal> {
    let cmd = s.cmd();
    let now = s.tool.paint_brush;
    let next = PaintBrush {
        color: a.color.map_or(now.color, |c| c.0),
        size: a.size.unwrap_or(now.size),
        hardness: a.hardness.unwrap_or(now.hardness),
        opacity: a.opacity.unwrap_or(now.opacity),
        mode: a.mode.unwrap_or(now.mode),
        size_chosen: now.size_chosen || a.size.is_some(),
    };
    next.validate().map_err(|p| Refusal::custom(BAD_PAINT, cmd, format!("The brush is not valid: {p}.")))?;
    s.tool.paint_brush = next;
    Ok(PaintBrushOut::from(&next))
}

// ---- registration ----

/// `image.paint` and `image.reset_paint` change the document and nothing else.
const PAINT: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::Soft,
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
    redraw: Redraw::Overlay,
    palette: true,
    doc_write: crate::spec::DocWrite::NotDocument,
    ..PAINT
};

/// The paint commands of the image family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "image.paint",
        "image",
        "Paint",
        "Paints brush strokes on an image, in pixels of the stored picture. The picture stays whole, so the paint can be erased or reset later.",
        &Typed::new(paint),
        PAINT,
    )
    .default_targets(available_to_paint)
    .refusals(&[BAD_PAINT, PICTURE_NOT_LOADED])
    .example(r##"{"id":"e1","strokes":[{"color":"#e03131","size":12,"points":[[20,20],[80,40],[120,30]]}]}"##),
    CommandSpec::new(
        "image.reset_paint",
        "image",
        "Reset paint",
        "Takes all the paint off an image and shows the picture as it was.",
        &Typed::new(reset_paint),
        Facts { palette: true, ..PAINT },
    )
    .default_targets(available_to_reset)
    .example(r#"{"id":"e1"}"#),
    CommandSpec::new(
        "image.paint_begin",
        "image",
        "Edit picture",
        "Opens the paint editor on an image: drag on the picture to paint with the session brush.",
        &Typed::new(paint_begin),
        Facts { mutates: Mutates::OBSERVED.with(Mutates::TOOL), needs_selection: Selection::Soft, ..EDITOR },
    )
    .default_targets(available_to_paint)
    .refusals(&[PICTURE_NOT_LOADED])
    .shortcuts(PAINT_KEYS)
    .example(r#"{"id":"e1"}"#),
    CommandSpec::new(
        "image.paint_end",
        "image",
        "Done editing picture",
        "Closes the paint editor. The strokes already painted stay in the document.",
        &Typed::new(paint_end),
        Facts { needs_selection: Selection::No, agent: false, ..EDITOR },
    )
    .available(paint_editor_open),
    CommandSpec::new(
        "image.paint_brush",
        "image",
        "Set brush",
        "Sets the brush of the paint editor and the default of `image.paint`: colour, size, hardness, opacity and paint or erase. Without arguments it reports the brush.",
        &Typed::new(paint_brush),
        Facts {
            mutates: Mutates::TOOL,
            needs_selection: Selection::No,
            view_only: Gate::Allowed,
            while_text_edit: TextGate::Allowed,
            palette: false,
            // The strip's sliders call it for every step of a drag: only a refusal is worth a ring entry.
            ring: RingMode::Quiet,
            ..EDITOR
        },
    )
    .refusals(&[BAD_PAINT])
    .example(r##"{"color":"#1971c2","size":16,"hardness":0.5,"opacity":0.8,"mode":"paint"}"##),
];

/// `B` opens the paint editor on the selected image.
const PAINT_KEYS: &[crate::spec::Binding] = &[crate::spec::Binding::new(&["B"], crate::spec::Scope::Canvas)];

#[cfg(test)]
#[path = "image_paint_tests.rs"]
pub(crate) mod tests;
