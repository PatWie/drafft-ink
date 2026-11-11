//! What the UI knows about the session, read once per change.
//!
//! The model is a plain snapshot: the command list with live availability and shortcuts, the tool,
//! the selection summary, the style of the selection (read with `style.get`), the settings, the
//! tabs and the history state. Surfaces read it and never reach into the `Session`.
//!
//! [`Model::refresh`] recaptures only when something changed (a cheap stamp over the session), so a
//! frame that was caused by a mouse move or a caret blink costs one hash and no registry walk.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

use drafftink_engine::spec::{CommandSpec, Host, Scope, TextGate};
use drafftink_engine::state::Tool;
use drafftink_engine::{Refusal, Session};
use drafftink_scene::SceneRead;
use serde_json::{Value, json};

use crate::keymap::{Chord, parse_chord};

/// One default binding of a command, parsed for this platform.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortcutView {
    /// Where it applies.
    pub scope: Scope,
    /// The chords that trigger it.
    pub chords: Vec<Chord>,
    /// The arguments the binding fixes (`null` for none). May hold `"$cursor"` placeholders.
    pub args: Value,
    /// The palette title of this binding, if it has its own.
    pub title: Option<&'static str>,
    /// The host runs this chord from its clipboard event ([`drafftink_engine::spec::Binding::host_clipboard`]).
    pub host_clipboard: bool,
}

/// A registered command as the UI sees it.
#[derive(Debug, Clone)]
pub struct CmdView {
    /// The id.
    pub id: &'static str,
    /// The title.
    pub title: &'static str,
    /// The family.
    pub family: &'static str,
    /// The summary.
    pub summary: &'static str,
    /// Listed in the palette.
    pub palette: bool,
    /// What a text edit does with it: `Commit` commands may run from a shortcut during a text edit.
    pub while_text_edit: TextGate,
    /// What a host does around the command (a dialog it closes, a toast it skips, a key that does not repeat).
    pub host: Host,
    /// `Ok(())`, or the refusal `execute` would return now. The text is the tooltip of a greyed control.
    pub available: Result<(), Refusal>,
    /// Default bindings.
    pub shortcuts: Vec<ShortcutView>,
    /// The calls without keys that have a title of their own.
    pub variants: Vec<VariantView>,
}

/// A named call of a command that has no key: fixed arguments and the title every surface shows for it.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantView {
    /// The fixed arguments.
    pub args: Value,
    /// The title.
    pub title: &'static str,
}

/// One property of the style panel, from `style.get`.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleProp {
    /// True when at least one target supports the property.
    pub applicable: bool,
    /// The common value, `"mixed"`, or `None` when not applicable. A removed fill is `null`.
    pub value: Option<Value>,
    /// The engine's text for why the property cannot be set now (the refusal `style.set` gives), when no target
    /// supports it.
    pub reason: Option<String>,
}

impl StyleProp {
    /// True when the targets disagree.
    pub fn is_mixed(&self) -> bool {
        self.value.as_ref().and_then(Value::as_str) == Some("mixed")
    }
}

/// The style of the selection, or the defaults for new elements when nothing is selected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleView {
    /// `selection` or `defaults`.
    pub source: String,
    /// The kinds among the targets (`rectangle`, `text`, ...).
    pub kinds: Vec<String>,
    /// Properties by name (`stroke_color`, `fill_color`, ...).
    pub props: BTreeMap<String, StyleProp>,
}

impl StyleView {
    /// A property.
    pub fn prop(&self, name: &str) -> Option<&StyleProp> {
        self.props.get(name)
    }

    /// True when the property applies to some target.
    pub fn applies(&self, name: &str) -> bool {
        self.props.get(name).is_some_and(|p| p.applicable)
    }

    /// The engine's reason why a property cannot be set, when it cannot.
    pub fn reason(&self, name: &str) -> Option<&str> {
        self.props.get(name)?.reason.as_deref()
    }

    /// The value as a string, when it is one and not `"mixed"`.
    pub fn str_value(&self, name: &str) -> Option<&str> {
        self.props.get(name)?.value.as_ref()?.as_str().filter(|s| *s != "mixed")
    }

    /// The value as a number.
    pub fn num_value(&self, name: &str) -> Option<f64> {
        self.props.get(name)?.value.as_ref()?.as_f64()
    }

    /// True when some target is of this kind.
    pub fn has_kind(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == kind)
    }
}

/// The common bounds of the selection, as `selection.get` reports them, in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundsView {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// One selected element, as far as the menus need to know it. Read with `element.query`, so it holds only
/// facts the query reports; a context menu uses them to offer the rows that fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedElement {
    /// The element id.
    pub id: String,
    /// What it is: `rectangle`, `text`, `frame`, ...
    pub kind: String,
    /// Its link, if it has one.
    pub link: Option<String>,
    /// The frame it belongs to.
    pub frame: Option<String>,
    /// The id of the text bound to it as its label, when it has one.
    pub label: Option<String>,
}

/// Whether the selected text is bold and italic. `Some(true)`: all of it. `Some(false)`: none of it. `None`: some of
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextFaces {
    /// The text is bold.
    pub bold: Option<bool>,
    /// The text is italic.
    pub italic: Option<bool>,
}

/// The camera, from `view.get`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewView {
    /// The world point at the middle of the viewport.
    pub scroll: [f64; 2],
    /// Screen pixels per world unit.
    pub zoom: f64,
    /// Viewport width in logical pixels.
    pub width: f64,
    /// Viewport height in logical pixels.
    pub height: f64,
}

impl ViewView {
    /// The world point under a screen point (logical pixels from the top-left of the canvas), the way
    /// `view.get` describes the camera. Used for dropping a library item where the pointer was released.
    pub fn screen_to_world(&self, p: egui::Pos2) -> [f64; 2] {
        let zoom = if self.zoom > 0.0 { self.zoom } else { 1.0 };
        [
            self.scroll[0] + (f64::from(p.x) - self.width / 2.0) / zoom,
            self.scroll[1] + (f64::from(p.y) - self.height / 2.0) / zoom,
        ]
    }
}

impl Default for ViewView {
    fn default() -> Self {
        Self { scroll: [0.0, 0.0], zoom: 1.0, width: 1280.0, height: 720.0 }
    }
}

/// One item of the shape library, from `library.list`.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    /// The id, which `library.insert` takes.
    pub id: String,
    /// The name.
    pub name: String,
    /// How many elements it holds.
    pub element_count: usize,
    /// Width of its bounds in world units.
    pub width: f64,
    /// Height of its bounds in world units.
    pub height: f64,
}

/// A tab as the tab bar shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TabView {
    /// The tab id such as `t2`.
    pub id: String,
    /// Position, counted from 0.
    pub index: usize,
    /// The document name.
    pub name: String,
    /// Unsaved changes.
    pub dirty: bool,
    /// The active tab.
    pub active: bool,
}

/// Undo and redo state of the active tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryView {
    /// Label of the step undo would take back.
    pub undo_label: Option<String>,
    /// Label of the step redo would re-apply.
    pub redo_label: Option<String>,
}

/// How the collaboration link stands. Fed by the app, which owns the connection.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum CollabStatus {
    /// No connection.
    #[default]
    Disconnected,
    /// Connecting.
    Connecting,
    /// Connected.
    Connected,
    /// The connection failed, with a reason.
    Failed(String),
}

/// Why a joined room is not usable yet, from `collab.status`'s `loading`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomLoading {
    /// Opening the connection.
    Connecting,
    /// Joined, waiting for the room's content.
    Syncing,
    /// A big update arrived and is about to be applied.
    Receiving {
        /// Its size in bytes.
        bytes: u64,
    },
    /// The content is in and the scene fills in slices.
    Importing {
        /// Items shown so far.
        done: usize,
        /// All items of the room.
        total: usize,
    },
}

impl RoomLoading {
    /// The state from the `loading` object of `collab.status`; `None` for a missing or unknown phase.
    pub fn from_status(v: &Value) -> Option<RoomLoading> {
        let count = |key: &str| v.get(key).and_then(Value::as_u64);
        match v.get("phase")?.as_str()? {
            "connecting" => Some(RoomLoading::Connecting),
            "syncing" => Some(RoomLoading::Syncing),
            "receiving" => Some(RoomLoading::Receiving { bytes: count("bytes")? }),
            "importing" => Some(RoomLoading::Importing {
                done: usize::try_from(count("done")?).ok()?,
                total: usize::try_from(count("total")?).ok()?,
            }),
            _ => None,
        }
    }

    /// How far the import is, in `0..=1`, when it is known (only while importing a room with items).
    pub fn fraction(&self) -> Option<f32> {
        match *self {
            RoomLoading::Importing { done, total } if total > 0 => Some((done as f32 / total as f32).clamp(0.0, 1.0)),
            _ => None,
        }
    }

    /// What to tell the user, in plain words. An import shows a whole-number percentage.
    pub fn text(&self) -> String {
        match *self {
            RoomLoading::Connecting => "Connecting to the room...".to_owned(),
            RoomLoading::Syncing => "Waiting for the room's content...".to_owned(),
            RoomLoading::Receiving { bytes } => format!("Receiving the room ({})...", byte_size(bytes)),
            RoomLoading::Importing { .. } => match self.fraction() {
                Some(f) => format!("Loading the room: {}%", (f * 100.0).floor() as u32),
                None => "Loading the room...".to_owned(),
            },
        }
    }
}

/// `bytes` as B, KB or MB with one decimal for the larger two.
fn byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

/// A collaborator.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// The id `collab.status` gives the peer: what `collab.follow` takes (names may repeat or be empty).
    pub id: String,
    /// The name; empty shows as "Anonymous".
    pub name: String,
    /// Cursor colour as `#rrggbb`.
    pub color: String,
    /// The peer has a cursor on the canvas right now.
    pub has_cursor: bool,
    /// The camera follows this peer (`collab.follow`).
    pub followed: bool,
}

impl Peer {
    /// A peer from one entry of `collab.status`'s `peers`. `None` when the entry has no id.
    pub fn from_status(v: &Value) -> Option<Peer> {
        let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Peer {
            id: text("id")?,
            name: text("name").unwrap_or_default(),
            color: text("color").unwrap_or_default(),
            has_cursor: v.get("pointer").is_some_and(|p| !p.is_null()),
            followed: v.get("followed").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Presence data for the collaboration button and panel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollabView {
    /// The link.
    pub status: CollabStatus,
    /// The room, when joined.
    pub room: Option<String>,
    /// Who else is in it.
    pub peers: Vec<Peer>,
    /// A server is chosen (`collab.connect` without a room): no socket is open and no room is joined, and the
    /// dialog offers the room and the access token. [`CollabView::status`] is still `Disconnected`.
    pub server_chosen: bool,
    /// Why the room is not usable yet, until its content is in.
    pub loading: Option<RoomLoading>,
}

impl CollabView {
    /// The view from the output of `collab.status` (`Session::query("collab.status", ..)`): the link, the
    /// room and the peers with their ids and whether this user follows them.
    pub fn from_status(out: &Value) -> CollabView {
        let text = |key: &str| out.get(key).and_then(Value::as_str);
        let status = match text("state") {
            Some("connecting") => CollabStatus::Connecting,
            Some("connected") => CollabStatus::Connected,
            Some("error") => {
                CollabStatus::Failed(text("error").or_else(|| text("text")).unwrap_or_default().to_owned())
            }
            _ => CollabStatus::Disconnected,
        };
        let peers = out
            .get("peers")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Peer::from_status).collect())
            .unwrap_or_default();
        let loading = out.get("loading").and_then(RoomLoading::from_status);
        CollabView {
            status,
            room: text("room").map(str::to_owned),
            peers,
            loading,
            server_chosen: text("state") == Some("ready"),
        }
    }
}

/// The desktop app's MCP server over HTTP, as the preferences show it. Written by the app through
/// [`crate::UiShell::set_agent_server`]; the web app has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentServerView {
    /// The address it listens on, while it runs.
    pub endpoint: Option<String>,
    /// The file with the token that clients send, while it runs.
    pub token_file: Option<String>,
    /// Why it could not start, in plain words.
    pub problem: Option<String>,
}

/// What the About dialog shows. The shell fills in what only it knows (the commit, the source address of a hosted copy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutInfo {
    /// The version of the app.
    pub version: String,
    /// The commit the app was built from, or `unknown` for a build outside a Git checkout.
    pub commit: String,
    /// The licence of the app, as an SPDX identifier.
    pub license: &'static str,
    /// Where the corresponding source is offered. AGPL-3.0 section 13 requires this for a copy that users reach over a
    /// network, so an operator who runs a modified copy sets it to their own source; the default is the project
    /// repository from the package metadata.
    pub source_url: String,
}

impl Default for AboutInfo {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: "unknown".to_owned(),
            license: "AGPL-3.0-only",
            source_url: env!("CARGO_PKG_REPOSITORY").to_owned(),
        }
    }
}

/// The snapshot.
#[derive(Debug, Clone)]
pub struct Model {
    /// Cmd instead of Ctrl for `Mod`. Set by the shell from egui's operating system.
    pub mac: bool,
    /// Every registered command, in registry order.
    pub commands: Vec<CmdView>,
    index: HashMap<&'static str, usize>,
    /// The tool in effect (a held tool wins).
    pub tool: Tool,
    /// Stay on the tool after drawing.
    pub tool_lock: bool,
    /// Selected elements.
    pub selection_count: usize,
    /// The document is view-only.
    pub view_only: bool,
    /// A text edit is running.
    pub text_editing: bool,
    /// A gesture is running.
    pub gesture_running: bool,
    /// A tool is held with a key (Space for the hand): releasing the key ends it.
    pub tool_held: bool,
    /// The point editor has a selected point (the `PointEditor` key scope applies).
    pub point_selected: bool,
    /// Bold and italic of the selected texts and labels, from the document; `None` when none is selected.
    pub text_faces: Option<TextFaces>,
    /// Bold and italic at the selection of the text editor while one runs. The editor owns the text then, so these
    /// win over [`Model::text_faces`]. Set by the app.
    pub editor_faces: Option<TextFaces>,
    /// The common bounds of the selection; `None` when nothing is selected.
    pub selection_bounds: Option<BoundsView>,
    /// The link of the selected element, when exactly one is selected and it has a link.
    pub selection_link: Option<String>,
    /// The selected elements (the first [`SELECTION_DETAIL_LIMIT`]), with what the menus need.
    pub selection: Vec<SelectedElement>,
    /// The camera.
    pub view: ViewView,
    /// The shape library, read while [`Model::wants_library`] is set.
    pub library: Vec<LibraryItem>,
    /// Why the library could not be read (no storage), as the refusal says.
    pub library_error: Option<String>,
    /// The library panel is open, so [`Model::refresh`] reads `library.list`. Set by the shell.
    pub wants_library: bool,
    /// The style of the selection or the defaults.
    pub style: StyleView,
    /// Settings by key.
    pub settings: BTreeMap<String, Value>,
    /// The tabs.
    pub tabs: Vec<TabView>,
    /// Undo and redo labels.
    pub history: HistoryView,
    /// The zoom in percent as the UI shows it.
    pub zoom_percent: f64,
    /// Page background of the active document, `#rrggbb`.
    pub background: String,
    /// Presence. Written by the app through `UiShell::set_collab`.
    pub collab: CollabView,
    /// The MCP server over HTTP. `None` where the app cannot serve one (the web).
    pub agent_server: Option<AgentServerView>,
    /// What the About dialog shows. Written by the app through `UiShell::set_about`.
    pub about: AboutInfo,
    stamp: Option<u64>,
    captured_mac: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            mac: false,
            commands: Vec::new(),
            index: HashMap::new(),
            tool: Tool::Select,
            tool_lock: false,
            selection_count: 0,
            view_only: false,
            text_editing: false,
            gesture_running: false,
            tool_held: false,
            point_selected: false,
            text_faces: None,
            editor_faces: None,
            selection_bounds: None,
            selection_link: None,
            selection: Vec::new(),
            view: ViewView::default(),
            library: Vec::new(),
            library_error: None,
            wants_library: false,
            style: StyleView::default(),
            settings: BTreeMap::new(),
            tabs: Vec::new(),
            history: HistoryView::default(),
            zoom_percent: 100.0,
            background: "#ffffff".to_owned(),
            collab: CollabView::default(),
            agent_server: None,
            about: AboutInfo::default(),
            stamp: None,
            captured_mac: false,
        }
    }
}

/// The text of a control for a command the registry does not have.
fn missing(id: &str) -> String {
    format!("`{id}` is not available in this build.")
}

impl Model {
    /// An empty model. Call [`Model::refresh`] before drawing.
    pub fn new(mac: bool) -> Self {
        Self { mac, ..Self::default() }
    }

    /// A hash of everything the model shows, cheap to compute.
    fn stamp_of(s: &Session) -> u64 {
        let mut h = DefaultHasher::new();
        let t = s.tab();
        t.id.0.hash(&mut h);
        t.doc.revision().hash(&mut h);
        s.tabs.len().hash(&mut h);
        s.active_index().hash(&mut h);
        for id in &t.observed.selection {
            id.as_str().hash(&mut h);
        }
        t.observed.editing_group.as_ref().map(|g| g.as_str().to_owned()).hash(&mut h);
        t.observed
            .linear_editor
            .as_ref()
            .map(|l| (l.element.as_str().to_owned(), l.selected_points.len()))
            .hash(&mut h);
        (s.tool.active.name(), s.tool.lock, s.tool.temporary.map(Tool::name)).hash(&mut h);
        format!("{:?}", s.tool.style_defaults).hash(&mut h);
        s.view_only.hash(&mut h);
        s.text_edit.is_some().hash(&mut h);
        s.gesture_running().hash(&mut h);
        s.settings.revision.hash(&mut h);
        t.view.zoom.to_bits().hash(&mut h);
        t.doc.undo_depth().hash(&mut h);
        t.doc.redo_depth().hash(&mut h);
        t.dirty().hash(&mut h);
        h.finish()
    }

    /// Forces the next [`Model::refresh`] to recapture (after commands ran).
    pub fn invalidate(&mut self) {
        self.stamp = None;
    }

    /// Recaptures from the session when its stamp changed (or after [`Model::invalidate`]). Returns
    /// true when it recaptured. The reads (`style.get`, `selection.get`, `element.query`, `view.get`,
    /// `library.list`) go through `Session::query`, which takes the session mutably but changes no state and
    /// leaves no ring entry.
    pub fn refresh(&mut self, s: &mut Session) -> bool {
        let stamp = Self::stamp_of(s);
        if self.stamp == Some(stamp) && !self.commands.is_empty() && self.captured_mac == self.mac {
            return false;
        }
        self.capture(s);
        self.captured_mac = self.mac;
        self.stamp = Some(stamp);
        true
    }

    fn capture(&mut self, s: &mut Session) {
        let mac = self.mac;
        self.commands = s
            .commands()
            .into_iter()
            .map(|c| {
                let spec = s.spec(c.id);
                CmdView {
                    id: c.id,
                    title: c.title,
                    family: c.family,
                    summary: c.summary,
                    palette: spec.is_some_and(|sp| sp.palette),
                    while_text_edit: spec.map_or(TextGate::Refused, |sp| sp.while_text_edit),
                    host: spec.map_or(Host::NONE, |sp| sp.host),
                    available: c.available,
                    shortcuts: spec.map(|sp| shortcuts_of(sp, mac)).unwrap_or_default(),
                    variants: spec.map(variants_of).unwrap_or_default(),
                }
            })
            .collect();
        self.index = self.commands.iter().enumerate().map(|(i, c)| (c.id, i)).collect();

        self.tool = s.tool.effective();
        self.tool_held = s.tool.temporary.is_some();
        self.tool_lock = s.tool.lock;
        let t = s.tab();
        self.selection_count = t.observed.selection.len();
        self.point_selected = t.observed.linear_editor.as_ref().is_some_and(|l| !l.selected_points.is_empty());
        self.zoom_percent = t.view.percent();
        self.background = t.doc.store().meta().background.to_hex();
        self.history = HistoryView { undo_label: t.doc.undo_label(), redo_label: t.doc.redo_label() };
        let active = s.active_index();
        self.tabs = s
            .tabs
            .iter()
            .enumerate()
            .map(|(index, t)| TabView {
                id: t.id.to_string(),
                index,
                name: t.doc.store().meta().name.clone(),
                dirty: t.dirty(),
                active: index == active,
            })
            .collect();
        self.view_only = s.view_only;
        self.text_editing = s.text_edit.is_some();
        self.gesture_running = s.gesture_running();
        self.settings = s.settings.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let read = read_selection(s);
        self.selection_bounds = read.bounds;
        self.selection_link = match read.items.as_slice() {
            [one] if self.selection_count == 1 => one.link.clone(),
            _ => None,
        };
        self.text_faces = read_text_faces(s, &read.items);
        self.selection = read.items;
        self.style = read_style(s);
        self.view = read_view(s);
        (self.library, self.library_error) = if self.wants_library { read_library(s) } else { (Vec::new(), None) };
    }

    // ---- reading ----

    /// Bold and italic of the text the panel acts on: the editor's selection while one runs, else the selected
    /// texts. `None` when there is no text to format.
    pub fn faces(&self) -> Option<TextFaces> {
        self.editor_faces.or(self.text_faces)
    }

    /// The command with this id.
    pub fn command(&self, id: &str) -> Option<&CmdView> {
        self.index.get(id).and_then(|&i| self.commands.get(i))
    }

    /// True when the command is registered and can run now.
    pub fn is_available(&self, id: &str) -> bool {
        self.command(id).is_some_and(|c| c.available.is_ok())
    }

    /// Why the command cannot run now: the refusal text, or a note that it is not registered. `None`
    /// when it can run.
    pub fn refusal_text(&self, id: &str) -> Option<String> {
        match self.command(id) {
            None => Some(missing(id)),
            Some(c) => c.available.as_ref().err().map(|r| r.message.clone()),
        }
    }

    /// The title of a command, or the id when it is not registered.
    pub fn title<'a>(&'a self, id: &'a str) -> &'a str {
        self.command(id).map_or(id, |c| c.title)
    }

    /// A setting.
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }

    /// A boolean setting.
    pub fn setting_bool(&self, key: &str) -> bool {
        self.setting(key).and_then(Value::as_bool).unwrap_or(false)
    }

    /// A string setting.
    pub fn setting_str(&self, key: &str) -> Option<&str> {
        self.setting(key).and_then(Value::as_str)
    }

    /// A number setting.
    pub fn setting_f64(&self, key: &str) -> Option<f64> {
        self.setting(key).and_then(Value::as_f64)
    }

    /// The text of the key combination for the first binding of `id` whose fixed arguments equal
    /// `args` (`null`/`{}` for none), such as `Ctrl+Z` or `Cmd+Z`. `None` when there is no such binding.
    pub fn shortcut_label(&self, id: &str, args: &Value) -> Option<String> {
        let cmd = self.command(id)?;
        let want = normalize_args(args);
        cmd.shortcuts
            .iter()
            .find(|b| normalize_args(&b.args) == want && !b.chords.is_empty())
            .map(|b| b.chords.iter().map(|c| c.label(self.mac)).collect::<Vec<_>>().join(" / "))
    }

    /// The name of a control for a command with these arguments: the title of the binding or variant that fixes
    /// the same arguments (`Rectangle tool`, `Copy as SVG`), else the title of the command (`Undo`).
    pub fn label(&self, id: &str, args: &Value) -> String {
        let Some(c) = self.command(id) else {
            return id.to_owned();
        };
        let want = normalize_args(args);
        c.shortcuts
            .iter()
            .find(|b| normalize_args(&b.args) == want && b.title.is_some())
            .and_then(|b| b.title)
            .or_else(|| c.variants.iter().find(|v| v.args == want).map(|v| v.title))
            .unwrap_or(c.title)
            .to_owned()
    }

    /// The tooltip of a command control: its name and its first shortcut.
    pub fn tooltip(&self, id: &str, args: &Value) -> String {
        let title = self.label(id, args);
        match self.shortcut_label(id, args) {
            Some(k) => format!("{title} ({k})"),
            None => title,
        }
    }
}

/// `{}`, `null` and "no args" compare equal.
fn normalize_args(v: &Value) -> Value {
    match v {
        Value::Null => json!({}),
        other => other.clone(),
    }
}

fn variants_of(spec: &CommandSpec) -> Vec<VariantView> {
    spec.variants
        .iter()
        .map(|v| VariantView { args: serde_json::from_str(v.args).unwrap_or(Value::Null), title: v.title })
        .collect()
}

fn shortcuts_of(spec: &CommandSpec, mac: bool) -> Vec<ShortcutView> {
    spec.shortcuts
        .iter()
        .map(|b| ShortcutView {
            scope: b.when,
            chords: b.chords(mac).iter().filter_map(|c| parse_chord(c)).collect(),
            args: if b.args.is_empty() { Value::Null } else { serde_json::from_str(b.args).unwrap_or(Value::Null) },
            title: b.title,
            host_clipboard: b.host_clipboard,
        })
        .collect()
}

/// How many selected elements [`Model::selection`] describes. A larger selection still has its count, bounds and style.
pub const SELECTION_DETAIL_LIMIT: usize = 50;

/// What the model reads about the selection.
struct SelectionRead {
    bounds: Option<BoundsView>,
    items: Vec<SelectedElement>,
}

fn text_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Reads the selection with `selection.get` (the common bounds) and, for a small selection, `element.query`
/// (the facts the menus need).
fn read_selection(s: &mut Session) -> SelectionRead {
    let Ok(got) = s.query("selection.get", json!({})) else {
        return SelectionRead { bounds: None, items: Vec::new() };
    };
    let number = |b: &Value, key: &str| b.get(key).and_then(Value::as_f64);
    let bounds = got.get("bounds").filter(|b| !b.is_null()).and_then(|b| {
        Some(BoundsView { x: number(b, "x")?, y: number(b, "y")?, w: number(b, "w")?, h: number(b, "h")? })
    });
    let count = got.get("count").and_then(Value::as_u64).unwrap_or(0);
    let items = if count == 0 { Vec::new() } else { read_selected_elements(s) };
    SelectionRead { bounds, items }
}

fn read_selected_elements(s: &mut Session) -> Vec<SelectedElement> {
    let args = json!({
        "filter": { "selected": true },
        "fields": ["id", "kind", "link", "frame", "bound_text"],
        "limit": SELECTION_DETAIL_LIMIT,
    });
    let Ok(out) = s.query("element.query", args) else {
        return Vec::new();
    };
    let items = out.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    items
        .iter()
        .filter_map(|item| {
            Some(SelectedElement {
                id: text_of(item, "id")?.to_owned(),
                kind: text_of(item, "kind")?.to_owned(),
                link: text_of(item, "link").map(str::to_owned),
                frame: text_of(item, "frame").map(str::to_owned),
                label: text_of(item, "bound_text").map(str::to_owned),
            })
        })
        .collect()
}

/// Bold and italic of the selected texts and of the labels of the selected shapes, from the runs `element.query`
/// reports.
fn read_text_faces(s: &mut Session, selected: &[SelectedElement]) -> Option<TextFaces> {
    let ids: Vec<&str> =
        selected.iter().flat_map(|e| if e.kind == "text" { Some(e.id.as_str()) } else { e.label.as_deref() }).collect();
    if ids.is_empty() {
        return None;
    }
    let out = s
        .query(
            "element.query",
            json!({ "filter": { "ids": ids }, "fields": ["id", "text", "runs"], "limit": SELECTION_DETAIL_LIMIT }),
        )
        .ok()?;
    let items = out.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    // How many characters have the face, against how many there are, per text.
    let counts = |item: &Value, face: &str| -> (usize, usize) {
        let chars = text_of(item, "text").map_or(0, |t| t.chars().count());
        let runs = item.get("runs").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let with: u64 = runs
            .iter()
            .filter(|r| r.get(face).and_then(Value::as_bool).unwrap_or(false))
            .map(|r| {
                let bound = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
                bound("end").saturating_sub(bound("start"))
            })
            .sum();
        (usize::try_from(with).unwrap_or(usize::MAX).min(chars), chars)
    };
    let state = |face: &str| {
        let (with, all) = items.iter().map(|i| counts(i, face)).fold((0, 0), |(w, a), (cw, ca)| (w + cw, a + ca));
        match (with, all) {
            (_, 0) => Some(false),
            (0, _) => Some(false),
            (w, a) if w == a => Some(true),
            _ => None,
        }
    };
    (!items.is_empty()).then(|| TextFaces { bold: state("bold"), italic: state("italic") })
}

/// Reads the camera with `view.get`.
fn read_view(s: &mut Session) -> ViewView {
    let Ok(got) = s.query("view.get", json!({})) else {
        return ViewView::default();
    };
    let number = |v: &Value, key: &str| v.get(key).and_then(Value::as_f64);
    let scroll =
        got.get("scroll").and_then(Value::as_array).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
    let viewport = got.get("viewport");
    match (scroll, number(&got, "zoom"), viewport) {
        (Some(scroll), Some(zoom), Some(vp)) => ViewView {
            scroll,
            zoom,
            width: number(vp, "w").unwrap_or(1280.0),
            height: number(vp, "h").unwrap_or(720.0),
        },
        _ => ViewView::default(),
    }
}

/// Reads the library with `library.list`. A refusal (no storage) is kept as text for the panel.
fn read_library(s: &mut Session) -> (Vec<LibraryItem>, Option<String>) {
    match s.query("library.list", json!({})) {
        Err(r) => (Vec::new(), Some(r.message)),
        Ok(out) => {
            let items = out.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let items = items
                .iter()
                .filter_map(|item| {
                    let number = |key: &str| item.pointer(&format!("/bounds/{key}")).and_then(Value::as_f64);
                    Some(LibraryItem {
                        id: text_of(item, "id")?.to_owned(),
                        name: text_of(item, "name").unwrap_or_default().to_owned(),
                        element_count: item
                            .get("element_count")
                            .and_then(Value::as_u64)
                            .and_then(|n| usize::try_from(n).ok())
                            .unwrap_or(0),
                        width: number("w").unwrap_or(0.0),
                        height: number("h").unwrap_or(0.0),
                    })
                })
                .collect();
            (items, None)
        }
    }
}

/// Reads the style panel's data with `style.get`: the style of the selection, or the defaults for new
/// elements when nothing is selected.
fn read_style(s: &mut Session) -> StyleView {
    let Ok(got) = s.query("style.get", json!({})) else {
        return StyleView::default();
    };
    let kinds = got
        .get("kinds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let props = got
        .get("props")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(name, info)| {
                    let applicable = info.get("applicable").and_then(Value::as_bool).unwrap_or(false);
                    let reason = text_of(info, "reason").map(str::to_owned);
                    (name.clone(), StyleProp { applicable, value: info.get("value").cloned(), reason })
                })
                .collect()
        })
        .unwrap_or_default();
    StyleView { source: text_of(&got, "source").unwrap_or_default().to_owned(), kinds, props }
}

/// The JSON Schema of a command's arguments (a JSON object with `properties`, `required`, `$defs`).
///
/// `None` for an unknown command, and while the schemas are not there yet (the web build fetches them as a file
/// when the page starts; see `drafftink_engine::schemas`).
pub fn args_schema(id: &str) -> Option<Value> {
    if !drafftink_engine::schemas::available() {
        return None;
    }
    let spec = drafftink_engine::Registry::global().find(id)?;
    serde_json::to_value(spec.args_schema()).ok()
}

/// What the engine says about one argument of a command (the doc comment of the field), when it says anything.
pub fn arg_doc(id: &str, arg: &str) -> Option<String> {
    args_schema(id)?.pointer(&format!("/properties/{arg}/description")).and_then(Value::as_str).map(str::to_owned)
}

/// The worked example of a command's arguments, when it has one.
pub fn example_args(id: &str) -> Option<Value> {
    let spec = drafftink_engine::Registry::global().find(id)?;
    if spec.example.is_empty() { None } else { serde_json::from_str(spec.example).ok() }
}
