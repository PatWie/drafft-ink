//! WebSocket client for collaboration.
//!
//! Provides a platform-agnostic WebSocket client interface for connecting
//! to the relay server.

use serde::{Deserialize, Serialize};

/// Messages sent to the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Join a room
    Join { room: String },
    /// Leave current room
    Leave,
    /// Sync CRDT data (base64 encoded Loro bytes)
    Sync { data: String },
    /// Awareness update (cursor position, selection, etc.)
    Awareness {
        peer_id: u64,
        #[serde(flatten)]
        state: AwarenessState,
    },
}

/// Messages received from the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Confirm room join with current state
    Joined {
        room: String,
        peer_count: usize,
        /// Initial sync data (if room has history)
        #[serde(skip_serializing_if = "Option::is_none")]
        initial_sync: Option<String>,
    },
    /// Peer joined the room
    PeerJoined { peer_id: String },
    /// Peer left the room
    PeerLeft { peer_id: String },
    /// Sync data from another peer
    Sync { from: String, data: String },
    /// Awareness update from another peer
    Awareness {
        from: String,
        peer_id: u64,
        #[serde(flatten)]
        state: AwarenessState,
    },
    /// Error message
    Error { message: String },
}

/// Awareness state for a peer
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AwarenessState {
    /// Cursor position (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CursorPosition>,
    /// User name/color
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub name: String,
    pub color: String,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// Events from the WebSocket client
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// Connected to server
    Connected,
    /// Disconnected from server
    Disconnected,
    /// Joined a room
    JoinedRoom {
        room: String,
        peer_count: usize,
        initial_sync: Option<Vec<u8>>,
    },
    /// A peer joined the room
    PeerJoined { peer_id: String },
    /// A peer left the room
    PeerLeft { peer_id: String },
    /// Received sync data from a peer
    SyncReceived { from: String, data: Vec<u8> },
    /// Received awareness update from a peer
    AwarenessReceived {
        from: String,
        peer_id: u64,
        state: AwarenessState,
    },
    /// Error occurred
    Error { message: String },
}

/// Base64 decoding
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const DECODE_TABLE: [i8; 128] = [
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 62, -1, -1,
        -1, 63, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, -1, -1, -1, -1, -1, -1, -1, 0, 1, 2, 3, 4,
        5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, -1, -1, -1,
        -1, -1, -1, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
        46, 47, 48, 49, 50, 51, -1, -1, -1, -1, -1,
    ];

    let input = input.trim_end_matches('=');
    let mut result = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;

    for c in input.bytes() {
        if c >= 128 {
            return None;
        }
        let val = DECODE_TABLE[c as usize];
        if val < 0 {
            return None;
        }
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            result.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Some(result)
}

/// Base64 encoding
pub fn base64_encode(data: &[u8]) -> String {
    const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        result.push(B64_CHARS[(b0 >> 2) as usize] as char);
        result.push(B64_CHARS[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);

        if chunk.len() > 1 {
            result.push(B64_CHARS[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(B64_CHARS[(b2 & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

// ============================================================================
// WASM WebSocket Client
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod wasm_client {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

    /// WebSocket client for WASM.
    ///
    /// Events are collected and must be polled via `poll_events()`.
    pub struct WasmWebSocket {
        ws: Option<WebSocket>,
        state: ConnectionState,
        events: Rc<RefCell<Vec<SyncEvent>>>,
        // Store closures to prevent them from being dropped
        _on_open: Option<Closure<dyn Fn()>>,
        _on_message: Option<Closure<dyn Fn(MessageEvent)>>,
        _on_close: Option<Closure<dyn Fn(CloseEvent)>>,
        _on_error: Option<Closure<dyn Fn(ErrorEvent)>>,
    }

    impl WasmWebSocket {
        /// Create a new disconnected WebSocket client.
        pub fn new() -> Self {
            Self {
                ws: None,
                state: ConnectionState::Disconnected,
                events: Rc::new(RefCell::new(Vec::new())),
                _on_open: None,
                _on_message: None,
                _on_close: None,
                _on_error: None,
            }
        }

        /// Connect to a WebSocket server.
        pub fn connect(&mut self, url: &str) -> Result<(), String> {
            if self.ws.is_some() {
                return Err("Already connected".to_string());
            }

            let ws =
                WebSocket::new(url).map_err(|e| format!("Failed to create WebSocket: {:?}", e))?;
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

            self.state = ConnectionState::Connecting;
            let events = self.events.clone();

            // onopen
            let events_open = events.clone();
            let on_open = Closure::wrap(Box::new(move || {
                events_open.borrow_mut().push(SyncEvent::Connected);
            }) as Box<dyn Fn()>);
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));

            // onmessage
            let events_msg = events.clone();
            let on_message = Closure::wrap(Box::new(move |e: MessageEvent| {
                if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
                    let s: String = txt.into();
                    // Parse and convert to SyncEvent
                    if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&s) {
                        let event = match server_msg {
                            ServerMessage::Joined {
                                room,
                                peer_count,
                                initial_sync,
                            } => {
                                let data = initial_sync.and_then(|s| super::base64_decode(&s));
                                SyncEvent::JoinedRoom {
                                    room,
                                    peer_count,
                                    initial_sync: data,
                                }
                            }
                            ServerMessage::PeerJoined { peer_id } => {
                                SyncEvent::PeerJoined { peer_id }
                            }
                            ServerMessage::PeerLeft { peer_id } => SyncEvent::PeerLeft { peer_id },
                            ServerMessage::Sync { from, data } => {
                                if let Some(bytes) = super::base64_decode(&data) {
                                    SyncEvent::SyncReceived { from, data: bytes }
                                } else {
                                    return;
                                }
                            }
                            ServerMessage::Awareness {
                                from,
                                peer_id,
                                state,
                            } => SyncEvent::AwarenessReceived {
                                from,
                                peer_id,
                                state,
                            },
                            ServerMessage::Error { message } => SyncEvent::Error { message },
                        };
                        events_msg.borrow_mut().push(event);
                    }
                }
            }) as Box<dyn Fn(MessageEvent)>);
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

            // onclose
            let events_close = events.clone();
            let on_close = Closure::wrap(Box::new(move |_e: CloseEvent| {
                events_close.borrow_mut().push(SyncEvent::Disconnected);
            }) as Box<dyn Fn(CloseEvent)>);
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

            // onerror
            let events_err = events;
            let on_error = Closure::wrap(Box::new(move |_e: ErrorEvent| {
                events_err.borrow_mut().push(SyncEvent::Error {
                    message: "WebSocket error".to_string(),
                });
            }) as Box<dyn Fn(ErrorEvent)>);
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

            self.ws = Some(ws);
            self._on_open = Some(on_open);
            self._on_message = Some(on_message);
            self._on_close = Some(on_close);
            self._on_error = Some(on_error);

            Ok(())
        }

        /// Disconnect from the server.
        pub fn disconnect(&mut self) {
            if let Some(ws) = self.ws.take() {
                let _ = ws.close();
            }
            self.state = ConnectionState::Disconnected;
            self._on_open = None;
            self._on_message = None;
            self._on_close = None;
            self._on_error = None;
        }

        /// Send a text message.
        pub fn send(&self, msg: &str) -> Result<(), String> {
            if let Some(ref ws) = self.ws {
                ws.send_with_str(msg)
                    .map_err(|e| format!("Send failed: {:?}", e))
            } else {
                Err("Not connected".to_string())
            }
        }

        /// Poll for pending events (non-blocking).
        pub fn poll_events(&mut self) -> Vec<SyncEvent> {
            let mut events = self.events.borrow_mut();

            // Update state based on events
            for event in events.iter() {
                match event {
                    SyncEvent::Connected => self.state = ConnectionState::Connected,
                    SyncEvent::Disconnected => self.state = ConnectionState::Disconnected,
                    SyncEvent::Error { .. } => self.state = ConnectionState::Error,
                    _ => {}
                }
            }

            std::mem::take(&mut *events)
        }

        /// Get current connection state.
        pub fn state(&self) -> ConnectionState {
            self.state
        }

        /// Check if connected.
        pub fn is_connected(&self) -> bool {
            self.state == ConnectionState::Connected
        }
    }

    impl Default for WasmWebSocket {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_client::WasmWebSocket;

// ============================================================================
// Native WebSocket Client
// ============================================================================

#[cfg(not(target_arch = "wasm32"))]
mod native_client {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use tungstenite::{Message, connect};
    use url::Url;

    /// Commands sent to the WebSocket thread.
    enum WsCommand {
        Send(String),
        Close,
    }

    /// WebSocket client for native platforms.
    ///
    /// Uses a background thread for non-blocking operation.
    pub struct NativeWebSocket {
        state: ConnectionState,
        events: Vec<SyncEvent>,
        /// Channel to send commands to the WebSocket thread.
        cmd_tx: Option<Sender<WsCommand>>,
        /// Channel to receive events from the WebSocket thread.
        event_rx: Option<Receiver<SyncEvent>>,
        /// Handle to the WebSocket thread.
        _thread: Option<JoinHandle<()>>,
    }

    impl NativeWebSocket {
        /// Create a new disconnected WebSocket client.
        pub fn new() -> Self {
            Self {
                state: ConnectionState::Disconnected,
                events: Vec::new(),
                cmd_tx: None,
                event_rx: None,
                _thread: None,
            }
        }

        /// Connect to a WebSocket server.
        pub fn connect(&mut self, url: &str) -> Result<(), String> {
            if self.cmd_tx.is_some() {
                return Err("Already connected".to_string());
            }

            // Validate URL
            let parsed_url = Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
            if parsed_url.scheme() != "ws" && parsed_url.scheme() != "wss" {
                return Err(format!(
                    "Invalid WebSocket URL scheme: {}",
                    parsed_url.scheme()
                ));
            }

            self.state = ConnectionState::Connecting;

            let (cmd_tx, cmd_rx) = channel::<WsCommand>();
            let (event_tx, event_rx) = channel::<SyncEvent>();

            let url = url.to_string();

            let handle = thread::spawn(move || {
                log::info!("WebSocket thread: connecting to {}", url);

                // Connect to WebSocket with timeout
                let ws_result = connect(&url);

                match ws_result {
                    Ok((mut socket, response)) => {
                        log::info!("WebSocket connected, status: {}", response.status());
                        let _ = event_tx.send(SyncEvent::Connected);

                        // Set read timeout on the underlying TCP stream for non-blocking behavior
                        // This is more reliable for tunneled/forwarded connections
                        {
                            let stream = socket.get_mut();
                            match stream {
                                tungstenite::stream::MaybeTlsStream::Plain(tcp) => {
                                    let _ = tcp.set_read_timeout(Some(Duration::from_millis(50)));
                                    let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));
                                }
                                #[allow(unreachable_patterns)]
                                _ => {
                                    // For TLS streams, we'll rely on WouldBlock/TimedOut errors
                                    log::debug!(
                                        "TLS or other stream - using default timeout handling"
                                    );
                                }
                            }
                        }

                        loop {
                            // Check for commands (non-blocking)
                            match cmd_rx.try_recv() {
                                Ok(WsCommand::Send(msg)) => {
                                    log::debug!(
                                        "WebSocket sending: {}",
                                        &msg[..msg.len().min(100)]
                                    );
                                    if let Err(e) = socket.send(Message::Text(msg)) {
                                        log::error!("WebSocket send error: {}", e);
                                        break;
                                    }
                                }
                                Ok(WsCommand::Close) => {
                                    log::info!("WebSocket close requested");
                                    let _ = socket.close(None);
                                    break;
                                }
                                Err(TryRecvError::Disconnected) => {
                                    log::info!("WebSocket command channel disconnected");
                                    break;
                                }
                                Err(TryRecvError::Empty) => {}
                            }

                            // Check for incoming messages (with timeout)
                            match socket.read() {
                                Ok(Message::Text(txt)) => {
                                    log::debug!(
                                        "WebSocket received: {}",
                                        &txt[..txt.len().min(100)]
                                    );
                                    if let Ok(server_msg) =
                                        serde_json::from_str::<ServerMessage>(&txt)
                                    {
                                        let event = match server_msg {
                                            ServerMessage::Joined {
                                                room,
                                                peer_count,
                                                initial_sync,
                                            } => {
                                                let data = initial_sync
                                                    .and_then(|s| super::base64_decode(&s));
                                                SyncEvent::JoinedRoom {
                                                    room,
                                                    peer_count,
                                                    initial_sync: data,
                                                }
                                            }
                                            ServerMessage::PeerJoined { peer_id } => {
                                                SyncEvent::PeerJoined { peer_id }
                                            }
                                            ServerMessage::PeerLeft { peer_id } => {
                                                SyncEvent::PeerLeft { peer_id }
                                            }
                                            ServerMessage::Sync { from, data } => {
                                                if let Some(bytes) = super::base64_decode(&data) {
                                                    SyncEvent::SyncReceived { from, data: bytes }
                                                } else {
                                                    continue;
                                                }
                                            }
                                            ServerMessage::Awareness {
                                                from,
                                                peer_id,
                                                state,
                                            } => SyncEvent::AwarenessReceived {
                                                from,
                                                peer_id,
                                                state,
                                            },
                                            ServerMessage::Error { message } => {
                                                SyncEvent::Error { message }
                                            }
                                        };
                                        let _ = event_tx.send(event);
                                    } else {
                                        log::warn!("Failed to parse server message: {}", txt);
                                    }
                                }
                                Ok(Message::Ping(data)) => {
                                    // Respond to ping with pong
                                    let _ = socket.send(Message::Pong(data));
                                }
                                Ok(Message::Close(_)) => {
                                    log::info!("WebSocket received close frame");
                                    break;
                                }
                                Ok(_) => {} // Ignore binary, pong
                                Err(tungstenite::Error::Io(ref e))
                                    if e.kind() == std::io::ErrorKind::WouldBlock
                                        || e.kind() == std::io::ErrorKind::TimedOut =>
                                {
                                    // Timeout on read, continue loop
                                    continue;
                                }
                                Err(e) => {
                                    log::error!("WebSocket read error: {}", e);
                                    break;
                                }
                            }
                        }

                        log::info!("WebSocket thread exiting");
                        let _ = event_tx.send(SyncEvent::Disconnected);
                    }
                    Err(e) => {
                        log::error!("WebSocket connection failed: {}", e);
                        let _ = event_tx.send(SyncEvent::Error {
                            message: format!("Connection failed: {}", e),
                        });
                    }
                }
            });

            self.cmd_tx = Some(cmd_tx);
            self.event_rx = Some(event_rx);
            self._thread = Some(handle);

            Ok(())
        }

        /// Disconnect from the server.
        pub fn disconnect(&mut self) {
            if let Some(tx) = self.cmd_tx.take() {
                let _ = tx.send(WsCommand::Close);
            }
            self.event_rx = None;
            self._thread = None;
            self.state = ConnectionState::Disconnected;
        }

        /// Send a text message.
        pub fn send(&self, msg: &str) -> Result<(), String> {
            if let Some(ref tx) = self.cmd_tx {
                tx.send(WsCommand::Send(msg.to_string()))
                    .map_err(|e| format!("Send failed: {}", e))
            } else {
                Err("Not connected".to_string())
            }
        }

        /// Poll for pending events (non-blocking).
        pub fn poll_events(&mut self) -> Vec<SyncEvent> {
            // Drain events from channel
            if let Some(ref rx) = self.event_rx {
                while let Ok(event) = rx.try_recv() {
                    // Update state based on event
                    match &event {
                        SyncEvent::Connected => self.state = ConnectionState::Connected,
                        SyncEvent::Disconnected => self.state = ConnectionState::Disconnected,
                        SyncEvent::Error { .. } => self.state = ConnectionState::Error,
                        _ => {}
                    }
                    self.events.push(event);
                }
            }

            std::mem::take(&mut self.events)
        }

        /// Get current connection state.
        pub fn state(&self) -> ConnectionState {
            self.state
        }

        /// Check if connected.
        pub fn is_connected(&self) -> bool {
            self.state == ConnectionState::Connected
        }
    }

    impl Default for NativeWebSocket {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for NativeWebSocket {
        fn drop(&mut self) {
            self.disconnect();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native_client::NativeWebSocket;

// ============================================================================
// Platform type alias
// ============================================================================

/// Platform-specific WebSocket client type.
#[cfg(target_arch = "wasm32")]
pub type PlatformWebSocket = WasmWebSocket;

#[cfg(not(target_arch = "wasm32"))]
pub type PlatformWebSocket = NativeWebSocket;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_roundtrip() {
        let data = b"Hello, World!";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(data.to_vec(), decoded);
    }

    #[test]
    fn test_base64_empty() {
        let data = b"";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(data.to_vec(), decoded);
    }

    #[test]
    fn test_base64_padding() {
        // 1 byte -> 2 chars + 2 padding
        assert_eq!(base64_encode(b"a"), "YQ==");
        // 2 bytes -> 3 chars + 1 padding
        assert_eq!(base64_encode(b"ab"), "YWI=");
        // 3 bytes -> 4 chars, no padding
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }

    #[test]
    fn test_client_message_serialize() {
        let msg = ClientMessage::Join {
            room: "test-room".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("join"));
        assert!(json.contains("test-room"));
    }

    #[test]
    fn test_server_message_deserialize() {
        let json = r#"{"type":"joined","room":"test","peer_count":2}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::Joined {
                room, peer_count, ..
            } => {
                assert_eq!(room, "test");
                assert_eq!(peer_count, 2);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
