//! drafftink-collab (L4): the collaboration client.
//!
//! * [`CollabClient`] is a sans-IO state machine for the protocol in `docs/protocol.md`: join and catch-up from the
//!   document's version vector, batched updates with acknowledgements, remote updates applied through
//!   `LoroStore::apply_remote`, resync, reconnect with backoff, offline editing, errors in plain words. It never
//!   reads a clock (the caller passes `now_ms`), owns no socket and does not know about commands.
//! * [`LocalPresence`] and [`PeerTable`] hold the ephemeral presence: cursor, selection, viewport, user, laser trail
//!   and follow mode. None of it enters the document.
//! * [`Transport`] is the byte pipe. [`NativeTransport`] (threads, `ws://` and `wss://` with rustls), `WebTransport`
//!   (browser `WebSocket`) and [`loopback`] (tests) implement it.
//! * [`CollabSession`] joins a client and a transport and exposes one call, [`CollabSession::pump`], which says what
//!   happened and when to come back. Transports wake the owner through a [`WakeFn`], so the app never polls.
//!
//! Security: addresses are checked by [`ServerUrl`] (only `ws://` and `wss://`, plain `ws://` only for this machine
//! unless the caller opts in), and the access token ([`AuthToken`]) is never logged or shown.
//!
//! Builds for wasm32. No `unwrap`, `expect`, `panic!` or `unsafe` outside tests.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod awareness;
mod client;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod session;
mod status;
mod transport;
mod url;
#[cfg(target_arch = "wasm32")]
mod web;

pub use awareness::{LocalPresence, Peer, PeerTable, TRAIL_MAX_POINTS, TRAIL_TTL_MS, Trail};
pub use client::{
    ACK_TIMEOUT_MS, AuthToken, CONNECT_TIMEOUT_MS, CollabClient, CollabConfig, CollabEvent, ConfigError, FollowEnd,
    JOIN_TIMEOUT_MS, MAX_INFLIGHT, Output, RESYNC_MAX_GAP_MS, RESYNC_MIN_GAP_MS, RemoteBatch, RemoteKind,
    SEND_DEBOUNCE_MS, STABLE_MS, SYNC_REPLY_TIMEOUT_MS, Stats, TrimmedChoice,
};
#[cfg(not(target_arch = "wasm32"))]
pub use native::{NativeOptions, NativeTransport, TlsOptions};
pub use session::{CollabSession, Pump};
pub use status::{ConnState, Problem, ProblemAction, Status, StopReason, explain};
pub use transport::{LoopbackServer, LoopbackTransport, Transport, TransportError, TransportEvent, WakeFn, loopback};
pub use url::{ServerUrl, UrlError, UrlPolicy};
#[cfg(target_arch = "wasm32")]
pub use web::WebTransport;
