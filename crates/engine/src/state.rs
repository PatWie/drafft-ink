//! The state classes of a session (docs/commands.md §1.1) that are plain data: observed editor state,
//! view, tool, settings, tabs, text-edit state and the typed extension map for family-private state.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, GroupId, Style};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::{TabDoc, UndoHistory};

/// Zoom that the UI shows as 100% (`zoom / BASE_ZOOM * 100`).
pub const BASE_ZOOM: f64 = 1.68;
/// Smallest zoom.
pub const ZOOM_MIN: f64 = 0.1;
/// Largest zoom.
pub const ZOOM_MAX: f64 = 10.0;

/// Editor state that undo restores (Excalidraw's `ObservedAppState`): selection, editing group and the
/// linear point editor. Selection changes alone never make an undo step; the next document step records
/// the state before and after it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Observed {
    /// Selected elements.
    pub selection: BTreeSet<ElementId>,
    /// Groups that are selected as a whole.
    pub selected_groups: BTreeSet<GroupId>,
    /// The group whose members are being edited individually.
    pub editing_group: Option<GroupId>,
    /// The line or arrow whose points are being edited.
    pub linear_editor: Option<LinearEditor>,
}

/// Point-edit mode of a line or arrow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinearEditor {
    /// The element.
    pub element: ElementId,
    /// Selected point indexes.
    pub selected_points: BTreeSet<usize>,
}

/// Canvas size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Viewport {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Device pixels per logical pixel.
    pub scale_factor: f64,
}

/// The camera of a tab. Not in undo history. `scroll` is the world point at the centre of the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct View {
    /// World point at the centre of the viewport.
    pub scroll: Point,
    /// Screen pixels per world unit.
    pub zoom: f64,
    /// Canvas size.
    pub viewport: Viewport,
}

impl Default for View {
    fn default() -> Self {
        Self {
            scroll: Point::ZERO,
            zoom: BASE_ZOOM,
            // Headless sessions never get `view.set_viewport`; fit and centre still need a size.
            viewport: Viewport { w: 1280.0, h: 720.0, scale_factor: 1.0 },
        }
    }
}

impl View {
    /// The world point under a screen point (logical pixels from the top-left of the canvas).
    pub fn screen_to_world(&self, p: Point) -> Point {
        Point::new(
            self.scroll.x + (p.x - self.viewport.w / 2.0) / self.zoom,
            self.scroll.y + (p.y - self.viewport.h / 2.0) / self.zoom,
        )
    }

    /// The screen point of a world point.
    pub fn world_to_screen(&self, p: Point) -> Point {
        Point::new(
            (p.x - self.scroll.x) * self.zoom + self.viewport.w / 2.0,
            (p.y - self.scroll.y) * self.zoom + self.viewport.h / 2.0,
        )
    }

    /// The visible world rectangle `(x, y, w, h)`.
    pub fn visible(&self) -> (f64, f64, f64, f64) {
        let w = self.viewport.w / self.zoom;
        let h = self.viewport.h / self.zoom;
        (self.scroll.x - w / 2.0, self.scroll.y - h / 2.0, w, h)
    }

    /// The zoom as the UI shows it, in percent.
    pub fn percent(&self) -> f64 {
        self.zoom / BASE_ZOOM * 100.0
    }
}

/// Clamps a zoom into the allowed range. Non-finite input gives the base zoom.
pub fn clamp_zoom(z: f64) -> f64 {
    if z.is_finite() { z.clamp(ZOOM_MIN, ZOOM_MAX) } else { BASE_ZOOM }
}

/// The drawing tools (docs/commands.md §2 `Tool`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Tool {
    Select,
    Hand,
    Rectangle,
    Diamond,
    Ellipse,
    Arrow,
    Line,
    Freehand,
    Highlighter,
    Text,
    Math,
    Frame,
    Eraser,
    Laser,
}

impl Tool {
    /// Lower-case name as used in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Hand => "hand",
            Self::Rectangle => "rectangle",
            Self::Diamond => "diamond",
            Self::Ellipse => "ellipse",
            Self::Arrow => "arrow",
            Self::Line => "line",
            Self::Freehand => "freehand",
            Self::Highlighter => "highlighter",
            Self::Text => "text",
            Self::Math => "math",
            Self::Frame => "frame",
            Self::Eraser => "eraser",
            Self::Laser => "laser",
        }
    }
}

/// Tool state: not in undo history, not in the document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolState {
    /// The tool in use.
    pub active: Tool,
    /// Stay on the tool after drawing one element.
    pub lock: bool,
    /// A temporary tool held with a key (Space for hand); `active` returns when it is released.
    pub temporary: Option<Tool>,
    /// Style for new elements.
    pub style_defaults: Style,
}

impl Default for ToolState {
    fn default() -> Self {
        Self { active: Tool::Select, lock: false, temporary: None, style_defaults: Style::default() }
    }
}

impl ToolState {
    /// The tool that is in effect now (the held one, else the active one).
    pub fn effective(&self) -> Tool {
        self.temporary.unwrap_or(self.active)
    }
}

/// Persisted settings (docs/commands.md §5). Values are JSON; the `settings` family validates writes
/// against the table of keys. `revision` counts changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    values: BTreeMap<String, Value>,
    /// Raised by every change.
    pub revision: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let defaults = [
            ("view.grid_style", json!("lines")),
            ("snap.grid", json!(false)),
            ("snap.smart_guides", json!(false)),
            ("snap.angle", json!(false)),
            ("snap.angle_step", json!(15)),
            ("export.scale", json!(2)),
            ("export.padding", json!(20)),
            ("export.background", json!(true)),
            ("export.dark", json!(false)),
            ("export.embed_scene", json!(true)),
            ("doc.default_background", json!("#ffffff")),
            ("input.nudge_step", json!(20)),
            ("input.box_select", json!("overlap")),
            ("arrow.binding", json!(true)),
            ("arrow.midpoint_snap", json!(false)),
            ("frame.clip", json!(true)),
            ("frame.show_names", json!(true)),
            ("frame.outline", json!(true)),
            ("history.depth", json!(50)),
            ("autosave.enabled", json!(true)),
            ("autosave.interval_s", json!(5)),
            ("clipboard.excalidraw_compat", json!(true)),
            ("collab.server_url", json!("ws://localhost:3030/ws")),
            ("collab.user_name", json!("")),
            ("collab.user_color", json!("#6366f1")),
            ("doc.recent", json!([])),
        ];
        Self { values: defaults.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(), revision: 0 }
    }
}

impl Settings {
    /// The value of a key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// A boolean setting, or `default` when absent or not a boolean.
    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    /// A number setting, or `default`.
    pub fn f64_or(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    /// An integer setting, or `default`.
    pub fn u64_or(&self, key: &str, default: u64) -> u64 {
        self.get(key).and_then(Value::as_u64).unwrap_or(default)
    }

    /// Sets a value without validation (the `settings` family validates first). Returns whether it changed.
    pub fn set_raw(&mut self, key: &str, value: Value) -> bool {
        if self.values.get(key) == Some(&value) {
            return false;
        }
        self.values.insert(key.to_owned(), value);
        self.revision += 1;
        true
    }

    /// Every key and value, sorted by key.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.values.iter()
    }

    /// The configured undo depth.
    pub fn history_depth(&self) -> usize {
        usize::try_from(self.u64_or("history.depth", 50)).unwrap_or(50).clamp(1, 1000)
    }
}

/// Tab identity. Stable for the life of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TabId(pub u32);

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// A text edit in progress (state of the `text` family; see docs/exceptions.md E16).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TextEdit {
    /// The element being edited; `None` while it is still a draft.
    pub element: Option<ElementId>,
    /// True while no element exists yet.
    pub draft: bool,
}

/// State that a family keeps without touching `Session`: one value per Rust type, created on first use.
/// It is outside the rollback transaction (class P: buffers and library), so a command that must stay
/// atomic writes it last.
#[derive(Default)]
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any>>,
}

impl Extensions {
    /// Runs `f` on the value of type `T`, creating it with `T::default()` on first use.
    pub fn with<T: Default + 'static, R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        let slot = self.map.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(T::default()));
        match slot.downcast_mut::<T>() {
            Some(v) => f(v),
            // Unreachable: the slot is keyed by `TypeId::of::<T>()`. A scratch value keeps this total.
            None => f(&mut T::default()),
        }
    }

    /// The value of type `T`, if it was ever created.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.map.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    /// Stores a value, replacing an earlier one of the same type.
    pub fn set<T: 'static>(&mut self, value: T) {
        self.map.insert(TypeId::of::<T>(), Box::new(value));
    }
}

/// One tab: a document with its own selection, camera and history.
pub struct Tab {
    /// Identity.
    pub id: TabId,
    /// The document store with its rollback journal.
    pub doc: TabDoc,
    /// Undo and redo.
    pub history: UndoHistory,
    /// Selection and friends.
    pub observed: Observed,
    /// Camera.
    pub view: View,
    /// Store revision at the last save or open. The document is dirty when the revision differs.
    pub saved_revision: u64,
    /// Family-private per-tab state (save target, room, ...).
    pub ext: Extensions,
}

impl Tab {
    /// A new empty tab.
    pub fn new(id: TabId) -> Self {
        Self {
            id,
            doc: TabDoc::new(),
            history: UndoHistory::new(),
            observed: Observed::default(),
            view: View::default(),
            saved_revision: 0,
            ext: Extensions::default(),
        }
    }

    /// True when the document changed since it was saved or opened.
    pub fn dirty(&self) -> bool {
        self.doc.revision() != self.saved_revision
    }
}
