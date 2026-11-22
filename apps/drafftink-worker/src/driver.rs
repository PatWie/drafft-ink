//! The room core as the shell calls it: connection numbers and times are plain numbers, and what the relay
//! decided comes back as [`Effects`] that the shell walks through. Everything here is ordinary Rust, so it runs in
//! native tests; `room.rs` only puts the JavaScript types on it.

use drafftink_protocol::ClientId;
use drafftink_protocol::v2::RoomId;
use drafftink_relay::{
    BlobStore, ConnId, Effect, Frame, ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus, Output, Relay,
    RelayConfig, RelayStore,
};
use serde_json::json;

use crate::edge::{EdgeError, decode_token, parse_name};

/// One thing for the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Send binary frame number `frame` of [`Effects::frame`] to `conn`.
    Send {
        /// The connection.
        conn: u32,
        /// Index into the frame table.
        frame: usize,
    },
    /// Send a text frame.
    Text {
        /// The connection.
        conn: u32,
        /// The text.
        text: &'static str,
    },
    /// Close the connection.
    Close {
        /// The connection.
        conn: u32,
    },
}

/// What a call of the core asks the shell to do. A frame that goes to many connections is stored once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Effects {
    items: Vec<Item>,
    frames: Vec<Frame>,
    wake_at: Option<u64>,
    deleted: bool,
    quota_hit: bool,
}

impl Effects {
    /// Translates the output of the core.
    pub fn from_output(output: Output) -> Self {
        let mut effects = Effects {
            wake_at: output.wake_at,
            deleted: output.deleted,
            quota_hit: output.quota_hit,
            ..Effects::default()
        };
        for effect in output.effects {
            match effect {
                Effect::Send { conn, frame } => {
                    let Some(conn) = narrow(conn) else { continue };
                    let index = effects.frame_index(frame);
                    effects.items.push(Item::Send { conn, frame: index });
                }
                Effect::Text { conn, text } => {
                    if let Some(conn) = narrow(conn) {
                        effects.items.push(Item::Text { conn, text });
                    }
                }
                Effect::Close { conn } => {
                    if let Some(conn) = narrow(conn) {
                        effects.items.push(Item::Close { conn });
                    }
                }
            }
        }
        effects
    }

    /// The index of `frame` in the table; frames that are the same allocation share an entry.
    fn frame_index(&mut self, frame: Frame) -> usize {
        match self.frames.iter().position(|known| std::sync::Arc::ptr_eq(known, &frame)) {
            Some(index) => index,
            None => {
                self.frames.push(frame);
                self.frames.len() - 1
            }
        }
    }

    /// The things to do, in order.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The bytes of a frame of the table.
    pub fn frame(&self, index: usize) -> Option<&[u8]> {
        self.frames.get(index).map(|f| &f[..])
    }

    /// When to call `on_wake`, in milliseconds since the epoch.
    pub fn wake_at(&self) -> Option<u64> {
        self.wake_at
    }

    /// The room is gone: the shell wipes its storage and forgets the room.
    pub fn deleted(&self) -> bool {
        self.deleted
    }

    /// A store call failed because the daily limit of the host is used up.
    pub fn quota_hit(&self) -> bool {
        self.quota_hit
    }
}

fn narrow(conn: ConnId) -> Option<u32> {
    u32::try_from(conn.0).ok()
}

/// The answer to an image request, ready for HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReply {
    /// The HTTP status.
    pub status: u16,
    /// The stable word of a failure (`unauthorized`, `daily_limit`, ...), or of success (`ok`, `created`).
    pub code: &'static str,
    /// For `daily_limit` and `rate_limited`: when to try again, in milliseconds.
    pub retry_after_ms: Option<u32>,
    /// `application/json` or `application/octet-stream`.
    pub content_type: &'static str,
    /// The body.
    pub body: Vec<u8>,
}

impl ImageReply {
    fn failure(status: ImageStatus, retry_after_ms: Option<u32>) -> Self {
        let body = json!({ "error": status.code(), "message": message_for(status) }).to_string().into_bytes();
        ImageReply {
            status: status.http(),
            code: status.code(),
            retry_after_ms,
            content_type: "application/json",
            body,
        }
    }

    fn from_response(response: ImageResponse) -> Self {
        let status = response.status;
        let json_reply = |value: serde_json::Value| ImageReply {
            status: status.http(),
            code: status.code(),
            retry_after_ms: None,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        };
        match (status, response.body) {
            (ImageStatus::Ok, ImageBody::Sealed(bytes)) => ImageReply {
                status: status.http(),
                code: status.code(),
                retry_after_ms: None,
                content_type: "application/octet-stream",
                body: bytes,
            },
            (ImageStatus::Ok, ImageBody::List(list)) => {
                json_reply(list.iter().map(|i| json!({ "name": i.name.to_string(), "size": i.size })).collect())
            }
            (ImageStatus::Created, ImageBody::Totals(t)) => {
                json_reply(json!({ "room_images": t.count, "room_image_bytes": t.bytes }))
            }
            (status, _) => ImageReply::failure(status, response.retry_after_ms),
        }
    }

    /// A refusal that the edge decided before the core was asked.
    pub fn refusal(error: EdgeError) -> Self {
        let status = match error {
            EdgeError::BadRoom => ImageStatus::RoomMissing,
            EdgeError::BadName => ImageStatus::BadRequest,
            EdgeError::NoToken | EdgeError::WrongRoom => ImageStatus::Unauthorized,
        };
        ImageReply::failure(status, None)
    }
}

/// Plain words for the body of a failure (the same texts as the `drafftink-headless` server).
fn message_for(status: ImageStatus) -> &'static str {
    match status {
        ImageStatus::Unauthorized => "This request does not carry the room's membership token.",
        ImageStatus::RoomMissing => "This room does not exist (any more).",
        ImageStatus::ImageMissing => "The room has no picture with this name.",
        ImageStatus::TooLarge => "This picture is larger than a shared room accepts.",
        ImageStatus::RoomFull => "This room already holds as many pictures as it may.",
        ImageStatus::RateLimited => "Too many pictures at once. Trying again in a moment.",
        ImageStatus::BadRequest => "The request is not a sealed picture with a valid name.",
        ImageStatus::Unavailable => "The server cannot store pictures right now. Trying again soon.",
        ImageStatus::DailyLimit => {
            "The free demo server has reached its daily limit and resets at 00:00 UTC. Try again later."
        }
        ImageStatus::Created | ImageStatus::Ok => "",
    }
}

/// What an image request asks for, as the shell states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageCall {
    /// `GET /r/<room>/f`.
    List,
    /// `GET` (and `HEAD`) of one picture.
    Get,
    /// `PUT` of one picture.
    Put,
}

/// The core of one room with its stores.
pub struct Driver<S, B> {
    relay: Relay<S, B>,
}

impl<S: RelayStore, B: BlobStore> Driver<S, B> {
    /// The core of `room`.
    pub fn new(room: RoomId, config: RelayConfig, store: S, blobs: B) -> Self {
        Driver { relay: Relay::new(room, config, store, blobs) }
    }

    /// The relay, for tests.
    pub fn relay(&self) -> &Relay<S, B> {
        &self.relay
    }

    /// A socket opened.
    pub fn connect(&mut self, conn: u32, now_ms: u64) -> Effects {
        Effects::from_output(self.relay.connect(ConnId(u64::from(conn)), now_ms))
    }

    /// A binary frame arrived.
    pub fn on_frame(&mut self, conn: u32, now_ms: u64, frame: &[u8]) -> Effects {
        Effects::from_output(self.relay.on_frame(ConnId(u64::from(conn)), now_ms, frame))
    }

    /// A text frame arrived.
    pub fn on_text(&mut self, conn: u32, now_ms: u64, text: &str) -> Effects {
        Effects::from_output(self.relay.on_text(ConnId(u64::from(conn)), now_ms, text))
    }

    /// A socket closed.
    pub fn on_closed(&mut self, conn: u32, now_ms: u64) -> Effects {
        Effects::from_output(self.relay.on_closed(ConnId(u64::from(conn)), now_ms))
    }

    /// The time of an earlier `wake_at` has come (or the object woke up).
    pub fn on_wake(&mut self, now_ms: u64) -> Effects {
        Effects::from_output(self.relay.on_wake(now_ms))
    }

    /// The runtime answered a `ping` of `conn` at `at_ms`.
    pub fn heard(&mut self, conn: u32, at_ms: u64) {
        self.relay.heard(ConnId(u64::from(conn)), at_ms);
    }

    /// A joined socket survived a hibernation.
    pub fn restore_member(
        &mut self,
        conn: u32,
        client: ClientId,
        order: u64,
        last_heard_ms: u64,
        now_ms: u64,
    ) -> Effects {
        Effects::from_output(self.relay.restore_member(ConnId(u64::from(conn)), client, order, last_heard_ms, now_ms))
    }

    /// A socket that had not joined yet survived a hibernation.
    pub fn restore_waiting(&mut self, conn: u32, connected_at_ms: u64, now_ms: u64) -> Effects {
        Effects::from_output(self.relay.restore_waiting(ConnId(u64::from(conn)), connected_at_ms, now_ms))
    }

    /// The client id and join order of a joined socket.
    pub fn member_of(&self, conn: u32) -> Option<(ClientId, u64)> {
        self.relay.member_of(ConnId(u64::from(conn)))
    }

    /// Answers a picture request. `token` is the base64url membership token of the `Authorization` header, `name`
    /// the path segment of the picture. The edge has checked both already, and the core checks them again.
    pub fn image(
        &mut self,
        now_ms: u64,
        call: ImageCall,
        token: &str,
        name: Option<&str>,
        body: &[u8],
    ) -> (ImageReply, Effects) {
        let Some(proof) = decode_token(token) else {
            return (ImageReply::refusal(EdgeError::NoToken), Effects::default());
        };
        let name = match (call, name.map(parse_name)) {
            (ImageCall::List, _) => None,
            (_, Some(Ok(name))) => Some(name),
            (_, Some(Err(error))) => return (ImageReply::refusal(error), Effects::default()),
            (_, None) => return (ImageReply::refusal(EdgeError::BadName), Effects::default()),
        };
        let op = match (call, name) {
            (ImageCall::Put, Some(name)) => ImageOp::Put { name, body },
            (ImageCall::Get, Some(name)) => ImageOp::Get { name },
            _ => ImageOp::List,
        };
        let (response, output) = self.relay.image_request(now_ms, ImageRequest { proof: &proof, op });
        (ImageReply::from_response(response), Effects::from_output(output))
    }
}
