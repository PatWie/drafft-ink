//! `element.create_sticky`: a sticky note (docs/commands.md §2.4).
//!
//! A note is a rectangle with a preset look and a centred label (`drafftink_edit::sticky`). The command validates
//! its arguments and delegates; the preset, the size rules and the font fit are the domain's.

use drafftink_edit::TargetError;
use drafftink_edit::create::{CreateError, FrameChoice};
use drafftink_edit::sticky::{self, NoteColor, NoteSpec, StickyError};
use drafftink_geom::kurbo::Point;
use drafftink_scene::SceneRead;
use drafftink_scene::limits::MAX_TEXT_CHARS;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::clipboard::At;
use super::clipboard::common::resolve_at;
use super::element_common::{edit_with_measure, select_exactly};
use super::element_create::{FrameArg, check_name, refuse, resolve_id};
use super::view::RectArg;
use crate::args;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed, Undo,
};

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Arguments of `element.create_sticky`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StickyArgs {
    /// The middle of the note: a world point `[x, y]`, `viewport_center` or `$cursor`. Default the middle of the
    /// visible canvas. Not with `bounds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<At>,
    /// Side of the square note in world units, 75 or more. Default 250. Not with `bounds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// The exact box of the note instead of `at` and `size`. Both sides must be at least 75.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<RectArg>,
    /// The text of the note. Blank or absent makes an empty note; type into it later with `text.edit_begin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Paper colour: yellow (default), pink, green, blue or orange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<NoteColor>,
    /// The largest label font size, 16 to 512. Default 28. The text gets the largest size at or below this that fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// A unique name, so that agents can address the note as `@name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The id of the new note. Default: a fresh id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Select the new note (default true).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub select: bool,
    /// `"auto"` (default): join the frame under the note's centre. An id or `@name`: join that frame. `null`: no frame.
    #[serde(default, skip_serializing_if = "FrameArg::is_auto")]
    pub frame: FrameArg,
    /// Start typing into the note right away (as `text.edit_begin`). Default false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub edit: bool,
}

/// What `element.create_sticky` made.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct StickyOut {
    /// The id of the note.
    pub id: String,
    /// The id of its label, when it has text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Its box after the label fitted (a note grows when the text cannot fit at the smallest size).
    pub bounds: RectArg,
    /// The frame it joined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// The label font size the fit chose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
}

fn refuse_sticky(cmd: &str, e: StickyError) -> Refusal {
    match e {
        StickyError::TooSmall { width, height } => {
            let dims = match (width, height) {
                (true, true) => "width and height",
                (true, false) => "width",
                _ => "height",
            };
            Refusal::custom(
                "too_small",
                cmd,
                format!(
                    "The sticky note is smaller than {} units in {dims}. Give a size, or bounds with w and h, of at least {}.",
                    sticky::MIN_SIZE,
                    sticky::MIN_SIZE
                ),
            )
        }
        StickyError::BadFontSize => Refusal::bad_arg(
            cmd,
            "font_size",
            format!("must be a number from {} to {}", sticky::FONT_MIN, sticky::FONT_MAX),
        ),
        StickyError::Create(e) => refuse(cmd, e),
    }
}

/// The box of the note, from `bounds` or from `at` and `size`.
fn note_box(s: &Session, a: &StickyArgs) -> Result<(f64, f64, f64, f64), Refusal> {
    let cmd = s.cmd();
    if let Some(b) = a.bounds {
        for (key, given) in [("at", a.at.is_some()), ("size", a.size.is_some())] {
            if given {
                return Err(Refusal::bad_arg(cmd, key, "does not apply together with `bounds`. Give `bounds`, or `at` and `size`"));
            }
        }
        let (x, y, w, h) =
            (args::world(cmd, "bounds.x", b.x)?, args::world(cmd, "bounds.y", b.y)?, args::world(cmd, "bounds.w", b.w)?, args::world(cmd, "bounds.h", b.h)?);
        return Ok(drafftink_edit::create::normalized_box(x, y, w, h));
    }
    let side = match a.size {
        Some(v) => args::positive(cmd, "size", v, args::MAX_WORLD)?,
        None => sticky::DEFAULT_SIZE,
    };
    let centre: Point = match &a.at {
        Some(at) => resolve_at(s, at)?,
        None => s.view().scroll,
    };
    Ok((centre.x - side / 2.0, centre.y - side / 2.0, side, side))
}

fn create_sticky(s: &mut Session, a: StickyArgs) -> Result<StickyOut, Refusal> {
    let cmd = s.cmd();
    let bounds = note_box(s, &a)?;
    sticky::check_size(bounds.2, bounds.3).map_err(|e| refuse_sticky(cmd, e))?;
    if let Some(size) = a.font_size {
        sticky::check_ceiling(size).map_err(|e| refuse_sticky(cmd, e))?;
    }
    if let Some(t) = &a.text {
        let n = t.chars().count();
        if n > MAX_TEXT_CHARS {
            return Err(Refusal::limit_exceeded(cmd, n, "characters", MAX_TEXT_CHARS));
        }
    }
    let id = resolve_id(s, cmd, &a.id)?;
    let name = check_name(s, cmd, &a.name)?;
    let frame = match &a.frame {
        FrameArg::Auto => FrameChoice::Auto,
        FrameArg::NoFrame => FrameChoice::None,
        FrameArg::Frame(r) => FrameChoice::Frame(s.resolve_ref(r)?),
    };
    let defaults = s.tool.defaults();
    let spec = NoteSpec {
        id,
        bounds,
        color: a.color.unwrap_or_default(),
        text: a.text.clone(),
        font_ceiling: a.font_size,
        base: defaults.style,
        font: defaults.text,
        name,
        frame,
    };
    let (note, rect, font_size) = edit_with_measure::<_, StickyError>(s, |tx, measure, ids| {
        let note = sticky::create_note(tx, measure, ids, &spec)?;
        let rect = tx.get(&note.id).map(|e| e.transform).ok_or_else(|| CreateError::Target(TargetError::NotFound(note.id.clone())))?;
        let font_size = note.label.as_ref().and_then(|l| tx.get(l)).and_then(|e| e.text_data().map(|t| t.font.size));
        Ok((note, rect, font_size))
    })
    .map_err(|e| refuse_sticky(cmd, e))?;
    if a.select {
        select_exactly(s, std::slice::from_ref(&note.id), false);
    }
    if a.edit {
        s.run_nested("text.edit_begin", json!({ "id": note.id.as_str() }))?;
    }
    Ok(StickyOut {
        id: note.id.as_str().to_owned(),
        label: note.label.map(|l| l.as_str().to_owned()),
        bounds: RectArg { x: rect.x, y: rect.y, w: rect.w, h: rect.h },
        frame: note.joined.frame.map(|f| f.as_str().to_owned()),
        font_size,
    })
}

const FACTS: Facts = Facts {
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
    palette: true,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

const STICKY_KEYS: &[Binding] = &[Binding::new(&["N"], Scope::Canvas).with_args(r#"{"at":"$cursor","edit":true}"#)];

/// Commands of this file.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "element.create_sticky",
    "element",
    "Add sticky note",
    "Creates a sticky note: a coloured, rounded rectangle with centred text that is sized to fit, and selects it.",
    &Typed::new(create_sticky),
    FACTS,
)
.shortcuts(STICKY_KEYS)
.refusals(&["too_small", "id_in_use", "name_taken", "bad_name"])
.example(r#"{"text":"Ask Sam about the budget","color":"pink","at":[200,150]}"#)];

#[cfg(test)]
#[path = "element_sticky_tests.rs"]
mod tests;
