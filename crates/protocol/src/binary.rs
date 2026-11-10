//! The binary form: the production wire format. One message per WebSocket binary frame.
//!
//! A frame is `tag: u8` followed by the fields of that message in declaration order. Client
//! tags are below 0x80, server tags are 0x80 and above. See `docs/protocol.md` for the table.
//! Byte payloads (Loro updates, version vectors) are length-prefixed raw bytes, never base64.

use crate::error::{DecodeError, EncodeError};
use crate::ids::{ClientId, RoomId};
use crate::limits::*;
use crate::message::*;
use crate::wire::{Reader, Writer};

// Client tags.
const C_JOIN: u8 = 0x01;
const C_UPDATE: u8 = 0x02;
const C_REQUEST_SYNC: u8 = 0x03;
const C_AWARENESS: u8 = 0x04;
const C_PING: u8 = 0x05;
const C_PONG: u8 = 0x06;
const C_LEAVE: u8 = 0x07;

// Server tags.
const S_JOINED: u8 = 0x81;
const S_SYNC_RESPONSE: u8 = 0x82;
const S_UPDATE: u8 = 0x83;
const S_ACK: u8 = 0x84;
const S_PEER_JOINED: u8 = 0x85;
const S_PEER_LEFT: u8 = 0x86;
const S_PEER_AWARENESS: u8 = 0x87;
const S_ERROR: u8 = 0x88;
const S_PING: u8 = 0x89;
const S_PONG: u8 = 0x8a;

// Awareness flag bits.
const A_USER: u8 = 1;
const A_CURSOR: u8 = 2;
const A_VIEWPORT: u8 = 4;
const A_SELECTION: u8 = 8;
const A_ALL: u8 = A_USER | A_CURSOR | A_VIEWPORT | A_SELECTION;

pub(crate) fn check_version_range(client_min: u16, client_max: u16) -> Result<(), DecodeError> {
    if negotiate_version(client_min, client_max).is_some() {
        Ok(())
    } else {
        Err(DecodeError::UnsupportedVersion {
            client_min,
            client_max,
            ours_min: MIN_PROTOCOL_VERSION,
            ours_max: PROTOCOL_VERSION,
        })
    }
}

fn finish(w: Writer) -> Result<Vec<u8>, EncodeError> {
    let out = w.finish();
    if out.len() > MAX_FRAME_BYTES {
        return Err(EncodeError::FrameTooLarge {
            len: out.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    Ok(out)
}

fn split_tag(frame: &[u8]) -> Result<(u8, Reader<'_>), DecodeError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(DecodeError::FrameTooLarge {
            len: frame.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    let (&tag, rest) = frame.split_first().ok_or(DecodeError::Empty)?;
    Ok((tag, Reader::new(rest)))
}

// ---- shared field codecs --------------------------------------------------------------------

fn put_opt_user(w: &mut Writer, user: &Option<UserInfo>) {
    match user {
        Some(u) => {
            w.u8(1);
            put_user(w, u);
        }
        None => w.u8(0),
    }
}

fn put_user(w: &mut Writer, u: &UserInfo) {
    w.str(&u.name);
    w.u32(u.color);
}

fn get_user(r: &mut Reader<'_>) -> Result<UserInfo, DecodeError> {
    Ok(UserInfo {
        name: r.string("user.name", MAX_USER_NAME_BYTES)?,
        color: r.u32("user.color")?,
    })
}

fn get_opt_user(r: &mut Reader<'_>) -> Result<Option<UserInfo>, DecodeError> {
    if r.bool("user flag")? {
        Ok(Some(get_user(r)?))
    } else {
        Ok(None)
    }
}

fn put_peer(w: &mut Writer, p: &PeerInfo) {
    w.raw(p.client_id.as_bytes());
    put_opt_user(w, &p.user);
}

fn get_peer(r: &mut Reader<'_>) -> Result<PeerInfo, DecodeError> {
    let client_id = ClientId::from_bytes(r.id16("client_id")?);
    Ok(PeerInfo {
        client_id,
        user: get_opt_user(r)?,
    })
}

fn put_awareness(w: &mut Writer, a: &AwarenessState) {
    let mut flags = 0;
    if a.user.is_some() {
        flags |= A_USER;
    }
    if a.cursor.is_some() {
        flags |= A_CURSOR;
    }
    if a.viewport.is_some() {
        flags |= A_VIEWPORT;
    }
    if !a.selection.is_empty() {
        flags |= A_SELECTION;
    }
    w.u8(flags);
    if let Some(u) = &a.user {
        put_user(w, u);
    }
    if let Some(p) = &a.cursor {
        w.f64(p.x);
        w.f64(p.y);
    }
    if let Some(v) = &a.viewport {
        w.f64(v.x);
        w.f64(v.y);
        w.f64(v.width);
        w.f64(v.height);
    }
    if !a.selection.is_empty() {
        w.count(a.selection.len());
        for id in &a.selection {
            w.str(id);
        }
    }
}

fn get_awareness(r: &mut Reader<'_>) -> Result<AwarenessState, DecodeError> {
    let flags = r.u8("awareness flags")?;
    if flags & !A_ALL != 0 {
        return Err(DecodeError::UnknownValue {
            field: "awareness flags",
            value: u32::from(flags),
        });
    }
    let user = if flags & A_USER != 0 {
        Some(get_user(r)?)
    } else {
        None
    };
    let cursor = if flags & A_CURSOR != 0 {
        Some(Point {
            x: r.f64("cursor.x")?,
            y: r.f64("cursor.y")?,
        })
    } else {
        None
    };
    let viewport = if flags & A_VIEWPORT != 0 {
        Some(Viewport {
            x: r.f64("viewport.x")?,
            y: r.f64("viewport.y")?,
            width: r.f64("viewport.width")?,
            height: r.f64("viewport.height")?,
        })
    } else {
        None
    };
    let mut selection = Vec::new();
    if flags & A_SELECTION != 0 {
        let n = r.count("selection", MAX_SELECTION_IDS, 1)?;
        selection.reserve(n);
        for _ in 0..n {
            selection.push(r.string("selection id", MAX_ELEMENT_ID_BYTES)?);
        }
    }
    Ok(AwarenessState {
        user,
        cursor,
        selection,
        viewport,
    })
}

fn get_room(r: &mut Reader<'_>) -> Result<RoomId, DecodeError> {
    Ok(RoomId::new(r.string("room", MAX_ROOM_ID_BYTES)?)?)
}

// ---- client messages ------------------------------------------------------------------------

impl ClientMessage {
    /// Encodes the message as one binary frame. Fails when the message breaks a size limit.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate()?;
        match self {
            ClientMessage::Join {
                min_version,
                max_version,
                room,
                client_id,
                auth_token,
                version,
                user,
            } => {
                let mut w = Writer::new(C_JOIN);
                // The version range comes first and its layout never changes, so a server of
                // any version can tell that it cannot talk to this client.
                w.u16(*min_version);
                w.u16(*max_version);
                w.str(room.as_str());
                w.raw(client_id.as_bytes());
                match auth_token {
                    Some(t) => {
                        w.u8(1);
                        w.str(t);
                    }
                    None => w.u8(0),
                }
                w.bytes(version);
                put_opt_user(&mut w, user);
                finish(w)
            }
            ClientMessage::Update { seq, update } => {
                let mut w = Writer::new(C_UPDATE);
                w.u32(*seq);
                w.bytes(update);
                finish(w)
            }
            ClientMessage::RequestSync { version } => {
                let mut w = Writer::new(C_REQUEST_SYNC);
                w.bytes(version);
                finish(w)
            }
            ClientMessage::Awareness(state) => {
                let mut w = Writer::new(C_AWARENESS);
                put_awareness(&mut w, state);
                finish(w)
            }
            ClientMessage::Ping { nonce } => {
                let mut w = Writer::new(C_PING);
                w.u64(*nonce);
                finish(w)
            }
            ClientMessage::Pong { nonce } => {
                let mut w = Writer::new(C_PONG);
                w.u64(*nonce);
                finish(w)
            }
            ClientMessage::Leave => finish(Writer::new(C_LEAVE)),
        }
    }

    /// Decodes one binary frame. Any input is safe: the result is a message or a typed error.
    ///
    /// A `Join` whose version range does not overlap ours returns
    /// [`DecodeError::UnsupportedVersion`] before the rest of the frame is read, because a
    /// newer client may have changed the layout after the range.
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        let (tag, mut r) = split_tag(frame)?;
        let msg = match tag {
            C_JOIN => {
                let min_version = r.u16("min_version")?;
                let max_version = r.u16("max_version")?;
                check_version_range(min_version, max_version)?;
                let room = get_room(&mut r)?;
                let client_id = ClientId::from_bytes(r.id16("client_id")?);
                let auth_token = if r.bool("auth flag")? {
                    Some(r.string("auth_token", MAX_AUTH_TOKEN_BYTES)?)
                } else {
                    None
                };
                let version = r.bytes("version", MAX_VERSION_BYTES)?;
                let user = get_opt_user(&mut r)?;
                ClientMessage::Join {
                    min_version,
                    max_version,
                    room,
                    client_id,
                    auth_token,
                    version,
                    user,
                }
            }
            C_UPDATE => ClientMessage::Update {
                seq: r.u32("seq")?,
                update: r.bytes("update", MAX_UPDATE_BYTES)?,
            },
            C_REQUEST_SYNC => ClientMessage::RequestSync {
                version: r.bytes("version", MAX_VERSION_BYTES)?,
            },
            C_AWARENESS => ClientMessage::Awareness(get_awareness(&mut r)?),
            C_PING => ClientMessage::Ping {
                nonce: r.u64("nonce")?,
            },
            C_PONG => ClientMessage::Pong {
                nonce: r.u64("nonce")?,
            },
            C_LEAVE => ClientMessage::Leave,
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.finish()?;
        msg.validate()?;
        Ok(msg)
    }
}

// ---- server messages ------------------------------------------------------------------------

impl ServerMessage {
    /// Encodes the message as one binary frame. Fails when the message breaks a size limit.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate()?;
        match self {
            ServerMessage::Joined {
                protocol_version,
                server,
                room,
                client_id,
                peers,
                server_version,
                limits,
            } => {
                let mut w = Writer::new(S_JOINED);
                w.u16(*protocol_version);
                w.str(server);
                w.str(room.as_str());
                w.raw(client_id.as_bytes());
                w.count(peers.len());
                for p in peers {
                    put_peer(&mut w, p);
                }
                w.bytes(server_version);
                w.u32(limits.max_frame_bytes);
                w.u32(limits.max_update_bytes);
                w.u16(limits.max_updates_per_sec);
                w.u16(limits.max_awareness_per_sec);
                finish(w)
            }
            ServerMessage::SyncResponse {
                kind,
                data,
                server_version,
            } => {
                let mut w = Writer::new(S_SYNC_RESPONSE);
                w.u8(match kind {
                    SyncKind::Updates => 0,
                    SyncKind::Snapshot => 1,
                });
                w.bytes(data);
                w.bytes(server_version);
                finish(w)
            }
            ServerMessage::Update { from, update } => {
                let mut w = Writer::new(S_UPDATE);
                w.raw(from.as_bytes());
                w.bytes(update);
                finish(w)
            }
            ServerMessage::Ack { seq } => {
                let mut w = Writer::new(S_ACK);
                w.u32(*seq);
                finish(w)
            }
            ServerMessage::PeerJoined { peer } => {
                let mut w = Writer::new(S_PEER_JOINED);
                put_peer(&mut w, peer);
                finish(w)
            }
            ServerMessage::PeerLeft { client_id } => {
                let mut w = Writer::new(S_PEER_LEFT);
                w.raw(client_id.as_bytes());
                finish(w)
            }
            ServerMessage::PeerAwareness { from, state } => {
                let mut w = Writer::new(S_PEER_AWARENESS);
                w.raw(from.as_bytes());
                put_awareness(&mut w, state);
                finish(w)
            }
            ServerMessage::Error {
                code,
                message,
                retry_after_ms,
            } => {
                let mut w = Writer::new(S_ERROR);
                w.u8(code.as_u8());
                w.str(message);
                match retry_after_ms {
                    Some(ms) => {
                        w.u8(1);
                        w.u32(*ms);
                    }
                    None => w.u8(0),
                }
                finish(w)
            }
            ServerMessage::Ping { nonce } => {
                let mut w = Writer::new(S_PING);
                w.u64(*nonce);
                finish(w)
            }
            ServerMessage::Pong { nonce } => {
                let mut w = Writer::new(S_PONG);
                w.u64(*nonce);
                finish(w)
            }
        }
    }

    /// Decodes one binary frame. Any input is safe: the result is a message or a typed error.
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        let (tag, mut r) = split_tag(frame)?;
        let msg = match tag {
            S_JOINED => {
                let protocol_version = r.u16("protocol_version")?;
                let server = r.string("server", MAX_SERVER_NAME_BYTES)?;
                let room = get_room(&mut r)?;
                let client_id = ClientId::from_bytes(r.id16("client_id")?);
                // A peer takes at least 17 bytes: 16 for the id and 1 for the user flag.
                let n = r.count("peers", MAX_PEERS, 17)?;
                let mut peers = Vec::with_capacity(n);
                for _ in 0..n {
                    peers.push(get_peer(&mut r)?);
                }
                let server_version = r.bytes("server_version", MAX_VERSION_BYTES)?;
                let limits = Limits {
                    max_frame_bytes: r.u32("limits.max_frame_bytes")?,
                    max_update_bytes: r.u32("limits.max_update_bytes")?,
                    max_updates_per_sec: r.u16("limits.max_updates_per_sec")?,
                    max_awareness_per_sec: r.u16("limits.max_awareness_per_sec")?,
                };
                ServerMessage::Joined {
                    protocol_version,
                    server,
                    room,
                    client_id,
                    peers,
                    server_version,
                    limits,
                }
            }
            S_SYNC_RESPONSE => {
                let kind = match r.u8("sync kind")? {
                    0 => SyncKind::Updates,
                    1 => SyncKind::Snapshot,
                    v => {
                        return Err(DecodeError::UnknownValue {
                            field: "sync kind",
                            value: u32::from(v),
                        });
                    }
                };
                ServerMessage::SyncResponse {
                    kind,
                    data: r.bytes("data", MAX_SYNC_BYTES)?,
                    server_version: r.bytes("server_version", MAX_VERSION_BYTES)?,
                }
            }
            S_UPDATE => ServerMessage::Update {
                from: ClientId::from_bytes(r.id16("from")?),
                update: r.bytes("update", MAX_UPDATE_BYTES)?,
            },
            S_ACK => ServerMessage::Ack { seq: r.u32("seq")? },
            S_PEER_JOINED => ServerMessage::PeerJoined {
                peer: get_peer(&mut r)?,
            },
            S_PEER_LEFT => ServerMessage::PeerLeft {
                client_id: ClientId::from_bytes(r.id16("client_id")?),
            },
            S_PEER_AWARENESS => ServerMessage::PeerAwareness {
                from: ClientId::from_bytes(r.id16("from")?),
                state: get_awareness(&mut r)?,
            },
            S_ERROR => {
                let raw = r.u8("error code")?;
                let code = ErrorCode::from_u8(raw).ok_or(DecodeError::UnknownValue {
                    field: "error code",
                    value: u32::from(raw),
                })?;
                let message = r.string("message", MAX_ERROR_MESSAGE_BYTES)?;
                let retry_after_ms = if r.bool("retry flag")? {
                    Some(r.u32("retry_after_ms")?)
                } else {
                    None
                };
                ServerMessage::Error {
                    code,
                    message,
                    retry_after_ms,
                }
            }
            S_PING => ServerMessage::Ping {
                nonce: r.u64("nonce")?,
            },
            S_PONG => ServerMessage::Pong {
                nonce: r.u64("nonce")?,
            },
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.finish()?;
        msg.validate()?;
        Ok(msg)
    }
}
