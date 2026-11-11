//! The table of settings keys (docs/commands.md §5): type, range, default and the check that a value
//! must pass before it is stored. This is the only place that knows what a valid value looks like.
//!
//! A value is normalised when it is checked: colours become `#rrggbb`, whole numbers become integers,
//! and a style object keeps only the properties it names. The normal form is what is stored, compared
//! (a repeat of the current value is `changed: false`) and written to the `Storage` port.

use std::collections::BTreeMap;

use drafftink_scene::Color;
use serde_json::{Map, Number, Value, json};

use crate::ports::FileRef;
use crate::refusal::echo;

/// What a key holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `true` or `false`.
    Bool,
    /// A finite number in `min..=max`.
    Number {
        /// Smallest accepted value.
        min: f64,
        /// Largest accepted value.
        max: f64,
    },
    /// A whole number in `min..=max`.
    Int {
        /// Smallest accepted value.
        min: i64,
        /// Largest accepted value.
        max: i64,
    },
    /// One of a fixed list of words. The order is the order `settings.cycle` walks.
    Choice(&'static [&'static str]),
    /// A colour: `#rgb`, `#rrggbb`, `#rrggbbaa` or `transparent`.
    Color,
    /// Text of at most this many characters.
    Text {
        /// Longest accepted text, in characters.
        max_chars: usize,
    },
    /// A collaboration server address: `ws://`, `wss://` or a path such as `/ws`.
    ServerUrl,
    /// An object of style properties (`StyleProps` of docs/commands.md §2.7).
    StyleProps,
    /// A list of file references, newest first.
    RecentFiles {
        /// Most entries kept.
        max: usize,
    },
}

/// A default value in a form a `const` can hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Def {
    /// A boolean.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A text, colour or choice.
    Str(&'static str),
    /// An empty list or object, whichever the kind needs.
    Empty,
}

/// One setting.
#[derive(Debug, Clone, Copy)]
pub struct KeyDef {
    /// The key, such as `snap.angle_step`.
    pub name: &'static str,
    /// What it holds.
    pub kind: Kind,
    /// Its default.
    pub default: Def,
    /// True: commands cannot write it. Another command maintains it (`doc.recent`).
    pub read_only: bool,
    /// True: a person may write it, an agent may not. It opens a door to the machine (`agent.mcp_http`), so the
    /// agent that the door would serve cannot be the one to open it.
    pub user_only: bool,
    /// One line for people and agents.
    pub summary: &'static str,
}

/// The grid styles, in the order `settings.cycle` walks (the old `GridStyle::next`).
pub const GRID_STYLES: &[&str] = &["none", "lines", "horizontal_lines", "crosses", "dots"];
/// The theme choices (docs/commands.md §9 item 13).
pub const THEMES: &[&str] = &["light", "dark", "system"];
/// The fonts the UI text can use: the canvas families that are bundled (the words of the `font_family` property).
pub const UI_FONTS: &[&str] = &["gelpen", "gelpen_serif", "vanilla_extract", "noto_sans", "plex_sans"];
/// The path types of `arrow.type` (the words of `drafftink_edit::arrow_defaults::path_name`).
pub const ARROW_TYPES: &[&str] = &["sharp", "round", "elbow"];
/// The heads of `arrow.start_head` and `arrow.end_head` (the words of `drafftink_edit::arrow_defaults::name_of`).
pub const HEAD_NAMES: &[&str] = &[
    "none",
    "arrow",
    "bar",
    "circle",
    "circle_outline",
    "triangle",
    "triangle_outline",
    "diamond",
    "diamond_outline",
    "cardinality_one",
    "cardinality_many",
    "cardinality_one_or_many",
    "cardinality_exactly_one",
    "cardinality_zero_or_one",
    "cardinality_zero_or_many",
];
/// Most entries of `doc.recent`.
pub const MAX_RECENT: usize = 10;

const fn key(name: &'static str, kind: Kind, default: Def, summary: &'static str) -> KeyDef {
    KeyDef { name, kind, default, read_only: false, user_only: false, summary }
}

const fn user_key(name: &'static str, kind: Kind, default: Def, summary: &'static str) -> KeyDef {
    KeyDef { user_only: true, ..key(name, kind, default, summary) }
}

/// Every setting, in the order of docs/commands.md §5, then `theme`.
pub const KEYS: &[KeyDef] = &[
    key("view.grid_style", Kind::Choice(GRID_STYLES), Def::Str("lines"), "How the background grid is drawn."),
    key("snap.grid", Kind::Bool, Def::Bool(false), "Snap dragged points to the grid."),
    key("snap.smart_guides", Kind::Bool, Def::Bool(false), "Snap to the edges, centres and spacing of other elements."),
    key("snap.angle", Kind::Bool, Def::Bool(false), "Snap the angle of lines and arrows."),
    key(
        "snap.angle_step",
        Kind::Number { min: 1.0, max: 90.0 },
        Def::Int(15),
        "Angle step in degrees when angle snapping is on.",
    ),
    key("export.scale", Kind::Number { min: 0.25, max: 4.0 }, Def::Int(2), "Default PNG export scale."),
    key("export.padding", Kind::Number { min: 0.0, max: 200.0 }, Def::Int(20), "Default export padding in world units."),
    key("export.background", Kind::Bool, Def::Bool(true), "Draw the background colour in exports."),
    key("export.dark", Kind::Bool, Def::Bool(false), "Export with the dark theme colours."),
    key("export.embed_scene", Kind::Bool, Def::Bool(true), "Embed the scene in exported PNG files."),
    key("doc.default_background", Kind::Color, Def::Str("#ffffff"), "Background colour of new documents."),
    key("input.nudge_step", Kind::Number { min: 0.1, max: 1000.0 }, Def::Int(20), "Distance of an arrow-key nudge."),
    key(
        "input.box_select",
        Kind::Choice(&["overlap", "contain"]),
        Def::Str("overlap"),
        "Whether a selection box picks elements it touches or only those it contains.",
    ),
    key("style.defaults", Kind::StyleProps, Def::Empty, "Style properties that new elements start with."),
    key("arrow.binding", Kind::Bool, Def::Bool(true), "Bind arrow ends to shapes."),
    key("arrow.midpoint_snap", Kind::Bool, Def::Bool(false), "Snap arrow ends to the middle of a shape's edge."),
    key(
        "arrow.type",
        Kind::Choice(ARROW_TYPES),
        Def::Str("sharp"),
        "Path type of new arrows (the arrow tool): straight, curved or elbow.",
    ),
    key("arrow.start_head", Kind::Choice(HEAD_NAMES), Def::Str("none"), "Head at the start of new arrows."),
    key("arrow.end_head", Kind::Choice(HEAD_NAMES), Def::Str("arrow"), "Head at the end of new arrows."),
    key("frame.clip", Kind::Bool, Def::Bool(true), "Clip frame children to the frame."),
    key("frame.show_names", Kind::Bool, Def::Bool(true), "Show frame names."),
    key("frame.outline", Kind::Bool, Def::Bool(true), "Draw the frame outline."),
    key("history.depth", Kind::Int { min: 10, max: 1000 }, Def::Int(200), "Most undo steps kept per tab."),
    key("autosave.enabled", Kind::Bool, Def::Bool(true), "Keep the workspace in storage while you work."),
    key("autosave.interval_s", Kind::Int { min: 1, max: 300 }, Def::Int(5), "Longest time in seconds between workspace saves."),
    key("clipboard.excalidraw_compat", Kind::Bool, Def::Bool(true), "Copy in a form Excalidraw can paste."),
    key(
        "collab.server_url",
        Kind::ServerUrl,
        Def::Str("ws://localhost:3030/ws"),
        "Address of the collaboration server.",
    ),
    user_key(
        "agent.mcp_http",
        Kind::Bool,
        Def::Bool(false),
        "Serve MCP to agents over HTTP on 127.0.0.1 (desktop app only; a person turns it on, an agent cannot).",
    ),
    key("collab.user_name", Kind::Text { max_chars: 40 }, Def::Str(""), "Name other people see."),
    key("collab.user_color", Kind::Color, Def::Str("#6366f1"), "Colour of your cursor for other people."),
    KeyDef {
        name: "doc.recent",
        kind: Kind::RecentFiles { max: MAX_RECENT },
        default: Def::Empty,
        read_only: true,
        user_only: false,
        summary: "Recently saved or opened documents. Maintained by `doc.open` and `doc.save`.",
    },
    key("theme", Kind::Choice(THEMES), Def::Str("system"), "Light or dark colours; system follows the device."),
    key(
        "ui.font",
        Kind::Choice(UI_FONTS),
        Def::Str("gelpen"),
        "The font of menus, bars and dialogs: one of the bundled canvas fonts. Does not change the canvas.",
    ),
];

/// The definition of a key.
pub fn find(name: &str) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.name == name)
}

/// Every key at its default, for `Settings::default`.
pub fn defaults() -> BTreeMap<String, Value> {
    KEYS.iter().map(|k| (k.name.to_owned(), k.default_value())).collect()
}

/// Why a value was refused: what the key accepts and what it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// What the key accepts, as a phrase ("a number from 1 to 90").
    pub expected: String,
    /// What was given, as a phrase ("the text \"fast\"").
    pub got: String,
}

/// The names of the style properties (`StyleProps` of docs/commands.md §2.7).
pub const STYLE_PROPS: &[&str] = &[
    "stroke_color",
    "fill_color",
    "fill_pattern",
    "stroke_width",
    "stroke_style",
    "sloppiness",
    "corner_radius",
    "opacity",
    "freehand_style",
    "pressure_mode",
    "font_family",
    "font_weight",
    "font_size",
    "text_align",
    "vertical_align",
    "line_height",
];

const FILL_PATTERNS: &[&str] = &["solid", "hachure", "zigzag", "cross_hatch", "dots", "dashed", "zigzag_line"];
const STROKE_STYLES: &[&str] = &["solid", "dashed", "dotted"];
const SLOPPINESS: &[&str] = &["architect", "artist", "cartoonist", "drunk"];
const FREEHAND_STYLES: &[&str] = &["normal", "calligraphy"];
const PRESSURE_MODES: &[&str] = &["uniform", "simulated", "device"];
/// The canvas font families (the words of `font_family`); the UI font is one of them.
const FONT_FAMILIES: &[&str] = UI_FONTS;
const FONT_WEIGHTS: &[&str] = &["light", "regular", "heavy"];
const TEXT_ALIGNS: &[&str] = &["left", "center", "right"];
const VERTICAL_ALIGNS: &[&str] = &["top", "middle", "bottom"];

/// The colour a setting names. Palette names such as `blue-500` are not resolved here: a setting holds
/// the hex form, and the `style` family turns a palette name into hex before it stores defaults.
pub fn parse_color(s: &str) -> Option<Color> {
    Color::parse(s).ok()
}

fn describe(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => format!("{b}"),
        Value::Number(n) => format!("the number {n}"),
        Value::String(s) => format!("the text \"{}\"", echo(s)),
        Value::Array(_) => "a list".to_owned(),
        Value::Object(_) => "an object".to_owned(),
    }
}

fn words(list: &[&str]) -> String {
    let q: Vec<String> = list.iter().map(|w| format!("`{w}`")).collect();
    match q.as_slice() {
        [] => String::new(),
        [a] => a.clone(),
        [init @ .., last] => format!("{} or {}", init.join(", "), last),
    }
}

/// A finite `f64` as the JSON number to store: a whole value becomes an integer so `15` and `15.0`
/// compare equal.
fn number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        // Whole and below 2^53, so the conversion is exact.
        Value::Number(Number::from(n as i64))
    } else {
        Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 9.0e15 { format!("{}", n as i64) } else { format!("{n}") }
}

impl KeyDef {
    /// The default as JSON.
    pub fn default_value(&self) -> Value {
        match (self.default, self.kind) {
            (Def::Bool(b), _) => Value::Bool(b),
            (Def::Int(i), _) => json!(i),
            (Def::Str(s), _) => Value::String(s.to_owned()),
            // The built-in style defaults, so `settings.get style.defaults` shows real values.
            (Def::Empty, Kind::StyleProps) => {
                let raw = serde_json::to_value(drafftink_edit::style::StyleDefaults::default().to_props())
                    .unwrap_or_else(|_| Value::Object(Map::new()));
                match raw.as_object() {
                    Some(obj) => normalize_style(obj).unwrap_or(raw),
                    None => raw,
                }
            }
            (Def::Empty, _) => Value::Array(Vec::new()),
        }
    }

    /// What the key accepts, as a phrase that completes "must be ...".
    pub fn expected(&self) -> String {
        match self.kind {
            Kind::Bool => "true or false".to_owned(),
            Kind::Number { min, max } => format!("a number from {} to {}", number_text(min), number_text(max)),
            Kind::Int { min, max } => format!("a whole number from {min} to {max}"),
            Kind::Choice(list) => format!("one of {}", words(list)),
            Kind::Color => "a colour such as `#6366f1`, or `transparent`".to_owned(),
            Kind::Text { max_chars } => format!("text of at most {max_chars} characters"),
            Kind::ServerUrl => "a server address that starts with `ws://` or `wss://`, or a path such as `/ws`".to_owned(),
            Kind::StyleProps => format!("an object with some of the style properties {}", words(STYLE_PROPS)),
            Kind::RecentFiles { max } => format!("a list of at most {max} file references"),
        }
    }

    /// Checks a value and returns it in normal form.
    pub fn normalize(&self, v: &Value) -> Result<Value, Mismatch> {
        let bad = || Mismatch { expected: self.expected(), got: describe(v) };
        match self.kind {
            Kind::Bool => v.as_bool().map(Value::Bool).ok_or_else(bad),
            Kind::Number { min, max } => match v.as_f64() {
                Some(n) if n.is_finite() && n >= min && n <= max => Ok(number_value(n)),
                _ => Err(bad()),
            },
            Kind::Int { min, max } => {
                let n = v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64));
                match n {
                    Some(n) if n >= min && n <= max => Ok(json!(n)),
                    _ => Err(bad()),
                }
            }
            Kind::Choice(list) => match v.as_str() {
                Some(s) if list.contains(&s) => Ok(Value::String(s.to_owned())),
                _ => Err(bad()),
            },
            Kind::Color => v.as_str().and_then(parse_color).map(|c| Value::String(c.to_hex())).ok_or_else(bad),
            Kind::Text { max_chars } => match v.as_str() {
                Some(s) if s.chars().count() <= max_chars => Ok(Value::String(s.to_owned())),
                _ => Err(bad()),
            },
            Kind::ServerUrl => match v.as_str() {
                Some(s) if server_url_ok(s) => Ok(Value::String(s.to_owned())),
                _ => Err(bad()),
            },
            Kind::StyleProps => match v.as_object() {
                Some(obj) => normalize_style(obj),
                None => Err(bad()),
            },
            Kind::RecentFiles { max } => match v.as_array() {
                Some(list) if list.len() <= max && list.iter().all(|f| serde_json::from_value::<FileRef>(f.clone()).is_ok()) => {
                    Ok(v.clone())
                }
                _ => Err(bad()),
            },
        }
    }
}

fn server_url_ok(s: &str) -> bool {
    s.len() <= 2048
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && (s.starts_with("ws://") && s.len() > 5 || s.starts_with("wss://") && s.len() > 6 || s.starts_with('/') && !s.starts_with("//"))
}

/// Checks a style object. The result keeps the properties given, with colours as hex and whole numbers
/// as integers.
fn normalize_style(obj: &Map<String, Value>) -> Result<Value, Mismatch> {
    let mut out = Map::new();
    for (k, val) in obj {
        if !STYLE_PROPS.contains(&k.as_str()) {
            return Err(Mismatch {
                expected: format!("an object with some of the style properties {}", words(STYLE_PROPS)),
                got: format!("an object with the unknown property `{}`", echo(k)),
            });
        }
        match style_prop(k, val) {
            Some(n) => {
                out.insert(k.clone(), n);
            }
            None => {
                return Err(Mismatch {
                    expected: format!("an object whose `{k}` is {}", style_prop_expected(k)),
                    got: format!("`{k}` set to {}", describe(val)),
                });
            }
        }
    }
    Ok(Value::Object(out))
}

fn style_choice(v: &Value, list: &[&str]) -> Option<Value> {
    v.as_str().filter(|s| list.contains(s)).map(|s| Value::String(s.to_owned()))
}

fn style_number(v: &Value, min: f64, max: f64, min_open: bool) -> Option<Value> {
    let n = v.as_f64().filter(|n| n.is_finite() && *n <= max && if min_open { *n > min } else { *n >= min })?;
    Some(number_value(n))
}

fn style_prop(name: &str, v: &Value) -> Option<Value> {
    use drafftink_scene::limits::{MAX_CORNER_RADIUS, MAX_FONT_SIZE, MAX_STROKE_WIDTH};
    match name {
        "stroke_color" => v.as_str().and_then(parse_color).map(|c| Value::String(c.to_hex())),
        "fill_color" if v.is_null() => Some(Value::Null),
        "fill_color" => v.as_str().and_then(parse_color).map(|c| Value::String(c.to_hex())),
        "fill_pattern" => style_choice(v, FILL_PATTERNS),
        "stroke_width" => style_number(v, 0.0, MAX_STROKE_WIDTH, true),
        "stroke_style" => style_choice(v, STROKE_STYLES),
        "sloppiness" => style_choice(v, SLOPPINESS),
        "corner_radius" => style_number(v, 0.0, MAX_CORNER_RADIUS, false),
        "opacity" => style_number(v, 0.0, 1.0, false),
        "freehand_style" => style_choice(v, FREEHAND_STYLES),
        "pressure_mode" => style_choice(v, PRESSURE_MODES),
        "font_family" => style_choice(v, FONT_FAMILIES),
        "font_weight" => style_choice(v, FONT_WEIGHTS),
        "font_size" => style_number(v, 0.0, MAX_FONT_SIZE, true),
        "text_align" => style_choice(v, TEXT_ALIGNS),
        "vertical_align" => style_choice(v, VERTICAL_ALIGNS),
        "line_height" => style_number(v, 1.0, 3.0, false),
        _ => None,
    }
}

fn style_prop_expected(name: &str) -> String {
    use drafftink_scene::limits::{MAX_CORNER_RADIUS, MAX_FONT_SIZE, MAX_STROKE_WIDTH};
    match name {
        "stroke_color" => "a colour".to_owned(),
        "fill_color" => "a colour or null".to_owned(),
        "fill_pattern" => format!("one of {}", words(FILL_PATTERNS)),
        "stroke_width" => format!("a number above 0 and at most {}", number_text(MAX_STROKE_WIDTH)),
        "stroke_style" => format!("one of {}", words(STROKE_STYLES)),
        "sloppiness" => format!("one of {}", words(SLOPPINESS)),
        "corner_radius" => format!("a number from 0 to {}", number_text(MAX_CORNER_RADIUS)),
        "opacity" => "a number from 0 to 1".to_owned(),
        "freehand_style" => format!("one of {}", words(FREEHAND_STYLES)),
        "pressure_mode" => format!("one of {}", words(PRESSURE_MODES)),
        "font_family" => format!("one of {}", words(FONT_FAMILIES)),
        "font_weight" => format!("one of {}", words(FONT_WEIGHTS)),
        "font_size" => format!("a number above 0 and at most {}", number_text(MAX_FONT_SIZE)),
        "text_align" => format!("one of {}", words(TEXT_ALIGNS)),
        "vertical_align" => format!("one of {}", words(VERTICAL_ALIGNS)),
        "line_height" => "a number from 1 to 3".to_owned(),
        _ => "valid".to_owned(),
    }
}
