//! Clients of an encrypted room for tests against a real server: the real sealed client over real sockets, with a tap on
//! every websocket frame and on every HTTP exchange of the picture transport.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock.
#![allow(clippy::disallowed_methods)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use drafftink_collab::{
    BlobAuth, BlobOp, BlobRequest, BlobResponse, BlobTransport, BlobTransportError, CollabEvent, CollabSession,
    ImageEvent, NativeBlobTransport, NativeTransport, Resume, SealedClient, SealedConfig, ServerUrl, Transport,
    TransportError, TransportEvent, UrlPolicy,
};
use drafftink_doc::{FilePolicy, LoroConfig, LoroStore};
use drafftink_protocol::v2::JoinMode;
use drafftink_protocol::{ClientId, UserInfo};
use drafftink_scene::{FileEntry, FileId, SceneStore, SceneTx};
use drafftink_seal::RoomKey;

/// How long a test waits for something it knows is coming.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// The name every test client gives itself, so that tests can look for it.
pub const USER: &str = "Zelda-Canary";

/// One HTTP exchange of the picture transport.
#[derive(Debug, Clone, Default)]
pub struct Exchange {
    /// The client that made it.
    pub who: u8,
    /// `PUT`, `GET` or `HEAD`.
    pub method: &'static str,
    /// The full address.
    pub url: String,
    /// The `Authorization` header as sent.
    pub auth: String,
    /// The request body.
    pub request: Vec<u8>,
    /// The status, or `None` when the exchange failed or has no answer yet.
    pub status: Option<u16>,
    /// The response body.
    pub response: Vec<u8>,
    /// The failure text, when the answer was an error.
    pub error: Option<String>,
}

impl Exchange {
    /// The last path segment: the keyed name, or `f` for the list.
    pub fn name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or_default()
    }
}

/// What crossed the wire, shared by every client of a test.
#[derive(Clone, Default)]
pub struct Taps {
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
    http: Arc<Mutex<Vec<Exchange>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Taps {
    /// Every websocket frame, both directions.
    pub fn frames(&self) -> Vec<Vec<u8>> {
        lock(&self.frames).clone()
    }

    /// Every picture request with its answer.
    pub fn exchanges(&self) -> Vec<Exchange> {
        lock(&self.http).clone()
    }

    /// The requests of one method, optionally of one client.
    pub fn count(&self, method: &str, who: Option<u8>) -> usize {
        lock(&self.http).iter().filter(|e| e.method == method && who.is_none_or(|w| w == e.who)).count()
    }

    /// Adds a body that crossed the wire by another route (a hand-made request).
    pub fn push_frame(&self, bytes: Vec<u8>) {
        lock(&self.frames).push(bytes);
    }
}

/// A websocket transport that keeps every frame that crosses it, in both directions.
pub struct Recording {
    inner: NativeTransport,
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Transport for Recording {
    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError> {
        self.inner.open(url)
    }
    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        lock(&self.frames).push(frame.clone());
        self.inner.send(frame)
    }
    fn send_text(&mut self, text: &str) -> Result<(), TransportError> {
        self.inner.send_text(text)
    }
    fn close(&mut self) {
        self.inner.close();
    }
    fn poll(&mut self) -> Option<TransportEvent> {
        let event = self.inner.poll()?;
        if let TransportEvent::Message(bytes) = &event {
            lock(&self.frames).push(bytes.clone());
        }
        Some(event)
    }
}

/// How a client's picture transport misbehaves.
#[derive(Clone, Default)]
pub struct BlobOptions {
    /// Sends this header instead of the client's own (a stranger's token).
    pub auth_override: Option<BlobAuth>,
    /// The first `PUT` reaches the server, and its answer is lost on the way back.
    pub lose_first_put_answer: bool,
}

/// The real picture transport with a tap on every request and answer.
pub struct RecordingBlobs {
    who: u8,
    inner: NativeBlobTransport,
    http: Arc<Mutex<Vec<Exchange>>>,
    pending: HashMap<u64, usize>,
    lose: HashSet<u64>,
    options: BlobOptions,
    puts: u32,
}

impl BlobTransport for RecordingBlobs {
    fn send(&mut self, mut request: BlobRequest) -> Result<(), BlobTransportError> {
        if let Some(auth) = &self.options.auth_override {
            request.auth = auth.clone();
        }
        let is_put = matches!(request.op, BlobOp::Put(_));
        let exchange = Exchange {
            who: self.who,
            method: request.op.method(),
            url: request.url.clone(),
            auth: request.auth.header_value().to_owned(),
            request: if let BlobOp::Put(body) = &request.op { body.clone() } else { Vec::new() },
            ..Exchange::default()
        };
        let index = {
            let mut http = lock(&self.http);
            http.push(exchange);
            http.len() - 1
        };
        self.pending.insert(request.id, index);
        if is_put {
            self.puts += 1;
            if self.options.lose_first_put_answer && self.puts == 1 {
                self.lose.insert(request.id);
            }
        }
        self.inner.send(request)
    }

    fn poll(&mut self) -> Option<BlobResponse> {
        let response = self.inner.poll()?;
        if let Some(index) = self.pending.remove(&response.id)
            && let Some(exchange) = lock(&self.http).get_mut(index)
        {
            match &response.result {
                Ok(reply) => {
                    exchange.status = Some(reply.status);
                    exchange.response = reply.body.clone();
                }
                Err(text) => exchange.error = Some(text.clone()),
            }
        }
        if self.lose.remove(&response.id) {
            return Some(BlobResponse { id: response.id, result: Err("the answer got lost".to_owned()) });
        }
        Some(response)
    }
}

/// The clock of a test: milliseconds since it started, one for all clients.
#[derive(Clone, Copy)]
pub struct Clock(Instant);

impl Clock {
    pub fn new() -> Self {
        Clock(Instant::now())
    }

    pub fn now(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap()
    }
}

/// A sealed client with its store.
pub struct Peer {
    pub n: u8,
    pub session: CollabSession<Recording>,
    pub store: LoroStore,
    /// Every picture event so far, in order.
    pub images: Vec<ImageEvent>,
}

impl Peer {
    /// Puts a picture into the store as a manifest only (the bytes stay in the view), as a room does.
    pub fn put_picture(&mut self, clock: Clock, mime: &str, bytes: &[u8]) -> (FileId, Arc<[u8]>) {
        let (id, entry) = FileEntry::new(mime, bytes.to_vec());
        let shared = Arc::clone(&entry.bytes);
        let mut tx = self.store.begin();
        tx.put_file(entry).unwrap();
        tx.commit().unwrap();
        self.session.client_mut().local_changed(clock.now());
        (id, shared)
    }

    pub fn arrived(&self, id: &FileId) -> Option<Arc<[u8]>> {
        self.images.iter().find_map(|e| match e {
            ImageEvent::Arrived { id: got, bytes } if got == id => Some(Arc::clone(bytes)),
            _ => None,
        })
    }

    pub fn damaged(&self, id: &FileId) -> bool {
        self.images.iter().any(|e| matches!(e, ImageEvent::Damaged { id: got } if got == id))
    }

    pub fn rejected(&self, id: &FileId) -> Option<drafftink_collab::ImageRefusal> {
        self.images.iter().find_map(|e| match e {
            ImageEvent::Rejected { id: got, why } if got == id => Some(*why),
            _ => None,
        })
    }

    pub fn pump(&mut self, clock: Clock) {
        let out = self.session.pump(clock.now(), &mut self.store);
        for event in out.events {
            if let CollabEvent::Image(image) = event {
                self.images.push(image);
            }
        }
    }

    pub fn offer(&mut self, clock: Clock, id: &FileId, bytes: &Arc<[u8]>) {
        self.session.client_mut().offer_image(clock.now(), id, Arc::clone(bytes));
    }

    pub fn want(&mut self, clock: Clock, id: &FileId) {
        self.session.client_mut().want_image(clock.now(), id);
    }

    pub fn online(&self) -> bool {
        self.session.client().status().state == drafftink_collab::ConnState::Online
    }

    /// Online and nothing waits to be sent.
    pub fn settled(&self) -> bool {
        self.online() && !self.session.client().status().unsynced
    }
}

/// A client of `url` that has `key`, named [`USER`], with its picture transport.
pub fn peer(n: u8, key: &RoomKey, url: &str, mode: JoinMode, taps: &Taps, options: BlobOptions) -> Peer {
    let cfg = SealedConfig {
        url: ServerUrl::parse(url, UrlPolicy::default()).unwrap(),
        key: RoomKey::from_bytes(*key.expose_bytes()),
        client_id: ClientId::from_bytes([n; 16]),
        user: Some(UserInfo { name: USER.to_owned(), color: 0x33_66_99 }),
        mode,
        resume: Resume::default(),
        jitter_seed: u64::from(n),
        replace_when_trimmed: false,
    };
    let transport = Recording { inner: NativeTransport::new(Arc::new(|| {})), frames: Arc::clone(&taps.frames) };
    let mut client = SealedClient::new(cfg).unwrap();
    client.start(0);
    let mut store =
        LoroStore::with_config(LoroConfig { peer_id: Some(u64::from(n) + 1000), ..Default::default() }).unwrap();
    store.set_file_policy(FilePolicy::External);
    let mut session = CollabSession::new(client, transport);
    session.set_blobs(Box::new(RecordingBlobs {
        who: n,
        inner: NativeBlobTransport::new(Arc::new(|| {})),
        http: Arc::clone(&taps.http),
        pending: HashMap::new(),
        lose: HashSet::new(),
        options,
        puts: 0,
    }));
    Peer { n, session, store, images: Vec::new() }
}

/// Drives the peers until `done` holds. A bound ends a hung test.
pub fn pump_until(clock: Clock, peers: &mut [&mut Peer], what: &str, done: impl Fn(&[&mut Peer]) -> bool) {
    let started = Instant::now();
    loop {
        for p in peers.iter_mut() {
            p.pump(clock);
        }
        if done(peers) {
            return;
        }
        assert!(started.elapsed() < PATIENCE, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Like [`pump_until`], for a test that runs on the tokio runtime next to the server. Pumping never blocks.
pub async fn drive(clock: Clock, peers: &mut [&mut Peer], what: &str, done: impl Fn(&[&mut Peer]) -> bool) {
    let started = Instant::now();
    loop {
        for p in peers.iter_mut() {
            p.pump(clock);
        }
        if done(peers) {
            return;
        }
        assert!(started.elapsed() < PATIENCE, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Like [`pump_for`], on the tokio runtime.
pub async fn drive_for(clock: Clock, peers: &mut [&mut Peer], ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        for p in peers.iter_mut() {
            p.pump(clock);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Drives the peers for `ms` of real time.
pub fn pump_for(clock: Clock, peers: &mut [&mut Peer], ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        for p in peers.iter_mut() {
            p.pump(clock);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A picture of `len` bytes that starts like a PNG, holds `marker`, and differs from every other `seed`.
pub fn picture(marker: &str, seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(marker.as_bytes());
    let mut x = u32::from(seed).wrapping_mul(2_654_435_761).wrapping_add(88_172_645);
    while bytes.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        bytes.push((x >> 8) as u8);
    }
    bytes.truncate(len);
    bytes
}

/// The 32 raw bytes of a file id.
pub fn raw_id(id: &FileId) -> [u8; 32] {
    let mut raw = [0u8; 32];
    for (out, pair) in raw.iter_mut().zip(id.as_str().as_bytes().chunks(2)) {
        *out = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    raw
}
