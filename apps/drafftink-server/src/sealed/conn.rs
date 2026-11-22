//! One WebSocket connection of a sealed room. The first frame is checked here, before any room is loaded: a
//! connection that does not hold the room's membership token never costs a room task.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use drafftink_protocol::DecodeError;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, RoomId, ServerMessage};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio::time::timeout;
use tracing::debug;

use super::actor::{Cmd, OUTBOUND_QUEUE, Outbound, RoomHandle};
use super::{SealedHost, Unavailable};

/// Longest a single socket write may take before the peer counts as dead.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);

async fn writer(mut sink: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Outbound>) {
    while let Some(next) = rx.recv().await {
        let frame = match next {
            Outbound::Binary(frame) => Message::Binary(frame.to_vec().into()),
            Outbound::Text(text) => Message::Text(text.into()),
            Outbound::Close => break,
        };
        if !matches!(timeout(WRITE_TIMEOUT, sink.send(frame)).await, Ok(Ok(()))) {
            return;
        }
    }
    let _ = timeout(Duration::from_secs(2), sink.send(Message::Close(None))).await;
    let _ = timeout(Duration::from_secs(2), sink.close()).await;
}

/// A refusal before the connection has a room: send the error and close.
async fn refuse(out: &mpsc::Sender<Outbound>, code: ErrorCode, message: &str) {
    if let Ok(frame) = ServerMessage::error(code, message).encode() {
        let _ = out.send(Outbound::Binary(frame.into())).await;
    }
    let _ = out.send(Outbound::Close).await;
}

fn decode_refusal(e: &DecodeError) -> (ErrorCode, &'static str) {
    match e {
        DecodeError::UnsupportedVersion { .. } | DecodeError::WrongLayout { .. } => (
            ErrorCode::UnsupportedVersion,
            "This server only hosts end-to-end encrypted rooms (protocol version 2). Update the app.",
        ),
        DecodeError::FrameTooLarge { .. } | DecodeError::FieldTooLong { .. } => {
            (ErrorCode::TooLarge, "the message is larger than this relay accepts")
        }
        _ => (ErrorCode::Malformed, "the message could not be read"),
    }
}

/// Reads the first frame and decides whether the connection may reach a room. `Ok` carries the frame to pass on.
async fn first_frame(
    stream: &mut SplitStream<WebSocket>,
    room: &RoomId,
    join_timeout: Duration,
) -> Result<Vec<u8>, (ErrorCode, &'static str)> {
    let message = match timeout(join_timeout, stream.next()).await {
        Err(_) => return Err((ErrorCode::NotJoined, "no Join arrived in time")),
        Ok(None | Some(Err(_))) => return Err((ErrorCode::NotJoined, "the connection closed before Join")),
        Ok(Some(Ok(message))) => message,
    };
    let Message::Binary(bytes) = message else {
        return Err((ErrorCode::NotJoined, "the first message must be a binary Join frame"));
    };
    match ClientMessage::decode(&bytes) {
        Ok(ClientMessage::Join { room: named, proof, .. }) => {
            if named == *room && named.accepts(&proof) {
                Ok(bytes.to_vec())
            } else {
                Err((ErrorCode::Unauthorized, "The room link is not valid for this room."))
            }
        }
        Ok(_) => Err((ErrorCode::NotJoined, "the first message must be Join")),
        Err(e) => Err(decode_refusal(&e)),
    }
}

/// Runs a connection until it ends.
pub(crate) async fn serve(socket: WebSocket, host: Arc<SealedHost>, room: RoomId, _permit: OwnedSemaphorePermit) {
    let (sink, mut stream) = socket.split();
    let (out, rx) = mpsc::channel(OUTBOUND_QUEUE);
    let mut writer_task = tokio::spawn(writer(sink, rx));
    let timing = &host.config.timing;
    let first = match first_frame(&mut stream, &room, timing.join_timeout).await {
        Ok(first) => first,
        Err((code, text)) => {
            refuse(&out, code, text).await;
            let _ = timeout(Duration::from_secs(3), &mut writer_task).await;
            return;
        }
    };
    let handle: RoomHandle = match host.room(room).await {
        Ok(handle) => handle,
        Err(why) => {
            let text = match why {
                Unavailable::TooManyRooms => "This server has as many rooms open as it can. Trying again in a moment.",
                Unavailable::Storage(text) => text,
            };
            refuse(&out, ErrorCode::Unavailable, text).await;
            let _ = timeout(Duration::from_secs(3), &mut writer_task).await;
            return;
        }
    };
    let conn = host.next_conn_id();
    let idle = timing.idle_timeout;
    let started = handle.tx.send(Cmd::Connect { conn, out: out.clone() }).await.is_ok()
        && handle.tx.send(Cmd::Frame { conn, bytes: first }).await.is_ok();
    drop(out);
    if started {
        pump(&mut stream, &handle, conn, idle, &mut writer_task).await;
    }
    let _ = timeout(Duration::from_secs(5), handle.tx.send(Cmd::Closed { conn })).await;
    if timeout(Duration::from_secs(3), &mut writer_task).await.is_err() {
        writer_task.abort();
    }
}

/// Forwards frames to the room until the socket ends, goes quiet or the writer stops.
async fn pump(
    stream: &mut SplitStream<WebSocket>,
    handle: &RoomHandle,
    conn: drafftink_relay::ConnId,
    idle: Duration,
    writer_task: &mut tokio::task::JoinHandle<()>,
) {
    loop {
        let message = tokio::select! {
            biased;
            _ = &mut *writer_task => return,
            next = timeout(idle, stream.next()) => match next {
                Err(_) => {
                    debug!("closing an idle sealed connection");
                    return;
                }
                Ok(None | Some(Err(_))) => return,
                Ok(Some(Ok(message))) => message,
            },
        };
        let cmd = match message {
            Message::Binary(bytes) => Cmd::Frame { conn, bytes: bytes.to_vec() },
            Message::Text(text) => Cmd::Text { conn, text: text.as_str().to_owned() },
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => return,
        };
        if handle.tx.send(cmd).await.is_err() {
            return;
        }
    }
}
