#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! One conformance suite for every `RelayStore` and `BlobStore`: memory, files and SQLite.

use drafftink_protocol::ClientId;
use drafftink_protocol::v2::{ImageName, RoomId, StoredRecord};
use drafftink_relay::{
    BlobStore, CasOutcome, MemoryBlobStore, MemoryRelayStore, RelayStore, RoomMeta, Segment, StoredSnapshot,
};
use drafftink_server::sealed::{FsBlobStore, FsRelayStore};

fn room() -> RoomId {
    RoomId::from_bytes([0xAB; 16])
}

fn record(seq: u64, len: usize) -> StoredRecord {
    StoredRecord {
        seq,
        from: ClientId::from_bytes([1; 16]),
        ctr: seq,
        record: vec![u8::try_from(seq).unwrap_or(0); len],
    }
}

fn segment(first: u64, last: u64, len: usize) -> Segment {
    Segment { records: (first..=last).map(|seq| record(seq, len)).collect() }
}

fn meta_after(head: u64, records: u32, bytes: u64) -> RoomMeta {
    RoomMeta { head_seq: head, log_records: records, log_bytes: bytes, deadline_ms: 777, ..RoomMeta::default() }
}

/// A way to open the stores of one room again and again, as a restart does.
trait Backend {
    fn relay(&self) -> Box<dyn RelayStore>;
    fn blobs(&self) -> Box<dyn BlobStore>;
    fn persistent(&self) -> bool;
}

fn suite(backend: &dyn Backend) {
    let mut store = backend.relay();
    assert_eq!(store.load_meta().unwrap(), None, "no room yet");
    assert_eq!(store.read_snapshot().unwrap(), None);
    assert!(store.read_since(0, 10, usize::MAX).unwrap().is_empty());
    assert_eq!(
        store.put_snapshot_cas(0, &StoredSnapshot { covers_seq: 0, record: vec![1] }).unwrap(),
        CasOutcome::Stale,
        "no room: nothing to swap"
    );

    store.save_meta(&RoomMeta { deadline_ms: 777, ..RoomMeta::default() }).unwrap();
    assert_eq!(store.load_meta().unwrap().unwrap().deadline_ms, 777);

    // Three segments of two records of 10 bytes: sequence numbers 1 to 6.
    for (n, (first, last)) in [(1, 2), (3, 4), (5, 6)].into_iter().enumerate() {
        let written = u32::try_from(n + 1).unwrap() * 2;
        store.append_segment(&segment(first, last, 10), &meta_after(last, written, u64::from(written) * 10)).unwrap();
    }
    let all = store.read_since(0, 100, usize::MAX).unwrap();
    assert_eq!(all.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(all[2], record(3, 10), "records come back as they went in");
    assert_eq!(store.read_since(3, 100, usize::MAX).unwrap().iter().map(|r| r.seq).collect::<Vec<_>>(), [4, 5, 6]);
    assert_eq!(store.read_since(0, 4, usize::MAX).unwrap().len(), 4, "at most max_records");
    assert_eq!(
        store.read_since(0, 100, 15).unwrap().len(),
        2,
        "stops once the bytes reach the target, after at least one"
    );
    assert_eq!(store.read_since(0, 100, 0).unwrap().len(), 1, "always one");
    assert!(store.read_since(6, 100, usize::MAX).unwrap().is_empty());
    assert_eq!(store.load_meta().unwrap().unwrap(), meta_after(6, 6, 60));

    // Compare-and-set.
    let snap = |covers| StoredSnapshot { covers_seq: covers, record: vec![9; 33] };
    assert_eq!(store.put_snapshot_cas(5, &snap(4)).unwrap(), CasOutcome::Stale, "wrong expectation");
    assert_eq!(store.put_snapshot_cas(0, &snap(7)).unwrap(), CasOutcome::Stale, "cover beyond the head");
    assert_eq!(store.put_snapshot_cas(0, &snap(0)).unwrap(), CasOutcome::Stale, "not an empty room");
    assert_eq!(store.read_snapshot().unwrap(), None, "a refused swap changes nothing");
    assert_eq!(store.read_since(0, 100, usize::MAX).unwrap().len(), 6);
    let CasOutcome::Swapped(log) = store.put_snapshot_cas(0, &snap(3)).unwrap() else { panic!("accepted") };
    assert_eq!(
        (log.records, log.bytes),
        (3, 40),
        "segment 1-2 went; 3-4 straddles the cover and stays whole, so 4 is counted; 5-6 stays"
    );
    assert_eq!(store.read_snapshot().unwrap(), Some(snap(3)));
    assert_eq!(store.read_since(3, 100, usize::MAX).unwrap().iter().map(|r| r.seq).collect::<Vec<_>>(), [4, 5, 6]);
    let meta = store.load_meta().unwrap().unwrap();
    assert_eq!(
        (meta.snapshot_seq, meta.has_snapshot, meta.snapshot_bytes, meta.head_seq, meta.log_records, meta.deadline_ms),
        (3, true, 33, 6, 3, 777)
    );
    assert_eq!(
        store.put_snapshot_cas(0, &snap(6)).unwrap(),
        CasOutcome::Stale,
        "the snapshot moved: expecting 0 is stale"
    );
    assert!(matches!(store.put_snapshot_cas(3, &snap(6)).unwrap(), CasOutcome::Swapped(_)));
    assert!(store.read_since(0, 100, usize::MAX).unwrap().iter().all(|r| r.seq > 3));

    if backend.persistent() {
        let mut again = backend.relay();
        assert_eq!(again.load_meta().unwrap(), store.load_meta().unwrap(), "a restart sees the same room");
        assert_eq!(again.read_snapshot().unwrap().unwrap().covers_seq, 6);
    }

    store.delete_all().unwrap();
    assert_eq!(store.load_meta().unwrap(), None);
    assert_eq!(store.read_snapshot().unwrap(), None);
    assert!(store.read_since(0, 100, usize::MAX).unwrap().is_empty());
    store.delete_all().unwrap();
}

fn seed_suite(backend: &dyn Backend) {
    let mut store = backend.relay();
    store.save_meta(&RoomMeta::default()).unwrap();
    let seed = StoredSnapshot { covers_seq: 0, record: vec![5; 10] };
    assert!(matches!(store.put_snapshot_cas(0, &seed).unwrap(), CasOutcome::Swapped(_)), "an empty room takes a seed");
    assert_eq!(store.put_snapshot_cas(0, &seed).unwrap(), CasOutcome::Stale, "once");
    let meta = store.load_meta().unwrap().unwrap();
    assert!(meta.has_snapshot && meta.snapshot_seq == 0);
    assert_eq!(store.read_snapshot().unwrap(), Some(seed));
}

fn blob_suite(backend: &dyn Backend) {
    let mut blobs = backend.blobs();
    let (a, b) = (ImageName::from_bytes([1; 16]), ImageName::from_bytes([0xFF; 16]));
    assert_eq!(blobs.get(&a).unwrap(), None);
    assert_eq!(blobs.head(&a).unwrap(), None);
    assert_eq!(blobs.stats().unwrap().count, 0);
    assert!(blobs.list().unwrap().is_empty());
    blobs.put(&a, &[1; 100]).unwrap();
    blobs.put(&b, &[2; 50]).unwrap();
    assert_eq!(blobs.get(&a).unwrap().unwrap(), vec![1; 100]);
    assert_eq!(blobs.head(&b).unwrap(), Some(50));
    let stats = blobs.stats().unwrap();
    assert_eq!((stats.count, stats.bytes), (2, 150));
    blobs.put(&a, &[3; 10]).unwrap();
    assert_eq!(blobs.get(&a).unwrap().unwrap(), vec![3; 10], "an upload under the same name replaces");
    let mut list = blobs.list().unwrap();
    list.sort_by_key(|i| i.name);
    assert_eq!(list.iter().map(|i| (i.name, i.size)).collect::<Vec<_>>(), [(a, 10), (b, 50)]);
    if backend.persistent() {
        assert_eq!(backend.blobs().get(&b).unwrap().unwrap(), vec![2; 50]);
    }
    blobs.delete_all().unwrap();
    assert_eq!(blobs.stats().unwrap().count, 0);
    assert_eq!(blobs.get(&a).unwrap(), None);
    blobs.delete_all().unwrap();
}

struct Memory(MemoryRelayStore, MemoryBlobStore);

impl Backend for Memory {
    fn relay(&self) -> Box<dyn RelayStore> {
        Box::new(self.0.clone())
    }
    fn blobs(&self) -> Box<dyn BlobStore> {
        Box::new(self.1.clone())
    }
    fn persistent(&self) -> bool {
        false
    }
}

struct Files(tempfile::TempDir);

impl Backend for Files {
    fn relay(&self) -> Box<dyn RelayStore> {
        Box::new(FsRelayStore::new(self.0.path(), &room()))
    }
    fn blobs(&self) -> Box<dyn BlobStore> {
        Box::new(FsBlobStore::new(self.0.path(), &room()))
    }
    fn persistent(&self) -> bool {
        true
    }
}

#[test]
fn memory_store() {
    let backend = Memory(MemoryRelayStore::new(), MemoryBlobStore::new());
    suite(&backend);
    seed_suite(&Memory(MemoryRelayStore::new(), MemoryBlobStore::new()));
    blob_suite(&backend);
}

#[test]
fn file_store() {
    suite(&Files(tempfile::tempdir().unwrap()));
    seed_suite(&Files(tempfile::tempdir().unwrap()));
    blob_suite(&Files(tempfile::tempdir().unwrap()));
}

#[test]
fn file_store_drops_a_segment_written_by_a_crash_before_its_meta() {
    let files = Files(tempfile::tempdir().unwrap());
    let mut store = files.relay();
    store.save_meta(&RoomMeta::default()).unwrap();
    store.append_segment(&segment(1, 2, 5), &meta_after(2, 2, 10)).unwrap();
    // A crash after the segment of 3 to 4 and before the meta.
    let dir = std::fs::read_dir(files.0.path().join("relay")).unwrap().next().unwrap().unwrap().path();
    std::fs::write(dir.join(format!("seg-{:020}-{:020}", 3, 4)), [0u8; 3]).unwrap();
    let mut reopened = files.relay();
    assert_eq!(reopened.load_meta().unwrap().unwrap().head_seq, 2);
    reopened.append_segment(&segment(3, 3, 5), &meta_after(3, 3, 15)).unwrap();
    assert_eq!(reopened.read_since(0, 10, usize::MAX).unwrap().iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
}

#[test]
fn file_store_reports_damaged_files_and_never_panics() {
    let files = Files(tempfile::tempdir().unwrap());
    let mut store = files.relay();
    store.save_meta(&RoomMeta::default()).unwrap();
    store.append_segment(&segment(1, 1, 5), &meta_after(1, 1, 5)).unwrap();
    let dir = std::fs::read_dir(files.0.path().join("relay")).unwrap().next().unwrap().unwrap().path();
    for garbage in [&b""[..], &b"x"[..], &[0xFF; 100][..]] {
        std::fs::write(dir.join("meta"), garbage).unwrap();
        assert!(files.relay().load_meta().is_err());
    }
    std::fs::write(dir.join("meta"), drafftink_server_meta(1)).unwrap();
    std::fs::write(dir.join(format!("seg-{:020}-{:020}", 1, 1)), [0xFFu8; 10]).unwrap();
    assert!(files.relay().read_since(0, 10, usize::MAX).is_err());
}

fn drafftink_server_meta(head: u64) -> Vec<u8> {
    // The meta of the fixture room, as the store writes it: a version byte and the seven fields.
    let mut bytes = vec![1u8];
    bytes.extend_from_slice(&head.to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    bytes.push(0);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&[0; 8]);
    bytes
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use super::*;
    use drafftink_server::sealed::SqliteRelayDb;

    struct Db(tempfile::TempDir);

    impl Db {
        fn db(&self) -> SqliteRelayDb {
            SqliteRelayDb::open(&self.0.path().join("test.sqlite3")).unwrap()
        }
    }

    impl Backend for Db {
        fn relay(&self) -> Box<dyn RelayStore> {
            Box::new(self.db().relay_store(&room()))
        }
        fn blobs(&self) -> Box<dyn BlobStore> {
            Box::new(self.db().blob_store(&room()))
        }
        fn persistent(&self) -> bool {
            true
        }
    }

    #[test]
    fn sqlite_store() {
        suite(&Db(tempfile::tempdir().unwrap()));
        seed_suite(&Db(tempfile::tempdir().unwrap()));
        blob_suite(&Db(tempfile::tempdir().unwrap()));
    }

    #[test]
    fn two_rooms_in_one_database_do_not_mix() {
        let dir = Db(tempfile::tempdir().unwrap());
        let db = dir.db();
        let other = RoomId::from_bytes([0xCD; 16]);
        let mut a = db.relay_store(&room());
        let mut b = db.relay_store(&other);
        a.save_meta(&RoomMeta::default()).unwrap();
        a.append_segment(&segment(1, 1, 3), &meta_after(1, 1, 3)).unwrap();
        assert_eq!(b.load_meta().unwrap(), None);
        assert!(b.read_since(0, 10, usize::MAX).unwrap().is_empty());
        let name = ImageName::from_bytes([4; 16]);
        db.blob_store(&room()).put(&name, &[1]).unwrap();
        assert_eq!(db.blob_store(&other).get(&name).unwrap(), None);
        b.delete_all().unwrap();
        assert!(a.load_meta().unwrap().is_some());
    }
}
