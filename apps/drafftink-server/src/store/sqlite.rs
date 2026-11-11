//! The SQLite provider: one database file in WAL mode.
//!
//! Tables: `rooms` (id, snapshot, modified time) and `updates` (room, log index, bytes). A
//! compaction replaces the snapshot and deletes the covered rows in one transaction.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use drafftink_protocol::RoomId;
use rusqlite::{Connection, OptionalExtension, params};

use super::{RoomMeta, RoomStore, StoreError, StoredRoom, check_room, lock};
use crate::clock::now_ms;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS rooms (
    id          TEXT PRIMARY KEY NOT NULL,
    snapshot    BLOB,
    modified_ms INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS updates (
    room TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    seq  INTEGER NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (room, seq)
) WITHOUT ROWID;
";

/// Rooms in a SQLite database.
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
}

fn backend(e: rusqlite::Error) -> StoreError {
    StoreError::Backend(format!("sqlite: {e}"))
}

impl SqliteStore {
    /// Opens (and creates) the database file.
    pub async fn open(path: PathBuf) -> Result<Self, StoreError> {
        let conn = tokio::task::spawn_blocking(move || -> Result<Connection, StoreError> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let conn = Connection::open(&path).map_err(backend)?;
            conn.busy_timeout(std::time::Duration::from_secs(10)).map_err(backend)?;
            // WAL with synchronous=FULL: a committed append survives a power cut.
            conn.pragma_update(None, "journal_mode", "WAL").map_err(backend)?;
            conn.pragma_update(None, "synchronous", "FULL").map_err(backend)?;
            conn.pragma_update(None, "foreign_keys", "ON").map_err(backend)?;
            conn.execute_batch(SCHEMA).map_err(backend)?;
            Ok(conn)
        })
        .await
        .map_err(|e| StoreError::io(format!("storage task failed: {e}")))??;
        Ok(SqliteStore { conn: Arc::new(Mutex::new(conn)) })
    }

    async fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    ) -> Result<T, StoreError> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || f(&mut lock(&conn)).map_err(backend))
            .await
            .map_err(|e| StoreError::io(format!("storage task failed: {e}")))?
    }
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

#[async_trait]
impl RoomStore for SqliteStore {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError> {
        check_room(room)?;
        let id = room.as_str().to_owned();
        self.with(move |conn| {
            let tx = conn.transaction()?;
            let row: Option<Option<Vec<u8>>> =
                tx.query_row("SELECT snapshot FROM rooms WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
            let Some(snapshot) = row else { return Ok(None) };
            let mut stmt = tx.prepare("SELECT data FROM updates WHERE room = ?1 ORDER BY seq")?;
            let updates = stmt.query_map(params![id], |r| r.get::<_, Vec<u8>>(0))?.collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            tx.commit()?;
            Ok(Some(StoredRoom { snapshot, updates }))
        })
        .await
    }

    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError> {
        check_room(room)?;
        let (id, data, now) = (room.as_str().to_owned(), update.to_vec(), to_i64(now_ms()));
        self.with(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO rooms (id, snapshot, modified_ms) VALUES (?1, NULL, ?2)
                 ON CONFLICT(id) DO UPDATE SET modified_ms = excluded.modified_ms",
                params![id, now],
            )?;
            tx.execute(
                "INSERT INTO updates (room, seq, data)
                 SELECT ?1, COALESCE(MAX(seq) + 1, 0), ?2 FROM updates WHERE room = ?1",
                params![id, data],
            )?;
            tx.commit()
        })
        .await
    }

    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError> {
        check_room(room)?;
        let (id, data, now) = (room.as_str().to_owned(), snapshot.to_vec(), to_i64(now_ms()));
        self.with(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO rooms (id, snapshot, modified_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET snapshot = excluded.snapshot, modified_ms = excluded.modified_ms",
                params![id, data, now],
            )?;
            tx.execute(
                "DELETE FROM updates WHERE room = ?1 AND seq IN
                 (SELECT seq FROM updates WHERE room = ?1 ORDER BY seq LIMIT ?2)",
                params![id, to_i64(covered)],
            )?;
            tx.commit()
        })
        .await
    }

    async fn delete(&self, room: &RoomId) -> Result<(), StoreError> {
        check_room(room)?;
        let id = room.as_str().to_owned();
        self.with(move |conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM updates WHERE room = ?1", params![id])?;
            tx.execute("DELETE FROM rooms WHERE id = ?1", params![id])?;
            tx.commit()
        })
        .await
    }

    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError> {
        let rows = self
            .with(|conn| {
                let mut stmt = conn.prepare("SELECT id, modified_ms FROM rooms")?;
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, ms)| {
                Some(RoomMeta { room: RoomId::new(id).ok()?, modified_ms: u64::try_from(ms).unwrap_or(0) })
            })
            .collect())
    }
}
