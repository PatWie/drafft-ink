# Collaboration protocol

Crate: `crates/protocol` (`drafftink-protocol`, layer L0). Users: the sync client in `crates/collab` and the server in `apps/drafftink-server` (`drafftink-headless serve`). Protocol versions: **1**, plaintext rooms (sections 1 to 10), and **2**, sealed end-to-end encrypted rooms (section 11). A build speaks both (`MIN_PROTOCOL_VERSION = 1`, `PROTOCOL_VERSION = 2`). Nothing in sections 1 to 10 changed when version 2 was added, except what section 2 says about `Join`.

The protocol replaces the old scheme, where every local edit cleared and re-added every shape and sent a full base64 Loro snapshot, the server kept only the last snapshot ("last writer wins"), and every received snapshot rebuilt the whole document (2 s at 1k shapes). Now a local edit sends only the Loro update for that edit, a late joiner or a reconnecting client receives only what its version vector lacks, and the receiving document applies the update as a patch.

The protocol crate knows nothing about Loro. Updates, snapshots and version vectors are opaque byte strings. It builds for `wasm32-unknown-unknown` and depends only on `serde`, `serde_json`, `thiserror` and, for the room id of version 2 (a hash of the membership token, section 11), `sha2`, `subtle`, `zeroize` and `base64ct`.

## 1. Transport and framing

- One WebSocket per client per room, path `/ws`. Use `wss://` outside localhost.
- One message per WebSocket **binary** frame. The binary form is the only form a production server accepts.
- A JSON debug form exists (`to_json` / `from_json`): `{"type":"update","seq":3,"update":"dead"}`. Byte fields are hex strings. It is for logs, tests and manual debugging. A server accepts text frames only when started with `--debug-json` (off by default), and a client never sends them.
- A frame is `tag: u8` plus the message fields in the order below. Integers are little-endian and fixed width. Lengths and counts are LEB128 varints of at most 5 bytes. A byte string or a text is `varint length` then the raw bytes (UTF-8 for text). A flag or bool is one byte, 0 or 1; any other value is an error. Update payloads are raw bytes, so a 1000 byte update costs 1000 bytes plus 7 bytes of envelope (base64 added 33%).
- Decoding is strict: unknown tags, unknown enum values, unknown flag bits, trailing bytes, invalid UTF-8, non-finite numbers, and every length above its limit are errors with a typed `DecodeError`. A length is checked against the field limit and against the bytes that remain before anything is allocated.
- Client tags are below 0x80 and server tags are 0x80 and above, so a frame sent in the wrong direction fails with `UnknownTag`.

### Client to server

| Tag | Message | Fields |
|---|---|---|
| 0x01 | `Join` | `min_version u16`, `max_version u16`, `room str`, `client_id [16]`, `auth_token opt str`, `version bytes`, `user opt UserInfo` |
| 0x02 | `Update` | `seq u32`, `update bytes` |
| 0x03 | `RequestSync` | `version bytes` |
| 0x04 | `Awareness` | `AwarenessState` |
| 0x05 | `Ping` | `nonce u64` |
| 0x06 | `Pong` | `nonce u64` |
| 0x07 | `Leave` | none |

### Server to client

| Tag | Message | Fields |
|---|---|---|
| 0x81 | `Joined` | `protocol_version u16`, `server str`, `room str`, `client_id [16]`, `peers list PeerInfo`, `server_version bytes`, `limits` (`max_frame_bytes u32`, `max_update_bytes u32`, `max_updates_per_sec u16`, `max_awareness_per_sec u16`) |
| 0x82 | `SyncResponse` | `kind u8` (0 updates, 1 snapshot), `data bytes`, `server_version bytes` |
| 0x83 | `Update` | `from [16]`, `update bytes` |
| 0x84 | `Ack` | `seq u32` |
| 0x85 | `PeerJoined` | `PeerInfo` |
| 0x86 | `PeerLeft` | `client_id [16]` |
| 0x87 | `PeerAwareness` | `from [16]`, `AwarenessState` |
| 0x88 | `Error` | `code u8`, `message str`, `retry_after_ms opt u32` |
| 0x89 | `Ping` | `nonce u64` |
| 0x8a | `Pong` | `nonce u64` |

Shared shapes: `UserInfo = name str, color u32 (0xRRGGBB)`. `PeerInfo = client_id [16], user opt UserInfo`. `AwarenessState` = a flags byte (1 user, 2 cursor, 4 viewport, 8 selection), then the present parts in that order: user, cursor (`x f64`, `y f64`), viewport (`x`, `y`, `width`, `height`, all `f64`), selection (`count`, then `count` strings).

## 2. Versions and the handshake

`Join` is the hello. Its first five bytes (tag, `min_version`, `max_version`) keep their layout in every protocol version. A host reads them with `JoinVersions::peek` without reading the rest, picks the highest version both sides speak (`negotiate_version`, or `negotiate_version_within` for a host that speaks fewer versions) and returns it in `Joined.protocol_version`. It then hands the frame to the decoder of that version: `ClientMessage::decode` for 1 and `v2::ClientMessage::decode` for 2. If the ranges do not overlap, the host stops reading the frame (a newer client may have changed the rest of the layout), sends `Error { code: UnsupportedVersion }` whose message names the versions it speaks, and closes. The client must not retry that error until it is a different build.

A `Join` lays out the rest of its fields for one version, and a client cannot know before the answer which version the host picks. So a client sends `min_version == max_version`: 1 for a plaintext room, 2 for a room whose link has a key. The kind of room decides, and a room is plaintext or sealed for its whole life. A decoder that gets the other version's `Join` returns `DecodeError::WrongLayout` (error code `UnsupportedVersion`), and the version 1 decoder does the same for a range that does not include 1. A range that includes both (for example 1..=2) is read in the layout of the decoder the host picked.

A version bump is needed for any change to a frame layout, a tag, an enum value, an error code, or a limit that clients rely on. Adding behavior that does not change bytes does not need one. `MIN_PROTOCOL_VERSION` rises only when an old layout is dropped. Version 2 has its own `ErrorCode` enum, so the version 1 enum (numbers 1 to 14) is unchanged.

The first message on a connection must be `Join` (otherwise `NotJoined`), and a second `Join` is `AlreadyJoined`. The server closes a connection that sends no `Join` within 10 seconds.

## 3. Message flow

### Join and catch-up

```
client                                     server
  | Join{room, client_id, version=VV_c} ---> |  check version, origin, auth, room capacity
  | <--- Joined{peers, server_version=VV_s}  |
  | <--- SyncResponse{kind, data, VV_s'}     |  what VV_c lacks (Updates), or a Snapshot
  |  import data into the doc (patches)      |
  | Update{seq, changes since VV_s'} ------> |  the client's own changes the server lacks
  | <--- Ack{seq}                            |
  | Awareness{...} ------------------------> |  presence starts
```

- `version` in `Join` is the client's Loro version vector (Loro's `VersionVector::encode()`), opaque to the protocol. Empty means "I have nothing": the server answers with a snapshot. A version vector, not frontiers, is the contract: frontiers cannot tell the server which operations a peer lacks.
- The server computes `SyncResponse.data` from the client's version vector. It sends `kind = Updates` when the diff is small, and `kind = Snapshot` when the client has no history, when the diff would be larger than a snapshot, or when the client's version contains operations the server cannot place. The client imports both kinds the same way (Loro import merges). The kind tells the client how to react: a snapshot on a document with many shapes should be imported as one bulk patch, not shape by shape.
- `Joined.server` is the server's software and version for humans, `drafftink-headless/<version>` (it read `drafftink-server/<version>` before D18). It is informational: the sync client in `crates/collab` ignores it, and no client may branch on it.
- `Joined.server_version` and `SyncResponse.server_version` tell the client what the server has. The client exports its own updates since that version and sends them as `Update`. This is the upload half of the sync, and it is what makes edits made offline arrive.
- `Joined` is followed at once by a `SyncResponse`. `Joined` is small and carries the peer list and limits. The potentially large payload is in the second message.

### Live edits

```
client A                      server                        client B
  | Update{seq, bytes} -----> | merge into room doc           |
  | <--- Ack{seq}             | queue for the RoomStore       |
  |                           | Update{from=A, bytes} ------> | import (patch)
```

- One `Update` is the Loro update bytes for one local commit (or several commits coalesced by the client over at most about 30 ms). It is not a snapshot and not a diff against a version the server must hold.
- The server stamps `from`. A client never names its own id in a relayed message.
- The server never echoes an `Update` to its sender. `Ack` confirms that the update was merged into the room document. Durability follows within the store's flush interval (see §6). The client may use `Ack` for a "saved" indicator and to bound its unacknowledged queue; it does not need it for correctness because the version vector exchange on reconnect repairs any loss.
- If the server cannot import the bytes, it replies `InvalidUpdate` and does not relay them. The connection stays open.
- An update that raises `meta.schema` above the schema the server serves (`drafftink_server::schema::SERVED_SCHEMA`, the `SCHEMA_VERSION` of the document crate) is also answered with `InvalidUpdate`, with a message that names both numbers and says to update the app. The server rebuilds the room from before the update, so the update is neither relayed nor stored.

### Gaps and slow receivers

A WebSocket is ordered and reliable, so a gap only happens when the server drops messages for a slow receiver. Then the server sends `Error { ResyncRequired }` (the connection stays open) and drops that client's queued relays. The client answers with `RequestSync { version }` and the server replies with a `SyncResponse` computed from it. This replaces the old behavior, where a lagged receiver silently lost messages.

### Reconnect and resume

The client keeps its document and its `client_id`. After a drop it reconnects and sends `Join` with its **current** version vector. It does not start over. The server replies with only the missing operations, and the client uploads what it made while offline. Edits made during the outage are never lost, and the document is never rebuilt.

- Backoff: `reconnect_delay_ms(attempt, jitter)` gives 500 ms doubling to 30 s, with half of each delay randomized, so a crowd does not return together. A `RateLimited` or `Unavailable` error with `retry_after_ms` overrides it.
- If a second connection joins with the same `client_id` (reconnect before the server noticed the old socket died), the server closes the old one with `Error { Replaced }` and keeps the new one. A client that receives `Replaced` stops. It was replaced by itself in another tab or by a stolen id, and must not fight for the slot.
- Tabs of one browser use different `client_id`s. They are separate peers.
- Awareness is not resumed. The client sends its full awareness again after `Joined`.

### Leaving

`Leave` (or a closed socket) makes the server broadcast `PeerLeft`. A room with no peers stays loaded for the store's idle time, then is flushed and unloaded.

## 4. Awareness is not the document

| | Document updates | Awareness |
|---|---|---|
| Content | Loro operations | cursor, selection (element ids), viewport, user name and colour |
| Lifetime | forever, persisted | until the next message, never persisted |
| Delivery | reliable, every one matters, merged by the CRDT | latest wins, may be dropped |
| Rate | up to 60 per second per client | up to 20 per second per client |
| After reconnect | resumed from the version vector | sent again from scratch |
| Server | decodes, merges, stores | validates size, stamps `from`, relays, forgets |

- Each `Awareness` message replaces the sender's whole previous state. There are no partial updates and no history.
- Throttle: a client sends at most one awareness message per `AWARENESS_MIN_INTERVAL_MS` (50 ms), and only when something changed. A client resends its state every `AWARENESS_HEARTBEAT_MS` (10 s) while connected. A receiver removes a peer's cursor after `AWARENESS_TIMEOUT_MS` (30 s) of silence. A silent idle client therefore costs one small message per 10 s. A client that sets `cursor` to `None` hides its pointer.
- The server keeps the latest awareness of each connected peer only to fill `PeerInfo.user` for later joiners. It does not send awareness to late joiners; the next heartbeat does, within 10 s. (A server may choose to replay the latest states on join; clients must accept this.)
- Selections are lists of element ids, at most 512 ids of at most 64 bytes.

## 5. Limits

Hard ceilings are constants in `drafftink-protocol`. Decoders enforce them, and encoders refuse to build a frame above them (`EncodeError`). A server may announce lower values in `Joined.limits`.

| Limit | Value |
|---|---|
| Frame (one binary message) | 16 MiB (`MAX_FRAME_BYTES`) |
| JSON debug message | 32 MiB |
| `Update` payload | 4 MiB |
| `SyncResponse` payload | 15 MiB |
| Version vector blob | 256 KiB |
| Room id | 128 bytes: letters, digits, `-`, `_`, `.` |
| Auth token | 4096 bytes |
| User name | 64 bytes; colour fits `0xRRGGBB` |
| Selection | 512 ids of at most 64 bytes |
| Peers in `Joined` | 256 |
| Error message | 512 bytes |
| Updates per client | 60 per second (token bucket, burst 2x) |
| Awareness per client | 20 per second |
| Ping interval / idle timeout | 20 s / 50 s |

Server behavior on a breach: an oversize frame or field gets `TooLarge` and the connection closes. Too many messages get `RateLimited` with `retry_after_ms`, and the excess messages are dropped (for an `Update`, the client must resend it, so it keeps the update queued and retries after the delay). Repeated breaches close the connection. Room capacity is a server option (`RoomFull`).

Images are embedded in the document today, so a document with large images produces large snapshots. If a snapshot would exceed the `SyncResponse` limit, the server replies `TooLarge` to the join. Moving images out of the document into content-addressed blobs is a later protocol version.

## 6. Server persistence: the RoomStore trait

The server (not the protocol crate) holds one Loro document per loaded room. It depends on Loro to merge updates and to compute `SyncResponse` from a client's version vector. Storage sits behind a `RoomStore` trait in `apps/drafftink-server`. The protocol only fixes the contract below, so every provider behaves the same and shares one conformance test suite.

```rust
/// Durable storage for rooms. Async, object safe, errors are values.
trait RoomStore: Send + Sync {
    /// Latest compacted snapshot and the update log written after it. None for a new room.
    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError>;
    /// Append one opaque update to the room's log. Must be durable when it returns.
    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError>;
    /// Replace the snapshot with `snapshot` (which includes the first `covered` log entries)
    /// and drop those entries. Atomic: a crash leaves the old or the new state, never a mix.
    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError>;
    /// Delete a room.
    async fn delete(&self, room: &RoomId) -> Result<(), StoreError>;
    /// Room ids with their last-modified time, for expiry.
    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError>;
}
struct StoredRoom { snapshot: Option<Vec<u8>>, updates: Vec<Vec<u8>> }
```

Providers, chosen by config, filesystem by default:

| Provider | Layout | Notes |
|---|---|---|
| `memory` | maps in process | tests and throwaway servers; lost on restart |
| `filesystem` | `<dir>/<room>/snapshot.bin` and `updates/<n>.bin`; write to a temp file, `fsync`, rename | default |
| `sqlite` | one table of snapshots and one append-only table of updates, WAL mode | one writer transaction per compaction |
| `s3` | objects `rooms/<room>/snapshot` and `rooms/<room>/updates/<n>`; any S3-compatible service | batching matters: see the flush interval |

Handoff, in order:
1. A `Join` for an unloaded room calls `load`, imports the snapshot and then the log into a fresh Loro document, and starts serving.
2. Each valid `Update` is merged in memory, then queued for `append_update`. The server flushes the queue every flush interval (default 2 s; 0 for the filesystem and sqlite providers means "before `Ack`") and when the room unloads. `Ack` means "merged". It means "durable" only when the flush interval is 0.
3. When the log passes a size or count threshold (default 1000 entries or 8 MiB), the server calls `compact` with a fresh snapshot of the room document.
4. A room with no peers is flushed, compacted and unloaded after the idle time (default 10 min). Rooms untouched for the expiry time (default off) are deleted.
5. Because every entry is an update that Loro can merge in any order and any number of times, a crash between steps never corrupts a room. At worst the last flush interval of edits is missing from the server. The clients still hold them, and they upload them on their next join.

The server never trusts stored or received bytes: an import that fails is `InvalidUpdate` (live) or a failed load (storage), and a failed load makes the join answer `Unavailable` and does not overwrite the stored room.

## 7. Authentication and access

- Auth is **off by default**. A room id is a capability: clients generate it from at least 128 random bits (`RoomId::from_random_bytes`, 32 hex characters), and whoever knows it can join. This matches the "no account" promise.
- With `--auth` (server flag), `Join.auth_token` is required and the server checks it (the checker is pluggable: a static shared secret, or a signed token with room and expiry claims). A missing or bad token gets `Unauthorized` and the connection closes. With auth off the token is ignored and never logged.
- Tokens travel inside the first binary frame, not in the URL, so they stay out of proxy logs.

## 8. Security notes

- **Use `wss://`.** Frames are not encrypted by the protocol. The server expects a TLS-terminating proxy or its own TLS config. A client refuses plain `ws://` to a non-loopback host unless the user opts in.
- **Origin check.** The server checks the `Origin` header during the WebSocket upgrade against an allow-list (`--allowed-origin`, repeatable; default: the server's own origin and no cross-origin pages). A request without `Origin` (native clients) is allowed. A disallowed Origin gets HTTP 403 before the upgrade. This stops a hostile web page from joining rooms through a visitor's browser.
- **Every input is hostile.** Decoders return typed errors for all malformed input and never panic. The server also drops a connection after repeated malformed frames, limits the size of every field before allocating, and rate-limits per connection.
- **The server stamps identities.** `from` is set by the server. A client cannot speak for another peer, but any peer in a room can write to its document: there are no per-peer permissions in version 1, and a room's participants are fully trusted with its content. Anyone who joins can read and edit. Use a hard-to-guess room id, or turn auth on.
- **Opaque payloads.** The server must import update bytes only through Loro's own validating import, in a way that cannot panic or exhaust memory beyond the size limits (run it under a task that catches failure and answers `InvalidUpdate`).
- **No telemetry.** The server logs connection counts and errors, not document content or tokens.

## 9. How remote updates enter the client

The sync client (`crates/collab`, layer L5) owns the socket and the protocol state. It never touches the scene.

1. A received `Update` or `SyncResponse` becomes `doc.apply_remote(bytes)` on the doc layer (L3). The doc layer imports the bytes into the Loro document, reads the Loro diff events that import produces, and turns them into **incremental patches** for the scene store: added, changed and removed elements, and z-order moves. Only the elements named in the diff are touched. Nothing is cleared or rebuilt. The per-element render caches, the spatial index and the derived indexes (bound arrows, groups, frames) update only for those elements.
2. A `SyncResponse` of kind `Snapshot` goes through the same call. The diff is then large, and the doc layer applies it as one bulk patch.
3. The engine (L5) runs remote changes through the same path as local ones, but marks them. Each applied remote batch is recorded in the **recent ring** as an entry with `origin: remote`, the sender's `client_id` and user name, the number of elements added, changed and removed, and the time. The ring shows them next to local commands, and agents can see that the scene changed by someone else. Remote changes are not commands: they are not executed by the registry, they do not enter this client's undo stack (undo only reverts local operations; Loro's undo manager already does this), and they are not sent back. What the merge itself leaves broken (an arrow off its targets, a binding to a deleted shape, two labels for one shape, inconsistent groups) is repaired by the engine right after (docs/crdt.md section 10a). The repair is a deterministic function of the merged scene, so every client writes the same values and the clients stop writing after one round. Its writes are local operations of the repairing client with a system origin: they are sent like any local update, are on no undo stack, and appear in the ring as `remote.repair` with `origin: remote`.
4. If a remote patch removes or changes something the local user holds (selection, an element being dragged or edited), the engine reconciles after applying: it drops removed and deleted ids from the selection and the point editor, and cancels a running gesture that was changing an element the patch deleted or removed (`Session::cancel_gesture_on_removed`: the gesture ends with a `CollabNotice::Info` such as "Move was cancelled: someone else deleted an element you were changing.", the ring gets `gesture.cancel` with origin `remote`, and the working copy is read again from the replicated document, so the delete stands). Other changes to an element being dragged do not end the gesture: the working copy is kept until the gesture ends, and the commit then merges through the CRDT (docs/engine.md, "Known limits"). A text edit in progress is not cancelled.
5. The patch marks the affected region dirty, which is the only reason to redraw. A remote edit wakes an idle app; nothing else does.

Local edits leave in the reverse direction: each commit of the doc layer yields the exported Loro update since the last export, and the sync client sends it as an `Update`. A command that changes nothing sends nothing.

## 10. Testing

`crates/protocol/tests/` holds (version 1; version 2 is in `v2.rs`, section 11): round trips for every message in binary and JSON; truncation of every prefix of every frame; every single-bit flip; 30k random mutations and 50k random frames; oversize and overlong length prefixes; invalid UTF-8, flags and numbers; hostile JSON (deep nesting, wrong types, oversize); and version negotiation, including the "stop before reading the rest" rule. The invariant checked on every decoded frame: encoding it again and decoding the result gives the same message.

## 11. Protocol v2: sealed rooms

The design is `docs/design/collab-e2e.md` (sections 4 to 11). In short: the room key lives in the link after `#`, so no server sees it. Every update, snapshot, presence message and image is a *sealed record* made by `crates/seal` (`drafftink-seal`, L1: XChaCha20-Poly1305, random 24-byte nonces, HKDF-SHA256 subkeys). The relay checks membership, numbers records, stores them, and fans them out. It never reads a payload and never imports a Loro blob. Code: `crates/protocol/src/v2/`. Version 2 has no JSON debug form: a payload is not readable, and a `Join` carries a secret.

Routes: `wss://host/r/<room_id>/ws` and `https://host/r/<room_id>/f/<name>` (images, phase 2). The room id is in the path so that a Worker can pick the room before it reads a frame. The v1 route `/ws` stays. A text frame `ping` (every 20 s) is answered with `pong`; other text frames are refused (`KEEPALIVE_PING`, `KEEPALIVE_PONG`).

### Names and proofs

- `v2::RoomId`: 16 bytes, written as 22 characters of unpadded base64url. It is `SHA-256("drafft v1 room-id" || membership token)[..16]`. It is not the v1 `RoomId` (a name of up to 128 characters).
- `v2::MembershipToken`: 32 bytes, HKDF subkey `"drafft v1 membership"`. A relay accepts a join when `RoomId::accepts(&proof)` holds (constant time). It stores nothing and learns nothing about the key. It sees the token on the wire, so a relay can act as a member towards itself. Scheme 2 (Ed25519, later) removes that.
- `v2::Proof` is the `scheme` byte and the proof bytes of `Join`. Scheme 1 is the token. A proof of another length is representable (a relay answers `Unauthorized`). It is wiped on drop, and `Debug` does not print it. The same holds for `MembershipToken`.
- `v2::ImageName`: 16 bytes, 22 characters. `HMAC-SHA256(K_name, file_id)[..16]`, where `file_id` is the 32 raw bytes of the SHA-256 of the picture.

### The sealed record header

```
SealedRecord = header || nonce[24] || ciphertext || tag[16]
header       = version u8 (=1) | kind u8 | sender [16] | ctr u64 LE | covers_seq u64 LE      (34 bytes)
```

`kind`: 1 update, 2 snapshot, 3 presence, 4 direct, 5 image. `ctr` counts the sender's records. `covers_seq` is set on snapshots only. An image has a zero sender and counter. A relay reads the header with `RecordHeader::peek` and refuses a record that is shorter than `SEAL_OVERHEAD` (78 bytes: 34 + 24 + 4 for the length prefix inside the ciphertext + 16), has another format version or an unknown kind, or sets a field that must be zero. The header is in the associated data of the cipher, so a relay cannot change it. Each message checks the kind of the record it carries. Where a message repeats a header field (`from`, `ctr`, `covers_seq`), the decoder checks that they agree.

### Client to relay

| Tag | Message | Fields |
|---|---|---|
| 0x01 | `Join` | `min_version u16`, `max_version u16`, `room [16]`, `client_id [16]`, `mode u8` (0 open, 1 create), `scheme u8`, `proof bytes` (at most 256), `last_seq u64`, `captcha opt str` (at most 4096) |
| 0x10 | `Append` | `record` (kind 1) |
| 0x11 | `PutSnapshot` | `expected_snapshot_seq u64`, `record` (kind 2) |
| 0x12 | `Presence` | `record` (kind 3) |
| 0x13 | `Catchup` | `after_seq u64` |
| 0x14 | `Direct` | `to opt [16]`, `record` (kind 4) |
| 0x15 | `DeleteRoom` | none. Only a host that allows it accepts it. |
| 0x07 | `Leave` | none |

### Relay to client

| Tag | Message | Fields |
|---|---|---|
| 0x81 | `Joined` | `protocol_version u16`, `room_state u8` (0 existing, 1 created), `head_seq u64`, `snapshot_seq u64`, `log_records u32`, `log_bytes u32`, `expires_at_ms u64` (0 never), `peers list [16]`, `limits` (`max_frame_bytes u32`, `max_record_bytes u32`, `max_updates_per_sec u16`, `max_presence_per_sec u16`, `max_image_bytes u32`, `max_room_image_bytes u64`, `max_room_images u32`) |
| 0x82 | `Catchup` | `snapshot opt {covers_seq u64, record}`, `records list {seq u64, from [16], ctr u64, record}` (at most 64), `head_seq u64`, `more bool` |
| 0x83 | `Record` | `seq u64`, `from [16]`, `ctr u64`, `record` (live; not echoed to the sender) |
| 0x84 | `Ack` | `ctr u64`, `seq u64` (the record is sequenced and stored) |
| 0x85 | `PeerJoined` | `client_id [16]`, `max_presence_per_sec u16` |
| 0x86 | `PeerLeft` | `client_id [16]` |
| 0x87 | `PresenceBundle` | `list {from [16], record}` (at most 256, one bundle per `PRESENCE_TICK_MS`) |
| 0x88 | `Error` | `code u8`, `message str`, `retry_after_ms opt u32` |
| 0x89 | `SnapshotResult` | `ok bool`, `covers_seq u64`, `snapshot_seq u64`, `head_seq u64` |
| 0x8a | `CompactAdvice` | `head_seq u64` |
| 0x8b | `Direct` | `from [16]`, `record` |
| 0x8c | `Retention` | `expires_at_ms u64`, `expiring bool` |

Integers, lengths, options and strictness are as in section 1. The design's table has `PeerJoined` with a client id only, and its section 11.3 says the new personal presence rate travels with `PeerJoined`; the field is there (`RoomLimits::presence_share(peers)` computes it, and a client keeps the lower of its old and new rate).

### Error codes (`v2::ErrorCode`)

1 `Malformed`, 2 `UnsupportedVersion`, 3 `Unauthorized`, 4 `Forbidden`, 5 `RoomFull`, 6 `TooLarge`, 7 `RateLimited`, 9 `Replaced`, 10 `NotJoined`, 11 `AlreadyJoined`, 13 `Unavailable`, 14 `Internal` have the numbers and meaning of version 1. New: 15 `RoomMissing` (`open` on a room that does not exist), 16 `WrongRoomKind` (a plaintext room joined as sealed or the other way round), 17 `Conflict` (a compare-and-set failed), 18 `DailyLimit` (the host's capacity, with `retry_after_ms`). `ResyncRequired` (8) and `InvalidUpdate` (12) do not exist in version 2: the relay cannot import a payload, and a client repairs a gap with `Catchup`. A version 1 client cannot read codes 15 to 18, so a host that answers a version 1 `Join` for a sealed room uses a version 1 code.

### Limits of version 2

Hard ceilings are constants in `drafftink-protocol` (`limits.rs`); a relay may announce lower values in `Joined.limits`. The encoders refuse to build a frame above them, and the decoders enforce them before they allocate.

| Constant | Value |
|---|---|
| `MAX_FRAME_BYTES` | 16 MiB (as version 1) |
| `MAX_RECORD_BYTES` (update, direct) | 4 MiB |
| `MAX_SNAPSHOT_RECORD_BYTES` | 15 MiB (`MAX_SYNC_BYTES`) |
| `MAX_PRESENCE_RECORD_BYTES` | 64 KiB |
| `MAX_CATCHUP_RECORDS`, `CATCHUP_TARGET_BYTES` | 64 records; a relay stops adding records past 2 MiB and sets `more` (always at least one record) |
| `MAX_IMAGE_BYTES` | 4 MiB before sealing; `MAX_SEALED_IMAGE_BYTES` is that plus `SEAL_OVERHEAD` |
| `MAX_ROOM_IMAGE_BYTES`, `MAX_ROOM_IMAGES` | 50 MiB of sealed images, 200 images |
| `MAX_ROOM_STATE_BYTES`, `SELF_HOST_ROOM_STATE_BYTES` | 32 MiB (the demo), 256 MiB (self-host default) |
| `MAX_PEERS_PER_ROOM` | 64 (the protocol maximum is `MAX_PEERS`, 256) |
| `ROOM_PRESENCE_BUDGET`, `PRESENCE_TICK_MS` | 40 messages per second for the room, a bundle every 100 ms |
| Per connection | `Append` 60 per second with burst 2x (`DEFAULT_MAX_UPDATES_PER_SEC`), `PutSnapshot` once per `PUT_SNAPSHOT_MIN_INTERVAL_MS` (10 s), `Catchup` once per `CATCHUP_MIN_INTERVAL_MS` (1 s), `Direct` `MAX_DIRECT_PER_SEC` (5) per second, `Presence` by `Joined.limits.max_presence_per_sec`. A room: `MAX_ROOM_UPDATES_PER_SEC` (200). |
| `ROOM_TTL`, `ROOM_LEAVE_GRACE`, `ROOM_RECHECK` | 48 h after the last visit (an accepted `Join`), 60 s after the last connection closes, 1 h (design section 10). A private instance sets no TTL. |

### Tests

`crates/protocol/tests/v2.rs`: round trips and the wire bytes of every message; every truncation, every single-bit flip, 30,000 mutations and 50,000 random frames; huge length prefixes; each way a record can be wrong for its message; the `Join` handshake (same first five bytes, negotiation, `WrongLayout`); the membership check; error codes; the values of the constants. `crates/seal/tests/` holds the known-answer, tamper, replay, nonce, link, hostile and logging tests of the cipher side (`crates/seal/tests/vectors/README.md`).
