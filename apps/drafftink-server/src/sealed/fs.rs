//! Sealed rooms as files. Layout under `<root>/relay/<room>/`:
//!
//! ```text
//! meta                          the room meta (written last: it is the commit point of a write)
//! snapshot                      cover (8 bytes) and the sealed snapshot record
//! seg-<first>-<last>            one segment: the records of one flush, 20-digit sequence numbers
//! images/<hex of the name>      one sealed image each
//! ```
//!
//! Every file is written to a temporary name, fsynced and renamed, and the directory is fsynced, so a crash leaves
//! a file whole or absent. Segments above the meta's head (a crash between a segment and the meta) are removed
//! when the room is opened.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use drafftink_protocol::v2::{ImageName, RoomId, StoredRecord};
use drafftink_relay::{
    BlobInfo, BlobStats, BlobStore, CasOutcome, LogCounts, RelayStore, RoomMeta, Segment, StoreError, StoredSnapshot,
};

use drafftink_relay::codec::{
    decode_meta, decode_records, decode_snapshot, encode_meta, encode_records, encode_snapshot,
};

const META: &str = "meta";
const SNAPSHOT: &str = "snapshot";
const IMAGES: &str = "images";
const SEGMENT_PREFIX: &str = "seg-";
const TMP_SUFFIX: &str = ".tmp";
/// The directory below the storage root that holds sealed rooms, next to the plaintext rooms.
pub(super) const ROOMS_DIR: &str = "relay";

fn io(e: &std::io::Error) -> StoreError {
    StoreError::backend(format!("file system: {e}"))
}

/// A directory name for a room id: an upper case letter is stored as `~` and its lower case, so two ids that differ
/// in case stay different on case-insensitive file systems.
fn dir_name(room: &RoomId) -> String {
    room.to_string().chars().fold(String::new(), |mut out, c| {
        if c.is_ascii_uppercase() {
            out.push('~');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
        out
    })
}

fn room_from_dir_name(name: &str) -> Option<RoomId> {
    let mut text = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        text.push(if c == '~' { chars.next()?.to_ascii_uppercase() } else { c });
    }
    text.parse().ok()
}

fn sync_dir(dir: &Path) -> Result<(), StoreError> {
    // Directories cannot be opened for fsync on Windows; the rename is still atomic there.
    match File::open(dir) {
        Ok(file) => file.sync_all().or_else(|e| if cfg!(windows) { Ok(()) } else { Err(io(&e)) }),
        Err(_) if cfg!(windows) => Ok(()),
        Err(e) => Err(io(&e)),
    }
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), StoreError> {
    fs::create_dir_all(dir).map_err(|e| io(&e))?;
    let tmp = dir.join(format!("{name}{TMP_SUFFIX}"));
    let mut file = File::create(&tmp).map_err(|e| io(&e))?;
    file.write_all(bytes).map_err(|e| io(&e))?;
    file.sync_all().map_err(|e| io(&e))?;
    fs::rename(&tmp, dir.join(name)).map_err(|e| io(&e))?;
    sync_dir(dir)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(&e)),
    }
}

fn remove_dir_if_there(dir: &Path) -> Result<(), StoreError> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(&e)),
    }
}

/// The rooms stored under `root`.
pub(super) fn list_rooms(root: &Path) -> Vec<RoomId> {
    let Ok(entries) = fs::read_dir(root.join(ROOMS_DIR)) else { return Vec::new() };
    entries.flatten().filter_map(|e| e.file_name().to_str().and_then(room_from_dir_name)).collect()
}

/// One sealed room in a directory.
#[derive(Debug)]
pub struct FsRelayStore {
    dir: PathBuf,
    checked_orphans: bool,
}

impl FsRelayStore {
    /// The store of `room` under `root`. Nothing is created until the room is written.
    pub fn new(root: &Path, room: &RoomId) -> Self {
        FsRelayStore { dir: root.join(ROOMS_DIR).join(dir_name(room)), checked_orphans: false }
    }

    fn segments(&self) -> Result<Vec<SegmentFile>, StoreError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&e)),
        };
        let mut files: Vec<SegmentFile> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_owned();
                let (first, last) = name.strip_prefix(SEGMENT_PREFIX)?.split_once('-')?;
                Some(SegmentFile { first: first.parse().ok()?, last: last.parse().ok()?, path: e.path() })
            })
            .collect();
        files.sort_by_key(|f| f.first);
        Ok(files)
    }

    fn read_meta(&self) -> Result<Option<RoomMeta>, StoreError> {
        read_optional(&self.dir.join(META))?.map(|bytes| decode_meta(&bytes)).transpose()
    }

    fn write_meta(&self, meta: &RoomMeta) -> Result<(), StoreError> {
        write_atomic(&self.dir, META, &encode_meta(meta))
    }

    /// Removes the segments of a write that crashed before its meta was saved.
    fn drop_orphans(&mut self, head_seq: u64) -> Result<(), StoreError> {
        if !self.checked_orphans {
            for file in self.segments()?.into_iter().filter(|f| f.first > head_seq) {
                fs::remove_file(&file.path).map_err(|e| io(&e))?;
            }
            self.checked_orphans = true;
        }
        Ok(())
    }
}

struct SegmentFile {
    first: u64,
    last: u64,
    path: PathBuf,
}

impl RelayStore for FsRelayStore {
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError> {
        let meta = self.read_meta()?;
        if let Some(meta) = &meta {
            self.drop_orphans(meta.head_seq)?;
        }
        Ok(meta)
    }

    fn save_meta(&mut self, meta: &RoomMeta) -> Result<(), StoreError> {
        self.write_meta(meta)
    }

    fn append_segment(&mut self, segment: &Segment, meta: &RoomMeta) -> Result<(), StoreError> {
        let name = format!("{SEGMENT_PREFIX}{:020}-{:020}", segment.first_seq(), segment.last_seq());
        write_atomic(&self.dir, &name, &encode_records(&segment.records))?;
        self.write_meta(meta)
    }

    fn read_since(
        &mut self,
        after_seq: u64,
        max_records: usize,
        target_bytes: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        let head = self.read_meta()?.map_or(0, |m| m.head_seq);
        let mut out: Vec<StoredRecord> = Vec::new();
        let mut bytes = 0usize;
        for file in self.segments()?.into_iter().filter(|f| f.last > after_seq && f.first <= head) {
            let records = decode_records(&read_optional(&file.path)?.unwrap_or_default())?;
            for record in records.into_iter().filter(|r| r.seq > after_seq && r.seq <= head) {
                if out.len() >= max_records || (!out.is_empty() && bytes >= target_bytes) {
                    return Ok(out);
                }
                bytes += record.record.len();
                out.push(record);
            }
        }
        Ok(out)
    }

    fn read_snapshot(&mut self) -> Result<Option<StoredSnapshot>, StoreError> {
        read_optional(&self.dir.join(SNAPSHOT))?.map(|bytes| decode_snapshot(&bytes)).transpose()
    }

    fn put_snapshot_cas(
        &mut self,
        expected_snapshot_seq: u64,
        snapshot: &StoredSnapshot,
    ) -> Result<CasOutcome, StoreError> {
        let Some(mut meta) = self.read_meta()? else { return Ok(CasOutcome::Stale) };
        if !meta.accepts_snapshot(expected_snapshot_seq, snapshot.covers_seq) {
            return Ok(CasOutcome::Stale);
        }
        write_atomic(&self.dir, SNAPSHOT, &encode_snapshot(snapshot.covers_seq, &snapshot.record))?;
        let (covered, kept): (Vec<_>, Vec<_>) =
            self.segments()?.into_iter().partition(|f| f.last <= snapshot.covers_seq);
        let mut log = LogCounts { records: 0, bytes: 0 };
        for file in &kept {
            for record in decode_records(&read_optional(&file.path)?.unwrap_or_default())? {
                log.bytes += record.record.len() as u64;
                log.records += u32::from(record.seq > snapshot.covers_seq);
            }
        }
        meta.snapshot_seq = snapshot.covers_seq;
        meta.has_snapshot = true;
        meta.snapshot_bytes = snapshot.record.len() as u64;
        meta.log_records = log.records;
        meta.log_bytes = log.bytes;
        self.write_meta(&meta)?;
        for file in covered {
            fs::remove_file(&file.path).map_err(|e| io(&e))?;
        }
        Ok(CasOutcome::Swapped(log))
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        // The images go with the room, but the blob store deletes its own directory too.
        remove_dir_if_there(&self.dir)
    }
}

/// The sealed images of one room, one file each. The file name is the hex of the keyed name, so two names that
/// differ only in case stay different on case-insensitive file systems.
#[derive(Debug)]
pub struct FsBlobStore {
    dir: PathBuf,
}

impl FsBlobStore {
    /// The image store of `room` under `root`.
    pub fn new(root: &Path, room: &RoomId) -> Self {
        FsBlobStore { dir: root.join(ROOMS_DIR).join(dir_name(room)).join(IMAGES) }
    }

    fn path(&self, name: &ImageName) -> PathBuf {
        self.dir.join(hex(name.as_bytes()))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<ImageName> {
    let bytes: Vec<u8> =
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect::<Option<_>>()?;
    Some(ImageName::from_bytes(bytes.try_into().ok()?))
}

impl BlobStore for FsBlobStore {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), StoreError> {
        write_atomic(&self.dir, &hex(name.as_bytes()), sealed)
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, StoreError> {
        read_optional(&self.path(name))
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, StoreError> {
        match fs::metadata(self.path(name)) {
            Ok(meta) => Ok(Some(meta.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(&e)),
        }
    }

    fn list(&mut self) -> Result<Vec<BlobInfo>, StoreError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&e)),
        };
        let mut list: Vec<BlobInfo> = entries
            .flatten()
            .filter_map(|e| {
                let name = from_hex(e.file_name().to_str()?)?;
                Some(BlobInfo { name, size: e.metadata().ok()?.len() })
            })
            .collect();
        list.sort_by_key(|info| info.name);
        Ok(list)
    }

    fn stats(&mut self) -> Result<BlobStats, StoreError> {
        let list = self.list()?;
        Ok(BlobStats { count: u32::try_from(list.len()).unwrap_or(u32::MAX), bytes: list.iter().map(|i| i.size).sum() })
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        remove_dir_if_there(&self.dir)
    }
}
