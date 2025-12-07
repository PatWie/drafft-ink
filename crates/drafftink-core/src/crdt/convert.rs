//! Conversion between Shape types and Loro values.

use crate::shapes::{
    Arrow, Ellipse, FontFamily, FontWeight, Freehand, Group, Image, ImageFormat, Line, Rectangle,
    SerializableColor, Shape, ShapeStyle, ShapeTrait, Sloppiness, Text,
};
use kurbo::Point;
use loro::{LoroList, LoroMap, LoroMapValue, LoroResult, LoroValue};
use uuid::Uuid;

// Shape type identifiers
const TYPE_RECTANGLE: &str = "rectangle";
const TYPE_ELLIPSE: &str = "ellipse";
const TYPE_LINE: &str = "line";
const TYPE_ARROW: &str = "arrow";
const TYPE_FREEHAND: &str = "freehand";
const TYPE_TEXT: &str = "text";
const TYPE_GROUP: &str = "group";
const TYPE_IMAGE: &str = "image";

// Group keys
const KEY_CHILDREN: &str = "children";

// Common keys
const KEY_TYPE: &str = "type";
const KEY_ID: &str = "id";

// Style keys
const KEY_STROKE_R: &str = "stroke_r";
const KEY_STROKE_G: &str = "stroke_g";
const KEY_STROKE_B: &str = "stroke_b";
const KEY_STROKE_A: &str = "stroke_a";
const KEY_STROKE_WIDTH: &str = "stroke_width";
const KEY_FILL_R: &str = "fill_r";
const KEY_FILL_G: &str = "fill_g";
const KEY_FILL_B: &str = "fill_b";
const KEY_FILL_A: &str = "fill_a";
const KEY_HAS_FILL: &str = "has_fill";
const KEY_SLOPPINESS: &str = "sloppiness";
const KEY_SEED: &str = "seed";

// Rectangle keys
const KEY_X: &str = "x";
const KEY_Y: &str = "y";
const KEY_WIDTH: &str = "width";
const KEY_HEIGHT: &str = "height";
const KEY_CORNER_RADIUS: &str = "corner_radius";

// Line/Arrow keys
const KEY_START_X: &str = "start_x";
const KEY_START_Y: &str = "start_y";
const KEY_END_X: &str = "end_x";
const KEY_END_Y: &str = "end_y";

// Freehand keys
const KEY_POINTS: &str = "points";

// Text keys
const KEY_CONTENT: &str = "content";
const KEY_FONT_SIZE: &str = "font_size";
const KEY_FONT_FAMILY: &str = "font_family";
const KEY_FONT_WEIGHT: &str = "font_weight";

// Image keys
const KEY_SOURCE_WIDTH: &str = "source_width";
const KEY_SOURCE_HEIGHT: &str = "source_height";
const KEY_FORMAT: &str = "format";
const KEY_DATA_BASE64: &str = "data_base64";

// Helper functions to extract values from LoroMapValue (derefs to HashMap<String, LoroValue>)
fn get_double(map: &LoroMapValue, key: &str) -> Option<f64> {
    match map.get(key)? {
        LoroValue::Double(d) => Some(*d),
        LoroValue::I64(i) => Some(*i as f64),
        _ => None,
    }
}

fn get_i64(map: &LoroMapValue, key: &str) -> Option<i64> {
    match map.get(key)? {
        LoroValue::I64(i) => Some(*i),
        LoroValue::Double(d) => Some(*d as i64),
        _ => None,
    }
}

fn get_string(map: &LoroMapValue, key: &str) -> Option<String> {
    match map.get(key)? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn get_bool(map: &LoroMapValue, key: &str) -> Option<bool> {
    match map.get(key)? {
        LoroValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// Convert a Shape to Loro map entries.
pub fn shape_to_loro(shape: &Shape, map: &LoroMap) -> LoroResult<()> {
    match shape {
        Shape::Rectangle(rect) => {
            map.insert(KEY_TYPE, TYPE_RECTANGLE)?;
            map.insert(KEY_ID, rect.id().to_string())?;
            map.insert(KEY_X, rect.position.x)?;
            map.insert(KEY_Y, rect.position.y)?;
            map.insert(KEY_WIDTH, rect.width)?;
            map.insert(KEY_HEIGHT, rect.height)?;
            map.insert(KEY_CORNER_RADIUS, rect.corner_radius)?;
            style_to_loro(&rect.style, map)?;
        }
        Shape::Ellipse(ellipse) => {
            map.insert(KEY_TYPE, TYPE_ELLIPSE)?;
            map.insert(KEY_ID, ellipse.id().to_string())?;
            map.insert(KEY_X, ellipse.center.x)?;
            map.insert(KEY_Y, ellipse.center.y)?;
            map.insert(KEY_WIDTH, ellipse.radius_x)?;
            map.insert(KEY_HEIGHT, ellipse.radius_y)?;
            style_to_loro(&ellipse.style, map)?;
        }
        Shape::Line(line) => {
            map.insert(KEY_TYPE, TYPE_LINE)?;
            map.insert(KEY_ID, line.id().to_string())?;
            map.insert(KEY_START_X, line.start.x)?;
            map.insert(KEY_START_Y, line.start.y)?;
            map.insert(KEY_END_X, line.end.x)?;
            map.insert(KEY_END_Y, line.end.y)?;
            style_to_loro(&line.style, map)?;
        }
        Shape::Arrow(arrow) => {
            map.insert(KEY_TYPE, TYPE_ARROW)?;
            map.insert(KEY_ID, arrow.id().to_string())?;
            map.insert(KEY_START_X, arrow.start.x)?;
            map.insert(KEY_START_Y, arrow.start.y)?;
            map.insert(KEY_END_X, arrow.end.x)?;
            map.insert(KEY_END_Y, arrow.end.y)?;
            style_to_loro(&arrow.style, map)?;
        }
        Shape::Freehand(freehand) => {
            map.insert(KEY_TYPE, TYPE_FREEHAND)?;
            map.insert(KEY_ID, freehand.id().to_string())?;

            // Store points as a list of [x, y] pairs
            let points_list = map.insert_container(KEY_POINTS, LoroList::new())?;
            for point in &freehand.points {
                let point_list =
                    points_list.insert_container(points_list.len(), LoroList::new())?;
                point_list.push(point.x)?;
                point_list.push(point.y)?;
            }

            style_to_loro(&freehand.style, map)?;
        }
        Shape::Text(text) => {
            map.insert(KEY_TYPE, TYPE_TEXT)?;
            map.insert(KEY_ID, text.id().to_string())?;
            map.insert(KEY_X, text.position.x)?;
            map.insert(KEY_Y, text.position.y)?;
            map.insert(KEY_CONTENT, text.content.clone())?;
            map.insert(KEY_FONT_SIZE, text.font_size)?;
            map.insert(KEY_FONT_FAMILY, font_family_to_i64(text.font_family))?;
            map.insert(KEY_FONT_WEIGHT, font_weight_to_i64(text.font_weight))?;
            style_to_loro(&text.style, map)?;
        }
        Shape::Group(group) => {
            map.insert(KEY_TYPE, TYPE_GROUP)?;
            map.insert(KEY_ID, group.id().to_string())?;

            // Store children as a list of shape maps
            let children_list = map.insert_container(KEY_CHILDREN, LoroList::new())?;
            for child in group.children() {
                let child_map =
                    children_list.insert_container(children_list.len(), LoroMap::new())?;
                shape_to_loro(child, &child_map)?;
            }
        }
        Shape::Image(image) => {
            map.insert(KEY_TYPE, TYPE_IMAGE)?;
            map.insert(KEY_ID, image.id().to_string())?;
            map.insert(KEY_X, image.position.x)?;
            map.insert(KEY_Y, image.position.y)?;
            map.insert(KEY_WIDTH, image.width)?;
            map.insert(KEY_HEIGHT, image.height)?;
            map.insert(KEY_SOURCE_WIDTH, image.source_width as i64)?;
            map.insert(KEY_SOURCE_HEIGHT, image.source_height as i64)?;
            map.insert(KEY_FORMAT, image_format_to_i64(image.format))?;
            map.insert(KEY_DATA_BASE64, image.data_base64.clone())?;
            style_to_loro(&image.style, map)?;
        }
    }
    Ok(())
}

/// Convert style properties to Loro map entries.
fn style_to_loro(style: &ShapeStyle, map: &LoroMap) -> LoroResult<()> {
    map.insert(KEY_STROKE_R, style.stroke_color.r as i64)?;
    map.insert(KEY_STROKE_G, style.stroke_color.g as i64)?;
    map.insert(KEY_STROKE_B, style.stroke_color.b as i64)?;
    map.insert(KEY_STROKE_A, style.stroke_color.a as i64)?;
    map.insert(KEY_STROKE_WIDTH, style.stroke_width)?;
    map.insert(KEY_SLOPPINESS, sloppiness_to_i64(style.sloppiness))?;
    map.insert(KEY_SEED, style.seed as i64)?;

    if let Some(fill) = style.fill_color {
        map.insert(KEY_HAS_FILL, true)?;
        map.insert(KEY_FILL_R, fill.r as i64)?;
        map.insert(KEY_FILL_G, fill.g as i64)?;
        map.insert(KEY_FILL_B, fill.b as i64)?;
        map.insert(KEY_FILL_A, fill.a as i64)?;
    } else {
        map.insert(KEY_HAS_FILL, false)?;
    }

    Ok(())
}

/// Convert a Loro map to a Shape.
pub fn shape_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let shape_type = get_string(map, KEY_TYPE)?;

    match shape_type.as_str() {
        TYPE_RECTANGLE => rectangle_from_loro(map),
        TYPE_ELLIPSE => ellipse_from_loro(map),
        TYPE_LINE => line_from_loro(map),
        TYPE_ARROW => arrow_from_loro(map),
        TYPE_FREEHAND => freehand_from_loro(map),
        TYPE_TEXT => text_from_loro(map),
        TYPE_GROUP => group_from_loro(map),
        TYPE_IMAGE => image_from_loro(map),
        _ => None,
    }
}

fn rectangle_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let x = get_double(map, KEY_X)?;
    let y = get_double(map, KEY_Y)?;
    let width = get_double(map, KEY_WIDTH)?;
    let height = get_double(map, KEY_HEIGHT)?;
    let corner_radius = get_double(map, KEY_CORNER_RADIUS).unwrap_or(0.0);
    let style = style_from_loro(map)?;

    let mut rect = Rectangle::new(Point::new(x, y), width, height);
    rect.corner_radius = corner_radius;
    rect.style = style;

    // Reconstruct with correct ID via serde (since id is private)
    if let Ok(id) = Uuid::parse_str(&id_str) {
        let json = serde_json::json!({
            "id": id.to_string(),
            "position": { "x": x, "y": y },
            "width": width,
            "height": height,
            "corner_radius": corner_radius,
            "style": style_to_json(&rect.style)
        });
        if let Ok(rect_with_id) = serde_json::from_value::<Rectangle>(json) {
            return Some(Shape::Rectangle(rect_with_id));
        }
    }

    Some(Shape::Rectangle(rect))
}

fn ellipse_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let center_x = get_double(map, KEY_X)?;
    let center_y = get_double(map, KEY_Y)?;
    let radius_x = get_double(map, KEY_WIDTH)?;
    let radius_y = get_double(map, KEY_HEIGHT)?;
    let style = style_from_loro(map)?;

    let mut ellipse = Ellipse::new(Point::new(center_x, center_y), radius_x, radius_y);
    ellipse.style = style;

    if let Ok(id) = Uuid::parse_str(&id_str) {
        let json = serde_json::json!({
            "id": id.to_string(),
            "center": { "x": center_x, "y": center_y },
            "radius_x": radius_x,
            "radius_y": radius_y,
            "style": style_to_json(&ellipse.style)
        });
        if let Ok(ellipse_with_id) = serde_json::from_value::<Ellipse>(json) {
            return Some(Shape::Ellipse(ellipse_with_id));
        }
    }

    Some(Shape::Ellipse(ellipse))
}

fn line_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let start_x = get_double(map, KEY_START_X)?;
    let start_y = get_double(map, KEY_START_Y)?;
    let end_x = get_double(map, KEY_END_X)?;
    let end_y = get_double(map, KEY_END_Y)?;
    let style = style_from_loro(map)?;

    let mut line = Line::new(Point::new(start_x, start_y), Point::new(end_x, end_y));
    line.style = style;

    if let Ok(id) = Uuid::parse_str(&id_str) {
        let json = serde_json::json!({
            "id": id.to_string(),
            "start": { "x": start_x, "y": start_y },
            "end": { "x": end_x, "y": end_y },
            "style": style_to_json(&line.style)
        });
        if let Ok(line_with_id) = serde_json::from_value::<Line>(json) {
            return Some(Shape::Line(line_with_id));
        }
    }

    Some(Shape::Line(line))
}

fn arrow_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let start_x = get_double(map, KEY_START_X)?;
    let start_y = get_double(map, KEY_START_Y)?;
    let end_x = get_double(map, KEY_END_X)?;
    let end_y = get_double(map, KEY_END_Y)?;
    let style = style_from_loro(map)?;

    let mut arrow = Arrow::new(Point::new(start_x, start_y), Point::new(end_x, end_y));
    arrow.style = style;

    if let Ok(id) = Uuid::parse_str(&id_str) {
        let json = serde_json::json!({
            "id": id.to_string(),
            "start": { "x": start_x, "y": start_y },
            "end": { "x": end_x, "y": end_y },
            "head_size": arrow.head_size,
            "style": style_to_json(&arrow.style)
        });
        if let Ok(arrow_with_id) = serde_json::from_value::<Arrow>(json) {
            return Some(Shape::Arrow(arrow_with_id));
        }
    }

    Some(Shape::Arrow(arrow))
}

fn freehand_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let style = style_from_loro(map)?;

    let points: Vec<Point> = if let Some(LoroValue::List(points_list)) = map.get(KEY_POINTS) {
        points_list
            .iter()
            .filter_map(|p| {
                if let LoroValue::List(coords) = p {
                    if coords.len() >= 2 {
                        let x = match coords.first()? {
                            LoroValue::Double(d) => *d,
                            LoroValue::I64(i) => *i as f64,
                            _ => return None,
                        };
                        let y = match coords.get(1)? {
                            LoroValue::Double(d) => *d,
                            LoroValue::I64(i) => *i as f64,
                            _ => return None,
                        };
                        return Some(Point::new(x, y));
                    }
                }
                None
            })
            .collect()
    } else {
        vec![]
    };

    let mut freehand = Freehand::from_points(points.clone());
    freehand.style = style;

    if let Ok(id) = Uuid::parse_str(&id_str) {
        let points_json: Vec<serde_json::Value> = points
            .iter()
            .map(|p| serde_json::json!({ "x": p.x, "y": p.y }))
            .collect();
        let json = serde_json::json!({
            "id": id.to_string(),
            "points": points_json,
            "style": style_to_json(&freehand.style)
        });
        if let Ok(freehand_with_id) = serde_json::from_value::<Freehand>(json) {
            return Some(Shape::Freehand(freehand_with_id));
        }
    }

    Some(Shape::Freehand(freehand))
}

fn text_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let x = get_double(map, KEY_X)?;
    let y = get_double(map, KEY_Y)?;
    let content = get_string(map, KEY_CONTENT)?;
    let font_size = get_double(map, KEY_FONT_SIZE).unwrap_or(20.0);
    let font_family = get_i64(map, KEY_FONT_FAMILY)
        .map(i64_to_font_family)
        .unwrap_or_default();
    let font_weight = get_i64(map, KEY_FONT_WEIGHT)
        .map(i64_to_font_weight)
        .unwrap_or_default();
    let style = style_from_loro(map)?;

    let mut text = Text::new(Point::new(x, y), content.clone())
        .with_font_size(font_size)
        .with_font_family(font_family)
        .with_font_weight(font_weight);
    text.style = style;

    if let Ok(id) = Uuid::parse_str(&id_str) {
        let json = serde_json::json!({
            "id": id.to_string(),
            "position": { "x": x, "y": y },
            "content": content,
            "font_size": font_size,
            "font_family": font_family,
            "font_weight": font_weight,
            "style": style_to_json(&text.style)
        });
        if let Ok(text_with_id) = serde_json::from_value::<Text>(json) {
            return Some(Shape::Text(text_with_id));
        }
    }

    Some(Shape::Text(text))
}

fn group_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;

    // Get the children list
    let children_value = map.get(KEY_CHILDREN)?;
    let children_list = match &children_value {
        LoroValue::Container(_container_id) => {
            // In collaborative mode, children would be stored as a container
            // For now, we handle it as a list
            return None; // TODO: Handle container references
        }
        LoroValue::List(list) => list,
        _ => return None,
    };

    let mut children = Vec::new();
    for child_value in children_list.iter() {
        if let LoroValue::Map(child_map) = child_value {
            let child_map_value = LoroMapValue::from(child_map.clone());
            if let Some(child_shape) = shape_from_loro(&child_map_value) {
                children.push(child_shape);
            }
        }
    }

    // Reconstruct with correct ID
    if let Ok(id) = Uuid::parse_str(&id_str) {
        // Create a group with the specific ID using serde
        let children_json: Vec<serde_json::Value> = children
            .iter()
            .map(|c| serde_json::to_value(c).unwrap_or_default())
            .collect();
        let json = serde_json::json!({
            "id": id.to_string(),
            "children": children_json,
            "style": style_to_json(&ShapeStyle::default())
        });
        if let Ok(group_with_id) = serde_json::from_value::<Group>(json) {
            return Some(Shape::Group(group_with_id));
        }
    }

    Some(Shape::Group(Group::new(children)))
}

fn style_from_loro(map: &LoroMapValue) -> Option<ShapeStyle> {
    let stroke_r = get_i64(map, KEY_STROKE_R)? as u8;
    let stroke_g = get_i64(map, KEY_STROKE_G)? as u8;
    let stroke_b = get_i64(map, KEY_STROKE_B)? as u8;
    let stroke_a = get_i64(map, KEY_STROKE_A)? as u8;
    let stroke_width = get_double(map, KEY_STROKE_WIDTH)?;
    let sloppiness = get_i64(map, KEY_SLOPPINESS)
        .map(i64_to_sloppiness)
        .unwrap_or_default();
    // Use existing seed or generate a new one for backward compatibility with old documents
    let seed = get_i64(map, KEY_SEED).map(|v| v as u32).unwrap_or_else(|| {
        // Generate a seed using atomic counter (works on all platforms including WASM)
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEED_COUNTER: AtomicU32 = AtomicU32::new(1);
        let counter = SEED_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut x = counter.wrapping_mul(0x9E3779B9);
        x ^= x >> 16;
        x = x.wrapping_mul(0x85EBCA6B);
        x ^= x >> 13;
        x
    });

    let fill_color = if get_bool(map, KEY_HAS_FILL).unwrap_or(false) {
        let fill_r = get_i64(map, KEY_FILL_R).unwrap_or(0) as u8;
        let fill_g = get_i64(map, KEY_FILL_G).unwrap_or(0) as u8;
        let fill_b = get_i64(map, KEY_FILL_B).unwrap_or(0) as u8;
        let fill_a = get_i64(map, KEY_FILL_A).unwrap_or(255) as u8;
        Some(SerializableColor::new(fill_r, fill_g, fill_b, fill_a))
    } else {
        None
    };

    Some(ShapeStyle {
        stroke_color: SerializableColor::new(stroke_r, stroke_g, stroke_b, stroke_a),
        stroke_width,
        fill_color,
        sloppiness,
        seed,
    })
}

/// Helper to convert ShapeStyle to JSON for serde reconstruction
fn style_to_json(style: &ShapeStyle) -> serde_json::Value {
    serde_json::json!({
        "stroke_color": {
            "r": style.stroke_color.r,
            "g": style.stroke_color.g,
            "b": style.stroke_color.b,
            "a": style.stroke_color.a
        },
        "stroke_width": style.stroke_width,
        "fill_color": style.fill_color.map(|c| serde_json::json!({
            "r": c.r, "g": c.g, "b": c.b, "a": c.a
        })),
        "sloppiness": style.sloppiness,
        "seed": style.seed
    })
}

// Enum conversion helpers

fn sloppiness_to_i64(s: Sloppiness) -> i64 {
    match s {
        Sloppiness::Architect => 0,
        Sloppiness::Artist => 1,
        Sloppiness::Cartoonist => 2,
    }
}

fn i64_to_sloppiness(v: i64) -> Sloppiness {
    match v {
        0 => Sloppiness::Architect,
        1 => Sloppiness::Artist,
        _ => Sloppiness::Cartoonist,
    }
}

fn font_family_to_i64(f: FontFamily) -> i64 {
    match f {
        FontFamily::GelPen => 0,
        FontFamily::Roboto => 1,
        FontFamily::ArchitectsDaughter => 2,
    }
}

fn i64_to_font_family(v: i64) -> FontFamily {
    match v {
        0 => FontFamily::GelPen,
        1 => FontFamily::Roboto,
        _ => FontFamily::ArchitectsDaughter,
    }
}

fn font_weight_to_i64(w: FontWeight) -> i64 {
    match w {
        FontWeight::Light => 0,
        FontWeight::Regular => 1,
        FontWeight::Heavy => 2,
    }
}

fn i64_to_font_weight(v: i64) -> FontWeight {
    match v {
        0 => FontWeight::Light,
        1 => FontWeight::Regular,
        _ => FontWeight::Heavy,
    }
}

fn image_format_to_i64(f: ImageFormat) -> i64 {
    match f {
        ImageFormat::Png => 0,
        ImageFormat::Jpeg => 1,
        ImageFormat::WebP => 2,
    }
}

fn i64_to_image_format(v: i64) -> ImageFormat {
    match v {
        0 => ImageFormat::Png,
        1 => ImageFormat::Jpeg,
        _ => ImageFormat::WebP,
    }
}

fn image_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let id_str = get_string(map, KEY_ID)?;
    let x = get_double(map, KEY_X)?;
    let y = get_double(map, KEY_Y)?;
    let width = get_double(map, KEY_WIDTH)?;
    let height = get_double(map, KEY_HEIGHT)?;
    let source_width = get_i64(map, KEY_SOURCE_WIDTH)? as u32;
    let source_height = get_i64(map, KEY_SOURCE_HEIGHT)? as u32;
    let format = get_i64(map, KEY_FORMAT)
        .map(i64_to_image_format)
        .unwrap_or(ImageFormat::Png);
    let data_base64 = get_string(map, KEY_DATA_BASE64)?;
    let style = style_from_loro(map)?;

    if let Ok(id) = Uuid::parse_str(&id_str) {
        let json = serde_json::json!({
            "id": id.to_string(),
            "position": { "x": x, "y": y },
            "width": width,
            "height": height,
            "source_width": source_width,
            "source_height": source_height,
            "format": format,
            "data_base64": data_base64,
            "style": style_to_json(&style)
        });
        if let Ok(image_with_id) = serde_json::from_value::<Image>(json) {
            return Some(Shape::Image(image_with_id));
        }
    }

    None
}
