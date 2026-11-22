# Design: end-to-end encrypted collaboration rooms

Status: proposal for review. Nothing here is implemented. Date: 2026-11-18; section 19 (private instance) and the phase table were added 2026-11-22. Author: design agent for the drafftink rewrite.

This document designs encrypted rooms for drafft.ink. The room key lives in the URL fragment, so no server ever sees it. The demo server is one Cloudflare Durable Object per room on the Workers Free plan. The same relay core also runs in `drafftink-headless serve`.

Facts about Cloudflare and crypto crates were read from the linked pages on 2026-11-18. "Unverified" marks anything I could not confirm. Day estimates are my own.

Contents: 1 Summary, 2 What exists today, 3 Excalidraw in short, 4 Threat model, 5 Keys, 6 Crypto, 7 Wire protocol v2, 8 Sync and catch-up, 9 Images, 10 Retention, expiry and reseed, 11 Abuse and cost control, 12 Native and mobile, 13 Command layer, 14 Crates and hosts, 15 Testing, 16 Phases and effort, 17 Risks and surprises, 18 Open questions, 19 Private instance (canvas.patwie.com), 20 Sources.

---

## 1. Summary

**Decisions of the user (binding), and how this design meets them**

| # | Decision | Design |
|---|---|---|
| 1 | E2E first, key in the fragment | `https://drafft.ink/#room=<id>,<key>`. 256-bit key. XChaCha20-Poly1305 seals every update, snapshot, presence message and image. The relay stores and forwards bytes it cannot read. |
| 2 | Demo on one Durable Object per room, Free plan | SQLite-backed DO (the only kind on Free). The relay does no crypto and no Loro work, so the DO stays small. Paid plan is a plan change: no code path depends on the plan. Free-plan numbers are in section 11. |
| 3 | Same core in `drafftink-headless serve` | New sans-IO crate `drafftink-relay`. The Worker and `serve` are thin hosts around it. `crates/collab` stays transport-agnostic. |
| 4 | Images as separate encrypted files | Plain HTTP `PUT`/`GET` through the Worker. No chunking anywhere (sources in section 7.4). `MAX_IMAGE_BYTES = 4 MiB`, `MAX_ROOM_IMAGE_BYTES = 50 MiB`. |
| 5 | Retention | `ROOM_TTL = 48 h` after the last visit (a join). Edits do not move it. A DO alarm deletes the room. Rule for a connected person at the deadline: section 10. |
| 6 | Commands | `collab.*` stays the only door. Two arguments and one command are new (section 13). |
| 7 | Private instance (2026-11-22) | A second deployment on your own account at `canvas.patwie.com`: Google sign-in through Cloudflare Access for an email allow-list, rooms never expire, images in R2, the Worker re-checks the Access JWT, desktop and agents use an Access service token. Same code, a different `[env.private]`. Section 19. |

**The most important design choices**

1. **The room id is a hash of the membership token** (`id = SHA-256(token)[..16]`, token derived from the key by HKDF). The relay checks membership with no stored verifier, so there is no "first joiner registers" step, no squatting of a room id, and a Worker can reject non-members before it wakes a Durable Object.
2. **The relay is a blind, ordered log.** It assigns a sequence number, stores sealed records in segments, keeps one sealed snapshot, and fans out. It never parses Loro.
3. **Client-side compaction with compare-and-set.** One peer (chosen by the relay) seals a snapshot and the relay swaps it in only if the snapshot version it expected is still current.
4. **The client tracks `relay_vv`** (the version vector of what the relay is known to hold). Offline edits, lost acks and a crashed tab are all repaired by one step: upload `export_updates_since(relay_vv)`.
5. **Images are referenced, not embedded, inside rooms.** The document keeps `{mime, size}` per image. Bytes live in a local blob cache and, sealed, on the relay under a keyed name (`HMAC(K_name, file_id)`), so the relay never sees a content hash.
6. **The room tab must be cached with its CRDT history and its room id.** Today it is not (section 2.3). Without that, "reseed an expired room from the cache" is not possible.
7. **Reseed is a compare-and-set on an empty room.** The first cache holder wins. Others take the room and keep their copy in a separate tab. Two copies are never merged by guesswork.
8. **Presence is budgeted per room.** On the Free plan, 30 people moving cursors at 20 Hz would use the daily request quota in under an hour. The relay hands out a per-client rate and sends presence in bundles.

**Impossible or risky in the decisions** (details in section 17): the Free plan caps a demo at about 28 active room-hours a day; the docs do not say whether spend limits apply to the Free plan; a browser cannot read the status of a failed WebSocket upgrade; a hostile room member can crash every browser tab with one malformed Loro blob because the wasm build aborts on panic; the app does not cache the room link today.

---

## 2. What exists today

### 2.1 Collaboration

- **Protocol v1** (`docs/protocol.md`, crate `drafftink-protocol`, L0). Binary frames over one WebSocket. `Join` carries a Loro version vector. The server merges updates, answers "what does this vector lack", validates (`InvalidUpdate`, schema gate), stores through `RoomStore`. Room id: up to 128 characters, access by knowing the id, optional bearer token.
- **Client** (`crates/collab`, L4). `CollabClient` is a sans-IO state machine. `Transport` is a frame pipe (`open`, `send`, `close`, `poll`) with `NativeTransport`, `WebTransport` and `loopback()`. `CollabSession::pump` moves bytes. After each `SyncResponse` the client uploads whatever the server's version vector lacks. This one step already repairs offline edits and lost acks.
- **Server** (`apps/drafftink-server`, run as `drafftink-headless serve`). A tokio actor per room holds a Loro doc. Not sans-IO.
- **Engine** (`crates/engine`, L5). The `collab.*` commands (`docs/commands.md` section 2.16) drive the client. Remote edits go through `Session::apply_remote`, which is not a command.
- **Document** (`crates/doc`, L3). Loro with flat per-field maps. Elements are *mergeable* containers: the container id comes from the element id, so two peers that create the same id merge field by field (`docs/crdt.md` section 4). `files[sha256] = {mime, bytes}` holds images inside the document. `meta.schema = 1`.

### 2.2 Where plaintext assumptions sit

- The server imports every update (`InvalidUpdate`, `meta.schema` check), computes diffs from version vectors, compacts, and enforces `--max-room-bytes`. Under E2E none of this is possible on the relay (section 8.6).
- Images are embedded in Loro. A 4.2 MiB set of images made an 8.6 MiB snapshot in the earlier research (`/home/ubuntu/git/drafft-ink-plan/research/serverless-collab.md`, section 2). Cloudflare's 32 MiB WebSocket limit would hold it, but memory and the Free-plan quotas would not.
- The Join frame carries user names in the clear (`UserInfo`), and `PeerInfo` lists them.

### 2.3 Does the web app keep a local copy of a room tab? (checked in the code)

**Partly. The document is cached. The link between that copy and the room is not.**

What I read: `crates/engine/src/cmd/tab/workspace.rs`, `docs/commands.md` ("The workspace record"), `crates/app/src/startup.rs`, `crates/engine/src/cmd/collab/address.rs`.

- The workspace record (IndexedDB on the web, files on native) saves **every tab**, including a room tab, as the native JSON document. `Session::workspace_tick` writes when the revision changes. A remote edit raises the revision, so the cached copy follows the room. Images are embedded in that JSON.
- The record does **not** store the room. The index has `id, key, name, dirty, revision, view, selection` per tab. `RoomBinding` (`crates/engine/src/cmd/tab.rs`) lives in `Tab::ext` and is never written. A restored tab is a plain local tab.
- Startup (`startup::start`): when the page address names a room and the restored session has content, it runs `tab.new` and joins in the new empty tab. The stale copy stays in its own unbound tab, and the room loads from the server. So after a reload you get **two tabs**, and an expired room would give an empty tab next to a full, unrelated copy.
- The cached document is rebuilt from JSON. It has **no Loro history** of the room. Two people who restore from JSON and then write into the same room create independent operations on the same mergeable containers. Maps merge field by field. A `LoroText` does not: two independent inserts of the same text into one container concatenate it (my reading of `ensure_mergeable_text` in `docs/crdt.md` section 4; to be confirmed with a test in phase 1).

**Proposal** (phase 1, section 8.7): the workspace index (schema 2) stores `room: {id, key?, relay_vv, last_seq}` per room tab, and an extra record `workspace.t<id>.loro` holds the tab's Loro snapshot. Restore binds the tab to its room. Startup matches a link to a restored tab by room id instead of opening a new tab. Cached copies then share one CRDT lineage, so two of them can be merged or reseeded without duplication.

---

## 3. Excalidraw's E2E design, in my words

Source: `/home/ubuntu/git/excalidraw` (read-only). `P/` is `packages/excalidraw/`, `A/` is `excalidraw-app/`.

- **Link.** `#room=<roomId>,<roomKey>`. The id is 10 random bytes as 20 hex characters. The key is a 128-bit AES-GCM key as 22 base64url characters. Browsers never send the fragment to a server (RFC 3986 section 3.5, RFC 9110 section 17). Page scripts can read it.
- **Cipher.** AES-GCM, random 12-byte IV per message, **no associated data** (`P/data/encryption.ts:50-78`). A ciphertext is therefore not tied to a room or a message type. Messages are JSON, encrypted, not compressed. Share links (`#json=<id>,<key>`) deflate first.
- **Relay.** A small socket.io server that stores nothing. Events carry `(roomId, ciphertext, iv)`. It sees the room id, socket ids, sizes and timing. The `user-follow` payload is plaintext.
- **Catch-up.** A newcomer announces itself and every peer answers with the full scene; the first answer wins. Fallbacks load the stored scene after 5 s, on connect error, or when alone.
- **Merge.** Per element, higher `version` wins, then `versionNonce`. Not a CRDT: two edits of one element lose one.
- **Persistence.** One Firestore document per room, `{sceneVersion, iv, ciphertext}`, saved every 20 s in a transaction that reconciles with the stored copy (`A/data/firebase.ts`).
- **Images.** File id is the SHA-1 of the content (40 hex). Each file is deflated, encrypted with the room key and stored at `files/rooms/<roomId>/<fileId>`. Limit 4 MiB, checked after encryption. The uploader marks the element `saved`; peers then fetch. The repo has no cleanup. File ids are not verified against the content on download.
- **Local cache.** Saving to localStorage and IndexedDB is **paused while in a room**. Room content is never cached. If the server copy is gone and nobody is present, the client starts an empty canvas.
- **Access control.** The storage rules allow reads and writes to anyone who knows the path. The room id is the only gate. A key holder can forge messages, impersonate others, delete everything or bloat the scene. There is no revocation.
- **Weaknesses we avoid.** No AAD; one shared key for every purpose; plaintext metadata; unverified file ids and a weak hash; no output cap on decompression; a `window.alert` per failed decrypt (a member can spam it); first-answer-wins and load-failure-then-proceed races.

---

## 4. Threat model

**Goal.** Cloudflare, or the operator of a self-hosted relay, learns nothing about what is drawn, who is named what, or what the pictures show. A person who knows only the room id cannot read, write or fetch images.

### 4.1 What the relay (Cloudflare, or a self-hoster) still sees

| Visible | Why it cannot be hidden here |
|---|---|
| Room id (16 bytes, in the URL path and the Join frame) | It routes the request to the Durable Object. |
| IP addresses, TLS fingerprints, user agent, connection times | Transport. Cloudflare terminates TLS. |
| When people join and leave, and how many are connected | Connections are visible. |
| When edits happen, and the size of each sealed record | Records are forwarded one by one. Padding is possible (section 6.5), off in v1. |
| Number of records, snapshot size, total stored bytes | Needed for quotas. |
| Number, size and upload time of images, and a keyed name for each | Needed to store them. The name is `HMAC(K_name, file_id)`, so identical pictures in two rooms do not match. Inside one room the relay can see that two uploads are the same name. |
| Which client ids are peers in a room, and presence timing | The relay fans out presence. Names, cursors and selections are sealed. |
| The membership token (32 bytes) | Sent on every join and image request. See 4.3. |

### 4.2 What a hostile relay can do

- **Refuse service** or delete anything. Availability is not protected. Clients keep a local copy (section 10), which is the remedy.
- **Drop, delay, duplicate or reorder** records. CRDT import is idempotent and order-insensitive, so duplicates and reordering are harmless. A dropped record is detected when a per-sender counter skips (section 8.3), except for a dropped *tail*.
- **Roll back** by withholding the newest records. Not detectable in v1. Accepted.
- **Replay a record into another room or kind.** Refused by the AAD (section 6.3).
- **Join as a member.** It sees the membership token on the wire (Cloudflare terminates TLS, so it sees the token in the clear). With the token it can fetch ciphertext and upload junk. It cannot decrypt. A later signature scheme (5.5) removes the replayable token.
- **Serve malicious JavaScript.** Inherent to browser E2E: whoever serves `drafft.ink` can ship code that steals keys. Out of scope. Self-hosting and the open source build are the remedy. The link, not the page, is the secret.

### 4.3 What a malicious room member can do

A member holds the key, so the cryptography does not limit them. In v1 a member can:

- **Corrupt or erase the document.** The relay cannot validate. Clients validate each import (`LoroStore::apply_remote` returns errors, hides invalid elements), and the full history survives in snapshots until the next compaction, so a restore is possible later (not in v1).
- **Flood.** Bounded by the relay: per-connection and per-room rate limits, record size, a state-size cap and an image quota (section 11). A flood costs everyone's daily quota on the Free plan. That is a denial of service that only members can start.
- **Forge another member's identity inside sealed data** (names, `sender` in a record header). Members are mutually trusted. The relay at least enforces `header.sender == the connection's client id`.
- **Poison an image name** by uploading junk under a real image's name. Every download verifies the plaintext hash against the file id, so junk is detected, shown as "damaged picture", and any member who has the real bytes can overwrite it.
- **Crash every other client with a malformed Loro blob.** See the risk in 17.4. This is the worst member attack and needs a client-side mitigation (crash fence, 8.5).
- **Keep a room alive** by rejoining every 47 hours. Intended: a visit is a visit.

### 4.4 Accepted, and out of scope

The private instance (section 19) adds a layer in front of all of this (only allow-listed Google accounts can connect) and one more thing Cloudflare sees (the email of each connection); 19.7 lists both.

- No key rotation or revocation in v1. To remove a person, start a new room.
- No forward secrecy and no post-compromise security. A leaked link exposes the whole room, including stored history.
- The link in browser history, screenshots, shoulder surfing, clipboard managers, and browser extensions that read the page.
- Traffic analysis beyond the sizes and times listed above.
- Malware on the user's machine. A compromised build or page.
- A relay that lies about identities of members for people who do not hold the key.
- Local data at rest. The workspace cache and blob cache are plaintext in IndexedDB or the app folder, as they are today for local documents. A copy of the room key stored next to them (open question Q2) adds no exposure.

---

## 5. Keys

### 5.1 The root key and the link

- **Root key `K_root`: 32 random bytes** from the OS random source. In the browser, `getrandom` with the `wasm_js` feature uses `crypto.getRandomValues`. 256 bits to match the 256-bit cipher key. (128 bits like Excalidraw would be enough and give a 22-character key. I chose 256: the link grows by 21 characters, which nobody types.)
- **Link:** `https://drafft.ink/#room=<id>,<key>`, with `<id>` and `<key>` in unpadded base64url (22 and 43 characters). A self-hosted server adds `&server=<url>` as today.
- **Why both id and key** when the id is derivable: the comma tells the parser this is a v2 link (v1 room names never contain a comma), a log line or a person can tell which room it is without the key, and the client checks `id == derive(key)` and refuses a damaged link (`bad_room_link`). The cost is 22 redundant characters.
- The page keeps the fragment. It sets `Referrer-Policy: no-referrer` (a `<meta name="referrer">` tag if the host cannot send headers). The fragment is not sent in `Referer` anyway (RFC 3986), so this is defence in depth. The page does **not** strip the fragment with `history.replaceState`, because that breaks copying the address bar. The "Copy invite link" button is the primary way to share.
- The key never goes into a server address (`ServerUrl::parse` already refuses fragments), a command-line argument, a log line, the ring, `collab.status` or a setting (section 12 and 13).

### 5.2 Subkeys (HKDF-SHA256, RFC 5869)

`PRK = HKDF-Extract(salt = "drafft.ink room v1", IKM = K_root)`. Then `HKDF-Expand(PRK, info, 32)` for each label:

| Subkey | `info` | Used for |
|---|---|---|
| `K_content` | `"drafft v1 content"` | Sealing updates and snapshots |
| `K_presence` | `"drafft v1 presence"` | Sealing presence and direct peer messages |
| `K_image` | `"drafft v1 image"` | Sealing image files |
| `K_name` | `"drafft v1 image-name"` | HMAC key that turns a file id into a relay-visible name |
| `token` | `"drafft v1 membership"` | The membership token (32 bytes) |

`room_id = base64url( SHA-256("drafft v1 room-id" || token)[..16] )`. Separate keys per purpose mean one purpose can never be turned into an oracle for another, and the `kind` byte in the AAD (6.3) separates records even within a key. The label strings carry the version. A format change picks new labels.

### 5.3 Membership: how the relay checks it without learning the key

The user suggested "the first joiner registers a hash of a key-derived token". I propose a variant that needs no registration:

1. The joiner sends `token` (32 bytes) in `Join`, and in an `Authorization` header on image requests, over TLS.
2. The relay computes `SHA-256("drafft v1 room-id" || token)[..16]` and compares it, in constant time, with the room id in the URL and the frame.
3. A match proves the joiner derived the token from the key. A mismatch is `Unauthorized`.

Properties:

- **Stateless.** The relay stores nothing to check membership. A Worker can run the check and refuse a non-member before it touches a Durable Object, which saves Free-plan requests.
- **No squatting.** With "first joiner registers", someone who learns an unused room id (for example after expiry) could register their own hash and lock the real members out. Here nobody can produce a token for a room id without the key (a preimage).
- **The relay learns the token**, not the key. HKDF is one-way, so the token reveals nothing about `K_content` and the other subkeys. Holding the token lets one act as a member towards the relay: fetch ciphertext, upload junk, join. That is the "bearer" weakness in 4.2. Challenge-response with the same token would not help, because the relay would have to store the token. An Ed25519 variant (`ed25519-dalek` 3.0.0) added about 35 KB of wasm in my scratch build; the bearer variant adds none.
- **Upgrade path.** The `Join` frame carries `scheme u8` and `proof bytes`. Scheme 1 is the token. A later scheme 2 derives an Ed25519 key pair from `K_root` (`HKDF "drafft v1 sign"`), defines the room id as `H(public key)`, and has the joiner sign a relay nonce. The relay then stores and sees nothing replayable. The room id shape stays the same.

### 5.4 No key rotation in v1

A new key is a new room. The record header has a format version byte but no key id. A rotation would bump the version and add a `key_epoch`. To "remove someone", the UI offers "Start a new room from this one" (a command: `collab.join_room` with `on_local=merge` into a freshly generated room). That is a later UI item, not a protocol feature.

### 5.5 Later: read-only links (option only)

A viewer link would carry `K_read` (the content key, but not the signing key) and the relay would accept `Append` only from holders of a write signature. Clients would check a per-record signature (Secsync-style) because a read-key holder can seal records. This needs scheme 2 above, signatures on records, and a key split: `K_content` for confidentiality, `K_sign` for write rights. Not designed further here.

---

## 6. Crypto

### 6.1 Choice: XChaCha20-Poly1305

| | XChaCha20-Poly1305 | AES-256-GCM |
|---|---|---|
| Random nonce | 192 bits. A 50 % collision needs about 2^96 messages (draft-irtf-cfrg-xchacha-03). No counting. | 96 bits. NIST SP 800-38D caps a key at 2^32 invocations with random IVs. |
| No hardware AES (wasm) | ChaCha is fast in software. My measurement: 222 MiB/s in Node on an EPYC. | The `aes` crate falls back to bitsliced software: 58.5 MiB/s, about 3.8 times slower. |
| wasm size (release, no `wasm-opt`) | about 21 KB | about 35 KB |
| MSRV | 1.85 | `aes` 0.9.4 needs Rust 1.89 |
| Precedent | Secsync (XChaCha20-Poly1305), age (ChaCha20-Poly1305) | Excalidraw, Loro `%ELO`, y-webrtc |

I choose **XChaCha20-Poly1305**. The deciding points are the nonce: many tabs and devices share one key and none of them can keep a counter, so random nonces without a birthday limit remove a whole class of bugs. Size and speed in wasm come second. Both ciphers are fast enough for a whiteboard. Web Crypto (SubtleCrypto) has AES-GCM and HKDF but no XChaCha, is JavaScript-only (native would need a second implementation), and would split the format, so I dismiss it. It remains a fallback if wasm size ever matters.

Not key-committing: neither AES-GCM nor ChaCha20-Poly1305 commits to its key (Albertini et al., https://eprint.iacr.org/2020/1456). The partitioning-oracle attacks (Len, Grubbs, Ristenpart, https://eprint.iacr.org/2020/1491) need low-entropy keys. Here the key is 256 random bits, derived by HKDF, and the relay cannot try decryptions. Neither attack applies. If a future feature lets one room carry keys chosen by an attacker, add a 32-byte key-commitment tag then.

Audit status: the 2020 NCC Group review covered the older `aes-gcm` and `chacha20poly1305` code. I found no audit of the current 0.11 releases. State this in the release notes.

### 6.2 Crates (versions from crates.io on 2026-11-18)

| Crate | Version | Use |
|---|---|---|
| `chacha20poly1305` | 0.11.0 | `XChaCha20Poly1305` AEAD. wasm uses the portable backend (no simd128 backend exists). |
| `hkdf` | 0.13.0 | HKDF-SHA256 |
| `sha2` | 0.11.1 | SHA-256 (MSRV 1.88) |
| `hmac` | 0.13.0 | `K_name` file names |
| `getrandom` | 0.4.3 | Nonces and keys. On `wasm32-unknown-unknown` the **`wasm_js` feature** is required. I built a scratch crate with it and no `--cfg` flag was needed. Enable it only in the final binary crate or behind `cfg(all(target_arch = "wasm32", target_os = "unknown"))`, as the getrandom README asks. |
| `zeroize` | 1.9.2 | Wipe keys on drop |
| `subtle` | 2.6.1 | Constant-time compare |
| `base64ct` | 1.8.3 | base64url without padding |

I built `chacha20poly1305 + hkdf + sha2 + getrandom` for `wasm32-unknown-unknown` in `/tmp` and it compiled. The relay needs only `sha2` and `subtle`. It never encrypts and never needs a random source, because clients choose their own `client_id`. That keeps `getrandom` out of the Worker (whether `wasm_js` works inside a Worker is unverified, and now it does not matter).

### 6.3 Record format and AAD

A **sealed record** is what the relay stores and forwards:

```text
SealedRecord  = header || nonce[24] || ciphertext || tag[16]
header        = version u8 (=1) | kind u8 | sender [16] | ctr u64 LE | covers_seq u64 LE   (covers_seq: snapshots only, else 0)
plaintext     = len u32 LE | payload | zero padding        (padding is 0 bytes in v1; the length prefix makes padding possible later)
AAD           = "drafft.ink e2e v1" | room_id [16] | header (all 34 bytes)
```

`kind`: 1 update, 2 snapshot, 3 presence, 4 direct (peer to peer), 5 image. For an image the header's `sender` is zeros and `ctr` and `covers_seq` are 0, and the 16-byte image name is appended to the AAD (section 9.2).

Every field the relay or a receiver sees in plaintext is in the AAD, so editing the header (changing the sender, the counter, the kind or the snapshot cover) makes decryption fail. The room id in the AAD stops cross-room replay. The kind stops an update from being accepted as a snapshot. Overhead: 34 header + 24 nonce + 16 tag = 74 bytes per record.

**The relay's sequence number is not in the AAD.** The sender seals before the relay has chosen `seq`, so it cannot bind it. That is deliberate: the CRDT does not care about order. The per-sender counter `ctr` gives order within one sender and gap detection. A receiver keeps the last `ctr` per sender and drops a record at or below it (replay), with one exception: updates are applied even if out of order, because Loro import is idempotent, and `ctr` is used only to notice gaps.

### 6.4 Nonce strategy

A fresh random 24-byte nonce per record from the OS random source. No counters, no state to persist, safe across tabs, devices and restarts. A test generates a million nonces and checks for repeats, and another test checks that the same plaintext sealed twice gives different bytes.

### 6.5 Padding (later, no format change)

The plaintext has a length prefix. Padding to the next multiple of 256 bytes (or a power of two above 4 KiB) can be switched on by a client-side constant. v1 leaves it off and lists sizes as visible (4.1).

---

## 7. Wire protocol v2

### 7.1 Versioning

`drafftink-protocol` raises `PROTOCOL_VERSION` to 2 and keeps `MIN_PROTOCOL_VERSION = 1`. `Join` keeps its first five bytes (tag, `min_version`, `max_version`), as `docs/protocol.md` section 2 requires. The relay picks the highest common version. A room is **either** plaintext (v1) **or** sealed (v2), chosen when it is created. Joining a room of the other kind is `WrongRoomKind`. The Cloudflare Worker speaks v2 only. `drafftink-headless serve` speaks both. A v2-only server answers a v1 `Join` with `UnsupportedVersion`, whose message says "This server only hosts end-to-end encrypted rooms. Use a room link that has a key."

Routes (new): `wss://host/r/<room_id>/ws` and `https://host/r/<room_id>/f/<name>`. The room id must be in the path because the Worker picks the Durable Object before it reads a frame. The v1 route `/ws` stays.

### 7.2 Messages

Same framing as v1 (one binary frame, little-endian fixed integers, varint lengths, strict decoding, client tags below 0x80). `record` below means a `SealedRecord` as bytes; the relay parses only its header.

Client to relay:

| Tag | Message | Fields | Notes |
|---|---|---|---|
| 0x01 | `Join` | `min_version u16`, `max_version u16`, `room_id [16]`, `client_id [16]`, `mode u8` (0 open, 1 create), `scheme u8`, `proof bytes` (the token), `last_seq u64`, `captcha opt str` | Membership proof. `last_seq` is the last record the client holds (0 for none). `captcha` is a Turnstile token, only for `create` when the host requires it. |
| 0x10 | `Append` | `record` (kind 1) | Live update. The record's `ctr` identifies it. |
| 0x11 | `PutSnapshot` | `expected_snapshot_seq u64`, `record` (kind 2, `covers_seq` set) | Compaction, or a reseed when the room is empty. Compare-and-set. |
| 0x12 | `Presence` | `record` (kind 3) | Unreliable lane. Never stored. Rate-limited. |
| 0x13 | `Catchup` | `after_seq u64` | Ask for records after a point (gap repair). |
| 0x14 | `Direct` | `to opt [16]`, `record` (kind 4) | Peer-to-peer state request or reply. `to = none` broadcasts. |
| 0x15 | `DeleteRoom` | | Member only. Accepted only by a host with `ALLOW_DELETE_ROOM` (the private instance, 19.8). |
| 0x07 | `Leave` | | |

Relay to client:

| Tag | Message | Fields |
|---|---|---|
| 0x81 | `Joined` | `protocol_version`, `room_state u8` (0 existing, 1 created), `head_seq u64`, `snapshot_seq u64`, `log_records u32`, `log_bytes u32`, `expires_at_ms u64` (0 = never expires), `peers` (client ids only), `limits` |
| 0x82 | `Catchup` | `snapshot opt {covers_seq, record}`, `records list {seq u64, from [16], ctr u64, record}`, `head_seq u64`, `more bool` |
| 0x83 | `Record` | `seq u64`, `from [16]`, `ctr u64`, `record` (live fan-out; not echoed to the sender) |
| 0x84 | `Ack` | `ctr u64`, `seq u64` (the record is sequenced **and stored**) |
| 0x85 / 0x86 | `PeerJoined` / `PeerLeft` | `client_id` only |
| 0x87 | `PresenceBundle` | `list {from [16], record}` (up to one bundle per `PRESENCE_TICK_MS`) |
| 0x88 | `Error` | `code u8`, `message str`, `retry_after_ms opt u32` |
| 0x89 | `SnapshotResult` | `ok bool`, `covers_seq u64`, `snapshot_seq u64`, `head_seq u64` |
| 0x8a | `CompactAdvice` | `head_seq u64` (sent to one peer, see 8.4) |
| 0x8b | `Direct` | `from [16]`, `record` |
| 0x8c | `Retention` | `expires_at_ms u64`, `expiring bool` (sent when a later visit moves the deadline, or the room is past it) |

`limits` in `Joined`: `max_frame_bytes`, `max_record_bytes`, `max_updates_per_sec`, `max_presence_per_sec` (the personal rate, see 11.3), `max_image_bytes`, `max_room_image_bytes`, `max_room_images`.

Keepalive: the client sends the **text** frame `ping` every 20 s, and the relay answers `pong`. On Cloudflare this is a hibernation auto-response pair (`setWebSocketAutoResponse`), which answers without waking the object and without a request charge, so an idle room costs nothing. The pair is documented for the Hibernation API; I read that ping/pong are auto-handled and do not wake the object (https://developers.cloudflare.com/durable-objects/best-practices/websockets/), but the exact text-pair behaviour is for the phase 3 spike to confirm. Text frames other than these two are refused. The v1 binary `Ping`/`Pong` are not used in v2.

Error codes (new in v2): `Unauthorized`, `RoomMissing`, `WrongRoomKind`, `Conflict`, `RoomFull` (state or image quota), `DailyLimit` (host capacity, with `retry_after_ms`), `TooLarge`, `RateLimited`, `UnsupportedVersion`, `Replaced`.

### 7.3 Which fields are plaintext, and why

| Field | Plaintext | Why |
|---|---|---|
| room id, `client_id`, `mode`, token | yes | Routing and membership. |
| `kind`, `sender`, `ctr` | yes (and in the AAD) | The relay enforces `sender == client_id`, rate-limits by kind, and receivers detect gaps without decrypting. |
| `covers_seq`, `expected_snapshot_seq` | yes | The relay needs it for compare-and-set and for deleting records. |
| `seq`, `head_seq`, counts, sizes | yes | Assigned and used by the relay. |
| record payload (Loro bytes, presence, image bytes) | **sealed** | |
| user name, colour, cursor, selection, viewport | **sealed** (inside presence) | Only peers see them. |
| image names | keyed hash | 9.2. |

### 7.4 Size limits and the Durable Object message limit

| Limit | Value | Source or reason |
|---|---|---|
| Received WebSocket message in a Durable Object | **32 MiB**; a larger message closes the socket with code 1009 | https://developers.cloudflare.com/durable-objects/platform/limits/ and https://developers.cloudflare.com/workers/runtime-apis/websockets/ |
| Our frame (`MAX_FRAME_BYTES`) | 16 MiB (unchanged) | Half of Cloudflare's limit |
| One sealed update (`MAX_RECORD_BYTES`) | 4 MiB (unchanged) | Images left the document, so typical updates are 130 B to 32 KB |
| One sealed snapshot | 15 MiB (as `MAX_SYNC_BYTES`) | |
| `Catchup` frame | at most 64 records or 2 MiB, then `more = true` | Bounds memory and a slow reader |
| Sealed state of a room (snapshot plus log) `MAX_ROOM_STATE_BYTES` | 32 MiB (demo), 256 MiB (self-host default) | Quota. About 21 times the 1.5 MiB state of a 10,000-element drawing. |
| One image, sealed | `MAX_IMAGE_BYTES + 74` | |
| Image HTTP request body | Free plan: 100 MB per request (zone limit); we use 4 MiB | https://developers.cloudflare.com/workers/platform/limits/ |
| SQLite row, blob or string in a Durable Object | 8 MB | https://developers.cloudflare.com/durable-objects/platform/limits/ |

**No chunking is needed anywhere.** A 4 MiB image fits one HTTP request (100 MB limit), one SQLite row (8 MB limit), and, sealed, stays far under 32 MiB. The 16 MiB frame cap also fits one WebSocket message. I checked each of these against the Cloudflare pages above; the earlier research showed tldraw chunking at 256 KiB because of an older 1 MB limit, which no longer applies. If a future plan or host lowers the message limit, the `Catchup` framing already allows splitting snapshots at the record boundary, but a single sealed snapshot is one record.

### 7.5 Rates

Per connection, enforced by the relay core: `Append` 60/s (burst 2x, as v1), `PutSnapshot` 1 per 10 s, `Catchup` 1 per second, `Direct` 5/s, `Presence` by the personal rate from `Joined.limits` (11.3). Repeated breaches close the connection.

---

## 8. Sync and catch-up

### 8.1 Roles

- **Relay** (`drafftink-relay`, sans-IO). Membership check, sequence numbers, segment log, one snapshot, compare-and-set, presence bundling, rate limits, retention. It never imports a Loro blob.
- **Sealed client** (`crates/collab`, new module `sealed`). A sans-IO state machine parallel to `CollabClient`. It shares `PeerTable`, `LocalPresence`, `ServerUrl`, the backoff, `Status` and `Problem`. The engine sees the same events. `CollabSession` holds either client.
- **Transport** is unchanged: the same `Transport` trait carries v2 frames. Images use a second port (9.5).

### 8.2 Join and catch-up

```text
client A                       relay (DO "room")                      storage
  | Join{room, token, mode, last_seq} -> |  H(token) == room id ?  (else Unauthorized)
  |                                      |  load meta / records ----------------> |
  |                                      |  deadline = now + ROOM_TTL; set alarm  |
  | <---------- Joined{head, snapshot_seq, expires_at, peers, limits}             |
  | <---------- Catchup{snapshot?, records, head, more}                           |
  |  open each record with K_content; apply_remote (snapshot first, then records) |
  |  relay_vv = union of applied blobs' end versions                              |
  |-- Append{upload export_updates_since(relay_vv)} -> |  (own offline edits)     |
  | <---------- Ack{ctr, seq}                                                     |
  |-- Presence{sealed user, cursor} ---------> |  bundled to the others           |
```

- `last_seq = 0` gets the snapshot (if any) plus every record after it. A client that holds records up to `last_seq` and whose last snapshot is still current gets only records after `last_seq`. If a compaction happened since (the relay's `snapshot_seq > last_seq`), the client gets the new snapshot and the records after it. The client imports a snapshot with the normal merge, so nothing of its own is lost.
- **Fresh room.** `Joined.room_state = 1` (created) when the room did not exist and `mode = create`. With `mode = open` on a missing room the relay answers `Error{RoomMissing}` and creates nothing. That stops a mistyped key from silently making a new room.
- **Fallback to peers.** If the relay's storage is unreadable (`Error` or `Unavailable` after `Joined`) or the catch-up does not complete in 10 s while peers are present, the joiner sends `Direct{to: none, StateRequest{vv}}` (sealed, kind 4). Any peer replies `Direct{to: requester, StateReply{export_updates_since(vv)}}`. This is the v1 server's job done by a peer. It works only while a peer is connected. It is not on the normal path.

### 8.3 Live edits, order and gaps

```text
client A                 relay                           client B
  | Append{rec ctr=17} ->  | seq = ++head                    |
  |                        | add to segment buffer           |
  |                        |-- Record{seq, from=A, ctr, rec}->|  open, apply_remote
  |                        | flush segment (<= 1 s) -------> storage
  | <- Ack{ctr=17, seq}    |  (after the flush: stored)      |
  |  relay_vv += end version of rec 17                       |
```

- The relay orders by `seq`. Receivers apply on arrival. Because Loro import is order-insensitive and idempotent, `seq` serves as a cursor (`last_seq` = highest `seq` such that all lower ones were received) and as the compaction cover. It is not needed for correctness.
- A receiver that sees `seq` jump sends `Catchup{after_seq: last_seq}`. A receiver that sees a per-sender `ctr` skip does the same. Requests are spaced and back off as in v1 (`docs/collab.md` "Sync, in short" item 4). A dropped tail is invisible until the next record or a rejoin.
- `Ack` means "sequenced and stored" (the relay flushes before it acknowledges). The Durable Object holds outgoing messages until the storage write is confirmed (https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/), so there is no separate "durable" message. The client keeps at most 32 unacknowledged updates, as in v1.
- **Segments.** The relay does not write one row per record. It buffers appended records and writes one segment row per flush (every `FLUSH_MS = 1000`, or at 256 KiB). This keeps the Free plan's 100,000 rows-written-per-day quota (11.2) usable. Self-host mode sets `FLUSH_MS` freely.

### 8.4 Compaction: who, when, how

- **Trigger.** The relay counts records and bytes since the snapshot. Past `COMPACT_RECORDS = 500` or `COMPACT_BYTES = 4 MiB`, it sends `CompactAdvice{head_seq}` to **one** peer: the longest-connected one. If no `SnapshotResult` follows within 30 s it asks the next. It also advises at join when a newcomer finds `log_records > 100`, because the newcomer has just applied the whole log.
- **Who compacts.** A client, never the relay. On advice it waits until it has applied every record up to `head_seq`, then exports its own full snapshot (`export_snapshot()`, or the shallow snapshot policy of `docs/crdt.md` section 13 above 8 MiB), seals it with `covers_seq = last contiguous seq applied`, and sends `PutSnapshot{expected_snapshot_seq, record}`. The snapshot may contain more than `covers_seq` records (the client's own unsent edits). That is harmless because import is idempotent.
- **Compare-and-set.** The relay accepts only if `expected_snapshot_seq == current snapshot_seq` and `snapshot_seq < covers_seq <= head_seq`. One more case is the **seed** of a room that has no snapshot and no records (`head_seq = 0`): `expected_snapshot_seq = 0` and `covers_seq = 0` are accepted exactly once (section 10.3). Then, in one storage transaction, it stores the snapshot and deletes segments entirely at or below `covers_seq`. Otherwise `SnapshotResult{ok: false}` and nothing changes. Two racing compactors cannot lose a live record.
- **Delete accounting.** On Free, each deleted row counts as a row written. A compaction of 20 segments costs about 21 rows.
- **Cost.** A 10,000-element room has a snapshot of about 1.5 MiB (`docs/crdt.md` section 15). One compaction per 500 records is cheap.

### 8.5 Offline edits and the crash fence

- **Offline edits.** The sealed client does what the v1 client does after every `SyncResponse`, with one change: the v1 server tells the client its version vector, the sealed relay cannot. The client keeps `relay_vv`: the version vector of operations it knows the relay holds. It is the union of the end versions of every applied snapshot and record and every acknowledged own update (read from the blob with Loro's import-blob metadata; to be confirmed against the pinned Loro version in phase 1). After catch-up it sends `export_updates_since(relay_vv)`. This repairs offline edits, lost acks, a crashed tab and a dropped tail. If `relay_vv` is too small, the client resends some operations the relay already has. That is a few duplicate bytes, not a fault.
- **Reconnect.** The client keeps its document, `client_id` and `last_seq`. It joins with `last_seq` and gets only what is new. The backoff is the v1 backoff.
- **Crash fence (mitigation for 17.4).** On wasm the release profile aborts on panic, so a malformed Loro blob that makes Loro assert would end the page, and it would do it again on every load, since the blob is in the stored log. Before importing a batch the client writes `inflight = {first_seq, last_seq}` to `sessionStorage` (native: a small file). After a clean apply it clears it. At startup, a leftover `inflight` means the previous run died while applying. The client then imports that range one record at a time, setting the marker per record, so the culprit is found. It records the poison `seq` in the tab's local data, skips it, and shows "One change from someone in this room could not be read and was skipped." This is cheap (one synchronous storage write per batch) and removes the permanent-poison failure. It does not stop the first crash. Longer term, importing in a Web Worker removes it (also listed in `docs/collab.md` "Known limits").

### 8.6 What validation moves to the client

| Today on the server | In a sealed room |
|---|---|
| `InvalidUpdate` for bytes Loro rejects | Receiver: `apply_remote` already returns `DocError`, never panics on native. A failed record is skipped and shown once, never an alert per message (Excalidraw's flaw). |
| `meta.schema` gate (`SERVED_SCHEMA`) | Receiver: `apply_remote` returns `NewerSchema` and the store goes read-only with "Update the app" (`docs/crdt.md` section 8). A hostile update that raises the schema can switch every client to read-only. Accepted: members are trusted. A "new room" is the remedy. |
| Size and rate by content | Relay: by ciphertext size and by counts only. |
| `--max-room-bytes` | Relay: on sealed bytes, `MAX_ROOM_STATE_BYTES`. |
| Version-vector diff for joiners | Replaced by `seq` cursors and client-side `relay_vv`. |
| Compaction | Client (8.4). |
| Merge repair (`docs/crdt.md` section 10a) | Unchanged. Already client-side. |
| Peer count and names | Peer ids by the relay. Names sealed. |

The relay keeps the two checks it can do: record size and counts, and that `header.sender` equals the connection's `client_id`.

### 8.7 The local cache of a room tab (new)

- Workspace index schema 2: each tab entry may carry `room: {id, key, relay_vv, last_seq, joined_ms}`. The key is stored with the cached document, which is already plaintext (open question Q2 asks the user to confirm; the alternative stores only the id and needs the link on every start).
- Next to the JSON document, `workspace.t<id>.loro` holds the tab's Loro snapshot. Restore opens the tab from it (the document, undo is empty as today), re-binds `RoomBinding`, and keeps one CRDT lineage with the room.
- Startup: a link whose room id matches a restored tab binds that tab. No `tab.new`, no duplicate. A restored room tab without a link reconnects with `mode = open` (never `create`), so it cannot revive an expired room by itself.
- `collab.leave_room` removes the `room` entry. The document stays as a local copy, as today.
- An older build reads a schema-2 index as "newer schema" and leaves it alone (the existing rule). Tests in `workspace_tests`.

---

## 9. Images

### 9.1 Model

- In a room, `files[file_id] = {mime, size}` in the Loro document. **No bytes.** `file_id` stays the SHA-256 of the plaintext bytes, as today. The id lives inside the sealed document, so the relay never sees it.
- Bytes live in a **local blob cache** (a `Storage` port namespace `Blobs`, keyed by `file_id`, plaintext, evicted with the workspace) and, sealed with `K_image`, on the relay.
- Local documents (a local tab, a `.drafftink` file, the native JSON) keep embedding images. **The native JSON format does not change.**
- Implementation sketch: the store has a `FilePolicy` (`Embedded` or `External`). In `External` mode `put_file` writes the manifest to Loro and the bytes to the view and the blob cache only. A manifest without bytes shows the existing "image blob missing" placeholder (`DecodePolicy::Deferred` and the late-blob path already exist in `crates/render`) until the bytes arrive.

### 9.2 Naming on the relay

`name = base64url( HMAC-SHA256(K_name, file_id)[..16] )` (22 characters). The relay sees only this. The same picture in two rooms has two unrelated names. Inside one room, uploading the same picture twice gives the same name, which dedups. The id is therefore not a plaintext hash on the relay.

A sealed image is `SealedRecord{kind 5}` where the AAD also binds the name: `AAD = "drafft.ink e2e v1" | room_id | header | name`. A downloader checks the AEAD, then recomputes `SHA-256(plaintext)` and compares it with `file_id`. A mismatch is "damaged picture" and the bytes are discarded. This fixes Excalidraw's unverified file ids.

### 9.3 Upload and download

```text
uploader A                  relay (Worker -> DO)                      storage       peer B
  | image.insert: bytes, file_id = sha256, size <= MAX_IMAGE_BYTES (4 MiB)
  | commit manifest files[file_id]={mime,size} + element -> Append (sealed)  ------> B
  | blob cache <- bytes (local, at once: A sees the picture immediately)
  | seal = K_image.seal(bytes, AAD+name)
  | PUT /r/<room>/f/<name>   Authorization: Drafft-Member <token>
  |   body: sealed (<= 4 MiB + 74) -> Worker: check H(token)==room, Content-Length <= cap
  |                                 -> DO: check room quota, store row ----> rows
  | <- 201 Created {room_image_bytes}                                         |
  |                                                                           | B: manifest arrives, bytes missing
  |                                                                           |  GET /r/<room>/f/<name>
  |                                                  DO: read row ----------> | <- 200 sealed bytes
  |                                                                           |  open, sha256 == file_id ?
  |                                                                           |  blob cache <- bytes; element redraws
```

- **Order.** The manifest update may reach peers before the upload finishes. A peer that gets `404` retries with backoff (1, 2, 4, 8 s, up to 2 minutes), then shows the placeholder with "waiting for the picture". The uploader shows the "unsynced" dot until the `PUT` succeeds. This also covers an uploader that goes offline right after inserting: the upload resumes on reconnect, from the blob cache.
- **Lazy download.** A peer fetches a picture when its element is near the viewport or an export needs it, with at most 3 concurrent requests. A 50-picture room costs a viewer only what they look at, which matters on Free (each request counts, 11.2).
- **List.** `GET /r/<room>/f` returns `[{name, size}]` (sealed names only). Used after a reseed and by tests.
- **Statuses.** `201` stored, `200` exists or read, `404` missing, `401` bad token, `413` over `MAX_IMAGE_BYTES`, `507` over `MAX_ROOM_IMAGE_BYTES` or `MAX_ROOM_IMAGES`, `429` rate limited with `Retry-After`, `503` with `{"error":"daily_limit"}` when the host is out of quota. Bodies are small JSON with a stable `code`.
- **Overwrite** by the same name is allowed and re-accounted (heals a poisoned name, 4.3). Delete is not offered in v1; images disappear with the room.
- **Membership.** Every request carries the token. The Worker verifies `SHA-256("drafft v1 room-id" || token)[..16] == room id` before it calls the Durable Object, so a request for an unknown room never starts one.

### 9.4 Limits (named constants)

Defined in `drafftink-protocol` (`limits.rs`), checked by the engine, the relay core and the Worker. The relay announces its own, possibly lower, values in `Joined.limits`.

| Constant | Value | Note |
|---|---|---|
| `MAX_IMAGE_BYTES` | 4 MiB | Plaintext, before sealing. The user may lower it to 2 MiB. |
| `MAX_ROOM_IMAGE_BYTES` | 50 MiB | Sum of sealed sizes in one room. About 12 images at the maximum. |
| `MAX_ROOM_IMAGES` | 200 | Bounds the row count. |
| `SEAL_OVERHEAD` | 74 B | |

Why 50 MiB: the Free plan has 5 GB of Durable Object storage for the whole account (https://developers.cloudflare.com/durable-objects/platform/pricing/), so 50 MiB per room allows about 100 full rooms at once. Typical rooms use a few MiB. The demo also needs a global guard (11.4).

Refusals the user sees (plain words): `image_too_large`: "This picture is 6.2 MB. Pictures in a shared room can be at most 4 MB. Save it smaller (for example as a JPEG) and insert it again." `room_images_full`: "This room already holds 50 MB of pictures. Remove some, or start a new room."

### 9.5 Ports and hosts

A second port beside `Transport`: `BlobTransport { put, get, head, list }`, with the same three kinds of implementation as `Transport`: native (a small blocking HTTPS client such as `ureq` with rustls, run on the background thread), web (`fetch` through `web-sys`), loopback (tests). The sans-IO `sealed` client emits `BlobJob`s and consumes `BlobResult`s, exactly like frames, so the engine and the tests stay free of sockets.

### 9.6 The file format and the migration

- **Loro schema bump 1 to 2.** `files[id]` gains `size` and `bytes` becomes optional. A bump is required by `docs/crdt.md` section 14 (a stored key changes meaning). The v1-to-v2 step adds `size = bytes.len()` to each entry, derives only from data and is idempotent, as the migration rules demand. A schema-1 build opens a schema-2 document as "newer: update the app". `SERVED_SCHEMA` on a v1 `serve` moves to 2.
- **Native JSON stays `version: 1`.** Saving a room tab (`doc.save`, `doc.export` as JSON) **embeds** the images from the blob cache, so a saved file is self-contained and any build opens it. An image that is not in the cache (never downloaded) is left out. The existing validator already treats "blob missing from `files`" as a warning, not an error, so such a file saves and reopens with a placeholder. `doc.save` says so in its output (`images_missing: n`). "Saving a room may extract images" from the brief is therefore not needed. The reverse (extraction on join) is.
- **Embedded images meet a room (extraction on join).** `collab.join_room {on_local: merge}` with a local document that has embedded images: for each embedded file the client seals and uploads it, writes the manifest-only entry in the same sealed commit, and keeps the bytes in the blob cache. A document with more than `MAX_ROOM_IMAGES` pictures, or any picture over `MAX_IMAGE_BYTES`, or more than `MAX_ROOM_IMAGE_BYTES` in total, refuses `local_too_large_for_room` before anything is sent, naming the first offender. Images do not need new ids.
- **Old files** (drafft-ink JSON, Excalidraw files) go through `doc.open` as today, into a local tab, and then follow the rule above if the user shares the tab.
- **Workspace** records keep embedding in the per-tab JSON (as today). The blob cache keys are the same `file_id`s.

### 9.7 Export

`doc.export` (PNG, SVG, JSON) reads pictures from the blob cache, so it works offline for every picture that was ever seen on this device. For pictures not in the cache:

- Online: the command first fetches them. It returns the existing pending refusal (`render_pending` pattern, `docs/web-app.md`) as `images_pending` ("3 pictures of this room are still downloading. Run the export again in a moment.") and the shell re-runs it.
- Offline, expired or damaged: `images_unavailable`: "2 pictures of this room could not be loaded, so the export would have empty frames. Pass `missing_images: "placeholder"` to export without them." The default is the refusal, so an export is never silently incomplete.

---

## 10. Retention, expiry and reseed

### 10.1 The rule (user's clarification, with the case it left open)

- `ROOM_TTL = 48 h`. Each room has a hard cap of `ROOM_TTL` after its most recent **visit**.
- A visit is an accepted `Join` (membership already verified). It sets `deadline = now + ROOM_TTL`. Edits, presence, pings and image requests do **not** move the deadline. A non-member cannot move it, because the check precedes the reset.
- A reconnect after a network drop is also a `Join`, so it also counts. I accept this: it is rare, and a person who keeps reconnecting is a person who is visiting.
- **Someone still connected when the deadline passes.** The user suggested: do not delete under a connected person, delete after they leave. I propose the same rule with two additions:
  1. **Grace.** Delete `ROOM_LEAVE_GRACE = 60 s` after the last socket closes, not at once. A page reload or a flaky network looks like leave-then-join. A rejoin within the grace is a visit and re-arms 48 h. Without this, a reload past the deadline would destroy the room.
  2. **Honesty.** `Joined.expires_at_ms` and `Retention{expires_at_ms, expiring}` let the app say so. While the room is past its deadline the status bar shows "This room is past its 48-hour limit and will be deleted when the last person leaves. Save a copy." A later joiner clears `expiring` and moves the deadline, as any visit does.

  A rule I considered and rejected: "if anyone is connected at the deadline, extend by 48 h". It makes the cap soft and lets one forgotten tab keep a room alive for ever.

On the private instance (section 19) `ROOM_TTL` is unset: no deadline, no alarm, no deletion. Only `collab.delete_room` removes a room there. Everything in this section is the demo's rule.

### 10.2 State machine (in `drafftink-relay`)

State: `deadline_ms`, `expiring: bool`. Inputs: `Joined(now)`, `SocketClosed(now, live_count)`, `Wake(now)`. Output: `next_wake_ms` and `Delete`.

```text
Join accepted       : deadline = now + TTL; expiring = false; wake_at = deadline
Wake(now)           : if now < deadline            -> wake_at = deadline        (a join moved it)
                      else if live_sockets > 0     -> expiring = true; wake_at = now + RECHECK (1 h)
                      else                         -> Delete
SocketClosed        : if live == 0 and now >= deadline -> wake_at = now + GRACE
```

`RECHECK` (1 hour) covers a socket that died without a close event. At each recheck only sockets heard from within 2 minutes count as live (clients send `ping` every 20 s). On the Durable Object, `wake_at` is `setAlarm`; on the server it is a timer. The core never reads a clock: the host passes `now_ms`, so tests run with simulated time.

Durable Object notes (from the docs): one alarm per object, `setAlarm` overrides; at-least-once with retries (2 s exponential, up to 6) so the handler must be idempotent and catch its own errors; alarms may be late by up to a minute; each `setAlarm` is a row written and each invocation is a request (https://developers.cloudflare.com/durable-objects/api/alarms/). Deletion: `deleteAlarm()` then `deleteAll()`, which removes the whole SQLite database including the key-value data (https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/). The page is silent on whether `deleteAll()` also clears the alarm, so call `deleteAlarm()` first, and the spike verifies it. No maximum alarm horizon is documented, so now plus 48 h is not forbidden (unverified).

Room images are in the same database and go with the room. Self-hosters set `--room-ttl` (off by default, so rooms persist). Later: images in R2 or S3 with a 2-day lifecycle rule use the same `BlobStore` trait (section 14) and need no protocol change.

### 10.3 The cached copy and an expired room

```text
person P (has a cached copy of room R)          relay                         storage
  | opens the link  #room=<id>,<key>
  | startup matches the id to a restored tab (8.7), binds it
  | Join{mode=open, last_seq=...} --------------> | no such room
  | <---------------------------------- Error{RoomMissing}
  | the tab has a copy and the link has the key -> if_missing = restore
  | Join{mode=create, last_seq=0} --------------> | create room; deadline = now + TTL
  | <---------------------------------- Joined{room_state=created, head=0, snapshot=0}
  | seal export_snapshot() with covers_seq = 0 (kind 2)
  | PutSnapshot{expected_snapshot_seq=0, rec} --> | seed: accept only if head==0 and no snapshot yet
  | <---------------------------------- SnapshotResult{ok}
  | for each image in the blob cache: PUT /f/<name> (up to the room quota)
  | toast: "This room had expired. It was restored from your copy saved on <date>."
```

- **Who restores.** The command `collab.join_room` takes `if_missing: "refuse" | "restore" | "create"` (default `refuse`). The startup path passes `restore` when a restored tab matches the link, and the default otherwise. A plain app start without a link only reconnects with `open`, so an expired room is never revived behind the user's back (this keeps the 48-hour deletion honest).
- **Two cache holders race.** The seed is a compare-and-set on an empty room (a seed `PutSnapshot` with `covers_seq = 0` is accepted only while `head_seq = 0` and the room has no snapshot; an `Append` that arrived first makes `head_seq > 0`). The first wins. The second gets `SnapshotResult{ok: false}` or a non-empty `Joined`, so it takes the room as an ordinary join with `on_local = discard` for the room tab and **keeps its own copy in a new tab** named "<name> (saved copy)", and says so. Because both copies share one CRDT lineage when they came from the same room (8.7), merging them would also be safe, but the user decides: `collab.join_room {on_local: "merge"}` stays available.
- **Different lineage.** A cache restored from a JSON file has no room history. Its text elements could duplicate when merged into a room that holds the same elements. So a JSON-only copy is restored only into an empty room, and is never merged into a non-empty one by default.
- **Images** are re-uploaded from the blob cache; names are deterministic, so a second restorer's uploads simply overwrite equal bytes.

### 10.4 No cache

- `mode = open` on a missing room gives `RoomMissing`, and the tab shows: "This room is gone. Rooms are deleted 48 hours after the last visit, or the link is incomplete. Start an empty room with the same link?" The button runs `collab.join_room {if_missing: "create"}`. The link keeps working for the new room (the key is the same, the room is empty).
- A typo in the key gives a different room id, so also `RoomMissing`. The text mentions "or the link is incomplete".

---

## 11. Abuse and cost control

### 11.1 What the Free plan gives (read 2026-11-18)

| Resource | Free limit | Source |
|---|---|---|
| Durable Object classes | SQLite-backed only (KV-backed is not available on Free) | https://developers.cloudflare.com/durable-objects/platform/pricing/ |
| DO requests | 100,000 per day. HTTP, WebSocket upgrades and alarm invocations count. **Incoming WebSocket messages count 20:1.** Outgoing messages and pings are free. | same |
| DO duration | 13,000 GB-s per day, billed at 128 MB regardless of use: about 101,000 s, so **about 28 active room-hours per day** for all rooms together. A hibernated object accrues none. | same |
| SQLite rows written | 100,000 per day. Deletes and `setAlarm` count. | same |
| SQLite rows read | 5 million per day | same |
| DO storage | 5 GB per account. (The limits page's FAQ says 1 GB per object on Free: unverified, conflicting.) | same, and the limits page |
| Over the limit | "Further operations of that type will fail with an error"; counters reset at 00:00 UTC. The exact error text and WebSocket close code are unverified. | same |
| Workers requests | 100,000 per day; 10 ms CPU per HTTP request. Over the limit: HTTP error 1027. | https://developers.cloudflare.com/workers/platform/limits/ |
| Static assets | free and unlimited | https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/ |
| DO CPU | 30 s per invocation by default. Whether Free gets 10 ms is not stated (unverified). Our relay does only copying and SQL, so I do not expect a problem; the spike measures it. | https://developers.cloudflare.com/durable-objects/platform/limits/ |

### 11.2 What this means for a room of dozens

- **Presence is the cost.** If 30 people each send 20 cursor updates a second, the object receives 600 messages a second. That is 30 requests a second, and 100,000 requests are gone in about 55 minutes. So presence is budgeted (11.3). Edits are cheap by comparison: one sealed update is 130 B to a few KB, a few per second per drawer.
- **Duration.** 28 active room-hours a day in total means the demo supports "a few classrooms a day", not a public service. This is the Free plan being a demo tier. The paid plan raises it (400,000 GB-s included, https://developers.cloudflare.com/durable-objects/platform/pricing/ and the earlier research). The code does not change.
- **Rows written.** One row per segment, one more per delete at compaction, a few per join. At about one segment per 1-2 seconds of active drawing, that is a few thousand rows per active hour: the same order as the duration quota. Both quotas land at "tens of room-hours a day".
- **Fan-out is free**, but the object is single-threaded with a soft limit of about 1,000 requests a second (https://developers.cloudflare.com/durable-objects/best-practices/websockets/). With the budget below, a 50-person room gets at most 40 incoming presence messages a second and sends 50 bundles every 100 ms: 500 sends a second, which is fine.

### 11.3 Presence budget

The relay sets a **personal presence rate** in `Joined.limits.max_presence_per_sec = clamp(ROOM_PRESENCE_BUDGET / max(peers, 1), 1, 20)` with `ROOM_PRESENCE_BUDGET = 40` messages a second for the whole room. Two people get 20 each, four get 10, 30 get 1.3. It is recomputed and sent with `PeerJoined` (clients take the lower of the old and new rate). The client already sends only when something changed, and sends nothing when alone (`docs/collab.md`). Receivers interpolate cursors, so a slow rate still looks smooth. The relay sends `PresenceBundle` every `PRESENCE_TICK_MS = 100`. Presence is never stored.

### 11.4 Limits and caps (all named constants, one table)

| Constant | Value | Where enforced |
|---|---|---|
| `MAX_PEERS_PER_ROOM` | 64 (v1 server default; protocol max 256) | relay core |
| `ROOM_PRESENCE_BUDGET` | 40 messages/s per room | relay core |
| `MAX_UPDATES_PER_SEC` | 60 per connection, burst 2x | relay core |
| `MAX_ROOM_UPDATES_PER_SEC` | 200 per room | relay core |
| `MAX_RECORD_BYTES` | 4 MiB | relay core, Worker (early reject) |
| `MAX_ROOM_STATE_BYTES` | 32 MiB demo | relay core |
| `MAX_IMAGE_BYTES`, `MAX_ROOM_IMAGE_BYTES`, `MAX_ROOM_IMAGES` | 4 MiB, 50 MiB, 200 | relay core and Worker |
| `ROOM_TTL`, `ROOM_LEAVE_GRACE`, `RECHECK` | 48 h, 60 s, 1 h | relay core |
| `MAX_ROOMS_DEMO`, `MAX_DEMO_BYTES` | 2,000 rooms, 4 GB | `Registry` object (below) |

- **Registry object.** One extra Durable Object, `Registry`, counts rooms and stored bytes for the whole demo, so the account does not hit the 5 GB quota (which would fail every write). A room reserves on creation and on each image upload, and releases on deletion. It is only called on create, delete and upload, not on edits. If it says no, the user sees the "demo is full" message (11.6). It is optional for phase 3a and needed before a public launch.
- **Room creation** (`mode = create`) is the abuse point, since the id is free. Options: (a) **Turnstile** on creation only: the Worker verifies the token with `siteverify` (a `fetch` that counts as one of 50 subrequests; tokens last 300 s and are single-use, https://developers.cloudflare.com/turnstile/get-started/server-side-validation/; the Free Turnstile plan has unlimited challenges, https://developers.cloudflare.com/turnstile/plans/). The widget needs JavaScript in the page and a visible challenge only when Cloudflare suspects a bot. (b) A per-IP creation budget kept in the `Registry` (store a keyed hash of the IP, expire it daily). The Workers Rate Limiting binding would be the natural tool, but its availability and cost on Free are not stated (https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/), its window is 10 or 60 s only and counted per location, so I do not depend on it. Recommendation: Turnstile in phase 4, per-IP budget only if abuse shows up.
- **A Worker route that fails before the object** (non-member, oversize, wrong content type) costs one Worker request and no Durable Object request.

### 11.5 Spending caps (paid plan, later)

The user will set caps in Billing, Spend Management, one for Workers and one for Durable Objects (https://developers.cloudflare.com/durable-objects/platform/spend-management/, https://developers.cloudflare.com/workers/platform/spend-management/). From the pages:

- A Durable Objects cap rejects new stub requests, SQL reads and writes, storage growth and `setAlarm` with "Exceeded the account's spending cap for Durable Objects". **Messages on already-open WebSockets are not blocked.** Running requests keep accruing duration.
- **Pending alarms are held, and alarms older than 7 days are deleted.** For retention that means a room whose deletion alarm fires during a cap block is deleted late or not at all. The `Registry` therefore also records `deadline`s and a Worker cron trigger (Free allows 5) can sweep expired rooms after a block. Phase 4 item. (An expired room that nobody opens costs only storage.)
- A Workers cap rejects invocations (HTTP error 1103). Static assets keep working, so the app still loads.
- The docs say nothing about spend limits on the **Free** plan; their examples concern paid usage. On Free, the hard daily limits above are the cap. Unverified: please check the dashboard once.

### 11.6 What a person sees (never silent)

A browser's `WebSocket` does not expose the HTTP status of a failed upgrade; it reports only a generic close (code 1006). So the client classifies failures itself:

1. **From the relay.** When the Worker runs, it answers a quota failure with an `Error{DailyLimit, retry_after_ms}` frame (accepting the upgrade and closing with code 1013) or HTTP `503 {"error":"daily_limit","retry_after_s":N}`. The Worker catches the storage exception and maps it. `retry_after` is the time to 00:00 UTC.
2. **Without a frame.** If the connection fails twice, the client calls `GET /api/status` (a Worker route that does not touch a Durable Object, same origin). `429` or `503` with `daily_limit`, or an HTTP error page, becomes the message. If even that fails and the browser is online, the message says the server is unreachable.
3. **Texts** (plain words, with the way out):
   - Daily limit: "The free demo server has reached its daily limit and resets at 00:00 UTC (in 5 h 12 min). Your drawing is saved on this device. You can keep working alone, run your own server (`drafftink-headless serve`), or try again later."
   - Spending cap: "This server has stopped taking new work because its monthly spending limit was reached. The owner can raise it. Your drawing is saved on this device."
   - Demo full: "The demo server is full of rooms right now. Try again in a few hours, or run your own server."
4. These are `Problem`s with a `Wait` or `AskAdmin` action. The `Status` text and `retry_at_ms` show a countdown. Local work continues, since the document is local-first. An unsent edit stays queued (`unsynced` dot) and is sent on reconnect.

---

## 12. Native and mobile

- **One crypto crate.** `drafftink-seal` (new, L1, builds for `wasm32-unknown-unknown`): keys, HKDF, `seal`/`open`, the record header, base64url, link parsing. Native, web and the tests call the same code. The browser does not use SubtleCrypto.
- **Transport.** `NativeTransport` (tokio-tungstenite and rustls) and `WebTransport` carry v2 frames unchanged. The native `Transport` only learns the path (`/r/<id>/ws`). `BlobTransport` is the new port (9.5). Native sends `ping` as a text frame like the browser (same code path in the sealed client).
- **Link handling.**
  - Web: the shell reads `location.hash`, calls `RoomLink::parse` (extended for the comma form), and runs `collab.connect`/`collab.join_room`. The Collaborate dialog gets a paste box that accepts a full link or `<id>,<key>`.
  - Desktop: `drafftink --room-link-file FILE`, or `DRAFFTINK_ROOM_LINK`, or standard input. **No flag takes the link itself**, for the same reason as the token (`docs/collab.md`, "The access token": arguments are visible to every user of the machine). An OS URL handler (`drafftink://room/<id>,<key>`, and `https://drafft.ink/#room=...` as a universal link) passes the URL to the app; this is user-initiated and short-lived, which I accept. Phase 1 builds the file, variable and paste-box paths; URL handlers are a packaging item (`docs/packaging.md`).
  - Mobile: the web app on a phone is the web path. A native mobile shell, if any, uses the universal link and the same crates. The fragment reaches an app that is opened by universal link (the OS passes the full URL). I did not verify this on iOS or Android.
- **Key storage.** Native: the workspace record in the app data folder, like the document (Q2). No keychain in v1.
- **Private instance.** A native client of `canvas.patwie.com` sends an Access service token as headers on the WebSocket handshake and on image requests. The browser uses the Access cookie. See 19.6.
- **`drafftink-headless`** as a client (headless MCP, `run`) joins sealed rooms with the same link via env or file, so agents can work in an encrypted room. The room link is a capability: an agent that has it can read the room. `collab.status` shows the room id and never the key.
- **wasm size.** XChaCha, HKDF and SHA-256 add about 32 KB to the wasm (my build, without `wasm-opt`). `cargo xtask size` budgets in `perf/budgets.json` need a bump of that order.

---

## 13. The command layer

Collab actions stay commands. The collab family is `docs/commands.md` section 2.16.

### 13.1 Changed commands

| Command | Change |
|---|---|
| `collab.connect {url, room?, on_local, token?}` | `room` accepts a v2 link or `<id>,<key>` besides a v1 name. A v2 room takes `if_missing` like `join_room`. `room` is a **secret argument** (`CommandSpec::secret_args` now lists `token` and `room`), so the ring shows `"<hidden>"` and no output repeats it. New refusals: `bad_room_link` ("The room link is damaged or incomplete. A private room link looks like `https://drafft.ink/#room=<id>,<key>` and must include the key after the comma. Ask for the link again."), `wrong_server_mode` ("This server only hosts end-to-end encrypted rooms, but `{room}` is a plain room name. Use a link that includes a key."). |
| `collab.join_room {room?, on_local, token?, if_missing}` | Omitting `room` generates a key (random 256 bits, the entropy port) and creates a sealed room (`mode = create`). An explicit plain name (letters, digits, `-`, `_`) still means a v1 plaintext room on a server that allows them. New arg `if_missing: "refuse" \| "restore" \| "create" = refuse`. Refusal `room_missing` (10.4) carries `data.has_cache` so a UI can offer "Restore". Refusal `local_too_large_for_room` (9.6). Output: `{room: <id>, e2e: true, state: "connecting"}`. It **does not return the link** (the key). |
| `collab.leave_room` | Unchanged. Also forgets the stored key of that tab (the document stays). |
| `collab.disconnect` | Unchanged. |
| `collab.status` | Adds `e2e: bool`, `expires_at_ms?`, `expiring: bool`, `images: {uploading, downloading, failed}`, `loading.phase: "images"`. `room` is the id, never the key. |
| `collab.set_pointer`, `collab.laser`, `collab.follow`, `collab.set_user` | Unchanged for callers. The laser flag can now be a real field because presence is sealed and version-free (the v1 limitation in `docs/collab.md` disappears for v2 rooms). |
| `image.insert` | In a room tab: refuses `image_too_large` and `room_images_full` (9.4). |
| `doc.export`, `doc.save` | New refusals `images_pending`, `images_unavailable`; new arg `missing_images: "refuse" \| "placeholder" = refuse` for `doc.export` (9.7). `doc.save` notes `images_missing`. |
| `doc.new`, `doc.open`, `tab.close` | The existing `in_room` refusals are unchanged. |

### 13.2 New command

| Command | Args | Refusals | Effect |
|---|---|---|---|
| `collab.share_link` | `copy: bool = true`, `reveal: bool = false` | `not_in_room`; `not_e2e`: "This room has no key, so it has no private link." | Builds the link from the tab's room and key. With `copy`, writes it to the clipboard through the clipboard port. With `reveal` it also returns `{link}`. Without it the output is `{copied: true}`. NoUndo, read-only, quiet in the ring. |

A new registration **fact**, `secret_output: bool`, marks commands whose output may hold a key (`collab.share_link`). `engine.recent` hides it. A registry test fails when a registration omits the field, as AGENTS.md requires for facts.

Added by the private instance (19.6, 19.8):

| Command | Args | Refusals | Effect |
|---|---|---|---|
| `collab.set_access` | `server?`, `client_id`, `client_secret` | `bad_args` (empty or over-long, value not repeated), `port_unavailable` (no `Secrets` port, as in the web build) | Stores an Access service token for that server origin in the `Secrets` port. NoUndo, never in a batch, both values in `secret_args`. |
| `collab.clear_access` | `server?` | `not_found` | Removes it. |
| `collab.delete_room` | `confirm: bool = false` | `not_in_room`, `not_confirmed`: "This deletes the room and all its pictures for everyone. Run it again with confirm=true.", `not_allowed`: "This server does not allow deleting rooms." | Sends `DeleteRoom` (member only). Only hosts that set `ALLOW_DELETE_ROOM` accept it (the private instance). |

`collab.status` also shows `access: "service_token" | "browser" | "none"` (never a value).

### 13.3 Not commands

Sealing, uploading, downloading, compaction, retention, the crash fence and presence budgeting are inside the sealed client and `collab_poll`, like the v1 sync. They change no document by themselves except `apply_remote`, which already is not a command.

### 13.4 Rules from AGENTS.md that this respects

- A refusal is a value with plain words and the way forward. A decision is an argument with a safe default (`if_missing = refuse`, `missing_images = refuse`, `reveal = false`).
- Domain rules live in the domain crates: key and record rules in `seal`, the room and retention rules in `relay`, image limits in `protocol`. Commands validate and delegate.
- Nothing below L6 touches egui or a socket. `seal`, `relay` and `collab` build for wasm32.
- The key never appears in `Debug` (newtype `RoomKey` with a placeholder, as `AuthToken`), logs, the ring, status or a refusal. Tests copy `tests/logging.rs`.

---

## 14. Crates and hosts

```text
L0  protocol     v2 messages, limits (image, room), RoomId, codec
L1  seal  (new)  RoomKey, subkeys, SealedRecord, seal/open, link parse     depends: RustCrypto crates, protocol
L1  relay (new)  sans-IO room core: membership, seq, segments, CAS, presence bundling,
                 rate limits, retention state machine, BlobStore + RelayStore traits (sync), memory stores
L4  collab       adds module `sealed` (client), BlobTransport, Transport unchanged
L5  engine       collab.* commands, workspace schema 2, blob cache, image refusals
L7  apps/drafftink-server   host: tokio + axum around `relay`; filesystem and sqlite stores; v1 stays
L7  apps/drafftink-worker   host: Worker + Durable Object around `relay`; DO SQLite store
```

- `seal` and `relay` need entries in `cargo xtask layers` and `cargo xtask wasm`. `relay` depends on `protocol` and `seal`'s header type only. It must not depend on Loro, tokio, or the clock.
- **`RelayStore` is synchronous.** Durable Object SQLite is synchronous (`sql.exec`), and the tokio host can run the same calls on its room task. A synchronous trait keeps the core simple and testable. Methods: `load_meta`, `append_segment`, `read_since`, `put_snapshot_cas`, `delete_all`. `BlobStore` is separate: `put`, `get`, `list`, `total_bytes`. Demo: DO SQLite for both. The private instance puts blobs in R2 behind the same trait (19.8). Later: S3 for self-hosters, R2 for the demo with a 2-day lifecycle rule as a backstop (R2 lifecycle rules count whole days and run within about 24 hours, so the alarm stays the primary deletion), and a persistent `RelayStore` for self-hosters (filesystem and SQLite exist today for v1 and are reused).
- **Worker language.** Phase 3 starts with a spike: workers-rs 0.9.0 (marked Latest on GitHub, 11 Nov, Rust shown as Beta in the docs) supports SQLite-backed Durable Objects, the Hibernation API handlers and `serializeAttachment` (https://github.com/cloudflare/workers-rs). If it is awkward, the fallback is a thin TypeScript Durable Object that calls the Rust `relay` core compiled to wasm. Either way the rules live in the one core. The Worker bundle limit is 64 MiB uncompressed on Free and Paid (https://developers.cloudflare.com/workers/platform/limits/), and the relay has no Loro, so the old worry about Loro's startup time and 128 MB in the isolate does not arise.
- **Hibernation.** A room with people present stays awake (presence flows). An empty room, or one whose people are idle and quiet, hibernates and costs no duration. Per-socket state (`client_id`, `last_heard_ms`, personal rate) goes in `serializeAttachment` (16 KiB max per socket). The segment buffer is flushed before the object can go idle.
- **Static site.** The Worker serves the web app with static assets (free, unlimited) and takes only `/r/*` and `/api/*` first (`run_worker_first` with patterns). So the app still loads when the Worker quota is spent, and everything is same-origin (no CORS, readable error statuses).
- **`drafftink-headless serve`** serves sealed rooms on `/r/<id>/ws` and `/r/<id>/f/<name>` next to the v1 `/ws`, and a flag `--no-plaintext-rooms` refuses v1 rooms (Q4). `--room-ttl` and `--room-grace` configure retention (off by default).
- **Two deployments, one Worker codebase.** `[env.demo]` and `[env.private]` in `apps/drafftink-worker/wrangler.toml` differ only in configuration: TTL, blob store, Access check, limits, allowed origins (19.8).
- The v1 plaintext server stays for self-hosters who want server-side validation, MCP agents without keys, and persistence they trust.

### 14.1 What phase 1a built, and where it differs from the text above (2026-11-22)

`crates/seal` (`drafftink-seal`, L1) and protocol v2 in `crates/protocol` (`docs/protocol.md` section 11) exist. Differences from this design, with the reason:

- **The record header, the room id and the membership check are in `drafftink-protocol`, not in `seal`.** `relay` and `seal` are both L1 and `cargo xtask layers` allows no same-layer edge, so the relay could not use a type of `seal`. The relay reads `RecordHeader` and checks `RoomId::accepts(&Proof)`; both need only SHA-256 and a constant-time compare. `seal` derives the token from the key and seals.
- **`SEAL_OVERHEAD` is 78, not 74.** The plaintext starts with a 4-byte length prefix (section 6.3), which the 74 left out. A sealed image of `MAX_IMAGE_BYTES` is `MAX_SEALED_IMAGE_BYTES` = 4 MiB + 78.
- **The room id is `SHA-256("drafft v1 room-id" || token)[..16]`** as in section 5.2 (the build brief wrote it without the label).
- **`file_id` for `HMAC(K_name, file_id)` is the 32 raw bytes of the SHA-256**, not its hex text.
- **`v2::ErrorCode` is a separate enum** from the v1 `ErrorCode`, because a v1 test fixes the v1 numbers at 1 to 14. The shared meanings keep their numbers; 15 to 18 are new; `ResyncRequired` and `InvalidUpdate` do not exist in v2.
- **`PeerJoined` carries `max_presence_per_sec`** (section 11.3 says so; the table in 7.2 left it out).
- **A `Join` is sent with `min_version == max_version`** because the layout of the rest depends on the version. A decoder given the other version's `Join` answers `DecodeError::WrongLayout`.
- **A record's `from`, `ctr` and `covers_seq` fields must agree with its header** (checked by the decoder).
- **No JSON debug form for v2.**
- **The nonce source is a trait** (`NonceSource`, `OsNonces` in production) so that tests can seal with a known nonce and compare with vectors from another implementation.

---

## 15. Testing plan

All tests run targeted. Phase 1 starts with the crypto and relay tests, which need no network.

| Area | Tests |
|---|---|
| **Known answers** (`crates/seal/tests/vectors.rs`) | XChaCha20-Poly1305 vector from draft-irtf-cfrg-xchacha-03 appendix A. HKDF-SHA256 vectors from RFC 5869 appendix A. Our own vectors for each label (`content`, `presence`, `image`, `image-name`, `membership`, room id) and one full sealed record, generated once and **cross-checked with an independent implementation** (libsodium via Python) before they are committed. The same file runs natively and under `wasm32` (a wasm test under Node in `cargo xtask wasm`), so native and web agree byte for byte. |
| **Tamper** | Flip every bit of a sealed record, one at a time: `open` fails, never panics (exhaustive for small records, sampled for large). Change each header field, the room id, and the kind: refused. Truncate at every length. Wrong key: refused. Same for sealed images, including a changed name. |
| **Replay and reorder** | A record sealed for room A is refused in room B. An update refused as a snapshot. Records applied in reverse order and twice give the same document (property test with two docs). A per-sender counter that skips triggers `Catchup`. A replayed presence record is ignored. |
| **Nonces** | A million seals never repeat a nonce; the same plaintext seals differently each time. |
| **Membership** | Right token joins. Wrong token, a token for another room, a truncated token: `Unauthorized`. `create` on an existing room with a good token is an open. `open` on a missing room is `RoomMissing` and creates nothing (storage dump is empty). The Worker rejects a non-member without calling the object (test double counts calls). |
| **Relay sees only ciphertext** (`apps/drafftink-server/tests/blind.rs`, and the Worker test) | Two clients draw text containing the marker `CANARY-7f3a91c2`, name themselves `Zelda-Canary`, set a cursor, and upload a PNG whose bytes contain the marker. Then the test searches **all relay storage and all recorded wire frames** for: the marker, the user name, the room key (raw, hex, base64, base64url), the root-derived subkeys, `file_id` as hex, and `SHA-256(png)`. The result must be zero hits. The search covers the in-memory store dump, the SQLite file, the filesystem store, and the Durable Object's persisted SQLite under miniflare. The token is allowed only in the join frame, not in storage. |
| **Sync** | Join, catch-up from snapshot plus tail, late join, two and three clients converge. Relay restarted between edits. Offline edits merge on reconnect (client edits offline, relay gains records, rejoin). Dropped `Ack`. A crashed tab (document reloaded from the workspace cache) resends exactly `export_updates_since(relay_vv)`. Gap in `seq` repaired by `Catchup`. |
| **Compaction** | Advice goes to one peer. Two racing `PutSnapshot`s: one wins, no live record lost. A stale `expected_snapshot_seq` is refused. Records below `covers_seq` are deleted in the same transaction. A joiner after compaction converges. |
| **Peer fallback** | Relay storage made to fail: a joiner gets state from a peer via `Direct`. |
| **Retention** (simulated time, `drafftink-relay` unit tests and a Durable Object test with `runDurableObjectAlarm`) | Join sets the deadline. A later join moves it. Edits and image requests do not. Alarm before the deadline re-arms. Alarm after the deadline with nobody connected deletes. Alarm with someone connected sets `expiring`, does not delete, rechecks in an hour. Last socket closing after the deadline deletes after the grace. A rejoin inside the grace cancels it. A stale socket (silent for over 2 minutes) does not hold a room. After deletion, storage is empty and a join with `open` is `RoomMissing`. |
| **Reseed** | Expired room plus cache restores the content (snapshot) and the images. Two restorers: exactly one wins, the other keeps a separate copy tab. Restore with no matching tab: `room_missing`, with `has_cache = false`. JSON-only cache restores only into an empty room. A restored tab with no link reconnects with `open` and never creates. |
| **Cache** | Workspace schema 2 round trip: room id, key, `relay_vv`, Loro snapshot. Reload with the link opens one tab, not two. An older build leaves a schema-2 index alone. |
| **Images** | `MAX_IMAGE_BYTES` exactly accepted, one byte more refused (`413`, and the engine refusal first). Room total at the limit and over (`507`). Image count limit. Non-member upload and download refused (`401`). Corrupt download (flipped bit, wrong name, wrong hash) shows "damaged picture". Same picture twice gives one stored name. Upload resumes after going offline. A `404` then late arrival draws the picture. Export offline from the cache (PNG and SVG), `images_unavailable` for an uncached picture, `placeholder` option. Saved JSON of a room opens in a local tab with embedded images. |
| **Schema** | Loro schema 1 to 2 migration is idempotent and equal on two peers. A schema-1 build refuses a schema-2 document. |
| **Hostile** | Random frames and random sealed bodies never panic (`hostile.rs` style, as `crates/collab/tests/hostile.rs`). A poison blob in the log: the crash fence skips it on the next start (native test with a simulated abort). |
| **Cost and limits** | Presence rate follows the budget as peers join and leave; a 50-peer simulation keeps incoming presence at or under `ROOM_PRESENCE_BUDGET`. Quota errors map to `DailyLimit` and to the plain texts of 11.6. |
| **Transport parity** | One scenario list (`join`, `edit`, `offline`, `images`, `expire`) runs against the in-memory loopback, the real `drafftink-headless serve` child process (as `tests/server.rs` does), and the Worker under `wrangler dev` or miniflare with the native client. The wasm client against the same scenarios is a manual step until the repo has browser tests (`docs/collab.md`: "covered by compilation only"). |
| **Logging** | The key, token and links never appear in any log line, `Debug`, ring entry, status or refusal (copy of `tests/logging.rs`). |

---

## 16. Phases and effort

Days are engineer-days including tests and docs. They are estimates, not measurements.

| Phase | Content | Days |
|---|---|---|
| **0. Spike** (1-2 days, before phase 3; one more day for the private-account part, counted in 3P) | A DO with SQLite, hibernation, an alarm, a `ping`/`pong` auto-response and a 4 MiB `PUT` under `wrangler dev`. Answers: `deleteAll` vs alarm, the CPU limit on Free, the error seen at the daily limit, `CF-Connecting-IP` inside the object, workers-rs vs TypeScript shell. | 2 |
| **1. Crypto and sealed protocol, tested against `serve`** | `seal` crate and vectors (3). Protocol v2 types and codec with strict decoding tests (3). `relay` core with membership, seq, segments, CAS, presence budget, retention state machine, memory store (5). Server host: v2 route in `drafftink-headless serve`, filesystem and SQLite stores, `--room-ttl` (3). Sealed client: join, catch-up, live, `relay_vv`, offline merge, compaction, presence, peer fallback, crash fence (7). Engine: link parsing, `collab.*` changes, `share_link`, `secret_output`, status, workspace schema 2 with Loro snapshot and room binding, startup matching (4). UI strings and dialog paste box (1). Test suites of section 15 for these parts (4). | 30 |
| **2. Images as files** | Loro schema 2 and migration (1). `FilePolicy::External`, blob cache namespace, deferred display (3). Relay HTTP image routes and quotas in core and `serve` (2). `BlobTransport` native, web, loopback (2). Uploader, downloader with retries and lazy fetch (2). Extraction on join, export refusals and `missing_images` (2). Limits as constants, refusals, docs (1). Tests (2). | 15 |
| **3. Cloudflare Worker and Durable Object** | Worker router, membership precheck, static assets, `/api/status` (2). DO host around the core with hibernation, attachments, SQLite store, alarm (4). Image routes and the 8 MB row rule (1). Quota-error mapping and plain texts (1). `Registry` object (1). Miniflare tests for retention and blind storage (2). | 11 |
| **3P. Private instance** (new, 2026-11-22) | Access JWT check in the Worker, R2 `BlobStore`, persistent mode and `DeleteRoom`, service-token support in native clients, per-environment config and deploy workflows, runbook, spike on the real account. Details and day counts in 19.13. Can start once phase 3 serves a room; ships before or alongside phase 4. | 11.5 |
| **4. drafft.ink deploy** | `wrangler.toml` (SQLite migration `new_sqlite_classes`), domain route, secrets (Turnstile secret), `Referrer-Policy`, CI deploy, spend limits set in the dashboard, Turnstile widget, runbook and status page, a smoke test against production (3). | 3 |
| **Later** | Persistent rooms for self-hosters (reuse `RelayStore`, 1-2). R2 for the demo with a 2-day lifecycle backstop, and an S3 `BlobStore` for self-hosters (2; the R2 store itself is built in 3P). Paid plan: change the plan, raise `MAX_PEERS_PER_ROOM` and the presence budget (1). Read-only links with signatures (5-8). Padding (1). A "restore point" from the previous snapshot (3). Web Worker import to remove the poison crash (4-6). Cron sweep after a spend-cap block (1). | |

Total for phases 0 to 4: about **61 days**. With the private instance (phase 3P) the total is about **73 days**. Phase 1 alone gives a working, tested encrypted room on `drafftink-headless serve`. Suggested order: 0, 1, 2, 3, **3P**, 4. If the private instance goes first, the `Registry` and Turnstile work (about 3 days) waits for the public demo.

---

## 17. Risks and surprises in the decisions

1. **The Free plan is a small demo tier.** About 28 active room-hours a day (13,000 GB-s at 128 MB), 100,000 requests a day with incoming WebSocket messages at 20:1, 100,000 rows written a day, and 5 GB of storage for the whole account (https://developers.cloudflare.com/durable-objects/platform/pricing/). "Dozens in one room" is feasible only with the presence budget (11.3). A daily limit hit is likely on a good day. The messages in 11.6 make it visible, and the paid plan fixes it without a code change.
2. **Spend limits and the Free plan.** The spend-management pages describe paid usage only. Whether a cap applies on Free is unverified, and what status or close code a client sees is unverified. The cap does not block messages on open sockets, and it holds alarms (deleted after 7 days), which can delay room deletion (11.5).
3. **A browser cannot read the HTTP status of a failed WebSocket upgrade.** Hence the `/api/status` probe (11.6). This needs the app and the Worker on one origin.
4. **A hostile member can crash every tab.** Loro 1.17 asserts on some corrupt inputs (`docs/crdt.md` section 8), the wasm release profile aborts on panic, and a sealed record the relay cannot inspect goes into the stored log, so every future joiner would crash too. The crash fence (8.5) limits the damage to one crash per person. A real fix is importing in a Web Worker. I recommend treating this as accepted for the demo, with the fence in phase 1.
5. **The app does not cache the room link today** (2.3). The user's assumption that people keep a cached copy is true for the document, false for its link to the room, and the cached copy has no CRDT lineage. Reseed needs the workspace schema 2 change in phase 1.
6. **A Durable Object's CPU limit on Free is unstated** (30 s default for objects, but the page says objects have the same limits as Workers, whose Free limit is 10 ms per request). The relay only copies bytes and runs SQL. The spike measures a 4 MiB upload and a 15 MiB snapshot.
7. **The Workers Rate Limiting binding** may not exist on Free (unstated). The design does not depend on it.
8. **The relay learns the membership token.** Anyone who can see TLS-terminated traffic (Cloudflare) could act as a member towards the relay, but cannot decrypt. A signature scheme removes this (5.3).
9. **The "first joiner registers a hash" rule** from the brief is replaced by `room id = H(token)`, which is stateless and cannot be squatted. This is a design change you may want to veto.
10. **`deleteAll()` and the alarm.** The docs are silent. The spike checks it. I always call `deleteAlarm()` first.
11. **workers-rs is labelled Beta** in the Cloudflare docs. The TypeScript shell is the fallback.
12. **No chunking is needed.** I checked each limit (32 MiB message, 100 MB request body, 8 MB row). If the user lowers nothing, there is no chunking path to build or test.
13. **Private instance (section 19), the risky parts:** the Access cookie on the WebSocket upgrade, service-token headers on an upgrade, and session expiry on an open socket are not stated in the Access docs; Zero Trust Free needs a payment card; R2, D1 and Access cannot be capped by Spend management; `workers.dev` URLs bypass hostname-based Access; and Cloudflare sees the email of every connection. See 19.12.

---

## 18. Open questions

1. **Replace "first joiner registers a hash" by `room id = H(membership token)`** (5.3)? Yes is my recommendation. It removes squatting.
2. **Store the room key in the local workspace record** so a restored room tab reconnects without the link (8.7)? The cached document is already plaintext there. Recommended: yes. If no, only the id is stored and every restart needs the link.
3. **Reseed by link only.** Opening the app with no link never revives an expired room (10.3). Agreed?
4. **`drafftink-headless serve` default.** Accept plaintext (v1) rooms as today alongside encrypted ones, or refuse them unless `--allow-plaintext-rooms`? MCP agents and server-side validation use v1 rooms.
5. **`MAX_IMAGE_BYTES`.** 4 MiB now, 2 MiB later? And should `image.insert` offer to shrink an oversized picture instead of refusing (a later argument `fit: "shrink"`)?
6. **The 60-second grace and the 1-hour recheck** (10.1). Fine, or different values?
7. **Demo capacity guard.** Is the `Registry` object (and a global cap of 2,000 rooms or 4 GB) acceptable before launch, and Turnstile on room creation?
8. **Worker language.** Rust with workers-rs if the spike allows, TypeScript shell with the Rust core otherwise?
9. **Second email list in the Worker** (`ACCESS_ALLOWED_EMAILS`, 19.5)? It catches a policy widened to "Everyone", at the cost of two lists to maintain. Default: off.
10. **Service token for native clients** over `cloudflared access login` (19.6)? One token per device.
11. **`collab.delete_room`** on the private instance (19.8)? It is the only way a room is ever removed there.
12. **Access session length**: 7 days instead of the 24-hour default (19.3 step 7)?
13. **Private instance plan:** stay on Workers Free for now, or start on the $5 paid plan so that daily limits never lock you out?

---

## 19. Private instance (canvas.patwie.com)

Added 2026-11-22. A second deployment of the same code, on the user's own Cloudflare account at `canvas.patwie.com`. It serves the same web app as drafft.ink plus the relay, same-origin. Rooms never expire. Only Google accounts on an allow-list can reach it, through Cloudflare Access. End-to-end encryption stays on, so Cloudflare still sees only ciphertext.

Facts in this section were read from the linked pages on 2026-11-22. "Unverified" marks anything I could not confirm in the docs. The **spike in phase 0 must test** the unverified points that decide the design (19.12).

### 19.1 Overview

```text
browser ──https──> canvas.patwie.com ──> [ Cloudflare Access: Google sign-in, Emails allow-list ]
                                              │ only signed-in, allowed requests pass
                                              v
                                         Worker "private"
                                           ├─ static assets (the web app, free, no Worker run)
                                           ├─ /api/status                         (Worker)
                                           ├─ /r/<room>/ws   ──> Durable Object "Room"  (SQLite: records, snapshot, image manifest)
                                           └─ /r/<room>/f/<name> ──> R2 bucket (sealed images, private)

desktop / headless / MCP ──https+wss, headers CF-Access-Client-Id / -Secret──> same hostname, same checks
```

What differs from the demo is configuration, not code (19.8): no `ROOM_TTL`, images in R2, Access check on, no Turnstile, no `Registry`, larger limits.

### 19.2 Is a database (D1) needed? No.

- **Rooms already have a persistent database.** Each room is a Durable Object with its own private SQLite database (`ctx.storage.sql`). It survives restarts and deploys. With `ROOM_TTL` off nothing deletes it. The relay's `RelayStore` (records, one snapshot, image manifest) lives there. See https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/ (SQL API, key-value API and alarms live on the same object) and https://developers.cloudflare.com/workers/platform/storage-options/ ("choose Durable Object storage for per-room or per-user state").
- **The room needs no cross-room query.** The Worker finds a room by id (`idFromName(room_id)`). A Durable Object "can only read and modify keys associated with that Object" (https://developers.cloudflare.com/durable-objects/platform/pricing/ and the storage-options page).
- **Images go in R2** because a Durable Object holds at most 10 GB and an image is a plain blob that needs no SQL.
- **Where D1 would make sense later.** D1 is a managed SQLite database for relational data and ad-hoc SQL across all rows (https://developers.cloudflare.com/workers/platform/storage-options/). Free plan: 5 million rows read and 100,000 rows written per day, 5 GB total, 10 databases, 500 MB per database (https://developers.cloudflare.com/d1/platform/limits/). It fits:
  1. **A per-user room list** ("My rooms"), keyed by the Access email, shared across devices. **E2E catch:** a list entry that contains the room link would hand the key to Cloudflare. An entry may hold only a room id, a last-visit time, and an opaque blob **sealed on the client** with a key the server never has. Otherwise the list stays local, as it is today (the workspace record, 8.7). So D1 is useful here only with extra client work, and I recommend not building it now.
  2. **An admin overview** (rooms, bytes, last visit) with SQL. A Durable Object cannot answer "all rooms", and an index table in D1 can. The `Registry` object of 11.4 can hold simple counters instead.
  3. **Invites or per-room ACLs** if rooms ever get owners. That is a product decision outside this design.
- If D1 is added, it is a binding in the `private` environment and a new `RoomIndex` trait in the Worker host. The relay core does not change.

### 19.3 One-time setup the user does by hand

Plain words, in this order. Menu names in Cloudflare move; the linked page names them. Estimated time: about 1.5 hours, most of it DNS and Google consoles.

**Step 0. Prerequisites.** A Cloudflare account that belongs to you (this will be the *private* account, see 19.11), a payment card (Zero Trust needs one even on the free plan, R2 very likely does), and access to the registrar where `patwie.com` is registered.

**Step 1. Put the `patwie.com` zone on Cloudflare.**
1. Dashboard: Add a domain, `patwie.com`, Free plan. Cloudflare scans your existing DNS records. **Check the imported list line by line** (mail `MX`, `TXT` for SPF/DKIM, your other sub-domains). Anything missing stops working after the switch.
2. At the registrar, change the nameservers to the two Cloudflare gives you. Wait until the zone shows "Active".
3. Why: a Workers Custom Domain needs "an active Cloudflare zone" in the same account, and cannot be created on a hostname that already has a `CNAME` record (https://developers.cloudflare.com/workers/configuration/routing/custom-domains/). So `canvas.patwie.com` must have no existing record. Cloudflare creates the DNS record and the certificate itself.

**Step 2. The Worker and its custom domain.** Nothing to click. The first `wrangler deploy --env private` (19.9) creates the Worker `drafftink-relay-private` and binds it to `canvas.patwie.com` because the config says `routes = [{ pattern = "canvas.patwie.com", custom_domain = true }]`. Do the first deploy yourself with `wrangler login` (it uses your full login, not a scoped token). Turn **off** `workers.dev` and preview URLs for this Worker (`workers_dev = false`, `preview_urls = false`), so the only way in is the protected hostname.

**Step 3. R2 bucket (images).**
1. Dashboard: R2 object storage. Enable R2 (checkout flow; the docs say you need "an R2 subscription", https://developers.cloudflare.com/r2/get-started/; the free allowance is 10 GB-month storage, 1 million Class A and 10 million Class B operations a month, free egress, https://developers.cloudflare.com/r2/pricing/). Whether a card is required is not stated in the docs, a community thread says yes (unverified).
2. Create the bucket `drafftink-private-blobs`. Leave it **private**: do not enable the `r2.dev` URL and do not attach a public custom domain. Buckets are private by default (https://developers.cloudflare.com/r2/buckets/public-buckets/).
3. **Lifecycle rule: leave it off.** Rooms never expire, so an age rule would delete pictures of live rooms. It is useful only as a backstop if you later set a `ROOM_TTL`: then add a rule of `TTL + 2 days` (`wrangler r2 bucket lifecycle add drafftink-private-blobs --expire-days N`). Rules count whole days and run "typically within 24 hours" of the expiry time (https://developers.cloudflare.com/r2/buckets/object-lifecycles/), so a lifecycle rule can only be a backstop for the demo's 48-hour rule too, never the primary deletion.

**Step 4. Zero Trust (Cloudflare Access), free plan.**
1. Dashboard: Zero Trust. Choose a **team name**. Your team domain becomes `<team>.cloudflareaccess.com`. Choose it with care: if the organization is ever deleted the name stays locked (https://developers.cloudflare.com/cloudflare-one/faq/getting-started-faq/).
2. Choose the **Free** plan and enter payment details. The setup page says the step is "still needed but you will not be charged" on Free (https://developers.cloudflare.com/cloudflare-one/setup/). The Free plan is **up to 50 users** (https://blog.cloudflare.com/teams-plans; I could not load the official pricing page, so treat the exact number as unverified). A "user" is a seat that stays used once a person logs in, until you remove them, not a monthly count (https://developers.cloudflare.com/cloudflare-one/team-and-resources/users/seat-management/). **Service tokens do not use seats.** A community thread reports consent to overage charges at checkout and no documented hard $0 cap (unverified): keep the allow-list well under 50.

**Step 5. Google OAuth client (Google Cloud Console).** Steps follow https://developers.cloudflare.com/cloudflare-one/integrations/identity-providers/google/.
1. Create a project. APIs & Services, OAuth consent screen: audience **External**, app name, support email, contact email.
2. Credentials, Create OAuth client, type **Web application**.
3. Authorized JavaScript origin: `https://<team>.cloudflareaccess.com`. Authorized redirect URI: `https://<team>.cloudflareaccess.com/cdn-cgi/access/callback`.
4. Copy the client ID and client secret.
5. Publishing status: **Testing** is enough to start if you add every allowed email as a "test user" (Google caps test users at 100). Google says apps that request only `openid`, `email` and `profile` are not limited to the test list (https://developers.google.com/identity/protocols/oauth2/production-readiness/overview). I did not confirm that Cloudflare asks for only those scopes (unverified). If sign-in is refused for someone, add them as a test user, or publish the app.

**Step 6. Google as a login method in Zero Trust.** Zero Trust: Integrations, Identity providers, Add new, Google. Paste the client ID as "App ID" and the secret. Use the **Test** button. (The older path "Settings, Authentication" no longer exists.)

**Step 7. The Access application with the Emails policy.** Zero Trust: Access controls, Applications, Add an application, **Self-hosted**: https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/.
1. Public hostname: `canvas.patwie.com`, no path, so it covers the whole host: pages, `/r/*` WebSockets, image requests, `/api/*`.
2. Accepted login method: Google only, with "instant authentication" on.
3. Policy 1: action **Allow**, rule **Include: Emails** with your list (up to 1,000 emails per rule). Nothing else (no "Everyone", no email domain).
4. Policy 2 (for the desktop app and agents, step 8): action **Service Auth**, Include: the service token(s). A service token is let in only by a **Service Auth** policy, not by Allow (https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/). One application may hold both policies.
5. Session duration: the default is 24 hours (range: immediate to 1 month). I suggest 7 days for a private whiteboard. A person signs in again when it ends (19.6).
6. Copy the application's **AUD tag** (Additional settings). It is a config value for the Worker (19.5).
7. Do **not** use the "Protect this Worker" shortcut. Worker-level Access policies do not support WebSockets; the upgrade returns 403 (https://developers.cloudflare.com/workers/configuration/cloudflare-access/). Hostname-based Access, as above, is the supported way.

**Step 8. Service tokens (only if you use the desktop app, headless or MCP).** Zero Trust: Access controls, Service credentials, Service tokens, Create. One token per device or agent, so one can be revoked alone. Copy the **Client ID** and the **Client Secret** at once (the secret is shown once). The default life is one year (set at creation; a non-expiring option is unverified). Add each token to Policy 2. New organizations (created on or after 2026-10-05) have "strict service token authentication" on, which means a service-token request is judged only by Service Auth policies (same service-tokens page), so Policy 2 is required, not optional. Up to 50 tokens per account (https://developers.cloudflare.com/cloudflare-one/account-limits/).

**Step 9. Spending caps and alerts.** Dashboard: Billing, Spend management: set a limit for **Workers** and one for **Durable Objects** (https://developers.cloudflare.com/billing/manage/spend-management/). **R2, D1 and Access cannot be capped there**: the page says a Workers limit does not include "Durable Objects, Workers AI, Workers KV, R2, or other products", and only Workers and Durable Objects have limits. For R2 use Notifications (usage alerts), and a Budget alert (Billing, Billable Usage; for pay-as-you-go accounts, email only, never blocks, https://developers.cloudflare.com/billing/manage/budget-alerts/). R2 cost at your scale is cents: $0.015 per GB-month after 10 GB free.

**Step 10. Give the Worker its config and deploy** (19.9): set `ACCESS_TEAM_DOMAIN` and `ACCESS_AUD`, run the first deploy, then the smoke test (19.10 list).

### 19.4 Does the Access cookie cover the WebSocket and the image requests? (the sources)

- **Access in front of a Worker custom domain** is supported and documented: "every request is checked before your Worker runs. Approved visitors are let through, while everyone else is shown a login page or blocked" (https://developers.cloudflare.com/workers/configuration/cloudflare-access/). Hostname-based Access takes precedence over Worker-level Access (same page).
- **Same-origin cookie.** Access sets the `CF_Authorization` cookie on the application's hostname (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/). The page is `canvas.patwie.com`, and so are `fetch('/r/<id>/f/<name>')` and `new WebSocket('wss://canvas.patwie.com/r/<id>/ws')`, so the browser sends the cookie on all of them. For the WebSocket upgrade this follows from RFC 6455 section 4.1: the client request "MAY include any other header fields, for example, cookies" (https://www.rfc-editor.org/rfc/rfc6455), and an upgrade is an ordinary HTTP request that Access checks like any other. **The Access docs do not say this in so many words about WebSockets** (unverified). The phase 0 spike tests it with a real login. The reason the design is same-origin is to avoid the CORS cookie problems described in https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/cors/.
- **Not the same as Worker-level Access.** Only the hostname-based application is used (step 7.7).
- **A caveat for private browser windows:** they may block the cookie and break XHR (authorization-cookie page).

### 19.5 Defence in depth: the Worker validates the Access JWT too

Access is the gate. The Worker is the second lock, so a misconfigured Access application (deleted, wrong hostname, a policy set to "Everyone", `workers.dev` left on) cannot open the instance. This is a Cloudflare recommendation too: validate the JWT in the origin (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/).

**Which requests.** The Worker runs first for `/r/*` and `/api/*` (`run_worker_first` patterns). The static app files are the public open-source build, so they need no Worker check (and they do not use Worker quota). Every WebSocket upgrade and every image request is checked.

**What it checks** (`Cf-Access-Jwt-Assertion` header; the cookie is the fallback):

| Check | Value |
|---|---|
| Signature | RS256, key chosen by the token's `kid` from `https://<team>.cloudflareaccess.com/cdn-cgi/access/certs` (the `keys` array, not `public_cert`) |
| `iss` | `https://<team>.cloudflareaccess.com` (config `ACCESS_TEAM_DOMAIN`) |
| `aud` | contains the application's AUD tag (config `ACCESS_AUD`) |
| `exp`, `nbf` | `exp` in the future, `nbf` not in the future |
| Algorithm | only `RS256`. `none`, `HS256` and others are refused (no algorithm confusion). |
| Identity present | `email` (a person) **or** `common_name` (a service token). Neither: refused. |

Notes from the docs: the signing keys rotate every 6 weeks and the previous key stays valid for 7 days. Match on `kid`, cache the key set, and refetch on an unknown `kid` (the docs give no TTL; I use one hour and at most one refetch per minute). The service-token JWT has `common_name` equal to the client id and no `email` (https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/). The AUD tag stays the same unless the application is recreated.

**How, in the Worker.** There is no Rust sample in the Cloudflare docs. I propose a small pure module `access_jwt` in `apps/drafftink-worker`: parse the three parts, check the header and claims against injected `now`, `team`, `aud` and a key set, and verify the RS256 signature through Web Crypto `SubtleCrypto.verify` (`RSASSA-PKCS1-v1_5`, SHA-256) rather than an RSA crate in wasm, so the signature check takes microseconds, not milliseconds, inside the Free plan's 10 ms CPU limit. Because the claims logic is pure, it is unit-tested natively with fixture keys (19.10). `ctx.access` / `getIdentity()` is not a replacement: it is not available to Workers that serve static assets, and it is undefined when Access did not authenticate the request (https://developers.cloudflare.com/workers/configuration/cloudflare-access/). Using `ctx.access` additionally is allowed but not relied on.

**Optional second list.** Access's allow-list is the single source of truth. A copy in the Worker (`ACCESS_ALLOWED_EMAILS`, secret) would catch a policy widened to "Everyone". It also doubles the lists to maintain. Default: off. Open question Q9.

**Refusal.** A request without a valid assertion gets HTTP `401` and a small JSON body with a plain message, never a redirect and never silence:
- no header: `{"error":"access_required","message":"This server is only open to signed-in, approved users. Open https://canvas.patwie.com in your browser and sign in with Google. In the desktop app, enter the access token for this server in Preferences."}`
- bad signature, wrong audience or expired: `{"error":"access_invalid","message":"Your sign-in is not valid for this server or has expired. Reload the page and sign in again."}`
- no `Cf-Access-*` and an `Origin` that is not ours: refused before parsing.
The Worker logs the reason code, never the token, never the email.

**Identity stays out of the room.** After the check the Worker **strips** `Cf-Access-Jwt-Assertion`, `Cf-Access-Authenticated-User-Email`, `CF-Access-Client-*` and the `CF_Authorization` cookie from the request before it calls the Durable Object or R2. The relay therefore never sees who connected. Cloudflare itself still can (19.7).

### 19.6 Native, desktop, MCP and headless clients: an Access service token

These clients cannot do the Google login in a browser. I considered two ways and pick the first.

| | Service token (**chosen**) | `cloudflared access login` |
|---|---|---|
| How it works | Client ID and secret sent as headers `CF-Access-Client-Id` and `CF-Access-Client-Secret` on every request, including the WebSocket handshake. Access policy action **Service Auth**. | The `cloudflared` program opens a browser, the person signs in, `cloudflared access token -app=<url>` prints a JWT, sent as header `cf-access-token` (https://developers.cloudflare.com/cloudflare-one/tutorials/cli/). |
| Unattended use (MCP server, `drafftink-headless run`, CI, a laptop closed overnight) | Works | No. A new browser login is needed when the session ends. |
| Extra software | None | The `cloudflared` binary on every machine. |
| Cost | No seat (service tokens do not use seats) | One seat per person (already used for the browser). |
| Identity | A **machine**, not a person: the origin sees `common_name` (the client id), no email | The person's email |
| Revoke and rotate | Per token: rotate the secret (the client id stays, the old secret can live 1 hour to 30 days), or revoke at once. Expires in one year by default; Cloudflare alerts a week before. | By removing the person, or session expiry |
| Rust support | Plain headers on tungstenite and an HTTP client. Fully ours. | Shelling out. The login flow is not documented for re-implementation (unverified). |

**Why the token.** The desktop app, the headless MCP server and scripts must work without a person at a browser, and plain headers are the least code and the fewest moving parts. The disadvantage (a machine identity and a secret on disk) is real, so the rules below apply. The `cloudflared` route stays an alternative for a person who wants an identity on the desktop and does not mind the extra binary. **Managed OAuth** (authorization code with PKCE, tokens refreshed by the client, https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/managed-oauth/) would give native clients a per-person login without a secret on disk, but it needs a client that implements RFC 8707 and a loopback redirect. It is the right later upgrade and is not designed here.

**How it works in the app.**
- A new port `Secrets` (native: the OS credential store through the `keyring` crate if it is available on the platform, else a `0600` file in the app data folder; headless: a file or environment variable). It holds `AccessCredentials {client_id, client_secret}` **per server origin**.
- **Preferences** has, per server, "Access token for this server" with two fields and "Remove". It calls the commands below. It is entered once.
- The transports add the two headers to the WebSocket handshake (tungstenite takes arbitrary request headers) and to each blob request (`BlobTransport`). Rules, all tested:
  - Sent **only** to the stored origin over `https`/`wss`, never to another host, and never over plain `ws://` to a non-loopback host (the existing `UrlPolicy`).
  - **Redirects are not followed** with these headers. A `302` to `*.cloudflareaccess.com` means the token was refused, and is reported as `access_denied` (19.6, texts).
  - The secret has no `Display`, `Debug` prints `AccessCredentials(<hidden>)`, and it is held in a zeroizing buffer, like `AuthToken` and `RoomKey`.
- **Not in settings.** Like the v1 token and the room key (`docs/collab.md`, "The access token"): a setting would be readable by `settings.get`, written to disk in the clear and echoed. The credential store is the only place, and `collab.status` shows `access: "service_token" | "none"`, never a value.
- **Not in links.** A room link never contains it. The link is a room capability. Access is a server capability. They travel separately.
- **Headless and MCP:** `DRAFFTINK_ACCESS_CLIENT_ID` and `DRAFFTINK_ACCESS_CLIENT_SECRET_FILE` (a file path; no flag takes the secret itself, as for the room link in section 12), or the credential store.
- **A browser** never uses the token. It uses the cookie.

**Commands** (collab family): `collab.set_access {server?, client_id, client_secret}` and `collab.clear_access {server?}`. NoUndo, never in a batch, both `secret_args` (the ring shows `"<hidden>"`). Refusals: `bad_args` for an empty or over-long value (without repeating it), `port_unavailable` when there is no `Secrets` port (the web build, where the browser holds the cookie). `server` defaults to the chosen server.

**Texts the person sees** (plain words, from `Problem`s with the action `AskAdmin` or `UpdateApp`):
- No credentials and a `401`/`302` from Access: "This server is private. Open Preferences, Collaboration, and enter the access token for `canvas.patwie.com`. The server's owner can create one."
- Refused token: "The server did not accept the access token. It may have expired or been revoked. Ask the server's owner for a new one."
- Browser, expired session: "Your sign-in has expired. Reload the page to sign in again. Your drawing is saved on this device."

**Expired sessions in the browser.** The default reaction of Access to an expired session is a `302` to `<team>.cloudflareaccess.com`. For `fetch` that is a cross-origin redirect and fails as a CORS error. The fix in the docs: send the header `X-Requested-With: XMLHttpRequest`, and Access returns `401` instead (https://developers.cloudflare.com/cloudflare-one/access-controls/access-settings/session-management/, "AJAX"). The web `BlobTransport` and the `/api/status` probe send it. The WebSocket cannot carry a custom header, so a failed upgrade is classified by the probe, as in 11.6: `/api/status` with that header returns `401` (Access) or the Worker's own `401` JSON (a missing or bad assertion), and the two give different texts. An already open WebSocket after the session ends: the docs do not say; Access checks each HTTP request and an upgrade is one request, so the open socket most likely stays up until it drops (unverified). The reconnect then fails and shows "Your sign-in has expired". Spike item.

### 19.7 Threat model changes

**What Access adds.**
- Only allow-listed Google accounts, and holders of a listed service token, reach the Worker **at all**. A stranger who learns a room id, a room link or the hostname gets a login page. This is a layer in front of the room key, not a replacement: a signed-in, allow-listed person who has never seen a room link still cannot read or join it.
- It limits the denial-of-service surface: flooding, junk uploads and room creation need an account on the list. A malicious room member (4.3) must now also be an allowed user (or a holder of a service token).
- It leaves an attacker who knows only a room id unable even to ask the relay a question.

**What Cloudflare (and Google) still see, now in addition to 4.1.**
- **Email identities of everyone who connects.** Access sees who logs in and when. The Worker sees the verified JWT (with the email, or the service token's client id) on every WebSocket upgrade and image request before it strips the headers. So Cloudflare can link "this email was connected to room id X at this time", and from the DO's socket count, "these people were in the room together". The relay code stores and logs none of it (19.5), but Cloudflare can see it in its own logs and analytics. **The room's content stays sealed.** If linking people to rooms matters, do not use this instance for that room.
- **Google** sees that the person signed in to Cloudflare Access for this team (the OAuth redirect), not the rooms.
- **Service-token holders** appear as machines (`common_name`), not people.
- The R2 bucket holds sealed images under keyed names; its operations log shows object keys (`r/<room>/<name>`), sizes and times, but not content (as before).

**Accepted, as before.** Removing a person from the allow-list stops them connecting, but does not take back a link they hold or a copy they saved. There is still no key revocation (5.4). Access does not tell members apart: anyone on the list who has the link is a member.

### 19.8 Configuration: one codebase, two environments

`apps/drafftink-worker/wrangler.toml` has `[env.demo]` and `[env.private]`. **Bindings and `vars` are not inherited, so each environment repeats them** (https://developers.cloudflare.com/workers/wrangler/environments/). Each environment deploys as its own Worker (`drafftink-relay-demo`, `drafftink-relay-private`) and has its own Durable Object namespace and storage (https://developers.cloudflare.com/durable-objects/reference/environments/). The code reads only these settings:

| Setting | `demo` (drafft.ink) | `private` (canvas.patwie.com) |
|---|---|---|
| Account | the demo account | the private account (19.11) |
| Route | custom domain `drafft.ink` | custom domain `canvas.patwie.com` |
| `workers_dev`, preview URLs | off | **off** |
| `ROOM_TTL_SECS` | `172800` (48 h) | unset: **no expiry**, no alarm, `Joined.expires_at_ms = 0` |
| `ROOM_LEAVE_GRACE_SECS` | 60 | not used |
| `BLOB_STORE` | `do` (the room's SQLite) | `r2`, binding `BLOBS` to bucket `drafftink-private-blobs` |
| `ACCESS_REQUIRED` | false | **true**, with `ACCESS_TEAM_DOMAIN`, `ACCESS_AUD` |
| Turnstile on room creation | on (secret `TURNSTILE_SECRET`) | off (Access already gates creation) |
| `Registry` object and `MAX_ROOMS_DEMO`, `MAX_DEMO_BYTES` | on | off |
| `ALLOWED_ORIGINS` (WebSocket `Origin` check) | `https://drafft.ink` | `https://canvas.patwie.com` |
| `MAX_ROOM_STATE_BYTES` | 32 MiB | 256 MiB (a room's database has a 10 GB limit) |
| `MAX_ROOM_IMAGE_BYTES`, `MAX_ROOM_IMAGES` | 50 MiB, 200 | 500 MiB, 2,000 |
| `MAX_IMAGE_BYTES` | 4 MiB | 4 MiB (same: it is the sealed-file limit the clients know) |
| `MAX_PEERS_PER_ROOM`, `ROOM_PRESENCE_BUDGET` | 64, 40 msg/s | 64, 40 msg/s |
| Status texts | free-plan "daily limit" texts | the same code; the text names the owner |
| Migration | `new_sqlite_classes = ["Room", "Registry"]` | `new_sqlite_classes = ["Room"]` |

Limits are named constants in `drafftink-protocol` with the **ceilings**; an environment may only lower or, within the ceilings, raise them, and announces its values in `Joined.limits`, so a client never hard-codes a host's numbers. Durable Object classes must be SQLite-backed on Free (https://developers.cloudflare.com/durable-objects/platform/pricing/). Wrangler can declare them with `[[migrations]]` and `new_sqlite_classes`, or with the newer `exports` map (https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/); the second cannot be reverted after the first deploy, so I use the legacy `[[migrations]]` until the spike confirms the new form works with workers-rs. The account is chosen by `CLOUDFLARE_ACCOUNT_ID` and the token, not by the file (whether a per-environment account id can be pinned is unverified).

**Web app build.** One build for both. `collab.server_url` defaults to `/ws` on the web, "this page's server" (`docs/commands.md` settings), and the v2 client derives `/r/<id>/ws` and `/r/<id>/f/<name>` from the page origin. So **the web app needs no server URL for either deployment**, and no `ALLOWED_ORIGINS` surprises. The static files are the same `web-dist`, uploaded as the Worker's assets in each environment.

**R2 as the blob store.** The `BlobStore` trait of section 14 gets an R2 implementation in the Worker host. Layout: key `r/<room_id>/<name>`. The per-room quotas stay in the room's Durable Object:
- *Upload:* the Worker checks Access, the JWT and the membership token, then forwards the request to the room's Durable Object. The object checks the size limits and the quotas (`MAX_ROOM_IMAGE_BYTES`, `MAX_ROOM_IMAGES`, from its manifest table) and streams the body to R2 with its own `BLOBS` binding, then records `{name, size}`. One object request per upload.
- *Download:* after the membership check the Worker reads R2 directly and streams the object. No object is woken. Class B operation only.
- *List:* from the object's manifest (no R2 list call).
- *Streams:* the R2 docs warn that buffering a large body hits the 128 MB Worker memory limit, so bodies are streamed (https://developers.cloudflare.com/r2/api/workers/workers-api-usage/). Whether workers-rs `Bucket::put` accepts a stream is unverified; the TypeScript shell does. R2 allows one write per second to the same key (the 429 case); the client's upload of the same name is serialized.
- *Delete a room.* Rooms never expire, so storage would only grow. I add a member-only `DeleteRoom` message (tag 0x15) and command `collab.delete_room {confirm: bool = false}` (refusal `not_confirmed`: "This deletes the room and all its pictures for everyone. Run it again with confirm=true."), enabled by `ALLOW_DELETE_ROOM` in the private environment. The object lists `r/<room>/` in R2, deletes the objects (deletes are free, https://developers.cloudflare.com/r2/pricing/), then runs `deleteAlarm()` and `deleteAll()`. This is the only deletion path with `ROOM_TTL` off.
- R2 cost: first 10 GB-month free, then $0.015 per GB-month; free egress.

### 19.9 Deploy

**Manual:** `wrangler deploy --env private` from `apps/drafftink-worker` (and `--env demo` against the other account). Secrets per environment: `wrangler secret put ACCESS_AUD --env private` (and the others; secrets are per environment too).

**GitHub Actions**, separate from the demo deploy: two workflow files, `.github/workflows/deploy-private.yml` and `deploy-demo.yml`. Each uses `cloudflare/wrangler-action` with its own secrets (https://developers.cloudflare.com/workers/ci-cd/external-cicd/github-actions/). They run on a manual trigger (`workflow_dispatch`) and on a version tag, **never on every push**, because a deploy disconnects open WebSockets. The private workflow is protected by a GitHub Environment `private` with a required reviewer, so a pull request from a fork cannot reach its secrets. Secrets: `PRIVATE_CLOUDFLARE_API_TOKEN` and `PRIVATE_CLOUDFLARE_ACCOUNT_ID`, distinct from the `DEMO_` pair.

**The API token** (an *Account API token*, not your global key), created in the **private** account. Start from the "Edit Cloudflare Workers" template, then remove what is not needed (https://developers.cloudflare.com/fundamentals/api/reference/template/):

| Scope | Permission (dashboard name) | Why |
|---|---|---|
| Account | Workers Scripts: Edit | upload the Worker, apply the Durable Object migration |
| Account | Workers R2 Storage: Edit | bucket binding on deploy (and the one-time `lifecycle` command if ever used) |
| Account | Account Settings: Read | wrangler looks up the account |
| Zone (`patwie.com` only) | Workers Routes: Edit | the route / custom domain |
| User | User Details: Read, Memberships: Read | in the template; wrangler uses them to find the account |
| Zone (`patwie.com` only), **only if the deploy fails without it** | DNS: Edit | a custom domain creates a DNS record and a certificate; the docs do not list the permission it needs (unverified) |

Remove from the template: Workers KV Storage Edit, Workers Tail Read (unless you want `wrangler tail` from CI). Scope the token to the **one account** and the **one zone**. Do the first deploy by hand with `wrangler login`; if the CI deploy later fails on the route, add the DNS permission. **No** Zero Trust, Billing or R2 object permissions are needed. Access is configured by you in the dashboard (steps 4 to 8), not by CI. The token is created for a person to rotate yearly.

### 19.10 Tests added by the private instance

| Area | Tests |
|---|---|
| `access_jwt` (pure, native) | Valid token passes. Wrong `aud`, wrong `iss`, expired `exp`, future `nbf`, unknown `kid` (one refetch, then refuse), `alg: none`, `HS256` signed with the public key (algorithm confusion), a missing identity claim, a truncated or oversize token, a service-token JWT with `common_name` and no email: each refused with the expected code and text. Key rotation: both keys in the set verify. |
| Worker routing | Without a valid assertion, `/r/*` and `/api/*` give `401` JSON and **do not call** the Durable Object or R2 (test double counts calls). With one, the headers and cookie are stripped before the object sees the request. A request on a `workers.dev`-style host without the assertion is refused (backstop test). |
| Service token (native) | The headers are on the WebSocket handshake and every blob request, only to the stored origin, never to another host, never over `ws://` to a non-loopback host, never after a redirect. The secret is absent from every log line, `Debug`, the ring, `collab.status`, refusals and links (copy of `tests/logging.rs`). `collab.set_access` and `clear_access` round trip. A `302` from Access gives `access_denied`. |
| Persistent room | With `ROOM_TTL` off: no alarm is ever set, `Joined.expires_at_ms = 0`, a room survives a simulated 400 days with nobody connected, and a restart of the object, and still opens. `DeleteRoom` removes the state, the alarm and every R2 object under the prefix, and nothing else. |
| R2 blobs | The blind-storage test of section 15 also searches the persisted R2 objects (miniflare) for the marker. Quota is enforced by the object, not by R2. Download needs no object call. A failed R2 write leaves no manifest row. |
| Limits per environment | `Joined.limits` carries the environment's values. A client obeys them (a 3 MiB image is fine at 4 MiB, refused when the host announces 2 MiB). |
| Setup smoke test (manual checklist, runbook) | Signed out: the hostname shows the Google login. Signed in with an allowed email: the app loads, a room opens, a picture uploads and loads on a second device. An email that is not on the list is refused by Access. `workers.dev` URL is dead. A service-token client connects, a token with no Service Auth policy is refused. After deleting the Access application the Worker still answers `401`. |

### 19.11 Keep the demo on a different Cloudflare account

Spending limits are **account-wide per product** and cannot target one Worker or namespace (https://developers.cloudflare.com/billing/manage/spend-management/). The Free plan's daily limits are **per account** too: 100,000 Workers requests a day, which "resets at midnight UTC" and then gives error 1027 (https://developers.cloudflare.com/workers/platform/limits/), and the Durable Objects limits of 11.1 (the pricing page presents them as plan limits; "per account" is implied, not worded that way). So if both instances share one account:

- a busy day on the public demo would use up the daily quota and **lock you out of your own private instance**, and
- one spending cap would have to serve a public, abuse-prone service and a private one.

Put **the demo on its own account** (`drafft.ink`, Workers Free, its own caps) and **the private instance on your own account** (`canvas.patwie.com`, own caps, own Zero Trust organization, own R2). Two accounts mean two API tokens and two sets of GitHub secrets, as in 19.9. The private account may stay on Workers Free for a few people; if its daily limits ever bite, switching to the paid plan is again a plan change, not a code change.

### 19.12 Risks, unverified points and spike items for the private instance

1. **Cookie and the WebSocket upgrade** are not stated together in the Access docs (19.4). If the browser did not send the cookie, the instance would not work. RFC 6455 says cookies are allowed on the handshake, and I expect it to work. Spike item 1, with a real login.
2. **Service-token headers on a WebSocket upgrade** are not documented (19.6). Native clients can set handshake headers, so I expect it to work. Spike item 2.
3. **Open WebSocket after the Access session expires** is not documented; I expect the socket to live until it drops. Spike item 3.
4. **Whether Access-blocked requests count against Workers requests** is not stated. Likely not (the Worker never runs), unverified.
5. **R2 needs a payment method** (likely yes; not stated outright). **Zero Trust Free needs one** (stated). A community thread says the Zero Trust checkout asks consent to overage and that no hard $0 cap is documented (unverified).
6. **R2, D1 and Access cannot be capped** by Spend management. Only Workers and Durable Objects can. Use alerts, not caps, for R2.
7. **`workers.dev` and preview URLs** bypass hostname-based Access. They are turned off in the config and the Worker's JWT check is the backstop. Do not skip either.
8. **Email identities are visible to Cloudflare** in the logs of this deployment (19.7). This is the price of the allow-list.
9. **Google "Testing" status** limits sign-ins to 100 listed test users unless only basic scopes are requested (unverified for Cloudflare's flow). Add emails as test users, or publish.
10. **Custom-domain deploy permissions** are undocumented (19.9). The first deploy is by hand; add DNS Edit only if CI needs it.
11. **Team name lock-in.** A deleted Zero Trust organization keeps its team name locked.
12. **workers-rs and R2 streaming** `put` with a stream body is unverified. The TypeScript shell is the fallback (Q8).

### 19.13 Effort and where it fits

The private instance reuses phases 1 to 3 completely: the relay core (retention off is a configuration of the same state machine), the sealed protocol, images as files behind `BlobStore`, and the Worker with its Durable Object. It needs this extra work, **phase 3P**, which can start as soon as phase 3 has the Worker and Durable Object serving a room, and ships **before or alongside phase 4**:

| Item | Days |
|---|---|
| Spike additions on the real private account (needs your login): Access with a WebSocket and the cookie, service-token upgrade, session expiry, `X-Requested-With` 401, custom-domain deploy | 1 |
| `access_jwt` module, JWKS cache, Worker pre-check on `/r/*` and `/api/*`, header stripping, refusal texts, tests | 2 |
| R2 `BlobStore` in the Worker (streaming upload through the object, direct download, quotas, manifest) with tests | 2 |
| Persistent mode: `ROOM_TTL` unset path, `expires_at_ms = 0`, `DeleteRoom` message and `collab.delete_room` | 1.5 |
| Service-token support: `Secrets` port (keyring and file), `collab.set_access` and `clear_access`, headers on WebSocket and blob requests, redirect and host rules, Preferences panel, env/file for headless, redaction tests | 3 |
| Per-environment `wrangler.toml`, `[env.private]`, constants as variables, the two GitHub workflows, token scopes | 1 |
| Runbook `docs/private-instance.md`: the setup steps of 19.3, smoke-test checklist | 1 |
| **Total phase 3P** | **11.5** |

New overall total: phases 0 to 4 as before (**61 days**), plus phase 3P (**11.5**), plus 1 day more in phase 0 (counted in 3P): about **73 days**. If the private instance goes first, phase 4 shrinks by a day or two (no Turnstile, no `Registry` needed for it), and the `Registry` and Turnstile work (about 2 days of phase 3 and 1 of phase 4) can wait until the public demo is deployed.

Suggested order: 0 (spike, with a private-account part) → 1 → 2 → 3 (Worker, Durable Object, DO-backed images) → **3P (private instance live for you)** → 4 (public demo deploy, Turnstile, `Registry`, caps).

---

## 20. Sources

Cloudflare (read 2026-11-18):
- Durable Objects pricing and Free limits: https://developers.cloudflare.com/durable-objects/platform/pricing/
- Durable Objects limits (32 MiB message, 8 MB row): https://developers.cloudflare.com/durable-objects/platform/limits/
- Workers limits (100 MB body on Free, 10 ms CPU, 64 MiB script): https://developers.cloudflare.com/workers/platform/limits/
- WebSockets in Durable Objects, hibernation: https://developers.cloudflare.com/durable-objects/best-practices/websockets/
- Durable Object lifecycle (hibernation conditions): https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/
- Alarms: https://developers.cloudflare.com/durable-objects/api/alarms/
- SQLite storage API, `deleteAll`, output gate: https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/
- Spend management: https://developers.cloudflare.com/durable-objects/platform/spend-management/, https://developers.cloudflare.com/workers/platform/spend-management/, https://developers.cloudflare.com/billing/manage/spend-management/
- Static assets billing: https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/
- Rate Limiting binding: https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/
- Turnstile: https://developers.cloudflare.com/turnstile/plans/, https://developers.cloudflare.com/turnstile/get-started/server-side-validation/
- Custom domains: https://developers.cloudflare.com/workers/configuration/routing/custom-domains/
- workers-rs: https://github.com/cloudflare/workers-rs (0.9.0)

Cloudflare, added 2026-11-22 for the private instance:
- Workers with Access (hostname-based, WebSocket limitation): https://developers.cloudflare.com/workers/configuration/cloudflare-access/
- Self-hosted Access application: https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/
- Authorization cookie and CORS: https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/ , https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/cors/
- Validating the JWT: https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/
- Session management (AJAX 401): https://developers.cloudflare.com/cloudflare-one/access-controls/access-settings/session-management/
- Service tokens: https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/ ; managed OAuth: https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/managed-oauth/ ; `cloudflared` CLI: https://developers.cloudflare.com/cloudflare-one/tutorials/cli/
- Google identity provider: https://developers.cloudflare.com/cloudflare-one/integrations/identity-providers/google/ ; Google production readiness: https://developers.google.com/identity/protocols/oauth2/production-readiness/overview
- Zero Trust setup, FAQ, seats, limits: https://developers.cloudflare.com/cloudflare-one/setup/ , https://developers.cloudflare.com/cloudflare-one/faq/getting-started-faq/ , https://developers.cloudflare.com/cloudflare-one/team-and-resources/users/seat-management/ , https://developers.cloudflare.com/cloudflare-one/account-limits/ , https://blog.cloudflare.com/teams-plans
- R2: https://developers.cloudflare.com/r2/pricing/ , https://developers.cloudflare.com/r2/get-started/ , https://developers.cloudflare.com/r2/api/workers/workers-api-usage/ , https://developers.cloudflare.com/r2/buckets/object-lifecycles/ , https://developers.cloudflare.com/r2/buckets/public-buckets/ , https://developers.cloudflare.com/r2/platform/limits/
- D1 and storage choice: https://developers.cloudflare.com/d1/platform/limits/ , https://developers.cloudflare.com/workers/platform/storage-options/
- Spend management and budget alerts: https://developers.cloudflare.com/billing/manage/spend-management/ , https://developers.cloudflare.com/billing/manage/budget-alerts/
- Wrangler environments, Durable Object environments and migrations: https://developers.cloudflare.com/workers/wrangler/environments/ , https://developers.cloudflare.com/durable-objects/reference/environments/ , https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/
- GitHub Actions and token permissions: https://developers.cloudflare.com/workers/ci-cd/external-cicd/github-actions/ , https://developers.cloudflare.com/fundamentals/api/reference/template/ , https://developers.cloudflare.com/fundamentals/api/reference/permissions/
- WebSocket handshake may carry cookies: https://www.rfc-editor.org/rfc/rfc6455 section 4.1

Crypto:
- NIST SP 800-38D (2^32 random-IV invocations): https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38d.pdf
- XChaCha20-Poly1305 draft: https://www.ietf.org/archive/id/draft-irtf-cfrg-xchacha-03.txt
- Key commitment: https://eprint.iacr.org/2020/1456 ; partitioning oracles: https://eprint.iacr.org/2020/1491
- HKDF: https://www.rfc-editor.org/rfc/rfc5869 ; URI fragment: https://www.rfc-editor.org/rfc/rfc3986 section 3.5 ; https://www.rfc-editor.org/rfc/rfc9110 section 17
- RustCrypto audit (2020): https://web.archive.org/web/20200606025324/https://research.nccgroup.com/2020/02/26/public-report-rustcrypto-aes-gcm-and-chacha20poly1305-implementation-review/
- Crates read from crates.io: `chacha20poly1305` 0.11.0, `aes-gcm` 0.11.1, `hkdf` 0.13.0, `sha2` 0.11.1, `hmac` 0.13.0, `getrandom` 0.4.3, `zeroize` 1.9.2, `subtle` 2.6.1, `base64ct` 1.8.3, `ed25519-dalek` 3.0.0
- Precedents: Loro `%ELO`: https://github.com/loro-dev/protocol/blob/main/protocol-e2ee.md ; Secsync: https://www.secsync.com/docs/specification

Repository: `docs/protocol.md`, `docs/collab.md`, `docs/server.md`, `docs/crdt.md`, `docs/commands.md` (2.2 workspace, 2.16 collab), `docs/web-app.md`; `crates/engine/src/cmd/tab/workspace.rs`, `crates/app/src/startup.rs`, `crates/engine/src/cmd/collab/address.rs`, `crates/protocol/src/limits.rs`, `crates/scene/src/document.rs`; research `/home/ubuntu/git/drafft-ink-plan/research/serverless-collab.md`; Excalidraw: `/home/ubuntu/git/excalidraw` (`packages/excalidraw/data/encryption.ts`, `encode.ts`, `blob.ts`; `excalidraw-app/data/index.ts`, `firebase.ts`, `FileManager.ts`; `excalidraw-app/collab/`).
