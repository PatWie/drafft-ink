//! The image routes: `PUT` and `GET` of one sealed image and the list of a room's images. The membership token
//! travels in the `Authorization` header (`Drafft-Member <base64url token>`), never in the address.

use std::str::FromStr;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use base64ct::{Base64UrlUnpadded, Encoding};
use drafftink_protocol::MAX_SEALED_IMAGE_BYTES;
use drafftink_protocol::v2::{ImageName, MEMBERSHIP_TOKEN_BYTES, MembershipToken, Proof, RoomId};
use drafftink_relay::{ImageBody, ImageResponse, ImageStatus};
use serde_json::json;
use tokio::sync::oneshot;

use super::actor::{Cmd, ImageJob, OwnedImageOp};
use super::{SealedHost, Unavailable};
use crate::http::AppState;

/// The scheme word in the `Authorization` header.
const AUTH_SCHEME: &str = "Drafft-Member";

/// The routes, to be merged into the router of the server.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    // A sealed image is at most `MAX_SEALED_IMAGE_BYTES`; one byte more is read so that it is refused with 413.
    let limit = DefaultBodyLimit::max(MAX_SEALED_IMAGE_BYTES + 1);
    Router::new()
        .route("/r/{room}/f", get(list))
        .route("/r/{room}/f/{name}", put(put_image).layer(limit).get(get_image))
}

fn failure(status: ImageStatus, message: &str, retry_after_ms: Option<u32>) -> Response {
    let code = StatusCode::from_u16(status.http()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = json!({ "error": status.code(), "message": message });
    let mut response = (code, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response();
    if let Some(ms) = retry_after_ms
        && let Ok(value) = HeaderValue::from_str(&ms.div_ceil(1000).max(1).to_string())
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

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
            "The free demo server has reached its daily limit and resets at 00:00 UTC. Your pictures are saved on this device. Try again later."
        }
        ImageStatus::Created | ImageStatus::Ok => "",
    }
}

fn render(response: ImageResponse) -> Response {
    match (response.status, response.body) {
        (ImageStatus::Ok, ImageBody::Sealed(bytes)) => {
            ([(header::CONTENT_TYPE, "application/octet-stream"), (header::CACHE_CONTROL, "no-store")], bytes)
                .into_response()
        }
        (ImageStatus::Ok, ImageBody::List(list)) => {
            let items: Vec<_> = list.iter().map(|i| json!({ "name": i.name.to_string(), "size": i.size })).collect();
            ([(header::CONTENT_TYPE, "application/json")], json!(items).to_string()).into_response()
        }
        (ImageStatus::Created, ImageBody::Totals(totals)) => {
            let body = json!({ "room_images": totals.count, "room_image_bytes": totals.bytes });
            (StatusCode::CREATED, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
        }
        (status, _) => failure(status, message_for(status), response.retry_after_ms),
    }
}

/// What the path and headers name, checked before any room is loaded.
struct Target {
    room: RoomId,
    proof: Proof,
}

fn target(room: &str, headers: &HeaderMap) -> Result<Target, ImageStatus> {
    let room = RoomId::from_str(room).map_err(|_| ImageStatus::RoomMissing)?;
    let proof = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix(AUTH_SCHEME))
        .and_then(|v| decode_token(v.trim()))
        .ok_or(ImageStatus::Unauthorized)?;
    if room.accepts(&proof) { Ok(Target { room, proof }) } else { Err(ImageStatus::Unauthorized) }
}

async fn run(host: &Arc<SealedHost>, target: Target, op: OwnedImageOp) -> Response {
    let handle = match host.room(target.room).await {
        Ok(handle) => handle,
        Err(Unavailable::TooManyRooms | Unavailable::Storage(_)) => {
            return failure(ImageStatus::Unavailable, message_for(ImageStatus::Unavailable), Some(5000));
        }
    };
    let (reply, answer) = oneshot::channel();
    if handle.tx.send(Cmd::Image { job: ImageJob { proof: target.proof, op }, reply }).await.is_err() {
        return failure(ImageStatus::Unavailable, message_for(ImageStatus::Unavailable), Some(1000));
    }
    match answer.await {
        Ok(response) => render(response),
        Err(_) => failure(ImageStatus::Unavailable, message_for(ImageStatus::Unavailable), Some(1000)),
    }
}

fn image_name(name: &str) -> Result<ImageName, ImageStatus> {
    ImageName::from_str(name).map_err(|_| ImageStatus::BadRequest)
}

/// A failure that needs no more than its status.
fn refuse(status: ImageStatus) -> Response {
    failure(status, message_for(status), None)
}

async fn put_image(
    State(state): State<Arc<AppState>>,
    Path((room, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let target = match target(&room, &headers) {
        Ok(target) => target,
        Err(status) => return refuse(status),
    };
    match image_name(&name) {
        Ok(name) => run(&state.sealed, target, OwnedImageOp::Put { name, body: body.to_vec() }).await,
        Err(status) => refuse(status),
    }
}

async fn get_image(
    State(state): State<Arc<AppState>>,
    Path((room, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let target = match target(&room, &headers) {
        Ok(target) => target,
        Err(status) => return refuse(status),
    };
    match image_name(&name) {
        Ok(name) => run(&state.sealed, target, OwnedImageOp::Get { name }).await,
        Err(status) => refuse(status),
    }
}

async fn list(State(state): State<Arc<AppState>>, Path(room): Path<String>, headers: HeaderMap) -> Response {
    match target(&room, &headers) {
        Ok(target) => run(&state.sealed, target, OwnedImageOp::List).await,
        Err(status) => refuse(status),
    }
}

/// The membership token in an `Authorization` header: 32 bytes as unpadded base64url.
fn decode_token(text: &str) -> Option<Proof> {
    let mut bytes = [0u8; MEMBERSHIP_TOKEN_BYTES];
    let decoded = Base64UrlUnpadded::decode(text, &mut bytes).ok()?;
    (decoded.len() == MEMBERSHIP_TOKEN_BYTES).then(|| Proof::token(&MembershipToken::from_bytes(bytes)))
}
