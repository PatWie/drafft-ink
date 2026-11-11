//! Ports: everything outside the process comes through these traits (docs/commands.md §1.11).
//!
//! Commands never touch the OS, the network or the UI. A shell passes real implementations to
//! [`crate::Session::new`]; tests use the in-memory fakes in [`fake`]. A missing optional port makes the
//! commands that need it refuse with `port_unavailable`. Nothing here awaits: the shell resolves anything
//! asynchronous before it calls a command.

use drafftink_collab::Transport;
use drafftink_scene::{Color, Document, ElementId, IdSource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::TabId;

/// A port call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    /// The port does not support this operation on this platform.
    #[error("not supported here: {0}")]
    Unsupported(String),
    /// The thing asked for does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The operation failed.
    #[error("{0}")]
    Failed(String),
}

/// What a clipboard read may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    /// Drafft.ink or Excalidraw elements.
    Elements,
    /// Plain text.
    Text,
    /// A raster image.
    Image,
    /// SVG text.
    Svg,
}

/// Clipboard content.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipPayload {
    /// Elements as JSON (the clipboard family defines the shape).
    Elements(Value),
    /// Plain text.
    Text(String),
    /// Encoded image bytes with a media type.
    Image {
        /// The bytes.
        bytes: Vec<u8>,
        /// Media type such as `image/png`.
        mime: String,
    },
    /// SVG source.
    Svg(String),
}

impl ClipPayload {
    /// The kind of this payload.
    pub fn kind(&self) -> ClipKind {
        match self {
            Self::Elements(_) => ClipKind::Elements,
            Self::Text(_) => ClipKind::Text,
            Self::Image { .. } => ClipKind::Image,
            Self::Svg(_) => ClipKind::Svg,
        }
    }
}

/// The system clipboard, or the shell's copy of it.
pub trait Clipboard {
    /// The newest content of one of the wanted kinds, if any.
    fn read(&self, want: &[ClipKind]) -> Result<Option<ClipPayload>, PortError>;
    /// Replaces the content.
    fn write(&mut self, payload: ClipPayload) -> Result<(), PortError>;
}

/// Storage namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ns {
    /// Saved documents (the web document store).
    Documents,
    /// The autosave slot.
    Autosave,
    /// Persisted settings.
    Settings,
    /// The shape library.
    Library,
}

/// One stored entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredInfo {
    /// Key in the namespace.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Last write, milliseconds since the Unix epoch.
    pub modified_ms: u64,
}

/// Persistent key-value storage (IndexedDB on the web, the config directory natively).
pub trait Storage {
    /// The bytes under `key`.
    fn get(&self, ns: Ns, key: &str) -> Result<Option<Vec<u8>>, PortError>;
    /// Stores bytes under `key`.
    fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError>;
    /// Removes `key`. Removing a missing key is not an error.
    fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError>;
    /// Every entry of the namespace, sorted by name.
    fn list(&self, ns: Ns) -> Result<Vec<StoredInfo>, PortError>;
}

/// Where a file lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FileRef {
    /// A path on the native file system.
    Path {
        /// The path.
        path: String,
    },
    /// A name in the `Storage` documents namespace.
    Stored {
        /// The name.
        stored: String,
    },
    /// A download: the shell offers the bytes to the user under this name.
    Download {
        /// The file name.
        download: String,
    },
    /// An opaque File System Access handle.
    Handle {
        /// The handle id.
        handle: String,
    },
}

/// Names a file write that the port has accepted and will finish later (a browser file handle, a download
/// prompt). The shell reports the end with `Session::on_write_done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct WriteToken(pub u64);

/// How a [`Files::write`] ended, as far as the port knows when it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The bytes are written. Holds where the file ended up.
    Done(FileRef),
    /// The port started the write and will finish it later. Until the shell calls `Session::on_write_done`
    /// with this token, the document stays unsaved.
    Pending(WriteToken),
}

/// File access by [`FileRef`].
pub trait Files {
    /// Reads a whole file.
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError>;
    /// True when the file exists.
    fn exists(&self, file: &FileRef) -> bool;
    /// Writes a file. A native port returns [`WriteOutcome::Done`] or the error. A browser port, whose write
    /// completes after this call, returns [`WriteOutcome::Pending`] and reports the end later. A download
    /// hands the bytes to the shell.
    fn write(&mut self, file: &FileRef, bytes: &[u8], mime: &str) -> Result<WriteOutcome, PortError>;
}

/// Wall time for timestamps and monotonic time for timing. Code below L6 never calls
/// `Instant::now` (it panics in the browser).
pub trait Clock {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
    /// Milliseconds from a monotonic source.
    fn mono_ms(&self) -> u64;
}

/// Element ids and hand-drawn seeds come from the scene crate's [`IdSource`]: random in the app,
/// sequential in tests.
pub use drafftink_scene::IdSource as Ids;

/// The scene handed to the renderer.
pub type SceneView = Document;

/// What the offscreen render should draw.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRequest {
    /// World rectangle `(x, y, w, h)` to draw.
    pub region: (f64, f64, f64, f64),
    /// Output size in pixels.
    pub size_px: (u32, u32),
    /// Fill the document background.
    pub background: bool,
    /// Draw selection handles and guides.
    pub overlay: bool,
    /// Selected ids, for the overlay.
    pub selection: Vec<ElementId>,
    /// Background colour to use when `background` is set.
    pub background_color: Color,
    /// Draw with the dark palette (`doc.export` with `dark`).
    pub dark: bool,
}

/// Offscreen rendering for PNG export and screenshots.
pub trait Renderer {
    /// Renders PNG bytes.
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError>;
}

/// The network, as the collab client needs it. The shell owns the sockets: it builds a [`Transport`] for each
/// connection (native WebSocket thread, browser `WebSocket`) and hands it its wake-up hook, so the engine never
/// polls on a timer. Commands only call this port through the collab family.
pub trait Net {
    /// A new transport for one connection. It is dropped when the connection ends.
    fn open_transport(&mut self) -> Result<Box<dyn Transport>, PortError>;

    /// 128 random bits for client ids, room names and reconnect jitter. The engine has no random source of
    /// its own.
    fn random(&mut self) -> u128;

    /// The WebSocket address of `path` on the server that delivered this app, for web shells (`"/ws"` becomes
    /// `wss://host/ws`). `None` where there is no such server (native).
    fn same_origin_url(&self, _path: &str) -> Option<String> {
        None
    }

    /// True when plain `ws://` to another machine is allowed. Off by default: the drawing would travel
    /// unencrypted.
    fn allow_insecure(&self) -> bool {
        false
    }
}

/// Names a port, for `CommandSpec::ports` and refusal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    /// [`Clipboard`].
    Clipboard,
    /// [`Storage`].
    Storage,
    /// [`Files`].
    Files,
    /// [`Renderer`].
    Renderer,
    /// [`Net`].
    Net,
    /// [`Clock`].
    Clock,
}

impl PortKind {
    /// Every kind.
    pub const ALL: [PortKind; 6] =
        [Self::Clipboard, Self::Storage, Self::Files, Self::Renderer, Self::Net, Self::Clock];

    /// Lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Storage => "storage",
            Self::Files => "files",
            Self::Renderer => "renderer",
            Self::Net => "net",
            Self::Clock => "clock",
        }
    }
}

/// The ports of one session. Clock and ids are always present; the rest are optional.
pub struct Ports {
    /// Time.
    pub clock: Box<dyn Clock>,
    /// Ids and seeds.
    pub ids: Box<dyn IdSource>,
    /// The clipboard, if the shell has one.
    pub clipboard: Option<Box<dyn Clipboard>>,
    /// Persistent storage.
    pub storage: Option<Box<dyn Storage>>,
    /// File access.
    pub files: Option<Box<dyn Files>>,
    /// Offscreen rendering.
    pub renderer: Option<Box<dyn Renderer>>,
    /// The collab transport.
    pub net: Option<Box<dyn Net>>,
}

impl Ports {
    /// Ports with only the mandatory ones. Add the others with the `with_*` methods.
    pub fn new(clock: Box<dyn Clock>, ids: Box<dyn IdSource>) -> Self {
        Self { clock, ids, clipboard: None, storage: None, files: None, renderer: None, net: None }
    }

    /// Adds a clipboard.
    #[must_use]
    pub fn with_clipboard(mut self, p: Box<dyn Clipboard>) -> Self {
        self.clipboard = Some(p);
        self
    }

    /// Adds storage.
    #[must_use]
    pub fn with_storage(mut self, p: Box<dyn Storage>) -> Self {
        self.storage = Some(p);
        self
    }

    /// Adds file access.
    #[must_use]
    pub fn with_files(mut self, p: Box<dyn Files>) -> Self {
        self.files = Some(p);
        self
    }

    /// Adds a renderer.
    #[must_use]
    pub fn with_renderer(mut self, p: Box<dyn Renderer>) -> Self {
        self.renderer = Some(p);
        self
    }

    /// Adds a network transport.
    #[must_use]
    pub fn with_net(mut self, p: Box<dyn Net>) -> Self {
        self.net = Some(p);
        self
    }

    /// True when the session has this port.
    pub fn has(&self, kind: PortKind) -> bool {
        match kind {
            PortKind::Clipboard => self.clipboard.is_some(),
            PortKind::Storage => self.storage.is_some(),
            PortKind::Files => self.files.is_some(),
            PortKind::Renderer => self.renderer.is_some(),
            PortKind::Net => self.net.is_some(),
            PortKind::Clock => true,
        }
    }
}

/// A port write queued by a command. Writes are flushed only after the command or batch succeeded, so a
/// rolled-back batch has no side effect (docs/commands.md §1.7).
#[derive(Debug, Clone, PartialEq)]
pub enum PortWrite {
    /// Replace the clipboard content.
    Clipboard(ClipPayload),
    /// Store bytes.
    StoragePut {
        /// Namespace.
        ns: Ns,
        /// Key.
        key: String,
        /// Bytes.
        bytes: Vec<u8>,
    },
    /// Delete a key.
    StorageDelete {
        /// Namespace.
        ns: Ns,
        /// Key.
        key: String,
    },
    /// Write a file.
    File {
        /// Target.
        file: FileRef,
        /// Content.
        bytes: Vec<u8>,
        /// Media type.
        mime: String,
    },
    /// Write a document. The tab counts as saved only when this write is confirmed (see [`SaveWrite`]).
    Save(SaveWrite),
}

/// What must happen to a tab when its document write is confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SaveDone {
    /// The tab that was saved.
    pub tab: TabId,
    /// The revision of the document that was written.
    pub revision: u64,
    /// Where it was written.
    pub target: FileRef,
    /// Make `target` the save target and a recent file.
    pub remember: bool,
}

/// A document write queued by `doc.save`. Unlike a plain file write, its result decides whether the tab is
/// dirty: a failed write leaves it dirty and refuses the command, a pending write keeps it dirty until the
/// shell confirms.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveWrite {
    pub(crate) done: SaveDone,
    pub(crate) bytes: Vec<u8>,
    pub(crate) mime: String,
}

impl SaveWrite {
    pub(crate) fn new(done: SaveDone, bytes: Vec<u8>, mime: &str) -> Self {
        Self { done, bytes, mime: mime.to_owned() }
    }
}

/// What a performed write left open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Performed {
    /// Finished.
    Done,
    /// The port finishes it later.
    Pending(WriteToken),
}

impl Ports {
    /// Performs one write. A save of a document returns the raw port error; the other writes return a
    /// sentence for the warnings.
    pub(crate) fn perform(&mut self, w: &PortWrite) -> Result<Performed, String> {
        match w {
            PortWrite::Clipboard(p) => match self.clipboard.as_mut() {
                Some(c) => {
                    c.write(p.clone()).map(|()| Performed::Done).map_err(|e| format!("clipboard write failed: {e}"))
                }
                None => Err("clipboard write skipped: no clipboard port".to_owned()),
            },
            PortWrite::StoragePut { ns, key, bytes } => match self.storage.as_mut() {
                Some(s) => s
                    .put(*ns, key, bytes)
                    .map(|()| Performed::Done)
                    .map_err(|e| format!("storage write of `{key}` failed: {e}")),
                None => Err(format!("storage write of `{key}` skipped: no storage port")),
            },
            PortWrite::StorageDelete { ns, key } => match self.storage.as_mut() {
                Some(s) => s
                    .delete(*ns, key)
                    .map(|()| Performed::Done)
                    .map_err(|e| format!("storage delete of `{key}` failed: {e}")),
                None => Err(format!("storage delete of `{key}` skipped: no storage port")),
            },
            PortWrite::File { file, bytes, mime } => {
                self.write_file(file, bytes, mime).map_err(|e| format!("file write failed: {e}"))
            }
            PortWrite::Save(save) => match &save.done.target {
                FileRef::Stored { stored } => match self.storage.as_mut() {
                    Some(s) => {
                        s.put(Ns::Documents, stored, &save.bytes).map(|()| Performed::Done).map_err(|e| e.to_string())
                    }
                    None => Err("there is no storage port".to_owned()),
                },
                file => self.write_file(file, &save.bytes, &save.mime).map_err(|e| e.to_string()),
            },
        }
    }

    fn write_file(&mut self, file: &FileRef, bytes: &[u8], mime: &str) -> Result<Performed, PortError> {
        match self.files.as_mut() {
            Some(f) => Ok(match f.write(file, bytes, mime)? {
                WriteOutcome::Done(_) => Performed::Done,
                WriteOutcome::Pending(t) => Performed::Pending(t),
            }),
            None => Err(PortError::Unsupported("there is no files port".to_owned())),
        }
    }
}

/// In-memory fakes of every port, for tests and for the headless testkit.
///
/// Each fake is a cheap handle (`Clone`) onto shared state, so a test hands one clone to the session and
/// keeps another to inspect what the session did.
pub mod fake {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use super::{
        ClipKind, ClipPayload, Clipboard, Clock, FileRef, Files, Net, Ns, PortError, Ports, RenderRequest, Renderer,
        SceneView, Storage, StoredInfo, WriteOutcome,
    };
    use drafftink_collab::{LoopbackServer, Transport, loopback};
    use drafftink_scene::{Rng64, SeqIds, SplitMix64};

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A clock that only moves when the test says so.
    #[derive(Debug, Clone)]
    pub struct FakeClock {
        wall: Arc<AtomicU64>,
        mono: Arc<AtomicU64>,
    }

    impl Default for FakeClock {
        fn default() -> Self {
            Self { wall: Arc::new(AtomicU64::new(1_730_000_000_000)), mono: Arc::new(AtomicU64::new(0)) }
        }
    }

    impl FakeClock {
        /// A clock at a fixed wall time and monotonic time 0.
        pub fn new() -> Self {
            Self::default()
        }

        /// Moves both clocks forward.
        pub fn advance_ms(&self, ms: u64) {
            self.wall.fetch_add(ms, Ordering::SeqCst);
            self.mono.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.wall.load(Ordering::SeqCst)
        }
        fn mono_ms(&self) -> u64 {
            self.mono.load(Ordering::SeqCst)
        }
    }

    /// A clipboard that holds the last written payload.
    #[derive(Debug, Clone, Default)]
    pub struct FakeClipboard {
        content: Arc<Mutex<Option<ClipPayload>>>,
    }

    impl FakeClipboard {
        /// An empty clipboard.
        pub fn new() -> Self {
            Self::default()
        }

        /// What is on the clipboard now.
        pub fn content(&self) -> Option<ClipPayload> {
            lock(&self.content).clone()
        }

        /// Puts content on the clipboard as if another program had.
        pub fn set(&self, payload: ClipPayload) {
            *lock(&self.content) = Some(payload);
        }
    }

    impl Clipboard for FakeClipboard {
        fn read(&self, want: &[ClipKind]) -> Result<Option<ClipPayload>, PortError> {
            Ok(lock(&self.content).clone().filter(|p| want.is_empty() || want.contains(&p.kind())))
        }
        fn write(&mut self, payload: ClipPayload) -> Result<(), PortError> {
            *lock(&self.content) = Some(payload);
            Ok(())
        }
    }

    type StoredMap = BTreeMap<(Ns, String), (Vec<u8>, u64)>;
    type FileMap = BTreeMap<String, (Vec<u8>, String)>;

    /// Key-value storage in memory.
    #[derive(Debug, Clone, Default)]
    pub struct FakeStorage {
        data: Arc<Mutex<StoredMap>>,
        clock: FakeClock,
    }

    impl FakeStorage {
        /// Empty storage stamped by `clock`.
        pub fn new(clock: FakeClock) -> Self {
            Self { data: Arc::default(), clock }
        }

        /// The bytes under a key, for assertions.
        pub fn bytes(&self, ns: Ns, key: &str) -> Option<Vec<u8>> {
            lock(&self.data).get(&(ns, key.to_owned())).map(|(b, _)| b.clone())
        }

        /// Number of entries in all namespaces.
        pub fn len(&self) -> usize {
            lock(&self.data).len()
        }

        /// True when nothing is stored.
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    impl Storage for FakeStorage {
        fn get(&self, ns: Ns, key: &str) -> Result<Option<Vec<u8>>, PortError> {
            Ok(self.bytes(ns, key))
        }
        fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError> {
            lock(&self.data).insert((ns, key.to_owned()), (bytes.to_vec(), self.clock.now_ms()));
            Ok(())
        }
        fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError> {
            lock(&self.data).remove(&(ns, key.to_owned()));
            Ok(())
        }
        fn list(&self, ns: Ns) -> Result<Vec<StoredInfo>, PortError> {
            Ok(lock(&self.data)
                .iter()
                .filter(|((n, _), _)| *n == ns)
                .map(|((_, k), (b, t))| StoredInfo { name: k.clone(), size: b.len() as u64, modified_ms: *t })
                .collect())
        }
    }

    /// Files in memory. A download is recorded like any other write.
    #[derive(Debug, Clone, Default)]
    pub struct FakeFiles {
        data: Arc<Mutex<FileMap>>,
        fail_writes: Arc<AtomicBool>,
    }

    fn file_key(f: &FileRef) -> String {
        match f {
            FileRef::Path { path } => format!("path:{path}"),
            FileRef::Stored { stored } => format!("stored:{stored}"),
            FileRef::Download { download } => format!("download:{download}"),
            FileRef::Handle { handle } => format!("handle:{handle}"),
        }
    }

    impl FakeFiles {
        /// No files.
        pub fn new() -> Self {
            Self::default()
        }

        /// Puts a file there as if it already existed.
        pub fn insert(&self, file: &FileRef, bytes: &[u8], mime: &str) {
            lock(&self.data).insert(file_key(file), (bytes.to_vec(), mime.to_owned()));
        }

        /// Makes every later write fail (`true`) or work again (`false`), for tests of a full disk.
        pub fn fail_writes(&self, on: bool) {
            self.fail_writes.store(on, Ordering::SeqCst);
        }

        /// The bytes of a file, for assertions.
        pub fn bytes(&self, file: &FileRef) -> Option<Vec<u8>> {
            lock(&self.data).get(&file_key(file)).map(|(b, _)| b.clone())
        }

        /// Number of files.
        pub fn len(&self) -> usize {
            lock(&self.data).len()
        }

        /// True when there are none.
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    impl Files for FakeFiles {
        fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError> {
            self.bytes(file).ok_or_else(|| PortError::NotFound(file_key(file)))
        }
        fn exists(&self, file: &FileRef) -> bool {
            lock(&self.data).contains_key(&file_key(file))
        }
        fn write(&mut self, file: &FileRef, bytes: &[u8], mime: &str) -> Result<WriteOutcome, PortError> {
            if self.fail_writes.load(Ordering::SeqCst) {
                return Err(PortError::Failed("the disk is full".to_owned()));
            }
            self.insert(file, bytes, mime);
            Ok(WriteOutcome::Done(file.clone()))
        }
    }

    /// A renderer that returns a tiny deterministic byte string instead of a picture.
    #[derive(Debug, Clone, Default)]
    pub struct FakeRenderer {
        calls: Arc<AtomicU64>,
    }

    impl FakeRenderer {
        /// A renderer that has not been called.
        pub fn new() -> Self {
            Self::default()
        }

        /// How many renders were requested.
        pub fn calls(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Renderer for FakeRenderer {
        fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut out = b"\x89PNG-fake".to_vec();
            out.extend_from_slice(
                format!(" {}x{} elements={}", req.size_px.0, req.size_px.1, scene.elements.len()).as_bytes(),
            );
            Ok(out)
        }
    }

    /// A network whose connections are in-memory loopbacks. Each [`Net::open_transport`] makes a new pair and
    /// keeps the server end, which a test (or a [`crate::cmd::collab::fake_server::FakeServer`]) drives.
    #[derive(Clone)]
    pub struct FakeNet {
        state: Arc<Mutex<NetState>>,
    }

    struct NetState {
        servers: Vec<Arc<LoopbackServer>>,
        rng: SplitMix64,
        origin: Option<String>,
        fail_open: bool,
    }

    impl std::fmt::Debug for FakeNet {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FakeNet").field("connections", &lock(&self.state).servers.len()).finish()
        }
    }

    /// Makes every `FakeNet` of a process hand out different ids, so two sessions in one test are two clients.
    static NEXT_SEED: AtomicU64 = AtomicU64::new(1);

    impl Default for FakeNet {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeNet {
        /// A network with no connection yet.
        pub fn new() -> Self {
            let seed = NEXT_SEED.fetch_add(1, Ordering::SeqCst);
            Self {
                state: Arc::new(Mutex::new(NetState {
                    servers: Vec::new(),
                    rng: SplitMix64::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                    origin: None,
                    fail_open: false,
                })),
            }
        }

        /// The server ends of every connection opened so far, oldest first.
        pub fn servers(&self) -> Vec<Arc<LoopbackServer>> {
            lock(&self.state).servers.clone()
        }

        /// The server end of the newest connection.
        pub fn last_server(&self) -> Option<Arc<LoopbackServer>> {
            lock(&self.state).servers.last().cloned()
        }

        /// Makes `same_origin_url("/ws")` answer as a web shell would (`ws://page.example`).
        pub fn set_origin(&self, origin: &str) {
            lock(&self.state).origin = Some(origin.to_owned());
        }

        /// Makes the next `open_transport` calls fail, as a shell without sockets would.
        pub fn set_fail_open(&self, on: bool) {
            lock(&self.state).fail_open = on;
        }
    }

    impl Net for FakeNet {
        fn open_transport(&mut self) -> Result<Box<dyn Transport>, PortError> {
            let mut st = lock(&self.state);
            if st.fail_open {
                return Err(PortError::Unsupported("the fake network has no sockets".to_owned()));
            }
            let (transport, server) = loopback();
            server.set_auto_accept(true);
            st.servers.push(Arc::new(server));
            Ok(Box::new(transport))
        }

        fn random(&mut self) -> u128 {
            let mut st = lock(&self.state);
            (u128::from(st.rng.next_u64()) << 64) | u128::from(st.rng.next_u64())
        }

        fn same_origin_url(&self, path: &str) -> Option<String> {
            lock(&self.state).origin.as_ref().map(|o| format!("{o}{path}"))
        }
    }

    /// Handles onto one fake of every port. [`Fakes::ports`] builds the [`Ports`] for a session; the
    /// fields stay usable for assertions.
    #[derive(Debug, Clone)]
    pub struct Fakes {
        /// The shared clock.
        pub clock: FakeClock,
        /// The clipboard.
        pub clipboard: FakeClipboard,
        /// Storage.
        pub storage: FakeStorage,
        /// Files.
        pub files: FakeFiles,
        /// The renderer.
        pub renderer: FakeRenderer,
        /// The transport.
        pub net: FakeNet,
    }

    impl Default for Fakes {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Fakes {
        /// A fresh set of fakes.
        pub fn new() -> Self {
            let clock = FakeClock::new();
            Self {
                storage: FakeStorage::new(clock.clone()),
                clock,
                clipboard: FakeClipboard::new(),
                files: FakeFiles::new(),
                renderer: FakeRenderer::new(),
                net: FakeNet::new(),
            }
        }

        /// Ports for a session: every fake, sequential ids `e1`, `e2`, ....
        pub fn ports(&self) -> Ports {
            Ports::new(Box::new(self.clock.clone()), Box::new(SeqIds::new()))
                .with_clipboard(Box::new(self.clipboard.clone()))
                .with_storage(Box::new(self.storage.clone()))
                .with_files(Box::new(self.files.clone()))
                .with_renderer(Box::new(self.renderer.clone()))
                .with_net(Box::new(self.net.clone()))
        }

        /// Ports with only the clock and ids, as a headless session without a GPU or a browser has.
        pub fn headless_ports(&self) -> Ports {
            Ports::new(Box::new(self.clock.clone()), Box::new(SeqIds::new()))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn storage_round_trip_and_list() {
            let fakes = Fakes::new();
            let mut ports = fakes.ports();
            let Some(st) = ports.storage.as_mut() else { panic!("storage") };
            st.put(Ns::Documents, "b", b"22").unwrap();
            st.put(Ns::Documents, "a", b"1").unwrap();
            st.put(Ns::Autosave, "x", b"").unwrap();
            let names: Vec<String> = st.list(Ns::Documents).unwrap().into_iter().map(|i| i.name).collect();
            assert_eq!(names, ["a", "b"]);
            assert_eq!(fakes.storage.bytes(Ns::Documents, "b"), Some(b"22".to_vec()));
            st.delete(Ns::Documents, "a").unwrap();
            assert_eq!(st.get(Ns::Documents, "a").unwrap(), None);
        }

        #[test]
        fn clipboard_filters_by_kind() {
            let fakes = Fakes::new();
            fakes.clipboard.set(ClipPayload::Text("hi".into()));
            let mut ports = fakes.ports();
            let Some(c) = ports.clipboard.as_mut() else { panic!("clipboard") };
            assert_eq!(c.read(&[ClipKind::Image]).unwrap(), None);
            assert_eq!(c.read(&[ClipKind::Text]).unwrap(), Some(ClipPayload::Text("hi".into())));
        }

        #[test]
        fn clock_moves_only_when_told() {
            let fakes = Fakes::new();
            let t0 = fakes.clock.now_ms();
            assert_eq!(fakes.clock.now_ms(), t0);
            fakes.clock.advance_ms(250);
            assert_eq!(fakes.clock.now_ms(), t0 + 250);
            assert_eq!(fakes.clock.mono_ms(), 250);
        }
    }
}
