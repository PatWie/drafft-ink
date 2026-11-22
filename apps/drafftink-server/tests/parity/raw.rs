//! A hand-made client of a sealed room: it speaks the wire protocol itself, so that a scenario can do what the real
//! client never does (join with the wrong token, with another room's id).

use std::net::TcpStream;
use std::time::{Duration, Instant};

use drafftink_protocol::ClientId;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, JoinMode, Proof, RoomId, ServerMessage};
use drafftink_seal::RoomKeys;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::relay::Relay;

/// How long a hand-made client waits for an answer it knows is coming.
const WAIT: Duration = Duration::from_secs(30);

pub struct Raw {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    id: ClientId,
}

impl Raw {
    /// Opens the socket of room `room` on the relay.
    pub fn connect(relay: &Relay, room: RoomId, n: u8) -> Raw {
        let url = format!("ws://{}/r/{room}/ws", relay.addr);
        let (ws, _) = tungstenite::connect(url).expect("the upgrade works");
        if let MaybeTlsStream::Plain(stream) = ws.get_ref() {
            stream.set_read_timeout(Some(Duration::from_millis(250))).expect("a read timeout");
        }
        Raw { ws, id: ClientId::from_bytes([n; 16]) }
    }

    /// Sends a `Join` for `room` that carries the membership token of `proof_from`.
    pub fn join(&mut self, room: RoomId, proof_from: &RoomKeys, mode: JoinMode) {
        let message = ClientMessage::Join {
            min_version: 2,
            max_version: 2,
            room,
            client_id: self.id,
            mode,
            proof: Proof::token(proof_from.membership_token()),
            last_seq: 0,
            captcha: None,
        };
        self.ws.send(Message::Binary(message.encode().expect("encodes").into())).expect("sends");
    }

    /// The error code the relay answers with, or `None` when a message other than an error comes first.
    pub fn error(&mut self) -> Option<ErrorCode> {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    return match ServerMessage::decode(&bytes).expect("server frames decode") {
                        ServerMessage::Error { code, .. } => Some(code),
                        _ => None,
                    };
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return None,
            }
        }
        panic!("no answer within {}s", WAIT.as_secs());
    }
}
