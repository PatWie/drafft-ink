//! A relay of end-to-end encrypted rooms for tests: the real relay core ([`drafftink_relay::Relay`]) with memory
//! stores, joined to the loopback ends of several sessions. Like [`super::fake_server::FakeServer`] it is driven by
//! calling [`FakeRelay::step`] between the polls of the sessions it serves, with the time of the test.

use std::sync::Arc;

use drafftink_collab::{BlobOp, BlobReply, BlobRequest, LoopbackBlobServer, LoopbackServer};
use drafftink_protocol::v2::{ImageName, Proof, RoomId};
use drafftink_relay::{
    BlobStore, ConnId, Effect, ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus, MemoryBlobStore,
    MemoryRelayStore, Output, Relay, RelayConfig,
};

use crate::ports::fake::FakeNet;

/// One loopback end and the connection of the core that belongs to it.
struct Link {
    end: Arc<LoopbackServer>,
    /// How many opens of the end were seen: a reconnect is a new connection on the same end.
    opens: usize,
    conn: Option<ConnId>,
}

/// The relay of one room. The room is the one of the first connection that is attached.
pub struct FakeRelay {
    config: RelayConfig,
    relay: Option<Relay<MemoryRelayStore, MemoryBlobStore>>,
    room: Option<RoomId>,
    store: MemoryRelayStore,
    blobs: MemoryBlobStore,
    links: Vec<Link>,
    /// The picture requests of every session, as the HTTP routes of the real host would take them.
    blob_links: Vec<Arc<LoopbackBlobServer>>,
    /// Requests wait unanswered while this is set (a slow network).
    hold_blobs: bool,
    /// Uploads wait while this is set: the manifest of a picture reaches the others before its bytes do.
    hold_puts: bool,
    held_puts: Vec<(Arc<LoopbackBlobServer>, BlobRequest)>,
    /// How many picture requests were answered, for a test of an idle session.
    blob_requests: usize,
    next_conn: u64,
    wake_at: Option<u64>,
}

impl FakeRelay {
    /// A relay with this policy and no room yet.
    pub fn new(config: RelayConfig) -> Self {
        Self {
            config,
            relay: None,
            room: None,
            store: MemoryRelayStore::new(),
            blobs: MemoryBlobStore::new(),
            links: Vec::new(),
            blob_links: Vec::new(),
            hold_blobs: false,
            hold_puts: false,
            held_puts: Vec::new(),
            blob_requests: 0,
            next_conn: 0,
            wake_at: None,
        }
    }

    /// The sealed pictures the relay stores, by name.
    pub fn sealed_pictures(&self) -> Vec<(ImageName, Vec<u8>)> {
        let mut blobs = self.blobs.clone();
        let names = blobs.list().unwrap_or_default();
        names.into_iter().filter_map(|i| blobs.get(&i.name).ok().flatten().map(|b| (i.name, b))).collect()
    }

    /// Holds every picture request unanswered (or serves them again).
    pub fn hold_blobs(&mut self, on: bool) {
        self.hold_blobs = on;
    }

    /// Keeps uploads waiting (reads are answered): a picture is announced before it is there. Releasing serves them.
    pub fn hold_puts(&mut self, on: bool) {
        self.hold_puts = on;
    }

    /// How many picture requests the relay has answered.
    pub fn blob_requests(&self) -> usize {
        self.blob_requests
    }

    /// Flips one bit of every stored picture, as a hostile or failing store would.
    pub fn damage_pictures(&mut self) {
        let mut blobs = self.blobs.clone();
        for (name, mut bytes) in self.sealed_pictures() {
            if let Some(last) = bytes.last_mut() {
                *last ^= 1;
            }
            let _ = blobs.put(&name, &bytes);
        }
    }

    /// What the relay stored, for a test that looks for what must not be there.
    pub fn store(&self) -> &MemoryRelayStore {
        &self.store
    }

    /// How many people are in the room.
    pub fn members(&self) -> usize {
        self.relay.as_ref().map_or(0, Relay::member_count)
    }

    /// Serves the connections `net` opened since the last call.
    pub fn attach(&mut self, net: &FakeNet) {
        for end in net.servers() {
            if !self.links.iter().any(|l| Arc::ptr_eq(&l.end, &end)) {
                self.links.push(Link { end, opens: 0, conn: None });
            }
        }
        for end in net.blob_servers() {
            if !self.blob_links.iter().any(|l| Arc::ptr_eq(l, &end)) {
                self.blob_links.push(end);
            }
        }
    }

    /// The id of the room in `/r/<id>/ws`.
    fn room_of(url: &str) -> Option<RoomId> {
        url.split("/r/").nth(1)?.split('/').next()?.parse().ok()
    }

    /// Handles what the clients sent and the time that passed. Returns true when it did anything.
    pub fn step(&mut self, now_ms: u64) -> bool {
        let mut busy = false;
        for i in 0..self.links.len() {
            busy |= self.step_link(i, now_ms);
        }
        busy |= self.serve_blobs(now_ms);
        if let (Some(relay), Some(at)) = (self.relay.as_mut(), self.wake_at)
            && at <= now_ms
        {
            let out = relay.on_wake(now_ms);
            busy = true;
            self.apply(out);
        }
        busy
    }

    fn step_link(&mut self, i: usize, now_ms: u64) -> bool {
        let Some(link) = self.links.get(i) else { return false };
        let (end, opens) = (Arc::clone(&link.end), link.opens);
        let mut busy = false;
        if end.opens() != opens {
            // A new connection on this end: the old one is gone.
            busy = true;
            self.drop_conn(i, now_ms);
            if self.relay.is_none()
                && let Some(room) = end.last_url().as_deref().and_then(Self::room_of)
            {
                self.room = Some(room);
                self.relay = Some(Relay::new(room, self.config.clone(), self.store.clone(), self.blobs.clone()));
            }
            self.next_conn += 1;
            let conn = ConnId(self.next_conn);
            if let Some(link) = self.links.get_mut(i) {
                link.opens = end.opens();
                link.conn = Some(conn);
            }
            if let Some(relay) = self.relay.as_mut() {
                let out = relay.connect(conn, now_ms);
                self.apply(out);
            }
        }
        let conn = self.links.get(i).and_then(|l| l.conn);
        if conn.is_some() && !end.is_open() {
            busy = true;
            self.drop_conn(i, now_ms);
            return busy;
        }
        let Some(conn) = conn else { return busy };
        while let Some(frame) = end.recv_raw() {
            busy = true;
            if let Some(relay) = self.relay.as_mut() {
                let out = relay.on_frame(conn, now_ms, &frame);
                self.apply(out);
            }
        }
        for text in end.take_texts() {
            busy = true;
            if let Some(relay) = self.relay.as_mut() {
                let out = relay.on_text(conn, now_ms, &text);
                self.apply(out);
            }
        }
        busy
    }

    /// Answers the picture requests like the HTTP routes of a host: `PUT`, `GET` and `HEAD` of
    /// `/r/<room>/f/<name>` and `GET` of `/r/<room>/f`, with the membership token in `Authorization`.
    fn serve_blobs(&mut self, now_ms: u64) -> bool {
        if self.hold_blobs {
            return false;
        }
        let mut busy = false;
        let mut due: Vec<(Arc<LoopbackBlobServer>, BlobRequest)> = Vec::new();
        if !self.hold_puts {
            due.append(&mut self.held_puts);
        }
        for end in &self.blob_links {
            due.extend(end.take_requests().into_iter().map(|request| (Arc::clone(end), request)));
        }
        for (end, request) in due {
            if self.hold_puts && matches!(request.op, BlobOp::Put(_)) {
                self.held_puts.push((end, request));
                continue;
            }
            busy = true;
            self.blob_requests += 1;
            let reply = self.blob_reply(now_ms, &request);
            end.respond(request.id, Ok(reply));
        }
        busy
    }

    fn blob_reply(&mut self, now_ms: u64, request: &BlobRequest) -> BlobReply {
        let status = |status: u16| BlobReply { status, ..BlobReply::default() };
        let (Some(relay), Some(room)) = (self.relay.as_mut(), self.room.as_ref()) else { return status(404) };
        let Some((_, path)) = request.url.split_once(&format!("/r/{room}/f")) else { return status(404) };
        let Some(token) = request.auth.token() else { return status(401) };
        let proof = Proof::token(&token);
        let head = matches!(request.op, BlobOp::Head);
        let name = path.strip_prefix('/').map(str::parse::<ImageName>);
        let op = match (&request.op, name) {
            (BlobOp::Put(body), Some(Ok(name))) => ImageOp::Put { name, body },
            (BlobOp::Get | BlobOp::Head, Some(Ok(name))) => ImageOp::Get { name },
            (BlobOp::List, None) => ImageOp::List,
            _ => return status(400),
        };
        let (response, out) = relay.image_request(now_ms, ImageRequest { proof: &proof, op });
        self.apply(out);
        http_reply(response, head)
    }

    fn drop_conn(&mut self, i: usize, now_ms: u64) {
        let Some(conn) = self.links.get_mut(i).and_then(|l| l.conn.take()) else { return };
        if let Some(relay) = self.relay.as_mut() {
            let out = relay.on_closed(conn, now_ms);
            self.apply(out);
        }
    }

    fn apply(&mut self, out: Output) {
        self.wake_at = out.wake_at;
        for effect in out.effects {
            match effect {
                Effect::Send { conn, frame } => {
                    if let Some(end) = self.end_of(conn) {
                        end.push_raw(frame.to_vec());
                    }
                }
                Effect::Text { conn, text } => {
                    if let Some(end) = self.end_of(conn) {
                        end.push_text(text);
                    }
                }
                Effect::Close { conn } => {
                    if let Some(end) = self.end_of(conn) {
                        end.close_cleanly();
                    }
                    if let Some(link) = self.links.iter_mut().find(|l| l.conn == Some(conn)) {
                        link.conn = None;
                    }
                }
            }
        }
    }

    fn end_of(&self, conn: ConnId) -> Option<Arc<LoopbackServer>> {
        self.links.iter().find(|l| l.conn == Some(conn)).map(|l| Arc::clone(&l.end))
    }
}

/// The HTTP form of an answer: the status, the sealed bytes or JSON, and `Retry-After` in whole seconds.
fn http_reply(response: ImageResponse, head: bool) -> BlobReply {
    let retry_after_ms = response.retry_after_ms.map(|ms| u64::from(ms).div_ceil(1000).max(1) * 1000);
    let json = |v: serde_json::Value| v.to_string().into_bytes();
    let body = match (response.status, response.body) {
        (ImageStatus::Ok, ImageBody::Sealed(bytes)) => bytes,
        (ImageStatus::Ok, ImageBody::List(list)) => {
            json(list.iter().map(|i| serde_json::json!({ "name": i.name.to_string(), "size": i.size })).collect())
        }
        (ImageStatus::Created, ImageBody::Totals(t)) => {
            json(serde_json::json!({ "room_images": t.count, "room_image_bytes": t.bytes }))
        }
        (status, _) => json(serde_json::json!({ "error": status.code() })),
    };
    let content_length = u64::try_from(body.len()).ok();
    BlobReply {
        status: response.status.http(),
        body: if head { Vec::new() } else { body },
        content_length,
        retry_after_ms,
    }
}
