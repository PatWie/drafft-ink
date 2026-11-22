//! The Rust core of the Cloudflare Worker host (design `docs/design/collab-e2e.md`, sections 14 and 19). The
//! TypeScript shell in this directory owns the sockets, the clock and the storage calls; the rules live in
//! `drafftink-relay`.
pub mod access_jwt;
