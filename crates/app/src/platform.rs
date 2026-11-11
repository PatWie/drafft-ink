//! The seam between the shared app and one platform (desktop or web).
//!
//! A platform builds a [`Platform`]: the engine ports it can serve, a [`Host`] for the things only the
//! operating system can do (pickers), and a [`WakeCell`] that every background thread or browser
//! callback uses to wake the event loop. Results of asynchronous work (a chosen file, the clipboard
//! text) come back through the [`Inbox`] as [`Inbound`] messages, which the app drains on its next wake.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use drafftink_bridge::BridgeHost;
use drafftink_engine::Ports;
use drafftink_engine::ports::{ClipKind, ClipPayload, FileRef};
use drafftink_gpu::{ClipboardContent, RedrawReason, Waker};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wakes the event loop from any thread or browser callback.
///
/// The shell's [`Waker`] exists only after the loop started, but the ports (a network transport that
/// wakes on a frame, the bridge that wakes on a request) are built before. They hold a clone of this
/// cell, and the app fills it in `ShellApp::setup`. A wake before that is dropped; the app polls
/// everything once at its first frame anyway.
#[derive(Clone, Default)]
pub struct WakeCell(Arc<Mutex<Option<Waker>>>);

impl WakeCell {
    /// An empty cell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wakes the loop for `reason`. Returns false when the loop is not running (yet, or any more).
    pub fn wake(&self, reason: RedrawReason) -> bool {
        lock(&self.0).as_ref().is_some_and(|w| w.wake(reason))
    }

    /// A closure that wakes the loop with `reason`, for transports and the bridge.
    pub fn hook(&self, reason: RedrawReason) -> Arc<dyn Fn() + Send + Sync> {
        let cell = self.clone();
        Arc::new(move || {
            cell.wake(reason);
        })
    }

    pub(crate) fn set(&self, waker: Waker) {
        *lock(&self.0) = Some(waker);
    }
}

impl std::fmt::Debug for WakeCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WakeCell").field("ready", &lock(&self.0).is_some()).finish()
    }
}

/// A file the user chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickedFile {
    /// A file the session can read itself (a path on the desktop). It also becomes the save target.
    Ref(FileRef),
    /// A file the platform already read (a browser file input, a dropped file).
    Bytes {
        /// The file name.
        name: String,
        /// The content.
        bytes: Vec<u8>,
    },
}

/// What the app asks the host to do. Every job ends in at most one [`Inbound`] message; a cancelled
/// picker sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostJob {
    /// Choose a document to open (`.drafftink`, `.json`, `.excalidraw`, PNG with a scene). Answered by
    /// [`Inbound::Picked`] with [`PickKind::Document`].
    PickDocument,
    /// Choose one or more pictures. Answered by [`Inbound::Picked`] with [`PickKind::Images`].
    PickImages,
    /// Choose an Excalidraw library. Answered by [`Inbound::Picked`] with [`PickKind::Library`].
    PickLibrary,
    /// Choose an Excalidraw library to merge into the library panel. Answered by [`Inbound::Picked`] with
    /// [`PickKind::LibraryImport`].
    PickLibraryImport,
    /// Choose where to save. `suggested` is a file name. Answered by [`Inbound::SaveTarget`].
    PickSaveTarget {
        /// A file name such as `Untitled.drafftink`.
        suggested: String,
    },
    /// Read the text on the system clipboard. Answered by [`Inbound::ClipboardText`].
    ReadClipboardText,
    /// Fetch a font file. Answered by [`Inbound::FontLoaded`], or by [`Inbound::Notice`] when it fails. Only a
    /// build that leaves fonts out of the binary (the web) asks for this.
    LoadFont(FontFile),
}

/// A font file the platform loads for the app: a text font for the canvas or one for the egui UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFile {
    /// Noto Sans or the math font, for the typesetter.
    Text(drafftink_typeset::ExtraFont),
    /// A font of the egui UI.
    Ui(drafftink_ui::theme::UiFont),
}

impl FontFile {
    /// The file name (the asset the web build serves).
    pub fn file_name(self) -> &'static str {
        match self {
            FontFile::Text(f) => f.file_name(),
            FontFile::Ui(f) => f.file_name(),
        }
    }
}

/// What a [`HostJob::PickDocument`] and its siblings were for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    /// A document for `doc.open`.
    Document,
    /// Pictures for `image.insert`.
    Images,
    /// A library to open as a tab (`library.import` with `open_as_tab`).
    Library,
    /// A library to merge into the library panel (`library.import` with `merge`).
    LibraryImport,
}

/// A message from a platform callback to the app. Drained on the app's thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// The user chose files.
    Picked {
        /// What they were chosen for.
        kind: PickKind,
        /// The files.
        files: Vec<PickedFile>,
    },
    /// The user chose a save location (a path on the desktop, a download name on the web).
    SaveTarget(FileRef),
    /// The text on the system clipboard, for the equation editor.
    ClipboardText(String),
    /// A plain note for a toast (a picker failed, a file could not be read).
    Notice(String),
    /// A font file arrived ([`HostJob::LoadFont`]).
    FontLoaded {
        /// Which font.
        file: FontFile,
        /// The bytes of the font file.
        bytes: Vec<u8>,
    },
}

/// The sending end of the app's inbox. Cheap to clone and `Send`.
#[derive(Clone)]
pub struct Inbox {
    tx: Sender<Inbound>,
    wake: WakeCell,
}

impl Inbox {
    /// Delivers `message` and wakes the loop.
    pub fn post(&self, message: Inbound) {
        // A closed receiver means the app is gone; there is nobody to tell.
        if self.tx.send(message).is_ok() {
            self.wake.wake(RedrawReason::Load);
        }
    }
}

impl std::fmt::Debug for Inbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Inbox")
    }
}

/// The receiving end, owned by the app.
pub(crate) struct InboxRx(pub(crate) Receiver<Inbound>);

/// Makes the two ends of the inbox.
pub(crate) fn inbox(wake: WakeCell) -> (Inbox, InboxRx) {
    let (tx, rx) = channel();
    (Inbox { tx, wake }, InboxRx(rx))
}

/// What only the platform can do. Methods return at once; a result arrives later through the inbox.
pub trait Host {
    /// Starts `job`. `reply` delivers the answer, possibly from another thread or a browser callback.
    fn start(&mut self, job: HostJob, reply: Inbox);
}

/// A host that can do nothing (tests and headless runs): every job is answered with a notice.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHost;

impl Host for NoHost {
    fn start(&mut self, job: HostJob, reply: Inbox) {
        reply.post(Inbound::Notice(format!("{job:?} is not available in this build.")));
    }
}

/// The system clipboard as the engine's synchronous `Clipboard` port sees it.
///
/// The port cannot wait for the operating system. The shell delivers a paste as an event with the content
/// already read, and the app stores it here before it runs `clipboard.paste`; the port then reads it back, kind
/// by kind. A write by `clipboard.copy` lands in the out-slot, and the app hands it to the shell, which writes
/// it to the system clipboard inside the event that asked for it (the browser requires that).
#[derive(Clone, Default)]
pub struct SharedClipboard {
    inner: Arc<Mutex<ClipSlots>>,
}

#[derive(Default)]
struct ClipSlots {
    incoming: Option<ClipboardContent>,
    outgoing: Option<ClipPayload>,
}

impl SharedClipboard {
    /// An empty clipboard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores what the system clipboard held, for the next read.
    pub fn set_incoming(&self, content: Option<ClipboardContent>) {
        lock(&self.inner).incoming = content;
    }

    /// Takes what a command wrote, to hand it to the system clipboard.
    pub fn take_outgoing(&self) -> Option<ClipboardContent> {
        lock(&self.inner).outgoing.take().map(|p| crate::clip::to_content(&p))
    }
}

impl drafftink_engine::ports::Clipboard for SharedClipboard {
    fn read(&self, want: &[ClipKind]) -> Result<Option<ClipPayload>, drafftink_engine::PortError> {
        let slots = lock(&self.inner);
        let Some(content) = slots.incoming.as_ref() else { return Ok(None) };
        let kinds: &[ClipKind] =
            if want.is_empty() { &[ClipKind::Elements, ClipKind::Image, ClipKind::Svg, ClipKind::Text] } else { want };
        Ok(kinds.iter().find_map(|k| crate::clip::payload_of(content, *k)))
    }

    fn write(&mut self, payload: ClipPayload) -> Result<(), drafftink_engine::PortError> {
        let mut slots = lock(&self.inner);
        // The next paste inside this app sees what was just copied, whatever the system clipboard says.
        slots.incoming = Some(crate::clip::to_content(&payload));
        slots.outgoing = Some(payload);
        Ok(())
    }
}

/// Everything a platform provides to build an [`App`](crate::App).
pub struct Platform {
    /// The engine ports this platform serves. Build the clipboard port from `clipboard`.
    pub ports: Ports,
    /// Pickers and other OS services.
    pub host: Box<dyn Host>,
    /// The cell the ports' background threads wake the loop with. The app fills it in `setup`.
    pub wake: WakeCell,
    /// The clipboard the ports and the app share.
    pub clipboard: SharedClipboard,
    /// The text engine the session and the renderer share (one set of font caches).
    pub typesetter: drafftink_engine::services::SharedTypesetter,
    /// Requests from the agent bridge (native server) or the page (web), run once per frame.
    pub bridge: Option<BridgeHost>,
    /// macOS: `Mod` is Cmd.
    pub is_mac: bool,
}
