//! What the UI produces, and the one function that routes it.
//!
//! A frame of the UI returns a `Vec<UiAction>`: every action of the frame, in the order the user made
//! them. Nothing in the UI changes the session while it draws. The host then calls [`dispatch`], which
//! sends each action through [`route`] and executes it:
//!
//! ```text
//! widget click / key / menu item / palette row
//!        |  builds
//!        v
//!    UiAction ──route()──> Routed::Command(Call { id, args, origin }) ──> CommandSink::run (Session::execute_from)
//!                          Routed::Local(Verb)                         ──> UiState (open a dialog, close a menu)
//!                          (a verb that is a setting, `ui.toggle_minimap`, routes to its command instead)
//!                          Routed::Host(HostRequest)                   ──> returned to the app (file picker, clipboard)
//! ```
//!
//! [`route`] is the only place that decides which command an action runs.

use drafftink_engine::spec::{Host, HostEffect, RefusalToast};
use drafftink_engine::{Origin, Refusal, Registry, Session};
use serde_json::{Value, json};

use crate::scrub::ScrubControl;
use crate::state::{ConvertPopup, SearchResults, Toast, UiState};

/// One call of a registered command: what [`CommandSink::run`] receives.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The command id, such as `history.undo`.
    pub id: String,
    /// The arguments (`{}` for none).
    pub args: Value,
    /// Which surface asked. Recorded in the ring, never changes a result.
    pub origin: Origin,
}

impl Call {
    /// A call with an origin.
    pub fn new(origin: Origin, id: impl Into<String>, args: Value) -> Self {
        Self { id: id.into(), args, origin }
    }
}

/// Where a scrubbing control is in its gesture (docs/exceptions.md E13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubPhase {
    /// The first change of a drag: `gesture.begin`.
    Begin,
    /// Every further change: `gesture.update`.
    Update,
    /// The pointer was released: `gesture.end`.
    End,
    /// Esc during the drag: `gesture.cancel`.
    Cancel,
}

/// One step of a scrubbed control (the opacity slider). The final state is a plain `command` call with
/// the same `args`.
#[derive(Debug, Clone, PartialEq)]
pub struct Scrub {
    /// Where in the drag.
    pub phase: ScrubPhase,
    /// The control that scrubs.
    pub control: ScrubControl,
    /// The command the gesture ends with, such as `style.set` (that of the control).
    pub command: &'static str,
    /// The full arguments of this frame (absolute, never an increment).
    pub args: Value,
}

/// Actions that only change what the UI shows. They are not commands and never reach agents
/// (docs/commands.md §6.3). The ones that produce a result end by building a command call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    /// `ui.close_overlays`: closes menu, popovers, dialogs, the palette.
    CloseOverlays,
    /// `ui.toggle_menu`: the hamburger menu.
    ToggleMenu,
    /// `ui.open_file`: file picker (the host's), ends with `doc.open`.
    OpenFile,
    /// `ui.open_recent`: the list of recent documents, ends with `doc.open`.
    OpenRecent,
    /// `ui.save_as`: a name field, then the save picker; ends with `doc.rename` and `doc.save` with `to`.
    SaveAs,
    /// `ui.insert_image`: image picker, ends with `image.insert`.
    InsertImage,
    /// `ui.open_color_picker`: colour popover for the stroke.
    OpenColorStroke,
    /// `ui.open_color_picker`: colour popover for the fill.
    OpenColorFill,
    /// `ui.open_color_picker`: colour popover for the page background.
    OpenColorBackground,
    /// `ui.pick_color`: the eyedropper for the stroke colour.
    PickColorStroke,
    /// `ui.pick_color`: the eyedropper for the fill colour.
    PickColorFill,
    /// `ui.open_font_picker`: font family popover.
    OpenFontPicker,
    /// `ui.open_palette`: the command palette.
    OpenPalette,
    /// `ui.open_search`: search box over `element.query`.
    OpenSearch,
    /// `ui.open_shortcuts`: the shortcuts dialog.
    OpenShortcuts,
    /// `ui.open_collab`: the collaboration dialog.
    OpenCollab,
    /// `ui.toggle_stats`: the stats panel.
    ToggleStats,
    /// `ui.toggle_zen`: hides the chrome.
    ToggleZen,
    /// `ui.toggle_debug`: the debug panel (frame rate, GPU, scene).
    ToggleDebug,
    /// `ui.toggle_minimap`: the minimap. The choice is the setting `view.minimap`, so it is kept between runs and the
    /// verb ends with `settings.toggle` (see [`Verb::setting_toggle`]).
    ToggleMinimap,
    /// `ui.open_equation_editor`: the LaTeX dialog.
    OpenEquationEditor,
    /// `ui.open_link_editor`: the link field for the selected element, ends with `element.set_link`.
    OpenLinkEditor,
    /// `ui.open_export`: the export dialog, ends with `doc.export` or `clipboard.copy_as`.
    OpenExport,
    /// `ui.open_preferences`: the preferences dialog, ends with `settings.set` and `settings.toggle`.
    OpenPreferences,
    /// `ui.toggle_library`: the library panel, ends with `library.*`.
    ToggleLibrary,
    /// `ui.rename_document`: the rename field for the document, ends with `doc.rename`.
    RenameDocument,
    /// `ui.open_about`: the About dialog: version, licence, and the offer of the source code.
    OpenAbout,
}

impl Verb {
    /// Every verb, in the order of docs/commands.md §6.3.
    pub const ALL: [Verb; 27] = [
        Verb::CloseOverlays,
        Verb::ToggleMenu,
        Verb::OpenFile,
        Verb::OpenRecent,
        Verb::SaveAs,
        Verb::InsertImage,
        Verb::OpenColorStroke,
        Verb::OpenColorFill,
        Verb::OpenColorBackground,
        Verb::PickColorStroke,
        Verb::PickColorFill,
        Verb::OpenFontPicker,
        Verb::OpenPalette,
        Verb::OpenSearch,
        Verb::OpenShortcuts,
        Verb::OpenCollab,
        Verb::ToggleStats,
        Verb::ToggleZen,
        Verb::ToggleDebug,
        Verb::ToggleMinimap,
        Verb::OpenEquationEditor,
        Verb::OpenLinkEditor,
        Verb::OpenExport,
        Verb::OpenPreferences,
        Verb::ToggleLibrary,
        Verb::RenameDocument,
        Verb::OpenAbout,
    ];

    /// The id of the verb in the catalog (§6.3). The three colour targets share `ui.open_color_picker`.
    pub fn id(self) -> &'static str {
        match self {
            Verb::CloseOverlays => "ui.close_overlays",
            Verb::ToggleMenu => "ui.toggle_menu",
            Verb::OpenFile => "ui.open_file",
            Verb::OpenRecent => "ui.open_recent",
            Verb::SaveAs => "ui.save_as",
            Verb::InsertImage => "ui.insert_image",
            Verb::OpenColorStroke | Verb::OpenColorFill | Verb::OpenColorBackground => "ui.open_color_picker",
            Verb::PickColorStroke | Verb::PickColorFill => "ui.pick_color",
            Verb::OpenFontPicker => "ui.open_font_picker",
            Verb::OpenPalette => "ui.open_palette",
            Verb::OpenSearch => "ui.open_search",
            Verb::OpenShortcuts => "ui.open_shortcuts",
            Verb::OpenCollab => "ui.open_collab",
            Verb::ToggleStats => "ui.toggle_stats",
            Verb::ToggleZen => "ui.toggle_zen",
            Verb::ToggleDebug => "ui.toggle_debug",
            Verb::ToggleMinimap => "ui.toggle_minimap",
            Verb::OpenEquationEditor => "ui.open_equation_editor",
            Verb::OpenLinkEditor => "ui.open_link_editor",
            Verb::OpenExport => "ui.open_export",
            Verb::OpenPreferences => "ui.open_preferences",
            Verb::ToggleLibrary => "ui.toggle_library",
            Verb::RenameDocument => "ui.rename_document",
            Verb::OpenAbout => "ui.open_about",
        }
    }

    /// A short title for the palette and the shortcuts dialog.
    pub fn title(self) -> &'static str {
        match self {
            Verb::CloseOverlays => "Close dialogs and menus",
            Verb::ToggleMenu => "Open or close the main menu",
            Verb::OpenFile => "Open a file",
            Verb::OpenRecent => "Open a recent document",
            Verb::SaveAs => "Save as",
            Verb::InsertImage => "Insert an image",
            Verb::OpenColorStroke => "Pick the stroke colour",
            Verb::OpenColorFill => "Pick the fill colour",
            Verb::OpenColorBackground => "Pick the background colour",
            Verb::PickColorStroke => "Pick the stroke colour from the canvas",
            Verb::PickColorFill => "Pick the fill colour from the canvas",
            Verb::OpenFontPicker => "Pick the font",
            Verb::OpenPalette => "Command palette",
            Verb::OpenSearch => "Search the canvas",
            Verb::OpenShortcuts => "Keyboard shortcuts",
            Verb::OpenCollab => "Collaboration",
            Verb::ToggleStats => "Show or hide the stats panel",
            Verb::ToggleZen => "Zen mode (hide the interface)",
            Verb::ToggleDebug => "Show or hide the debug panel",
            Verb::ToggleMinimap => "Show or hide the minimap",
            Verb::OpenEquationEditor => "Edit the equation",
            Verb::OpenLinkEditor => "Edit the link",
            Verb::OpenExport => "Export…",
            Verb::OpenPreferences => "Preferences",
            Verb::ToggleLibrary => "Show or hide the library",
            Verb::RenameDocument => "Rename the document",
            Verb::OpenAbout => "About Drafft.ink",
        }
    }

    /// The setting this verb flips, for a verb whose whole effect is a setting: the UI keeps no state of its own for
    /// it, [`route`] sends it to `settings.toggle`, and the choice is stored with the other settings.
    pub fn setting_toggle(self) -> Option<&'static str> {
        match self {
            Verb::ToggleMinimap => Some("view.minimap"),
            _ => None,
        }
    }
}

/// Things only the app can do, because they need the operating system. The UI never does them itself:
/// it builds a request, the app does the work and then calls a command with the result.
#[derive(Debug, Clone, PartialEq)]
pub enum HostRequest {
    /// Show a file picker. The app calls `doc.open` with the chosen file.
    PickFileToOpen,
    /// Show the list of stored or recent documents. The app calls `doc.open` with the chosen one.
    PickRecent,
    /// Show a save picker. The app calls `doc.save` with `to` set.
    PickSaveTarget,
    /// Show an image picker. The app calls `image.insert`.
    PickImage,
    /// Show a picker for an Excalidraw library. The app calls `library.import` with `mode: open_as_tab`.
    PickLibrary,
    /// Copy text to the system clipboard (the equation editor's Copy button).
    CopyText(String),
    /// Ask for the clipboard text. The app answers by calling [`UiState::set_pasted_text`].
    PasteText,
    /// A style change made while a text edit is running. The text editor applies it to the selected
    /// range or to the element; `style.set` would refuse with `text_editing` (docs/exceptions.md E16).
    TextEditStyle(Value),
    /// Bold or italic chosen while a text edit runs: the arguments of `text.format`. The text editor applies them
    /// to its selection, or to what is typed next when nothing is selected.
    TextEditFormat(Value),
    /// Open a link in the browser.
    OpenUrl(String),
    /// Ask the platform to pick a colour from anywhere on the screen (the browser's `EyeDropper`). The app answers
    /// with `style.set` for `target`.
    PickScreenColor(crate::state::ColorTarget),
    /// Show a picker for an Excalidraw library file. The app calls `library.import` with `mode: merge`
    /// (the library panel's Import button; `PickLibrary` opens a library as a tab instead).
    PickLibraryImport,
}

/// One thing the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    /// Run a command.
    Command(Call),
    /// One step of a scrubbed control.
    Scrub(Scrub),
    /// Change what the UI shows.
    Verb(Verb),
    /// Ask the app for something that needs the OS.
    Host(HostRequest),
}

impl UiAction {
    /// A command call with the origin `ui`.
    pub fn run(id: impl Into<String>, args: Value) -> Self {
        UiAction::Command(Call::new(Origin::Ui, id, args))
    }

    /// A command call with a given origin.
    pub fn run_from(origin: Origin, id: impl Into<String>, args: Value) -> Self {
        UiAction::Command(Call::new(origin, id, args))
    }

    /// `tool.set` for a tool.
    pub fn set_tool(origin: Origin, tool: drafftink_engine::state::Tool) -> Self {
        Self::run_from(origin, "tool.set", json!({ "tool": tool.name() }))
    }

    /// `style.set` with a props object.
    pub fn style(props: Value) -> Self {
        Self::run("style.set", json!({ "props": props }))
    }

    /// The call, when this action is a plain command.
    pub fn as_call(&self) -> Option<&Call> {
        match self {
            UiAction::Command(c) => Some(c),
            _ => None,
        }
    }
}

/// Where an action goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Routed {
    /// Execute this call.
    Command(Call),
    /// Handled by the UI's own state.
    Local(Verb),
    /// Handed to the app.
    Host(HostRequest),
}

/// The one routing function: an action to the command it runs, the verb it applies, or the request it
/// hands to the app. A scrub step becomes a call of the `gesture.*` commands (the gesture id comes from
/// the state, which remembers the running scrub).
pub fn route(action: &UiAction, state: &UiState) -> Routed {
    match action {
        UiAction::Command(c) => Routed::Command(c.clone()),
        UiAction::Verb(v) => match v.setting_toggle() {
            Some(key) => Routed::Command(Call::new(Origin::Ui, "settings.toggle", json!({ "key": key }))),
            None => Routed::Local(*v),
        },
        UiAction::Host(h) => Routed::Host(h.clone()),
        UiAction::Scrub(s) => {
            let gesture = state.scrub_gesture().map(str::to_owned);
            let (id, args) = match s.phase {
                ScrubPhase::Begin => ("gesture.begin", json!({ "command": s.command, "args": s.args })),
                ScrubPhase::Update => ("gesture.update", json!({ "gesture": gesture, "args": s.args })),
                ScrubPhase::End => ("gesture.end", json!({ "gesture": gesture, "args": s.args })),
                ScrubPhase::Cancel => ("gesture.cancel", json!({ "gesture": gesture })),
            };
            Routed::Command(Call::new(Origin::Ui, id, args))
        }
    }
}

/// Anything that can run a command. The session is one; a test uses a recorder.
pub trait CommandSink {
    /// Runs a command and returns its output or its refusal.
    fn run(&mut self, origin: Origin, id: &str, args: Value) -> Result<Value, Refusal>;
}

impl CommandSink for Session {
    fn run(&mut self, origin: Origin, id: &str, args: Value) -> Result<Value, Refusal> {
        self.execute_from(origin, id, args)
    }
}

/// A command that was ran, with what it returned.
#[derive(Debug, Clone)]
pub struct Ran {
    /// The call.
    pub call: Call,
    /// The output, or the refusal.
    pub result: Result<Value, Refusal>,
}

/// What [`dispatch`] did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Every command that ran, in order.
    pub ran: Vec<Ran>,
    /// Requests for the app.
    pub host: Vec<HostRequest>,
}

impl Report {
    /// The refusals among the ran commands.
    pub fn refusals(&self) -> impl Iterator<Item = &Refusal> {
        self.ran.iter().filter_map(|e| e.result.as_ref().err())
    }
}

/// Codes that a shell shows without a toast when a shortcut caused them (docs/commands.md §1.5).
pub fn quiet_from_keyboard(code: &str) -> bool {
    drafftink_engine::refusal::QUIET_FROM_KEYBOARD.contains(&code)
}

/// Executes every action of a frame, in order. A refusal becomes a toast with its text unchanged, plus
/// a button for each suggestion. Three refusal codes open a UI flow instead of only a toast:
/// `unsaved_changes` asks before discarding, `no_save_target` opens the save picker.
pub fn dispatch<S: CommandSink>(sink: &mut S, state: &mut UiState, actions: Vec<UiAction>, now: f64) -> Report {
    let mut report = Report::default();
    for action in &actions {
        if let UiAction::Scrub(s) = action {
            state.scrub.routed(s);
        }
        match route(action, state) {
            Routed::Local(v) => state.apply_verb(v, &mut report.host),
            Routed::Host(h) => report.host.push(h),
            Routed::Command(call) => {
                let result = sink.run(call.origin, &call.id, call.args.clone());
                state.after_command(&call, &result, now, &mut report.host);
                report.ran.push(Ran { call, result });
            }
        }
    }
    report
}

impl UiState {
    /// Bookkeeping after a command ran: the running scrub, toasts, the confirm box, the save picker.
    pub(crate) fn after_command(
        &mut self,
        call: &Call,
        result: &Result<Value, Refusal>,
        now: f64,
        host: &mut Vec<HostRequest>,
    ) {
        // What the command asks of a host is a fact of its registration (`CommandSpec::host`), not a list of ids here.
        let facts = Registry::global().find(&call.id).map_or(Host::NONE, |spec| spec.host);
        match (facts.effect, result) {
            // The equation editor ends with this command, whatever it answered.
            (HostEffect::EndsEquationEdit, _) => self.equation = None,
            // A rename dialog closes when the name was taken; a refusal (empty name) keeps it open next to its toast.
            (HostEffect::CompletesRename | HostEffect::CompletesRenameThenSaveAs, Ok(_)) => {
                self.rename = None;
                // Save As renames first and asks for the place afterwards.
                if self.save_as.take().is_some() && facts.effect == HostEffect::CompletesRenameThenSaveAs {
                    host.push(HostRequest::PickSaveTarget);
                }
            }
            // A write the port finishes later is not done yet: the dialog stays, so that the person can see that
            // nothing is confirmed and try again. A refused write never gets here; it is a refusal with the reason.
            (HostEffect::CompletesExport, Ok(out)) if out.get("pending_write").is_some() => {
                self.push_toast(Toast::note("The export has started, but the file is not confirmed yet.", now));
            }
            (HostEffect::CompletesExport, Ok(_)) => self.export = None,
            (HostEffect::CompletesLibraryAdd, Ok(_)) => self.library_name.clear(),
            // Tab or Shift+Tab converted the selection: show the popup with the type it became. Once the popup is
            // open, a click on one of its types updates it too.
            (HostEffect::ShowsConvertPopup, Ok(out))
                if call.origin == Origin::Shortcut || self.convert_popup.is_some() =>
            {
                let to = out.get("to").and_then(Value::as_str).map(str::to_owned);
                self.convert_popup = Some(ConvertPopup { current: to });
            }
            // The link dialog closes when the link was set; a refusal keeps it open next to its toast.
            (HostEffect::CompletesLinkEdit, Ok(_)) => self.link_editor = None,
            // The engine never opens a browser: an address comes back and goes to the app.
            (HostEffect::ReturnsUrlToOpen, Ok(out)) => {
                if let Some(url) = out.get("url").and_then(Value::as_str) {
                    host.push(HostRequest::OpenUrl(url.to_owned()));
                }
            }
            (HostEffect::ShowsSearchResults, Ok(out)) if self.search_open => {
                let query = call.args.pointer("/filter/text").and_then(Value::as_str).unwrap_or_default();
                self.search_results = Some(SearchResults::from_output(query, out));
            }
            (HostEffect::BeginsGesture, Ok(out)) => {
                self.scrub.began(out.get("gesture").and_then(Value::as_str).map(str::to_owned));
            }
            (HostEffect::BeginsGesture, Err(_)) => self.scrub.began(None),
            (HostEffect::EndsGestureOnRefusal, Err(_)) => self.scrub.lost(),
            (HostEffect::EndsGesture, _) => self.scrub.ended(),
            _ => {}
        }
        let waited = self.waiting.iter().position(|c| c == call);
        let Err(refusal) = result else {
            self.waiting.retain(|c| c != call);
            return;
        };
        if refusal.code == "render_pending" {
            // Not an error: the picture is being drawn. The same call runs again in the next frame (the port wakes
            // the app when it is done), and the dialog that asked stays as it is meanwhile.
            if waited.is_none() {
                self.waiting.push(call.clone());
                self.push_toast(Toast::note("Drawing the picture\u{2026}", now));
            }
            return;
        }
        if let Some(i) = waited {
            self.waiting.remove(i);
        }
        if refusal.code == "no_save_target" {
            host.push(HostRequest::PickSaveTarget);
            return;
        }
        if refusal.code == "unsaved_changes" && call.args.get("on_unsaved").is_none() {
            self.ask_confirm(call.clone(), refusal.message.clone());
            return;
        }
        // An export says why it did nothing, also from the keyboard: Mod+E on an empty drawing is not a stray key.
        if call.origin == Origin::Shortcut
            && quiet_from_keyboard(refusal.code)
            && facts.effect != HostEffect::CompletesExport
        {
            return;
        }
        if facts.refusal == RefusalToast::Silent && refusal.code == "no_gesture" {
            // A scrub that lost its gesture (the engine ended it, or it was cancelled) is not worth a toast: the
            // control just stops scrubbing. Every other refusal of such a command (a locked element, say) is shown.
            return;
        }
        self.push_toast(Toast::from_refusal(refusal, now).with_args(&call.args));
    }
}

#[cfg(test)]
#[path = "host_facts_tests.rs"]
mod host_facts_tests;
