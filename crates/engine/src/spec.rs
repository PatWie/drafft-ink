//! `CommandSpec`: the static description of one command (docs/commands.md §1.2).
//!
//! A family declares its commands as a `pub const COMMANDS: &[CommandSpec]` in `cmd/<family>.rs`. Every
//! fact is a required field of [`Facts`], so a base value that omits one does not compile, and a registration that
//! inherits a fact from its base (`..BASE`) is checked against `docs/commands.md` Appendix A for every field. The handler
//! takes a typed argument struct, never a `Value`: [`Typed`] deserialises, validates and serialises.

use std::any::Any;
use std::marker::PhantomData;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::args;
use crate::ports::PortKind;
use crate::refusal::Refusal;
use crate::session::Session;

/// A JSON Schema document.
pub type Schema = schemars::Schema;

/// How a command takes part in undo history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Undo {
    /// One undo step when the command changed the document; nothing when it changed nothing.
    Undoable,
    /// Never touches history: view, tool, settings, reads, undo itself, document replacement.
    NoUndo,
    /// Can run as `gesture.begin/update/end` and is one `Undoable` step at the end. Called plainly it
    /// behaves as `Undoable`.
    Gesture,
}

impl Undo {
    /// The name used in the catalog and in `engine.commands`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Undoable => "Undoable",
            Self::NoUndo => "NoUndo",
            Self::Gesture => "Gesture",
        }
    }
}

/// Merge rapid repeats of a command on the same targets into one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coalesce {
    /// Steps with the same key may merge. Usually the command id.
    pub key: &'static str,
    /// Longest gap between repeats, in milliseconds.
    pub ms: u64,
}

macro_rules! bitset {
    ($(#[$m:meta])* $name:ident { $( $(#[$cm:meta])* $c:ident = $bit:expr => ($letter:literal, $text:literal) ),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $name(pub u16);

        impl $name {
            $( $(#[$cm])* pub const $c: $name = $name($bit); )+
            /// The empty set.
            pub const NONE: $name = $name(0);
            /// Every member with its letter and name.
            pub const MEMBERS: &'static [($name, &'static str, &'static str)] = &[ $( ($name::$c, $letter, $text) ),+ ];

            /// True when no member is set.
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// True when every member of `other` is set.
            pub const fn contains(self, other: $name) -> bool {
                self.0 & other.0 == other.0 && other.0 != 0
            }

            /// The union.
            #[must_use]
            pub const fn with(self, other: $name) -> $name {
                $name(self.0 | other.0)
            }

            /// Members as letters, for example `"D,O"`; `"-"` when empty.
            pub fn letters(self) -> String {
                let v: Vec<&str> = Self::MEMBERS.iter().filter(|(m, _, _)| self.contains(*m)).map(|(_, l, _)| *l).collect();
                if v.is_empty() { "-".to_owned() } else { v.join(",") }
            }

            /// Members by name, for JSON.
            pub fn names(self) -> Vec<&'static str> {
                Self::MEMBERS.iter().filter(|(m, _, _)| self.contains(*m)).map(|(_, _, n)| *n).collect()
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.letters())
            }
        }
    };
}

bitset! {
    /// The state classes a command may change (docs/commands.md §1.1).
    Mutates {
        /// `D`: the document.
        DOC = 1 => ("D", "doc"),
        /// `O`: observed editor state (selection, editing group, point editor).
        OBSERVED = 2 => ("O", "observed"),
        /// `V`: the camera.
        VIEW = 4 => ("V", "view"),
        /// `T`: the tool and style defaults.
        TOOL = 8 => ("T", "tool"),
        /// `B`: tabs and per-tab document metadata.
        TABS = 16 => ("B", "tabs"),
        /// `S`: settings.
        SETTINGS = 32 => ("S", "settings"),
        /// `C`: the collab session.
        COLLAB = 64 => ("C", "collab"),
        /// `P`: ports and session buffers.
        PORTS = 128 => ("P", "ports"),
        /// `E`: a gesture or a text-edit session.
        EDIT_SESSION = 256 => ("E", "edit_session"),
    }
}

bitset! {
    /// The ports a command may need.
    PortSet {
        /// The clipboard.
        CLIPBOARD = 1 => ("Clipboard", "clipboard"),
        /// Persistent storage.
        STORAGE = 2 => ("Storage", "storage"),
        /// File access.
        FILES = 4 => ("Files", "files"),
        /// Offscreen rendering.
        RENDERER = 8 => ("Renderer", "renderer"),
        /// The network.
        NET = 16 => ("Net", "net"),
        /// The clock.
        CLOCK = 32 => ("Clock", "clock"),
    }
}

impl PortSet {
    /// The set containing one port kind.
    pub const fn of(kind: PortKind) -> PortSet {
        match kind {
            PortKind::Clipboard => Self::CLIPBOARD,
            PortKind::Storage => Self::STORAGE,
            PortKind::Files => Self::FILES,
            PortKind::Renderer => Self::RENDERER,
            PortKind::Net => Self::NET,
            PortKind::Clock => Self::CLOCK,
        }
    }
}

/// Whether the command uses the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// Not used.
    No,
    /// Used only as a default; an empty selection is not an error.
    Soft,
    /// The command has an `ids` argument defaulting to the selection; an empty list refuses `no_target`.
    Required,
}

/// Where a command may appear in a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Batch {
    /// Anywhere.
    Allowed,
    /// Only as the final step.
    Last,
    /// Not at all.
    Never,
}

/// A yes/no gate for a session mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// The command runs in this mode.
    Allowed,
    /// The command refuses in this mode.
    Refused,
}

/// What a running text edit does to a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextGate {
    /// The command runs during a text edit.
    Allowed,
    /// The command refuses with `text_editing`.
    Refused,
    /// The engine commits the text edit first, then runs the command.
    Commit,
}

/// The dirty reason a successful command sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Redraw {
    /// Nothing to redraw.
    None,
    /// Handles, guides and cursors only.
    Overlay,
    /// Re-composite cached fragments with a new transform.
    Camera,
    /// Re-encode changed element fragments.
    Scene,
}

/// Whether the ring records successes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RingMode {
    /// Every call.
    Always,
    /// Only a refusal (high-rate commands).
    Quiet,
}

/// How a command that changes the document relates to undo and to view-only mode (docs/commands.md §1.2, test 2).
///
/// Every registration declares it. The registry tests derive the rules from it instead of keeping lists of ids:
/// a command whose `mutates` has `DOC` is `Edit`, `Replaces` or `Managed`, and any other command is `NotDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocWrite {
    /// The command does not change the document.
    NotDocument,
    /// An ordinary edit: it has an undo class other than `NoUndo` and refuses in view-only mode.
    Edit,
    /// It replaces the document wholesale (`doc.new`, `doc.open`, `doc.show_intro`, `collab.join_room`): `NoUndo`
    /// because the old document is gone, and allowed in view-only mode because it is not an edit of the open one.
    Replaces,
    /// It manages the document state itself (`history.undo`, `history.redo`, the `gesture` commands, `tool.cancel`):
    /// allowed in view-only mode, and its undo class is its own business (it restores or streams steps).
    Managed,
}

impl DocWrite {
    /// The word in `docs/commands.md` Appendix A2.
    pub const fn name(self) -> &'static str {
        match self {
            Self::NotDocument => "not_document",
            Self::Edit => "edit",
            Self::Replaces => "replaces",
            Self::Managed => "managed",
        }
    }
}

/// What a host (the UI shell) does after the command ran, besides showing the result. A host reads this fact
/// from the registration; it never matches on a command id. `docs/commands.md` Appendix A2 lists every command
/// that has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostEffect {
    /// Nothing special.
    None,
    /// Success closes the rename dialog that asked for it (`doc.rename`, `tab.rename`, `library.rename`, `frame.rename`).
    CompletesRename,
    /// Like `CompletesRename`, and a pending Save As goes on to ask for the place (`doc.rename`).
    CompletesRenameThenSaveAs,
    /// Success closes the export dialog.
    CompletesExport,
    /// Success clears the name field of the library panel.
    CompletesLibraryAdd,
    /// Success after a keyboard conversion, or inside the open popup, shows the popup with the type it became.
    ShowsConvertPopup,
    /// Success closes the link editor.
    CompletesLinkEdit,
    /// Success returns an address in `url` for the host to open (the engine never opens a browser).
    ReturnsUrlToOpen,
    /// Success fills the search panel while it is open.
    ShowsSearchResults,
    /// Success starts a scrub gesture of the host.
    BeginsGesture,
    /// A refusal ends the host's scrub.
    EndsGestureOnRefusal,
    /// Any answer ends the host's scrub.
    EndsGesture,
    /// Any answer closes the equation editor, which ends with this command.
    EndsEquationEdit,
}

impl HostEffect {
    /// The word in `docs/commands.md` Appendix A2.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "-",
            Self::CompletesRename => "completes_rename",
            Self::CompletesRenameThenSaveAs => "completes_rename_then_save_as",
            Self::CompletesExport => "completes_export",
            Self::CompletesLibraryAdd => "completes_library_add",
            Self::ShowsConvertPopup => "shows_convert_popup",
            Self::CompletesLinkEdit => "completes_link_edit",
            Self::ReturnsUrlToOpen => "returns_url_to_open",
            Self::ShowsSearchResults => "shows_search_results",
            Self::BeginsGesture => "begins_gesture",
            Self::EndsGestureOnRefusal => "ends_gesture_on_refusal",
            Self::EndsGesture => "ends_gesture",
            Self::EndsEquationEdit => "ends_equation_edit",
        }
    }
}

/// Whether a host shows a refusal of the command as a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalToast {
    /// Show it.
    Show,
    /// Do not, when the refusal is `no_gesture`: the command is a follow-up of a control that handles the loss
    /// itself (a scrub that lost its gesture just stops scrubbing). Any other refusal of it is shown.
    Silent,
}

/// What a held key does with the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyRepeat {
    /// Every auto-repeat event runs the command again (`element.nudge`, zoom keys).
    Repeats,
    /// Only the first press runs it; auto-repeat events are ignored (`tool.hold`).
    FirstPressOnly,
}

/// The facts a host needs about a command, apart from the ones the engine itself enforces. Every field has a
/// neutral value in [`Host::NONE`]; a registration states the ones that differ with [`CommandSpec::host`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    /// What the host does after a success or a refusal.
    pub effect: HostEffect,
    /// Whether the host toasts a refusal.
    pub refusal: RefusalToast,
    /// What a held key does.
    pub repeat: KeyRepeat,
    /// During a text edit the host sends a style choice to the text editor (which applies it to the selected
    /// range or to the element and hands it to `text.edit_end`) instead of running this command, which
    /// refuses with `text_editing`.
    pub text_editor_takes_style: bool,
}

impl Host {
    /// The neutral value: nothing special.
    pub const NONE: Host = Host {
        effect: HostEffect::None,
        refusal: RefusalToast::Show,
        repeat: KeyRepeat::Repeats,
        text_editor_takes_style: false,
    };

    /// Only the effect differs from [`Host::NONE`].
    pub const fn effect(effect: HostEffect) -> Host {
        Host { effect, ..Self::NONE }
    }

    /// Only the key repeat differs: auto-repeat events do not run the command again.
    pub const fn first_press_only() -> Host {
        Host { repeat: KeyRepeat::FirstPressOnly, ..Self::NONE }
    }

    /// Only the style hand-over differs: see [`Host::text_editor_takes_style`].
    pub const fn text_editor_takes_style() -> Host {
        Host { text_editor_takes_style: true, ..Self::NONE }
    }

    /// The host does not toast a refusal.
    #[must_use]
    pub const fn silent_refusals(mut self) -> Host {
        self.refusal = RefusalToast::Silent;
        self
    }

    /// True for the neutral value.
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }
}

/// Where a shortcut applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Focus on the canvas, no text edit.
    Canvas,
    /// Inside a text edit.
    Text,
    /// Everywhere.
    Always,
    /// On the canvas while the line/arrow point editor has a selected point. The shell checks
    /// this scope before `Canvas`, so the same key can mean "delete point" here and
    /// "delete element" on the canvas.
    PointEditor,
}

/// A default key binding with fixed arguments. `Mod` is Cmd on macOS and Ctrl elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    /// Chords such as `"Mod+Shift+Z"`, `"Esc"`, `"Space"`. All of them trigger the command.
    pub keys: &'static [&'static str],
    /// Chords that replace `keys` on macOS when they differ. Empty: the same as `keys`.
    pub keys_mac: &'static [&'static str],
    /// The fixed arguments as JSON text (`""` for none). Text because `Value` is not const.
    pub args: &'static str,
    /// Where it applies.
    pub when: Scope,
    /// Palette entry for this binding, if it has its own.
    pub title: Option<&'static str>,
    /// The platform also delivers this chord as a clipboard event (copy, cut, paste), and the host runs the
    /// command from that event because only the event carries the system clipboard. A host that does so must
    /// not run the binding as well, or one key press runs the command twice.
    pub host_clipboard: bool,
}

impl Binding {
    /// A binding with the same keys on every platform.
    pub const fn new(keys: &'static [&'static str], when: Scope) -> Self {
        Self { keys, keys_mac: &[], args: "", when, title: None, host_clipboard: false }
    }

    /// Fixes some arguments (JSON text).
    #[must_use]
    pub const fn with_args(mut self, args: &'static str) -> Self {
        self.args = args;
        self
    }

    /// Gives the binding its own palette title.
    #[must_use]
    pub const fn titled(mut self, title: &'static str) -> Self {
        self.title = Some(title);
        self
    }

    /// Marks the chord as one the platform turns into a clipboard event (see [`Binding::host_clipboard`]).
    #[must_use]
    pub const fn host_clipboard(mut self) -> Self {
        self.host_clipboard = true;
        self
    }

    /// Different keys on macOS.
    #[must_use]
    pub const fn mac(mut self, keys: &'static [&'static str]) -> Self {
        self.keys_mac = keys;
        self
    }

    /// The chords for a platform.
    pub fn chords(&self, mac: bool) -> &'static [&'static str] {
        if mac && !self.keys_mac.is_empty() { self.keys_mac } else { self.keys }
    }
}

/// Registered but not built yet, or ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Works.
    Stable,
    /// Registered so that the catalog, palette and tests see it; refuses with `not_supported`.
    Planned(&'static str),
}

/// How the executor runs the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Txn {
    /// Steps 5 to 8 of docs/commands.md §1.4 around the handler: rollback on failure, invariants,
    /// one undo step, port flush.
    Standard,
    /// The handler manages document state itself (gesture protocol, history). The executor still does
    /// lookup, gates, availability, argument checks, panic catching and the ring.
    Own,
}

/// The facts every registration must give. A struct literal with no `..Default` is required, so
/// a missing fact is a compile error. Families share a base value and override with `..BASE`.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    /// History class.
    pub undo: Undo,
    /// True: never changes any state class.
    pub reads_only: bool,
    /// State classes it changes.
    pub mutates: Mutates,
    /// Selection use.
    pub needs_selection: Selection,
    /// Ports it may need.
    pub ports: PortSet,
    /// Batch rule.
    pub batch: Batch,
    /// Allowed while the document is view-only.
    pub view_only: Gate,
    /// Allowed while a gesture runs.
    pub while_gesture: Gate,
    /// Behaviour during a text edit.
    pub while_text_edit: TextGate,
    /// Dirty reason on success.
    pub redraw: Redraw,
    /// Ring mode.
    pub ring: RingMode,
    /// Listed in the command palette.
    pub palette: bool,
    /// Exposed to agents.
    pub agent: bool,
    /// How the command relates to the document, undo and view-only mode.
    pub doc_write: DocWrite,
}

impl Facts {
    /// A read-only command: allowed in every state, changes nothing, exposed to agents, listed in no palette.
    pub const READ_ONLY: Facts = Facts {
        undo: Undo::NoUndo,
        reads_only: true,
        mutates: Mutates::NONE,
        needs_selection: Selection::No,
        ports: PortSet::NONE,
        batch: Batch::Allowed,
        view_only: Gate::Allowed,
        while_gesture: Gate::Allowed,
        while_text_edit: TextGate::Allowed,
        redraw: Redraw::None,
        ring: RingMode::Always,
        palette: false,
        agent: true,
        doc_write: DocWrite::NotDocument,
    };

    /// A camera command: changes the view only, allowed in every state.
    pub const CAMERA: Facts =
        Facts { reads_only: false, mutates: Mutates::VIEW, redraw: Redraw::Camera, ..Facts::READ_ONLY };
}

/// One command. Build it with [`CommandSpec::new`] and the optional `const` methods.
pub struct CommandSpec {
    // identity
    /// `family.verb`, lower snake_case, one dot, unique.
    pub id: &'static str,
    /// Palette and menu text, sentence case.
    pub title: &'static str,
    /// The part of `id` before the dot.
    pub family: &'static str,
    /// One sentence for `engine.describe` and MCP.
    pub summary: &'static str,
    // contract
    /// JSON Schema of the argument struct.
    pub args: fn() -> Schema,
    /// JSON Schema of the output struct (the common keys are added by `engine.describe`).
    pub output: fn() -> Schema,
    /// State-dependent availability. Never looks at the arguments.
    pub available: Check,
    /// Checks that need the default arguments (the current selection): `too_few`, `needs_single`. Used by
    /// `Session::availability`, so a disabled control shows the text `execute` would return.
    pub default_targets: Option<Check>,
    /// The typed handler.
    pub handler: &'static dyn Handler,
    /// Command-specific refusal codes this command can return (the shared ones are implied).
    pub refusals: &'static [&'static str],
    /// A worked example of the arguments, as JSON text.
    pub example: &'static str,
    // history
    /// History class.
    pub undo: Undo,
    /// Undo merging.
    pub coalesce: Option<Coalesce>,
    // facts
    /// True: never changes any state class.
    pub reads_only: bool,
    /// State classes it changes.
    pub mutates: Mutates,
    /// Selection use.
    pub needs_selection: Selection,
    /// Ports it may need.
    pub ports: PortSet,
    /// Batch rule.
    pub batch: Batch,
    /// Allowed while view-only.
    pub view_only: Gate,
    /// Allowed during a gesture.
    pub while_gesture: Gate,
    /// Behaviour during a text edit.
    pub while_text_edit: TextGate,
    /// Dirty reason on success.
    pub redraw: Redraw,
    /// Ring mode.
    pub ring: RingMode,
    /// In the palette.
    pub palette: bool,
    /// Exposed to agents.
    pub agent: bool,
    /// How it relates to the document, undo and view-only mode.
    pub doc_write: DocWrite,
    /// What a host does around it. [`Host::NONE`] for most commands.
    pub host: Host,
    /// The agent bridge gives the command a tool of its own (`engine.commands`, `engine.describe`,
    /// `engine.recent`), so no family tool lists it.
    pub own_agent_tool: bool,
    /// Default keys.
    pub shortcuts: &'static [Binding],
    /// Stable or planned.
    pub status: Status,
    /// How the executor runs it.
    pub txn: Txn,
    /// Runs although the document is blocked by a newer schema, although it mutates the document: it
    /// replaces the document or ends an edit in progress. See [`CommandSpec::allowed_when_blocked`].
    pub works_when_blocked: bool,
    /// Top-level arguments that hold a secret (an access token). The ring records `"<hidden>"` for them, so
    /// `engine.recent` and the agent bridge never show them.
    pub secret_args: &'static [&'static str],
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CommandSpec({})", self.id)
    }
}

/// A state check: `Ok` or the refusal a disabled control shows.
pub type Check = fn(&Session) -> Result<(), Refusal>;

/// The default `available`: always.
pub fn always(_: &Session) -> Result<(), Refusal> {
    Ok(())
}

/// The schema of a type. A build without `schemas` generates nothing: the schema code of the types is then never
/// referenced and not linked, and the table of `schemas` answers instead.
#[cfg(feature = "schemas")]
fn schema_of<T: JsonSchema>() -> Schema {
    schemars::schema_for!(T)
}

#[cfg(not(feature = "schemas"))]
fn schema_of<T: JsonSchema>() -> Schema {
    crate::schemas::empty()
}

impl CommandSpec {
    /// The JSON Schema of the arguments: generated from the argument type, or, in a build without `schemas`, read
    /// from the installed table (empty until the host installs it, see [`crate::schemas`]).
    pub fn args_schema(&self) -> Schema {
        #[cfg(feature = "schemas")]
        {
            (self.args)()
        }
        #[cfg(not(feature = "schemas"))]
        {
            crate::schemas::args_of(self.id)
        }
    }

    /// The JSON Schema of the output (without the common keys that `engine.describe` adds).
    pub fn output_schema(&self) -> Schema {
        #[cfg(feature = "schemas")]
        {
            (self.output)()
        }
        #[cfg(not(feature = "schemas"))]
        {
            crate::schemas::output_of(self.id)
        }
    }

    /// The top-level keys of the arguments when the type refuses other keys, else none.
    pub fn arg_keys(&self) -> Vec<String> {
        #[cfg(feature = "schemas")]
        {
            self.handler.arg_keys()
        }
        #[cfg(not(feature = "schemas"))]
        {
            let info = crate::schemas::arg_info_of(self.id);
            if info.closed { info.keys.clone() } else { Vec::new() }
        }
    }

    /// A registration. `family` must equal the part of `id` before the dot (tested).
    pub const fn new<A, O>(
        id: &'static str,
        family: &'static str,
        title: &'static str,
        summary: &'static str,
        handler: &'static Typed<A, O>,
        facts: Facts,
    ) -> Self
    where
        A: DeserializeOwned + Serialize + JsonSchema + Clone + 'static,
        O: Serialize + JsonSchema + 'static,
    {
        Self {
            id,
            title,
            family,
            summary,
            args: schema_of::<A>,
            output: schema_of::<O>,
            available: always,
            default_targets: None,
            handler,
            refusals: &[],
            example: "",
            undo: facts.undo,
            coalesce: None,
            reads_only: facts.reads_only,
            mutates: facts.mutates,
            needs_selection: facts.needs_selection,
            ports: facts.ports,
            batch: facts.batch,
            view_only: facts.view_only,
            while_gesture: facts.while_gesture,
            while_text_edit: facts.while_text_edit,
            redraw: facts.redraw,
            ring: facts.ring,
            palette: facts.palette,
            agent: facts.agent,
            doc_write: facts.doc_write,
            host: Host::NONE,
            own_agent_tool: false,
            shortcuts: &[],
            status: Status::Stable,
            txn: Txn::Standard,
            works_when_blocked: false,
            secret_args: &[],
        }
    }

    /// Sets the state-dependent availability check.
    #[must_use]
    pub const fn available(mut self, f: Check) -> Self {
        self.available = f;
        self
    }

    /// States what a host does around the command (a dialog it closes, a toast it skips, a key that does not
    /// repeat). Listed in `docs/commands.md` Appendix A2; a test compares the two.
    #[must_use]
    pub const fn host(mut self, host: Host) -> Self {
        self.host = host;
        self
    }

    /// The agent bridge has a tool of its own for this command.
    #[must_use]
    pub const fn own_agent_tool(mut self) -> Self {
        self.own_agent_tool = true;
        self
    }

    /// Sets the check that uses the default arguments.
    #[must_use]
    pub const fn default_targets(mut self, f: Check) -> Self {
        self.default_targets = Some(f);
        self
    }

    /// Declares the command-specific refusal codes.
    #[must_use]
    pub const fn refusals(mut self, codes: &'static [&'static str]) -> Self {
        self.refusals = codes;
        self
    }

    /// Names the arguments that hold a secret: the ring records `"<hidden>"` instead of their value.
    #[must_use]
    pub const fn secret_args(mut self, names: &'static [&'static str]) -> Self {
        self.secret_args = names;
        self
    }

    /// Gives a worked example of the arguments (JSON text).
    #[must_use]
    pub const fn example(mut self, json: &'static str) -> Self {
        self.example = json;
        self
    }

    /// Merges rapid repeats into one undo step.
    #[must_use]
    pub const fn coalesce(mut self, key: &'static str, ms: u64) -> Self {
        self.coalesce = Some(Coalesce { key, ms });
        self
    }

    /// Sets the default key bindings.
    #[must_use]
    pub const fn shortcuts(mut self, b: &'static [Binding]) -> Self {
        self.shortcuts = b;
        self
    }

    /// Marks the command planned: registered, refuses with `not_supported`.
    #[must_use]
    pub const fn planned(mut self, reason: &'static str) -> Self {
        self.status = Status::Planned(reason);
        self
    }

    /// The command still runs when a newer version of the app raised the document's schema, so that this build
    /// holds it read-only (docs/crdt.md §8). Every other command that mutates the document refuses with
    /// `newer_schema`. Declare it on commands that replace the document or end an edit in progress, so that
    /// the user is never stuck.
    #[must_use]
    pub const fn allowed_when_blocked(mut self) -> Self {
        self.works_when_blocked = true;
        self
    }

    /// The command manages its own document state (gesture protocol, history).
    #[must_use]
    pub const fn own_transaction(mut self) -> Self {
        self.txn = Txn::Own;
        self
    }

    /// The facts of this registration as one value. Tests destructure it without `..`, so a new fact cannot be
    /// added without deciding how it is compared with `docs/commands.md`.
    pub fn facts(&self) -> Facts {
        Facts {
            undo: self.undo,
            reads_only: self.reads_only,
            mutates: self.mutates,
            needs_selection: self.needs_selection,
            ports: self.ports,
            batch: self.batch,
            view_only: self.view_only,
            while_gesture: self.while_gesture,
            while_text_edit: self.while_text_edit,
            redraw: self.redraw,
            ring: self.ring,
            palette: self.palette,
            agent: self.agent,
            doc_write: self.doc_write,
        }
    }

    /// True when the command manages its own document state (gesture protocol, history).
    pub fn is_own_transaction(&self) -> bool {
        self.txn == Txn::Own
    }

    /// True when the command can be begun as a gesture.
    pub fn is_gesture(&self) -> bool {
        self.undo == Undo::Gesture
    }
}

/// Type-erased access to a command's typed handler.
pub trait Handler: Sync {
    /// Validates JSON against the argument type. `command` is the id, for the message.
    fn parse(&self, command: &str, args: &Value) -> Result<Box<dyn Any>, Refusal>;
    /// Runs the handler with parsed arguments (a `Box` from [`Handler::parse`] or a typed caller).
    fn run(&self, s: &mut Session, args: &dyn Any) -> Result<Value, Refusal>;
    /// The arguments as JSON, for the ring and the gesture log.
    fn to_json(&self, args: &dyn Any) -> Option<Value>;
    /// Boxes a copy of typed arguments if `args` is the right type.
    fn clone_args(&self, args: &dyn Any) -> Option<Box<dyn Any>>;
    /// The names of the top-level argument keys, from the schema (empty when the schema has none).
    fn arg_keys(&self) -> Vec<String>;
}

/// A typed handler: `fn(&mut Session, A) -> Result<O, Refusal>`.
pub struct Typed<A, O> {
    f: fn(&mut Session, A) -> Result<O, Refusal>,
    _types: PhantomData<fn(A) -> O>,
}

impl<A, O> Typed<A, O> {
    /// Wraps a handler.
    pub const fn new(f: fn(&mut Session, A) -> Result<O, Refusal>) -> Self {
        Self { f, _types: PhantomData }
    }
}

impl<A, O> Handler for Typed<A, O>
where
    A: DeserializeOwned + Serialize + JsonSchema + Clone + 'static,
    O: Serialize + JsonSchema + 'static,
{
    fn parse(&self, command: &str, args: &Value) -> Result<Box<dyn Any>, Refusal> {
        let a: A = args::parse_args(command, args)?;
        Ok(Box::new(a))
    }

    fn run(&self, s: &mut Session, args: &dyn Any) -> Result<Value, Refusal> {
        let Some(a) = args.downcast_ref::<A>().cloned() else {
            return Err(Refusal::internal(s.cmd(), "the typed arguments do not match this command"));
        };
        let out = (self.f)(s, a)?;
        serde_json::to_value(out).map_err(|e| Refusal::internal(s.cmd(), format!("output is not JSON: {e}")))
    }

    fn to_json(&self, args: &dyn Any) -> Option<Value> {
        args.downcast_ref::<A>().and_then(|a| serde_json::to_value(a).ok())
    }

    fn clone_args(&self, args: &dyn Any) -> Option<Box<dyn Any>> {
        args.downcast_ref::<A>().map(|a| Box::new(a.clone()) as Box<dyn Any>)
    }

    fn arg_keys(&self) -> Vec<String> {
        let info = args::arg_info::<A>();
        if info.closed { info.keys.clone() } else { Vec::new() }
    }
}

/// An argument struct with no fields, for commands that take none.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}
