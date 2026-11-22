//! `RoomCore`: the relay of one room as a JavaScript class. Every method is synchronous and takes the time from
//! the caller; the shell is the only thing that has a clock, a socket or a database.

use drafftink_protocol::ClientId;
use drafftink_protocol::v2::RoomId;
use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;

use super::hosts::{BlobHostJs, JsBlobs, JsSql, SqlHostJs};
use super::{js_error, ms, num};
use crate::config;
use crate::driver::{Driver, Effects, ImageCall, ImageReply, Item};
use crate::edge;
use crate::host::{HostBlobs, HostStore};

/// What the shell does with one entry of [`JsEffects`].
#[wasm_bindgen(js_name = EffectKind)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    /// Send binary frame `frame(i)` to `conn(i)`.
    Send = 0,
    /// Send the text `text(i)` to `conn(i)`.
    Text = 1,
    /// Close `conn(i)`.
    Close = 2,
}

/// What a call of the core asks the shell to do, in order. Read it with `count`, `kind`, `conn`, `frame`, `text`.
#[wasm_bindgen(js_name = Effects)]
pub struct JsEffects(Effects);

#[wasm_bindgen(js_class = Effects)]
impl JsEffects {
    /// How many things to do.
    pub fn count(&self) -> usize {
        self.0.items().len()
    }

    /// What entry `index` is; `undefined` past the end.
    pub fn kind(&self, index: usize) -> Option<EffectKind> {
        self.0.items().get(index).map(|item| match item {
            Item::Send { .. } => EffectKind::Send,
            Item::Text { .. } => EffectKind::Text,
            Item::Close { .. } => EffectKind::Close,
        })
    }

    /// The connection of entry `index`.
    pub fn conn(&self, index: usize) -> u32 {
        match self.0.items().get(index) {
            Some(Item::Send { conn, .. } | Item::Text { conn, .. } | Item::Close { conn }) => *conn,
            None => 0,
        }
    }

    /// The frame to send for entry `index`. Entries with the same `frameId` carry the same bytes, so the shell
    /// can ask once and send the result to each of them.
    pub fn frame(&self, index: usize) -> Uint8Array {
        match self.0.items().get(index) {
            Some(Item::Send { frame, .. }) => {
                self.0.frame(*frame).map_or_else(|| Uint8Array::new_with_length(0), Uint8Array::from)
            }
            _ => Uint8Array::new_with_length(0),
        }
    }

    /// Which frame entry `index` sends: equal numbers are the same bytes, so the shell can reuse them.
    #[wasm_bindgen(js_name = frameId)]
    pub fn frame_id(&self, index: usize) -> i32 {
        match self.0.items().get(index) {
            Some(Item::Send { frame, .. }) => i32::try_from(*frame).unwrap_or(-1),
            _ => -1,
        }
    }

    /// The text of entry `index`.
    pub fn text(&self, index: usize) -> String {
        match self.0.items().get(index) {
            Some(Item::Text { text, .. }) => (*text).to_owned(),
            _ => String::new(),
        }
    }

    /// When to call `onWake`, in milliseconds since the epoch.
    #[wasm_bindgen(getter, js_name = wakeAt)]
    pub fn wake_at(&self) -> Option<f64> {
        self.0.wake_at().map(num)
    }

    /// The room was deleted: wipe its storage and forget it.
    #[wasm_bindgen(getter)]
    pub fn deleted(&self) -> bool {
        self.0.deleted()
    }

    /// A store call failed because the daily limit of the host is used up.
    #[wasm_bindgen(getter, js_name = quotaHit)]
    pub fn quota_hit(&self) -> bool {
        self.0.quota_hit()
    }
}

/// The answer to a picture request.
#[wasm_bindgen(js_name = ImageReply)]
pub struct JsImageReply {
    reply: ImageReply,
    effects: Effects,
}

#[wasm_bindgen(js_class = ImageReply)]
impl JsImageReply {
    /// The HTTP status.
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> u16 {
        self.reply.status
    }

    /// The stable word of the result (`created`, `unauthorized`, `daily_limit`, ...).
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> String {
        self.reply.code.to_owned()
    }

    /// For a rate limit and the daily limit: when to try again, in milliseconds.
    #[wasm_bindgen(getter, js_name = retryAfterMs)]
    pub fn retry_after_ms(&self) -> Option<u32> {
        self.reply.retry_after_ms
    }

    /// `application/json` or `application/octet-stream`.
    #[wasm_bindgen(getter, js_name = contentType)]
    pub fn content_type(&self) -> String {
        self.reply.content_type.to_owned()
    }

    /// The body.
    pub fn body(&self) -> Uint8Array {
        Uint8Array::from(&self.reply.body[..])
    }

    /// A store call failed because the daily limit of the host is used up.
    #[wasm_bindgen(getter, js_name = quotaHit)]
    pub fn quota_hit(&self) -> bool {
        self.effects.quota_hit()
    }

    /// When to call `onWake`, in milliseconds since the epoch.
    #[wasm_bindgen(getter, js_name = wakeAt)]
    pub fn wake_at(&self) -> Option<f64> {
        self.effects.wake_at().map(num)
    }
}

/// What a picture request asks for.
#[wasm_bindgen(js_name = ImageCall)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsImageCall {
    /// List the room's pictures.
    List = 0,
    /// Read one picture.
    Get = 1,
    /// Store one picture.
    Put = 2,
}

impl From<JsImageCall> for ImageCall {
    fn from(call: JsImageCall) -> Self {
        match call {
            JsImageCall::List => ImageCall::List,
            JsImageCall::Get => ImageCall::Get,
            JsImageCall::Put => ImageCall::Put,
        }
    }
}

/// The relay of one room.
#[wasm_bindgen(js_name = RoomCore)]
pub struct RoomCore {
    driver: Driver<HostStore<JsSql>, HostBlobs<JsBlobs>>,
}

fn client_id(bytes: &[u8]) -> Result<ClientId, JsError> {
    let array: [u8; 16] = bytes.try_into().map_err(|_| JsError::new("a client id has 16 bytes"))?;
    Ok(ClientId::from_bytes(array))
}

#[wasm_bindgen(js_class = RoomCore)]
impl RoomCore {
    /// The core of room `room` (base64url), with the variables of the Worker as JSON and the shell's stores.
    #[wasm_bindgen(constructor)]
    pub fn new(room: &str, settings_json: &str, sql: SqlHostJs, blobs: BlobHostJs) -> Result<RoomCore, JsError> {
        let room: RoomId = edge::parse_room(room).map_err(|e| js_error(&e))?;
        let cfg = config::relay_config(settings_json).map_err(|e| js_error(&e))?;
        let driver = Driver::new(room, cfg, HostStore::new(JsSql::new(sql)), HostBlobs::new(JsBlobs::new(blobs)));
        Ok(RoomCore { driver })
    }

    /// A socket opened.
    pub fn connect(&mut self, conn: u32, now_ms: f64) -> JsEffects {
        JsEffects(self.driver.connect(conn, ms(now_ms)))
    }

    /// A binary frame arrived. Hostile bytes are answered with an error frame and a close, never an exception.
    #[wasm_bindgen(js_name = onFrame)]
    pub fn on_frame(&mut self, conn: u32, now_ms: f64, frame: &[u8]) -> JsEffects {
        JsEffects(self.driver.on_frame(conn, ms(now_ms), frame))
    }

    /// A text frame arrived.
    #[wasm_bindgen(js_name = onText)]
    pub fn on_text(&mut self, conn: u32, now_ms: f64, text: &str) -> JsEffects {
        JsEffects(self.driver.on_text(conn, ms(now_ms), text))
    }

    /// A socket closed.
    #[wasm_bindgen(js_name = onClosed)]
    pub fn on_closed(&mut self, conn: u32, now_ms: f64) -> JsEffects {
        JsEffects(self.driver.on_closed(conn, ms(now_ms)))
    }

    /// The time of an earlier `wakeAt` has come, or the object woke up.
    #[wasm_bindgen(js_name = onWake)]
    pub fn on_wake(&mut self, now_ms: f64) -> JsEffects {
        JsEffects(self.driver.on_wake(ms(now_ms)))
    }

    /// The runtime answered a `ping` of the socket at `at_ms` (it never reaches the core).
    pub fn heard(&mut self, conn: u32, at_ms: f64) {
        self.driver.heard(conn, ms(at_ms));
    }

    /// A joined socket survived a hibernation.
    #[wasm_bindgen(js_name = restoreMember)]
    pub fn restore_member(
        &mut self,
        conn: u32,
        client: &[u8],
        order: f64,
        last_heard_ms: f64,
        now_ms: f64,
    ) -> Result<JsEffects, JsError> {
        let client = client_id(client)?;
        Ok(JsEffects(self.driver.restore_member(conn, client, ms(order), ms(last_heard_ms), ms(now_ms))))
    }

    /// A socket that had not joined yet survived a hibernation.
    #[wasm_bindgen(js_name = restoreWaiting)]
    pub fn restore_waiting(&mut self, conn: u32, connected_at_ms: f64, now_ms: f64) -> JsEffects {
        JsEffects(self.driver.restore_waiting(conn, ms(connected_at_ms), ms(now_ms)))
    }

    /// The client id (16 bytes) of a joined socket, for its attachment.
    #[wasm_bindgen(js_name = memberClient)]
    pub fn member_client(&self, conn: u32) -> Option<Vec<u8>> {
        self.driver.member_of(conn).map(|(client, _)| client.as_bytes().to_vec())
    }

    /// The join order of a joined socket, for its attachment.
    #[wasm_bindgen(js_name = memberOrder)]
    pub fn member_order(&self, conn: u32) -> Option<f64> {
        self.driver.member_of(conn).map(|(_, order)| num(order))
    }

    /// Answers a picture request. `token` is the base64url token of the `Authorization` header, `name` the path
    /// segment. `body` is read only for `Put`.
    pub fn image(
        &mut self,
        now_ms: f64,
        call: JsImageCall,
        token: &str,
        name: Option<String>,
        body: &[u8],
    ) -> JsImageReply {
        let (reply, effects) = self.driver.image(ms(now_ms), call.into(), token, name.as_deref(), body);
        JsImageReply { reply, effects }
    }
}
