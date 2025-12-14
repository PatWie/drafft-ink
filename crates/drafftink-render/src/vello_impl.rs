//! Vello-based renderer implementation.

use crate::renderer::{RenderContext, Renderer, ShapeRenderer};
use crate::text_editor::TextEditState;
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as KurboShape, Stroke};
use parley::layout::PositionedLayoutItem;
use parley::{FontContext, LayoutContext};
use peniko::{Brush, Color, Fill};
use drafftink_core::selection::{get_handles, Handle, HandleKind};
use drafftink_core::shapes::{Shape, ShapeStyle, ShapeTrait};
use drafftink_core::snap::{SnapTarget, SnapTargetKind};
use vello::Scene;

/// Result of PNG rendering - contains the raw RGBA pixel data and dimensions.
#[derive(Debug)]
pub struct PngRenderResult {
    /// RGBA pixel data (4 bytes per pixel).
    pub rgba_data: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Embedded GelPen fonts (Regular, Light, Heavy variants)
static GELPEN_REGULAR: &[u8] = include_bytes!("../assets/GelPen.ttf");
static GELPEN_LIGHT: &[u8] = include_bytes!("../assets/GelPenLight.ttf");
static GELPEN_HEAVY: &[u8] = include_bytes!("../assets/GelPenHeavy.ttf");
/// Embedded Roboto fonts (Light, Regular, Bold)
static ROBOTO_LIGHT: &[u8] = include_bytes!("../assets/Roboto-Light.ttf");
static ROBOTO_REGULAR: &[u8] = include_bytes!("../assets/Roboto-Regular.ttf");
static ROBOTO_BOLD: &[u8] = include_bytes!("../assets/Roboto-Bold.ttf");
/// Embedded Architects Daughter font for handwritten style
static ARCHITECTS_DAUGHTER: &[u8] = include_bytes!("../assets/ArchitectsDaughter.ttf");

/// Vello-based renderer for GPU-accelerated 2D graphics.
pub struct VelloRenderer {
    /// The Vello scene being built.
    scene: Scene,
    /// Selection highlight color.
    selection_color: Color,
    /// Font context for text rendering (cached to avoid re-registering fonts).
    font_cx: FontContext,
    /// Layout context for text rendering.
    layout_cx: LayoutContext<Brush>,
    /// Current zoom level (for zoom-independent UI elements).
    zoom: f64,
    /// Image cache to avoid re-decoding images every frame.
    /// Key is the shape ID (as string), value is the decoded peniko ImageData.
    image_cache: std::collections::HashMap<String, peniko::ImageData>,
}

impl Default for VelloRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a Parley BoundingBox to a Kurbo Rect.
fn convert_rect(rect: &parley::BoundingBox) -> Rect {
    Rect::new(rect.x0, rect.y0, rect.x1, rect.y1)
}

/// Simple seeded random number generator (xorshift32).
/// Used for deterministic hand-drawn effects.
struct SimpleRng {
    state: u32,
}

impl SimpleRng {
    fn new(seed: u32) -> Self {
        Self { state: seed.max(1) }
    }
    
    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }
    
    /// Random float in range [-1, 1]
    fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64 / u32::MAX as f64) * 2.0 - 1.0
    }
    
    /// Random offset scaled by amount
    fn offset(&mut self, amount: f64) -> f64 {
        self.next_f64() * amount
    }
}

/// Apply hand-drawn effect to a path based on roughness level.
/// This mimics the Excalidraw/rough.js algorithm:
/// - Endpoints are randomly offset (lines overshoot/undershoot at corners)
/// - Lines have a slight bow (curve in the middle)
/// - Each stroke_index produces completely different randomness
/// 
/// roughness: 0 = clean, 1 = slight wobble, 2 = very sketchy
/// seed: stable random seed from the shape's style (persisted, doesn't change on transform)
/// stroke_index: 0 or 1 for multi-stroke effect (different random offsets)
fn apply_hand_drawn_effect(path: &BezPath, roughness: f64, zoom: f64, seed: u32, stroke_index: u32) -> BezPath {
    if roughness <= 0.0 {
        return path.clone();
    }
    
    // Scale effect inversely with zoom so it looks consistent at all zoom levels
    let scale = 1.0 / zoom.sqrt();
    
    // Values tuned to match Excalidraw/rough.js feel
    // These create the "overshoot" effect at corners
    let max_randomness_offset = roughness * 2.0 * scale;
    let bowing = roughness * 1.0;
    
    // Use the shape's stable seed combined with stroke_index for deterministic randomness
    // The seed is stored in the shape's style, so it doesn't change when the shape is transformed
    let combined_seed = seed.wrapping_add(stroke_index.wrapping_mul(99991)); // Large prime for very different sequences
    let mut rng = SimpleRng::new(combined_seed);
    
    let mut result = BezPath::new();
    let mut last_point = Point::ZERO;
    
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => {
                // Offset the start point
                let wobbled = Point::new(
                    p.x + rng.offset(max_randomness_offset),
                    p.y + rng.offset(max_randomness_offset),
                );
                result.move_to(wobbled);
                last_point = *p;
            }
            PathEl::LineTo(p) => {
                // This is the key rough.js algorithm for lines:
                // 1. Calculate line length
                // 2. Add bowing (perpendicular offset at midpoint)
                // 3. Offset both endpoints randomly (creates overshoot)
                
                let dx = p.x - last_point.x;
                let dy = p.y - last_point.y;
                let len = (dx * dx + dy * dy).sqrt();
                
                // Calculate bowing amount - proportional to length
                let bow_offset = bowing * roughness * len / 200.0;
                let bow = rng.offset(bow_offset) * scale;
                
                // Perpendicular vector for bowing
                let (perp_x, perp_y) = if len > 0.001 {
                    (-dy / len, dx / len)
                } else {
                    (0.0, 0.0)
                };
                
                // Control point with bowing
                let mid_x = (last_point.x + p.x) / 2.0 + perp_x * bow;
                let mid_y = (last_point.y + p.y) / 2.0 + perp_y * bow;
                
                // End point with random offset (creates overshoot at corners)
                let end = Point::new(
                    p.x + rng.offset(max_randomness_offset),
                    p.y + rng.offset(max_randomness_offset),
                );
                
                // Use quadratic bezier for the bowed line
                result.quad_to(Point::new(mid_x, mid_y), end);
                last_point = *p;
            }
            PathEl::QuadTo(p1, p2) => {
                let wobbled_p1 = Point::new(
                    p1.x + rng.offset(max_randomness_offset * 0.7),
                    p1.y + rng.offset(max_randomness_offset * 0.7),
                );
                let wobbled_p2 = Point::new(
                    p2.x + rng.offset(max_randomness_offset),
                    p2.y + rng.offset(max_randomness_offset),
                );
                result.quad_to(wobbled_p1, wobbled_p2);
                last_point = *p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                let wobbled_p1 = Point::new(
                    p1.x + rng.offset(max_randomness_offset * 0.5),
                    p1.y + rng.offset(max_randomness_offset * 0.5),
                );
                let wobbled_p2 = Point::new(
                    p2.x + rng.offset(max_randomness_offset * 0.5),
                    p2.y + rng.offset(max_randomness_offset * 0.5),
                );
                let wobbled_p3 = Point::new(
                    p3.x + rng.offset(max_randomness_offset),
                    p3.y + rng.offset(max_randomness_offset),
                );
                result.curve_to(wobbled_p1, wobbled_p2, wobbled_p3);
                last_point = *p3;
            }
            PathEl::ClosePath => {
                // Don't close - let the overshoot show at the closing corner too
                // The path visually closes but endpoints won't match perfectly
                result.close_path();
            }
        }
    }
    
    result
}

impl VelloRenderer {
    /// Create a new Vello renderer.
    pub fn new() -> Self {
        let mut font_cx = FontContext::new();
        // Register all font variants
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(GELPEN_REGULAR)),
            None,
        );
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(GELPEN_LIGHT)),
            None,
        );
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(GELPEN_HEAVY)),
            None,
        );
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(ROBOTO_LIGHT)),
            None,
        );
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(ROBOTO_REGULAR)),
            None,
        );
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(ROBOTO_BOLD)),
            None,
        );
        font_cx.collection.register_fonts(
            vello::peniko::Blob::new(std::sync::Arc::new(ARCHITECTS_DAUGHTER)),
            None,
        );
        
        Self {
            scene: Scene::new(),
            selection_color: Color::from_rgba8(59, 130, 246, 255),
            font_cx,
            layout_cx: LayoutContext::new(),
            zoom: 1.0,
            image_cache: std::collections::HashMap::new(),
        }
    }

    /// Get the built scene for rendering.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Take ownership of the scene (resets internal scene).
    pub fn take_scene(&mut self) -> Scene {
        std::mem::take(&mut self.scene)
    }

    /// Get mutable references to both font and layout contexts for text editing.
    pub fn contexts_mut(&mut self) -> (&mut FontContext, &mut LayoutContext<Brush>) {
        (&mut self.font_cx, &mut self.layout_cx)
    }
    
    /// Build a scene for export (shapes only, no grid/selection/guides).
    /// Returns the scene and the scaled bounds (for texture dimensions).
    /// 
    /// `scale` is the export resolution multiplier (1 = 1x, 2 = 2x, 3 = 3x).
    pub fn build_export_scene(&mut self, document: &drafftink_core::canvas::CanvasDocument, scale: f64) -> (Scene, Option<Rect>) {
        self.scene.reset();
        self.zoom = scale;
        
        let bounds = document.bounds();
        
        // If no shapes, return empty scene
        if bounds.is_none() {
            return (std::mem::take(&mut self.scene), None);
        }
        
        let bounds = bounds.unwrap();
        
        // Add padding around the content (in logical pixels)
        let padding = 20.0;
        let padded_bounds = bounds.inflate(padding, padding);
        
        // Transform: translate to origin, then scale up
        let transform = Affine::scale(scale)
            * Affine::translate((-padded_bounds.x0, -padded_bounds.y0));
        
        // Scaled output dimensions
        let scaled_width = padded_bounds.width() * scale;
        let scaled_height = padded_bounds.height() * scale;
        
        // Fill background with white (at scaled size)
        let bg_rect = Rect::new(0.0, 0.0, scaled_width, scaled_height);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::WHITE,
            None,
            &bg_rect,
        );
        
        // Render all shapes with scaled transform
        for shape in document.shapes_ordered() {
            self.render_shape(shape, transform, false);
        }
        
        // Return scaled bounds for texture dimensions
        let scaled_bounds = Rect::new(0.0, 0.0, scaled_width, scaled_height);
        (std::mem::take(&mut self.scene), Some(scaled_bounds))
    }
    
    /// Build a scene for exporting selected shapes only.
    /// 
    /// `scale` is the export resolution multiplier (1 = 1x, 2 = 2x, 3 = 3x).
    pub fn build_export_scene_selection(
        &mut self, 
        document: &drafftink_core::canvas::CanvasDocument,
        selection: &[drafftink_core::shapes::ShapeId],
        scale: f64,
    ) -> (Scene, Option<Rect>) {
        self.scene.reset();
        self.zoom = scale;
        
        if selection.is_empty() {
            return (std::mem::take(&mut self.scene), None);
        }
        
        // Calculate bounds of selected shapes
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        
        let mut shapes_to_render = Vec::new();
        for &shape_id in selection {
            if let Some(shape) = document.get_shape(shape_id) {
                let b = shape.bounds();
                min_x = min_x.min(b.x0);
                min_y = min_y.min(b.y0);
                max_x = max_x.max(b.x1);
                max_y = max_y.max(b.y1);
                shapes_to_render.push(shape);
            }
        }
        
        if shapes_to_render.is_empty() {
            return (std::mem::take(&mut self.scene), None);
        }
        
        let bounds = Rect::new(min_x, min_y, max_x, max_y);
        
        // Add padding around the content (in logical pixels)
        let padding = 20.0;
        let padded_bounds = bounds.inflate(padding, padding);
        
        // Transform: translate to origin, then scale up
        let transform = Affine::scale(scale)
            * Affine::translate((-padded_bounds.x0, -padded_bounds.y0));
        
        // Scaled output dimensions
        let scaled_width = padded_bounds.width() * scale;
        let scaled_height = padded_bounds.height() * scale;
        
        // Fill background with white (at scaled size)
        let bg_rect = Rect::new(0.0, 0.0, scaled_width, scaled_height);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::WHITE,
            None,
            &bg_rect,
        );
        
        // Render selected shapes with scaled transform
        for shape in shapes_to_render {
            self.render_shape(shape, transform, false);
        }
        
        // Return scaled bounds for texture dimensions
        let scaled_bounds = Rect::new(0.0, 0.0, scaled_width, scaled_height);
        (std::mem::take(&mut self.scene), Some(scaled_bounds))
    }

    /// Render a shape path with the given style.
    fn render_path(&mut self, path: &BezPath, style: &ShapeStyle, transform: Affine) {
        let roughness = style.sloppiness.roughness();
        let seed = style.seed;
        
        // Fill if present (use clean path for fill)
        if let Some(fill_color) = style.fill() {
            let fill_path = if roughness > 0.0 {
                apply_hand_drawn_effect(path, roughness * 0.3, self.zoom, seed, 0)
            } else {
                path.clone()
            };
            self.scene.fill(
                Fill::NonZero,
                transform,
                fill_color,
                None,
                &fill_path,
            );
        }

        // For hand-drawn style, draw multiple strokes like rough.js
        if roughness > 0.0 {
            let stroke = Stroke::new(style.stroke_width);
            
            // First stroke
            let path1 = apply_hand_drawn_effect(path, roughness, self.zoom, seed, 0);
            self.scene.stroke(
                &stroke,
                transform,
                style.stroke(),
                None,
                &path1,
            );
            
            // Second stroke with different seed - this creates the "sketchy" double-line effect
            let path2 = apply_hand_drawn_effect(path, roughness, self.zoom, seed, 1);
            self.scene.stroke(
                &stroke,
                transform,
                style.stroke(),
                None,
                &path2,
            );
        } else {
            // Clean stroke for Architect mode
            let stroke = Stroke::new(style.stroke_width);
            self.scene.stroke(
                &stroke,
                transform,
                style.stroke(),
                None,
                path,
            );
        }
    }

    /// Render a text shape using Parley for proper text layout.
    fn render_text(&mut self, text: &drafftink_core::shapes::Text, transform: Affine) {
        use parley::layout::PositionedLayoutItem;
        use parley::StyleProperty;
        
        // Skip empty text
        if text.content.is_empty() {
            // Draw a placeholder cursor/caret for empty text (position is top-left)
            let cursor_height = text.font_size * 1.2;
            let cursor = kurbo::Line::new(
                Point::new(text.position.x, text.position.y),
                Point::new(text.position.x, text.position.y + cursor_height),
            );
            let stroke = Stroke::new(2.0);
            self.scene.stroke(&stroke, transform, Color::from_rgba8(100, 100, 100, 200), None, &cursor);
            return;
        }
        
        use drafftink_core::shapes::{FontFamily, FontWeight};
        
        let style = &text.style;
        let brush = Brush::Solid(style.stroke());
        let font_size = text.font_size as f32;
        
        // Determine font name based on family and weight
        // Font names must match the name table in the TTF files
        // For GelPen: each weight is a separate font family (no weight metadata)
        // For Roboto: Use "Roboto" family with proper weight metadata
        //   - Roboto-Light.ttf has weight 300 (LIGHT)
        //   - Roboto-Regular.ttf has weight 400 (NORMAL)
        //   - Roboto-Bold.ttf has weight 700 (BOLD)
        // For ArchitectsDaughter: single font only
        let (font_name, parley_weight) = match (&text.font_family, &text.font_weight) {
            (FontFamily::GelPen, FontWeight::Light) => ("GelPenLight", parley::FontWeight::NORMAL),
            (FontFamily::GelPen, FontWeight::Regular) => ("GelPen", parley::FontWeight::NORMAL),
            (FontFamily::GelPen, FontWeight::Heavy) => ("GelPenHeavy", parley::FontWeight::NORMAL),
            (FontFamily::Roboto, FontWeight::Light) => ("Roboto", parley::FontWeight::LIGHT),
            (FontFamily::Roboto, FontWeight::Regular) => ("Roboto", parley::FontWeight::NORMAL),
            (FontFamily::Roboto, FontWeight::Heavy) => ("Roboto", parley::FontWeight::BOLD),
            (FontFamily::ArchitectsDaughter, _) => ("Architects Daughter", parley::FontWeight::NORMAL),
        };
        
        // Build the layout using cached font context
        let mut builder = self.layout_cx.ranged_builder(&mut self.font_cx, &text.content, 1.0, false);
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::Brush(brush.clone()));
        builder.push_default(StyleProperty::FontWeight(parley_weight));
        builder.push_default(StyleProperty::FontStack(parley::FontStack::Single(
            parley::FontFamily::Named(font_name.into())
        )));
        let mut layout = builder.build(&text.content);
        
        // Compute layout (no max width constraint for now)
        layout.break_all_lines(None);
        layout.align(None, parley::Alignment::Start, parley::AlignmentOptions::default());
        
        // Cache the computed layout dimensions for accurate bounds
        let layout_width = layout.width() as f64;
        let layout_height = layout.height() as f64;
        text.set_cached_size(layout_width, layout_height);
        
        // Create transform to position text
        // text.position is where user clicked - treat as top-left of text box
        // Parley layouts have y=0 at top, with baseline offset down
        let text_transform = transform * Affine::translate((text.position.x, text.position.y));
        
        // Count glyphs for debugging
        let mut glyph_count = 0;
        
        // Render each line (adapted from Parley's vello example)
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let mut x = glyph_run.offset();
                let y = glyph_run.baseline();
                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                let synthesis = run.synthesis();
                let glyph_xform = synthesis
                    .skew()
                    .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
                
                let glyphs: Vec<vello::Glyph> = glyph_run.glyphs().map(|glyph| {
                    let gx = x + glyph.x;
                    let gy = y - glyph.y;
                    x += glyph.advance;
                    glyph_count += 1;
                    vello::Glyph {
                        id: glyph.id,
                        x: gx,
                        y: gy,
                    }
                }).collect();
                
                if !glyphs.is_empty() {
                    self.scene
                        .draw_glyphs(font)
                        .brush(&brush)
                        .hint(true)
                        .transform(text_transform)
                        .glyph_transform(glyph_xform)
                        .font_size(font_size)
                        .normalized_coords(run.normalized_coords())
                        .draw(Fill::NonZero, glyphs.into_iter());
                }
            }
        }
        
        // If no glyphs were rendered (font not found), draw a fallback rectangle
        if glyph_count == 0 {
            // Approximate bounds (position is top-left)
            let width = text.content.len() as f64 * text.font_size * 0.6;
            let height = text.font_size * 1.2;
            let rect = Rect::new(
                text.position.x,
                text.position.y,
                text.position.x + width.max(20.0),
                text.position.y + height,
            );
            self.scene.fill(Fill::NonZero, transform, Color::from_rgba8(255, 100, 100, 100), None, &rect);
        }
    }

    /// Render an image shape.
    fn render_image(&mut self, image: &drafftink_core::shapes::Image, transform: Affine) {
        use std::sync::Arc;
        
        let id_str = image.id().to_string();
        
        // Check if we have a cached decoded image
        let image_data = if let Some(cached) = self.image_cache.get(&id_str) {
            cached.clone()
        } else {
            // Decode the image data
            if let Some(raw_data) = image.data() {
                // Try to decode using the image crate
                if let Ok(decoded) = ::image::load_from_memory(&raw_data) {
                    let rgba = decoded.to_rgba8();
                    let (width, height) = rgba.dimensions();
                    let blob = peniko::Blob::new(Arc::new(rgba.into_vec()));
                    let img_data = peniko::ImageData {
                        data: blob,
                        format: peniko::ImageFormat::Rgba8,
                        width,
                        height,
                        alpha_type: peniko::ImageAlphaType::Alpha,
                    };
                    self.image_cache.insert(id_str.clone(), img_data.clone());
                    img_data
                } else {
                    // Failed to decode - draw placeholder
                    self.render_image_placeholder(image, transform);
                    return;
                }
            } else {
                // No data - draw placeholder
                self.render_image_placeholder(image, transform);
                return;
            }
        };
        
        // Calculate the transform to scale and position the image
        let bounds = image.bounds();
        let scale_x = bounds.width() / image_data.width as f64;
        let scale_y = bounds.height() / image_data.height as f64;
        
        let image_transform = transform
            * Affine::translate((bounds.x0, bounds.y0))
            * Affine::scale_non_uniform(scale_x, scale_y);
        
        self.scene.draw_image(&image_data.into(), image_transform);
    }
    
    /// Render a placeholder for images that couldn't be loaded.
    fn render_image_placeholder(&mut self, image: &drafftink_core::shapes::Image, transform: Affine) {
        let bounds = image.bounds();
        
        // Draw a gray rectangle with an X
        let rect_path = bounds.to_path(0.1);
        self.scene.fill(Fill::NonZero, transform, Color::from_rgba8(200, 200, 200, 255), None, &rect_path);
        
        // Draw diagonal lines (X pattern)
        let stroke = Stroke::new(2.0);
        let mut x_path = BezPath::new();
        x_path.move_to(Point::new(bounds.x0, bounds.y0));
        x_path.line_to(Point::new(bounds.x1, bounds.y1));
        x_path.move_to(Point::new(bounds.x1, bounds.y0));
        x_path.line_to(Point::new(bounds.x0, bounds.y1));
        self.scene.stroke(&stroke, transform, Color::from_rgba8(150, 150, 150, 255), None, &x_path);
        
        // Draw border
        self.scene.stroke(&stroke, transform, Color::from_rgba8(100, 100, 100, 255), None, &rect_path);
    }

    /// Render a text shape in edit mode using PlainEditor state.
    /// This renders the text with cursor and selection highlights.
    pub fn render_text_editing(
        &mut self,
        text: &drafftink_core::shapes::Text,
        edit_state: &mut TextEditState,
        transform: Affine,
    ) {
        use drafftink_core::shapes::{FontFamily as ShapeFontFamily, FontWeight};
        
        let style = &text.style;
        let brush = Brush::Solid(style.stroke());
        
        // Determine font name and parley weight based on family and weight
        // Use same logic as render_text - all Roboto variants use "Roboto" family with weight
        let (font_name, parley_weight) = match (&text.font_family, &text.font_weight) {
            (ShapeFontFamily::GelPen, FontWeight::Light) => ("GelPenLight", parley::FontWeight::NORMAL),
            (ShapeFontFamily::GelPen, FontWeight::Regular) => ("GelPen", parley::FontWeight::NORMAL),
            (ShapeFontFamily::GelPen, FontWeight::Heavy) => ("GelPenHeavy", parley::FontWeight::NORMAL),
            (ShapeFontFamily::Roboto, FontWeight::Light) => ("Roboto", parley::FontWeight::LIGHT),
            (ShapeFontFamily::Roboto, FontWeight::Regular) => ("Roboto", parley::FontWeight::NORMAL),
            (ShapeFontFamily::Roboto, FontWeight::Heavy) => ("Roboto", parley::FontWeight::BOLD),
            (ShapeFontFamily::ArchitectsDaughter, _) => ("Architects Daughter", parley::FontWeight::NORMAL),
        };
        
        // Configure the editor styles
        edit_state.set_font_size(text.font_size as f32);
        edit_state.set_brush(brush.clone());
        
        // Set the font family and weight in the editor
        {
            use parley::{FontStack, FontFamily, StyleProperty};
            let styles = edit_state.editor_mut().edit_styles();
            styles.insert(StyleProperty::FontStack(FontStack::Single(
                FontFamily::Named(font_name.into())
            )));
            styles.insert(StyleProperty::FontWeight(parley_weight));
        }
        
        // Create transform to position text at the shape's position
        let text_transform = transform * Affine::translate((text.position.x, text.position.y));
        
        // IMPORTANT: First compute the layout - this must happen before cursor/selection geometry
        let layout = edit_state.editor_mut().layout(&mut self.font_cx, &mut self.layout_cx);
        
        // Render glyphs first (text content)
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let glyph_style = glyph_run.style();
                let mut x = glyph_run.offset();
                let y = glyph_run.baseline();
                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                let synthesis = run.synthesis();
                let glyph_xform = synthesis
                    .skew()
                    .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
                
                let glyphs: Vec<vello::Glyph> = glyph_run.glyphs().map(|glyph| {
                    let gx = x + glyph.x;
                    let gy = y - glyph.y;
                    x += glyph.advance;
                    vello::Glyph {
                        id: glyph.id,
                        x: gx,
                        y: gy,
                    }
                }).collect();
                
                if !glyphs.is_empty() {
                    self.scene
                        .draw_glyphs(font)
                        .brush(&glyph_style.brush)
                        .hint(true)
                        .transform(text_transform)
                        .glyph_transform(glyph_xform)
                        .font_size(font_size)
                        .normalized_coords(run.normalized_coords())
                        .draw(Fill::NonZero, glyphs.into_iter());
                }
            }
        }
        
        // Selection color (semi-transparent blue)
        let selection_color = Color::from_rgba8(70, 130, 180, 128); // STEEL_BLUE-ish
        
        // Draw selection background (now layout is computed)
        edit_state.editor().selection_geometry_with(|rect, _| {
            self.scene.fill(
                Fill::NonZero,
                text_transform,
                selection_color,
                None,
                &convert_rect(&rect),
            );
        });
        
        // Draw cursor if visible (now layout is computed)
        if edit_state.is_cursor_visible() {
            if let Some(cursor) = edit_state.editor().cursor_geometry(1.5) {
                // Cursor color (contrasting with text)
                let cursor_color = Color::from_rgba8(0, 0, 0, 255);
                self.scene.fill(
                    Fill::NonZero,
                    text_transform,
                    cursor_color,
                    None,
                    &convert_rect(&cursor),
                );
            } else if edit_state.text().is_empty() {
                // If text is empty, show a placeholder cursor at origin
                let cursor_height = text.font_size * 1.2;
                let cursor_rect = Rect::new(0.0, 0.0, 1.5, cursor_height);
                self.scene.fill(
                    Fill::NonZero,
                    text_transform,
                    Color::from_rgba8(0, 0, 0, 255),
                    None,
                    &cursor_rect,
                );
            }
        }
    }

    /// Render shape-specific selection handles.
    /// Handles are scaled inversely with zoom to maintain constant screen size.
    fn render_shape_handles(&mut self, shape: &Shape, transform: Affine) {
        let handles = get_handles(shape);
        // Scale handle size inversely with zoom to maintain constant screen size
        let handle_size = 8.0 / self.zoom;
        let stroke_width = 1.0 / self.zoom;
        let dash_len = 4.0 / self.zoom;

        // For lines/arrows, draw a light dashed line connecting the endpoints
        // For rectangles/ellipses, draw the bounding box
        match shape {
            Shape::Line(_) | Shape::Arrow(_) => {
                // Just draw the endpoint handles, no bounding box
            }
            _ => {
                // Draw selection rectangle for non-line shapes
                let bounds = shape.bounds();
                let stroke = Stroke::new(stroke_width).with_dashes(0.0, &[dash_len, dash_len]);
                let mut path = BezPath::new();
                path.move_to(Point::new(bounds.x0, bounds.y0));
                path.line_to(Point::new(bounds.x1, bounds.y0));
                path.line_to(Point::new(bounds.x1, bounds.y1));
                path.line_to(Point::new(bounds.x0, bounds.y1));
                path.close_path();
                
                self.scene.stroke(
                    &stroke,
                    transform,
                    self.selection_color,
                    None,
                    &path,
                );
            }
        }

        // Draw handles
        for handle in handles {
            self.render_handle(&handle, transform, handle_size);
        }
    }

    /// Render a single handle.
    /// Stroke widths are scaled inversely with zoom to maintain constant screen size.
    fn render_handle(&mut self, handle: &Handle, transform: Affine, size: f64) {
        let pos = handle.position;
        let stroke_width_thick = 2.0 / self.zoom;
        let stroke_width_thin = 1.5 / self.zoom;
        
        // Different handle shapes based on type
        match handle.kind {
            HandleKind::Endpoint(_) => {
                // Circle handle for endpoints (lines/arrows)
                let radius = size / 2.0;
                let ellipse = kurbo::Ellipse::new(pos, (radius, radius), 0.0);
                let path = ellipse.to_path(0.1);
                
                // White fill
                self.scene.fill(
                    Fill::NonZero,
                    transform,
                    Color::WHITE,
                    None,
                    &path,
                );
                
                // Blue border
                self.scene.stroke(
                    &Stroke::new(stroke_width_thick),
                    transform,
                    self.selection_color,
                    None,
                    &path,
                );
            }
            HandleKind::Corner(_) | HandleKind::Edge(_) => {
                // Square handle for corners/edges
                let half = size / 2.0;
                let rect = Rect::new(
                    pos.x - half,
                    pos.y - half,
                    pos.x + half,
                    pos.y + half,
                );
                let path = rect.to_path(0.1);
                
                // White fill
                self.scene.fill(
                    Fill::NonZero,
                    transform,
                    Color::WHITE,
                    None,
                    &path,
                );
                
                // Blue border
                self.scene.stroke(
                    &Stroke::new(stroke_width_thin),
                    transform,
                    self.selection_color,
                    None,
                    &path,
                );
            }
        }
    }
}

impl Renderer for VelloRenderer {
    fn build_scene(&mut self, ctx: &RenderContext) {
        // Clear the scene
        self.scene.reset();
        self.selection_color = ctx.selection_color;
        self.zoom = ctx.canvas.camera.zoom;

        let camera_transform = ctx.canvas.camera.transform();

        // Draw grid based on style
        use crate::renderer::GridStyle;
        match ctx.grid_style {
            GridStyle::None => {}
            GridStyle::Lines => {
                self.render_grid_lines(
                    Rect::new(0.0, 0.0, ctx.viewport_size.width, ctx.viewport_size.height),
                    camera_transform,
                    20.0,
                );
            }
            GridStyle::CrossPlus => {
                self.render_grid_crosses(
                    Rect::new(0.0, 0.0, ctx.viewport_size.width, ctx.viewport_size.height),
                    camera_transform,
                    20.0,
                );
            }
            GridStyle::Dots => {
                self.render_grid_dots(
                    Rect::new(0.0, 0.0, ctx.viewport_size.width, ctx.viewport_size.height),
                    camera_transform,
                    20.0,
                );
            }
        }

        // Draw all shapes in z-order (skip shape being edited - it's rendered separately)
        for shape in ctx.canvas.document.shapes_ordered() {
            // Skip shape being edited (will be rendered with cursor/selection separately)
            if ctx.editing_shape_id == Some(shape.id()) {
                continue;
            }
            let is_selected = ctx.canvas.is_selected(shape.id());
            self.render_shape(shape, camera_transform, is_selected);
        }

        // Draw preview shape if tool is active
        if let Some(preview) = ctx.canvas.tool_manager.preview_shape() {
            self.render_shape(&preview, camera_transform, false);
        }

        // Draw selection rectangle (marquee)
        if let Some(rect) = ctx.selection_rect {
            self.render_selection_rect(rect, camera_transform);
        }

        // Draw nearby snap targets (small indicators on shapes)
        if !ctx.nearby_snap_targets.is_empty() {
            self.render_snap_targets(&ctx.nearby_snap_targets, camera_transform);
        }

        // Draw snap guides
        if let Some(snap_point) = ctx.snap_point {
            self.render_snap_guides(snap_point, camera_transform, ctx.viewport_size);
        }

        // Draw angle snap guides (polar rays and arc)
        if let Some(ref angle_info) = ctx.angle_snap_info {
            self.render_angle_snap_guides(angle_info, camera_transform, ctx.viewport_size);
        }
    }
}

impl VelloRenderer {
    /// Render snap guides (crosshairs at the snap point).
    fn render_snap_guides(&mut self, snap_point: Point, transform: Affine, viewport_size: kurbo::Size) {
        // Snap guide color - subtle magenta/pink
        let guide_color = Color::from_rgba8(236, 72, 153, 180); // Pink-500 with alpha
        
        // Stroke width scaled inversely with zoom for constant screen appearance
        let stroke_width = 1.0 / self.zoom;
        let stroke = Stroke::new(stroke_width);
        
        // We need to draw lines in world coordinates that span the visible area
        // Get the inverse transform to convert screen bounds to world bounds
        let inv_transform = transform.inverse();
        let world_top_left = inv_transform * Point::new(0.0, 0.0);
        let world_bottom_right = inv_transform * Point::new(viewport_size.width, viewport_size.height);
        
        // Horizontal line through snap point (full width)
        let mut h_path = BezPath::new();
        h_path.move_to(Point::new(world_top_left.x, snap_point.y));
        h_path.line_to(Point::new(world_bottom_right.x, snap_point.y));
        self.scene.stroke(&stroke, transform, guide_color, None, &h_path);
        
        // Vertical line through snap point (full height)
        let mut v_path = BezPath::new();
        v_path.move_to(Point::new(snap_point.x, world_top_left.y));
        v_path.line_to(Point::new(snap_point.x, world_bottom_right.y));
        self.scene.stroke(&stroke, transform, guide_color, None, &v_path);
        
        // Small circle at the intersection
        let circle_radius = 4.0 / self.zoom;
        let circle = kurbo::Circle::new(snap_point, circle_radius);
        self.scene.stroke(&Stroke::new(stroke_width * 2.0), transform, guide_color, None, &circle);
    }

    /// Render nearby snap targets as small indicators.
    fn render_snap_targets(&mut self, targets: &[SnapTarget], transform: Affine) {
        // Different colors for different target types
        let corner_color = Color::from_rgba8(59, 130, 246, 150);    // Blue for corners
        let midpoint_color = Color::from_rgba8(16, 185, 129, 150);  // Emerald for midpoints  
        let center_color = Color::from_rgba8(245, 158, 11, 150);    // Amber for centers
        
        // Size scaled inversely with zoom for constant screen appearance
        let size = 4.0 / self.zoom;
        let stroke_width = 1.0 / self.zoom;
        
        for target in targets {
            let color = match target.kind {
                SnapTargetKind::Corner => corner_color,
                SnapTargetKind::Midpoint => midpoint_color,
                SnapTargetKind::Center => center_color,
                SnapTargetKind::Edge => corner_color, // Treat edges like corners
            };
            
            match target.kind {
                SnapTargetKind::Corner | SnapTargetKind::Edge => {
                    // Draw a small square for corners
                    let half = size;
                    let rect = Rect::new(
                        target.point.x - half,
                        target.point.y - half,
                        target.point.x + half,
                        target.point.y + half,
                    );
                    self.scene.stroke(&Stroke::new(stroke_width), transform, color, None, &rect);
                }
                SnapTargetKind::Midpoint => {
                    // Draw a small diamond for midpoints
                    let mut path = BezPath::new();
                    path.move_to(Point::new(target.point.x, target.point.y - size));
                    path.line_to(Point::new(target.point.x + size, target.point.y));
                    path.line_to(Point::new(target.point.x, target.point.y + size));
                    path.line_to(Point::new(target.point.x - size, target.point.y));
                    path.close_path();
                    self.scene.stroke(&Stroke::new(stroke_width), transform, color, None, &path);
                }
                SnapTargetKind::Center => {
                    // Draw a small circle for centers
                    let circle = kurbo::Circle::new(target.point, size);
                    self.scene.stroke(&Stroke::new(stroke_width), transform, color, None, &circle);
                }
            }
        }
    }

    /// Render angle snap visualization (polar rays and angle arc).
    fn render_angle_snap_guides(
        &mut self,
        info: &crate::renderer::AngleSnapInfo,
        transform: Affine,
        viewport_size: kurbo::Size,
    ) {
        use std::f64::consts::PI;
        
        // Colors
        let ray_color = Color::from_rgba8(100, 100, 100, 60); // Very subtle gray
        let active_ray_color = Color::from_rgba8(236, 72, 153, 200); // Magenta for active snap angle
        let arc_color = Color::from_rgba8(236, 72, 153, 220); // Magenta for angle arc
        
        // Stroke widths scaled inversely with zoom
        let thin_stroke_width = 0.5 / self.zoom;
        let thick_stroke_width = 1.5 / self.zoom;
        
        let start = info.start_point;
        
        // Calculate ray length based on viewport
        let inv_transform = transform.inverse();
        let world_top_left = inv_transform * Point::new(0.0, 0.0);
        let world_bottom_right = inv_transform * Point::new(viewport_size.width, viewport_size.height);
        let viewport_diagonal = ((world_bottom_right.x - world_top_left.x).powi(2)
            + (world_bottom_right.y - world_top_left.y).powi(2))
        .sqrt();
        let ray_length = viewport_diagonal;
        
        // Draw polar rays at 15° intervals (0°, 15°, 30°, ..., 345°)
        let mut path = BezPath::new();
        for i in 0..24 {
            let angle_deg = i as f64 * 15.0;
            let angle_rad = angle_deg * PI / 180.0;
            let end_x = start.x + ray_length * angle_rad.cos();
            let end_y = start.y + ray_length * angle_rad.sin();
            path.move_to(start);
            path.line_to(Point::new(end_x, end_y));
        }
        self.scene.stroke(&Stroke::new(thin_stroke_width), transform, ray_color, None, &path);
        
        // Highlight the active snap angle ray if snapped
        if info.is_snapped {
            let angle_rad = info.angle_degrees * PI / 180.0;
            let mut active_path = BezPath::new();
            active_path.move_to(start);
            active_path.line_to(Point::new(
                start.x + ray_length * angle_rad.cos(),
                start.y + ray_length * angle_rad.sin(),
            ));
            self.scene.stroke(&Stroke::new(thick_stroke_width), transform, active_ray_color, None, &active_path);
            
            // Draw angle arc from 0° to the snapped angle
            let arc_radius = 30.0 / self.zoom;
            let segments = (info.angle_degrees.abs() / 5.0).ceil() as usize;
            let segments = segments.max(2).min(72); // At least 2, at most 72 segments
            
            if segments > 1 {
                let mut arc_path = BezPath::new();
                let start_angle = 0.0_f64;
                let end_angle = info.angle_degrees * PI / 180.0;
                
                let first_x = start.x + arc_radius * start_angle.cos();
                let first_y = start.y + arc_radius * start_angle.sin();
                arc_path.move_to(Point::new(first_x, first_y));
                
                for i in 1..=segments {
                    let t = i as f64 / segments as f64;
                    let angle = start_angle + t * (end_angle - start_angle);
                    let x = start.x + arc_radius * angle.cos();
                    let y = start.y + arc_radius * angle.sin();
                    arc_path.line_to(Point::new(x, y));
                }
                
                self.scene.stroke(&Stroke::new(thick_stroke_width), transform, arc_color, None, &arc_path);
            }
            
            // Draw angle label (e.g., "45°")
            // Position the label at the midpoint of the arc
            let label_angle = info.angle_degrees * PI / 360.0; // Half the angle
            let label_radius = arc_radius + 15.0 / self.zoom;
            let _label_pos = Point::new(
                start.x + label_radius * label_angle.cos(),
                start.y + label_radius * label_angle.sin(),
            );
            
            // Note: Text rendering would require Parley/font setup which is complex
            // For now, we skip the text label - the arc itself shows the angle visually
        }
    }

    /// Render a selection rectangle (marquee).
    /// Stroke width and dash pattern are scaled inversely with zoom.
    fn render_selection_rect(&mut self, rect: Rect, transform: Affine) {
        // Fill with semi-transparent blue
        let fill_color = Color::from_rgba8(59, 130, 246, 25);
        let mut path = BezPath::new();
        path.move_to(Point::new(rect.x0, rect.y0));
        path.line_to(Point::new(rect.x1, rect.y0));
        path.line_to(Point::new(rect.x1, rect.y1));
        path.line_to(Point::new(rect.x0, rect.y1));
        path.close_path();

        self.scene.fill(
            Fill::NonZero,
            transform,
            fill_color,
            None,
            &path,
        );

        // Stroke with blue dashed line - scale inversely with zoom
        let stroke_width = 1.0 / self.zoom;
        let dash_len = 4.0 / self.zoom;
        let stroke = Stroke::new(stroke_width).with_dashes(0.0, &[dash_len, dash_len]);
        self.scene.stroke(
            &stroke,
            transform,
            self.selection_color,
            None,
            &path,
        );
    }
}

impl VelloRenderer {
    /// Calculate grid bounds from viewport and transform.
    fn grid_bounds(&self, viewport: Rect, transform: Affine, grid_size: f64) -> (f64, f64, f64, f64) {
        let inv = transform.inverse();
        let world_tl = inv * Point::new(viewport.x0, viewport.y0);
        let world_br = inv * Point::new(viewport.x1, viewport.y1);

        let start_x = (world_tl.x / grid_size).floor() * grid_size;
        let start_y = (world_tl.y / grid_size).floor() * grid_size;
        let end_x = (world_br.x / grid_size).ceil() * grid_size;
        let end_y = (world_br.y / grid_size).ceil() * grid_size;

        (start_x, start_y, end_x, end_y)
    }

    /// Render full grid lines.
    fn render_grid_lines(&mut self, viewport: Rect, transform: Affine, grid_size: f64) {
        let grid_color = Color::from_rgba8(200, 200, 200, 100);
        let stroke = Stroke::new(0.5);

        let (start_x, start_y, end_x, end_y) = self.grid_bounds(viewport, transform, grid_size);

        // Vertical lines
        let mut x = start_x;
        while x <= end_x {
            let mut path = BezPath::new();
            path.move_to(Point::new(x, start_y));
            path.line_to(Point::new(x, end_y));
            self.scene.stroke(&stroke, transform, grid_color, None, &path);
            x += grid_size;
        }

        // Horizontal lines
        let mut y = start_y;
        while y <= end_y {
            let mut path = BezPath::new();
            path.move_to(Point::new(start_x, y));
            path.line_to(Point::new(end_x, y));
            self.scene.stroke(&stroke, transform, grid_color, None, &path);
            y += grid_size;
        }
    }

    /// Render grid as small crosses (+) at intersections.
    /// Uses batched paths for performance.
    fn render_grid_crosses(&mut self, viewport: Rect, transform: Affine, grid_size: f64) {
        let grid_color = Color::from_rgba8(180, 180, 180, 60); // Reduced opacity
        let stroke = Stroke::new(1.0);
        let cross_size = 3.0; // Half-size of the cross arms

        let (start_x, start_y, end_x, end_y) = self.grid_bounds(viewport, transform, grid_size);

        // Batch all crosses into a single path for performance
        let mut path = BezPath::new();
        
        let mut x = start_x;
        while x <= end_x {
            let mut y = start_y;
            while y <= end_y {
                // Horizontal arm
                path.move_to(Point::new(x - cross_size, y));
                path.line_to(Point::new(x + cross_size, y));
                // Vertical arm
                path.move_to(Point::new(x, y - cross_size));
                path.line_to(Point::new(x, y + cross_size));
                y += grid_size;
            }
            x += grid_size;
        }
        
        // Single draw call for all crosses
        self.scene.stroke(&stroke, transform, grid_color, None, &path);
    }

    /// Render grid as dots at intersections.
    /// Uses batched rectangles for performance.
    fn render_grid_dots(&mut self, viewport: Rect, transform: Affine, grid_size: f64) {
        let grid_color = Color::from_rgba8(160, 160, 160, 70); // Reduced opacity
        let dot_size = 1.5; // Half-size of the dot

        let (start_x, start_y, end_x, end_y) = self.grid_bounds(viewport, transform, grid_size);

        // Batch all dots into a single path for performance
        let mut path = BezPath::new();
        
        let mut x = start_x;
        while x <= end_x {
            let mut y = start_y;
            while y <= end_y {
                // Add a small square for each dot (cheaper than ellipse)
                let rect = Rect::new(
                    x - dot_size,
                    y - dot_size,
                    x + dot_size,
                    y + dot_size,
                );
                path.move_to(Point::new(rect.x0, rect.y0));
                path.line_to(Point::new(rect.x1, rect.y0));
                path.line_to(Point::new(rect.x1, rect.y1));
                path.line_to(Point::new(rect.x0, rect.y1));
                path.close_path();
                y += grid_size;
            }
            x += grid_size;
        }
        
        // Single draw call for all dots
        self.scene.fill(Fill::NonZero, transform, grid_color, None, &path);
    }

    #[allow(dead_code)]
    fn render_selection_handles(&mut self, bounds: Rect, transform: Affine) {
        let handle_size = 8.0;
        let stroke = Stroke::new(2.0);

        // Selection rectangle
        let mut path = BezPath::new();
        path.move_to(Point::new(bounds.x0, bounds.y0));
        path.line_to(Point::new(bounds.x1, bounds.y0));
        path.line_to(Point::new(bounds.x1, bounds.y1));
        path.line_to(Point::new(bounds.x0, bounds.y1));
        path.close_path();

        self.scene.stroke(
            &stroke,
            transform,
            self.selection_color,
            None,
            &path,
        );

        // Corner handles
        let corners = [
            Point::new(bounds.x0, bounds.y0),
            Point::new(bounds.x1, bounds.y0),
            Point::new(bounds.x1, bounds.y1),
            Point::new(bounds.x0, bounds.y1),
        ];

        for corner in corners {
            let handle_rect = Rect::new(
                corner.x - handle_size / 2.0,
                corner.y - handle_size / 2.0,
                corner.x + handle_size / 2.0,
                corner.y + handle_size / 2.0,
            );

            // White fill
            self.scene.fill(
                Fill::NonZero,
                transform,
                Color::WHITE,
                None,
                &handle_rect.to_path(0.1),
            );

            // Blue border
            self.scene.stroke(
                &Stroke::new(1.5),
                transform,
                self.selection_color,
                None,
                &handle_rect.to_path(0.1),
            );
        }
    }
}

impl ShapeRenderer for VelloRenderer {
    fn render_shape(&mut self, shape: &Shape, transform: Affine, selected: bool) {
        // Special handling for different shape types
        match shape {
            Shape::Text(text) => {
                self.render_text(text, transform);
            }
            Shape::Group(group) => {
                // Render each child in the group
                for child in group.children() {
                    // Children are not individually selected when the group is selected
                    self.render_shape(child, transform, false);
                }
            }
            Shape::Image(image) => {
                self.render_image(image, transform);
            }
            _ => {
                let path = shape.to_path();
                self.render_path(&path, shape.style(), transform);
            }
        }

        // Draw selection highlight with shape-specific handles
        if selected {
            self.render_shape_handles(shape, transform);
        }
    }

    fn render_grid(&mut self, viewport: Rect, transform: Affine, grid_size: f64) {
        // Default implementation - full lines
        self.render_grid_lines(viewport, transform, grid_size);
    }

    fn render_selection_handles(&mut self, _bounds: Rect, _transform: Affine) {
        // Not used - we use shape-specific handles via render_shape_handles
    }
}

impl VelloRenderer {
    /// Draw a remote user's cursor at a screen position.
    /// 
    /// The cursor is rendered as a small pointer arrow with the user's color.
    pub fn draw_cursor(&mut self, screen_pos: Point, color: Color) {
        // Draw cursor pointer (simple triangle pointing up-right)
        let mut path = BezPath::new();
        // Triangle: tip at screen_pos, pointing up-right
        path.move_to(screen_pos); // tip
        path.line_to(Point::new(screen_pos.x, screen_pos.y + 18.0)); // bottom-left
        path.line_to(Point::new(screen_pos.x + 14.0, screen_pos.y + 14.0)); // bottom-right
        path.close_path();
        
        // Fill the cursor
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            color,
            None,
            &path,
        );
        
        // White stroke for visibility against any background
        let stroke = Stroke::new(1.5);
        self.scene.stroke(&stroke, Affine::IDENTITY, Color::WHITE, None, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafftink_core::canvas::Canvas;
    use drafftink_core::shapes::Rectangle;

    #[test]
    fn test_renderer_creation() {
        let renderer = VelloRenderer::new();
        assert!(renderer.scene().encoding().is_empty());
    }

    #[test]
    fn test_build_empty_scene() {
        let mut renderer = VelloRenderer::new();
        let canvas = Canvas::new();
        let ctx = RenderContext::new(&canvas, kurbo::Size::new(800.0, 600.0));

        renderer.build_scene(&ctx);
        // Scene should have grid lines at minimum
    }

    #[test]
    fn test_build_scene_with_shapes() {
        let mut renderer = VelloRenderer::new();
        let mut canvas = Canvas::new();

        let rect = Rectangle::new(Point::new(100.0, 100.0), 200.0, 150.0);
        canvas.document.add_shape(Shape::Rectangle(rect));

        let ctx = RenderContext::new(&canvas, kurbo::Size::new(800.0, 600.0));
        renderer.build_scene(&ctx);
    }
}
