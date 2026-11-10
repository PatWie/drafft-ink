//! The WebSocket plumbing that the server and the relay share: the handshake with its `Host` and `Origin`
//! checks, the size limits, and reading one text frame at a time.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::{WebSocketConfig, frame::coding::CloseCode};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async_with_config};

use crate::guard;

/// A WebSocket configuration with both size limits set to `max`.
pub fn config(max: usize) -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(max)).max_frame_size(Some(max))
}

/// Accepts a WebSocket on `stream` for the bridge listening on `port`. The upgrade is answered `403` when the
/// `Host` header is not this loopback port or the `Origin` header is not allowed.
pub async fn accept(
    stream: TcpStream,
    port: u16,
    allowed_origins: Arc<Vec<String>>,
    max_frame: usize,
) -> Result<WebSocketStream<TcpStream>, tungstenite::Error> {
    // The error type is fixed by the library's `Callback` trait.
    #[allow(clippy::result_large_err)]
    let check = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok());
        let verdict = guard::check_host(header("host"), port).and_then(|()| guard::check_origin(header("origin"), &allowed_origins));
        match verdict {
            Ok(()) => Ok(resp),
            Err(why) => {
                log::warn!("bridge: refused a connection: {why}");
                let mut r = ErrorResponse::new(Some(format!("drafftink bridge: {why}\n")));
                *r.status_mut() = StatusCode::FORBIDDEN;
                Err(r)
            }
        }
    };
    accept_hdr_async_with_config(stream, check, Some(config(max_frame))).await
}

/// Why reading a frame stopped.
#[derive(Debug)]
pub enum RecvError {
    /// The peer sent a binary frame. The bridge speaks text only.
    Binary,
    /// The frame was too large or the stream broke.
    Ws(tungstenite::Error),
}

impl RecvError {
    /// The close code to answer with.
    pub fn close_code(&self) -> CloseCode {
        match self {
            Self::Binary => CloseCode::Unsupported,
            Self::Ws(tungstenite::Error::Capacity(_)) => CloseCode::Size,
            Self::Ws(_) => CloseCode::Error,
        }
    }
}

/// The next text frame; `None` when the peer closed. Pings and pongs are answered by the library.
pub async fn recv_text<S>(ws: &mut WebSocketStream<S>) -> Result<Option<String>, RecvError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match ws.next().await {
            None => return Ok(None),
            Some(Err(e)) => return Err(RecvError::Ws(e)),
            Some(Ok(Message::Text(t))) => return Ok(Some(t.as_str().to_owned())),
            Some(Ok(Message::Close(_))) => return Ok(None),
            Some(Ok(Message::Binary(_))) => return Err(RecvError::Binary),
            Some(Ok(_)) => {}
        }
    }
}
