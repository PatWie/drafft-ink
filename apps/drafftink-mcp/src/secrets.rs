//! The service token of a private server for a program without a screen: read from the environment, never stored.
//!
//! `DRAFFTINK_ACCESS_CLIENT_ID` holds the client id and `DRAFFTINK_ACCESS_CLIENT_SECRET_FILE` the path of a file that
//! holds the client secret. No flag and no variable takes the secret itself: arguments and environment variables are
//! visible to the other users of the machine, a file has permissions. The token is the program's *ambient* token. The
//! session sends it to the server of the setting `collab.server_url` and to no other (`docs/collab.md`, "Access service
//! token"). It cannot be changed with `collab.set_access`: that refuses and says where the token comes from.

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use drafftink_engine::PortError;
use drafftink_engine::ports::{AccessCredentials, AccessError, AccessOrigin, Secrets};

/// The variable that holds the client id.
pub const ID_VARIABLE: &str = "DRAFFTINK_ACCESS_CLIENT_ID";
/// The variable that holds the path of the file with the client secret.
pub const SECRET_FILE_VARIABLE: &str = "DRAFFTINK_ACCESS_CLIENT_SECRET_FILE";

/// The most bytes read from the secret file. A secret is under 512 bytes.
const MAX_SECRET_FILE_BYTES: u64 = 4096;

/// Why the environment holds no usable token. The text says what to change and never holds the secret.
#[derive(Debug, thiserror::Error)]
pub enum EnvSecretsError {
    /// One of the two variables is set and the other is not.
    #[error("set both {ID_VARIABLE} and {SECRET_FILE_VARIABLE}, or neither")]
    OnlyOne,
    /// The secret file cannot be read.
    #[error("{SECRET_FILE_VARIABLE} names {}, which cannot be read: {reason}", .path.display())]
    Unreadable {
        /// The path.
        path: PathBuf,
        /// What the system said.
        reason: String,
    },
    /// The values do not make a token.
    #[error("the access token of {ID_VARIABLE} and {SECRET_FILE_VARIABLE} is not usable: {0}")]
    Invalid(#[from] AccessError),
}

/// The ambient token of the program. `Debug` prints a placeholder.
pub struct EnvSecrets {
    credentials: AccessCredentials,
}

impl fmt::Debug for EnvSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnvSecrets(<hidden>)")
    }
}

impl EnvSecrets {
    /// Reads the two variables. `Ok(None)` when neither is set.
    pub fn from_env() -> Result<Option<Self>, EnvSecretsError> {
        let id = std::env::var(ID_VARIABLE).ok().filter(|v| !v.trim().is_empty());
        let file = std::env::var_os(SECRET_FILE_VARIABLE).filter(|v| !v.is_empty()).map(PathBuf::from);
        Self::from_parts(id.as_deref(), file.as_deref())
    }

    /// The token made of a client id and the path of the file with the secret. `Ok(None)` when both are missing.
    pub fn from_parts(id: Option<&str>, secret_file: Option<&Path>) -> Result<Option<Self>, EnvSecretsError> {
        let (id, file) = match (id, secret_file) {
            (None, None) => return Ok(None),
            (Some(id), Some(file)) => (id, file),
            _ => return Err(EnvSecretsError::OnlyOne),
        };
        let secret = read_secret(file)?;
        Ok(Some(Self { credentials: AccessCredentials::new(id.trim(), secret.trim())? }))
    }
}

/// The text of the secret file.
fn read_secret(path: &Path) -> Result<String, EnvSecretsError> {
    let unreadable = |reason: String| EnvSecretsError::Unreadable { path: path.to_owned(), reason };
    let mut file = std::fs::File::open(path).map_err(|e| unreadable(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if file.metadata().is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
            log::warn!("{} is readable by other users. Run `chmod 600` on it.", path.display());
        }
    }
    let mut text = String::new();
    file.by_ref().take(MAX_SECRET_FILE_BYTES + 1).read_to_string(&mut text).map_err(|e| unreadable(e.to_string()))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_SECRET_FILE_BYTES {
        return Err(unreadable("it is larger than a token can be".to_owned()));
    }
    Ok(text)
}

impl Secrets for EnvSecrets {
    fn get(&self, _origin: &AccessOrigin) -> Result<Option<AccessCredentials>, PortError> {
        Ok(None)
    }

    fn put(&mut self, _origin: &AccessOrigin, _credentials: AccessCredentials) -> Result<(), PortError> {
        Err(read_only())
    }

    fn delete(&mut self, _origin: &AccessOrigin) -> Result<bool, PortError> {
        Err(read_only())
    }

    fn ambient(&self) -> Option<AccessCredentials> {
        Some(self.credentials.clone())
    }
}

fn read_only() -> PortError {
    PortError::Unsupported(format!(
        "this program takes its access token from {ID_VARIABLE} and {SECRET_FILE_VARIABLE} and stores none"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn the_token_is_the_id_and_the_content_of_the_file_without_the_line_break() {
        let (_dir, path) = secret_file("the-secret-value\n");
        let env = EnvSecrets::from_parts(Some(" the-id.access "), Some(&path)).unwrap().unwrap();
        let token = env.ambient().unwrap();
        assert_eq!((token.client_id(), token.expose_secret()), ("the-id.access", "the-secret-value"));
        let origin = AccessOrigin::parse("https://canvas.example.com").unwrap();
        assert_eq!(env.get(&origin).unwrap(), None, "nothing is stored for an origin");
    }

    #[test]
    fn neither_variable_means_no_token_and_one_alone_is_a_mistake() {
        assert!(EnvSecrets::from_parts(None, None).unwrap().is_none());
        let (_dir, path) = secret_file("s");
        for (id, file) in [(Some("id"), None), (None, Some(path.as_path()))] {
            let e = EnvSecrets::from_parts(id, file).unwrap_err();
            assert!(matches!(e, EnvSecretsError::OnlyOne), "{e}");
            assert!(e.to_string().contains(ID_VARIABLE) && e.to_string().contains(SECRET_FILE_VARIABLE));
        }
    }

    #[test]
    fn a_file_that_cannot_be_used_is_refused_without_showing_what_it_holds() {
        let missing = Path::new("/nonexistent/folder/secret");
        let e = EnvSecrets::from_parts(Some("id"), Some(missing)).unwrap_err();
        assert!(
            matches!(e, EnvSecretsError::Unreadable { .. }) && e.to_string().contains("/nonexistent/folder/secret")
        );
        for (text, why) in
            [("", "empty"), ("two words", "inner space"), ("x\ny", "line break inside"), ("é", "not ASCII")]
        {
            let (_dir, path) = secret_file(text);
            let e = EnvSecrets::from_parts(Some("id"), Some(&path)).unwrap_err();
            assert!(matches!(e, EnvSecretsError::Invalid(_)), "{why}: {e}");
            assert!(!e.to_string().contains(text.trim()) || text.trim().is_empty(), "{why}: {e}");
        }
        let (_dir, big) = secret_file(&"a".repeat(5000));
        assert!(EnvSecrets::from_parts(Some("id"), Some(&big)).is_err());
    }

    #[test]
    fn the_token_cannot_be_stored_or_removed_and_debug_hides_it() {
        let (_dir, path) = secret_file("visible-secret-value");
        let mut env = EnvSecrets::from_parts(Some("visible-id"), Some(&path)).unwrap().unwrap();
        let origin = AccessOrigin::parse("https://canvas.example.com").unwrap();
        let credentials = AccessCredentials::new("a", "b").unwrap();
        let e = env.put(&origin, credentials).unwrap_err().to_string();
        assert!(e.contains(ID_VARIABLE) && !e.contains("visible"), "{e}");
        assert!(env.delete(&origin).is_err());
        let shown = format!("{env:?}");
        assert!(!shown.contains("visible"), "{shown}");
    }
}
