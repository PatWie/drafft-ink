//! Protocol v2 messages (`docs/design/collab-e2e.md` section 7.2). Plain data; the codec is in `binary`.
//!
//! A `record` is a sealed record as bytes. The relay reads its header ([`RecordHeader`]) and nothing else.
//! These messages have no JSON debug form: a sealed payload is not readable, and a `Join` carries a secret.

use crate::error::{Rule, ValidationError};
use crate::ids::ClientId;
use crate::limits::*;
use crate::v2::ids::{Proof, RoomId};
use crate::v2::record::{RecordHeader, RecordKind};

/// Whether a `Join` may create the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JoinMode {
    /// Join a room that exists. A missing room is `RoomMissing` and nothing is created, so a mistyped link
    /// does not make a new room.
    Open = 0,
    /// Join the room, or create it when it does not exist.
    Create = 1,
}

impl JoinMode {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(JoinMode::Open),
            1 => Some(JoinMode::Create),
            _ => None,
        }
    }
}

/// Whether the room existed when the joiner arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RoomState {
    /// The room existed.
    Existing = 0,
    /// This `Join` created the room.
    Created = 1,
}

impl RoomState {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(RoomState::Existing),
            1 => Some(RoomState::Created),
            _ => None,
        }
    }
}

/// Why a relay sends an `Error`. The numbers are stable. They agree with [`crate::ErrorCode`] where the meaning is
/// the same, and 15 and above are new in v2. A plaintext (v1) client cannot read the new ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    /// The message could not be decoded or broke a rule.
    Malformed = 1,
    /// No protocol version in common, or this relay does not host this kind of room.
    UnsupportedVersion = 2,
    /// The proof does not match the room id.
    Unauthorized = 3,
    /// The request is understood but not allowed (a disallowed Origin, `DeleteRoom` on a host that refuses it).
    Forbidden = 4,
    /// The room has no space left: people, stored state or pictures.
    RoomFull = 5,
    /// A frame, record or field is larger than the limit.
    TooLarge = 6,
    /// The client sends too fast. `retry_after_ms` says when to try again.
    RateLimited = 7,
    /// Another connection with the same client id joined, so this one is closed.
    Replaced = 9,
    /// The first message was not `Join`.
    NotJoined = 10,
    /// A `Join` arrived on a connection that already joined.
    AlreadyJoined = 11,
    /// The relay cannot serve the room now (storage failure, shutdown). Retry later.
    Unavailable = 13,
    /// A bug in the relay.
    Internal = 14,
    /// `Join` with `mode = Open` for a room that does not exist (never created, or expired).
    RoomMissing = 15,
    /// The room is of the other kind: plaintext (v1) and sealed (v2) rooms do not mix.
    WrongRoomKind = 16,
    /// A compare-and-set failed: `PutSnapshot` expected another snapshot, or a seed found the room not empty.
    Conflict = 17,
    /// The host has used up its capacity for today. `retry_after_ms` says when it resets.
    DailyLimit = 18,
}

impl ErrorCode {
    /// The stable numeric value used in binary frames.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses the numeric value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => ErrorCode::Malformed,
            2 => ErrorCode::UnsupportedVersion,
            3 => ErrorCode::Unauthorized,
            4 => ErrorCode::Forbidden,
            5 => ErrorCode::RoomFull,
            6 => ErrorCode::TooLarge,
            7 => ErrorCode::RateLimited,
            9 => ErrorCode::Replaced,
            10 => ErrorCode::NotJoined,
            11 => ErrorCode::AlreadyJoined,
            13 => ErrorCode::Unavailable,
            14 => ErrorCode::Internal,
            15 => ErrorCode::RoomMissing,
            16 => ErrorCode::WrongRoomKind,
            17 => ErrorCode::Conflict,
            18 => ErrorCode::DailyLimit,
            _ => return None,
        })
    }

    /// Whether the relay closes the connection after sending this code. A client must not retry these without a
    /// change (a new build, a right link, a user action).
    pub const fn closes_connection(self) -> bool {
        matches!(
            self,
            ErrorCode::UnsupportedVersion
                | ErrorCode::Unauthorized
                | ErrorCode::Forbidden
                | ErrorCode::RoomFull
                | ErrorCode::Replaced
                | ErrorCode::NotJoined
                | ErrorCode::AlreadyJoined
                | ErrorCode::Malformed
                | ErrorCode::Internal
                | ErrorCode::RoomMissing
                | ErrorCode::WrongRoomKind
                | ErrorCode::DailyLimit
        )
    }

    /// Whether a client may reconnect on its own after this code.
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::RoomFull
                | ErrorCode::RateLimited
                | ErrorCode::Unavailable
                | ErrorCode::Internal
                | ErrorCode::DailyLimit
        )
    }
}

/// The limits that a relay announces in `Joined`. Every value is at most the matching hard ceiling in
/// [`crate::limits`](crate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomLimits {
    /// Largest frame the relay accepts.
    pub max_frame_bytes: u32,
    /// Largest sealed update the relay accepts.
    pub max_record_bytes: u32,
    /// `Append` messages per second the relay accepts from this client.
    pub max_updates_per_sec: u16,
    /// Presence messages per second this client may send: its share of [`ROOM_PRESENCE_BUDGET`].
    pub max_presence_per_sec: u16,
    /// Largest image before sealing.
    pub max_image_bytes: u32,
    /// Most sealed image bytes in the room.
    pub max_room_image_bytes: u64,
    /// Most images in the room.
    pub max_room_images: u32,
}

impl RoomLimits {
    /// The hard-ceiling defaults.
    pub const DEFAULT: RoomLimits = RoomLimits {
        max_frame_bytes: MAX_FRAME_BYTES as u32,
        max_record_bytes: MAX_RECORD_BYTES as u32,
        max_updates_per_sec: DEFAULT_MAX_UPDATES_PER_SEC,
        max_presence_per_sec: DEFAULT_MAX_AWARENESS_PER_SEC,
        max_image_bytes: MAX_IMAGE_BYTES as u32,
        max_room_image_bytes: MAX_ROOM_IMAGE_BYTES,
        max_room_images: MAX_ROOM_IMAGES,
    };

    /// The personal presence rate for a room of `peers` people: the room budget shared out, at least 1 and at most
    /// the per-client maximum.
    pub fn presence_share(peers: usize) -> u16 {
        let share = usize::from(ROOM_PRESENCE_BUDGET) / peers.max(1);
        u16::try_from(share).unwrap_or(u16::MAX).clamp(1, DEFAULT_MAX_AWARENESS_PER_SEC)
    }

    fn validate(&self) -> Result<(), ValidationError> {
        let above =
            |field, over: bool| if over { Err(ValidationError::new(field, Rule::AboveHardLimit)) } else { Ok(()) };
        above("limits.max_frame_bytes", self.max_frame_bytes as usize > MAX_FRAME_BYTES)?;
        above("limits.max_record_bytes", self.max_record_bytes as usize > MAX_RECORD_BYTES)?;
        above("limits.max_image_bytes", self.max_image_bytes as usize > MAX_IMAGE_BYTES)?;
        above("limits.max_room_image_bytes", self.max_room_image_bytes > MAX_ROOM_IMAGE_BYTES)?;
        above("limits.max_room_images", self.max_room_images > MAX_ROOM_IMAGES)
    }
}

impl Default for RoomLimits {
    fn default() -> Self {
        RoomLimits::DEFAULT
    }
}

/// A message from a client to the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// The first message on a connection: handshake, membership proof and join in one. The layout of the rest of
    /// the frame is the v2 layout, so a client that joins a sealed room sends `min_version == max_version == 2`.
    Join {
        /// Lowest protocol version the client speaks.
        min_version: u16,
        /// Highest protocol version the client speaks.
        max_version: u16,
        /// The room to join.
        room: RoomId,
        /// The client's stable id.
        client_id: ClientId,
        /// Whether the join may create the room.
        mode: JoinMode,
        /// Proof that the client holds the room key. Never printed.
        proof: Proof,
        /// The last relay sequence number the client holds, 0 for none.
        last_seq: u64,
        /// A bot-check token, only for `Create` on a host that asks for one, at most [`MAX_CAPTCHA_BYTES`].
        captcha: Option<String>,
    },
    /// A live update: a sealed record of kind `Update`.
    Append {
        /// The sealed record.
        record: Vec<u8>,
    },
    /// Compaction, or the seed of an empty room: a sealed snapshot, swapped in only if the relay still holds the
    /// snapshot the client expected.
    PutSnapshot {
        /// The `snapshot_seq` the client saw. 0 when the room has no snapshot.
        expected_snapshot_seq: u64,
        /// The sealed record, of kind `Snapshot`.
        record: Vec<u8>,
    },
    /// The client's presence, sealed. Never stored.
    Presence {
        /// The sealed record, of kind `Presence`.
        record: Vec<u8>,
    },
    /// Ask for the records after a sequence number (gap repair).
    Catchup {
        /// The highest sequence number the client holds without a gap below it.
        after_seq: u64,
    },
    /// A peer-to-peer message: a state request or its reply.
    Direct {
        /// The receiver, or `None` for everybody else in the room.
        to: Option<ClientId>,
        /// The sealed record, of kind `Direct`.
        record: Vec<u8>,
    },
    /// Delete the room. Only a host that allows it accepts this (the private instance).
    DeleteRoom,
    /// Leave the room cleanly.
    Leave,
}

/// A sealed snapshot in a `Catchup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchupSnapshot {
    /// The last sequence number the snapshot includes. It equals `covers_seq` in the record's header.
    pub covers_seq: u64,
    /// The sealed record, of kind `Snapshot`.
    pub record: Vec<u8>,
}

/// A stored update, as the relay hands it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecord {
    /// The relay's sequence number.
    pub seq: u64,
    /// The sender, stamped by the relay. It equals `sender` in the record's header.
    pub from: ClientId,
    /// The sender's counter. It equals `ctr` in the record's header.
    pub ctr: u64,
    /// The sealed record, of kind `Update`.
    pub record: Vec<u8>,
}

/// One person's presence in a `PresenceBundle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceEntry {
    /// The sender. It equals `sender` in the record's header.
    pub from: ClientId,
    /// The sealed record, of kind `Presence`.
    pub record: Vec<u8>,
}

/// A message from the relay to a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// Answer to `Join`. A `Catchup` follows at once.
    Joined {
        /// The negotiated protocol version.
        protocol_version: u16,
        /// Whether this join created the room.
        room_state: RoomState,
        /// The newest sequence number in the room.
        head_seq: u64,
        /// The sequence number the stored snapshot covers, 0 for none.
        snapshot_seq: u64,
        /// How many records are stored after the snapshot.
        log_records: u32,
        /// How many bytes those records take.
        log_bytes: u32,
        /// When the room is deleted if nobody visits, in milliseconds since the epoch. 0 means never.
        expires_at_ms: u64,
        /// The other people in the room, by client id only.
        peers: Vec<ClientId>,
        /// Limits that apply to this connection.
        limits: RoomLimits,
    },
    /// What the client lacks: the stored snapshot (if newer than what it holds) and the records after it.
    Catchup {
        /// The snapshot, when the client needs it.
        snapshot: Option<CatchupSnapshot>,
        /// Updates in sequence order, at most [`MAX_CATCHUP_RECORDS`].
        records: Vec<StoredRecord>,
        /// The newest sequence number in the room.
        head_seq: u64,
        /// More records follow: ask again with `Catchup`.
        more: bool,
    },
    /// A live update from another peer. The relay does not echo it to its sender.
    Record(StoredRecord),
    /// The relay has sequenced and stored the update with this counter.
    Ack {
        /// The counter in the record's header.
        ctr: u64,
        /// The sequence number the relay gave it.
        seq: u64,
    },
    /// A person joined.
    PeerJoined {
        /// Their client id.
        client_id: ClientId,
        /// The new personal presence rate. A client takes the lower of its old and new rate.
        max_presence_per_sec: u16,
    },
    /// A person left or lost their connection.
    PeerLeft {
        /// Their client id.
        client_id: ClientId,
    },
    /// Presence of several people, at most one bundle per [`PRESENCE_TICK_MS`].
    PresenceBundle(Vec<PresenceEntry>),
    /// Something went wrong.
    Error {
        /// What kind of failure.
        code: ErrorCode,
        /// Plain words for the user or the log, at most [`MAX_ERROR_MESSAGE_BYTES`] bytes.
        message: String,
        /// For `RateLimited`, `Unavailable` and `DailyLimit`: wait this long before retrying.
        retry_after_ms: Option<u32>,
    },
    /// The answer to a `PutSnapshot`.
    SnapshotResult {
        /// Whether the relay swapped the snapshot in.
        ok: bool,
        /// The cover the client asked for.
        covers_seq: u64,
        /// The sequence number the stored snapshot covers now.
        snapshot_seq: u64,
        /// The newest sequence number in the room.
        head_seq: u64,
    },
    /// The relay asks this client to compact: seal a snapshot and send `PutSnapshot`.
    CompactAdvice {
        /// The newest sequence number in the room.
        head_seq: u64,
    },
    /// A peer-to-peer message from another person.
    Direct {
        /// The sender, stamped by the relay. It equals `sender` in the record's header.
        from: ClientId,
        /// The sealed record, of kind `Direct`.
        record: Vec<u8>,
    },
    /// The room's deadline moved, or the room is past it.
    Retention {
        /// When the room is deleted, in milliseconds since the epoch. 0 means never.
        expires_at_ms: u64,
        /// The room is past its deadline and will be deleted when the last person leaves.
        expiring: bool,
    },
}

impl ServerMessage {
    /// Builds an `Error`, cutting the message to the size limit on a character boundary.
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        let mut message = message.into();
        message.truncate(message.floor_char_boundary(MAX_ERROR_MESSAGE_BYTES));
        ServerMessage::Error { code, message, retry_after_ms: None }
    }
}

// ---- validation ---------------------------------------------------------------------------

/// Checks a sealed record: it is not larger than `max`, it parses and it is of the expected kind. Returns its header.
fn record_field(
    field: &'static str,
    record: &[u8],
    expected: RecordKind,
    max: usize,
) -> Result<RecordHeader, ValidationError> {
    let refuse = |rule| ValidationError::new(field, rule);
    if record.len() > max {
        return Err(refuse(Rule::TooLong));
    }
    let header = RecordHeader::peek(record).map_err(|_| refuse(Rule::BadRecord))?;
    if header.kind == expected { Ok(header) } else { Err(refuse(Rule::WrongRecordKind)) }
}

fn consistent(field: &'static str, ok: bool) -> Result<(), ValidationError> {
    if ok { Ok(()) } else { Err(ValidationError::new(field, Rule::Inconsistent)) }
}

fn check_len(field: &'static str, len: usize, max: usize) -> Result<(), ValidationError> {
    if len > max { Err(ValidationError::new(field, Rule::TooLong)) } else { Ok(()) }
}

impl StoredRecord {
    fn validate(&self, field: &'static str) -> Result<(), ValidationError> {
        let header = record_field(field, &self.record, RecordKind::Update, MAX_RECORD_BYTES)?;
        consistent(field, header.sender == self.from && header.ctr == self.ctr)
    }
}

impl ClientMessage {
    /// Checks every size limit and value rule. Encoders and decoders call this.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            ClientMessage::Join { min_version, max_version, proof, captcha, .. } => {
                if min_version > max_version {
                    return Err(ValidationError::new("version range", Rule::InvertedRange));
                }
                check_len("proof", proof.expose_bytes().len(), MAX_PROOF_BYTES)?;
                if let Some(captcha) = captcha {
                    check_len("captcha", captcha.len(), MAX_CAPTCHA_BYTES)?;
                }
                Ok(())
            }
            ClientMessage::Append { record } => {
                record_field("record", record, RecordKind::Update, MAX_RECORD_BYTES).map(drop)
            }
            ClientMessage::PutSnapshot { record, .. } => {
                record_field("record", record, RecordKind::Snapshot, MAX_SNAPSHOT_RECORD_BYTES).map(drop)
            }
            ClientMessage::Presence { record } => {
                record_field("record", record, RecordKind::Presence, MAX_PRESENCE_RECORD_BYTES).map(drop)
            }
            ClientMessage::Direct { record, .. } => {
                record_field("record", record, RecordKind::Direct, MAX_RECORD_BYTES).map(drop)
            }
            ClientMessage::Catchup { .. } | ClientMessage::DeleteRoom | ClientMessage::Leave => Ok(()),
        }
    }
}

impl ServerMessage {
    /// Checks every size limit and value rule. Encoders and decoders call this.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            ServerMessage::Joined { snapshot_seq, head_seq, peers, limits, .. } => {
                check_len("peers", peers.len(), MAX_PEERS)?;
                consistent("snapshot_seq", snapshot_seq <= head_seq)?;
                limits.validate()
            }
            ServerMessage::Catchup { snapshot, records, .. } => {
                check_len("records", records.len(), MAX_CATCHUP_RECORDS)?;
                if let Some(snapshot) = snapshot {
                    let header =
                        record_field("snapshot", &snapshot.record, RecordKind::Snapshot, MAX_SNAPSHOT_RECORD_BYTES)?;
                    consistent("snapshot", header.covers_seq == snapshot.covers_seq)?;
                }
                records.iter().try_for_each(|r| r.validate("records"))
            }
            ServerMessage::Record(record) => record.validate("record"),
            ServerMessage::PresenceBundle(entries) => {
                check_len("entries", entries.len(), MAX_BUNDLE_ENTRIES)?;
                entries.iter().try_for_each(|entry| {
                    let header =
                        record_field("entries", &entry.record, RecordKind::Presence, MAX_PRESENCE_RECORD_BYTES)?;
                    consistent("entries", header.sender == entry.from)
                })
            }
            ServerMessage::Direct { from, record } => {
                let header = record_field("record", record, RecordKind::Direct, MAX_RECORD_BYTES)?;
                consistent("record", header.sender == *from)
            }
            ServerMessage::Error { message, .. } => check_len("message", message.len(), MAX_ERROR_MESSAGE_BYTES),
            ServerMessage::Ack { .. }
            | ServerMessage::PeerJoined { .. }
            | ServerMessage::PeerLeft { .. }
            | ServerMessage::SnapshotResult { .. }
            | ServerMessage::CompactAdvice { .. }
            | ServerMessage::Retention { .. } => Ok(()),
        }
    }
}
