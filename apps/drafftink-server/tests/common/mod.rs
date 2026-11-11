//! Shared test support: a protocol client that owns a Loro document, server start helpers and a
//! tiny HTTP client.
#![allow(dead_code)]

pub mod mock_s3;

use std::net::SocketAddr;
use std::time::Duration;

use drafftink_protocol::{ClientId, ClientMessage, RoomId, ServerMessage, SyncKind, UserInfo};
use drafftink_server::config::{Config, Timing};
use drafftink_server::store::StorageConfig;
use futures_util::{SinkExt, StreamExt};
use loro::{ExportMode, LoroDoc, LoroValue, VersionVector};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub const WAIT: Duration = Duration::from_secs(5);

/// A config for tests: loopback, ephemeral port, memory store, store written before every Ack.
pub fn test_config() -> Config {
    Config {
        bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        web_dir: std::env::temp_dir().join("drafftink-no-such-web-dir"),
        storage: StorageConfig::Memory,
        timing: Timing { flush_interval: Duration::ZERO, ..Timing::default() },
        ..Config::default()
    }
}

pub fn room(name: &str) -> RoomId {
    RoomId::new(name).expect("valid room id")
}

pub fn client_id(n: u8) -> ClientId {
    ClientId::from_bytes([n; 16])
}

/// What a client sees on its socket.
#[derive(Debug)]
pub enum Event {
    Msg(ServerMessage),
    /// The socket closed (clean close, reset or end of stream).
    Closed,
    /// Nothing arrived in time.
    Timeout,
}

pub struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pub doc: LoroDoc,
    pub id: ClientId,
    pub room: RoomId,
    exported: VersionVector,
    seq: u32,
    pub json: bool,
}

pub fn put(doc: &LoroDoc, key: &str, value: LoroValue) {
    doc.get_map("elements").insert(key, value).expect("insert");
    doc.commit();
}

pub fn state(doc: &LoroDoc) -> LoroValue {
    doc.get_deep_value()
}

impl Client {
    pub async fn connect(addr: SocketAddr, n: u8, room_name: &str) -> Client {
        Self::try_connect(addr, None, n, room_name).await.expect("connect")
    }

    pub async fn try_connect(
        addr: SocketAddr,
        origin: Option<&str>,
        n: u8,
        room_name: &str,
    ) -> Result<Client, tokio_tungstenite::tungstenite::Error> {
        let mut req = format!("ws://{addr}/ws").into_client_request()?;
        if let Some(o) = origin {
            req.headers_mut().insert("Origin", o.parse().expect("header value"));
        }
        let (ws, _) = tokio_tungstenite::connect_async(req).await?;
        Ok(Client {
            ws,
            doc: LoroDoc::new(),
            id: client_id(n),
            room: room(room_name),
            exported: VersionVector::default(),
            seq: 0,
            json: false,
        })
    }

    /// A client whose socket has a tiny receive buffer, so a reader that stops reading makes
    /// the server's writes block after a few kilobytes instead of after megabytes.
    pub async fn connect_small_buffer(addr: SocketAddr, n: u8, room_name: &str) -> Client {
        let socket = tokio::net::TcpSocket::new_v4().expect("socket");
        socket.set_recv_buffer_size(4096).expect("rcvbuf");
        let stream = socket.connect(addr).await.expect("connect");
        let req = format!("ws://{addr}/ws").into_client_request().expect("request");
        let (ws, _) = tokio_tungstenite::client_async(req, MaybeTlsStream::Plain(stream)).await.expect("handshake");
        Client {
            ws,
            doc: LoroDoc::new(),
            id: client_id(n),
            room: room(room_name),
            exported: VersionVector::default(),
            seq: 0,
            json: false,
        }
    }

    pub async fn send(&mut self, msg: &ClientMessage) {
        let frame = if self.json {
            Message::Text(msg.to_json().expect("json").into())
        } else {
            Message::Binary(msg.encode().expect("encode").into())
        };
        self.ws.send(frame).await.expect("send");
    }

    pub async fn send_raw(&mut self, bytes: Vec<u8>) {
        // Errors are fine here: the server may have closed already.
        let _ = self.ws.send(Message::Binary(bytes.into())).await;
    }

    pub async fn send_text(&mut self, text: &str) {
        let _ = self.ws.send(Message::Text(text.to_owned().into())).await;
    }

    pub async fn next_within(&mut self, wait: Duration) -> Event {
        loop {
            match tokio::time::timeout(wait, self.ws.next()).await {
                Err(_) => return Event::Timeout,
                Ok(None) | Ok(Some(Err(_))) => return Event::Closed,
                Ok(Some(Ok(Message::Close(_)))) => return Event::Closed,
                Ok(Some(Ok(Message::Binary(b)))) => {
                    return Event::Msg(ServerMessage::decode(&b).expect("server frames decode"));
                }
                Ok(Some(Ok(Message::Text(t)))) => {
                    return Event::Msg(ServerMessage::from_json(&t).expect("server json decodes"));
                }
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    /// The next message; fails the test on close or timeout.
    pub async fn recv(&mut self) -> ServerMessage {
        match self.next_within(WAIT).await {
            Event::Msg(m) => m,
            other => panic!("expected a message, got {other:?}"),
        }
    }

    /// Reads messages until `pick` returns a value; other messages are skipped.
    pub async fn recv_until<T>(&mut self, mut pick: impl FnMut(&ServerMessage) -> Option<T>) -> T {
        loop {
            let m = self.recv().await;
            if let Some(v) = pick(&m) {
                return v;
            }
        }
    }

    /// Reads until the server closes; returns the error codes seen on the way.
    pub async fn expect_closed(&mut self) -> Vec<drafftink_protocol::ErrorCode> {
        let mut codes = Vec::new();
        loop {
            match self.next_within(WAIT).await {
                Event::Msg(ServerMessage::Error { code, .. }) => codes.push(code),
                Event::Msg(_) => {}
                Event::Closed => return codes,
                Event::Timeout => panic!("the server did not close the connection (errors so far: {codes:?})"),
            }
        }
    }

    pub fn join_message(&self, token: Option<&str>) -> ClientMessage {
        ClientMessage::Join {
            min_version: 1,
            max_version: 1,
            room: self.room.clone(),
            client_id: self.id,
            auth_token: token.map(str::to_owned),
            version: self.doc.oplog_vv().encode(),
            user: Some(UserInfo { name: format!("user{}", self.id.as_bytes()[0]), color: 0x336699 }),
        }
    }

    /// Joins and imports the first `SyncResponse`. Returns the `Joined` message and the sync.
    pub async fn join(&mut self) -> (ServerMessage, ServerMessage) {
        self.join_with(None).await
    }

    pub async fn join_with(&mut self, token: Option<&str>) -> (ServerMessage, ServerMessage) {
        let join = self.join_message(token);
        self.send(&join).await;
        let joined = self.recv().await;
        assert!(matches!(joined, ServerMessage::Joined { .. }), "expected Joined, got {joined:?}");
        let sync = self.recv().await;
        self.apply_sync(&sync);
        (joined, sync)
    }

    pub fn apply_sync(&mut self, sync: &ServerMessage) {
        let ServerMessage::SyncResponse { data, .. } = sync else { panic!("expected SyncResponse, got {sync:?}") };
        if !data.is_empty() {
            self.doc.import(data).expect("server data imports");
        }
    }

    /// Sends our own changes the server lacks, as the protocol describes after `Joined`.
    pub async fn upload_since(&mut self, server_version: &[u8]) -> Option<u32> {
        let vv = VersionVector::decode(server_version).expect("server version decodes");
        if vv.includes_vv(&self.doc.oplog_vv()) {
            return None;
        }
        let bytes = self.doc.export(ExportMode::updates(&vv)).expect("export");
        self.exported = self.doc.oplog_vv();
        self.seq += 1;
        let seq = self.seq;
        self.send(&ClientMessage::Update { seq, update: bytes }).await;
        Some(seq)
    }

    /// Edits the document and sends the update. Returns its `seq`.
    pub async fn edit(&mut self, key: &str, value: LoroValue) -> u32 {
        put(&self.doc, key, value);
        let bytes = self.doc.export(ExportMode::updates(&self.exported)).expect("export");
        self.exported = self.doc.oplog_vv();
        self.seq += 1;
        let seq = self.seq;
        self.send(&ClientMessage::Update { seq, update: bytes }).await;
        seq
    }

    /// Writes `meta.<key>` (the document's own bookkeeping, not an element) and sends the update. Returns its `seq`.
    pub async fn edit_meta(&mut self, key: &str, value: LoroValue) -> u32 {
        self.doc.get_map("meta").insert(key, value).expect("insert");
        self.doc.commit();
        let bytes = self.doc.export(ExportMode::updates(&self.exported)).expect("export");
        self.exported = self.doc.oplog_vv();
        self.seq += 1;
        let seq = self.seq;
        self.send(&ClientMessage::Update { seq, update: bytes }).await;
        seq
    }

    /// Edits and waits for the `Ack`.
    pub async fn edit_acked(&mut self, key: &str, value: LoroValue) {
        let seq = self.edit(key, value).await;
        self.recv_until(|m| matches!(m, ServerMessage::Ack { seq: s } if *s == seq).then_some(())).await;
    }

    /// Makes local changes without sending them (offline editing).
    pub fn edit_offline(&mut self, key: &str, value: LoroValue) {
        put(&self.doc, key, value);
    }

    /// Imports relayed updates until the document equals `want`.
    pub async fn converge_to(&mut self, want: &LoroValue) {
        for _ in 0..200 {
            if state(&self.doc) == *want {
                return;
            }
            match self.recv().await {
                ServerMessage::Update { update, .. } => {
                    self.doc.import(&update).expect("relayed update imports");
                }
                ServerMessage::SyncResponse { data, .. } => {
                    self.doc.import(&data).expect("sync imports");
                }
                _ => {}
            }
        }
        panic!("document did not converge:\n have {:?}\n want {want:?}", state(&self.doc));
    }

    pub fn close_now(self) {
        drop(self);
    }

    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

pub fn text(s: &str) -> LoroValue {
    LoroValue::from(s)
}

pub fn sync_kind(sync: &ServerMessage) -> SyncKind {
    match sync {
        ServerMessage::SyncResponse { kind, .. } => *kind,
        other => panic!("expected SyncResponse, got {other:?}"),
    }
}

/// A small HTTP/1.1 GET with `Connection: close`. Returns status, lowercase headers and body.
pub async fn http_get(addr: SocketAddr, path: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.expect("read");
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("header end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().expect("status line").split(' ').nth(1).expect("code").parse().expect("number");
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    (status, headers, raw[split + 4..].to_vec())
}

pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Deterministic pseudo-random bytes (xorshift), so failures reproduce.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 24) as u8).collect()
    }
}
