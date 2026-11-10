//! `clipboard.copy_as`: the selection (or everything) on the system clipboard as a PNG, an SVG or text.
//!
//! One implementation replaces the three copies of the old copy-PNG block. The picture is made from the same
//! subset of the document that `copy` uses, so elements outside the scope never show up in it.

use std::collections::HashSet;

use drafftink_edit::group::units::{Relations, with_companions};
use drafftink_edit::transform::geometry::union_bounds;
use drafftink_io::native;
use drafftink_io::svg::{SvgOptions, SvgStyle, export_svg};
use drafftink_scene::{Element, ElementId, ElementKind, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::ports::{ClipPayload, PortWrite, RenderRequest};
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
    /// Dark theme. Not supported yet: it is ignored and a warning says so.
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
    if a.dark == Some(true) || (a.dark.is_none() && s.settings.bool_or("export.dark", false)) {
        s.warn("The dark theme is not supported for copies yet, so the light theme was used.");
    }

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
            let opts = SvgOptions {
                style: SvgStyle::Sketch,
                background,
                padding,
                scale,
                selection: None,
                embed_scene: false,
                fonts: Vec::new(),
            };
            let svg = export_svg(&doc, &opts, s.services.io(), Some(s.services.math_svg()))
                .map_err(|e| Refusal::nothing_to_do(cmd, "copy", &e.to_string()))?;
            let n = svg.len();
            (ClipPayload::Svg(svg), n, None)
        }
        Format::Png => {
            let live: Vec<&Element> = doc.elements.iter().filter(|e| !e.deleted && e.container_id().is_none()).collect();
            let b = union_bounds(live.iter().copied())
                .or_else(|| union_bounds(doc.elements.iter().filter(|e| !e.deleted)))
                .ok_or_else(|| Refusal::nothing_to_do(cmd, "copy", "the elements have no size"))?;
            let region = (b.x0 - padding, b.y0 - padding, b.width() + 2.0 * padding, b.height() + 2.0 * padding);
            let (pw, ph) = ((region.2 * scale).ceil().max(1.0), (region.3 * scale).ceil().max(1.0));
            if !(pw.is_finite() && ph.is_finite()) || pw.max(ph) > MAX_SIDE_PX {
                return Err(Refusal::bad_arg(
                    cmd,
                    "scale",
                    format!("would make a picture over {MAX_SIDE_PX} pixels on a side. Lower `scale` or copy fewer elements"),
                ));
            }
            let size = (pw as u32, ph as u32);
            let req = RenderRequest {
                region,
                size_px: size,
                background,
                overlay: false,
                selection: Vec::new(),
                background_color: doc.background,
            };
            let png = match s.ports.renderer.as_ref() {
                Some(r) => r
                    .render_png(&doc, &req)
                    .map_err(|e| Refusal::io_failed(cmd, "render", "the picture", &e.to_string()))?,
                None => return Err(Refusal::internal(cmd, "the renderer disappeared")),
            };
            let n = png.len();
            (ClipPayload::Image { bytes: png, mime: "image/png".to_owned() }, n, Some([size.0, size.1]))
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
