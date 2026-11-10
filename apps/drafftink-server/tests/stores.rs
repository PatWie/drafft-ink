//! The conformance suite: every `RoomStore` provider must pass the same cases.
//!
//! Providers: memory, filesystem, sqlite (feature `sqlite`), s3 (feature `s3`). The S3 cases
//! run against an in-process S3-compatible mock (`common::mock_s3`) and, when
//! `DRAFFTINK_TEST_S3_ENDPOINT` is set, again against that real endpoint (MinIO, RustFS,
//! Garage, ...). Variables for the real run: `DRAFFTINK_TEST_S3_ENDPOINT` (http://host:port),
//! `DRAFFTINK_TEST_S3_BUCKET` (must exist), `DRAFFTINK_TEST_S3_ACCESS_KEY`,
//! `DRAFFTINK_TEST_S3_SECRET_KEY`. Each case uses its own key prefix.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code: a failed expectation is the failure

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use drafftink_protocol::RoomId;
use drafftink_server::clock::now_ms;
use drafftink_server::store::{RoomStore, StoreError, StoredRoom};

type Opened = Arc<dyn RoomStore>;
type Opener = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Opened> + Send>> + Send + Sync>;

/// What the provider keeps across a reopen.
#[derive(Clone, Copy, PartialEq)]
enum Durability {
    Volatile,
    Durable,
}

struct Provider {
    open: Opener,
    durability: Durability,
    /// Keeps temp dirs and mock servers alive for the length of a case.
    _guard: Box<dyn std::any::Any + Send>,
}

fn id(s: &str) -> RoomId {
    RoomId::new(s).expect("valid room id")
}

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

// ---- providers -------------------------------------------------------------------------------

fn memory_provider() -> Provider {
    let store: Opened = Arc::new(drafftink_server::store::MemoryStore::new());
    Provider {
        open: Arc::new(move || {
            let s = store.clone();
            Box::pin(async move { s })
        }),
        durability: Durability::Volatile,
        _guard: Box::new(()),
    }
}

fn fs_provider() -> Provider {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rooms");
    Provider {
        open: Arc::new(move || {
            let path = path.clone();
            Box::pin(
                async move { Arc::new(drafftink_server::store::FsStore::open(path).await.expect("open fs")) as Opened },
            )
        }),
        durability: Durability::Durable,
        _guard: Box::new(dir),
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_provider() -> Provider {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("db").join("rooms.sqlite3");
    Provider {
        open: Arc::new(move || {
            let path = path.clone();
            Box::pin(async move {
                Arc::new(drafftink_server::store::SqliteStore::open(path).await.expect("open sqlite")) as Opened
            })
        }),
        durability: Durability::Durable,
        _guard: Box::new(dir),
    }
}

#[cfg(feature = "s3")]
mod s3 {
    use super::*;
    use drafftink_server::store::{S3Config, S3Store};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub async fn mock_provider() -> Provider {
        let mock = common::mock_s3::MockS3::start().await;
        let config = S3Config {
            bucket: "bucket".into(),
            endpoint: Some(mock.endpoint()),
            region: Some("us-east-1".into()),
            access_key_id: Some("test-key".into()),
            secret_access_key: Some("test-secret".into()),
            prefix: Some("drafftink".into()),
            allow_http: true,
            ..S3Config::default()
        };
        provider(config, Box::new(mock))
    }

    /// A provider for a real endpoint named in the environment, if there is one.
    pub fn real_provider() -> Option<Provider> {
        let endpoint = std::env::var("DRAFFTINK_TEST_S3_ENDPOINT").ok()?;
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let config = S3Config {
            bucket: std::env::var("DRAFFTINK_TEST_S3_BUCKET").unwrap_or_else(|_| "drafftink-test".into()),
            endpoint: Some(endpoint),
            region: Some("us-east-1".into()),
            access_key_id: std::env::var("DRAFFTINK_TEST_S3_ACCESS_KEY").ok(),
            secret_access_key: std::env::var("DRAFFTINK_TEST_S3_SECRET_KEY").ok(),
            prefix: Some(format!("conformance/{}-{n}-{}", std::process::id(), now_ms())),
            allow_http: true,
            ..S3Config::default()
        };
        Some(provider(config, Box::new(())))
    }

    fn provider(config: S3Config, guard: Box<dyn std::any::Any + Send>) -> Provider {
        Provider {
            open: Arc::new(move || {
                let config = config.clone();
                Box::pin(async move { Arc::new(S3Store::new(&config).expect("s3 store")) as Opened })
            }),
            durability: Durability::Durable,
            _guard: guard,
        }
    }
}

// ---- cases -----------------------------------------------------------------------------------

async fn unknown_room_loads_as_none(p: &Provider) {
    let s = (p.open)().await;
    assert_eq!(s.load(&id("nobody")).await.expect("load"), None);
}

async fn appended_updates_load_in_order(p: &Provider) {
    let s = (p.open)().await;
    let r = id("order");
    let updates: Vec<Vec<u8>> = (0..25u8).map(|i| bytes(10 + usize::from(i), i)).collect();
    for u in &updates {
        s.append_update(&r, u).await.expect("append");
    }
    let got = s.load(&r).await.expect("load").expect("room exists");
    assert_eq!(got, StoredRoom { snapshot: None, updates });
}

async fn compact_replaces_the_snapshot_and_drops_covered_entries(p: &Provider) {
    let s = (p.open)().await;
    let r = id("compact");
    for i in 0..5u8 {
        s.append_update(&r, &bytes(8, i)).await.expect("append");
    }
    s.compact(&r, b"snapshot-1", 3).await.expect("compact");
    let got = s.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.snapshot.as_deref(), Some(&b"snapshot-1"[..]));
    assert_eq!(got.updates, vec![bytes(8, 3), bytes(8, 4)], "entries beyond `covered` stay, in order");
    s.append_update(&r, &bytes(8, 9)).await.expect("append after compaction");
    s.compact(&r, b"snapshot-2", 3).await.expect("second compaction");
    let got = s.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.snapshot.as_deref(), Some(&b"snapshot-2"[..]));
    assert!(got.updates.is_empty());
    s.append_update(&r, &bytes(8, 10)).await.expect("append");
    let got = s.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.updates, vec![bytes(8, 10)]);
}

async fn compact_of_a_new_room_and_over_covering_are_fine(p: &Provider) {
    let s = (p.open)().await;
    let fresh = id("fresh");
    s.compact(&fresh, b"only-a-snapshot", 0).await.expect("compact new room");
    let got = s.load(&fresh).await.expect("load").expect("exists");
    assert_eq!((got.snapshot.as_deref(), got.updates.len()), (Some(&b"only-a-snapshot"[..]), 0));
    let r = id("over");
    s.append_update(&r, b"a").await.expect("append");
    s.compact(&r, b"s", 100).await.expect("covered beyond the log");
    assert!(s.load(&r).await.expect("load").expect("exists").updates.is_empty());
}

async fn rooms_are_isolated_and_case_sensitive(p: &Provider) {
    let s = (p.open)().await;
    let names = ["Ab", "ab", "AB", "a.b", "a_b", "a-b", "7"];
    for (i, n) in names.iter().enumerate() {
        s.append_update(&id(n), &[i as u8]).await.expect("append");
    }
    for (i, n) in names.iter().enumerate() {
        let got = s.load(&id(n)).await.expect("load").expect("exists");
        assert_eq!(got.updates, vec![vec![i as u8]], "room {n}");
    }
    let long = id(&"x".repeat(128));
    s.append_update(&long, b"long").await.expect("append to the longest id");
    assert_eq!(s.load(&long).await.expect("load").expect("exists").updates, vec![b"long".to_vec()]);
}

async fn delete_removes_everything_and_is_idempotent(p: &Provider) {
    let s = (p.open)().await;
    let (a, b) = (id("del-a"), id("del-b"));
    for r in [&a, &b] {
        s.append_update(r, b"u").await.expect("append");
        s.compact(r, b"s", 0).await.expect("compact");
        s.append_update(r, b"v").await.expect("append");
    }
    s.delete(&a).await.expect("delete");
    s.delete(&a).await.expect("delete again");
    s.delete(&id("never-existed")).await.expect("delete unknown");
    assert_eq!(s.load(&a).await.expect("load"), None);
    assert!(s.load(&b).await.expect("load").is_some(), "other rooms are untouched");
    let listed: Vec<String> = s.list().await.expect("list").into_iter().map(|m| m.room.to_string()).collect();
    assert!(!listed.contains(&"del-a".to_owned()) && listed.contains(&"del-b".to_owned()), "{listed:?}");
    // A deleted room can be created again and starts from the beginning.
    s.append_update(&a, b"again").await.expect("append");
    assert_eq!(s.load(&a).await.expect("load").expect("exists").updates, vec![b"again".to_vec()]);
}

async fn list_reports_rooms_with_recent_times(p: &Provider) {
    let s = (p.open)().await;
    let before = now_ms();
    let names = ["l1", "L2", "l3.x", "l4"];
    for (i, n) in names.iter().enumerate() {
        if i % 2 == 0 {
            s.append_update(&id(n), b"u").await.expect("append");
        } else {
            s.compact(&id(n), b"s", 0).await.expect("compact");
        }
    }
    let after = now_ms();
    let mut listed = s.list().await.expect("list");
    listed.sort_by(|a, b| a.room.cmp(&b.room));
    let ids: Vec<String> = listed.iter().map(|m| m.room.to_string()).collect();
    assert_eq!(ids, ["L2", "l1", "l3.x", "l4"]);
    let slack = 10 * 60 * 1000; // file systems and servers have their own clocks
    for m in &listed {
        assert!(
            m.modified_ms + slack >= before && m.modified_ms <= after + slack,
            "{m:?} not within [{before}, {after}]"
        );
    }
}

async fn payloads_round_trip_exactly(p: &Provider) {
    let s = (p.open)().await;
    let r = id("payloads");
    let every_byte: Vec<u8> = (0..=255u8).collect();
    let big: Vec<u8> = (0..5 * 1024 * 1024).map(|i: usize| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    s.append_update(&r, &[]).await.expect("empty payload");
    s.append_update(&r, &every_byte).await.expect("all byte values");
    s.append_update(&r, &big).await.expect("5 MiB");
    s.compact(&id("payloads-snap"), &big, 0).await.expect("big snapshot");
    let got = s.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.updates.len(), 3);
    assert_eq!(got.updates[0], Vec::<u8>::new());
    assert_eq!(got.updates[1], every_byte);
    assert!(got.updates[2] == big, "5 MiB payload differs");
    let snap = s.load(&id("payloads-snap")).await.expect("load").expect("exists").snapshot.expect("snapshot");
    assert!(snap == big, "5 MiB snapshot differs");
}

async fn rooms_that_start_with_a_dot_are_refused(p: &Provider) {
    let s = (p.open)().await;
    for bad in [".", "..", ".hidden"] {
        let r = id(bad);
        assert!(matches!(s.append_update(&r, b"x").await, Err(StoreError::InvalidRoom(_))), "append {bad}");
        assert!(matches!(s.load(&r).await, Err(StoreError::InvalidRoom(_))), "load {bad}");
        assert!(matches!(s.compact(&r, b"x", 0).await, Err(StoreError::InvalidRoom(_))), "compact {bad}");
        assert!(matches!(s.delete(&r).await, Err(StoreError::InvalidRoom(_))), "delete {bad}");
    }
    assert!(s.list().await.expect("list").is_empty(), "nothing leaked into the store");
}

async fn concurrent_appends_to_different_rooms_all_land(p: &Provider) {
    let s = (p.open)().await;
    let mut tasks = Vec::new();
    for room in 0..8u8 {
        let s = s.clone();
        tasks.push(tokio::spawn(async move {
            let r = id(&format!("par-{room}"));
            for i in 0..10u8 {
                s.append_update(&r, &[room, i]).await.expect("append");
            }
        }));
    }
    for t in tasks {
        t.await.expect("task");
    }
    for room in 0..8u8 {
        let got = s.load(&id(&format!("par-{room}"))).await.expect("load").expect("exists");
        let want: Vec<Vec<u8>> = (0..10u8).map(|i| vec![room, i]).collect();
        assert_eq!(got.updates, want);
    }
}

async fn data_survives_reopening(p: &Provider) {
    let r = id("durable");
    {
        let s = (p.open)().await;
        for i in 0..4u8 {
            s.append_update(&r, &bytes(5, i)).await.expect("append");
        }
        s.compact(&r, b"snap", 2).await.expect("compact");
        s.append_update(&r, &bytes(5, 4)).await.expect("append");
    }
    let s = (p.open)().await;
    let got = s.load(&r).await.expect("load");
    if p.durability == Durability::Durable {
        let got = got.expect("room survives");
        assert_eq!(got.snapshot.as_deref(), Some(&b"snap"[..]));
        assert_eq!(got.updates, vec![bytes(5, 2), bytes(5, 3), bytes(5, 4)]);
        // The reopened store continues the log without reusing indexes.
        s.append_update(&r, &bytes(5, 5)).await.expect("append after reopen");
        let again = s.load(&r).await.expect("load").expect("exists");
        assert_eq!(again.updates.last(), Some(&bytes(5, 5)));
        assert_eq!(again.updates.len(), 4);
    }
}

async fn appending_after_a_reopen_without_load_continues_the_log(p: &Provider) {
    if p.durability == Durability::Volatile {
        return;
    }
    let r = id("noload");
    {
        let s = (p.open)().await;
        s.append_update(&r, b"one").await.expect("append");
        s.append_update(&r, b"two").await.expect("append");
    }
    let s = (p.open)().await;
    s.append_update(&r, b"three").await.expect("append on a fresh store object");
    let got = s.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.updates, vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
}

macro_rules! suite {
    ($name:ident, $provider:expr) => {
        mod $name {
            use super::*;
            macro_rules! case {
                ($case:ident) => {
                    #[tokio::test]
                    async fn $case() {
                        let p = $provider.await;
                        super::$case(&p).await;
                    }
                };
            }
            case!(unknown_room_loads_as_none);
            case!(appended_updates_load_in_order);
            case!(compact_replaces_the_snapshot_and_drops_covered_entries);
            case!(compact_of_a_new_room_and_over_covering_are_fine);
            case!(rooms_are_isolated_and_case_sensitive);
            case!(delete_removes_everything_and_is_idempotent);
            case!(list_reports_rooms_with_recent_times);
            case!(payloads_round_trip_exactly);
            case!(rooms_that_start_with_a_dot_are_refused);
            case!(concurrent_appends_to_different_rooms_all_land);
            case!(data_survives_reopening);
            case!(appending_after_a_reopen_without_load_continues_the_log);
        }
    };
}

suite!(memory, async { memory_provider() });
suite!(filesystem, async { fs_provider() });
#[cfg(feature = "sqlite")]
suite!(sqlite, async { sqlite_provider() });
#[cfg(feature = "s3")]
suite!(s3_mock, s3::mock_provider());

/// Runs the whole suite against a real S3-compatible endpoint when one is configured.
#[cfg(feature = "s3")]
#[tokio::test]
async fn s3_real_endpoint_when_configured() {
    if s3::real_provider().is_none() {
        eprintln!("DRAFFTINK_TEST_S3_ENDPOINT is not set: skipping the real S3 run");
        return;
    }
    macro_rules! run {
        ($($case:ident),*) => {$(
            let p = s3::real_provider().expect("configured");
            $case(&p).await;
        )*};
    }
    run!(
        unknown_room_loads_as_none,
        appended_updates_load_in_order,
        compact_replaces_the_snapshot_and_drops_covered_entries,
        compact_of_a_new_room_and_over_covering_are_fine,
        rooms_are_isolated_and_case_sensitive,
        delete_removes_everything_and_is_idempotent,
        list_reports_rooms_with_recent_times,
        payloads_round_trip_exactly,
        rooms_that_start_with_a_dot_are_refused,
        concurrent_appends_to_different_rooms_all_land,
        data_survives_reopening,
        appending_after_a_reopen_without_load_continues_the_log
    );
}

// ---- filesystem specifics --------------------------------------------------------------------

#[tokio::test]
async fn fs_ignores_and_cleans_up_interrupted_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = drafftink_server::store::FsStore::open(dir.path().to_path_buf()).await.expect("open");
    let r = id("crashy");
    store.append_update(&r, b"good").await.expect("append");
    store.compact(&r, b"snap", 0).await.expect("compact");
    let room_dir = dir.path().join("crashy");
    std::fs::write(room_dir.join("snapshot.bin.tmp"), b"half a snapshot").expect("write");
    std::fs::write(room_dir.join("updates").join("00000000000000000007.bin.tmp"), b"half an update").expect("write");
    let got = store.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.snapshot.as_deref(), Some(&b"snap"[..]));
    assert_eq!(got.updates, vec![b"good".to_vec()]);
    assert!(!room_dir.join("updates").join("00000000000000000007.bin.tmp").exists(), "leftover removed");
}

#[tokio::test]
async fn fs_layout_is_snapshot_and_numbered_update_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = drafftink_server::store::FsStore::open(dir.path().to_path_buf()).await.expect("open");
    let r = id("Layout");
    store.append_update(&r, b"a").await.expect("append");
    store.append_update(&r, b"b").await.expect("append");
    store.compact(&r, b"s", 1).await.expect("compact");
    let room_dir = dir.path().join("~layout");
    assert_eq!(std::fs::read(room_dir.join("snapshot.bin")).expect("snapshot"), b"s");
    let mut files: Vec<String> = std::fs::read_dir(room_dir.join("updates"))
        .expect("updates dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["00000000000000000001.bin"]);
}

#[tokio::test]
async fn fs_with_snapshot_and_already_covered_entries_loads_both() {
    // A crash after the snapshot rename but before the covered entries were deleted leaves
    // both. Loading returns both; replaying the entries on the snapshot is harmless for Loro.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = drafftink_server::store::FsStore::open(dir.path().to_path_buf()).await.expect("open");
    let r = id("halfway");
    store.append_update(&r, b"u0").await.expect("append");
    store.append_update(&r, b"u1").await.expect("append");
    std::fs::write(dir.path().join("halfway").join("snapshot.bin"), b"snap").expect("write snapshot");
    let got = store.load(&r).await.expect("load").expect("exists");
    assert_eq!(got.snapshot.as_deref(), Some(&b"snap"[..]));
    assert_eq!(got.updates.len(), 2);
}

#[allow(dead_code)]
fn unused(_: &AtomicU64) {
    let _ = Ordering::SeqCst;
}
