//! The real ports of the headless server: the system clock, random ids, and files on disk inside folders
//! the user allowed. There is no clipboard, storage or renderer: a command that needs one refuses with
//! `port_unavailable` and says what to do instead (for PNG: use SVG, or attach to the desktop app).

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use drafftink_engine::Session;
use drafftink_engine::ports::{Clock, FileRef, Files, PortError, Ports};
use drafftink_scene::{RandomIds, Rng64};

/// Largest file the server reads or writes.
const MAX_FILE_BYTES: u64 = 256 << 20;

/// Wall time and monotonic time from the operating system.
#[derive(Debug)]
struct SystemClock {
    started: Instant,
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    fn mono_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// 64 random bits from the operating system. Falls back to the clock if the system has none, which only
/// weakens id uniqueness across processes, not safety.
#[derive(Debug)]
struct OsRng;

impl Rng64 for OsRng {
    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        match getrandom::fill(&mut bytes) {
            Ok(()) => u64::from_le_bytes(bytes),
            Err(_) => {
                SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(0))
            }
        }
    }
}

/// Which folders the agent may read from and write to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileScope {
    /// Anywhere the user can (`--allow-any-path`).
    Anywhere,
    /// Inside these folders (canonical paths).
    Folders(Vec<PathBuf>),
}

impl FileScope {
    /// The folders that exist, made canonical. A folder that cannot be resolved is left out.
    pub fn folders(dirs: &[PathBuf]) -> Self {
        Self::Folders(dirs.iter().filter_map(|d| d.canonicalize().ok()).collect())
    }

    /// The folders, for messages.
    pub fn describe(&self) -> String {
        match self {
            Self::Anywhere => "anywhere".to_owned(),
            Self::Folders(f) if f.is_empty() => "nowhere".to_owned(),
            Self::Folders(f) => f.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
        }
    }

    fn outside(&self, shown: &str) -> PortError {
        PortError::Failed(format!(
            "`{shown}` is outside the folders this server may use ({}). Ask the user to start drafftink-mcp with --allow-dir <folder> for another folder",
            self.describe()
        ))
    }

    /// The canonical path to use for `path`: the file itself when it exists, else its folder plus the file
    /// name (a file about to be written). Fails when the result is not inside an allowed folder.
    fn resolve(&self, path: &str) -> Result<PathBuf, PortError> {
        let raw = Path::new(path);
        let full = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            std::env::current_dir().map_err(|e| PortError::Failed(e.to_string()))?.join(raw)
        };
        let resolved = match full.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                let name =
                    full.file_name().ok_or_else(|| PortError::Failed(format!("`{path}` does not name a file")))?;
                let parent = full.parent().ok_or_else(|| PortError::Failed(format!("`{path}` has no folder")))?;
                let parent = parent
                    .canonicalize()
                    .map_err(|_| PortError::NotFound(format!("the folder of `{path}` does not exist")))?;
                parent.join(name)
            }
        };
        match self {
            Self::Anywhere => Ok(resolved),
            Self::Folders(folders) if folders.iter().any(|f| resolved.starts_with(f)) => Ok(resolved),
            Self::Folders(_) => Err(self.outside(path)),
        }
    }
}

/// Files on disk, restricted by a [`FileScope`]. Only `FileRef::Path` is served; a download or a stored name
/// belongs to the browser.
#[derive(Debug)]
struct DiskFiles {
    scope: FileScope,
}

impl DiskFiles {
    fn path_of(&self, file: &FileRef) -> Result<PathBuf, PortError> {
        match file {
            FileRef::Path { path } => self.scope.resolve(path),
            _ => Err(PortError::Unsupported(
                "this server writes files by `path` only. Give {\"path\": \"...\"}".to_owned(),
            )),
        }
    }
}

impl Files for DiskFiles {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError> {
        let path = self.path_of(file)?;
        let meta = std::fs::metadata(&path).map_err(|e| PortError::NotFound(format!("{}: {e}", path.display())))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(PortError::Failed(format!("{} is larger than {} MiB", path.display(), MAX_FILE_BYTES >> 20)));
        }
        std::fs::read(&path).map_err(|e| PortError::Failed(format!("{}: {e}", path.display())))
    }

    fn exists(&self, file: &FileRef) -> bool {
        self.path_of(file).is_ok_and(|p| p.exists())
    }

    fn write(&mut self, file: &FileRef, bytes: &[u8], _mime: &str) -> Result<FileRef, PortError> {
        let path = self.path_of(file)?;
        std::fs::write(&path, bytes).map_err(|e| PortError::Failed(format!("{}: {e}", path.display())))?;
        Ok(FileRef::Path { path: path.display().to_string() })
    }
}

/// The ports of a headless session: real clock, random ids, files inside `scope`.
pub fn headless_ports(scope: FileScope) -> Ports {
    Ports::new(Box::new(SystemClock { started: Instant::now() }), Box::new(RandomIds(OsRng)))
        .with_files(Box::new(DiskFiles { scope }))
}

/// A headless session: the real text engine and [`headless_ports`].
pub fn headless_session(scope: FileScope) -> Session {
    Session::new(headless_ports(scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_outside_the_allowed_folders_are_refused_with_the_way_out() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut files = DiskFiles { scope: FileScope::folders(&[dir.path().to_owned()]) };
        let inside = FileRef::Path { path: dir.path().join("a.json").display().to_string() };
        files.write(&inside, b"{}", "application/json").unwrap();
        assert_eq!(files.read(&inside).unwrap(), b"{}");
        for bad in [other.path().join("x.json"), dir.path().join("../escape.json"), PathBuf::from("/etc/passwd")] {
            let r = FileRef::Path { path: bad.display().to_string() };
            let e = files.write(&r, b"x", "text/plain").unwrap_err().to_string();
            assert!(e.contains("--allow-dir"), "{bad:?}: {e}");
            assert!(files.read(&r).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_folder_does_not_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let files = DiskFiles { scope: FileScope::folders(&[dir.path().to_owned()]) };
        let r = FileRef::Path { path: dir.path().join("link/secret.txt").display().to_string() };
        assert!(files.read(&r).is_err());
    }

    #[test]
    fn only_paths_are_served() {
        let files = DiskFiles { scope: FileScope::Anywhere };
        assert!(files.read(&FileRef::Download { download: "x".into() }).is_err());
    }
}
