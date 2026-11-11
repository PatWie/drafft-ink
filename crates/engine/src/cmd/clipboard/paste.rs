//! `clipboard.paste`: find what the clipboard holds, then put it in.
//!
//! Finding (`source: "system"`, the old native order): the drafft.ink payload, Excalidraw clipboard JSON, text
//! that parses as a Mermaid diagram, an image, plain text. `plain_text` skips to the last step. The other sources
//! name the data: `internal` is the session's last copy, `{text}`, `{image}` and `{elements}` are what a shell
//! read from the browser's paste event.
//!
//! Putting in: elements go through `drafftink_edit::paste::insert_elements` (new ids, new groups, remapped
//! bindings, centred on `at`), an image through the `image.insert` logic, a diagram through
//! `diagram.import_mermaid`, and text becomes a text element in the default font.

use drafftink_edit::CopyError;
use drafftink_edit::create::CreateError;
use drafftink_geom::kurbo::Point;
use drafftink_io::clipboard::{self as wire, TextKind};
use drafftink_io::{Imported, native};
use drafftink_scene::{Document, ElementId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::common::{At, edit_with, resolve_at, select_only};
use super::CopyBuffer;
use crate::cmd::image::{Bytes, DEFAULT_FIT, DEFAULT_OFFSET, Loaded, Placement, insert_pictures};
use crate::ports::{ClipKind, ClipPayload};
use crate::refusal::{Refusal, echo};
use crate::session::Session;

/// The words `source` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceWord {
    /// The system clipboard through the `Clipboard` port.
    #[default]
    System,
    /// The session's last copy.
    Internal,
}

/// Plain text given by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextSource {
    /// The text. It is sniffed like clipboard text unless `plain_text` is set.
    pub text: String,
}

/// An image given by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageSource {
    /// The picture: `{base64, name?, mime?}` or `{path}`.
    pub image: Bytes,
}

/// Clipboard elements given by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElementsSource {
    /// A drafft.ink clipboard payload (what `clipboard.copy` writes) or an Excalidraw clipboard object.
    pub elements: Value,
}

/// Where the data comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Source {
    /// `"system"` or `"internal"`.
    Word(SourceWord),
    /// `{text}`.
    Text(TextSource),
    /// `{image}`.
    Image(ImageSource),
    /// `{elements}`.
    Elements(ElementsSource),
}

impl Default for Source {
    fn default() -> Self {
        Source::Word(SourceWord::System)
    }
}

fn yes() -> bool {
    true
}

/// Arguments of `clipboard.paste`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteArgs {
    /// Where the data comes from. Default: the system clipboard.
    #[serde(default)]
    pub source: Source,
    /// Where the middle of what is pasted goes, in world units. Default: the middle of the visible canvas. The
    /// keyboard passes the pointer position.
    #[serde(default)]
    pub at: At,
    /// Select what was pasted.
    #[serde(default = "yes")]
    pub select: bool,
    /// Paste the text as a text element without looking for Excalidraw data or a Mermaid diagram.
    #[serde(default)]
    pub plain_text: bool,
}

impl Default for PasteArgs {
    fn default() -> Self {
        Self { source: Source::default(), at: At::default(), select: true, plain_text: false }
    }
}

/// What kind of data was pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// drafft.ink elements.
    Elements,
    /// Excalidraw elements.
    Excalidraw,
    /// A Mermaid diagram.
    Mermaid,
    /// A picture.
    Image,
    /// A text element.
    Text,
}

/// Output of `clipboard.paste`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PasteOut {
    /// What was found and pasted.
    pub kind: Kind,
    /// The new elements that make the selection (bound text excluded). With `select` false and a Mermaid
    /// diagram this is empty: read `created`.
    pub ids: Vec<String>,
    /// Elements created, bound text included.
    pub count: usize,
}

/// What was found.
enum Found {
    Elements { doc: Document, kind: Kind },
    Mermaid(String),
    Image(Loaded),
    Text(String),
}

fn empty(cmd: &str) -> Refusal {
    Refusal::custom(
        "clipboard_empty",
        cmd,
        "The clipboard has nothing drafft.ink can paste. Supported: copied elements, Excalidraw data, images, Mermaid diagrams and plain text.",
    )
}

fn unreadable(cmd: &str, what: &str, msg: &str, hint: &str) -> Refusal {
    Refusal::parse_failed(cmd, what, msg, hint)
}

fn note_import(s: &mut Session, imported: &Imported) {
    for w in imported.report.warnings.iter().take(5) {
        s.warn(w.clone());
    }
    let more = imported.report.warnings.len().saturating_sub(5) + imported.report.omitted;
    if more > 0 {
        s.warn(format!("{more} more notes about the pasted data were left out."));
    }
}

/// Reads an elements value: our envelope, or an Excalidraw clipboard object.
fn from_elements(s: &mut Session, v: &Value) -> Result<Found, Refusal> {
    let cmd = s.cmd();
    if wire::is_envelope(v) {
        let doc = wire::unpack(v).map_err(|e| {
            unreadable(cmd, "The copied elements", &e.to_string(), "Copy them again with `clipboard.copy`.")
        })?;
        return Ok(Found::Elements { doc, kind: Kind::Elements });
    }
    let text = v.to_string();
    let imported = drafftink_io::excalidraw::import_clipboard(&text, &mut *s.ports.ids).map_err(|e| {
        unreadable(
            cmd,
            "The clipboard elements",
            &e.to_string(),
            "Pass drafft.ink clipboard data (from `clipboard.copy`) or an Excalidraw clipboard object.",
        )
    })?;
    note_import(s, &imported);
    Ok(Found::Elements { doc: imported.document, kind: Kind::Excalidraw })
}

/// Sniffs text: elements, a diagram, or plain text.
fn from_text(s: &mut Session, text: &str) -> Result<Found, Refusal> {
    let cmd = s.cmd();
    match wire::classify_text(text, &mut *s.ports.ids) {
        Ok(TextKind::Elements(imported)) => {
            note_import(s, &imported);
            let kind = if native::sniff(text.as_bytes()) == native::Format::ExcalidrawClipboard {
                Kind::Excalidraw
            } else {
                Kind::Elements
            };
            Ok(Found::Elements { doc: imported.document, kind })
        }
        Ok(TextKind::Mermaid(_)) => Ok(Found::Mermaid(text.to_owned())),
        Ok(TextKind::Plain { note }) => {
            if let Some(n) = note {
                s.warn(format!("The text looks like a Mermaid diagram but could not be read ({}), so it was pasted as text.", echo(&n)));
            }
            Ok(Found::Text(text.to_owned()))
        }
        Err(e) => Err(unreadable(
            cmd,
            "The pasted Excalidraw data",
            &e.to_string(),
            "Copy the elements again in Excalidraw, or paste as plain text with `plain_text`.",
        )),
    }
}

fn clipboard_error(cmd: &str, e: &crate::ports::PortError) -> Refusal {
    Refusal::io_failed(cmd, "read", "the system clipboard", &e.to_string())
}

/// The system clipboard, in the old native order.
fn from_system(s: &mut Session, plain_text: bool) -> Result<Found, Refusal> {
    let cmd = s.cmd();
    if s.ports.clipboard.is_none() {
        return Err(Refusal::custom(
            "port_unavailable",
            cmd,
            "`clipboard.paste` with source system needs the system clipboard, which this session does not provide. Pass `source` with the data instead.",
        )
        .suggesting(cmd, Some(json!({"source": "internal"}))));
    }
    let read = |s: &Session, kind: ClipKind| -> Result<Option<ClipPayload>, Refusal> {
        match s.ports.clipboard.as_ref() {
            Some(c) => c.read(&[kind]).map_err(|e| clipboard_error(cmd, &e)),
            None => Ok(None),
        }
    };
    if plain_text {
        return match read(s, ClipKind::Text)? {
            Some(ClipPayload::Text(t)) if !t.trim().is_empty() => Ok(Found::Text(t)),
            _ => Err(empty(cmd)),
        };
    }
    if let Some(ClipPayload::Elements(v)) = read(s, ClipKind::Elements)? {
        return from_elements(s, &v);
    }
    let mut plain: Option<String> = None;
    if let Some(ClipPayload::Text(t)) = read(s, ClipKind::Text)?
        && !t.trim().is_empty()
    {
        match from_text(s, &t)? {
            Found::Text(t) => plain = Some(t),
            found => return Ok(found),
        }
    }
    if let Some(ClipPayload::Image { bytes, .. }) = read(s, ClipKind::Image)? {
        if drafftink_io::picture::probe(&bytes).is_ok() || plain.is_none() {
            return Ok(Found::Image(Loaded { bytes, name: "the clipboard image".to_owned() }));
        }
        s.warn("The clipboard image is not a PNG, JPEG or WebP, so its text was pasted instead.");
    }
    plain.map(Found::Text).ok_or_else(|| empty(cmd))
}

fn find(s: &mut Session, a: &PasteArgs) -> Result<Found, Refusal> {
    let cmd = s.cmd();
    match &a.source {
        Source::Word(SourceWord::System) => from_system(s, a.plain_text),
        Source::Word(SourceWord::Internal) => {
            if a.plain_text {
                return Err(Refusal::bad_arg(
                    cmd,
                    "plain_text",
                    "cannot be used with source \"internal\", which holds elements and no text",
                ));
            }
            match s.ext.with::<CopyBuffer, _>(|b| b.envelope.clone()) {
                Some(v) => from_elements(s, &v),
                None => Err(empty(cmd)),
            }
        }
        Source::Text(t) if t.text.trim().is_empty() => Err(empty(cmd)),
        Source::Text(t) if a.plain_text => Ok(Found::Text(t.text.clone())),
        Source::Text(t) => from_text(s, &t.text),
        Source::Image(i) => Ok(Found::Image(i.image.load(s, "the pasted image")?)),
        Source::Elements(e) => from_elements(s, &e.elements),
    }
}

fn paste_refusal(cmd: &str, e: drafftink_edit::paste::PasteError) -> Refusal {
    use drafftink_edit::paste::PasteError as E;
    match e {
        E::Empty | E::Text(CreateError::EmptyText) => empty(cmd),
        E::TooMany { n, max } => Refusal::limit_exceeded(cmd, n, "elements to paste", max),
        E::Text(CreateError::TextTooLong { n, max }) => Refusal::limit_exceeded(cmd, n, "characters of text", max),
        E::NotFinite => Refusal::bad_arg(cmd, "at", "puts the pasted elements outside the area the document allows"),
        E::Copy(CopyError::Invalid { .. }) => unreadable(cmd, "The pasted elements", &e.to_string(), "Copy them again."),
        other => Refusal::internal(cmd, other.to_string()),
    }
}

pub(super) fn paste(s: &mut Session, a: PasteArgs) -> Result<PasteOut, Refusal> {
    let cmd = s.cmd();
    let at = resolve_at(s, &a.at)?;
    let found = find(s, &a)?;
    match found {
        Found::Elements { doc, kind } => put_elements(s, &doc, kind, at, a.select),
        Found::Mermaid(text) => put_mermaid(s, &text, at, a.select),
        Found::Image(pic) => {
            let place = Placement { at, fit: DEFAULT_FIT, offset: DEFAULT_OFFSET, select: a.select, downscale: true };
            let done = insert_pictures(s, vec![pic], &place)?;
            Ok(PasteOut { kind: Kind::Image, ids: done.iter().map(|i| i.id.clone()).collect(), count: done.len() })
        }
        Found::Text(text) => put_text(s, cmd, &text, at, a.select),
    }
}

fn put_elements(s: &mut Session, doc: &Document, kind: Kind, at: Point, select: bool) -> Result<PasteOut, Refusal> {
    let cmd = s.cmd();
    let opts = drafftink_edit::paste::PasteOpts { center: Some(at) };
    let report = edit_with::<_, drafftink_edit::paste::PasteError>(s, |tx, ids, _| {
        drafftink_edit::paste::insert_elements(tx, &doc.elements, &doc.files, &opts, ids)
    })
    .map_err(|e| paste_refusal(cmd, e))?;
    if select {
        select_only(s, &report.selection);
    }
    Ok(PasteOut {
        kind,
        ids: report.selection.iter().map(|i| i.as_str().to_owned()).collect(),
        count: report.created.len(),
    })
}

fn put_mermaid(s: &mut Session, text: &str, at: Point, select: bool) -> Result<PasteOut, Refusal> {
    let cmd = s.cmd();
    if s.spec("diagram.import_mermaid").is_none() {
        // The diagram family is not part of this build: keep the text rather than lose the paste.
        s.warn("Mermaid import is not available in this build, so the diagram text was pasted as text.");
        return put_text(s, cmd, text, at, select);
    }
    s.run_nested(
        "diagram.import_mermaid",
        json!({"source": {"text": text}, "at": [at.x, at.y], "select": select}),
    )?;
    let ids: Vec<String> = if select { s.selection().iter().map(|i: &ElementId| i.as_str().to_owned()).collect() } else { Vec::new() };
    Ok(PasteOut { kind: Kind::Mermaid, count: ids.len(), ids })
}

fn put_text(s: &mut Session, cmd: &str, text: &str, at: Point, select: bool) -> Result<PasteOut, Refusal> {
    let font = s.tool.text_defaults.clone();
    let style = s.tool.style_defaults.clone();
    let id = edit_with::<_, drafftink_edit::paste::PasteError>(s, |tx, ids, services| {
        drafftink_edit::paste::insert_text(tx, services.edit(), ids, text, font, style, at)
    })
    .map_err(|e| paste_refusal(cmd, e))?;
    if select {
        select_only(s, std::slice::from_ref(&id));
    }
    Ok(PasteOut { kind: Kind::Text, ids: vec![id.as_str().to_owned()], count: 1 })
}
