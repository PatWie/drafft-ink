//! The filesystem provider (the default).
//!
//! Layout: `<dir>/<room>/snapshot.bin` and `<dir>/<room>/updates/<n>.bin`, where `<n>` is a
//! 20 digit log index. Every file is written to a temporary name, fsynced, renamed into place,
//! and the directory is fsynced, so a crash leaves a file whole or absent.
//!
//! Directory names: an upper case letter is stored as `~` and its lower case, so rooms `Ab`
//! and `ab` stay different on case-insensitive filesystems.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use drafftink_protocol::RoomId;

use super::{NextIndex, RoomMeta, RoomStore, StoreError, StoredRoom, check_room};

const SNAPSHOT: &str = "snapshot.bin";
const UPDATES: &str = "updates";
const TMP_SUFFIX: &str = ".tmp";
/// Longest file name most filesystems accept, with room to spare.
const MAX_DIR_NAME: usize = 200;

/// Rooms as files under a directory.
pub struct FsStore {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    next: NextIndex,
}

impl FsStore {
    /// Opens (and creates) the root directory.
    pub async fn open(root: PathBuf) -> Result<Self, StoreError> {
        let made = root.clone();
        run(move || fs::create_dir_all(&made).map_err(StoreError::from)).await?;
        Ok(FsStore { inner: Arc::new(Inner { root, next: NextIndex::default() }) })
    }
}

async fn run<T: Send + 'static>(f: impl FnOnce() -> Result<T, StoreError> + Send + 'static) -> Result<T, StoreError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| StoreError::io(format!("storage task failed: {e}")))?
}

fn dir_name(room: &RoomId) -> Result<String, StoreError> {
    check_room(room)?;
    let mut out = String::with_capacity(room.as_str().len());
    for c in room.as_str().chars() {
        if c.is_ascii_uppercase() {
            out.push('~');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if out.len() > MAX_DIR_NAME {
        return Err(StoreError::InvalidRoom("the room id is too long for the filesystem store".into()));
    }
    Ok(out)
}

fn room_from_dir_name(name: &str) -> Option<RoomId> {
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            out.push(chars.next()?.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    RoomId::new(out).ok()
}

fn sync_dir(dir: &Path) -> Result<(), StoreError> {
    // Directories cannot be opened for fsync on Windows; the rename is still atomic there.
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

/// Writes `bytes` to `path` atomically.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path.parent().ok_or_else(|| StoreError::io("path has no parent"))?;
    let name = path.file_name().ok_or_else(|| StoreError::io("path has no name"))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(TMP_SUFFIX);
    let tmp = parent.join(tmp_name);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    sync_dir(parent)
}

/// Log entries of a room as `(index, path)`, oldest first. Leftover temporary files are
/// removed.
fn log_entries(room_dir: &Path) -> Result<Vec<(u64, PathBuf)>, StoreError> {
    let dir = room_dir.join(UPDATES);
    let read = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for item in read {
        let item = item?;
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        if name.ends_with(TMP_SUFFIX) {
            let _ = fs::remove_file(&path);
            continue;
        }
        if let Some(n) = name.strip_suffix(".bin").and_then(|s| s.parse::<u64>().ok()) {
            out.push((n, path));
        }
    }
    out.sort();
    Ok(out)
}

fn remove_if_exists(path: &Path) -> Result<(), StoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl Inner {
    fn room_dir(&self, room: &RoomId) -> Result<PathBuf, StoreError> {
        Ok(self.root.join(dir_name(room)?))
    }

    fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError> {
        let dir = self.room_dir(room)?;
        let snapshot = read_if_exists(&dir.join(SNAPSHOT))?;
        let entries = log_entries(&dir)?;
        let next = entries.last().map_or(0, |(n, _)| n + 1);
        let mut updates = Vec::with_capacity(entries.len());
        for (_, path) in &entries {
            updates.push(fs::read(path)?);
        }
        self.next.set(room, next);
        if snapshot.is_none() && updates.is_empty() {
            return Ok(None);
        }
        Ok(Some(StoredRoom { snapshot, updates }))
    }

    fn append(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError> {
        let dir = self.room_dir(room)?;
        let updates_dir = dir.join(UPDATES);
        fs::create_dir_all(&updates_dir)?;
        let next = match self.next.get(room) {
            Some(n) => n,
            None => log_entries(&dir)?.last().map_or(0, |(n, _)| n + 1),
        };
        write_atomic(&updates_dir.join(format!("{next:020}.bin")), update)?;
        self.next.set(room, next + 1);
        Ok(())
    }

    fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError> {
        let dir = self.room_dir(room)?;
        let entries = log_entries(&dir)?;
        fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(SNAPSHOT), snapshot)?;
        let drop_n = usize::try_from(covered).unwrap_or(usize::MAX);
        for (_, path) in entries.iter().take(drop_n) {
            remove_if_exists(path)?;
        }
        if !entries.is_empty() {
            sync_dir(&dir.join(UPDATES))?;
        }
        Ok(())
    }

    fn delete(&self, room: &RoomId) -> Result<(), StoreError> {
        let dir = self.room_dir(room)?;
        self.next.forget(room);
        match fs::remove_dir_all(&dir) {
            Ok(()) => sync_dir(&self.root),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn list(&self) -> Result<Vec<RoomMeta>, StoreError> {
        let mut out = Vec::new();
        for item in fs::read_dir(&self.root)? {
            let item = item?;
            let Some(room) = room_from_dir_name(&item.file_name().to_string_lossy()) else { continue };
            if !item.file_type()?.is_dir() {
                continue;
            }
            let dir = item.path();
            let mut newest = modified_ms(&dir.join(SNAPSHOT));
            for (_, path) in log_entries(&dir)? {
                newest = newest.max(modified_ms(&path));
            }
            out.push(RoomMeta { room, modified_ms: newest });
        }
        Ok(out)
    }
}

fn modified_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[async_trait]
impl RoomStore for FsStore {
    fn name(&self) -> &'static str {
        "filesystem"
    }

    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError> {
        let (inner, room) = (self.inner.clone(), room.clone());
        run(move || inner.load(&room)).await
    }

    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError> {
        let (inner, room, update) = (self.inner.clone(), room.clone(), update.to_vec());
        run(move || inner.append(&room, &update)).await
    }

    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError> {
        let (inner, room, snapshot) = (self.inner.clone(), room.clone(), snapshot.to_vec());
        run(move || inner.compact(&room, &snapshot, covered)).await
    }

    async fn delete(&self, room: &RoomId) -> Result<(), StoreError> {
        let (inner, room) = (self.inner.clone(), room.clone());
        run(move || inner.delete(&room)).await
    }

    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError> {
        let inner = self.inner.clone();
        run(move || inner.list()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_names_round_trip_and_fold_case() {
        let room = RoomId::new("AbC-1_x.y").expect("valid");
        let name = dir_name(&room).expect("name");
        assert_eq!(name, "~ab~c-1_x.y");
        assert_eq!(room_from_dir_name(&name), Some(room));
        assert_ne!(dir_name(&RoomId::new("ab").expect("valid")).expect("n"), name);
    }

    #[test]
    fn dot_rooms_are_refused() {
        for bad in [".", "..", ".hidden"] {
            let room = RoomId::new(bad).expect("valid id");
            assert!(matches!(dir_name(&room), Err(StoreError::InvalidRoom(_))), "{bad}");
        }
    }

    #[test]
    fn overlong_encoded_names_are_refused() {
        let room = RoomId::new("A".repeat(128)).expect("valid id");
        assert!(matches!(dir_name(&room), Err(StoreError::InvalidRoom(_))));
    }
}
