//! A client for the bridge server (native only). The MCP app uses it in attached mode; tests use it as the
//! agent. It refuses any URL that is not loopback, so a typo or a hostile config cannot send the token
//! across a network.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

use crate::rpc::{self, Incoming, RpcError};
use crate::ws::{self, RecvError};

/// Largest frame the client accepts (a command list with schemas is a few MiB).
pub const MAX_REPLY_BYTES: usize = 64 << 20;

/// Why the client could not talk to the bridge.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The URL is not `ws://127.0.0.1:<port>` or `ws://localhost:<port>`.
    #[error("`{0}` is not a loopback URL. The bridge only listens on 127.0.0.1, so use ws://127.0.0.1:<port>/")]
    NotLoopback(String),
    /// The connection failed.
    #[error("could not connect to the bridge: {0}")]
    Connect(String),
    /// The server refused or the call failed.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// The connection closed.
    #[error("the bridge closed the connection")]
    Closed,
    /// The server sent something that is not the protocol.
    #[error("unexpected message from the bridge: {0}")]
    Protocol(String),
    /// No answer in time.
    #[error("no answer from the bridge within {0} s")]
    Timeout(u64),
}

/// True for `ws://127.0.0.1[:port][/..]`, `ws://localhost[:port][/..]` and `ws://[::1][:port][/..]`.
pub fn is_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("ws://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = if authority.starts_with('[') {
        authority.split_once(']').map_or("", |(h, _)| h.get(1..).unwrap_or(""))
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
}

/// Options for [`BridgeClient::connect_with`].
#[derive(Debug, Clone)]
pub struct ConnectOpts {
    /// Send this `Origin` header, as a browser page would.
    pub origin: Option<String>,
    /// Largest frame accepted.
    pub max_frame_bytes: usize,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        Self { origin: None, max_frame_bytes: MAX_REPLY_BYTES }
    }
}

/// A connected, authenticated client.
pub struct BridgeClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    events: VecDeque<(String, Value)>,
    hello: Value,
}

impl BridgeClient {
    /// Connects and says `hello`.
    pub async fn connect(url: &str, token: &str) -> Result<Self, ClientError> {
        Self::connect_with(url, token, &ConnectOpts::default()).await
    }

    /// Connects with options and says `hello`.
    pub async fn connect_with(url: &str, token: &str, opts: &ConnectOpts) -> Result<Self, ClientError> {
        Self::connect_with_hello(url, opts, json!({ "token": token })).await
    }

    async fn open(url: &str, opts: &ConnectOpts) -> Result<Self, ClientError> {
        if !is_loopback_url(url) {
            return Err(ClientError::NotLoopback(url.to_owned()));
        }
        let mut req = url.into_client_request().map_err(|e| ClientError::Connect(e.to_string()))?;
        if let Some(o) = &opts.origin {
            let v = o.parse().map_err(|_| ClientError::Connect(format!("bad Origin `{o}`")))?;
            req.headers_mut().insert("origin", v);
        }
        let (ws, _) = connect_async_with_config(req, Some(ws::config(opts.max_frame_bytes)), true)
            .await
            .map_err(|e| ClientError::Connect(describe_connect_error(&e)))?;
        Ok(Self { ws, next_id: 1, events: VecDeque::new(), hello: Value::Null })
    }

    /// Like [`BridgeClient::connect_with`], with the whole `hello` parameter object (the page adds `page`).
    pub async fn connect_with_hello(url: &str, opts: &ConnectOpts, hello: Value) -> Result<Self, ClientError> {
        let mut c = Self::open(url, opts).await?;
        c.hello = c.call("hello", hello).await?;
        Ok(c)
    }

    /// The server's answer to `hello`: protocol, version, registry digest, limits.
    pub fn hello(&self) -> &Value {
        &self.hello
    }

    /// Calls a method and waits for its answer. Notifications that arrive meanwhile are kept for
    /// [`BridgeClient::take_events`].
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let id_value = json!(id);
        self.ws
            .send(Message::text(rpc::request_text(&id_value, method, &params)))
            .await
            .map_err(|_| ClientError::Closed)?;
        loop {
            let text = match ws::recv_text(&mut self.ws).await {
                Ok(Some(t)) => t,
                Ok(None) => return Err(ClientError::Closed),
                Err(RecvError::Binary) => return Err(ClientError::Protocol("a binary frame".to_owned())),
                Err(RecvError::Ws(_)) => return Err(ClientError::Closed),
            };
            match rpc::parse(&text) {
                Ok(Incoming::Response { id: rid, outcome }) if rid == id_value || rid.is_null() => {
                    return outcome.map_err(ClientError::Rpc);
                }
                Ok(Incoming::Notification { method, params }) => self.events.push_back((method, params)),
                Ok(_) => {}
                Err(m) => return Err(ClientError::Protocol(m.error.message)),
            }
        }
    }

    /// Like [`BridgeClient::call`] with a time limit. A timeout does not cancel the request.
    pub async fn call_within(&mut self, method: &str, params: Value, limit: Duration) -> Result<Value, ClientError> {
        match tokio::time::timeout(limit, self.call(method, params)).await {
            Ok(r) => r,
            Err(_) => Err(ClientError::Timeout(limit.as_secs())),
        }
    }

    /// Notifications received so far (`changed` after `subscribe`), oldest first.
    pub fn take_events(&mut self) -> Vec<(String, Value)> {
        self.events.drain(..).collect()
    }

    /// Waits for the next notification.
    pub async fn next_event(&mut self, limit: Duration) -> Option<(String, Value)> {
        if let Some(e) = self.events.pop_front() {
            return Some(e);
        }
        let waited = tokio::time::timeout(limit, async {
            loop {
                let text = ws::recv_text(&mut self.ws).await.ok().flatten()?;
                if let Ok(Incoming::Notification { method, params }) = rpc::parse(&text) {
                    return Some((method, params));
                }
            }
        })
        .await;
        waited.ok().flatten()
    }

    /// Plays the other side: answers the requests that the peer sends (the relay calling a page) until `stop`
    /// fires or the connection ends. `handle` gets each frame and returns the frame to send back, if any.
    pub async fn serve(&mut self, mut handle: impl FnMut(&str) -> Option<String>, mut stop: tokio::sync::oneshot::Receiver<()>) {
        loop {
            tokio::select! {
                _ = &mut stop => break,
                frame = ws::recv_text(&mut self.ws) => match frame {
                    Ok(Some(text)) => {
                        if let Some(answer) = handle(&text) && self.ws.send(Message::text(answer)).await.is_err() {
                            break;
                        }
                    }
                    _ => break,
                },
            }
        }
    }

    /// Closes the connection.
    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// A refused upgrade carries the server's reason in the body; show it.
pub fn describe_connect_error(e: &tungstenite::Error) -> String {
    match e {
        tungstenite::Error::Http(resp) => {
            let body = resp.body().as_ref().map(|b| String::from_utf8_lossy(b).trim().to_owned()).unwrap_or_default();
            format!("HTTP {}{}{}", resp.status(), if body.is_empty() { "" } else { ": " }, body)
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback_url;

    #[test]
    fn only_loopback_urls_are_allowed() {
        for ok in ["ws://127.0.0.1:4000", "ws://127.0.0.1:4000/", "ws://localhost:4000/x", "ws://[::1]:9/", "ws://LOCALHOST:1"] {
            assert!(is_loopback_url(ok), "{ok}");
        }
        for bad in ["wss://127.0.0.1:1", "http://127.0.0.1", "ws://example.com:1", "ws://127.0.0.1.evil.com:1/",
            "ws://127.0.0.1@evil.com/", "ws://localhost.evil.com", "ws://0.0.0.0:1", "ws://10.0.0.1:1", "", "127.0.0.1:1"] {
            assert!(!is_loopback_url(bad), "{bad}");
        }
    }
}
