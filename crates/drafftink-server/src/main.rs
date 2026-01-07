//! DrafftInk WebSocket Relay Server
//!
//! A simple relay server that broadcasts CRDT updates between clients in the same room.
//!
//! ## Protocol
//!
//! Messages are JSON with the following format:
//! ```json
//! { "type": "join", "room": "room-id" }
//! { "type": "sync", "data": "<base64-encoded-loro-bytes>" }
//! { "type": "awareness", "peer_id": 123, "cursor": { "x": 100, "y": 200 } }
//! ```

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use uuid::Uuid;

/// Server configuration
const MAX_ROOM_HISTORY: usize = 100;
const CHANNEL_CAPACITY: usize = 256;

/// A message sent between clients
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

/// Awareness state for a peer
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A message broadcast to clients
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

/// Room state
struct Room {
    /// Broadcast channel for this room
    tx: broadcast::Sender<(String, ServerMessage)>,
    /// Connected peer IDs
    peers: HashSet<String>,
    /// Last sync data (for new joiners)
    last_sync: Option<String>,
}

impl Room {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            tx,
            peers: HashSet::new(),
            last_sync: None,
        }
    }
}

/// Shared application state
struct AppState {
    /// Active rooms
    rooms: DashMap<String, Room>,
}

impl AppState {
    fn new() -> Self {
        Self {
            rooms: DashMap::new(),
        }
    }

    /// Get or create a room
    fn get_or_create_room(&self, room_id: &str) -> broadcast::Sender<(String, ServerMessage)> {
        self.rooms
            .entry(room_id.to_string())
            .or_insert_with(Room::new)
            .tx
            .clone()
    }

    /// Add peer to room
    fn join_room(
        &self,
        room_id: &str,
        peer_id: &str,
    ) -> (
        broadcast::Receiver<(String, ServerMessage)>,
        Option<String>,
        usize,
    ) {
        let mut room = self
            .rooms
            .entry(room_id.to_string())
            .or_insert_with(Room::new);
        room.peers.insert(peer_id.to_string());
        let rx = room.tx.subscribe();
        let initial_sync = room.last_sync.clone();
        let peer_count = room.peers.len();
        (rx, initial_sync, peer_count)
    }

    /// Remove peer from room
    fn leave_room(&self, room_id: &str, peer_id: &str) {
        if let Some(mut room) = self.rooms.get_mut(room_id) {
            room.peers.remove(peer_id);
            // Clean up empty rooms
            if room.peers.is_empty() {
                drop(room);
                self.rooms.remove(room_id);
            }
        }
    }

    /// Update room's last sync data
    fn update_sync(&self, room_id: &str, data: String) {
        if let Some(mut room) = self.rooms.get_mut(room_id) {
            room.last_sync = Some(data);
        }
    }

    /// Broadcast message to room
    fn broadcast(&self, room_id: &str, from: &str, msg: ServerMessage) {
        if let Some(room) = self.rooms.get(room_id) {
            let _ = room.tx.send((from.to_string(), msg));
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "drafftink_server=info,tower_http=info".into()),
        )
        .init();

    let state = Arc::new(AppState::new());

    let app = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler))
        .route("/health", get(health))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3030));
    info!("DrafftInk relay server listening on {}", addr);
    info!("WebSocket endpoint: ws://localhost:3030/ws");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Index page
async fn index() -> &'static str {
    "DrafftInk Relay Server - Connect via WebSocket at /ws"
}

/// Health check
async fn health() -> &'static str {
    "ok"
}

/// WebSocket upgrade handler
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle a WebSocket connection
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let peer_id = Uuid::new_v4().to_string();
    info!("New connection: {}", peer_id);

    let (mut sender, mut receiver) = socket.split();
    let mut current_room: Option<String> = None;
    let mut room_rx: Option<broadcast::Receiver<(String, ServerMessage)>> = None;

    loop {
        tokio::select! {
            // Handle incoming messages from client
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                match client_msg {
                                    ClientMessage::Join { room } => {
                                        // Leave current room if any
                                        if let Some(ref old_room) = current_room {
                                            state.leave_room(old_room, &peer_id);
                                            state.broadcast(old_room, &peer_id, ServerMessage::PeerLeft {
                                                peer_id: peer_id.clone(),
                                            });
                                        }

                                        // Join new room
                                        let (rx, initial_sync, peer_count) = state.join_room(&room, &peer_id);
                                        room_rx = Some(rx);
                                        current_room = Some(room.clone());

                                        // Send joined confirmation
                                        let joined = ServerMessage::Joined {
                                            room: room.clone(),
                                            peer_count,
                                            initial_sync,
                                        };
                                        if sender.send(Message::Text(serde_json::to_string(&joined).unwrap().into())).await.is_err() {
                                            break;
                                        }

                                        // Notify others
                                        state.broadcast(&room, &peer_id, ServerMessage::PeerJoined {
                                            peer_id: peer_id.clone(),
                                        });

                                        info!("Peer {} joined room {}", peer_id, room);
                                    }
                                    ClientMessage::Leave => {
                                        if let Some(ref room) = current_room {
                                            state.leave_room(room, &peer_id);
                                            state.broadcast(room, &peer_id, ServerMessage::PeerLeft {
                                                peer_id: peer_id.clone(),
                                            });
                                            info!("Peer {} left room {}", peer_id, room);
                                        }
                                        current_room = None;
                                        room_rx = None;
                                    }
                                    ClientMessage::Sync { data } => {
                                        if let Some(ref room) = current_room {
                                            // Store as last sync for new joiners
                                            state.update_sync(room, data.clone());
                                            // Broadcast to others
                                            state.broadcast(room, &peer_id, ServerMessage::Sync {
                                                from: peer_id.clone(),
                                                data,
                                            });
                                        }
                                    }
                                    ClientMessage::Awareness { peer_id: awareness_peer_id, state: awareness_state } => {
                                        if let Some(ref room) = current_room {
                                            state.broadcast(room, &peer_id, ServerMessage::Awareness {
                                                from: peer_id.clone(),
                                                peer_id: awareness_peer_id,
                                                state: awareness_state,
                                            });
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Invalid message from {}: {}", peer_id, e);
                                let err = ServerMessage::Error {
                                    message: format!("Invalid message: {}", e),
                                };
                                let _ = sender.send(Message::Text(serde_json::to_string(&err).unwrap().into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Binary messages are treated as raw sync data
                        if let Some(ref room) = current_room {
                            let data_b64 = base64_encode(&data);
                            state.update_sync(room, data_b64.clone());
                            state.broadcast(room, &peer_id, ServerMessage::Sync {
                                from: peer_id.clone(),
                                data: data_b64,
                            });
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Ok(_)) => {} // Ignore ping/pong
                    Some(Err(e)) => {
                        warn!("WebSocket error for {}: {}", peer_id, e);
                        break;
                    }
                }
            }

            // Handle broadcast messages from room
            msg = async {
                match &mut room_rx {
                    Some(rx) => rx.recv().await.ok(),
                    None => {
                        // No room joined, just wait forever
                        std::future::pending::<Option<(String, ServerMessage)>>().await
                    }
                }
            } => {
                if let Some((from, server_msg)) = msg {
                    // Don't echo back to sender
                    if from != peer_id {
                        let json = serde_json::to_string(&server_msg).unwrap();
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }

    // Cleanup on disconnect
    if let Some(ref room) = current_room {
        state.leave_room(room, &peer_id);
        state.broadcast(
            room,
            &peer_id,
            ServerMessage::PeerLeft {
                peer_id: peer_id.clone(),
            },
        );
    }
    info!("Connection closed: {}", peer_id);
}

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Simple base64 encoding
fn base64_encode(data: &[u8]) -> String {
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
