//! `clipboard.copy_as`: the selection (or everything) on the system clipboard as a PNG, an SVG or text.
//!
//! One implementation replaces the three copies of the old copy-PNG block; the picture itself is made by
//! `doc::export::render_png`, the code that `doc.export` uses. The picture is made from the same
//! subset of the document that `copy` uses, so elements outside the scope never show up in it.

use std::collections::HashSet;

use drafftink_edit::group::units::{Relations, with_companions};
use drafftink_io::native;
use drafftink_scene::{Element, ElementId, ElementKind, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::cmd::doc::export::{PngOpts, SvgLook, render_png, render_svg};
use crate::ports::{ClipPayload, PortWrite};
use crate::refusal::Refusal;
use crate::session::{Ref, Session};

/// The largest picture side `copy_as` renders, in pixels.
pub const MAX_SIDE_PX: f64 = 8192.0;

/// What is put on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// A PNG picture. Needs the renderer.
    #[default]
    Png,
    /// An SVG drawing.
    Svg,
    /// The text of the text elements and labels, one block per line.
    Text,
}

/// What is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeWord {
    /// The selection.
    #[default]
    Selection,
    /// Every element.
    All,
    /// The elements in `ids`.
    Ids,
}

/// Arguments of `clipboard.copy_as`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CopyAsArgs {
    /// png, svg or text.
    pub format: Format,
    /// selection, all or ids.
    pub scope: ScopeWord,
    /// The elements for scope `ids`.
    pub ids: Option<Vec<Ref>>,
    /// Pixels per world unit (0.25 to 4). Default: the setting `export.scale`.
    pub scale: Option<f64>,
    /// Space around the content in world units (0 to 1000). Default: the setting `export.padding`.
    pub padding: Option<f64>,
    /// Paint the document background. Default: the setting `export.background`.
    pub background: Option<bool>,
    /// Dark theme (png). Default: the setting `export.dark`. Ignored for svg and text.
    pub dark: Option<bool>,
}

/// Output of `clipboard.copy_as`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CopyAsOut {
    /// What was written.
    pub format: Format,
    /// Elements in the picture or text, bound text included.
    pub count: usize,
    /// Size of what was written in bytes.
    pub bytes: usize,
    /// Picture size in pixels, for png.
    pub size_px: Option<[u32; 2]>,
}

fn scope_elements(s: &Session, a: &CopyAsArgs) -> Result<Vec<ElementId>, Refusal> {
    let cmd = s.cmd();
    match a.scope {
        ScopeWord::Selection => {
            if a.ids.is_some() {
                return Err(Refusal::bad_arg(cmd, "ids", "is only used with scope \"ids\""));
            }
            s.targets(None)
        }
        ScopeWord::Ids => match &a.ids {
            Some(ids) => s.targets(Some(ids)),
            None => Err(Refusal::bad_arg(cmd, "ids", "is needed with scope \"ids\"")),
        },
        ScopeWord::All => {
            if a.ids.is_some() {
                return Err(Refusal::bad_arg(cmd, "ids", "is only used with scope \"ids\""));
            }
            let all: Vec<ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
            if all.is_empty() {
                return Err(Refusal::nothing_to_do(cmd, "copy", "the document has no elements. Draw something first"));
            }
            Ok(all)
        }
    }
}

pub(super) fn copy_as(s: &mut Session, a: CopyAsArgs) -> Result<CopyAsOut, Refusal> {
    let cmd = s.cmd();
    let scale = match a.scale {
        Some(v) => args::in_range(cmd, "scale", v, 0.25, 4.0)?,
        None => s.settings.f64_or("export.scale", 2.0).clamp(0.25, 4.0),
    };
    let padding = match a.padding {
        Some(v) => args::in_range(cmd, "padding", v, 0.0, 1000.0)?,
        None => s.settings.f64_or("export.padding", 20.0).clamp(0.0, 1000.0),
    };
    let background = a.background.unwrap_or_else(|| s.settings.bool_or("export.background", true));
    if s.ports.clipboard.is_none() {
        return Err(Refusal::port_unavailable(
            cmd,
            "the system clipboard",
            "headless",
            "Use `doc.export` to get the data instead.",
        ));
    }
    if a.format == Format::Png && s.ports.renderer.is_none() {
        return Err(Refusal::port_unavailable(
            cmd,
            "the renderer",
            "headless without a GPU",
            "Use format svg or text.",
        ));
    }
    let given = scope_elements(s, &a)?;
    let store = s.store();
    let rel = Relations::build(store);
    let all = with_companions(store, &rel, &given, true);
    let keep: HashSet<ElementId> = all.iter().cloned().collect();
    let doc = native::subset(&s.tab().doc.snapshot(), &keep);
    let dark = a.dark.unwrap_or_else(|| s.settings.bool_or("export.dark", false));

    let (payload, bytes, size_px) = match a.format {
        Format::Text => {
            let text = text_of(doc.elements.iter());
            if text.is_empty() {
                return Err(Refusal::nothing_to_do(cmd, "copy", "none of the elements holds text"));
            }
            let n = text.len();
            (ClipPayload::Text(text), n, None)
        }
        Format::Svg => {
            let look = SvgLook { scale, padding, background, dark, embed_scene: false };
            let svg = render_svg(s, &doc, &look)
                .map_err(|e| Refusal::nothing_to_do(cmd, "copy", &e.to_string()))?;
            let n = svg.len();
            (ClipPayload::Svg(svg), n, None)
        }
        Format::Png => {
            let opts = PngOpts { scale, padding, background, dark, clip_to_frame: false, max_side_px: MAX_SIDE_PX };
            let png = render_png(s, cmd, &doc, &opts)?;
            let n = png.bytes.len();
            (ClipPayload::Image { bytes: png.bytes, mime: "image/png".to_owned() }, n, Some([png.width, png.height]))
        }
    };
    s.queue_write(PortWrite::Clipboard(payload));
    Ok(CopyAsOut { format: a.format, count: all.len(), bytes, size_px })
}

/// The content of every text element, in z-order, one per line.
fn text_of<'a>(elements: impl Iterator<Item = &'a Element>) -> String {
    elements
        .filter(|e| !e.deleted)
        .filter_map(|e| match &e.kind {
            ElementKind::Text(t) if !t.content.trim().is_empty() => Some(t.content.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
