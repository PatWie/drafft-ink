//! Sealed rooms in the SQLite database of the server. Tables (all keyed by the 16 bytes of the room id):
//! `sealed_rooms` (the meta), `sealed_snapshots`, `sealed_segments` (one row per flush) and `sealed_images`.
//! A snapshot swap is one transaction: the snapshot, the new meta and the deleted segments.

use std::path::Path;
use std::sync::{Arc, Mutex};

use drafftink_protocol::v2::{ImageName, RoomId, StoredRecord};
use drafftink_relay::{
    BlobInfo, BlobStats, BlobStore, CasOutcome, LogCounts, RelayStore, RoomMeta, Segment, StoreError, StoredSnapshot,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use drafftink_relay::codec::{decode_records, encode_records};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sealed_rooms (
    room           BLOB PRIMARY KEY NOT NULL,
    head_seq       INTEGER NOT NULL,
    snapshot_seq   INTEGER NOT NULL,
    has_snapshot   INTEGER NOT NULL,
    snapshot_bytes INTEGER NOT NULL,
    log_records    INTEGER NOT NULL,
    log_bytes      INTEGER NOT NULL,
    deadline_ms    INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sealed_snapshots (
    room   BLOB PRIMARY KEY NOT NULL,
    covers INTEGER NOT NULL,
    record BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sealed_segments (
    room  BLOB NOT NULL,
    first INTEGER NOT NULL,
    last  INTEGER NOT NULL,
    data  BLOB NOT NULL,
    PRIMARY KEY (room, first)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sealed_images (
    room BLOB NOT NULL,
    name BLOB NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (room, name)
) WITHOUT ROWID;
";

fn backend(e: rusqlite::Error) -> StoreError {
    StoreError::backend(format!("sqlite: {e}"))
}

fn lock(conn: &Mutex<Connection>) -> std::sync::MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn to_u64(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// The database of every sealed room of a server.
#[derive(Debug, Clone)]
pub struct SqliteRelayDb {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRelayDb {
    /// Opens (and creates) the database file.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::backend(format!("file system: {e}")))?;
        }
        let conn = Connection::open(path).map_err(backend)?;
        conn.busy_timeout(std::time::Duration::from_secs(10)).map_err(backend)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(backend)?;
        conn.pragma_update(None, "synchronous", "FULL").map_err(backend)?;
        conn.execute_batch(SCHEMA).map_err(backend)?;
        Ok(SqliteRelayDb { conn: Arc::new(Mutex::new(conn)) })
    }

    /// The rooms in the database.
    pub(super) fn list_rooms(&self) -> Vec<RoomId> {
        let conn = lock(&self.conn);
        let Ok(mut stmt) = conn.prepare("SELECT room FROM sealed_rooms") else { return Vec::new() };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0)) else { return Vec::new() };
        rows.flatten().filter_map(|bytes| Some(RoomId::from_bytes(bytes.try_into().ok()?))).collect()
    }

    /// The store of one room.
    pub fn relay_store(&self, room: &RoomId) -> SqliteRelayStore {
        SqliteRelayStore { db: self.clone(), room: room.as_bytes().to_vec() }
    }

    /// The image store of one room.
    pub fn blob_store(&self, room: &RoomId) -> SqliteBlobStore {
        SqliteBlobStore { db: self.clone(), room: room.as_bytes().to_vec() }
    }
}

/// One sealed room in the database.
#[derive(Debug)]
pub struct SqliteRelayStore {
    db: SqliteRelayDb,
    room: Vec<u8>,
}

fn read_meta(tx: &Transaction<'_>, room: &[u8]) -> Result<Option<RoomMeta>, rusqlite::Error> {
    tx.query_row(
        "SELECT head_seq, snapshot_seq, has_snapshot, snapshot_bytes, log_records, log_bytes, deadline_ms \
         FROM sealed_rooms WHERE room = ?1",
        params![room],
        |r| {
            Ok(RoomMeta {
                head_seq: to_u64(r.get(0)?),
                snapshot_seq: to_u64(r.get(1)?),
                has_snapshot: r.get::<_, i64>(2)? != 0,
                snapshot_bytes: to_u64(r.get(3)?),
                log_records: u32::try_from(r.get::<_, i64>(4)?).unwrap_or(u32::MAX),
                log_bytes: to_u64(r.get(5)?),
                deadline_ms: to_u64(r.get(6)?),
            })
        },
    )
    .optional()
}

fn write_meta(tx: &Transaction<'_>, room: &[u8], m: &RoomMeta) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT OR REPLACE INTO sealed_rooms \
         (room, head_seq, snapshot_seq, has_snapshot, snapshot_bytes, log_records, log_bytes, deadline_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            room,
            to_i64(m.head_seq),
            to_i64(m.snapshot_seq),
            i64::from(m.has_snapshot),
            to_i64(m.snapshot_bytes),
            i64::from(m.log_records),
            to_i64(m.log_bytes),
            to_i64(m.deadline_ms)
        ],
    )
    .map(drop)
}

impl SqliteRelayStore {
    fn run<T>(&self, f: impl FnOnce(&mut Transaction<'_>, &[u8]) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut conn = lock(&self.db.conn);
        let mut tx = conn.transaction().map_err(backend)?;
        let out = f(&mut tx, &self.room)?;
        tx.commit().map_err(backend)?;
        Ok(out)
    }
}

impl RelayStore for SqliteRelayStore {
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError> {
        self.run(|tx, room| read_meta(tx, room).map_err(backend))
    }

    fn save_meta(&mut self, meta: &RoomMeta) -> Result<(), StoreError> {
        self.run(|tx, room| write_meta(tx, room, meta).map_err(backend))
    }

    fn append_segment(&mut self, segment: &Segment, meta: &RoomMeta) -> Result<(), StoreError> {
        self.run(|tx, room| {
            tx.execute(
                "INSERT OR REPLACE INTO sealed_segments (room, first, last, data) VALUES (?1, ?2, ?3, ?4)",
                params![
                    room,
                    to_i64(segment.first_seq()),
                    to_i64(segment.last_seq()),
                    encode_records(&segment.records)
                ],
            )
            .map_err(backend)?;
            write_meta(tx, room, meta).map_err(backend)
        })
    }

    fn read_since(
        &mut self,
        after_seq: u64,
        max_records: usize,
        target_bytes: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        self.run(|tx, room| {
            let head = read_meta(tx, room).map_err(backend)?.map_or(0, |m| m.head_seq);
            let mut stmt = tx
                .prepare(
                    "SELECT data FROM sealed_segments WHERE room = ?1 AND last > ?2 AND first <= ?3 ORDER BY first",
                )
                .map_err(backend)?;
            let rows = stmt
                .query_map(params![room, to_i64(after_seq), to_i64(head)], |r| r.get::<_, Vec<u8>>(0))
                .map_err(backend)?;
            let mut out: Vec<StoredRecord> = Vec::new();
            let mut bytes = 0usize;
            for data in rows {
                let records = decode_records(&data.map_err(backend)?)?;
                for record in records.into_iter().filter(|r| r.seq > after_seq && r.seq <= head) {
                    if out.len() >= max_records || (!out.is_empty() && bytes >= target_bytes) {
                        return Ok(out);
                    }
                    bytes += record.record.len();
                    out.push(record);
                }
            }
            Ok(out)
        })
    }

    fn read_snapshot(&mut self) -> Result<Option<StoredSnapshot>, StoreError> {
        self.run(|tx, room| {
            tx.query_row("SELECT covers, record FROM sealed_snapshots WHERE room = ?1", params![room], |r| {
                Ok(StoredSnapshot { covers_seq: to_u64(r.get(0)?), record: r.get(1)? })
            })
            .optional()
            .map_err(backend)
        })
    }

    fn put_snapshot_cas(
        &mut self,
        expected_snapshot_seq: u64,
        snapshot: &StoredSnapshot,
    ) -> Result<CasOutcome, StoreError> {
        self.run(|tx, room| {
            let Some(mut meta) = read_meta(tx, room).map_err(backend)? else { return Ok(CasOutcome::Stale) };
            if !meta.accepts_snapshot(expected_snapshot_seq, snapshot.covers_seq) {
                return Ok(CasOutcome::Stale);
            }
            let covers = to_i64(snapshot.covers_seq);
            tx.execute(
                "INSERT OR REPLACE INTO sealed_snapshots (room, covers, record) VALUES (?1, ?2, ?3)",
                params![room, covers, snapshot.record],
            )
            .map_err(backend)?;
            tx.execute("DELETE FROM sealed_segments WHERE room = ?1 AND last <= ?2", params![room, covers])
                .map_err(backend)?;
            let mut log = LogCounts { records: 0, bytes: 0 };
            let mut stmt = tx.prepare("SELECT data FROM sealed_segments WHERE room = ?1").map_err(backend)?;
            let rows = stmt.query_map(params![room], |r| r.get::<_, Vec<u8>>(0)).map_err(backend)?;
            for data in rows {
                for record in decode_records(&data.map_err(backend)?)? {
                    log.bytes += record.record.len() as u64;
                    log.records += u32::from(record.seq > snapshot.covers_seq);
                }
            }
            drop(stmt);
            meta.snapshot_seq = snapshot.covers_seq;
            meta.has_snapshot = true;
            meta.snapshot_bytes = snapshot.record.len() as u64;
            meta.log_records = log.records;
            meta.log_bytes = log.bytes;
            write_meta(tx, room, &meta).map_err(backend)?;
            Ok(CasOutcome::Swapped(log))
        })
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        self.run(|tx, room| {
            for table in ["sealed_rooms", "sealed_snapshots", "sealed_segments", "sealed_images"] {
                tx.execute(&format!("DELETE FROM {table} WHERE room = ?1"), params![room]).map_err(backend)?;
            }
            Ok(())
        })
    }
}

/// The sealed images of one room in the database.
#[derive(Debug)]
pub struct SqliteBlobStore {
    db: SqliteRelayDb,
    room: Vec<u8>,
}

impl SqliteBlobStore {
    fn with<T>(&self, f: impl FnOnce(&Connection, &[u8]) -> Result<T, rusqlite::Error>) -> Result<T, StoreError> {
        f(&lock(&self.db.conn), &self.room).map_err(backend)
    }
}

impl BlobStore for SqliteBlobStore {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), StoreError> {
        self.with(|conn, room| {
            conn.execute(
                "INSERT OR REPLACE INTO sealed_images (room, name, data) VALUES (?1, ?2, ?3)",
                params![room, name.as_bytes().as_slice(), sealed],
            )
            .map(drop)
        })
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, StoreError> {
        self.with(|conn, room| {
            conn.query_row(
                "SELECT data FROM sealed_images WHERE room = ?1 AND name = ?2",
                params![room, name.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()
        })
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, StoreError> {
        self.with(|conn, room| {
            conn.query_row(
                "SELECT length(data) FROM sealed_images WHERE room = ?1 AND name = ?2",
                params![room, name.as_bytes().as_slice()],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|size| size.map(to_u64))
        })
    }

    fn list(&mut self) -> Result<Vec<BlobInfo>, StoreError> {
        self.with(|conn, room| {
            let mut stmt =
                conn.prepare("SELECT name, length(data) FROM sealed_images WHERE room = ?1 ORDER BY name")?;
            let rows = stmt.query_map(params![room], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))?;
            Ok(rows
                .flatten()
                .filter_map(|(name, size)| {
                    Some(BlobInfo { name: ImageName::from_bytes(name.try_into().ok()?), size: to_u64(size) })
                })
                .collect())
        })
    }

    fn stats(&mut self) -> Result<BlobStats, StoreError> {
        self.with(|conn, room| {
            conn.query_row(
                "SELECT count(*), coalesce(sum(length(data)), 0) FROM sealed_images WHERE room = ?1",
                params![room],
                |r| {
                    Ok(BlobStats {
                        count: u32::try_from(r.get::<_, i64>(0)?).unwrap_or(u32::MAX),
                        bytes: to_u64(r.get(1)?),
                    })
                },
            )
        })
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        self.with(|conn, room| conn.execute("DELETE FROM sealed_images WHERE room = ?1", params![room]).map(drop))
    }
}
