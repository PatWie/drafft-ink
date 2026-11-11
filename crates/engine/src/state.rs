//! The state classes of a session (docs/commands.md §1.1) that are plain data: observed editor state,
//! view, tool, settings, tabs, text-edit state and the typed extension map for family-private state.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_doc::ObservedState;
use drafftink_edit::arrow_defaults::ArrowDefaults;
use drafftink_edit::style::StyleDefaults;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Brush, ElementId, GroupId, PressureMode, Style, TextStyle};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::TabDoc;

/// Zoom that the UI shows as 100% (`zoom / BASE_ZOOM * 100`).
pub const BASE_ZOOM: f64 = 1.68;
/// Smallest zoom.
pub const ZOOM_MIN: f64 = 0.1;
/// Largest zoom.
pub const ZOOM_MAX: f64 = 30.0;

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

impl Observed {
    /// The form the replicated store keeps with an undo step.
    pub fn to_state(&self) -> ObservedState {
        ObservedState {
            selection: self.selection.iter().cloned().collect(),
            selected_groups: self.selected_groups.iter().cloned().collect(),
            editing_group: self.editing_group.clone(),
            point_editor: self.linear_editor.as_ref().map(|l| l.element.clone()),
        }
    }

    /// Back from the stored form. Selected points of a point editor are not stored and start empty.
    pub fn from_state(s: &ObservedState) -> Self {
        Self {
            selection: s.selection.iter().cloned().collect(),
            selected_groups: s.selected_groups.iter().cloned().collect(),
            editing_group: s.editing_group.clone(),
            linear_editor: s
                .point_editor
                .as_ref()
                .map(|e| LinearEditor { element: e.clone(), selected_points: BTreeSet::new() }),
        }
    }
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
    Lasso,
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
            Self::Lasso => "lasso",
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
    /// Style for new shapes (set by `style.set`).
    pub style_defaults: Style,
    /// Font for new text.
    pub text_defaults: TextStyle,
    /// Brush for new freehand strokes.
    pub brush_default: Brush,
    /// Pressure mode for new freehand strokes.
    pub pressure_default: PressureMode,
    /// Path type and heads of new arrows (`tool.set_arrow`; the settings `arrow.type`, `arrow.start_head` and
    /// `arrow.end_head` keep them).
    pub arrow_default: ArrowDefaults,
}

impl Default for ToolState {
    fn default() -> Self {
        let d = StyleDefaults::default();
        Self {
            active: Tool::Select,
            lock: false,
            temporary: None,
            style_defaults: d.style,
            text_defaults: d.text,
            brush_default: d.brush,
            pressure_default: d.pressure_mode,
            arrow_default: ArrowDefaults::default(),
        }
    }
}

impl ToolState {
    /// The tool that is in effect now (the held one, else the active one).
    pub fn effective(&self) -> Tool {
        self.temporary.unwrap_or(self.active)
    }

    /// All defaults for new elements in one value.
    pub fn defaults(&self) -> StyleDefaults {
        StyleDefaults {
            style: self.style_defaults.clone(),
            text: self.text_defaults.clone(),
            brush: self.brush_default,
            pressure_mode: self.pressure_default,
        }
    }

    /// Replaces all defaults for new elements.
    pub fn set_defaults(&mut self, d: StyleDefaults) {
        self.style_defaults = d.style;
        self.text_defaults = d.text;
        self.brush_default = d.brush;
        self.pressure_default = d.pressure_mode;
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
    /// Every key at its default (the table of keys is in `cmd/settings/keys.rs`).
    fn default() -> Self {
        Self { values: crate::cmd::settings::keys::defaults(), revision: 0 }
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
        usize::try_from(self.u64_or("history.depth", 200)).unwrap_or(200).clamp(1, 1000)
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

/// The text editor's latest result as `text.edit_end` arguments. The editor stages it while the person types, so
/// that a command with the `Commit` gate (a toolbar click, `Mod+S`) ends the edit with what was typed instead of
/// losing it. It is not part of the document or of any undo step.
#[derive(Debug, Default, Clone)]
pub(crate) struct StagedEdit(pub(crate) Option<serde_json::Value>);

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
            observed: Observed::default(),
            view: View::default(),
            saved_revision: 0,
            ext: Extensions::default(),
        }
    }

    /// True when the content differs from what was saved or opened. Undoing back to the saved state is clean.
    pub fn dirty(&self) -> bool {
        !self.doc.same_content(self.doc.revision(), self.saved_revision)
    }
}
