//! `doc.export`: the one export implementation. PNG goes through the `Renderer` port with the scene
//! embedded (`png_meta::embed_scene`); SVG, JSON and `.excalidraw` are pure functions of `drafftink-io`.

use std::collections::HashSet;

use drafftink_edit::library;
use drafftink_geom::kurbo::Rect;
use drafftink_io::svg::{SvgOptions, SvgStyle, export_svg};
use drafftink_io::{excalidraw, native, png_meta};
use drafftink_scene::{Document, ElementId, Kind, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::files::{self, base64_of};
use crate::args;
use crate::ports::{FileRef, PortKind, RenderRequest};
use crate::refusal::Refusal;
use crate::session::{Ref, Session};

/// Most pixels along one side of an exported PNG.
const MAX_SIDE_PX: f64 = 16_384.0;
/// Most pixels in an exported PNG.
const MAX_AREA_PX: f64 = 100_000_000.0;

/// The export format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// A picture. Needs the renderer.
    #[default]
    Png,
    /// A vector picture.
    Svg,
    /// Drafft.ink JSON.
    Json,
    /// An Excalidraw scene.
    Excalidraw,
}

impl ExportFormat {
    fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Svg => "image/svg+xml",
            Self::Json => "application/json",
            Self::Excalidraw => "application/vnd.excalidraw+json",
        }
    }
}

/// What to export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportScope {
    /// The selection if there is one, else everything.
    #[default]
    Auto,
    /// Everything.
    All,
    /// The selection.
    Selection,
    /// The elements in `ids`.
    Ids,
    /// One frame and its children, clipped to the frame.
    Frame,
}

/// The only word `to` accepts besides a file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InlineWord {
    /// Return the bytes in the output.
    #[default]
    Inline,
}

/// Where the export goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Dest {
    /// `"inline"`: the bytes come back in the output as base64.
    Inline(InlineWord),
    /// A file: `{path}`, `{download}`, `{stored}` or `{handle}`.
    File(FileRef),
}

impl Default for Dest {
    fn default() -> Self {
        Dest::Inline(InlineWord::Inline)
    }
}

/// Arguments of `doc.export`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ExportArgs {
    /// `png` (default), `svg`, `json` or `excalidraw`.
    pub format: ExportFormat,
    /// `auto` (default: the selection if there is one, else everything), `all`, `selection`, `ids` or `frame`.
    pub scope: ExportScope,
    /// The elements, for scope `ids`.
    pub ids: Option<Vec<Ref>>,
    /// The frame, for scope `frame`.
    pub frame: Option<Ref>,
    /// PNG and SVG size factor, 0.25 to 4. Default: the setting `export.scale`.
    pub scale: Option<f64>,
    /// World units around the content, 0 to 1000. Default: the setting `export.padding`.
    pub padding: Option<f64>,
    /// Paint the document background. Default: the setting `export.background`.
    pub background: Option<bool>,
    /// Draw with the dark palette (PNG only). Default: the setting `export.dark`.
    pub dark: Option<bool>,
    /// Embed the scene so that the picture can be opened again with `doc.open`. Default: the setting `export.embed_scene`.
    pub embed_scene: Option<bool>,
    /// `inline` (default) returns the bytes; a file is written through the files port.
    pub to: Dest,
}

/// The bytes of an inline export.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExportBytes {
    /// The content, base64.
    pub base64: String,
    /// Its media type.
    pub mime: &'static str,
}

/// What an export produced.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExportOut {
    /// Always false: an export changes no document.
    pub changed: bool,
    /// The format.
    pub format: ExportFormat,
    /// Its media type.
    pub mime: &'static str,
    /// Size in bytes.
    pub size: usize,
    /// Elements exported.
    pub elements: usize,
    /// Width in pixels (PNG).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Height in pixels (PNG).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Whether the scene is embedded in the picture (PNG, SVG).
    pub scene_embedded: bool,
    /// The content, for `to: inline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<ExportBytes>,
    /// Where it was written, for a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_to: Option<FileRef>,
}

fn no_renderer() -> Refusal {
    Refusal::custom(
        "port_unavailable",
        "doc.export",
        "`doc.export` with format png needs the renderer, which this session does not provide (headless without a GPU). Use format svg or json.",
    )
}

/// The ids to export, with bound text and frame children, as a set.
fn chosen(s: &Session, id: &str, a: &ExportArgs) -> Result<Vec<ElementId>, Refusal> {
    let given: Vec<ElementId> = match a.scope {
        ExportScope::All => return Ok(s.store().iter().map(|e| e.id.clone()).collect()),
        ExportScope::Auto if s.selection().is_empty() => return Ok(s.store().iter().map(|e| e.id.clone()).collect()),
        ExportScope::Auto | ExportScope::Selection => s.targets(None)?,
        ExportScope::Ids => {
            let refs = a.ids.as_deref().ok_or_else(|| Refusal::bad_arg(id, "ids", "is needed when scope is `ids`"))?;
            s.targets(Some(refs))?
        }
        ExportScope::Frame => {
            let r = a.frame.as_ref().ok_or_else(|| Refusal::bad_arg(id, "frame", "is needed when scope is `frame`"))?;
            let f = s.resolve_ref(r)?;
            let kind = s.element(&f).map(|e| e.tag());
            if kind != Some(Kind::Frame) {
                let name = kind.map_or("unknown", |k| k.name());
                return Err(Refusal::wrong_kind(id, name, "frames"));
            }
            vec![f]
        }
    };
    library::members(s.store(), &given).map_err(|e| Refusal::internal(id, e.to_string()))
}

fn io_failed_export(id: &str, what: &str, e: &dyn std::fmt::Display) -> Refusal {
    Refusal::io_failed(id, "write", what, &e.to_string())
}

/// How [`render_png`] draws.
pub(crate) struct PngOpts {
    /// Pixels per world unit.
    pub scale: f64,
    /// World units around the content.
    pub padding: f64,
    /// Paint the document background.
    pub background: bool,
    /// Draw with the dark palette.
    pub dark: bool,
    /// The region is the bounds of the frame in the document, not of everything in it.
    pub clip_to_frame: bool,
    /// Most pixels along one side. The area limit [`MAX_AREA_PX`] always applies.
    pub max_side_px: f64,
}

/// A PNG made by the renderer port.
pub(crate) struct RenderedPng {
    /// The encoded picture.
    pub bytes: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// The one PNG implementation: the region of `doc` (its content plus padding), the size limits, the request
/// to the `Renderer` port. `doc.export` and `clipboard.copy_as` both end here. The caller has checked that
/// the port exists, and decides which elements are in `doc` and what happens to the bytes (scene embedding,
/// file, clipboard).
pub(crate) fn render_png(s: &Session, id: &str, doc: &Document, opts: &PngOpts) -> Result<RenderedPng, Refusal> {
    let content: Rect =
        library::bounds(doc).ok_or_else(|| Refusal::nothing_to_do(id, "export", "the chosen elements have no size"))?;
    let region = if opts.clip_to_frame {
        // Clipped to the frame: its own bounds, no padding of other content.
        doc.elements
            .iter()
            .filter(|e| e.tag() == Kind::Frame)
            .map(drafftink_edit::query::element_bounds)
            .next()
            .unwrap_or(content)
    } else {
        content
    };
    let pad = opts.padding;
    let region = Rect::new(region.x0 - pad, region.y0 - pad, region.x1 + pad, region.y1 + pad);
    let (w, h) = ((region.width() * opts.scale).round().max(1.0), (region.height() * opts.scale).round().max(1.0));
    let side = opts.max_side_px;
    if !(w.is_finite() && h.is_finite()) || w > side || h > side || w * h > MAX_AREA_PX {
        return Err(Refusal::bad_arg(
            id,
            "scale",
            format!(
                "the picture would be {w} by {h} pixels, and the limit is {side} on a side and {MAX_AREA_PX} pixels in all. Lower `scale` or use fewer elements"
            ),
        ));
    }
    let (wp, hp) = (w as u32, h as u32);
    let req = RenderRequest {
        region: (region.x0, region.y0, region.width(), region.height()),
        size_px: (wp, hp),
        background: opts.background,
        overlay: false,
        selection: Vec::new(),
        background_color: doc.background,
        dark: opts.dark,
    };
    let bytes = match s.ports.renderer.as_ref() {
        Some(r) => r.render_png(doc, &req).map_err(|e| Refusal::io_failed(id, "render", "the picture", &e.to_string()))?,
        None => return Err(Refusal::internal(id, "the renderer disappeared")),
    };
    Ok(RenderedPng { bytes, width: wp, height: hp })
}

pub(super) fn export(s: &mut Session, a: ExportArgs) -> Result<ExportOut, Refusal> {
    let id = s.cmd();
    let scale = args::in_range(id, "scale", a.scale.unwrap_or_else(|| s.settings.f64_or("export.scale", 2.0)), 0.25, 4.0)?;
    let padding =
        args::in_range(id, "padding", a.padding.unwrap_or_else(|| s.settings.f64_or("export.padding", 20.0)), 0.0, 1000.0)?;
    let background = a.background.unwrap_or_else(|| s.settings.bool_or("export.background", true));
    let dark = a.dark.unwrap_or_else(|| s.settings.bool_or("export.dark", false));
    let embed = a.embed_scene.unwrap_or_else(|| s.settings.bool_or("export.embed_scene", true));
    if a.format == ExportFormat::Png && !s.has_port(PortKind::Renderer) {
        return Err(no_renderer());
    }
    if let Dest::File(f) = &a.to {
        files::require_writable(s, id, f)?;
    }
    if s.store().is_empty() {
        return Err(Refusal::nothing_to_do(
            id,
            "export",
            "the document has no elements. Draw something first, or pass `ids`",
        ));
    }
    let ids = chosen(s, id, &a)?;
    let keep: HashSet<ElementId> = ids.iter().cloned().collect();
    let sub: Document = native::subset(&s.tab().doc.snapshot(), &keep);
    let elements = sub.elements.len();
    let mut width = None;
    let mut height = None;
    let mut scene_embedded = false;

    let bytes: Vec<u8> = match a.format {
        ExportFormat::Json => native::save_pretty(&sub).map_err(|e| io_failed_export(id, "the document", &e))?.into_bytes(),
        ExportFormat::Excalidraw => {
            let (text, report) = excalidraw::export_scene_with_report(&sub).map_err(|e| io_failed_export(id, "the scene", &e))?;
            for w in report.warnings {
                s.warn(w);
            }
            text.into_bytes()
        }
        ExportFormat::Svg => {
            if dark {
                s.warn("`dark` is ignored for svg: the picture uses the document's own colours");
            }
            let opts = SvgOptions {
                style: SvgStyle::Sketch,
                background,
                padding,
                scale,
                selection: None,
                embed_scene: embed,
                // SERVICES: embedded fonts come with the renderer's font registry; none are embedded yet.
                fonts: Vec::new(),
            };
            scene_embedded = embed;
            export_svg(&sub, &opts, s.services.io(), Some(s.services.math_svg()))
                .map_err(|e| io_failed_export(id, "the picture", &e))?
                .into_bytes()
        }
        ExportFormat::Png => {
            let opts = PngOpts {
                scale,
                padding,
                background,
                dark,
                clip_to_frame: a.scope == ExportScope::Frame,
                max_side_px: MAX_SIDE_PX,
            };
            let rendered = render_png(s, id, &sub, &opts)?;
            width = Some(rendered.width);
            height = Some(rendered.height);
            let png = rendered.bytes;
            if embed {
                let json = native::save(&sub).map_err(|e| io_failed_export(id, "the scene", &e))?;
                match png_meta::embed_scene(&png, &json) {
                    Ok(with) => {
                        scene_embedded = true;
                        with
                    }
                    Err(e) => {
                        s.warn(format!("the scene could not be embedded in the picture: {e}"));
                        png
                    }
                }
            } else {
                png
            }
        }
    };

    let mut out = ExportOut {
        changed: false,
        format: a.format,
        mime: a.format.mime(),
        size: bytes.len(),
        elements,
        width,
        height,
        scene_embedded,
        bytes: None,
        saved_to: None,
    };
    match a.to {
        Dest::Inline(_) => out.bytes = Some(ExportBytes { base64: base64_of(&bytes), mime: a.format.mime() }),
        Dest::File(f) => {
            files::queue_file(s, id, &f, bytes, a.format.mime())?;
            out.saved_to = Some(f);
        }
    }
    Ok(out)
}
