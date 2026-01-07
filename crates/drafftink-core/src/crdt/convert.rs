//! Conversion between Shape types and Loro values.

use crate::shapes::{
    Arrow, Ellipse, FillPattern, FontFamily, FontWeight, Freehand, Group, Image, ImageFormat, Line,
    Math, PathStyle, Rectangle, SerializableColor, Shape, ShapeStyle, ShapeTrait, Sloppiness, Text,
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
const KEY_FILL_PATTERN: &str = "fill_pattern";
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
const KEY_INTERMEDIATE_POINTS: &str = "intermediate_points";
const KEY_PATH_STYLE: &str = "path_style";
const KEY_HEAD_SIZE: &str = "head_size";

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

// Rotation key (shared by Rectangle, Ellipse, Text, Image)
const KEY_ROTATION: &str = "rotation";

// Helper functions to extract values from LoroMapValue
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

fn get_id(map: &LoroMapValue) -> Option<Uuid> {
    Uuid::parse_str(&get_string(map, KEY_ID)?).ok()
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
            map.insert(KEY_ROTATION, rect.rotation)?;
            style_to_loro(&rect.style, map)?;
        }
        Shape::Ellipse(ellipse) => {
            map.insert(KEY_TYPE, TYPE_ELLIPSE)?;
            map.insert(KEY_ID, ellipse.id().to_string())?;
            map.insert(KEY_X, ellipse.center.x)?;
            map.insert(KEY_Y, ellipse.center.y)?;
            map.insert(KEY_WIDTH, ellipse.radius_x)?;
            map.insert(KEY_HEIGHT, ellipse.radius_y)?;
            map.insert(KEY_ROTATION, ellipse.rotation)?;
            style_to_loro(&ellipse.style, map)?;
        }
        Shape::Line(line) => {
            map.insert(KEY_TYPE, TYPE_LINE)?;
            map.insert(KEY_ID, line.id().to_string())?;
            map.insert(KEY_START_X, line.start.x)?;
            map.insert(KEY_START_Y, line.start.y)?;
            map.insert(KEY_END_X, line.end.x)?;
            map.insert(KEY_END_Y, line.end.y)?;
            map.insert(KEY_PATH_STYLE, path_style_to_i64(line.path_style))?;
            let pts_list = map.insert_container(KEY_INTERMEDIATE_POINTS, LoroList::new())?;
            for p in &line.intermediate_points {
                let pt = pts_list.insert_container(pts_list.len(), LoroList::new())?;
                pt.push(p.x)?;
                pt.push(p.y)?;
            }
            style_to_loro(&line.style, map)?;
        }
        Shape::Arrow(arrow) => {
            map.insert(KEY_TYPE, TYPE_ARROW)?;
            map.insert(KEY_ID, arrow.id().to_string())?;
            map.insert(KEY_START_X, arrow.start.x)?;
            map.insert(KEY_START_Y, arrow.start.y)?;
            map.insert(KEY_END_X, arrow.end.x)?;
            map.insert(KEY_END_Y, arrow.end.y)?;
            map.insert(KEY_HEAD_SIZE, arrow.head_size)?;
            map.insert(KEY_PATH_STYLE, path_style_to_i64(arrow.path_style))?;
            let pts_list = map.insert_container(KEY_INTERMEDIATE_POINTS, LoroList::new())?;
            for p in &arrow.intermediate_points {
                let pt = pts_list.insert_container(pts_list.len(), LoroList::new())?;
                pt.push(p.x)?;
                pt.push(p.y)?;
            }
            style_to_loro(&arrow.style, map)?;
        }
        Shape::Freehand(freehand) => {
            map.insert(KEY_TYPE, TYPE_FREEHAND)?;
            map.insert(KEY_ID, freehand.id().to_string())?;
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
            map.insert(KEY_ROTATION, text.rotation)?;
            style_to_loro(&text.style, map)?;
        }
        Shape::Group(group) => {
            map.insert(KEY_TYPE, TYPE_GROUP)?;
            map.insert(KEY_ID, group.id().to_string())?;
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
            map.insert(KEY_ROTATION, image.rotation)?;
            style_to_loro(&image.style, map)?;
        }
        Shape::Math(math) => {
            map.insert(KEY_TYPE, "math")?;
            map.insert(KEY_ID, math.id().to_string())?;
            map.insert(KEY_X, math.position.x)?;
            map.insert(KEY_Y, math.position.y)?;
            map.insert(KEY_CONTENT, math.latex.clone())?;
            map.insert(KEY_FONT_SIZE, math.font_size)?;
            map.insert(KEY_ROTATION, math.rotation)?;
            style_to_loro(&math.style, map)?;
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
    map.insert(KEY_FILL_PATTERN, fill_pattern_to_i64(style.fill_pattern))?;
    map.insert("opacity", style.opacity)?;

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
        "math" => math_from_loro(map),
        _ => None,
    }
}

fn rectangle_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Rectangle(Rectangle::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_X)?, get_double(map, KEY_Y)?),
        get_double(map, KEY_WIDTH)?,
        get_double(map, KEY_HEIGHT)?,
        get_double(map, KEY_CORNER_RADIUS).unwrap_or(0.0),
        get_double(map, KEY_ROTATION).unwrap_or(0.0),
        style_from_loro(map)?,
    )))
}

fn ellipse_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Ellipse(Ellipse::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_X)?, get_double(map, KEY_Y)?),
        get_double(map, KEY_WIDTH)?,
        get_double(map, KEY_HEIGHT)?,
        get_double(map, KEY_ROTATION).unwrap_or(0.0),
        style_from_loro(map)?,
    )))
}

fn line_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Line(Line::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_START_X)?, get_double(map, KEY_START_Y)?),
        Point::new(get_double(map, KEY_END_X)?, get_double(map, KEY_END_Y)?),
        points_from_loro(map, KEY_INTERMEDIATE_POINTS),
        get_i64(map, KEY_PATH_STYLE)
            .map(i64_to_path_style)
            .unwrap_or_default(),
        style_from_loro(map)?,
    )))
}

fn arrow_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Arrow(Arrow::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_START_X)?, get_double(map, KEY_START_Y)?),
        Point::new(get_double(map, KEY_END_X)?, get_double(map, KEY_END_Y)?),
        points_from_loro(map, KEY_INTERMEDIATE_POINTS),
        get_i64(map, KEY_PATH_STYLE)
            .map(i64_to_path_style)
            .unwrap_or_default(),
        get_double(map, KEY_HEAD_SIZE).unwrap_or(15.0),
        style_from_loro(map)?,
    )))
}

fn freehand_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Freehand(Freehand::reconstruct(
        get_id(map)?,
        points_from_loro(map, KEY_POINTS),
        style_from_loro(map)?,
    )))
}

fn text_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Text(Text::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_X)?, get_double(map, KEY_Y)?),
        get_string(map, KEY_CONTENT)?,
        get_double(map, KEY_FONT_SIZE).unwrap_or(20.0),
        get_i64(map, KEY_FONT_FAMILY)
            .map(i64_to_font_family)
            .unwrap_or_default(),
        get_i64(map, KEY_FONT_WEIGHT)
            .map(i64_to_font_weight)
            .unwrap_or_default(),
        get_double(map, KEY_ROTATION).unwrap_or(0.0),
        style_from_loro(map)?,
    )))
}

fn group_from_loro(map: &LoroMapValue) -> Option<Shape> {
    let children_list = match map.get(KEY_CHILDREN)? {
        LoroValue::List(list) => list,
        _ => return None,
    };

    let children: Vec<Shape> = children_list
        .iter()
        .filter_map(|v| {
            if let LoroValue::Map(child_map) = v {
                shape_from_loro(&child_map.clone())
            } else {
                None
            }
        })
        .collect();

    Some(Shape::Group(Group::reconstruct(get_id(map)?, children)))
}

fn image_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Image(Image::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_X)?, get_double(map, KEY_Y)?),
        get_double(map, KEY_WIDTH)?,
        get_double(map, KEY_HEIGHT)?,
        get_i64(map, KEY_SOURCE_WIDTH)? as u32,
        get_i64(map, KEY_SOURCE_HEIGHT)? as u32,
        get_i64(map, KEY_FORMAT)
            .map(i64_to_image_format)
            .unwrap_or(ImageFormat::Png),
        get_string(map, KEY_DATA_BASE64)?,
        get_double(map, KEY_ROTATION).unwrap_or(0.0),
        style_from_loro(map)?,
    )))
}

fn math_from_loro(map: &LoroMapValue) -> Option<Shape> {
    Some(Shape::Math(Math::reconstruct(
        get_id(map)?,
        Point::new(get_double(map, KEY_X)?, get_double(map, KEY_Y)?),
        get_string(map, KEY_CONTENT)?,
        get_double(map, KEY_FONT_SIZE).unwrap_or(Math::DEFAULT_FONT_SIZE),
        get_double(map, KEY_ROTATION).unwrap_or(0.0),
        style_from_loro(map)?,
    )))
}

fn points_from_loro(map: &LoroMapValue, key: &str) -> Vec<Point> {
    let Some(LoroValue::List(list)) = map.get(key) else {
        return vec![];
    };
    list.iter()
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
    let fill_pattern = get_i64(map, KEY_FILL_PATTERN)
        .map(i64_to_fill_pattern)
        .unwrap_or_default();
    let seed = get_i64(map, KEY_SEED).map(|v| v as u32).unwrap_or_else(|| {
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
        Some(SerializableColor::new(
            get_i64(map, KEY_FILL_R).unwrap_or(0) as u8,
            get_i64(map, KEY_FILL_G).unwrap_or(0) as u8,
            get_i64(map, KEY_FILL_B).unwrap_or(0) as u8,
            get_i64(map, KEY_FILL_A).unwrap_or(255) as u8,
        ))
    } else {
        None
    };

    Some(ShapeStyle {
        stroke_color: SerializableColor::new(stroke_r, stroke_g, stroke_b, stroke_a),
        stroke_width,
        fill_color,
        fill_pattern,
        sloppiness,
        seed,
        opacity: get_double(map, "opacity").unwrap_or(1.0),
    })
}

// Enum conversion helpers

fn sloppiness_to_i64(s: Sloppiness) -> i64 {
    match s {
        Sloppiness::Architect => 0,
        Sloppiness::Artist => 1,
        Sloppiness::Cartoonist => 2,
        Sloppiness::Drunk => 3,
    }
}

fn i64_to_sloppiness(v: i64) -> Sloppiness {
    match v {
        0 => Sloppiness::Architect,
        1 => Sloppiness::Artist,
        2 => Sloppiness::Cartoonist,
        _ => Sloppiness::Drunk,
    }
}

fn path_style_to_i64(s: PathStyle) -> i64 {
    match s {
        PathStyle::Direct => 0,
        PathStyle::Flowing => 1,
        PathStyle::Angular => 2,
    }
}

fn i64_to_path_style(v: i64) -> PathStyle {
    match v {
        0 => PathStyle::Direct,
        1 => PathStyle::Flowing,
        _ => PathStyle::Angular,
    }
}

fn font_family_to_i64(f: FontFamily) -> i64 {
    match f {
        FontFamily::GelPen => 0,
        FontFamily::VanillaExtract => 1,
        FontFamily::GelPenSerif => 2,
    }
}

fn i64_to_font_family(v: i64) -> FontFamily {
    match v {
        0 => FontFamily::GelPen,
        1 => FontFamily::VanillaExtract,
        _ => FontFamily::GelPenSerif,
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

fn fill_pattern_to_i64(p: FillPattern) -> i64 {
    match p {
        FillPattern::Solid => 0,
        FillPattern::Hachure => 1,
        FillPattern::ZigZag => 2,
        FillPattern::CrossHatch => 3,
        FillPattern::Dots => 4,
        FillPattern::Dashed => 5,
        FillPattern::ZigZagLine => 6,
    }
}

fn i64_to_fill_pattern(v: i64) -> FillPattern {
    match v {
        0 => FillPattern::Solid,
        1 => FillPattern::Hachure,
        2 => FillPattern::ZigZag,
        3 => FillPattern::CrossHatch,
        4 => FillPattern::Dots,
        5 => FillPattern::Dashed,
        _ => FillPattern::ZigZagLine,
    }
}
