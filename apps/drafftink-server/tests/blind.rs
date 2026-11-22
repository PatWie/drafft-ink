#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! The relay is blind: two clients draw text that holds a marker, name themselves, move a cursor and share a picture
//! that holds the marker. The picture goes through the real client path (a manifest in the document, `offer_image`,
//! `want_image`, the native picture transport) and, as an independent route, through a hand-made `PUT`. Then every
//! byte the relay stored and everything that crossed the wire (websocket frames, and the address, header and body of
//! every HTTP exchange) is searched for what must not be there: the marker, the user name, the room key in every
//! spelling, the keys derived from it, the file ids and hashes, the pictures, and the media type. Zero hits. The
//! keyed name of each picture must be in storage (the relay sees names) and must not look like the file id. The
//! membership token is allowed in the join frame and the `Authorization` header and nowhere else.
//!
//! The session and the search are in `common/canary.rs`, which the transport parity suite (`parity`) runs against the
//! child process and the Worker too.

mod common;

use std::path::Path;

use common::canary::{Evidence, MARKER, Shared, assert_blind_relay, canary_key, forbidden, run_session};
use common::room_client::{Taps, picture};
use common::*;
use drafftink_seal::RoomKeys;
use drafftink_server::config::Config;
use drafftink_server::store::StorageConfig;

async fn canary(config: Config, stored_under: &Path) {
    let server = drafftink_server::start(config).await.expect("starts");
    let addr = server.local_addr();
    let url = format!("ws://{addr}/ws");
    let session = tokio::task::spawn_blocking(move || run_session(&url, addr)).await.unwrap();
    server.shutdown().await;
    assert_blind_relay(&session, stored_under);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_filesystem_relay_stores_and_sees_nothing_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.storage = StorageConfig::Filesystem { dir: dir.path().to_path_buf() };
    canary(config, dir.path()).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sqlite_relay_stores_and_sees_nothing_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.storage = StorageConfig::Sqlite { path: dir.path().join("relay.sqlite3") };
    canary(config, dir.path()).await;
}

/// The check can fail: a picture that is stored in the clear is found.
#[test]
fn the_search_finds_what_it_looks_for() {
    let key = canary_key();
    let keys = RoomKeys::new(&key);
    let leaked = Shared::new(&keys, picture(MARKER, 3, 500));
    let taps = Taps::default();
    let needles = forbidden(&key, std::slice::from_ref(&leaked));
    for (what, needle) in &needles {
        let clean = Evidence::new(vec![b"nothing to see".to_vec()], &taps);
        clean.assert_blind(std::slice::from_ref(&(what.clone(), needle.clone())));
        let dirty = Evidence::new(vec![[b"xx".as_slice(), needle, b"yy"].concat()], &taps);
        let found = std::panic::catch_unwind(|| dirty.assert_blind(&[(what.clone(), needle.clone())]));
        assert!(found.is_err(), "{what} is found in storage");
    }
}
