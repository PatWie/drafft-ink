//! Export of the scene: native JSON, SVG and PNG.
//!
//! JSON and SVG are plain functions of the store. PNG is split in two: [`Core::plan_png`] decides what to draw
//! (a Vello scene, the canvas colour, the pixel size) without a GPU, and the renderer draws it
//! ([`crate::target::Renderer::render_image`]); [`encode_png`] writes the pixels.

use std::collections::HashSet;

use drafftink_geom::kurbo::{Rect, Size, Vec2};
use drafftink_gpu::Image;
use drafftink_io::native;
use drafftink_io::png_meta::embed_scene;
use drafftink_io::svg::{SvgOptions, SvgStyle, export_svg};
use drafftink_render::{PaintIndex, RenderOptions, Scene, SceneComposer, View, build_paint_index};
use drafftink_scene::{ElementId, SceneRead};
use serde::Deserialize;

use crate::core::{Core, parse_ids};
use crate::error::ApiError;
use crate::options::ThemeOption;
use crate::target::Renderer;

/// The look of an SVG export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SvgLook {
    /// Hand-drawn, per each element's sloppiness.
    #[default]
    Sketch,
    /// Precise lines.
    Clean,
}

/// Options of `toSVG`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SvgExport {
    /// Hand-drawn or precise.
    pub look: SvgLook,
    /// Paint the canvas colour behind the drawing.
    pub background: bool,
    /// Space around the drawing in world units.
    pub padding: Option<f64>,
    /// Scale of the `width` and `height` attributes.
    pub scale: Option<f64>,
    /// Export only these elements (and the text bound to them).
    pub selection: Option<Vec<String>>,
    /// Embed the scene so the SVG can be loaded again.
    pub embed_scene: bool,
}

impl Default for SvgExport {
    fn default() -> Self {
        Self { look: SvgLook::Sketch, background: true, padding: None, scale: None, selection: None, embed_scene: true }
    }
}

/// Options of `toPNG`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PngOptions {
    /// Pixels per world unit.
    pub scale: f64,
    /// Space around the drawing in world units.
    pub padding: f64,
    /// Paint the canvas colour; `false` leaves the background transparent.
    pub background: bool,
    /// Export only these elements.
    pub selection: Option<Vec<String>>,
    /// Light or dark; defaults to the stage's theme.
    pub theme: Option<ThemeOption>,
    /// Store the scene in the PNG so it can be loaded again.
    pub embed_scene: bool,
}

impl Default for PngOptions {
    fn default() -> Self {
        Self { scale: 1.0, padding: 20.0, background: true, selection: None, theme: None, embed_scene: true }
    }
}

/// What the renderer has to draw for a PNG, worked out without a GPU.
pub struct PngPlan {
    scene: Scene,
    base: peniko::Color,
    size: [u32; 2],
    /// The scene as native JSON, for embedding.
    embedded: Option<String>,
}

impl PngPlan {
    /// The image size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Draws the plan and encodes the PNG.
    pub async fn render(self, renderer: &mut Renderer) -> Result<Vec<u8>, ApiError> {
        let image = renderer.render_image(&self.scene, self.base, self.size).await?;
        encode_png(&image, self.embedded.as_deref())
    }
}

impl Core {
    /// The scene as native JSON.
    pub fn to_json(&self, pretty: bool) -> Result<String, ApiError> {
        let doc = self.document();
        Ok(if pretty { native::save_pretty(&doc)? } else { native::save(&doc)? })
    }

    /// The scene (or a selection of it) as SVG.
    pub fn to_svg(&mut self, opts: &SvgExport) -> Result<String, ApiError> {
        let selection = opts.selection.as_deref().map(parse_ids).transpose()?;
        let mut svg = SvgOptions {
            style: match opts.look {
                SvgLook::Sketch => SvgStyle::Sketch,
                SvgLook::Clean => SvgStyle::Clean,
            },
            background: opts.background,
            selection,
            embed_scene: opts.embed_scene,
            ..SvgOptions::default()
        };
        if let Some(padding) = opts.padding {
            svg.padding = padding;
        }
        if let Some(scale) = opts.scale {
            svg.scale = scale;
        }
        let doc = self.document();
        let text = self.fonts.svg_text();
        Ok(export_svg(&doc, &svg, &text, Some(&text))?)
    }

    /// Works out what to draw for a PNG of at most `max_side` pixels on a side.
    pub fn plan_png(&mut self, opts: &PngOptions, max_side: u32) -> Result<PngPlan, ApiError> {
        if !(opts.scale.is_finite() && opts.scale > 0.0 && opts.padding.is_finite() && opts.padding >= 0.0) {
            return Err(ApiError::invalid("scale must be above zero and padding must be zero or more"));
        }
        let selection: Option<Vec<ElementId>> = opts.selection.as_deref().map(parse_ids).transpose()?;
        let store = self.canvas.store();
        let content = self
            .frame
            .content_bounds(store, selection.as_deref())
            .ok_or_else(|| ApiError::invalid("there is nothing to export: no visible elements"))?;
        let area = Rect::new(
            content.x0 - opts.padding,
            content.y0 - opts.padding,
            content.x1 + opts.padding,
            content.y1 + opts.padding,
        );
        let pixels = |len: f64| (len * opts.scale).ceil().max(1.0);
        let (w, h) = (pixels(area.width()), pixels(area.height()));
        if w > f64::from(max_side) || h > f64::from(max_side) {
            return Err(ApiError::invalid(format!(
                "the image would be {w} by {h} pixels and this device draws at most {max_side} on a side; use a smaller scale"
            )));
        }
        let size = [w as u32, h as u32];
        let theme = opts.theme.map_or_else(|| self.frame.theme(), Into::into);
        let hidden: Vec<ElementId> = match &selection {
            None => Vec::new(),
            Some(keep) => {
                let keep: HashSet<&ElementId> = keep.iter().collect();
                store.iter().filter(|e| !keep.contains(&e.id)).map(|e| e.id.clone()).collect()
            }
        };
        let mut composer = SceneComposer::new();
        composer.set_options(RenderOptions::for_export(theme));
        composer.set_hidden(hidden);
        let index: PaintIndex = build_paint_index(store);
        let view = View::new(opts.scale, Vec2::new(-area.x0 * opts.scale, -area.y0 * opts.scale), Size::new(w, h));
        let mut scene = Scene::new();
        composer.compose(store, &index, &view, self.fonts.typesetter(), &mut scene);
        let base = if opts.background { composer.background(store) } else { peniko::Color::TRANSPARENT };
        let embedded = if opts.embed_scene {
            let mut doc = self.document();
            if let Some(keep) = &selection {
                doc = native::subset(&doc, &keep.iter().cloned().collect());
            }
            Some(native::save(&doc)?)
        } else {
            None
        };
        Ok(PngPlan { scene, base, size, embedded })
    }
}

/// Writes an image as PNG, with the scene in a text chunk when given.
pub(crate) fn encode_png(image: &Image, scene_json: Option<&str>) -> Result<Vec<u8>, ApiError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&image.rgba)?;
    }
    match scene_json {
        Some(json) => Ok(embed_scene(&bytes, json)?),
        None => Ok(bytes),
    }
}
