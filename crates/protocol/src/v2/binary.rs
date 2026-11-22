//! The binary form of protocol v2. Same framing as v1 (`crate::wire`): one message per WebSocket binary frame, a
//! tag byte, little-endian fixed integers, varint lengths. Client tags are below 0x80, relay tags 0x80 and above.

use crate::binary::{finish, split_tag};
use crate::error::{DecodeError, EncodeError};
use crate::ids::ClientId;
use crate::limits::*;
use crate::v2::ids::{Proof, ProofScheme, RoomId};
use crate::v2::message::*;
use crate::version::{JOIN_TAG, JoinVersions};
use crate::wire::{Reader, Writer};

// Client tags. `JOIN_TAG` (0x01) is shared with v1.
const C_LEAVE: u8 = 0x07;
const C_APPEND: u8 = 0x10;
const C_PUT_SNAPSHOT: u8 = 0x11;
const C_PRESENCE: u8 = 0x12;
const C_CATCHUP: u8 = 0x13;
const C_DIRECT: u8 = 0x14;
const C_DELETE_ROOM: u8 = 0x15;

// Relay tags.
const S_JOINED: u8 = 0x81;
const S_CATCHUP: u8 = 0x82;
const S_RECORD: u8 = 0x83;
const S_ACK: u8 = 0x84;
const S_PEER_JOINED: u8 = 0x85;
const S_PEER_LEFT: u8 = 0x86;
const S_PRESENCE_BUNDLE: u8 = 0x87;
const S_ERROR: u8 = 0x88;
const S_SNAPSHOT_RESULT: u8 = 0x89;
const S_COMPACT_ADVICE: u8 = 0x8a;
const S_DIRECT: u8 = 0x8b;
const S_RETENTION: u8 = 0x8c;

fn unknown(field: &'static str, value: u8) -> DecodeError {
    DecodeError::UnknownValue { field, value: u32::from(value) }
}

fn put_client(w: &mut Writer, id: &ClientId) {
    w.raw(id.as_bytes());
}

fn get_client(r: &mut Reader<'_>, what: &'static str) -> Result<ClientId, DecodeError> {
    Ok(ClientId::from_bytes(r.id16(what)?))
}

fn put_stored(w: &mut Writer, rec: &StoredRecord) {
    w.u64(rec.seq);
    put_client(w, &rec.from);
    w.u64(rec.ctr);
    w.bytes(&rec.record);
}

fn get_stored(r: &mut Reader<'_>) -> Result<StoredRecord, DecodeError> {
    Ok(StoredRecord {
        seq: r.u64("seq")?,
        from: get_client(r, "from")?,
        ctr: r.u64("ctr")?,
        record: r.bytes("record", MAX_RECORD_BYTES)?,
    })
}

/// The smallest a [`StoredRecord`] is on the wire: seq, sender, counter, a one byte length and the smallest record.
const MIN_STORED_BYTES: usize = 8 + 16 + 8 + 1 + SEAL_OVERHEAD;
/// The smallest a [`PresenceEntry`] is on the wire.
const MIN_ENTRY_BYTES: usize = 16 + 1 + SEAL_OVERHEAD;

impl ClientMessage {
    /// Encodes the message as one binary frame. Fails when the message breaks a size limit or a rule.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate()?;
        let frame = match self {
            ClientMessage::Join { min_version, max_version, room, client_id, mode, proof, last_seq, captcha } => {
                let mut w = Writer::new(JOIN_TAG);
                // The version range comes first and its layout never changes (see `JoinVersions`).
                w.u16(*min_version);
                w.u16(*max_version);
                w.raw(room.as_bytes());
                put_client(&mut w, client_id);
                w.u8(mode.as_u8());
                w.u8(proof.scheme().as_u8());
                w.bytes(proof.expose_bytes());
                w.u64(*last_seq);
                w.option(captcha.as_deref(), Writer::str);
                w
            }
            ClientMessage::Append { record } => {
                let mut w = Writer::new(C_APPEND);
                w.bytes(record);
                w
            }
            ClientMessage::PutSnapshot { expected_snapshot_seq, record } => {
                let mut w = Writer::new(C_PUT_SNAPSHOT);
                w.u64(*expected_snapshot_seq);
                w.bytes(record);
                w
            }
            ClientMessage::Presence { record } => {
                let mut w = Writer::new(C_PRESENCE);
                w.bytes(record);
                w
            }
            ClientMessage::Catchup { after_seq } => {
                let mut w = Writer::new(C_CATCHUP);
                w.u64(*after_seq);
                w
            }
            ClientMessage::Direct { to, record } => {
                let mut w = Writer::new(C_DIRECT);
                w.option(to.as_ref(), put_client);
                w.bytes(record);
                w
            }
            ClientMessage::DeleteRoom => Writer::new(C_DELETE_ROOM),
            ClientMessage::Leave => Writer::new(C_LEAVE),
        };
        finish(frame)
    }

    /// Decodes one binary frame. Any input is safe: the result is a message or a typed error.
    ///
    /// A `Join` whose range does not overlap ours is [`DecodeError::UnsupportedVersion`], and one that does not
    /// include version 2 is [`DecodeError::WrongLayout`] (it is a v1 `Join`). Both are returned before the rest of
    /// the frame is read.
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        let (tag, mut r) = split_tag(frame)?;
        let msg = match tag {
            JOIN_TAG => {
                let min_version = r.u16("min_version")?;
                let max_version = r.u16("max_version")?;
                JoinVersions { min: min_version, max: max_version }.check_codec(PROTOCOL_V2)?;
                let room = RoomId::from_bytes(r.id16("room")?);
                let client_id = get_client(&mut r, "client_id")?;
                let raw = r.u8("mode")?;
                let mode = JoinMode::from_u8(raw).ok_or_else(|| unknown("mode", raw))?;
                let raw = r.u8("scheme")?;
                let scheme = ProofScheme::from_u8(raw).ok_or_else(|| unknown("scheme", raw))?;
                let proof = Proof::new(scheme, r.bytes("proof", MAX_PROOF_BYTES)?);
                let last_seq = r.u64("last_seq")?;
                let captcha = r.option("captcha flag", |r| r.string("captcha", MAX_CAPTCHA_BYTES))?;
                ClientMessage::Join { min_version, max_version, room, client_id, mode, proof, last_seq, captcha }
            }
            C_APPEND => ClientMessage::Append { record: r.bytes("record", MAX_RECORD_BYTES)? },
            C_PUT_SNAPSHOT => ClientMessage::PutSnapshot {
                expected_snapshot_seq: r.u64("expected_snapshot_seq")?,
                record: r.bytes("record", MAX_SNAPSHOT_RECORD_BYTES)?,
            },
            C_PRESENCE => ClientMessage::Presence { record: r.bytes("record", MAX_PRESENCE_RECORD_BYTES)? },
            C_CATCHUP => ClientMessage::Catchup { after_seq: r.u64("after_seq")? },
            C_DIRECT => ClientMessage::Direct {
                to: r.option("to flag", |r| get_client(r, "to"))?,
                record: r.bytes("record", MAX_RECORD_BYTES)?,
            },
            C_DELETE_ROOM => ClientMessage::DeleteRoom,
            C_LEAVE => ClientMessage::Leave,
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.finish()?;
        msg.validate()?;
        Ok(msg)
    }
}

impl ServerMessage {
    /// Encodes the message as one binary frame. Fails when the message breaks a size limit or a rule.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate()?;
        let frame = match self {
            ServerMessage::Joined {
                protocol_version,
                room_state,
                head_seq,
                snapshot_seq,
                log_records,
                log_bytes,
                expires_at_ms,
                peers,
                limits,
            } => {
                let mut w = Writer::new(S_JOINED);
                w.u16(*protocol_version);
                w.u8(room_state.as_u8());
                w.u64(*head_seq);
                w.u64(*snapshot_seq);
                w.u32(*log_records);
                w.u32(*log_bytes);
                w.u64(*expires_at_ms);
                w.count(peers.len());
                peers.iter().for_each(|p| put_client(&mut w, p));
                w.u32(limits.max_frame_bytes);
                w.u32(limits.max_record_bytes);
                w.u16(limits.max_updates_per_sec);
                w.u16(limits.max_presence_per_sec);
                w.u32(limits.max_image_bytes);
                w.u64(limits.max_room_image_bytes);
                w.u32(limits.max_room_images);
                w
            }
            ServerMessage::Catchup { snapshot, records, head_seq, more } => {
                let mut w = Writer::new(S_CATCHUP);
                w.option(snapshot.as_ref(), |w, s| {
                    w.u64(s.covers_seq);
                    w.bytes(&s.record);
                });
                w.count(records.len());
                records.iter().for_each(|rec| put_stored(&mut w, rec));
                w.u64(*head_seq);
                w.u8(u8::from(*more));
                w
            }
            ServerMessage::Record(rec) => {
                let mut w = Writer::new(S_RECORD);
                put_stored(&mut w, rec);
                w
            }
            ServerMessage::Ack { ctr, seq } => {
                let mut w = Writer::new(S_ACK);
                w.u64(*ctr);
                w.u64(*seq);
                w
            }
            ServerMessage::PeerJoined { client_id, max_presence_per_sec } => {
                let mut w = Writer::new(S_PEER_JOINED);
                put_client(&mut w, client_id);
                w.u16(*max_presence_per_sec);
                w
            }
            ServerMessage::PeerLeft { client_id } => {
                let mut w = Writer::new(S_PEER_LEFT);
                put_client(&mut w, client_id);
                w
            }
            ServerMessage::PresenceBundle(entries) => {
                let mut w = Writer::new(S_PRESENCE_BUNDLE);
                w.count(entries.len());
                for entry in entries {
                    put_client(&mut w, &entry.from);
                    w.bytes(&entry.record);
                }
                w
            }
            ServerMessage::Error { code, message, retry_after_ms } => {
                let mut w = Writer::new(S_ERROR);
                w.u8(code.as_u8());
                w.str(message);
                w.option(*retry_after_ms, Writer::u32);
                w
            }
            ServerMessage::SnapshotResult { ok, covers_seq, snapshot_seq, head_seq } => {
                let mut w = Writer::new(S_SNAPSHOT_RESULT);
                w.u8(u8::from(*ok));
                w.u64(*covers_seq);
                w.u64(*snapshot_seq);
                w.u64(*head_seq);
                w
            }
            ServerMessage::CompactAdvice { head_seq } => {
                let mut w = Writer::new(S_COMPACT_ADVICE);
                w.u64(*head_seq);
                w
            }
            ServerMessage::Direct { from, record } => {
                let mut w = Writer::new(S_DIRECT);
                put_client(&mut w, from);
                w.bytes(record);
                w
            }
            ServerMessage::Retention { expires_at_ms, expiring } => {
                let mut w = Writer::new(S_RETENTION);
                w.u64(*expires_at_ms);
                w.u8(u8::from(*expiring));
                w
            }
        };
        finish(frame)
    }

    /// Decodes one binary frame. Any input is safe: the result is a message or a typed error.
    pub fn decode(frame: &[u8]) -> Result<Self, DecodeError> {
        let (tag, mut r) = split_tag(frame)?;
        let msg = match tag {
            S_JOINED => {
                let protocol_version = r.u16("protocol_version")?;
                let raw = r.u8("room_state")?;
                let room_state = RoomState::from_u8(raw).ok_or_else(|| unknown("room_state", raw))?;
                let head_seq = r.u64("head_seq")?;
                let snapshot_seq = r.u64("snapshot_seq")?;
                let log_records = r.u32("log_records")?;
                let log_bytes = r.u32("log_bytes")?;
                let expires_at_ms = r.u64("expires_at_ms")?;
                let n = r.count("peers", MAX_PEERS, 16)?;
                let peers = (0..n).map(|_| get_client(&mut r, "peer")).collect::<Result<Vec<_>, _>>()?;
                let limits = RoomLimits {
                    max_frame_bytes: r.u32("limits.max_frame_bytes")?,
                    max_record_bytes: r.u32("limits.max_record_bytes")?,
                    max_updates_per_sec: r.u16("limits.max_updates_per_sec")?,
                    max_presence_per_sec: r.u16("limits.max_presence_per_sec")?,
                    max_image_bytes: r.u32("limits.max_image_bytes")?,
                    max_room_image_bytes: r.u64("limits.max_room_image_bytes")?,
                    max_room_images: r.u32("limits.max_room_images")?,
                };
                ServerMessage::Joined {
                    protocol_version,
                    room_state,
                    head_seq,
                    snapshot_seq,
                    log_records,
                    log_bytes,
                    expires_at_ms,
                    peers,
                    limits,
                }
            }
            S_CATCHUP => {
                let snapshot = r.option("snapshot flag", |r| {
                    Ok(CatchupSnapshot {
                        covers_seq: r.u64("covers_seq")?,
                        record: r.bytes("snapshot", MAX_SNAPSHOT_RECORD_BYTES)?,
                    })
                })?;
                let n = r.count("records", MAX_CATCHUP_RECORDS, MIN_STORED_BYTES)?;
                let records = (0..n).map(|_| get_stored(&mut r)).collect::<Result<Vec<_>, _>>()?;
                ServerMessage::Catchup { snapshot, records, head_seq: r.u64("head_seq")?, more: r.bool("more")? }
            }
            S_RECORD => ServerMessage::Record(get_stored(&mut r)?),
            S_ACK => ServerMessage::Ack { ctr: r.u64("ctr")?, seq: r.u64("seq")? },
            S_PEER_JOINED => ServerMessage::PeerJoined {
                client_id: get_client(&mut r, "client_id")?,
                max_presence_per_sec: r.u16("max_presence_per_sec")?,
            },
            S_PEER_LEFT => ServerMessage::PeerLeft { client_id: get_client(&mut r, "client_id")? },
            S_PRESENCE_BUNDLE => {
                let n = r.count("entries", MAX_BUNDLE_ENTRIES, MIN_ENTRY_BYTES)?;
                let entries = (0..n)
                    .map(|_| {
                        Ok(PresenceEntry {
                            from: get_client(&mut r, "from")?,
                            record: r.bytes("record", MAX_PRESENCE_RECORD_BYTES)?,
                        })
                    })
                    .collect::<Result<Vec<_>, DecodeError>>()?;
                ServerMessage::PresenceBundle(entries)
            }
            S_ERROR => {
                let raw = r.u8("error code")?;
                let code = ErrorCode::from_u8(raw).ok_or_else(|| unknown("error code", raw))?;
                let message = r.string("message", MAX_ERROR_MESSAGE_BYTES)?;
                let retry_after_ms = r.option("retry flag", |r| r.u32("retry_after_ms"))?;
                ServerMessage::Error { code, message, retry_after_ms }
            }
            S_SNAPSHOT_RESULT => ServerMessage::SnapshotResult {
                ok: r.bool("ok")?,
                covers_seq: r.u64("covers_seq")?,
                snapshot_seq: r.u64("snapshot_seq")?,
                head_seq: r.u64("head_seq")?,
            },
            S_COMPACT_ADVICE => ServerMessage::CompactAdvice { head_seq: r.u64("head_seq")? },
            S_DIRECT => ServerMessage::Direct {
                from: get_client(&mut r, "from")?,
                record: r.bytes("record", MAX_RECORD_BYTES)?,
            },
            S_RETENTION => {
                ServerMessage::Retention { expires_at_ms: r.u64("expires_at_ms")?, expiring: r.bool("expiring")? }
            }
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.finish()?;
        msg.validate()?;
        Ok(msg)
    }
}
