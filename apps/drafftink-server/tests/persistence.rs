//! Persistence through the server: restart and crash with every provider, compaction, flush
//! batching, idle unload, expiry and storage failures.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code: a failed expectation is the failure

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use drafftink_protocol::{ErrorCode, RoomId, ServerMessage, SyncKind};
use drafftink_server::config::Config;
use drafftink_server::store::{MemoryStore, RoomMeta, RoomStore, StorageConfig, StoreError, StoredRoom};
use drafftink_server::{ServerHandle, start_with_store};
use loro::{ExportMode, LoroDoc};

type Store = Arc<dyn RoomStore>;
type Opener = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Store> + Send>> + Send + Sync>;

struct Setup {
    name: &'static str,
    open: Opener,
    _guard: Box<dyn std::any::Any + Send>,
}

fn opener(storage: StorageConfig) -> Opener {
    Arc::new(move || {
        let storage = storage.clone();
        Box::pin(async move { drafftink_server::store::open(&storage).await.expect("open store") })
    })
}

async fn setups() -> Vec<Setup> {
    let mut out = Vec::new();
    let mem: Store = Arc::new(MemoryStore::new());
    out.push(Setup {
        name: "memory",
        open: Arc::new(move || {
            let m = mem.clone();
            Box::pin(async move { m })
        }),
        _guard: Box::new(()),
    });
    let dir = tempfile::tempdir().expect("tempdir");
    out.push(Setup {
        name: "filesystem",
        open: opener(StorageConfig::Filesystem { dir: dir.path().join("rooms") }),
        _guard: Box::new(dir),
    });
    #[cfg(feature = "sqlite")]
    {
        let dir = tempfile::tempdir().expect("tempdir");
        out.push(Setup {
            name: "sqlite",
            open: opener(StorageConfig::Sqlite { path: dir.path().join("rooms.sqlite3") }),
            _guard: Box::new(dir),
        });
    }
    #[cfg(feature = "s3")]
    {
        use drafftink_server::store::S3Config;
        let mock = mock_s3::MockS3::start().await;
        let config = S3Config {
            bucket: "bucket".into(),
            endpoint: Some(mock.endpoint()),
            access_key_id: Some("k".into()),
            secret_access_key: Some("s".into()),
            allow_http: true,
            ..S3Config::default()
        };
        out.push(Setup { name: "s3 (mock)", open: opener(StorageConfig::S3(config)), _guard: Box::new(mock) });
        if let Ok(endpoint) = std::env::var("DRAFFTINK_TEST_S3_ENDPOINT") {
            let config = S3Config {
                bucket: std::env::var("DRAFFTINK_TEST_S3_BUCKET").unwrap_or_else(|_| "drafftink-test".into()),
                endpoint: Some(endpoint),
                access_key_id: std::env::var("DRAFFTINK_TEST_S3_ACCESS_KEY").ok(),
                secret_access_key: std::env::var("DRAFFTINK_TEST_S3_SECRET_KEY").ok(),
                prefix: Some(format!("server-tests/{}-{}", std::process::id(), drafftink_server::clock::now_ms())),
                allow_http: true,
                ..S3Config::default()
            };
            out.push(Setup {
                name: "s3 (real endpoint)",
                open: opener(StorageConfig::S3(config)),
                _guard: Box::new(()),
            });
        }
    }
    out
}

async fn run(config: Config, store: Store) -> ServerHandle {
    start_with_store(config, store).await.expect("start")
}

fn doc_from(room: &str, keys: usize) -> LoroDoc {
    let _ = room;
    let doc = LoroDoc::new();
    for i in 0..keys {
        put(&doc, &format!("k{i}"), text(&format!("value {i}")));
    }
    doc
}

async fn poll<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    for _ in 0..150 {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rooms_survive_a_graceful_restart_with_every_provider() {
    for setup in setups().await {
        let store = (setup.open)().await;
        let server = run(test_config(), store).await;
        let mut a = Client::connect(server.local_addr(), 1, "restart").await;
        a.join().await;
        for i in 0..20 {
            a.edit_acked(&format!("k{i}"), text(&format!("v{i}"))).await;
        }
        let want = state(&a.doc);
        server.shutdown().await;

        let store = (setup.open)().await;
        let server = run(test_config(), store).await;
        let mut b = Client::connect(server.local_addr(), 2, "restart").await;
        let (_, sync) = b.join().await;
        assert_eq!(sync_kind(&sync), SyncKind::Snapshot, "{}", setup.name);
        assert_eq!(state(&b.doc), want, "{}: the room came back", setup.name);
        // And it keeps working: an old client resumes against the reloaded room.
        a.close_now();
        let mut a2 = Client::connect(server.local_addr(), 1, "restart").await;
        a2.join().await;
        a2.edit_acked("after", text("restart")).await;
        b.converge_to(&state(&a2.doc)).await;
        server.shutdown().await;
    }
}

#[tokio::test]
async fn acknowledged_edits_survive_a_crash_when_the_flush_interval_is_zero() {
    for setup in setups().await {
        // Run the server on its own runtime and kill that runtime without any shutdown.
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let store = (setup.open)().await;
        let server = rt.spawn(run(test_config(), store)).await.expect("server starts");
        let addr = server.local_addr();
        let mut a = Client::connect(addr, 1, "crash").await;
        a.join().await;
        for i in 0..15 {
            a.edit_acked(&format!("k{i}"), text("v")).await;
        }
        let want = state(&a.doc);
        std::mem::forget(server);
        rt.shutdown_background();
        a.close_now();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The log (no compaction ran) holds the edits; a new server replays it.
        let reopened = (setup.open)().await;
        let stored = reopened.load(&room("crash")).await.expect("load").expect("room stored");
        assert!(!stored.updates.is_empty(), "{}: the crash left a log, not a snapshot", setup.name);
        let server = run(test_config(), reopened).await;
        let mut b = Client::connect(server.local_addr(), 2, "crash").await;
        b.join().await;
        assert_eq!(state(&b.doc), want, "{}: replayed from the log", setup.name);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn the_log_is_compacted_when_it_grows() {
    for setup in setups().await {
        let store = (setup.open)().await;
        let mut config = test_config();
        config.timing.compact_entries = 4;
        let server = run(config, store.clone()).await;
        let mut a = Client::connect(server.local_addr(), 1, "compaction").await;
        a.join().await;
        for i in 0..11 {
            a.edit_acked(&format!("k{i}"), text("v")).await;
        }
        let stored = store.load(&room("compaction")).await.expect("load").expect("stored");
        assert!(stored.snapshot.is_some(), "{}: compacted", setup.name);
        assert!(stored.updates.len() < 4, "{}: log stays short: {}", setup.name, stored.updates.len());
        let want = state(&a.doc);
        server.shutdown().await;
        // The compacted room loads.
        let server = run(test_config(), (setup.open)().await).await;
        let mut b = Client::connect(server.local_addr(), 2, "compaction").await;
        b.join().await;
        assert_eq!(state(&b.doc), want, "{}", setup.name);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn updates_are_batched_into_one_log_entry_per_flush_interval() {
    let store: Store = Arc::new(MemoryStore::new());
    let mut config = test_config();
    config.timing.flush_interval = Duration::from_millis(400);
    let server = run(config, store.clone()).await;
    let mut a = Client::connect(server.local_addr(), 1, "batch").await;
    a.join().await;
    for i in 0..10 {
        a.edit_acked(&format!("k{i}"), text("v")).await;
    }
    assert!(store.load(&room("batch")).await.expect("load").is_none(), "Ack means merged, not yet stored");
    let stored = poll("the flush", async || store.load(&room("batch")).await.expect("load")).await;
    assert_eq!(stored.updates.len(), 1, "ten edits, one write");
    let doc = LoroDoc::new();
    doc.import(&stored.updates[0]).expect("import");
    assert_eq!(state(&doc), state(&a.doc));
    server.shutdown().await;
}

#[tokio::test]
async fn an_empty_room_is_saved_and_unloaded_after_the_idle_time_and_comes_back() {
    for setup in setups().await {
        let store = (setup.open)().await;
        let mut config = test_config();
        config.timing.room_idle = Duration::from_millis(200);
        let server = run(config, store.clone()).await;
        let mut a = Client::connect(server.local_addr(), 1, "idle").await;
        a.join().await;
        a.edit_acked("k", text("v")).await;
        let want = state(&a.doc);
        assert_eq!(server.loaded_rooms().await, 1);
        a.send(&drafftink_protocol::ClientMessage::Leave).await;
        a.expect_closed().await;
        poll("unload", async || (server.loaded_rooms().await == 0).then_some(())).await;
        let stored = store.load(&room("idle")).await.expect("load").expect("stored");
        assert!(stored.snapshot.is_some() && stored.updates.is_empty(), "{}: compacted on unload", setup.name);
        let mut b = Client::connect(server.local_addr(), 2, "idle").await;
        b.join().await;
        assert_eq!(state(&b.doc), want, "{}", setup.name);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn a_room_with_peers_is_not_unloaded() {
    let mut config = test_config();
    config.timing.room_idle = Duration::from_millis(100);
    let server = run(config, Arc::new(MemoryStore::new())).await;
    let mut a = Client::connect(server.local_addr(), 1, "busy").await;
    a.join().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.loaded_rooms().await, 1);
    a.edit_acked("k", text("v")).await;
    server.shutdown().await;
}

#[tokio::test]
async fn expiry_deletes_stale_stored_rooms_but_never_a_loaded_one() {
    use drafftink_server::outbox::outbox;
    use drafftink_server::room::{PeerSetup, RoomManager};
    let store: Store = Arc::new(MemoryStore::new());
    for r in ["stale", "loaded"] {
        store.append_update(&room(r), b"x").await.expect("append");
    }
    let config = Arc::new(Config::default());
    let manager = RoomManager::new(config, store.clone());
    // Load "loaded" by joining it (its stored bytes are not a Loro update, so use a real room).
    let doc = doc_from("loaded", 2);
    store.delete(&room("loaded")).await.expect("delete");
    store.compact(&room("loaded"), &doc.export(ExportMode::Snapshot).expect("export"), 0).await.expect("compact");
    let (out, _rx) = outbox(16, 1 << 20);
    let setup =
        PeerSetup { conn: 1, client_id: client_id(1), user: None, version: vec![], protocol_version: 1, outbox: out };
    manager.join(&room("loaded"), setup).await.expect("join");

    let hour = Duration::from_secs(3600);
    let now = drafftink_server::clock::now_ms();
    assert_eq!(manager.expire_rooms(hour, now).await, 0, "nothing is old yet");
    assert_eq!(manager.expire_rooms(hour, now + 2 * 3_600_000).await, 1, "only the unloaded room goes");
    assert!(store.load(&room("stale")).await.expect("load").is_none());
    assert!(store.load(&room("loaded")).await.expect("load").is_some());
}

#[tokio::test]
async fn stored_data_that_is_a_snapshot_plus_already_covered_updates_loads() {
    // The state a crash leaves between "snapshot stored" and "covered entries deleted".
    let doc = LoroDoc::new();
    let mut updates = Vec::new();
    let mut last = loro::VersionVector::default();
    for i in 0..5 {
        put(&doc, &format!("k{i}"), text("v"));
        updates.push(doc.export(ExportMode::updates(&last)).expect("export"));
        last = doc.oplog_vv();
    }
    let store: Store = Arc::new(MemoryStore::new());
    store.compact(&room("halfway"), &doc.export(ExportMode::Snapshot).expect("export"), 0).await.expect("compact");
    for u in &updates {
        store.append_update(&room("halfway"), u).await.expect("append");
    }
    let server = run(test_config(), store.clone()).await;
    let mut c = Client::connect(server.local_addr(), 1, "halfway").await;
    c.join().await;
    assert_eq!(state(&c.doc), state(&doc));
    c.edit_acked("more", text("v")).await;
    server.shutdown().await;
    // After the shutdown compaction the log is gone and the data complete.
    let stored = store.load(&room("halfway")).await.expect("load").expect("stored");
    assert!(stored.updates.is_empty());
    let check = LoroDoc::new();
    check.import(&stored.snapshot.expect("snapshot")).expect("import");
    assert_eq!(state(&check), state(&c.doc));
}

// ---- failing stores -----------------------------------------------------------------------------

#[derive(Default)]
struct Faults {
    fail_appends: AtomicBool,
    hang_loads: AtomicBool,
    append_calls: AtomicUsize,
}

struct FaultyStore {
    inner: MemoryStore,
    faults: Arc<Faults>,
}

#[async_trait]
impl RoomStore for FaultyStore {
    fn name(&self) -> &'static str {
        "faulty"
    }
    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError> {
        if self.faults.hang_loads.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
        self.inner.load(room).await
    }
    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError> {
        self.faults.append_calls.fetch_add(1, Ordering::SeqCst);
        if self.faults.fail_appends.load(Ordering::SeqCst) {
            return Err(StoreError::Io("disk on fire".into()));
        }
        self.inner.append_update(room, update).await
    }
    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError> {
        self.inner.compact(room, snapshot, covered).await
    }
    async fn delete(&self, room: &RoomId) -> Result<(), StoreError> {
        self.inner.delete(room).await
    }
    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError> {
        self.inner.list().await
    }
}

fn faulty() -> (Store, Arc<Faults>) {
    let faults = Arc::new(Faults::default());
    (Arc::new(FaultyStore { inner: MemoryStore::new(), faults: faults.clone() }), faults)
}

#[tokio::test]
async fn failed_store_writes_are_retried_and_clients_keep_working() {
    let (store, faults) = faulty();
    faults.fail_appends.store(true, Ordering::SeqCst);
    let mut config = test_config();
    config.timing.flush_retry = Duration::from_millis(100);
    let server = run(config, store.clone()).await;
    let mut a = Client::connect(server.local_addr(), 1, "flaky").await;
    let mut b = Client::connect(server.local_addr(), 2, "flaky").await;
    a.join().await;
    b.join().await;
    a.edit_acked("k1", text("v")).await;
    b.converge_to(&state(&a.doc)).await;
    poll("a retry", async || (faults.append_calls.load(Ordering::SeqCst) >= 3).then_some(())).await;
    assert!(store.load(&room("flaky")).await.expect("load").is_none(), "nothing was stored while writes fail");
    faults.fail_appends.store(false, Ordering::SeqCst);
    let stored = poll("the data to land", async || store.load(&room("flaky")).await.expect("load")).await;
    let doc = LoroDoc::new();
    for u in &stored.updates {
        doc.import(u).expect("import");
    }
    assert_eq!(state(&doc), state(&a.doc), "the retry wrote everything, nothing was lost");
    server.shutdown().await;
}

#[tokio::test]
async fn damaged_stored_data_refuses_the_join_and_is_never_overwritten() {
    let store: Store = Arc::new(MemoryStore::new());
    store.append_update(&room("damaged"), b"this is not a loro update").await.expect("append");
    let server = run(test_config(), store.clone()).await;
    for n in 1..=2u8 {
        let mut c = Client::connect(server.local_addr(), n, "damaged").await;
        let j = c.join_message(None);
        c.send(&j).await;
        let m = c.next_within(WAIT).await;
        assert!(
            matches!(
                &m,
                Event::Msg(ServerMessage::Error { code: ErrorCode::Unavailable, retry_after_ms: Some(_), .. })
            ),
            "{m:?}"
        );
        c.expect_closed().await;
    }
    let stored = store.load(&room("damaged")).await.expect("load").expect("still there");
    assert_eq!((stored.snapshot, stored.updates), (None, vec![b"this is not a loro update".to_vec()]));
    // Other rooms are unaffected.
    let mut ok = Client::connect(server.local_addr(), 3, "fine").await;
    ok.join().await;
    ok.edit_acked("k", text("v")).await;
    server.shutdown().await;
}

#[tokio::test]
async fn a_hanging_store_fails_the_join_after_the_store_timeout() {
    let (store, faults) = faulty();
    faults.hang_loads.store(true, Ordering::SeqCst);
    let mut config = test_config();
    config.timing.store_timeout = Duration::from_millis(200);
    let server = run(config, store).await;
    let mut c = Client::connect(server.local_addr(), 1, "hang").await;
    let j = c.join_message(None);
    c.send(&j).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::Unavailable]);
    // The failed room is not stuck: once the store works the room loads.
    faults.hang_loads.store(false, Ordering::SeqCst);
    let mut c = Client::connect(server.local_addr(), 1, "hang").await;
    c.join().await;
    server.shutdown().await;
}

#[tokio::test]
async fn a_room_that_reaches_its_byte_limit_refuses_more_and_stays_up() {
    let mut config = test_config();
    config.limits.max_room_bytes = 40 * 1024;
    config.limits.max_updates_per_sec = 1000;
    let server = run(config, Arc::new(MemoryStore::new())).await;
    let mut a = Client::connect(server.local_addr(), 1, "limited").await;
    a.join().await;
    let mut rng = Rng(3);
    let mut refused = false;
    for i in 0..20 {
        let seq = a.edit(&format!("b{i}"), loro::LoroValue::from(rng.bytes(8 * 1024))).await;
        match a
            .recv_until(|m| matches!(m, ServerMessage::Ack { .. } | ServerMessage::Error { .. }).then(|| m.clone()))
            .await
        {
            ServerMessage::Ack { seq: s } => assert_eq!(s, seq),
            ServerMessage::Error { code: ErrorCode::TooLarge, .. } => {
                refused = true;
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(refused, "the room limit must stop growth");
    server.shutdown().await;
}
