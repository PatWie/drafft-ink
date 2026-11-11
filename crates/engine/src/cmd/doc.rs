//! `doc`: documents (docs/commands.md §2.1).
//!
//! A document is the content of a tab. The commands here create, open, save, export and list it. The
//! file formats are `drafftink-io` and the merge and insert rules are `drafftink_edit::library`; this
//! module validates arguments, checks the state (unsaved changes, rooms, ports) and moves bytes through
//! the ports.
//!
//! * [`files`]: `source` arguments, `FileRef`s over `Files` and `Storage`, the save target, the recent list.
//! * `open`: `doc.new`, `doc.open`, `doc.drop`, `doc.show_intro`.
//! * `save`: `doc.save`, `doc.autosave`, `doc.rename`, `doc.set_background`, `doc.clear`.
//! * `export`: `doc.export`, the single export path for PNG, SVG, JSON and `.excalidraw`.
//! * `list`: `doc.list`. (`doc.inspect` and `doc.outline` belong to the `doc_read` family.)

pub(crate) mod export;
pub(crate) mod files;
pub(crate) mod open;
mod list;
mod save;

pub use files::{AUTOSAVE_NAME, SaveTarget, save_target, set_save_target, use_stored_default_target};
pub use open::intro_document;

use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed,
    Undo,
};

/// A command that replaces or adds documents: tabs, selection, camera and the document change.
const REPLACES: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED).with(Mutates::TABS).with(Mutates::VIEW),
    needs_selection: Selection::No,
    ports: PortSet::FILES.with(PortSet::STORAGE),
    batch: Batch::Last,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: true,
    agent: true,
};

/// A command that edits the document.
const EDITS: Facts = Facts {
    undo: Undo::Undoable,
    mutates: Mutates::DOC,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    ..REPLACES
};

/// A command that only reads or writes through ports.
const PORTS: Facts = Facts {
    mutates: Mutates::PORTS,
    view_only: Gate::Allowed,
    redraw: Redraw::None,
    batch: Batch::Allowed,
    ..REPLACES
};

const SAVE_KEYS: &[Binding] = &[Binding::new(&["Mod+S"], Scope::Always)];
const EXPORT_KEYS: &[Binding] =
    &[Binding::new(&["Mod+E"], Scope::Canvas).with_args(r#"{"format":"png"}"#).titled("Export as PNG")];
const CLEAR_KEYS: &[Binding] = &[Binding::new(&["Mod+Delete"], Scope::Canvas)];

/// The doc family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "doc.new",
        "doc",
        "New document",
        "Replaces the active document with a blank one, or opens a blank one in a new tab.",
        &Typed::new(open::new),
        REPLACES,
    )
    .refusals(&["empty_name", "in_room", "no_save_target", "too_many_tabs"])
    .allowed_when_blocked()
    .example(r#"{"name":"Sketch","in_new_tab":true}"#),
    CommandSpec::new(
        "doc.open",
        "doc",
        "Open document",
        "Opens a drafft.ink, old drafft-ink, Excalidraw or PNG file (or bytes) into the active tab, a new tab, or merged into the active document.",
        &Typed::new(open::open),
        Facts { undo: Undo::Undoable, ..REPLACES },
    )
    .refusals(&["is_library", "newer_version", "in_room", "no_save_target", "too_many_tabs"])
    .allowed_when_blocked()
    .example(r#"{"source":{"path":"/home/me/plan.json"},"mode":"new_tab"}"#),
    CommandSpec::new(
        "doc.drop",
        "doc",
        "Open dropped files",
        "Routes dropped files by content: images are inserted, documents opened, libraries imported.",
        &Typed::new(open::drop_files),
        Facts { undo: Undo::Undoable, palette: false, agent: false, ..REPLACES },
    )
    .refusals(&["in_room", "no_save_target", "too_many_tabs"]),
    CommandSpec::new(
        "doc.save",
        "doc",
        "Save",
        "Writes the active document to its save target or to `to`, and marks it saved.",
        &Typed::new(save::save),
        Facts { mutates: Mutates::PORTS.with(Mutates::TABS), ..PORTS },
    )
    .shortcuts(SAVE_KEYS)
    .refusals(&["no_save_target", "would_overwrite"])
    .example(r#"{"to":{"path":"/home/me/plan.json"}}"#),
    CommandSpec::new(
        "doc.rename",
        "doc",
        "Rename document",
        "Sets the document name, 1 to 256 characters; the tab title follows.",
        &Typed::new(save::rename),
        Facts { mutates: Mutates::DOC.with(Mutates::TABS), ..EDITS },
    )
    .refusals(&["empty_name"])
    .example(r#"{"name":"Architecture"}"#),
    CommandSpec::new(
        "doc.set_background",
        "doc",
        "Set background colour",
        "Sets the background colour stored in the document; it is saved, synced and used by exports.",
        &Typed::new(save::set_background),
        Facts { palette: false, ..EDITS },
    )
    .example(r##"{"color":"#fafafa"}"##),
    CommandSpec::new(
        "doc.export",
        "doc",
        "Export",
        "Exports the selection or the whole document as PNG, SVG, JSON or .excalidraw, inline or to a file.",
        &Typed::new(export::export),
        Facts {
            needs_selection: Selection::Soft,
            ports: PortSet::FILES.with(PortSet::RENDERER),
            ..PORTS
        },
    )
    .shortcuts(EXPORT_KEYS)
    .example(r#"{"format":"svg","scope":"all"}"#),
    CommandSpec::new(
        "doc.clear",
        "doc",
        "Clear document",
        "Deletes every element, locked ones included, and clears the selection. One undo step.",
        &Typed::new(save::clear),
        Facts { mutates: Mutates::DOC.with(Mutates::OBSERVED), ..EDITS },
    )
    .available(save::not_empty)
    .shortcuts(CLEAR_KEYS)
    .refusals(&["empty_document"]),
    CommandSpec::new(
        "doc.show_intro",
        "doc",
        "Show intro document",
        "Loads the built-in welcome document into the active tab or a new tab.",
        &Typed::new(open::show_intro),
        Facts { ports: PortSet::NONE, ..REPLACES },
    )
    .refusals(&["in_room", "no_save_target", "too_many_tabs"])
    .allowed_when_blocked(),
    CommandSpec::new(
        "doc.autosave",
        "doc",
        "Autosave now",
        "Writes the workspace (every tab) to storage now, if a document changed since the last write.",
        &Typed::new(save::autosave),
        Facts {
            ports: PortSet::STORAGE,
            while_gesture: Gate::Allowed,
            while_text_edit: TextGate::Allowed,
            palette: false,
            agent: false,
            ..PORTS
        },
    )
    .available(save::needs_storage)
    .refusals(&["workspace_newer"]),
    CommandSpec::new(
        "doc.list",
        "doc",
        "List saved documents",
        "Lists recent, stored or autosaved documents with the reference to pass to `doc.open`.",
        &Typed::new(list::list),
        Facts { ports: PortSet::FILES.with(PortSet::STORAGE), palette: true, ..Facts::READ_ONLY },
    ),
];

#[cfg(test)]
#[path = "doc_tests.rs"]
mod tests;
