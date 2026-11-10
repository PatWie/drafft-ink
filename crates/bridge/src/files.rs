//! The per-launch token and where it lives (native only).
//!
//! The token is 32 bytes from the operating system's random generator, written as 64 hex characters to
//! `<dir>/<port>.token`. The directory is private to the user (mode 0700) and the file is created new with
//! mode 0600, so another user on the machine cannot read it. `<dir>/current.port` names the newest port
//! so that a client can find the app without arguments.
//!
//! Directory: `$DRAFFTINK_BRIDGE_DIR`, else `$XDG_RUNTIME_DIR/drafftink`, else
//! `~/.local/state/drafftink/run` (Linux), `~/Library/Application Support/drafftink/run` (macOS),
//! `%LOCALAPPDATA%\drafftink\run` (Windows). On Windows the per-user profile directory already restricts
//! access to the user; the permission bits below are Unix only.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::server::BridgeError;

/// A new random token: 32 bytes as 64 hex characters.
pub fn new_token() -> Result<String, BridgeError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| BridgeError::Random(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The directory for token files, or `None` when the environment names no home.
pub fn default_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = env("DRAFFTINK_BRIDGE_DIR") {
        return Some(d);
    }
    if cfg!(windows) {
        return env("LOCALAPPDATA").map(|d| d.join("drafftink").join("run"));
    }
    if let Some(d) = env("XDG_RUNTIME_DIR") {
        return Some(d.join("drafftink"));
    }
    let home = env("HOME")?;
    if cfg!(target_os = "macos") {
        Some(home.join("Library").join("Application Support").join("drafftink").join("run"))
    } else {
        Some(home.join(".local").join("state").join("drafftink").join("run"))
    }
}

#[cfg(unix)]
fn private_mode(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)?.permissions().mode() & 0o077 == 0)
}

/// Creates the directory (mode 0700 on Unix) or checks an existing one. A symlink or a file in its place is
/// refused, and a directory that other users can enter is tightened to 0700.
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(md) if md.file_type().is_symlink() => {
            return Err(io::Error::other(format!("{} is a symbolic link, refusing to put a token there", dir.display())));
        }
        Ok(md) if !md.is_dir() => {
            return Err(io::Error::other(format!("{} exists and is not a directory", dir.display())));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new().mode(0o700).create(dir)?;
            }
            #[cfg(not(unix))]
            fs::create_dir(dir)?;
        }
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !private_mode(dir)? {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        if !private_mode(dir)? {
            return Err(io::Error::other(format!("{} is not private to this user and could not be fixed", dir.display())));
        }
    }
    Ok(())
}

fn create_private_file(path: &Path, text: &str) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// The token file of a running server. Dropping it removes the files if they still hold this launch's token.
#[derive(Debug)]
pub struct TokenFile {
    token_path: PathBuf,
    port_path: PathBuf,
    token: String,
    port: u16,
}

impl TokenFile {
    /// Writes `<dir>/<port>.token` and `<dir>/current.port`, both private to the user.
    pub fn write(dir: &Path, port: u16, token: &str) -> io::Result<Self> {
        prepare_dir(dir)?;
        let token_path = dir.join(format!("{port}.token"));
        let port_path = dir.join("current.port");
        create_private_file(&token_path, &format!("{token}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // A directory owned by someone else would let that user read the token. Our own new file shows our uid.
            if fs::metadata(dir)?.uid() != fs::metadata(&token_path)?.uid() {
                let _ = fs::remove_file(&token_path);
                return Err(io::Error::other(format!("{} belongs to another user", dir.display())));
            }
        }
        create_private_file(&port_path, &format!("{port}\n"))?;
        Ok(Self { token_path, port_path, token: token.to_owned(), port })
    }

    /// Where the token is.
    pub fn path(&self) -> &Path {
        &self.token_path
    }
}

impl Drop for TokenFile {
    fn drop(&mut self) {
        let mine = |p: &Path, want: &str| fs::read_to_string(p).is_ok_and(|t| t.trim() == want);
        if mine(&self.token_path, &self.token) {
            let _ = fs::remove_file(&self.token_path);
        }
        if mine(&self.port_path, &self.port.to_string()) {
            let _ = fs::remove_file(&self.port_path);
        }
    }
}

/// A running app found through its token file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The port.
    pub port: u16,
    /// The token.
    pub token: String,
    /// The file the token came from.
    pub token_file: PathBuf,
}

/// Why a token file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    /// There is no file. The app is probably not running.
    #[error("no running drafft.ink app found: {0} does not exist. Start the desktop app, or pass --port and --token-file")]
    Missing(PathBuf),
    /// The file can be read by other users.
    #[error("{0} can be read by other users, so it is not trusted. Run `chmod 600` on it, or restart the app")]
    NotPrivate(PathBuf),
    /// The file has the wrong content.
    #[error("{0} does not hold a valid {1}")]
    Invalid(PathBuf, &'static str),
    /// Reading failed.
    #[error("could not read {0}: {1}")]
    Io(PathBuf, io::Error),
}

fn read_private(path: &Path) -> Result<String, FindError> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(FindError::Missing(path.to_owned())),
        Err(e) => return Err(FindError::Io(path.to_owned(), e)),
    };
    #[cfg(unix)]
    if !private_mode(path).map_err(|e| FindError::Io(path.to_owned(), e))? {
        return Err(FindError::NotPrivate(path.to_owned()));
    }
    Ok(text.trim().to_owned())
}

/// Finds the app: the given port, else the one in `current.port`; then its token file.
pub fn find(dir: &Path, port: Option<u16>) -> Result<Found, FindError> {
    let port = match port {
        Some(p) => p,
        None => {
            let path = dir.join("current.port");
            read_private(&path)?.parse().map_err(|_| FindError::Invalid(path, "port number"))?
        }
    };
    let token_file = dir.join(format!("{port}.token"));
    let token = read_private(&token_file)?;
    if crate::guard::token_problem(&token).is_some() {
        return Err(FindError::Invalid(token_file, "token"));
    }
    Ok(Found { port, token, token_file })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn token_is_64_hex_and_different_each_time() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn files_are_private_and_removed_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run");
        let token = new_token().unwrap();
        let f = TokenFile::write(&dir, 4242, &token).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(f.path()).unwrap().permissions().mode() & 0o777, 0o600);
        let found = find(&dir, None).unwrap();
        assert_eq!((found.port, found.token.as_str()), (4242, token.as_str()));
        drop(f);
        assert!(!dir.join("4242.token").exists());
        assert!(matches!(find(&dir, None), Err(FindError::Missing(_))));
    }

    #[test]
    fn a_loose_directory_is_tightened_and_a_loose_file_is_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let token = new_token().unwrap();
        let f = TokenFile::write(&dir, 1, &token).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        fs::set_permissions(f.path(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(find(&dir, Some(1)), Err(FindError::NotPrivate(_))));
    }

    #[test]
    fn a_symlinked_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(TokenFile::write(&link, 1, &new_token().unwrap()).is_err());
    }

    #[test]
    fn a_newer_launch_keeps_its_files_when_an_older_one_exits() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let old = TokenFile::write(&dir, 7, &new_token().unwrap()).unwrap();
        let newer = TokenFile::write(&dir, 7, &new_token().unwrap()).unwrap();
        drop(old);
        assert!(newer.path().exists());
    }
}
