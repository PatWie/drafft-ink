//! Windows only: embed the application icon and the version resource into `drafftink.exe`.
//!
//! The resource script is written to `OUT_DIR` so that the file version follows `CARGO_PKG_VERSION`.
//! On other targets this script does nothing. The check uses the *target* (`CARGO_CFG_TARGET_OS`), not the
//! host, so a cross build from Linux also embeds the resource when a resource compiler is installed.

use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// `1.2.3-rc.1` becomes `[1, 2, 3, 0]`: the numeric file version of a Windows version resource.
fn numeric_version(version: &str) -> [u16; 4] {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    [parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0), 0]
}

/// The `.rc` text: icon plus VERSIONINFO. Paths use forward slashes, which `rc` and `windres` both accept.
fn resource_script(icon: &Path, version: &str) -> Result<String, std::fmt::Error> {
    let [a, b, c, d] = numeric_version(version);
    let icon = icon.display().to_string().replace('\\', "/");
    let mut rc = String::new();
    writeln!(rc, "1 ICON \"{icon}\"")?;
    writeln!(rc, "1 VERSIONINFO")?;
    writeln!(rc, "FILEVERSION {a},{b},{c},{d}")?;
    writeln!(rc, "PRODUCTVERSION {a},{b},{c},{d}")?;
    writeln!(rc, "BEGIN")?;
    writeln!(rc, "  BLOCK \"StringFileInfo\"")?;
    writeln!(rc, "  BEGIN")?;
    writeln!(rc, "    BLOCK \"040904b0\"")?;
    writeln!(rc, "    BEGIN")?;
    writeln!(rc, "      VALUE \"ProductName\", \"Drafft.ink\\0\"")?;
    writeln!(rc, "      VALUE \"FileDescription\", \"Drafft.ink\\0\"")?;
    writeln!(rc, "      VALUE \"OriginalFilename\", \"drafftink.exe\\0\"")?;
    writeln!(rc, "      VALUE \"LegalCopyright\", \"AGPL-3.0-only\\0\"")?;
    writeln!(rc, "      VALUE \"FileVersion\", \"{version}\\0\"")?;
    writeln!(rc, "      VALUE \"ProductVersion\", \"{version}\\0\"")?;
    writeln!(rc, "    END")?;
    writeln!(rc, "  END")?;
    writeln!(rc, "  BLOCK \"VarFileInfo\"")?;
    writeln!(rc, "  BEGIN")?;
    writeln!(rc, "    VALUE \"Translation\", 0x0409, 1200")?;
    writeln!(rc, "  END")?;
    writeln!(rc, "END")?;
    Ok(rc)
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../packaging/icons/drafftink.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let icon = manifest_dir.join("../../packaging/icons/drafftink.ico");
    let version = env::var("CARGO_PKG_VERSION")?;
    let rc = PathBuf::from(env::var("OUT_DIR")?).join("drafftink.rc");
    fs::write(&rc, resource_script(&icon, &version)?)?;
    embed_resource::compile(&rc, embed_resource::NONE).manifest_required()?;
    Ok(())
}
