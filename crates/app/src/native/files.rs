//! Files by path, and "downloads" (a name the app saves into the downloads folder).

use std::fs;
use std::path::{Component, Path, PathBuf};

use drafftink_engine::PortError;
use drafftink_engine::ports::{FileRef, Files};

/// Reads and writes the files the user names.
pub(crate) struct NativeFiles {
    downloads: PathBuf,
}

impl NativeFiles {
    pub(crate) fn new(downloads: PathBuf) -> Self {
        Self { downloads }
    }

    /// `~/Downloads` when it exists, else the app's own `downloads` folder.
    fn downloads_dir(&self) -> PathBuf {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Downloads"))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| self.downloads.clone())
    }

    fn target(&self, file: &FileRef) -> Result<PathBuf, PortError> {
        match file {
            FileRef::Path { path } => Ok(PathBuf::from(path)),
            FileRef::Download { download } => {
                // A download is a bare file name. A path in it would escape the folder.
                let name = Path::new(download);
                let plain = name.components().count() == 1 && matches!(name.components().next(), Some(Component::Normal(_)));
                if !plain {
                    return Err(PortError::Failed(format!(
                        "`{download}` is not a plain file name. Use a name without folders, or give a path"
                    )));
                }
                Ok(self.downloads_dir().join(name))
            }
            FileRef::Stored { .. } => Err(PortError::Unsupported("stored documents are served by the storage port".into())),
            FileRef::Handle { .. } => Err(PortError::Unsupported("file handles exist only in the browser".into())),
        }
    }
}

impl Files for NativeFiles {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError> {
        let path = self.target(file)?;
        fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PortError::NotFound(path.display().to_string()),
            _ => PortError::Failed(format!("{}: {e}", path.display())),
        })
    }

    fn exists(&self, file: &FileRef) -> bool {
        self.target(file).is_ok_and(|p| p.exists())
    }

    fn write(&mut self, file: &FileRef, bytes: &[u8], _mime: &str) -> Result<FileRef, PortError> {
        let path = self.target(file)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| PortError::Failed(format!("{}: {e}", parent.display())))?;
        }
        fs::write(&path, bytes).map_err(|e| PortError::Failed(format!("{}: {e}", path.display())))?;
        Ok(match file {
            FileRef::Download { .. } => FileRef::Path { path: path.display().to_string() },
            other => other.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_name_cannot_leave_the_folder() {
        let f = NativeFiles::new(std::env::temp_dir().join("drafftink-dl-test"));
        for bad in ["../x", "a/b", "/etc/passwd", "..", "."] {
            assert!(f.read(&FileRef::Download { download: bad.into() }).is_err(), "{bad}");
        }
    }

    #[test]
    fn paths_round_trip() {
        let dir = std::env::temp_dir().join(format!("drafftink-files-test-{}", std::process::id()));
        let mut f = NativeFiles::new(dir.clone());
        let r = FileRef::Path { path: dir.join("sub").join("a.txt").display().to_string() };
        assert!(!f.exists(&r));
        f.write(&r, b"hi", "text/plain").unwrap();
        assert!(f.exists(&r));
        assert_eq!(f.read(&r).unwrap(), b"hi");
        let _ = fs::remove_dir_all(dir);
    }
}
