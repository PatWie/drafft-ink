//! State that exists only in the UI: which dialog is open, the palette query, the toasts.
//!
//! None of it is document, selection, tool or setting state, so none of it belongs to the `Session`
//! (docs/commands.md §1.1, "UI state is not in the Session"). Everything the UI shows about the
//! document comes from the [`crate::Model`]. Text-field buffers that belong to one dialog live in
//! egui's own memory, keyed by the dialog.

use drafftink_engine::Refusal;
use serde_json::Value;

use crate::action::{Call, HostRequest, Verb};

/// How long a toast is fully visible, in seconds.
pub const TOAST_HOLD_S: f64 = 4.0;
/// How long a toast takes to fade, in seconds.
pub const TOAST_FADE_S: f64 = 0.6;
/// Toasts shown at once. Older ones are dropped.
pub const MAX_TOASTS: usize = 4;

/// A refusal (or a note) shown for a few seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// The text, unchanged from the refusal.
    pub text: String,
    /// The refusal code, for tests and for styling.
    pub code: String,
    /// When it appeared (egui time, seconds).
    pub born: f64,
    /// A next step offered by the refusal, as a button.
    pub suggestion: Option<Call>,
}

impl Toast {
    /// A toast with the refusal's text and its first suggestion.
    pub fn from_refusal(r: &Refusal, now: f64) -> Self {
        let suggestion = r.suggest.first().map(|s| {
            Call::new(
                drafftink_engine::Origin::Ui,
                s.command.clone(),
                s.args.clone().unwrap_or_else(|| serde_json::json!({})),
            )
        });
        Self { text: r.message.clone(), code: r.code.to_owned(), born: now, suggestion }
    }

    /// A plain note.
    pub fn note(text: impl Into<String>, now: f64) -> Self {
        Self { text: text.into(), code: "note".to_owned(), born: now, suggestion: None }
    }

    /// Opacity from 1 down to 0, or `None` when the toast is over.
    pub fn alpha(&self, now: f64) -> Option<f32> {
        let age = now - self.born;
        if age < TOAST_HOLD_S {
            Some(1.0)
        } else if age < TOAST_HOLD_S + TOAST_FADE_S {
            Some((1.0 - (age - TOAST_HOLD_S) / TOAST_FADE_S).clamp(0.0, 1.0) as f32)
        } else {
            None
        }
    }
}

/// Which colour the popover edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTarget {
    /// Stroke colour (`style.set` `stroke_color`).
    Stroke,
    /// Fill colour (`style.set` `fill_color`).
    Fill,
    /// Page background (`doc.set_background`).
    Background,
}

/// The colours last picked in the full colour popover, one for the stroke and one for the fill. The top bar keeps
/// a swatch of each next to the colour wheel, so a colour from the grid is one click away afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastPicked {
    stroke: Option<String>,
    fill: Option<String>,
}

impl LastPicked {
    /// The colour last picked for the stroke or the fill, as a palette name or `#rrggbb[aa]`. The page background
    /// is not remembered.
    pub fn of(&self, target: ColorTarget) -> Option<&str> {
        match target {
            ColorTarget::Stroke => self.stroke.as_deref(),
            ColorTarget::Fill => self.fill.as_deref(),
            ColorTarget::Background => None,
        }
    }

    /// Remembers `color` for the stroke or the fill.
    pub fn remember(&mut self, target: ColorTarget, color: String) {
        match target {
            ColorTarget::Stroke => self.stroke = Some(color),
            ColorTarget::Fill => self.fill = Some(color),
            ColorTarget::Background => {}
        }
    }
}

/// The command palette.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaletteState {
    /// What the user typed.
    pub query: String,
    /// The highlighted row (index into the filtered list).
    pub selected: usize,
}

/// The equation editor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EquationState {
    /// The LaTeX being edited.
    pub latex: String,
    /// Set once the field has been focused, so focus is requested only on the first frame.
    pub focused: bool,
}

/// A question the UI asks before it repeats a refused command with a decision filled in
/// (`unsaved_changes`: discard, save first, or cancel).
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    /// The command that was refused.
    pub call: Call,
    /// The refusal text.
    pub message: String,
}

/// One element a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// The element id (the argument of `selection.select`).
    pub id: String,
    /// What the element is: `rectangle`, `text`, ...
    pub kind: String,
    /// The text of the element, else its name, else its id.
    pub label: String,
    /// The middle of the element in world units, for centring the view on it.
    pub center: Option<[f64; 2]>,
}

/// What the last `element.query` of the search box found.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    /// The text that was searched for.
    pub query: String,
    /// How many elements match (more than `hits` holds when the page was cut).
    pub total: usize,
    /// The elements of the page, in the order the command gave them.
    pub hits: Vec<SearchHit>,
}

impl SearchResults {
    /// Reads the output of `element.query`. Items that lack an id are skipped.
    pub(crate) fn from_output(query: &str, out: &Value) -> Self {
        let items = out.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let hits: Vec<SearchHit> = items.iter().filter_map(SearchHit::from_item).collect();
        let total =
            out.get("total").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).unwrap_or(hits.len());
        Self { query: query.to_owned(), total, hits }
    }
}

impl SearchHit {
    fn from_item(item: &Value) -> Option<Self> {
        let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let id = text("id")?;
        let number = |b: &Value, key: &str| b.get(key).and_then(Value::as_f64);
        let center = item
            .get("bounds")
            .and_then(|b| Some([number(b, "x")? + number(b, "w")? / 2.0, number(b, "y")? + number(b, "h")? / 2.0]));
        Some(Self {
            id: id.to_owned(),
            kind: text("kind").unwrap_or("element").to_owned(),
            label: text("text").or_else(|| text("name")).unwrap_or(id).to_owned(),
            center,
        })
    }
}

/// The context menu: where it was asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    /// The click, in screen points.
    pub screen: egui::Pos2,
    /// The same point in world units, when the canvas layer knows it. `Paste` puts its content here.
    pub world: Option<[f64; 2]>,
    /// The id of a locked element the menu is about (it cannot be selected): the menu offers to unlock it.
    pub locked: Option<String>,
    /// Which opening this is, so that a menu opened again starts with no flyout open.
    pub opened: u64,
}

/// The link dialog (`ui.open_link_editor`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkEditorState {
    /// The text in the field. `None` until the first frame, when it takes the link of the selected element.
    pub text: Option<String>,
    /// Set once the field has been focused, so focus is requested only on the first frame.
    pub focused: bool,
}

/// What a rename dialog renames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTarget {
    /// The document of the active tab: `doc.rename`.
    Document,
    /// A tab, by its id: `tab.rename`.
    Tab(String),
    /// A library item, by its id: `library.rename`.
    LibraryItem(String),
    /// A frame element, by its id: `frame.rename`.
    Frame(String),
}

/// The rename dialog: what is renamed and the text in the field.
#[derive(Debug, Clone, PartialEq)]
pub struct RenameState {
    /// What the name belongs to.
    pub target: RenameTarget,
    /// The text in the field.
    pub text: String,
    /// Set once the field has been focused, so focus is requested only on the first frame.
    pub focused: bool,
}

impl RenameState {
    /// A dialog for a target, starting from its current name.
    pub fn new(target: RenameTarget, current: impl Into<String>) -> Self {
        Self { target, text: current.into(), focused: false }
    }
}

/// The Save As dialog: the document name, asked before the save picker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveAsState {
    /// The text in the field. `None` until the first frame, when it takes the name of the document.
    pub text: Option<String>,
    /// Set once the field has been focused.
    pub focused: bool,
}

/// What `doc.export` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    /// A picture.
    #[default]
    Png,
    /// A vector picture.
    Svg,
    /// The document in the native format.
    Json,
    /// The document as an `.excalidraw` file.
    Excalidraw,
}

impl ExportFormat {
    /// Every format, in the order of the dialog.
    pub const ALL: [ExportFormat; 4] = [Self::Png, Self::Svg, Self::Json, Self::Excalidraw];

    /// The value of the `format` argument.
    pub fn arg(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
            Self::Json => "json",
            Self::Excalidraw => "excalidraw",
        }
    }

    /// The format of a `format` argument (`png`, `svg`, `json`, `excalidraw`).
    pub fn from_arg(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.arg() == word)
    }

    /// The label on the button.
    pub fn title(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Svg => "SVG",
            Self::Json => "JSON",
            Self::Excalidraw => ".excalidraw",
        }
    }

    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
            Self::Json => "json",
            Self::Excalidraw => "excalidraw",
        }
    }

    /// True for the formats that are pictures: scale, background, dark and padding mean something for them.
    pub fn is_picture(self) -> bool {
        matches!(self, Self::Png | Self::Svg)
    }
}

/// The export dialog. The options start from the `export.*` settings and are the dialog's own until the user
/// changes the settings in the preferences.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportState {
    /// The format.
    pub format: ExportFormat,
    /// Export only the selected elements (`scope: selection`), else everything (`scope: all`).
    pub selection_only: bool,
    /// Pixels per world unit.
    pub scale: f64,
    /// Draw the page background.
    pub background: bool,
    /// Dark theme colours.
    pub dark: bool,
    /// Embed the scene in the picture so that it can be opened again.
    pub embed_scene: bool,
    /// The options were taken from the settings.
    pub loaded: bool,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            format: ExportFormat::Png,
            selection_only: false,
            scale: 2.0,
            background: true,
            dark: false,
            embed_scene: true,
            loaded: false,
        }
    }
}

/// The popup that Tab and Shift+Tab open over a converted selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvertPopup {
    /// The type the last conversion gave (`element.convert`'s `to`).
    pub current: Option<String>,
}

/// The context menu of a tab.
#[derive(Debug, Clone, PartialEq)]
pub struct TabMenu {
    /// The tab id.
    pub tab: String,
    /// Where the right click was, in screen points.
    pub screen: egui::Pos2,
}

/// How much room the layout leaves for fingers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutPreference {
    /// Larger targets and collapsible panels on a narrow window or when touch was used.
    #[default]
    Auto,
    /// Always the touch layout.
    Touch,
    /// Always the desktop layout.
    Desktop,
}

impl LayoutPreference {
    /// Every choice, in the order of the preferences.
    pub const ALL: [LayoutPreference; 3] = [Self::Auto, Self::Touch, Self::Desktop];

    /// The label of the choice.
    pub fn title(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Touch => "Touch",
            Self::Desktop => "Desktop",
        }
    }
}

/// The two panels that collapse in the touch layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    /// The tool column.
    Tools,
    /// The properties panel.
    Properties,
}

/// Windows narrower than this (in points) get the touch layout when the preference is `Auto`.
pub const NARROW_WIDTH: f32 = 720.0;

/// Everything the UI remembers between frames.
#[derive(Debug, Default)]
pub struct UiState {
    /// The hamburger menu is open.
    pub menu_open: bool,
    /// The palette, when open.
    pub palette: Option<PaletteState>,
    /// The shortcuts dialog is open.
    pub shortcuts_open: bool,
    /// The About dialog is open.
    pub about_open: bool,
    /// The collaboration dialog is open.
    pub collab_open: bool,
    /// The colour popover, when open.
    pub color_popover: Option<ColorTarget>,
    /// The colours last picked in the colour popover.
    pub last_picked: LastPicked,
    /// The font family popover is open.
    pub font_picker_open: bool,
    /// The search box is open.
    pub search_open: bool,
    /// What the last search found, while the search box is open.
    pub search_results: Option<SearchResults>,
    /// The stats panel is shown.
    pub stats_open: bool,
    /// The chrome is hidden.
    pub zen: bool,
    /// The equation editor, when open.
    pub equation: Option<EquationState>,
    /// The link dialog, when open.
    pub link_editor: Option<LinkEditorState>,
    /// The context menu, when open.
    pub context_menu: Option<ContextMenu>,
    /// The recent documents list is open.
    pub recent_open: bool,
    /// A pending question.
    pub confirm: Option<Confirm>,
    /// The rename dialog, when open.
    pub rename: Option<RenameState>,
    /// The Save As dialog, when open.
    pub save_as: Option<SaveAsState>,
    /// The export dialog, when open.
    pub export: Option<ExportState>,
    /// The preferences dialog is open.
    pub preferences_open: bool,
    /// The library panel is open.
    pub library_open: bool,
    /// The name field of the library panel (for `library.add`).
    pub library_name: String,
    /// The Tab convert popup, when open.
    pub convert_popup: Option<ConvertPopup>,
    /// The context menu of a tab, when open.
    pub tab_menu: Option<TabMenu>,
    /// How many context menus were opened (the next one's [`ContextMenu::opened`]).
    menus_opened: u64,
    /// The layout preference (kept by the app between runs if it wants to).
    pub layout: LayoutPreference,
    /// A touch event was seen, or the app says the device is touch-first.
    pub touch_first: bool,
    /// What the user chose for each collapsible panel (`None`: the default for the window).
    panel_collapsed: [Option<bool>; 2],
    /// Toasts, oldest first.
    pub toasts: Vec<Toast>,
    /// Text the app pasted for the equation editor.
    pasted_text: Option<String>,
    /// The id of the gesture a scrubbed control began.
    scrub: Option<String>,
    /// The theme setting the visuals were last built for.
    pub(crate) applied_theme: Option<String>,
    /// Commands that were refused with `render_pending`: the picture is being drawn (the browser reads it back
    /// from the GPU asynchronously). Each frame runs them again until the port has the picture.
    pub(crate) waiting: Vec<Call>,
}

impl UiState {
    /// A fresh state: nothing open.
    pub fn new() -> Self {
        Self::default()
    }

    /// The commands to run again this frame because their picture was not ready (`UiShell::frame` does this).
    pub fn retries(&self) -> Vec<Call> {
        self.waiting.clone()
    }

    /// True while a command waits for its picture.
    pub fn waiting_for_picture(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// True when something that Esc should close is open.
    pub fn any_overlay_open(&self) -> bool {
        self.menu_open
            || self.palette.is_some()
            || self.shortcuts_open
            || self.about_open
            || self.collab_open
            || self.color_popover.is_some()
            || self.font_picker_open
            || self.search_open
            || self.equation.is_some()
            || self.link_editor.is_some()
            || self.context_menu.is_some()
            || self.recent_open
            || self.confirm.is_some()
            || self.rename.is_some()
            || self.save_as.is_some()
            || self.export.is_some()
            || self.preferences_open
            || self.convert_popup.is_some()
            || self.tab_menu.is_some()
    }

    /// True while a popup menu is open (the hamburger, the context menu, a tab menu). The arrow keys and Enter
    /// belong to it, not to the canvas.
    pub fn popup_menu_open(&self) -> bool {
        self.menu_open || self.context_menu.is_some() || self.tab_menu.is_some()
    }

    /// True when a modal dialog is open and keys must not reach the canvas.
    pub fn modal_open(&self) -> bool {
        self.palette.is_some()
            || self.shortcuts_open
            || self.about_open
            || self.collab_open
            || self.equation.is_some()
            || self.link_editor.is_some()
            || self.confirm.is_some()
            || self.search_open
            || self.rename.is_some()
            || self.save_as.is_some()
            || self.export.is_some()
            || self.preferences_open
    }

    /// Closes everything that Esc closes. Returns true when something was open.
    pub fn close_overlays(&mut self) -> bool {
        let was = self.any_overlay_open();
        self.menu_open = false;
        self.palette = None;
        self.shortcuts_open = false;
        self.about_open = false;
        self.collab_open = false;
        self.color_popover = None;
        self.font_picker_open = false;
        self.search_open = false;
        self.search_results = None;
        self.equation = None;
        self.link_editor = None;
        self.context_menu = None;
        self.recent_open = false;
        self.confirm = None;
        self.rename = None;
        self.save_as = None;
        self.export = None;
        self.preferences_open = false;
        self.convert_popup = None;
        self.tab_menu = None;
        was
    }

    /// Applies a verb to the UI state. Verbs that need the operating system are added to `host`.
    pub fn apply_verb(&mut self, verb: Verb, host: &mut Vec<HostRequest>) {
        match verb {
            Verb::CloseOverlays => {
                self.close_overlays();
            }
            Verb::ToggleMenu => {
                let open = !self.menu_open;
                self.close_overlays();
                self.menu_open = open;
            }
            Verb::OpenFile => host.push(HostRequest::PickFileToOpen),
            Verb::OpenRecent => {
                self.close_overlays();
                self.recent_open = true;
            }
            Verb::SaveAs => {
                self.close_overlays();
                self.save_as = Some(SaveAsState::default());
            }
            Verb::OpenExport => {
                self.close_overlays();
                self.export = Some(ExportState::default());
            }
            Verb::OpenPreferences => {
                self.close_overlays();
                self.preferences_open = true;
            }
            Verb::ToggleLibrary => self.library_open = !self.library_open,
            Verb::RenameDocument => {
                self.close_overlays();
                // The field takes the document's name on the first frame it is drawn.
                self.rename = Some(RenameState::new(RenameTarget::Document, String::new()));
            }
            Verb::InsertImage => host.push(HostRequest::PickImage),
            Verb::OpenColorStroke => self.toggle_color(ColorTarget::Stroke),
            Verb::OpenColorFill => self.toggle_color(ColorTarget::Fill),
            Verb::OpenColorBackground => self.toggle_color(ColorTarget::Background),
            Verb::OpenFontPicker => {
                let open = !self.font_picker_open;
                self.close_overlays();
                self.font_picker_open = open;
            }
            Verb::OpenPalette => {
                self.close_overlays();
                self.palette = Some(PaletteState::default());
            }
            Verb::OpenSearch => {
                self.close_overlays();
                self.search_open = true;
            }
            Verb::OpenShortcuts => {
                let open = !self.shortcuts_open;
                self.close_overlays();
                self.shortcuts_open = open;
            }
            Verb::OpenAbout => {
                let open = !self.about_open;
                self.close_overlays();
                self.about_open = open;
            }
            Verb::OpenCollab => {
                let open = !self.collab_open;
                self.close_overlays();
                self.collab_open = open;
            }
            Verb::ToggleStats => self.stats_open = !self.stats_open,
            Verb::ToggleZen => self.zen = !self.zen,
            Verb::OpenLinkEditor => {
                if self.link_editor.is_none() {
                    self.close_overlays();
                    self.link_editor = Some(LinkEditorState::default());
                }
            }
            Verb::OpenEquationEditor => {
                if self.equation.is_none() {
                    self.close_overlays();
                    self.equation = Some(EquationState::default());
                }
            }
        }
    }

    /// Opens the rename dialog for a tab, with its current name.
    pub fn open_rename(&mut self, target: RenameTarget, current: impl Into<String>) {
        self.close_overlays();
        self.rename = Some(RenameState::new(target, current));
    }

    /// The device is touch-first (the app knows from a media query or the platform). Touch events seen
    /// by the UI set it too.
    pub fn set_touch_first(&mut self, touch_first: bool) {
        self.touch_first = touch_first;
    }

    /// Looks at the input of the frame: a touch event means the device is used by touch.
    pub(crate) fn observe_input(&mut self, ctx: &egui::Context) {
        if !self.touch_first && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Touch { .. }))) {
            self.touch_first = true;
        }
    }

    /// True when the touch layout is in effect: chosen in the preferences, or automatic on a narrow window
    /// or after touch input.
    pub fn compact(&self, width: f32) -> bool {
        match self.layout {
            LayoutPreference::Touch => true,
            LayoutPreference::Desktop => false,
            LayoutPreference::Auto => self.touch_first || width < NARROW_WIDTH,
        }
    }

    /// True for a window so narrow that panels start collapsed.
    pub fn narrow(&self, width: f32) -> bool {
        self.compact(width) && width < NARROW_WIDTH
    }

    /// True when the panel is collapsed to its button. Only the touch layout collapses panels; there a
    /// panel starts collapsed on a narrow window and open on a wide one, until the user chooses.
    pub fn panel_collapsed(&self, panel: PanelKind, width: f32) -> bool {
        if !self.compact(width) {
            return false;
        }
        self.panel_collapsed[panel as usize].unwrap_or_else(|| self.narrow(width))
    }

    /// The user collapses or expands a panel of the touch layout.
    pub fn set_panel_collapsed(&mut self, panel: PanelKind, collapsed: bool) {
        self.panel_collapsed[panel as usize] = Some(collapsed);
    }

    fn toggle_color(&mut self, target: ColorTarget) {
        let next = if self.color_popover == Some(target) { None } else { Some(target) };
        self.close_overlays();
        self.color_popover = next;
    }

    /// Opens the equation editor with some LaTeX. The canvas layer calls this after `text.edit_begin`
    /// on a math element.
    pub fn open_equation(&mut self, latex: impl Into<String>) {
        self.close_overlays();
        self.equation = Some(EquationState { latex: latex.into(), focused: false });
    }

    /// Opens the context menu at a screen position. The canvas layer calls this on a right click and passes
    /// the world point under it, so that `Paste` can put its content there.
    pub fn open_context_menu(&mut self, screen: egui::Pos2, world: Option<[f64; 2]>) {
        self.close_overlays();
        self.menus_opened += 1;
        self.context_menu = Some(ContextMenu { screen, world, locked: None, opened: self.menus_opened });
    }

    /// Opens the context menu of a locked element, which cannot be selected: it offers to unlock it
    /// (`element.unlock_all` with this id). The canvas layer calls this for `ContextTarget::Locked`.
    pub fn open_context_menu_on_locked(
        &mut self,
        screen: egui::Pos2,
        world: Option<[f64; 2]>,
        element: impl Into<String>,
    ) {
        self.close_overlays();
        self.menus_opened += 1;
        self.context_menu =
            Some(ContextMenu { screen, world, locked: Some(element.into()), opened: self.menus_opened });
    }

    /// The app answers a [`HostRequest::PasteText`].
    pub fn set_pasted_text(&mut self, text: String) {
        self.pasted_text = Some(text);
    }

    /// Takes the pasted text, if the app delivered some.
    pub fn take_pasted_text(&mut self) -> Option<String> {
        self.pasted_text.take()
    }

    /// Adds a toast. Only the newest [`MAX_TOASTS`] are kept, and an identical toast that is still
    /// showing is not repeated.
    pub fn push_toast(&mut self, toast: Toast) {
        if let Some(existing) = self.toasts.iter_mut().find(|t| t.text == toast.text && t.code == toast.code) {
            existing.born = toast.born;
            return;
        }
        self.toasts.push(toast);
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    /// Drops the toasts that have faded out.
    pub fn expire_toasts(&mut self, now: f64) {
        self.toasts.retain(|t| t.alpha(now).is_some());
    }

    pub(crate) fn ask_confirm(&mut self, call: Call, message: String) {
        self.confirm = Some(Confirm { call, message });
    }

    /// The id of the running scrub gesture.
    pub fn scrub_gesture(&self) -> Option<&str> {
        self.scrub.as_deref()
    }

    pub(crate) fn set_scrub_gesture(&mut self, id: Option<String>) {
        self.scrub = id;
    }
}
