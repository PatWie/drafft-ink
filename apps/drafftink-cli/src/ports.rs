//! The ports of a command line session: the system clock, random ids, and files on disk.
//!
//! There is no clipboard, storage or renderer. A command that needs one refuses with `port_unavailable` and says
//! what to do instead. Files are not restricted to a folder, because the person who runs the tool is the one who
//! chose the paths (`drafftink-mcp` restricts them, because an agent chooses there).

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use drafftink_engine::Session;
use drafftink_engine::ports::{Clock, FileRef, Files, PortError, Ports, WriteOutcome};
use drafftink_scene::{IdSource, RandomIds, Rng64, SplitMix64};

/// Largest file that is read or written.
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

/// 64 random bits from the operating system. When the system has none, the clock stands in, which weakens only
/// the uniqueness of ids across processes.
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

/// Files on disk. Only `FileRef::Path` is served: a download or a stored name belongs to the browser.
#[derive(Debug)]
struct DiskFiles;

impl DiskFiles {
    fn path_of(file: &FileRef) -> Result<PathBuf, PortError> {
        match file {
            FileRef::Path { path } => Ok(PathBuf::from(path)),
            _ => Err(PortError::Unsupported(
                "this tool reads and writes files by `path` only. Give {\"path\": \"...\"}".to_owned(),
            )),
        }
    }
}

impl Files for DiskFiles {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError> {
        let path = Self::path_of(file)?;
        let meta = std::fs::metadata(&path).map_err(|e| PortError::NotFound(format!("{}: {e}", path.display())))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(PortError::Failed(format!("{} is larger than {} MiB", path.display(), MAX_FILE_BYTES >> 20)));
        }
        std::fs::read(&path).map_err(|e| PortError::Failed(format!("{}: {e}", path.display())))
    }

    fn exists(&self, file: &FileRef) -> bool {
        Self::path_of(file).is_ok_and(|p| p.exists())
    }

    fn write(&mut self, file: &FileRef, bytes: &[u8], _mime: &str) -> Result<WriteOutcome, PortError> {
        let path = Self::path_of(file)?;
        std::fs::write(&path, bytes).map_err(|e| PortError::Failed(format!("{}: {e}", path.display())))?;
        Ok(WriteOutcome::Done(FileRef::Path { path: path.display().to_string() }))
    }
}

/// Where element ids and hand-drawn seeds come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdSeed {
    /// The operating system: ids differ on every run.
    #[default]
    Random,
    /// A fixed seed: the same script makes the same ids and the same jitter on every run, so a generated file can be
    /// checked in and compared.
    Fixed(u64),
}

/// The ports of a command line session.
pub fn ports(seed: IdSeed) -> Ports {
    let ids: Box<dyn IdSource> = match seed {
        IdSeed::Random => Box::new(RandomIds(OsRng)),
        IdSeed::Fixed(seed) => Box::new(RandomIds(SplitMix64::new(seed))),
    };
    Ports::new(Box::new(SystemClock { started: Instant::now() }), ids).with_files(Box::new(DiskFiles))
}

/// A session with the real text engine and [`ports`].
pub fn session(seed: IdSeed) -> Session {
    Session::new(ports(seed))
}
