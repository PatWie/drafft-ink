//! The service tokens of private servers on the desktop: one file in the app's data folder that only its user can read
//! (`docs/design/collab-e2e.md` section 19.6, `docs/collab.md`, "Access service token").
//!
//! * The file is `<data folder>/secrets/access.json` in a folder of mode `0700`. It is created with mode `0600` from the
//!   first byte (no moment at which it is readable by others) as a temporary file in the same folder and renamed over
//!   the old one, so a crash leaves the old content or the new, never half of it.
//! * On Windows the file takes the permissions of the user's own application folder. No extra access list is set.
//! * The operating system's credential store (the `keyring` crate) is a later option. It pulls in D-Bus on Linux, which
//!   a self-contained app should not need for one small secret.
//! * The file is read once per session and every change is written through. Another program that changes the file
//!   while the app runs is not noticed until the next start.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use drafftink_engine::PortError;
use drafftink_engine::ports::{AccessCredentials, AccessOrigin, Secrets};
use serde::{Deserialize, Serialize};

/// The folder of the file, under the data folder.
const FOLDER: &str = "secrets";
/// The file.
const FILE: &str = "access.json";
/// The version of the file's layout.
const VERSION: u32 = 1;

/// One token as it is written.
#[derive(Serialize, Deserialize)]
struct Entry {
    client_id: String,
    client_secret: String,
}

/// The file's content.
#[derive(Serialize, Deserialize)]
struct Content {
    version: u32,
    /// By origin, as `AccessOrigin` prints it.
    tokens: BTreeMap<String, Entry>,
}

type Tokens = BTreeMap<AccessOrigin, AccessCredentials>;

/// The [`Secrets`] port of the desktop app. Cheap to build: nothing is read until the first call.
#[derive(Debug)]
pub(crate) struct NativeSecrets {
    path: PathBuf,
    loaded: Mutex<Option<Tokens>>,
}

impl NativeSecrets {
    /// The store of the data folder `root`. Nothing is created yet.
    pub(crate) fn new(root: &Path) -> Self {
        Self { path: root.join(FOLDER).join(FILE), loaded: Mutex::new(None) }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Tokens>> {
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn failed(&self, what: &str, e: &dyn std::fmt::Display) -> PortError {
        PortError::Failed(format!("{what} {}: {e}", self.path.display()))
    }

    /// The error for a file that cannot be used as it is. It says what to do.
    fn unusable(&self, why: &dyn std::fmt::Display) -> PortError {
        PortError::Failed(format!(
            "the file of access tokens, {}, cannot be used ({why}). Remove it and enter the tokens again",
            self.path.display()
        ))
    }

    /// Reads the file. A missing file holds no tokens.
    fn read(&self) -> Result<Tokens, PortError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Tokens::new()),
            Err(e) => return Err(self.failed("cannot read", &e)),
        };
        tighten(&self.path);
        let content: Content =
            serde_json::from_str(&text).map_err(|e| self.unusable(&format!("it is damaged: {e}")))?;
        if content.version != VERSION {
            return Err(self.unusable(&format!("it was written by another version of the app: {}", content.version)));
        }
        let mut tokens = Tokens::new();
        for (origin, entry) in content.tokens {
            // An entry that is no longer valid is left out, not an error for the others.
            if let (Ok(origin), Ok(credentials)) =
                (AccessOrigin::parse(&origin), AccessCredentials::new(entry.client_id, entry.client_secret))
            {
                tokens.insert(origin, credentials);
            }
        }
        Ok(tokens)
    }

    /// Writes `tokens` as the new content of the file.
    fn write(&self, tokens: &Tokens) -> Result<(), PortError> {
        let content = Content {
            version: VERSION,
            tokens: tokens
                .iter()
                .map(|(origin, c)| {
                    let entry =
                        Entry { client_id: c.client_id().to_owned(), client_secret: c.expose_secret().to_owned() };
                    (origin.to_string(), entry)
                })
                .collect(),
        };
        let text = serde_json::to_string_pretty(&content).map_err(|e| self.failed("cannot write", &e))?;
        replace_private(&self.path, text.as_bytes()).map_err(|e| self.failed("cannot write", &e))
    }

    /// The tokens, read on first use. A file that cannot be read is an error each time (it is not cached), so a
    /// repaired file works again at once.
    fn with_tokens<T>(&self, f: impl FnOnce(&mut Tokens) -> T) -> Result<T, PortError> {
        let mut loaded = self.lock();
        if loaded.is_none() {
            *loaded = Some(self.read()?);
        }
        Ok(f(loaded.get_or_insert_with(Tokens::new)))
    }
}

impl Secrets for NativeSecrets {
    fn get(&self, origin: &AccessOrigin) -> Result<Option<AccessCredentials>, PortError> {
        self.with_tokens(|tokens| tokens.get(origin).cloned())
    }

    fn put(&mut self, origin: &AccessOrigin, credentials: AccessCredentials) -> Result<(), PortError> {
        // A damaged file is replaced: its tokens cannot be read anyway.
        let mut tokens = self.with_tokens(|t| t.clone()).unwrap_or_default();
        tokens.insert(origin.clone(), credentials);
        self.write(&tokens)?;
        *self.lock() = Some(tokens);
        Ok(())
    }

    fn delete(&mut self, origin: &AccessOrigin) -> Result<bool, PortError> {
        let mut tokens = self.with_tokens(|t| t.clone())?;
        if tokens.remove(origin).is_none() {
            return Ok(false);
        }
        self.write(&tokens)?;
        *self.lock() = Some(tokens);
        Ok(true)
    }
}

/// Writes `bytes` to `path` through a temporary file that is private from its first byte, then renames it over `path`.
fn replace_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let folder = path.parent().ok_or_else(|| io::Error::other("the file has no folder"))?;
    create_private_folder(folder)?;
    let temporary = folder.join(format!("{FILE}.tmp-{}", std::process::id()));
    // A leftover of a crashed run of this process id is not trusted.
    let _ = fs::remove_file(&temporary);
    let written = (|| {
        let mut file = private_options().open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(e);
    }
    Ok(())
}

/// Options for a new file that only the user can read: mode `0600` where there are modes.
fn private_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
}

/// Creates the folder (and its parents) with mode `0700` where there are modes.
fn create_private_folder(folder: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // A folder that existed already with looser rights is tightened: the file inside is a secret.
        fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Takes group and other rights from a file that has them. Best effort: the secret was readable until now, and
/// reading it again is no worse.
fn tighten(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(text: &str) -> AccessOrigin {
        AccessOrigin::parse(text).unwrap()
    }

    fn credentials(id: &str, secret: &str) -> AccessCredentials {
        AccessCredentials::new(id, secret).unwrap()
    }

    #[test]
    fn tokens_round_trip_through_the_file_and_survive_a_new_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = NativeSecrets::new(dir.path());
        assert_eq!(store.get(&origin("https://a.example.com")).unwrap(), None, "no file yet");
        store.put(&origin("https://a.example.com"), credentials("id-a", "secret-a")).unwrap();
        store.put(&origin("http://127.0.0.1:3030"), credentials("id-b", "secret-b")).unwrap();
        store.put(&origin("https://a.example.com"), credentials("id-a2", "secret-a2")).unwrap();
        let again = NativeSecrets::new(dir.path());
        assert_eq!(again.get(&origin("https://a.example.com")).unwrap(), Some(credentials("id-a2", "secret-a2")));
        assert_eq!(again.get(&origin("http://127.0.0.1:3030")).unwrap(), Some(credentials("id-b", "secret-b")));
        assert_eq!(again.get(&origin("https://b.example.com")).unwrap(), None);
        assert!(store.delete(&origin("http://127.0.0.1:3030")).unwrap());
        assert!(!store.delete(&origin("http://127.0.0.1:3030")).unwrap(), "removing a missing token says so");
        assert_eq!(NativeSecrets::new(dir.path()).get(&origin("http://127.0.0.1:3030")).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_and_its_folder_are_private_and_no_temporary_file_is_left() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let mut store = NativeSecrets::new(dir.path());
        store.put(&origin("https://a.example.com"), credentials("id", "secret")).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.path().join(FOLDER)), 0o700);
        assert_eq!(mode(&dir.path().join(FOLDER).join(FILE)), 0o600);
        let names: Vec<_> = fs::read_dir(dir.path().join(FOLDER)).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, [FILE]);
        // A second write keeps the mode.
        store.put(&origin("https://b.example.com"), credentials("id", "secret")).unwrap();
        assert_eq!(mode(&dir.path().join(FOLDER).join(FILE)), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_or_folder_that_others_could_read_is_tightened() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let mut store = NativeSecrets::new(dir.path());
        store.put(&origin("https://a.example.com"), credentials("id", "secret")).unwrap();
        let folder = dir.path().join(FOLDER);
        let file = folder.join(FILE);
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let fresh = NativeSecrets::new(dir.path());
        assert!(fresh.get(&origin("https://a.example.com")).unwrap().is_some());
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        let mut fresh = fresh;
        fresh.put(&origin("https://b.example.com"), credentials("id", "secret")).unwrap();
        assert_eq!(fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn a_damaged_file_is_an_error_for_reading_and_is_replaced_by_the_next_token() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(FOLDER)).unwrap();
        fs::write(dir.path().join(FOLDER).join(FILE), "{ not json").unwrap();
        let mut store = NativeSecrets::new(dir.path());
        let e = store.get(&origin("https://a.example.com")).unwrap_err().to_string();
        assert!(e.contains("damaged") && e.contains("access.json") && e.contains("Remove it"), "{e}");
        assert!(store.delete(&origin("https://a.example.com")).is_err());
        store.put(&origin("https://a.example.com"), credentials("id", "secret")).unwrap();
        assert_eq!(store.get(&origin("https://a.example.com")).unwrap(), Some(credentials("id", "secret")));
    }

    #[test]
    fn a_file_of_another_version_is_refused_and_a_bad_entry_does_not_hide_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FOLDER).join(FILE);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, r#"{"version":2,"tokens":{}}"#).unwrap();
        assert!(NativeSecrets::new(dir.path()).get(&origin("https://a.example.com")).is_err());
        fs::write(
            &file,
            r#"{"version":1,"tokens":{"nonsense":{"client_id":"x","client_secret":"y"},
                "https://a.example.com":{"client_id":"id","client_secret":"secret"},
                "https://b.example.com":{"client_id":"","client_secret":"y"}}}"#,
        )
        .unwrap();
        let store = NativeSecrets::new(dir.path());
        assert_eq!(store.get(&origin("https://a.example.com")).unwrap(), Some(credentials("id", "secret")));
        assert_eq!(store.get(&origin("https://b.example.com")).unwrap(), None);
    }

    #[test]
    fn debug_shows_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = NativeSecrets::new(dir.path());
        store.put(&origin("https://a.example.com"), credentials("id-visible-test", "secret-visible-test")).unwrap();
        let shown = format!("{store:?}");
        assert!(!shown.contains("secret-visible-test") && !shown.contains("id-visible-test"), "{shown}");
    }
}
