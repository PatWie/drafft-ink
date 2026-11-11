//! Excalidraw elements to scene elements: one pass that parses each element into a draft, then a pass that resolves
//! ids and references, repairs what the validator would refuse, and assigns z-order keys.

use std::collections::{BTreeMap, HashMap, HashSet};

use drafftink_scene::limits::{
    MAX_COORD, MAX_CORNER_RADIUS, MAX_CUSTOM_KEYS, MAX_ELEMENTS, MAX_FILES, MAX_FIXED_POINT, MAX_FONT_SIZE,
    MAX_GROUP_DEPTH, MAX_LATEX_CHARS, MAX_LINK_BYTES, MAX_NAME_CHARS, MAX_POINTS, MAX_STROKE_WIDTH, MAX_TEXT_CHARS,
};
use drafftink_scene::{
    BindMode, Binding, Brush, Color, Crop, Element, ElementId, ElementKind, FileEntry, FileId, FillPattern,
    FixedSegment, FrameData, FreehandData, GroupId, Head, IdSource, ImageData, Kind, LinearData, MathData, PathType,
    PressureMode, Sloppiness, StrokeStyle, Style, TextAlign, TextData, TextStyle, Transform, VerticalAlign, ZIndex,
};
use kurbo::Point;
use serde_json::{Map, Value};

use super::image::{decode_data_url, mime_is_accepted, sniff_size};
use super::map::{self, Roundness};
use super::notes::{self, Note};
use super::read::{Rd, pair, warn};
use crate::error::{FormatName, ImportReport, IoError};
use crate::measure::{EstimateMeasure, TextMeasure};

/// Excalidraw's own default stroke colour, used when a colour is missing or unreadable.
const DEFAULT_STROKE: Color = Color::rgb(0x1e, 0x1e, 0x1e);
/// Smallest font size and line height taken as given; anything below is read as missing (the model needs a size above
/// zero, and a tiny one is never meant).
const MIN_FONT_SIZE: f64 = 0.01;
/// Size given to a shape that has no width or height (the old importer used the same).
const DEFAULT_SIDE: f64 = 100.0;

/// What `convert` needs to read.
pub(super) struct Source<'a> {
    /// The `elements` array.
    pub elements: &'a [Value],
    /// The `files` map, if any.
    pub files: Option<&'a Map<String, Value>>,
    /// Text put before every warning ("item 3: ").
    pub prefix: &'a str,
}

/// The converted elements, in z-order, and the picture blobs they use.
pub(super) struct Converted {
    pub elements: Vec<Element>,
    pub files: BTreeMap<FileId, FileEntry>,
}

struct RawBinding {
    target: String,
    mode: Option<BindMode>,
    fixed_point: Option<[f64; 2]>,
}

struct Draft {
    raw_id: Option<String>,
    kind: ElementKind,
    transform: Transform,
    style: Style,
    locked: bool,
    link: Option<String>,
    custom: Option<BTreeMap<String, Value>>,
    name: Option<String>,
    frame: Option<String>,
    container: Option<String>,
    label_position: Option<f64>,
    bindings: [Option<RawBinding>; 2],
    groups: Vec<String>,
    index: Option<ZIndex>,
}

struct Ctx<'a, 'b> {
    rep: &'a mut ImportReport,
    ids: &'a mut dyn IdSource,
    files: Files<'b>,
}

/// Converts the elements of one scene, clipboard or library item.
pub(super) fn convert(src: Source<'_>, ids: &mut dyn IdSource, rep: &mut ImportReport) -> Result<Converted, IoError> {
    let mut ctx = Ctx { rep: &mut *rep, ids: &mut *ids, files: Files::new(src.files) };
    let mut drafts: Vec<Draft> = Vec::new();
    if src.elements.len() > MAX_ELEMENTS {
        warn(ctx.rep, || {
            format!(
                "{}the file has {} elements; only the first {MAX_ELEMENTS} are read",
                src.prefix,
                src.elements.len()
            )
        });
    }
    for (i, v) in src.elements.iter().enumerate().take(MAX_ELEMENTS) {
        let n = i + 1;
        let Some(m) = v.as_object() else {
            warn(ctx.rep, || format!("{}element {n} is not an object and is skipped", src.prefix));
            continue;
        };
        let rd = Rd::new(m, src.prefix, n);
        if rd.bool("isDeleted", ctx.rep) == Some(true) {
            continue;
        }
        if let Some(d) = parse_element(rd, &mut ctx) {
            drafts.push(d);
        }
    }
    let files = std::mem::take(&mut ctx.files.out);
    let elements = resolve(drafts, ids, rep, src.prefix)?;
    // Keep only the blobs a surviving image refers to.
    let used: HashSet<&FileId> = elements
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Image(i) => i.file_id.as_ref(),
            _ => None,
        })
        .collect();
    let files = files.into_iter().filter(|(k, _)| used.contains(k)).collect();
    Ok(Converted { elements, files })
}

// ---------------------------------------------------------------------------------------------------------------
// Pictures

struct Files<'a> {
    map: Option<&'a Map<String, Value>>,
    cache: HashMap<String, Option<(FileId, u32, u32)>>,
    out: BTreeMap<FileId, FileEntry>,
}

impl<'a> Files<'a> {
    fn new(map: Option<&'a Map<String, Value>>) -> Self {
        Self { map, cache: HashMap::new(), out: BTreeMap::new() }
    }

    /// Decodes the picture named `raw` once. `Err` is the reason it cannot be used.
    fn resolve(&mut self, raw: &str) -> Result<(FileId, u32, u32), String> {
        if let Some(hit) = self.cache.get(raw) {
            return hit.clone().ok_or_else(|| "its picture was already rejected".to_owned());
        }
        let result = self.decode(raw);
        self.cache.insert(raw.to_owned(), result.as_ref().ok().cloned());
        result
    }

    fn decode(&mut self, raw: &str) -> Result<(FileId, u32, u32), String> {
        let entry = self
            .map
            .and_then(|m| m.get(raw))
            .and_then(Value::as_object)
            .ok_or_else(|| format!("the picture {raw:?} is not in `files`"))?;
        let url = entry
            .get("dataURL")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the picture {raw:?} has no `dataURL`"))?;
        let (url_mime, bytes) =
            decode_data_url(url).map_err(|e| format!("the picture {raw:?} cannot be used: {}", e.describe()))?;
        let declared = entry.get("mimeType").and_then(Value::as_str).map(str::to_ascii_lowercase);
        let mime = [declared.as_deref().unwrap_or(""), url_mime.as_str()]
            .into_iter()
            .find(|m| mime_is_accepted(m))
            .map(str::to_owned)
            .ok_or_else(|| format!("the picture {raw:?} has a media type that is not an image type"))?;
        let (w, h) = sniff_size(&bytes).unwrap_or((0, 0));
        let (id, file) = FileEntry::new(mime, bytes);
        if !self.out.contains_key(&id) && self.out.len() >= MAX_FILES {
            return Err(format!("the document already has {MAX_FILES} pictures"));
        }
        self.out.entry(id.clone()).or_insert(file);
        Ok((id, w, h))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Parsing one element

fn clamp_coord(v: f64) -> f64 {
    v.clamp(-MAX_COORD, MAX_COORD)
}

fn seed_of(v: f64) -> u32 {
    // Truncation to 32 bits is intended: Excalidraw seeds are up to 31 bits, anything else is folded.
    (v.abs().trunc() as u64 & u64::from(u32::MAX)) as u32
}

fn clean_text(s: &str, max_chars: usize) -> Option<String> {
    let t: String = s.chars().filter(|c| !c.is_control()).take(max_chars).collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn parse_color(rd: &Rd, key: &str, rep: &mut ImportReport) -> Option<Color> {
    let s = rd.str(key, rep)?;
    // `#rgba` is valid CSS, and Color reads only `#rgb`. Widen it to `#rrggbbaa`.
    let t = s.trim();
    let widened: String;
    let t = match t.strip_prefix('#') {
        Some(h) if h.len() == 4 && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
            widened = format!("#{}", h.chars().flat_map(|c| [c, c]).collect::<String>());
            widened.as_str()
        }
        _ => t,
    };
    match Color::parse(t) {
        Ok(c) => Some(c),
        Err(_) => {
            rd.warn(rep, || {
                format!(
                    "`{key}` is {:?}, which is not a colour this app reads; the default colour is used",
                    s.chars().take(30).collect::<String>()
                )
            });
            None
        }
    }
}

fn parse_element(rd: Rd, ctx: &mut Ctx) -> Option<Draft> {
    let ty = rd.str("type", ctx.rep);
    let Some(ty) = ty else {
        rd.warn(ctx.rep, || "has no `type` and is skipped".to_owned());
        return None;
    };
    let kind_name = match ty {
        "rectangle" | "diamond" | "ellipse" | "line" | "arrow" | "freedraw" | "text" | "image" | "frame"
        | "magicframe" => ty,
        "stickynote" => {
            rd.warn(ctx.rep, || "is a sticky note and is imported as a rectangle".to_owned());
            "rectangle"
        }
        "embeddable" | "iframe" => {
            rd.warn(ctx.rep, || format!("is an embedded web page (`{ty}`), which this app does not show; skipped"));
            return None;
        }
        "selection" => return None,
        other => {
            rd.warn(ctx.rep, || {
                format!("has the unknown type {:?} and is skipped", other.chars().take(30).collect::<String>())
            });
            return None;
        }
    };

    // customData and the drafftink note come first: the note overrides plain fields below.
    let (custom, note) = parse_custom(&rd, ctx.rep);
    let mut style = parse_style(&rd, ctx, &note);
    let rounding = parse_roundness(&rd, ctx.rep);
    let mut d = Draft {
        raw_id: rd.str("id", ctx.rep).map(str::to_owned),
        kind: ElementKind::Rectangle,
        transform: Transform::new(0.0, 0.0, 0.0, 0.0),
        style: Style::default(),
        locked: rd.bool("locked", ctx.rep).unwrap_or(false),
        link: parse_link(&rd, ctx.rep),
        custom,
        name: note.name.as_deref().and_then(|s| clean_text(s, MAX_NAME_CHARS)),
        frame: rd.str("frameId", ctx.rep).map(str::to_owned),
        container: None,
        label_position: None,
        bindings: [None, None],
        groups: parse_groups(&rd, ctx.rep),
        index: rd.str("index", ctx.rep).and_then(|s| ZIndex::parse(s).ok()),
    };
    let mut angle = rd.num("angle", ctx.rep).unwrap_or(0.0);
    if angle.abs() > 100.0 {
        angle %= std::f64::consts::TAU;
    }

    match kind_name {
        "rectangle" | "diamond" | "ellipse" => {
            let (x, y, w, h) = shape_box(&rd, ctx.rep, (DEFAULT_SIDE, DEFAULT_SIDE));
            d.transform = Transform { angle, ..Transform::new(x, y, w, h) };
            d.kind = match kind_name {
                "rectangle" => ElementKind::Rectangle,
                "diamond" => ElementKind::Diamond,
                _ => ElementKind::Ellipse,
            };
            if kind_name != "ellipse"
                && let Some(r) = rounding
            {
                style.corner_radius = map::corner_radius(r, w.min(h));
            }
        }
        "frame" | "magicframe" => {
            let (x, y, w, h) = shape_box(&rd, ctx.rep, (DEFAULT_SIDE, DEFAULT_SIDE));
            // Frames cannot be rotated or nested in this model.
            d.transform = Transform::new(x, y, w, h);
            if angle != 0.0 {
                rd.warn(ctx.rep, || "is a rotated frame; frames cannot rotate, so the rotation is dropped".to_owned());
            }
            if d.frame.take().is_some() {
                rd.warn(ctx.rep, || {
                    "is a frame inside a frame; frames cannot nest, so it is placed on its own".to_owned()
                });
            }
            let name = rd.str("name", ctx.rep).and_then(|s| clean_text(s, MAX_NAME_CHARS));
            d.kind = ElementKind::Frame(FrameData { name, clip: note.frame_clip.unwrap_or(true) });
            if kind_name == "magicframe" {
                rd.warn(ctx.rep, || "is a magic frame and is imported as a plain frame".to_owned());
            }
        }
        "image" => {
            let (x, y, w, h) = shape_box(&rd, ctx.rep, (DEFAULT_SIDE, DEFAULT_SIDE));
            let (sx, sy) = rd.get("scale").and_then(pair).map_or((1.0, 1.0), |[a, b]| (a, b));
            d.transform = Transform { angle, flip_x: sx < 0.0, flip_y: sy < 0.0, ..Transform::new(x, y, w, h) };
            d.kind = ElementKind::Image(parse_image(&rd, ctx, w, h)?);
        }
        "text" => {
            let (kind, transform, container, label_position) = parse_text(&rd, ctx, &note, angle);
            d.kind = kind;
            d.transform = transform;
            d.container = container;
            d.label_position = label_position;
        }
        "freedraw" => {
            let (kind, transform) = parse_freedraw(&rd, ctx, &note, angle)?;
            d.kind = kind;
            d.transform = transform;
        }
        _ => {
            // line, arrow
            let (kind, transform, bindings) = parse_linear(&rd, ctx, &note, kind_name == "arrow", angle, rounding)?;
            d.kind = kind;
            d.transform = transform;
            d.bindings = bindings;
        }
    }
    if let Some(r) = note.corner_radius.filter(|r| r.is_finite()) {
        style.corner_radius = r;
    }
    style.corner_radius = style.corner_radius.clamp(0.0, MAX_CORNER_RADIUS);
    d.style = style;
    Some(d)
}

fn parse_custom(rd: &Rd, rep: &mut ImportReport) -> (Option<BTreeMap<String, Value>>, Note) {
    let Some(obj) = rd.object("customData", rep) else { return (None, Note::default()) };
    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    let mut note = Note::default();
    for (k, v) in obj {
        if k == notes::KEY && v.is_object() {
            match Note::from_value(v) {
                Ok(n) => note = n,
                Err(why) => {
                    rd.warn(rep, || format!("has a damaged `customData.{}` note and it is ignored: {why}", notes::KEY))
                }
            }
            continue;
        }
        if map.len() >= MAX_CUSTOM_KEYS {
            rd.warn(rep, || format!("has more than {MAX_CUSTOM_KEYS} `customData` keys; the rest are dropped"));
            break;
        }
        map.insert(k.clone(), v.clone());
    }
    ((!map.is_empty()).then_some(map), note)
}

fn parse_link(rd: &Rd, rep: &mut ImportReport) -> Option<String> {
    let s = rd.str("link", rep)?;
    if s.is_empty() {
        return None;
    }
    if s.len() > MAX_LINK_BYTES || s.chars().any(char::is_control) {
        rd.warn(rep, || "has a link that is too long or has control characters; the link is dropped".to_owned());
        return None;
    }
    Some(s.to_owned())
}

fn parse_groups(rd: &Rd, rep: &mut ImportReport) -> Vec<String> {
    let Some(arr) = rd.array("groupIds", rep) else { return Vec::new() };
    // Longer lists are cut later, where nesting is checked; this only bounds memory.
    arr.iter().take(MAX_GROUP_DEPTH * 4).filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

fn parse_roundness(rd: &Rd, rep: &mut ImportReport) -> Option<Roundness> {
    if let Some(r) = rd.get("roundness") {
        let kind = r.get("type").and_then(Value::as_i64)?;
        let value = r.get("value").and_then(Value::as_f64).filter(|v| v.is_finite());
        return Some(Roundness { kind, value });
    }
    // Files from before roundness existed say `strokeSharpness: "round"`.
    if rd.m.get("roundness").is_none() && rd.str("strokeSharpness", rep) == Some("round") {
        return Some(Roundness { kind: map::ROUNDNESS_ADAPTIVE, value: None });
    }
    None
}

fn parse_style(rd: &Rd, ctx: &mut Ctx, note: &Note) -> Style {
    let rep = &mut *ctx.rep;
    let mut s =
        Style { stroke_color: parse_color(rd, "strokeColor", rep).unwrap_or(DEFAULT_STROKE), ..Style::default() };
    s.fill_color = match rd.str("backgroundColor", rep) {
        None | Some("transparent") => None,
        Some(_) => parse_color(rd, "backgroundColor", rep),
    };
    s.stroke_width = rd.num("strokeWidth", rep).map_or(2.0, |w| w.clamp(0.0, MAX_STROKE_WIDTH));
    s.stroke_style = match rd.str("strokeStyle", rep) {
        None => StrokeStyle::Solid,
        Some(name) => map::stroke_style_from_name(name).unwrap_or_else(|| {
            rd.warn(rep, || {
                format!("has the unknown `strokeStyle` {:?}; solid is used", name.chars().take(20).collect::<String>())
            });
            StrokeStyle::Solid
        }),
    };
    s.fill_pattern = match rd.str("fillStyle", rep) {
        None => FillPattern::Solid,
        Some(name) => map::fill_from_name(name).unwrap_or_else(|| {
            rd.warn(rep, || {
                format!("has the unknown `fillStyle` {:?}; solid is used", name.chars().take(20).collect::<String>())
            });
            FillPattern::Solid
        }),
    };
    s.sloppiness = rd.num("roughness", rep).map_or(Sloppiness::Artist, map::sloppiness_from_roughness);
    // Excalidraw's opacity is 0 to 100. A value of 1 means one percent, not "fully opaque".
    s.opacity = rd.num("opacity", rep).map_or(1.0, |o| (o / 100.0).clamp(0.0, 1.0));
    s.seed = match rd.num("seed", rep) {
        Some(v) => seed_of(v),
        None => ctx.ids.seed(),
    };
    if let Some(p) = note.fill_pattern {
        s.fill_pattern = p;
    }
    if let Some(p) = note.sloppiness {
        s.sloppiness = p;
    }
    s
}

/// `(x, y, w, h)` with a non-negative size, clamped to the scene limits.
fn norm_box(mut x: f64, mut y: f64, mut w: f64, mut h: f64) -> (f64, f64, f64, f64) {
    if w < 0.0 {
        x += w;
        w = -w;
    }
    if h < 0.0 {
        y += h;
        h = -h;
    }
    (clamp_coord(x), clamp_coord(y), w.min(MAX_COORD), h.min(MAX_COORD))
}

/// The box of a box-like element. A missing size is `default`.
fn shape_box(rd: &Rd, rep: &mut ImportReport, default: (f64, f64)) -> (f64, f64, f64, f64) {
    let x = rd.num("x", rep).unwrap_or(0.0);
    let y = rd.num("y", rep).unwrap_or(0.0);
    let w = rd.num("width", rep).unwrap_or(default.0);
    let h = rd.num("height", rep).unwrap_or(default.1);
    norm_box(x, y, w, h)
}

// ---------------------------------------------------------------------------------------------------------------
// Kinds

fn parse_image(rd: &Rd, ctx: &mut Ctx, w: f64, h: f64) -> Option<ImageData> {
    let Some(raw) = rd.str("fileId", ctx.rep) else {
        rd.warn(ctx.rep, || "is an image without a `fileId` and is skipped".to_owned());
        return None;
    };
    let (file_id, pw, ph) = match ctx.files.resolve(raw) {
        Ok(v) => v,
        Err(why) => {
            rd.warn(ctx.rep, || format!("is an image that is skipped: {why}"));
            return None;
        }
    };
    let crop_obj = rd.object("crop", ctx.rep);
    let num = |o: &Map<String, Value>, k: &str| o.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
    let (mut nw, mut nh) = (pw, ph);
    if nw == 0 || nh == 0 {
        // The header was not readable (SVG, say). Excalidraw's own record of the size comes next, then the box.
        let from_crop = crop_obj.and_then(|c| Some((num(c, "naturalWidth")?, num(c, "naturalHeight")?)));
        let (fw, fh) = from_crop.filter(|(a, b)| *a >= 1.0 && *b >= 1.0).unwrap_or((w.max(1.0), h.max(1.0)));
        // Rounding to whole pixels is the point; the value is clamped first.
        nw = fw.round().clamp(1.0, 1.0e6) as u32;
        nh = fh.round().clamp(1.0, 1.0e6) as u32;
    }
    let crop = crop_obj.and_then(|c| {
        let (x, y, cw, ch) = (num(c, "x")?, num(c, "y")?, num(c, "width")?, num(c, "height")?);
        let ok = x >= 0.0 && y >= 0.0 && cw > 0.0 && ch > 0.0 && [x, y, cw, ch].iter().all(|v| *v <= MAX_COORD);
        if !ok {
            rd.warn(ctx.rep, || {
                "has a crop that is empty or outside the picture; the whole picture is shown".to_owned()
            });
        }
        ok.then_some(Crop { x, y, width: cw, height: ch })
    });
    Some(ImageData { file_id: Some(file_id), natural_width: nw, natural_height: nh, crop })
}

fn parse_text(
    rd: &Rd,
    ctx: &mut Ctx,
    note: &Note,
    angle: f64,
) -> (ElementKind, Transform, Option<String>, Option<f64>) {
    let rep = &mut *ctx.rep;
    // `originalText` is the text before wrapping; `text` has the wrap points as line breaks.
    let content: String = rd
        .str("originalText", rep)
        .or_else(|| rd.str("text", rep))
        .unwrap_or("")
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect();
    let mut font = TextStyle {
        size: rd.num("fontSize", rep).filter(|s| *s >= MIN_FONT_SIZE).map_or(20.0, |s| s.min(MAX_FONT_SIZE)),
        line_height: rd.num("lineHeight", rep).filter(|l| *l >= MIN_FONT_SIZE).map_or(1.25, |l| l.min(10.0)),
        ..TextStyle::default()
    };
    if let Some(n) = rd.int("fontFamily", rep) {
        match map::font_from_number(n) {
            Some((f, w)) => (font.family, font.weight) = (f, w),
            None => rd.warn(rep, || format!("has the unknown `fontFamily` {n}; the default font is used")),
        }
    }
    if let Some(f) = note.font_family {
        font.family = f;
    }
    if let Some(w) = note.font_weight {
        font.weight = w;
    }
    font.align = match rd.str("textAlign", rep) {
        None | Some("left") => TextAlign::Left,
        Some("center") => TextAlign::Center,
        Some("right") => TextAlign::Right,
        Some(other) => {
            rd.warn(rep, || {
                format!("has the unknown `textAlign` {:?}; left is used", other.chars().take(20).collect::<String>())
            });
            TextAlign::Left
        }
    };
    font.vertical_align = match rd.str("verticalAlign", rep) {
        None | Some("top") => VerticalAlign::Top,
        Some("middle") => VerticalAlign::Middle,
        Some("bottom") => VerticalAlign::Bottom,
        Some(other) => {
            rd.warn(rep, || {
                format!("has the unknown `verticalAlign` {:?}; top is used", other.chars().take(20).collect::<String>())
            });
            VerticalAlign::Top
        }
    };
    let x = rd.num("x", rep).unwrap_or(0.0);
    let y = rd.num("y", rep).unwrap_or(0.0);
    let (w, h) = (rd.num("width", rep), rd.num("height", rep));
    // A missing size is estimated from the text.
    let est = if w.is_none() || h.is_none() {
        EstimateMeasure.text_size(&content, font.family, font.weight, font.size, font.line_height)
    } else {
        (0.0, 0.0)
    };
    let (x, y, w, h) = norm_box(x, y, w.unwrap_or(est.0), h.unwrap_or(est.1));
    let transform = Transform { angle, ..Transform::new(x, y, w, h) };
    let label_position = rd.num("labelPosition", rep).map(|p| p.clamp(0.0, 1.0));
    let container = rd.str("containerId", rep).map(str::to_owned);

    if note.math {
        let latex: String = content.chars().take(MAX_LATEX_CHARS).collect();
        return (ElementKind::Math(MathData { latex, font_size: font.size }), transform, None, None);
    }
    let len = content.chars().count();
    let mut runs = Vec::new();
    let mut prev_end = 0u32;
    let mut dropped = false;
    for r in &note.runs {
        if r.start < r.end && r.start >= prev_end && (r.end as usize) <= len {
            prev_end = r.end;
            runs.push(r.clone());
        } else {
            dropped = true;
        }
    }
    if dropped {
        rd.warn(rep, || "has text colour ranges that do not fit its text; they are dropped".to_owned());
    }
    let auto_resize = rd.bool("autoResize", rep).unwrap_or(true);
    let data = TextData { content, runs, font, auto_resize, container_id: None, label_position: None };
    (ElementKind::Text(data), transform, container, label_position)
}

/// The points of a line, arrow or freehand stroke, relative to the element's `x, y`.
fn parse_points(rd: &Rd, rep: &mut ImportReport) -> Result<Vec<Point>, ()> {
    let Some(arr) = rd.array("points", rep) else { return Ok(Vec::new()) };
    if arr.len() > MAX_POINTS {
        rd.warn(rep, || format!("has {} points, and the limit is {MAX_POINTS}; it is skipped", arr.len()));
        return Err(());
    }
    let pts: Vec<Point> = arr
        .iter()
        .filter_map(pair)
        .map(|[x, y]| Point::new(x.clamp(-MAX_COORD, MAX_COORD), y.clamp(-MAX_COORD, MAX_COORD)))
        .collect();
    if pts.len() != arr.len() {
        rd.warn(rep, || format!("has {} points that are not number pairs; they are dropped", arr.len() - pts.len()));
    }
    Ok(pts)
}

/// Local points (smallest coordinates 0) and the unrotated box of points given relative to `(x, y)`. Excalidraw
/// rotates a linear element about the centre of its points' bounding box, which is the model's pivot as well, so
/// angle and points carry over unchanged.
struct Fitted {
    local: Vec<Point>,
    transform: Transform,
    shift: kurbo::Vec2,
}

fn fit(rel: &[Point], x: f64, y: f64, angle: f64) -> Option<Fitted> {
    let (min_x, min_y, _, _) = drafftink_scene::geom::points_bounds(rel)?;
    let shift = kurbo::Vec2::new(min_x, min_y);
    // A box wider than the coordinate range (only in hostile files) is squeezed to the range.
    let local: Vec<Point> = rel
        .iter()
        .map(|p| {
            let q = *p - shift;
            Point::new(q.x.clamp(0.0, MAX_COORD), q.y.clamp(0.0, MAX_COORD))
        })
        .collect();
    let (_, _, w, h) = drafftink_scene::geom::points_bounds(&local)?;
    let (tx, ty) = (clamp_coord(x + min_x), clamp_coord(y + min_y));
    Some(Fitted { local, transform: Transform { angle, ..Transform::new(tx, ty, w, h) }, shift })
}

fn parse_freedraw(rd: &Rd, ctx: &mut Ctx, note: &Note, angle: f64) -> Option<(ElementKind, Transform)> {
    let rep = &mut *ctx.rep;
    let rel = parse_points(rd, rep).ok()?;
    let x = rd.num("x", rep).unwrap_or(0.0);
    let y = rd.num("y", rep).unwrap_or(0.0);
    let Some(f) = fit(&rel, x, y, angle) else {
        rd.warn(rep, || "is a stroke without points and is skipped".to_owned());
        return None;
    };
    let simulate = rd.bool("simulatePressure", rep);
    let mut pressures: Vec<f64> = rd
        .array("pressures", rep)
        .map(|a| a.iter().filter_map(Value::as_f64).filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0)).collect())
        .unwrap_or_default();
    // Only a pressure list that matches the points is usable.
    if pressures.len() != f.local.len() {
        pressures.clear();
    }
    let pressure_mode = if simulate == Some(true) {
        pressures.clear();
        PressureMode::Simulated
    } else if !pressures.is_empty() {
        PressureMode::Device
    } else {
        PressureMode::Uniform
    };
    let data = FreehandData {
        points: f.local,
        pressures,
        pressure_mode,
        brush: note.brush.unwrap_or(Brush::Normal),
        highlighter: note.highlighter.unwrap_or(false),
        closed: note.closed.unwrap_or(false),
    };
    Some((ElementKind::Freehand(data), f.transform))
}

fn parse_binding(rd: &Rd, key: &str, rep: &mut ImportReport) -> Option<RawBinding> {
    let obj = rd.object(key, rep)?;
    let target = obj.get("elementId").and_then(Value::as_str)?.to_owned();
    let mode = match obj.get("mode").and_then(Value::as_str) {
        None => None,
        Some("inside") => Some(BindMode::Inside),
        Some("orbit") => Some(BindMode::Orbit),
        // Excalidraw's "skip" means the end is deliberately not attached.
        Some("skip") => return None,
        Some(other) => {
            rd.warn(rep, || {
                format!(
                    "`{key}` has the unknown mode {:?}; it is worked out from the arrow's position",
                    other.chars().take(20).collect::<String>()
                )
            });
            None
        }
    };
    let fixed_point = obj.get("fixedPoint").and_then(pair);
    Some(RawBinding { target, mode, fixed_point })
}

fn parse_head(rd: &Rd, key: &str, rep: &mut ImportReport) -> Head {
    match rd.str(key, rep) {
        None => Head::None,
        Some(name) => map::head_from_name(name).unwrap_or_else(|| {
            rd.warn(rep, || {
                format!("has the unknown arrowhead {:?}; none is used", name.chars().take(30).collect::<String>())
            });
            Head::None
        }),
    }
}

fn parse_linear(
    rd: &Rd,
    ctx: &mut Ctx,
    note: &Note,
    is_arrow: bool,
    angle: f64,
    rounding: Option<Roundness>,
) -> Option<(ElementKind, Transform, [Option<RawBinding>; 2])> {
    let rep = &mut *ctx.rep;
    let mut rel = parse_points(rd, rep).ok()?;
    let x = rd.num("x", rep).unwrap_or(0.0);
    let y = rd.num("y", rep).unwrap_or(0.0);
    let polygon = !is_arrow && rd.bool("polygon", rep) == Some(true);
    let mut closed = false;
    if polygon {
        // A closed line repeats its first point at the end. This model closes implicitly.
        if rel.len() >= 4 && matches!((rel.first(), rel.last()), (Some(a), Some(b)) if (*a - *b).hypot() < 1.0e-9) {
            rel.pop();
        }
        closed = rel.len() >= 3;
    }
    if rel.len() < 2 {
        rd.warn(rep, || "is a line or arrow with fewer than two points and is skipped".to_owned());
        return None;
    }
    let f = fit(&rel, x, y, angle)?;
    let elbow = rd.bool("elbowed", rep) == Some(true);
    let mut data = LinearData::new(f.local);
    data.path = if elbow {
        PathType::Elbow
    } else if rounding.is_some() {
        PathType::Round
    } else {
        PathType::Sharp
    };
    data.closed = closed;
    data.start_head = parse_head(rd, "startArrowhead", rep);
    data.end_head = parse_head(rd, "endArrowhead", rep);
    data.head_size = note.head_size.filter(|h| h.is_finite() && *h >= 0.0).map(|h| h.min(MAX_STROKE_WIDTH * 10.0));
    if elbow {
        data.start_is_special = rd.bool("startIsSpecial", rep).unwrap_or(false);
        data.end_is_special = rd.bool("endIsSpecial", rep).unwrap_or(false);
        data.fixed_segments = parse_fixed_segments(rd, rep, data.points.len(), f.shift);
    }
    let bindings = [parse_binding(rd, "startBinding", rep), parse_binding(rd, "endBinding", rep)];
    let kind = if is_arrow { ElementKind::Arrow(data) } else { ElementKind::Line(data) };
    Some((kind, f.transform, bindings))
}

fn parse_fixed_segments(rd: &Rd, rep: &mut ImportReport, n_points: usize, shift: kurbo::Vec2) -> Vec<FixedSegment> {
    let Some(arr) = rd.array("fixedSegments", rep) else { return Vec::new() };
    let mut out: Vec<FixedSegment> = Vec::new();
    let mut dropped = 0usize;
    for v in arr.iter().take(n_points) {
        let seg = (|| {
            let index = u32::try_from(v.get("index")?.as_u64()?).ok()?;
            let [sx, sy] = pair(v.get("start")?)?;
            let [ex, ey] = pair(v.get("end")?)?;
            Some(FixedSegment { index, start: Point::new(sx, sy) - shift, end: Point::new(ex, ey) - shift })
        })();
        match seg {
            Some(s) => out.push(s),
            None => dropped += 1,
        }
    }
    out.sort_by_key(|s| s.index);
    let before = out.len();
    let mut last = 0u32;
    out.retain(|s| {
        let ok = s.index > last && (s.index as usize) < n_points;
        if ok {
            last = s.index;
        }
        ok
    });
    dropped += before - out.len();
    if dropped > 0 {
        rd.warn(rep, || {
            format!("has {dropped} fixed elbow segments that are damaged or out of range; they are dropped")
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Resolving references

fn sanitize_id(raw: &str) -> String {
    raw.chars().take(64).map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

pub(super) fn fresh_element_id(ids: &mut dyn IdSource, used: &HashSet<ElementId>) -> ElementId {
    for _ in 0..10_000 {
        let id = ids.element_id();
        if !used.contains(&id) {
            return id;
        }
    }
    // The source keeps repeating itself. Count up from a name no source makes.
    let mut n = used.len();
    loop {
        if let Ok(id) = ElementId::new(&format!("x{n}"))
            && !used.contains(&id)
        {
            return id;
        }
        n += 1;
    }
}

pub(super) fn fresh_group_id(ids: &mut dyn IdSource, used: &HashSet<GroupId>) -> GroupId {
    for _ in 0..10_000 {
        let id = ids.group_id();
        if !used.contains(&id) {
            return id;
        }
    }
    let mut n = used.len();
    loop {
        if let Ok(id) = GroupId::new(&format!("x{n}"))
            && !used.contains(&id)
        {
            return id;
        }
        n += 1;
    }
}

fn resolve(
    drafts: Vec<Draft>,
    ids: &mut dyn IdSource,
    rep: &mut ImportReport,
    prefix: &str,
) -> Result<Vec<Element>, IoError> {
    let count = drafts.len();
    // 1. Element ids. Raw ids that are clean and first-seen keep their text; the rest get fresh ones.
    let mut by_raw: HashMap<String, usize> = HashMap::with_capacity(count);
    for (i, d) in drafts.iter().enumerate() {
        if let Some(r) = &d.raw_id {
            by_raw.entry(r.clone()).or_insert(i);
        }
    }
    let mut used: HashSet<ElementId> = HashSet::with_capacity(count);
    let mut final_ids: Vec<Option<ElementId>> = vec![None; count];
    let mut renamed = 0usize;
    for (i, d) in drafts.iter().enumerate() {
        let Some(raw) = &d.raw_id else { continue };
        if by_raw.get(raw.as_str()) != Some(&i) {
            continue;
        }
        let clean = sanitize_id(raw);
        if let Ok(id) = ElementId::new(&clean)
            && used.insert(id.clone())
        {
            if clean != *raw {
                renamed += 1;
            }
            if let Some(slot) = final_ids.get_mut(i) {
                *slot = Some(id);
            }
        }
    }
    if renamed > 0 {
        warn(rep, || {
            format!(
                "{prefix}{renamed} element ids had characters outside A-Z a-z 0-9 _ - or were too long; they were changed"
            )
        });
    }
    let mut generated = 0usize;
    for slot in &mut final_ids {
        if slot.is_none() {
            let id = fresh_element_id(ids, &used);
            used.insert(id.clone());
            *slot = Some(id);
            generated += 1;
        }
    }
    if generated > 0 {
        warn(rep, || format!("{prefix}{generated} elements had no usable unique id and got new ones"));
    }
    let final_ids: Vec<ElementId> = final_ids.into_iter().flatten().collect();
    if final_ids.len() != count {
        // Unreachable: every slot was filled above. Refuse to continue with misaligned data.
        return Ok(Vec::new());
    }
    // raw id -> position, for references. A raw id that did not keep its text still resolves to its element.
    let pos_of = |raw: &str| by_raw.get(raw).copied();
    let kinds: Vec<Kind> = drafts.iter().map(|d| d.kind.kind()).collect();
    let transforms: Vec<Transform> = drafts.iter().map(|d| d.transform).collect();
    let endpoints: Vec<Option<[Point; 2]>> = drafts
        .iter()
        .map(|d| match &d.kind {
            ElementKind::Line(l) | ElementKind::Arrow(l) => {
                Some([l.points.first().copied()?, l.points.last().copied()?].map(|p| d.transform.local_to_world(p)))
            }
            _ => None,
        })
        .collect();

    // 2. Groups.
    let raw_groups: Vec<&[String]> = drafts.iter().map(|d| d.groups.as_slice()).collect();
    let groups = resolve_groups(&raw_groups, ids, rep, prefix);

    // 3. Elements, with frames, labels and bindings resolved against the drafts.
    let mut label_taken: HashSet<usize> = HashSet::new();
    let mut elements: Vec<Element> = Vec::with_capacity(count);
    let mut candidates: Vec<Option<ZIndex>> = Vec::with_capacity(count);
    let (mut bad_frames, mut bad_labels, mut bad_bindings) = (0usize, 0usize, 0usize);
    for (i, (d, (id, groups))) in drafts.into_iter().zip(final_ids.iter().zip(groups)).enumerate() {
        let mut e = Element::new(id.clone(), d.kind, ZIndex::first());
        e.transform = d.transform;
        e.style = d.style;
        e.locked = d.locked;
        e.link = d.link;
        e.custom = d.custom;
        e.name = d.name;
        e.group_ids.extend(groups);
        // Frame.
        if let Some(raw) = &d.frame {
            match pos_of(raw).filter(|j| *j != i && kinds.get(*j) == Some(&Kind::Frame)) {
                Some(j) => e.frame_id = final_ids.get(j).cloned(),
                None => bad_frames += 1,
            }
        }
        // Bound text.
        if let (Some(raw), ElementKind::Text(t)) = (&d.container, &mut e.kind) {
            let target = pos_of(raw).filter(|j| *j != i && kinds.get(*j).is_some_and(|k| k.is_label_container()));
            match target {
                Some(j) if label_taken.insert(j) => {
                    t.container_id = final_ids.get(j).cloned();
                    if kinds.get(j) == Some(&Kind::Arrow) {
                        t.label_position = d.label_position;
                    }
                }
                _ => bad_labels += 1,
            }
        }
        // Arrow bindings.
        if let ElementKind::Line(l) | ElementKind::Arrow(l) = &mut e.kind {
            let ends = endpoints.get(i).copied().flatten();
            let elbow = l.path == PathType::Elbow;
            for (which, raw) in d.bindings.into_iter().enumerate() {
                let Some(raw) = raw else { continue };
                let target = pos_of(&raw.target).filter(|j| *j != i && kinds.get(*j).is_some_and(|k| k.is_bindable()));
                let Some(j) = target else {
                    bad_bindings += 1;
                    continue;
                };
                let (Some(t), Some(tid)) = (transforms.get(j), final_ids.get(j)) else { continue };
                let end_point = ends.and_then(|p| p.get(which).copied());
                let b = make_binding(&raw, tid.clone(), t, end_point, elbow);
                if which == 0 {
                    l.start_binding = Some(b);
                } else {
                    l.end_binding = Some(b);
                }
            }
        }
        candidates.push(d.index);
        elements.push(e);
    }
    if bad_frames > 0 {
        warn(rep, || {
            format!("{prefix}{bad_frames} elements name a frame that does not exist and are placed outside any frame")
        });
    }
    if bad_labels > 0 {
        warn(rep, || {
            format!(
                "{prefix}{bad_labels} texts name a container that cannot hold a label (missing, taken, or the wrong kind) and are free text now"
            )
        });
    }
    if bad_bindings > 0 {
        warn(rep, || {
            format!(
                "{prefix}{bad_bindings} arrow ends are attached to something that is missing or cannot be attached to; they are loose now"
            )
        });
    }

    // 4. Order: frames above their children, labels above their containers.
    let order = reorder(&elements);
    let mut slots: Vec<Option<(Element, Option<ZIndex>)>> = elements.into_iter().zip(candidates).map(Some).collect();
    let mut ordered: Vec<Element> = Vec::with_capacity(count);
    let mut cands: Vec<Option<ZIndex>> = Vec::with_capacity(count);
    for p in order {
        if let Some((e, c)) = slots.get_mut(p).and_then(Option::take) {
            ordered.push(e);
            cands.push(c);
        }
    }

    // 5. Z-order keys.
    let keys = assign_indexes(cands, ordered.len())?;
    for (e, k) in ordered.iter_mut().zip(keys) {
        e.index = k;
    }
    Ok(ordered)
}

/// The binding for one arrow end. A missing mode or fixed point is worked out from where the arrow end lies.
fn make_binding(raw: &RawBinding, target: ElementId, t: &Transform, end: Option<Point>, elbow: bool) -> Binding {
    let derived = end.map(|p| ratio_in(t, p));
    let fixed_point =
        raw.fixed_point.or(derived).unwrap_or([0.5, 0.5]).map(|v| v.clamp(-MAX_FIXED_POINT, MAX_FIXED_POINT));
    let mode = raw.mode.unwrap_or_else(|| {
        let inside = fixed_point.iter().all(|v| *v > 0.0 && *v < 1.0);
        // Elbow arrows have no inside mode in files that predate it: they run to the outline.
        if inside && !elbow { BindMode::Inside } else { BindMode::Orbit }
    });
    Binding { target, mode, fixed_point }
}

/// Where a world point lies in a target's unrotated box, as ratios. A flat box gives 0.5.
fn ratio_in(t: &Transform, p: Point) -> [f64; 2] {
    let c = t.center();
    let (s, co) = (-t.angle).sin_cos();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    let (ux, uy) = (c.x + dx * co - dy * s, c.y + dx * s + dy * co);
    let r = |v: f64, lo: f64, size: f64| if size > 0.0 { (v - lo) / size } else { 0.5 };
    [r(ux, t.x, t.w), r(uy, t.y, t.h)]
}

/// Final group stacks (innermost first). Ids are made valid and unique, nesting is made consistent, and groups with
/// one member are removed.
fn resolve_groups(
    raw: &[&[String]],
    ids: &mut dyn IdSource,
    rep: &mut ImportReport,
    prefix: &str,
) -> Vec<Vec<GroupId>> {
    let mut map: HashMap<&str, GroupId> = HashMap::new();
    let mut used: HashSet<GroupId> = HashSet::new();
    for stack in raw {
        for g in *stack {
            if map.contains_key(g.as_str()) {
                continue;
            }
            let clean = sanitize_id(g);
            let id = match GroupId::new(&clean) {
                Ok(id) if !used.contains(&id) => id,
                _ => fresh_group_id(ids, &used),
            };
            used.insert(id.clone());
            map.insert(g.as_str(), id);
        }
    }
    let mut recorded: HashMap<GroupId, Vec<GroupId>> = HashMap::new();
    let mut members: HashMap<GroupId, usize> = HashMap::new();
    let mut stacks: Vec<Vec<GroupId>> = Vec::with_capacity(raw.len());
    let mut conflicts = 0usize;
    for stack in raw {
        // Outermost first, so that each group is checked against the groups around it.
        let mut kept: Vec<GroupId> = Vec::new();
        for g in stack.iter().rev() {
            let Some(id) = map.get(g.as_str()) else { continue };
            if kept.contains(id) || kept.len() >= MAX_GROUP_DEPTH {
                conflicts += 1;
                continue;
            }
            match recorded.get(id) {
                Some(outer) if *outer != kept => {
                    conflicts += 1;
                    continue;
                }
                Some(_) => {}
                None => {
                    recorded.insert(id.clone(), kept.clone());
                }
            }
            *members.entry(id.clone()).or_insert(0) += 1;
            kept.push(id.clone());
        }
        stacks.push(kept);
    }
    if conflicts > 0 {
        warn(rep, || {
            format!(
                "{prefix}{conflicts} group memberships contradicted other groups (or were nested too deep) and were dropped"
            )
        });
    }
    // A group needs two members. Inner groups of a one-member group have one member as well, so dropping them keeps
    // the rest consistent.
    for stack in &mut stacks {
        stack.retain(|g| members.get(g).copied().unwrap_or(0) >= 2);
        stack.reverse();
    }
    stacks
}

/// The order in which to emit the elements: array order, except that a frame follows its last child and a label
/// follows its container (the model wants children below the frame and labels above their container).
fn reorder(elements: &[Element]) -> Vec<usize> {
    let pos: HashMap<&ElementId, usize> = elements.iter().enumerate().map(|(i, e)| (&e.id, i)).collect();
    let mut anchor: HashMap<usize, usize> = HashMap::new();
    let mut last_child: HashMap<usize, usize> = HashMap::new();
    for (i, e) in elements.iter().enumerate() {
        if let Some(f) = e.frame_id.as_ref().and_then(|f| pos.get(f))
            && i > *f
        {
            let slot = last_child.entry(*f).or_insert(i);
            *slot = (*slot).max(i);
        }
        if let Some(c) = e.container_id().and_then(|c| pos.get(c))
            && *c > i
        {
            anchor.insert(i, *c);
        }
    }
    for (f, c) in last_child {
        anchor.insert(f, c);
    }
    let mut attached: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..elements.len() {
        if let Some(a) = anchor.get(&i) {
            attached.entry(*a).or_default().push(i);
        }
    }
    let mut out = Vec::with_capacity(elements.len());
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..elements.len() {
        if anchor.contains_key(&i) {
            continue;
        }
        stack.push(i);
        while let Some(p) = stack.pop() {
            out.push(p);
            if let Some(ms) = attached.get(&p) {
                stack.extend(ms.iter().rev());
            }
        }
    }
    out
}

/// Z-order keys for `n` elements: the file's own keys where they already increase, new keys in the gaps.
pub(super) fn assign_indexes(cands: Vec<Option<ZIndex>>, n: usize) -> Result<Vec<ZIndex>, IoError> {
    // Keep a candidate only if it is above the last one kept.
    let mut kept: Vec<Option<ZIndex>> = Vec::with_capacity(n);
    let mut last: Option<ZIndex> = None;
    for c in cands {
        match c {
            Some(k) if last.as_ref().is_none_or(|l| k > *l) => {
                last = Some(k.clone());
                kept.push(Some(k));
            }
            _ => kept.push(None),
        }
    }
    let fresh_all = || {
        ZIndex::rebalance(n)
            .map_err(|e| IoError::invalid(FormatName::Excalidraw, format!("cannot order {n} elements: {e}")))
    };
    let mut out: Vec<ZIndex> = Vec::with_capacity(n);
    let mut i = 0;
    while i < kept.len() {
        if let Some(Some(k)) = kept.get(i) {
            out.push(k.clone());
            i += 1;
            continue;
        }
        let start = i;
        while matches!(kept.get(i), Some(None)) {
            i += 1;
        }
        let lower = out.last().cloned();
        let upper = kept.get(i).and_then(Clone::clone);
        match ZIndex::n_between(lower.as_ref(), upper.as_ref(), i - start) {
            Ok(keys) => out.extend(keys),
            Err(_) => return fresh_all(),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_cleaned() {
        assert_eq!(sanitize_id("a b/é"), "a_b__");
        assert_eq!(sanitize_id(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn ratio_accounts_for_rotation() {
        let t = Transform { angle: std::f64::consts::FRAC_PI_2, ..Transform::new(0.0, 0.0, 100.0, 50.0) };
        // The centre stays put under any rotation.
        let r = ratio_in(&t, Point::new(50.0, 25.0));
        assert!((r[0] - 0.5).abs() < 1e-12 && (r[1] - 0.5).abs() < 1e-12);
        // A quarter turn clockwise moves the box's right edge midpoint to the bottom of the rotated shape.
        let r = ratio_in(&t, Point::new(50.0, 25.0 + 50.0));
        assert!((r[0] - 1.0).abs() < 1e-9 && (r[1] - 0.5).abs() < 1e-9, "{r:?}");
        assert_eq!(ratio_in(&Transform::new(0.0, 0.0, 0.0, 0.0), Point::new(3.0, 3.0)), [0.5, 0.5]);
    }

    #[test]
    fn indexes_keep_good_keys_and_fill_gaps() {
        let k = |s: &str| Some(ZIndex::parse(s).unwrap());
        let out = assign_indexes(vec![k("a0"), None, k("a1"), k("a0"), None], 5).unwrap();
        assert_eq!(out.len(), 5);
        assert!(out.windows(2).all(|w| w[0] < w[1]), "{out:?}");
        assert_eq!(out[0].as_str(), "a0");
        assert_eq!(out[2].as_str(), "a1");
        let all_none = assign_indexes(vec![None; 4], 4).unwrap();
        assert!(all_none.windows(2).all(|w| w[0] < w[1]));
        assert!(assign_indexes(Vec::new(), 0).unwrap().is_empty());
    }

    #[test]
    fn group_stacks_are_made_consistent() {
        let a = vec!["in".to_owned(), "out".to_owned()];
        let b = vec!["in".to_owned(), "out".to_owned()];
        let c = vec!["out".to_owned()];
        // `x` appears once and `y` is nested differently in two members.
        let d = vec!["x".to_owned()];
        let e = vec!["y".to_owned(), "out".to_owned()];
        let f = vec!["y".to_owned()];
        let raw: Vec<&[String]> = vec![&a, &b, &c, &d, &e, &f];
        let mut rep = ImportReport::default();
        let mut ids = drafftink_scene::SeqIds::new();
        let stacks = resolve_groups(&raw, &mut ids, &mut rep, "");
        let names: Vec<Vec<&str>> = stacks.iter().map(|s| s.iter().map(GroupId::as_str).collect()).collect();
        assert_eq!(names[0], ["in", "out"]);
        assert_eq!(names[1], ["in", "out"]);
        assert_eq!(names[2], ["out"]);
        assert!(names[3].is_empty(), "single-member group is dropped");
        assert!(names[5].is_empty() || names[4].is_empty(), "y cannot have two different outers");
        assert!(!rep.is_clean());
    }
}
