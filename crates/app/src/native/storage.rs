//! Storage on disk: one folder per namespace, one file per key.
//!
//! Keys are written percent-encoded, so any key makes a valid file name on every platform. A write goes to a
//! temporary file in the same folder and is renamed over the target, so a crash leaves the old content or the
//! new, never half of it.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use drafftink_engine::PortError;
use drafftink_engine::ports::{Ns, Storage, StoredInfo};

/// The files of the app under one root folder. Cheap to clone.
#[derive(Debug, Clone)]
pub struct NativeStore {
    root: PathBuf,
}

impl NativeStore {
    /// A store rooted at `root`. Folders are created on the first write.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
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
        let path = self.path(ns, key);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(failed("could not read", &path, &e)),
        }
    }

    fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError> {
        let dir = self.dir(ns);
        fs::create_dir_all(&dir).map_err(|e| failed("could not create the folder", &dir, &e))?;
        let target = dir.join(encode_key(key));
        let temp = dir.join(format!(".tmp-{}", encode_key(key)));
        let mut file = fs::File::create(&temp).map_err(|e| failed("could not write", &temp, &e))?;
        file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| failed("could not write", &temp, &e))?;
        drop(file);
        fs::rename(&temp, &target).map_err(|e| failed("could not replace", &target, &e))
    }

    fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError> {
        let path = self.path(ns, key);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(failed("could not delete", &path, &e)),
        }
    }

    fn list(&self, ns: Ns) -> Result<Vec<StoredInfo>, PortError> {
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
}
