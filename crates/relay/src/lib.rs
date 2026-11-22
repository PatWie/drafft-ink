//! drafftink-relay: the blind relay of end-to-end encrypted rooms (layer L1).
//!
//! [`Relay`] is the core of one room, written sans-IO. A host (`drafftink-headless serve` with tokio and axum, or a
//! Cloudflare Durable Object) owns the sockets and the clock. It feeds the core frames and the time in milliseconds
//! and carries out what comes back in an [`Output`]: frames to send, sockets to close, what the core wrote to its
//! store, and the time at which to call [`Relay::on_wake`].
//!
//! The core checks membership against the room id, numbers sealed records, keeps them in segments, holds one sealed
//! snapshot that a client swaps in by compare-and-set, bundles presence within a budget, enforces rate limits and
//! applies the retention policy. It never reads a payload: it sees the record header, sizes and counts, nothing else.
//! The design is `docs/design/collab-e2e.md` sections 8, 10 and 11.
//!
//! - [`RelayStore`] and [`BlobStore`] are the synchronous storage ports. [`MemoryRelayStore`] and
//!   [`MemoryBlobStore`] implement them for tests.
//! - [`RelayConfig`] and [`Retention`] are the policy: the demo deletes a room 48 hours after its last visit, a
//!   self-hosted relay keeps rooms for ever.
//! - [`Lifecycle`] is the retention state machine, usable on its own.
//! - [`codec`] is the byte layout the durable stores share.
//! - [`ms_until_utc_midnight`] is the reset of a host's daily limit, see [`StoreError::DailyLimit`].

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod codec;
mod config;
mod images;
mod lifecycle;
mod memory;
mod quota;
mod rate;
mod room;
mod store;

pub use config::{CompactRules, ConfigError, FlushRules, RelayConfig, Retention};
pub use images::{ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus};
pub use lifecycle::{Lifecycle, Verdict};
pub use memory::{MemoryBlobStore, MemoryRelayStore};
pub use quota::ms_until_utc_midnight;
pub use room::{ConnId, Effect, Frame, Output, Relay, StoreOp};
pub use store::{
    BlobInfo, BlobStats, BlobStore, CasOutcome, LogCounts, RelayStore, RoomMeta, Segment, StoreError, StoredSnapshot,
};
