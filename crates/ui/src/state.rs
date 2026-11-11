//! State that exists only in the UI: which dialog is open, the palette query, the toasts.
//!
//! None of it is document, selection, tool or setting state, so none of it belongs to the `Session`
//! (docs/commands.md §1.1, "UI state is not in the Session"). Everything the UI shows about the
//! document comes from the [`crate::Model`]. Text-field buffers that belong to one dialog live in
//! egui's own memory, keyed by the dialog.

use drafftink_engine::Refusal;

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

/// Everything the UI remembers between frames.
#[derive(Debug, Default)]
pub struct UiState {
    /// The hamburger menu is open.
    pub menu_open: bool,
    /// The palette, when open.
    pub palette: Option<PaletteState>,
    /// The shortcuts dialog is open.
    pub shortcuts_open: bool,
    /// The collaboration dialog is open.
    pub collab_open: bool,
    /// The colour popover, when open.
    pub color_popover: Option<ColorTarget>,
    /// The font family popover is open.
    pub font_picker_open: bool,
    /// The search box is open.
    pub search_open: bool,
    /// The stats panel is shown.
    pub stats_open: bool,
    /// The chrome is hidden.
    pub zen: bool,
    /// The equation editor, when open.
    pub equation: Option<EquationState>,
    /// The context menu, when open: where it was asked for, in screen points.
    pub context_menu: Option<egui::Pos2>,
    /// The recent documents list is open.
    pub recent_open: bool,
    /// A pending question.
    pub confirm: Option<Confirm>,
    /// Toasts, oldest first.
    pub toasts: Vec<Toast>,
    /// Text the app pasted for the equation editor.
    pasted_text: Option<String>,
    /// The id of the gesture a scrubbed control began.
    scrub: Option<String>,
    /// The theme setting the visuals were last built for.
    pub(crate) applied_theme: Option<String>,
}

impl UiState {
    /// A fresh state: nothing open.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when something that Esc should close is open.
    pub fn any_overlay_open(&self) -> bool {
        self.menu_open
            || self.palette.is_some()
            || self.shortcuts_open
            || self.collab_open
            || self.color_popover.is_some()
            || self.font_picker_open
            || self.search_open
            || self.equation.is_some()
            || self.context_menu.is_some()
            || self.recent_open
            || self.confirm.is_some()
    }

    /// True when a modal dialog is open and keys must not reach the canvas.
    pub fn modal_open(&self) -> bool {
        self.palette.is_some()
            || self.shortcuts_open
            || self.collab_open
            || self.equation.is_some()
            || self.confirm.is_some()
            || self.search_open
    }

    /// Closes everything that Esc closes. Returns true when something was open.
    pub fn close_overlays(&mut self) -> bool {
        let was = self.any_overlay_open();
        self.menu_open = false;
        self.palette = None;
        self.shortcuts_open = false;
        self.collab_open = false;
        self.color_popover = None;
        self.font_picker_open = false;
        self.search_open = false;
        self.equation = None;
        self.context_menu = None;
        self.recent_open = false;
        self.confirm = None;
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
            Verb::SaveAs => host.push(HostRequest::PickSaveTarget),
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
            Verb::OpenCollab => {
                let open = !self.collab_open;
                self.close_overlays();
                self.collab_open = open;
            }
            Verb::ToggleStats => self.stats_open = !self.stats_open,
            Verb::ToggleZen => self.zen = !self.zen,
            Verb::OpenEquationEditor => {
                if self.equation.is_none() {
                    self.close_overlays();
                    self.equation = Some(EquationState::default());
                }
            }
        }
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

    /// Opens the context menu at a screen position. The canvas layer calls this on a right click.
    pub fn open_context_menu(&mut self, at: egui::Pos2) {
        self.close_overlays();
        self.context_menu = Some(at);
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
