//! Storage on disk: one folder per namespace, one file per key.
//!
//! Keys are written percent-encoded, so any key makes a valid file name on every platform. A write goes to a
//! temporary file in the same folder and is renamed over the target, so a crash leaves the old content or the
//! new, never half of it.
//!
//! `put_later` hands the write to a worker thread (the autosave of a big document is 45 MB, and the write ends with
//! an `fsync`), so a frame does not wait for the disk. The worker keeps the order of the writes; every other call
//! waits for the writes before it, so a read sees what was written.

use std::collections::VecDeque;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::UNIX_EPOCH;

use drafftink_engine::PortError;
use drafftink_engine::ports::{Ns, Storage, StoredInfo};

/// The files of the app under one root folder. Cheap to clone; the clones share one writer.
#[derive(Debug, Clone)]
pub struct NativeStore {
    root: PathBuf,
    writer: Arc<OnceLock<Option<Writer>>>,
}

impl NativeStore {
    /// A store rooted at `root`. Folders are created on the first write.
    pub fn new(root: PathBuf) -> Self {
        Self { root, writer: Arc::new(OnceLock::new()) }
    }

    /// The thread that carries out `put_later`, started on first use. `None` when no thread can be started.
    fn writer(&self) -> Option<&Writer> {
        self.writer.get_or_init(Writer::start).as_ref()
    }

    /// Waits until every write that was handed to the writer is on disk.
    fn flush(&self) {
        if let Some(Some(w)) = self.writer.get() {
            w.wait_idle();
        }
    }

    fn dir(&self, ns: Ns) -> PathBuf {
        let name = match ns {
            Ns::Documents => "documents",
            Ns::Autosave => "autosave",
            Ns::Settings => "settings",
            Ns::Library => "library",
        };
        self.root.join(name)
    }

    fn path(&self, ns: Ns, key: &str) -> PathBuf {
        self.dir(ns).join(encode_key(key))
    }
}

/// Keeps letters, digits, `.`, `_` and `-` (not as the first character of a dot-file) and encodes the rest.
fn encode_key(key: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(key.len());
    for (i, b) in key.bytes().enumerate() {
        let plain = b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (b == b'.' && i > 0);
        if plain {
            out.push(char::from(b));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
    if out.is_empty() { "%".to_owned() } else { out }
}

fn decode_key(name: &str) -> Option<String> {
    if name == "%" {
        return Some(String::new());
    }
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn failed(what: &str, path: &Path, e: &std::io::Error) -> PortError {
    PortError::Failed(format!("{what} {}: {e}", path.display()))
}

impl Storage for NativeStore {
    fn get(&self, ns: Ns, key: &str) -> Result<Option<Vec<u8>>, PortError> {
        self.flush();
        let path = self.path(ns, key);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(failed("could not read", &path, &e)),
        }
    }

    fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError> {
        self.flush();
        write_atomic(&self.dir(ns), key, bytes)
    }

    fn put_later(&mut self, ns: Ns, key: &str, bytes: Vec<u8>) -> Result<(), PortError> {
        match self.writer() {
            Some(w) => {
                w.submit(Job::Put { dir: self.dir(ns), key: key.to_owned(), bytes });
                Ok(())
            }
            None => write_atomic(&self.dir(ns), key, &bytes),
        }
    }

    fn take_write_error(&mut self) -> Option<PortError> {
        let Some(Some(w)) = self.writer.get() else { return None };
        w.take_error().map(PortError::Failed)
    }

    fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError> {
        let path = self.path(ns, key);
        if let Some(Some(w)) = self.writer.get() {
            // Behind the writes that are queued, in order, without making the caller wait.
            w.submit(Job::Delete { path });
            return Ok(());
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(failed("could not delete", &path, &e)),
        }
    }

    fn list(&self, ns: Ns) -> Result<Vec<StoredInfo>, PortError> {
        self.flush();
        let dir = self.dir(ns);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(failed("could not list", &dir, &e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().filter(|n| !n.starts_with(".tmp-")).and_then(decode_key) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            out.push(StoredInfo { name, size: meta.len(), modified_ms });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

/// Writes `bytes` to a temporary file in `dir` and renames it over `key`.
fn write_atomic(dir: &Path, key: &str, bytes: &[u8]) -> Result<(), PortError> {
    fs::create_dir_all(dir).map_err(|e| failed("could not create the folder", dir, &e))?;
    let target = dir.join(encode_key(key));
    let temp = dir.join(format!(".tmp-{}", encode_key(key)));
    let mut file = fs::File::create(&temp).map_err(|e| failed("could not write", &temp, &e))?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| failed("could not write", &temp, &e))?;
    drop(file);
    fs::rename(&temp, &target).map_err(|e| failed("could not replace", &target, &e))
}

/// Work for the writer thread.
enum Job {
    Put { dir: PathBuf, key: String, bytes: Vec<u8> },
    Delete { path: PathBuf },
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Jobs queued or running.
    pending: usize,
    /// The first failure since it was last asked for.
    error: Option<String>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    /// The writer waits here for a job.
    work: Condvar,
    /// Callers wait here for the end of the jobs.
    idle: Condvar,
}

fn lock(shared: &Shared) -> MutexGuard<'_, Queue> {
    shared.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One thread that carries out the writes of `put_later` in the order they came.
struct Writer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer").field("pending", &lock(&self.shared).pending).finish()
    }
}

impl Writer {
    fn start() -> Option<Self> {
        let shared =
            Arc::new(Shared { queue: Mutex::new(Queue::default()), work: Condvar::new(), idle: Condvar::new() });
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new().name("drafftink-storage".into()).spawn(move || run(&theirs)).ok()?;
        Some(Self { shared, thread: Some(thread) })
    }

    fn submit(&self, job: Job) {
        let mut q = lock(&self.shared);
        q.jobs.push_back(job);
        q.pending += 1;
        self.shared.work.notify_one();
    }

    fn wait_idle(&self) {
        let mut q = lock(&self.shared);
        while q.pending > 0 {
            q = self.shared.idle.wait(q).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn take_error(&self) -> Option<String> {
        lock(&self.shared).error.take()
    }
}

impl Drop for Writer {
    /// The last owner of the store went away: the writes that are queued are finished first.
    fn drop(&mut self) {
        lock(&self.shared).closed = true;
        self.shared.work.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(shared: &Shared) {
    loop {
        let job = {
            let mut q = lock(shared);
            loop {
                if let Some(job) = q.jobs.pop_front() {
                    break job;
                }
                if q.closed {
                    return;
                }
                q = shared.work.wait(q).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let result = match job {
            Job::Put { dir, key, bytes } => write_atomic(&dir, &key, &bytes),
            Job::Delete { path } => match fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(failed("could not delete", &path, &e)),
                _ => Ok(()),
            },
        };
        let mut q = lock(shared);
        if let Err(e) = result {
            q.error.get_or_insert(e.to_string());
        }
        q.pending -= 1;
        if q.pending == 0 {
            shared.idle.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile_dir::Dir, NativeStore) {
        let dir = tempfile_dir::Dir::new();
        let s = NativeStore::new(dir.path().to_owned());
        (dir, s)
    }

    /// A throwaway folder without an extra dependency.
    mod tempfile_dir {
        use std::path::{Path, PathBuf};
        pub(super) struct Dir(PathBuf);
        impl Dir {
            pub(super) fn new() -> Self {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let p = std::env::temp_dir().join(format!("drafftink-store-test-{}-{n}", std::process::id()));
                let _ = std::fs::remove_dir_all(&p);
                Self(p)
            }
            pub(super) fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn put_get_list_delete_round_trip() {
        let (_d, mut s) = store();
        assert_eq!(s.get(Ns::Documents, "a").unwrap(), None);
        s.put(Ns::Documents, "a", b"one").unwrap();
        s.put(Ns::Documents, "a", b"two").unwrap();
        s.put(Ns::Autosave, "a", b"other namespace").unwrap();
        assert_eq!(s.get(Ns::Documents, "a").unwrap().as_deref(), Some(&b"two"[..]));
        let listed = s.list(Ns::Documents).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].name.as_str(), listed[0].size), ("a", 3));
        s.delete(Ns::Documents, "a").unwrap();
        s.delete(Ns::Documents, "a").unwrap();
        assert_eq!(s.get(Ns::Documents, "a").unwrap(), None);
        assert_eq!(s.get(Ns::Autosave, "a").unwrap().as_deref(), Some(&b"other namespace"[..]));
    }

    #[test]
    fn any_key_is_a_safe_file_name_and_comes_back_unchanged() {
        let (_d, mut s) = store();
        for key in ["../../etc/passwd", "a/b", "tab:1", "Ünï cödé", "", ".hidden", "x y"] {
            s.put(Ns::Library, key, b"x").unwrap();
            assert_eq!(s.get(Ns::Library, key).unwrap().as_deref(), Some(&b"x"[..]), "{key:?}");
        }
        let names: Vec<String> = s.list(Ns::Library).unwrap().into_iter().map(|i| i.name).collect();
        for key in ["../../etc/passwd", "a/b", "tab:1", "Ünï cödé", "", ".hidden", "x y"] {
            assert!(names.iter().any(|n| n == key), "{key:?} in {names:?}");
        }
        // Nothing escaped the root.
        assert!(!std::env::temp_dir().join("etc").exists());
    }

    #[test]
    fn a_leftover_temp_file_is_not_listed() {
        let (d, mut s) = store();
        s.put(Ns::Settings, "k", b"v").unwrap();
        std::fs::write(d.path().join("settings").join(".tmp-k"), b"half").unwrap();
        assert_eq!(s.list(Ns::Settings).unwrap().len(), 1);
    }

    #[test]
    fn a_later_write_is_there_for_the_next_read_and_the_last_write_wins() {
        let (_d, mut s) = store();
        for n in 0..20u8 {
            s.put_later(Ns::Autosave, "doc", vec![n; 1000]).unwrap();
        }
        s.put_later(Ns::Autosave, "index", b"last".to_vec()).unwrap();
        assert_eq!(s.get(Ns::Autosave, "doc").unwrap(), Some(vec![19; 1000]));
        assert_eq!(s.get(Ns::Autosave, "index").unwrap().as_deref(), Some(&b"last"[..]));
        assert_eq!(s.list(Ns::Autosave).unwrap().len(), 2);
        // A delete after a write is carried out after it.
        s.put_later(Ns::Autosave, "gone", b"x".to_vec()).unwrap();
        s.delete(Ns::Autosave, "gone").unwrap();
        assert_eq!(s.get(Ns::Autosave, "gone").unwrap(), None);
        assert_eq!(s.take_write_error(), None);
    }

    #[test]
    fn the_writes_are_finished_when_the_last_clone_goes() {
        let (d, s) = store();
        {
            let mut a = s.clone();
            a.put_later(Ns::Autosave, "doc", vec![7; 100_000]).unwrap();
            drop(a);
            drop(s);
        }
        let again = NativeStore::new(d.path().to_owned());
        assert_eq!(again.get(Ns::Autosave, "doc").unwrap().map(|b| b.len()), Some(100_000));
    }

    #[test]
    fn a_failed_later_write_is_reported_once() {
        let (d, mut s) = store();
        // A file where the folder of the namespace should be.
        std::fs::create_dir_all(d.path()).unwrap();
        std::fs::write(d.path().join("autosave"), b"in the way").unwrap();
        s.put_later(Ns::Autosave, "doc", b"x".to_vec()).unwrap();
        s.flush();
        let e = s.take_write_error().expect("the failure comes back");
        assert!(matches!(e, PortError::Failed(_)));
        assert_eq!(s.take_write_error(), None, "once");
    }
}
