//! The functions of the Worker (not of a room): the checks it makes before it calls a Durable Object.

use drafftink_protocol::v2::{ErrorCode, ServerMessage};
use drafftink_relay::ms_until_utc_midnight;
use wasm_bindgen::prelude::*;

use super::{js_error, ms, num};
use crate::config;
use crate::edge::{self, EdgeError};

/// The result of checking a request against its room.
#[wasm_bindgen(js_name = MemberCheck)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberCheck {
    /// The room id is valid and the token belongs to the room.
    Member = 0,
    /// The room id in the address is not valid.
    BadRoom = 1,
    /// The `Authorization` header is missing or malformed.
    NoToken = 2,
    /// The token belongs to another room.
    WrongRoom = 3,
}

/// Checks the room id of the address and the `Authorization` header (`Drafft-Member <token>`). Hashes 32 bytes.
#[wasm_bindgen(js_name = checkMember)]
pub fn check_member(room: &str, authorization: Option<String>) -> MemberCheck {
    match edge::check_member(room, authorization.as_deref()) {
        Ok(_) => MemberCheck::Member,
        Err(EdgeError::BadRoom) => MemberCheck::BadRoom,
        Err(EdgeError::NoToken | EdgeError::BadName) => MemberCheck::NoToken,
        Err(EdgeError::WrongRoom) => MemberCheck::WrongRoom,
    }
}

/// Whether `text` is a room id of a path.
#[wasm_bindgen(js_name = isRoomId)]
pub fn is_room_id(text: &str) -> bool {
    edge::parse_room(text).is_ok()
}

/// Whether `text` is a picture name of a path.
#[wasm_bindgen(js_name = isImageName)]
pub fn is_image_name(text: &str) -> bool {
    edge::parse_name(text).is_ok()
}

/// The id of the room that the base64url membership token belongs to, as it appears in paths.
#[wasm_bindgen(js_name = roomIdOfToken)]
pub fn room_id_of_token(token: &str) -> Option<String> {
    edge::room_id_of_token(token)
}

/// Checks the variables of the Worker. Throws a plain-words error that names the variable.
#[wasm_bindgen(js_name = checkSettings)]
pub fn check_settings(settings_json: &str) -> Result<(), JsError> {
    config::relay_config(settings_json).map(drop).map_err(|e| js_error(&e))
}

/// The largest sealed picture that the Worker lets through to a room, in bytes.
#[wasm_bindgen(js_name = sealedImageLimit)]
pub fn sealed_image_limit(settings_json: &str) -> Result<f64, JsError> {
    config::sealed_image_limit(settings_json).map(num).map_err(|e| js_error(&e))
}

/// The time of the next 00:00 UTC after `now_ms`, in milliseconds since the epoch.
#[wasm_bindgen(js_name = nextUtcMidnightMs)]
pub fn next_utc_midnight_ms(now_ms: f64) -> f64 {
    let now = ms(now_ms);
    num(now.saturating_add(u64::from(ms_until_utc_midnight(now))))
}

/// The `Error` frame of a connection that the Worker refuses because the daily limit is used up.
#[wasm_bindgen(js_name = dailyLimitFrame)]
pub fn daily_limit_frame(now_ms: f64) -> Result<Vec<u8>, JsError> {
    let text = "The free demo server has reached its daily limit and resets at 00:00 UTC. Your drawing is saved on this device. Try again later.";
    let mut message = ServerMessage::error(ErrorCode::DailyLimit, text);
    if let ServerMessage::Error { retry_after_ms, .. } = &mut message {
        *retry_after_ms = Some(ms_until_utc_midnight(ms(now_ms)));
    }
    message.encode().map_err(|e| js_error(&e))
}

/// The frame that answers a plaintext `Join` on `/ws`: "This server only hosts end-to-end encrypted rooms."
#[wasm_bindgen(js_name = encryptedOnlyFrame)]
pub fn encrypted_only_frame() -> Result<Vec<u8>, JsError> {
    edge::encrypted_only_frame().map_err(|e| js_error(&e))
}

/// The R2 key prefix of a room (`r/<room hex>/`), or `undefined` for a bad room id.
#[wasm_bindgen(js_name = r2Prefix)]
pub fn r2_prefix(room: &str) -> Option<String> {
    edge::r2_prefix(room).ok()
}

/// The R2 key of a picture (`r/<room hex>/<name hex>`), or `undefined` for a bad room id or name.
#[wasm_bindgen(js_name = r2Key)]
pub fn r2_key(room: &str, name: &str) -> Option<String> {
    edge::r2_key(room, name).ok()
}
