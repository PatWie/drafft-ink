# Packaging and releases

How Drafft.ink is built for every system and shipped. The design follows solidcraft's packaging (`packaging/`, `release.yml`), cut down to what a whiteboard needs. Everything here is plain shell or PowerShell that runs the same locally and in CI.

## What a release contains

`<v>` is the version in `[workspace.package]`, `<arch>` is `x86_64` or `aarch64`.

| File | Contents | Built by |
|---|---|---|
| `drafftink-<v>-linux-<arch>.AppImage` | desktop app + `drafftink-mcp`, one file, any distro with glibc 2.35 or newer | `scripts/package-linux.sh` |
| `drafftink-<v>-linux-<arch>.deb` | the same, as a Debian package | same |
| `drafftink-<v>-linux-<arch>.tar.gz` | the same as an FHS tree (`bin/`, `share/`) | same |
| `drafftink-<v>-macos-universal.dmg` | `Drafftink.app` (arm64 + x86_64 in one binary, `drafftink-mcp` inside it) | `scripts/package-macos.sh` |
| `drafftink-<v>-windows-x64.msi` | per-machine installer with a Start menu entry | `scripts/package-windows.ps1` |
| `drafftink-<v>-windows-x64.zip` | portable `drafftink.exe` + `drafftink-mcp.exe` | same |
| `drafftink-server-<v>-<os>-<arch>.tar.gz` (`.zip` on Windows) | `drafftink-server` + `web-dist/` (the web build) in one folder | the three scripts above |
| `drafftink-web-<v>.tar.gz`, `.zip` | the web build only, for any static host | `scripts/package-web.sh` |
| `SHA256SUMS.txt` | checksums of all of the above | `release.yml` |

Every versioned desktop file is also attached once more without the version (`drafftink-windows-x64.msi`, `drafftink-linux-x86_64.AppImage`, ...), so `https://github.com/<owner>/<repo>/releases/latest/download/<name>` works. Keep these names stable: links and any future update check depend on them.

**One binary.** The server does not embed the web build. It serves the folder named by `--web-dir` (default `./web-dist`), and the server archive ships that folder next to the binary, so "unpack and run `./drafftink-server`" serves the app on `http://127.0.0.1:3030`. Embedding the 17 MB wasm and its precompressed copies in the binary would make every server build depend on the wasm build and bloat it; revisit if single-file deployment becomes a requirement.

The desktop packages carry no server, and the server archives carry no desktop app. `drafftink-mcp` ships with the desktop app because an MCP client starts it (`drafftink-mcp --attach`) and the app is what it attaches to.

## Names that are fixed once chosen

- App id `ink.drafft.drafftink` (`packaging/env.sh`): the `.desktop` file name, the icon name, `StartupWMClass`, and the macOS bundle id. The window's Wayland `app_id` must equal it for the dock to find the icon. Check that the app sets it when the window code lands.
- Windows `UpgradeCode` `3b8f2c1e-6a47-4d52-9c1f-d4a7e0b95a31` in `packaging/windows/drafftink.wxs`. Changing it makes the MSI install next to the old version instead of replacing it.
- Document extension `.drafftink`, MIME type `application/x-drafftink`.

## Build requirements per system

| System | Build needs | Run needs |
|---|---|---|
| Linux | Rust 1.97.1 (`rust-toolchain.toml`) and a C compiler (`build-essential`). **No `-dev` packages**: winit loads X11, Wayland and xkbcommon with `dlopen`, SQLite is bundled, fonts are in the binary. Verified by a build with `PKG_CONFIG=/bin/false` and by a clean `ubuntu:22.04` container with only `build-essential curl ca-certificates`. | `libvulkan1` + a Vulkan driver, `libxkbcommon0`, X11 or Wayland libraries, `xdg-desktop-portal` for the file dialogs (the `.deb` depends on or recommends them). Details: `docs/building.md`. |
| macOS | Xcode command line tools, both Rust targets (`rustup target add aarch64-apple-darwin x86_64-apple-darwin`) | macOS 11+, Metal |
| Windows | MSVC build tools (`rc.exe` for the icon resource) | DirectX 12 |
| Web | `wasm32-unknown-unknown`, `wasm-bindgen-cli` 0.2.130 (equal to the crate pin), binaryen 117+ (`wasm-opt`), optional `brotli` | WebGPU browser |
| `.deb` | `dpkg-deb` | |
| AppImage | `appimagetool` (downloaded into `target/` on first use; run with extract-and-run, so FUSE is not needed to build) | FUSE 2 to run directly, or `--appimage-extract-and-run` |
| MSI | `dotnet tool install --global wix --version 5.0.2` | |

CI installs nothing on Linux for the workspace build (`ci.yml`, job `build-native`).

## Building locally

```sh
scripts/build-web.sh                         # dist/ (needs the web tools above); skip it and the server archive is skipped
scripts/package-linux.sh                     # dist-release/: tar.gz, deb, AppImage, server archive; then runs every binary with --version
scripts/package-linux.sh --skip-build --formats "tar deb"
scripts/package-web.sh                       # dist-release/drafftink-web-<v>.tar.gz and .zip
scripts/package-macos.sh [--sign [--notarize]]   # macOS only
pwsh scripts/package-windows.ps1 [-NoMsi]        # Windows only
```

All scripts read the version from `Cargo.toml`, write to `dist-release/` (override with `DIST=`), and end by running the packaged binaries with `--version`, which fails on a stub `main()`. `BIN_DIR=` packages binaries built elsewhere. `CROSS_TARGET=aarch64-unknown-linux-gnu CROSS_ARCH=aarch64` packages a cross build (the smoke step is skipped, as the binary cannot run on the host).

Icons live in `packaging/icons/` and are committed: `png/{16..512}.png`, `drafftink.ico`, `drafftink.icns`. They are cut from the old repo's `logo.png` (the mark without the word "Drafft", on white). `packaging/icons/generate.py` regenerates them (needs Pillow). The source logo is 293 px wide, so the 512 px and the macOS 1024 px entries are upscaled; replace `logo-original.png` with a vector or larger source when one exists and rerun the script.

The Windows exe gets its icon and version resource from `apps/drafftink/build.rs` (the `embed-resource` crate; it does nothing on other targets, and it follows the *target*, so a cross build embeds it too). The macOS `Info.plist` template is `packaging/macos/Info.plist.in`; the Linux desktop entry and MIME type are in `packaging/linux/`.

## CI (`.github/workflows/ci.yml`)

- `build-native`: `cargo build --workspace --release --locked` on ubuntu, windows and macos with `rust-cache`, then each of `drafftink`, `drafftink-mcp`, `drafftink-server` with `--version`.
- `packaging`: on ubuntu-22.04, shellcheck over the scripts, then `scripts/package-linux.sh --formats "tar deb appimage"` end to end.
- Both are in the `needs` of the aggregate `ci` job. macOS and Windows packaging are only exercised by `release.yml`.

## Releasing

1. Choose the version, edit `[workspace.package] version` in `Cargo.toml`, run `cargo update --workspace --offline`, `cargo xtask ci`, merge to `main`.
2. Tag the merge commit and push the tag: `git tag v0.2.0 && git push origin v0.2.0`. The workflow fails at once if the tag differs from the Cargo version (`v` + version).
3. `release.yml` builds the web app, then Linux (x86_64 on ubuntu-22.04, aarch64 on ubuntu-22.04-arm), macOS (one macos-15 job, both targets, `lipo`) and Windows in parallel, then creates a **draft** release: all files, fixed-name copies, `SHA256SUMS.txt`, and notes from `.github/release-notes.md` plus GitHub's generated change list. A version with a `-` (`0.3.0-rc.1`) makes a prerelease.
4. Check the draft: every file present (the job fails when one is missing), `sha256sum --check --ignore-missing SHA256SUMS.txt`, run the server archive and open the page. Publish: `gh release edit v0.2.0 --draft=false --latest` (a prerelease: `--draft=false --prerelease --latest=false`).
5. A manual run of the workflow (Actions tab, "Run workflow") builds everything and keeps the files as workflow artifacts without creating a release. Use it to try the pipeline before tagging.

Never move or re-push a published tag; release a new version instead.

## Signing and notarization

All builds are **unsigned** unless the secrets below exist; the workflow detects them and signs, otherwise it builds the same files unsigned. The release notes tell users how to get past the warnings.

macOS (needs an Apple Developer ID, 99 USD per year). Repository secrets:

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE_P12` | base64 of the exported "Developer ID Application" certificate (`base64 -i cert.p12`) |
| `MACOS_CERTIFICATE_PASSWORD` | the password of that `.p12` |
| `MACOS_SIGN_IDENTITY` | the identity name, e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | for notarization: Apple ID, team id, an app-specific password from appleid.apple.com |

With the first three the app and the dmg are signed with the hardened runtime and `packaging/macos/entitlements.plist`; with all six the dmg is also notarized (`xcrun notarytool submit --wait`) and stapled. Unsigned, users right-click > Open once, or run `xattr -dr com.apple.quarantine /Applications/Drafftink.app`. The signing path is written to Apple's documented flow but has **not been run**: no Mac or Apple certificate was available.

Windows: not signed, so SmartScreen warns on first run ("More info" > "Run anyway"). Options when it matters: an OV/EV code-signing certificate or Azure Trusted Signing, then a `signtool sign` step on `drafftink.exe`, `drafftink-mcp.exe` and the MSI before upload. SignPath (free for open source) is what solidcraft uses. Not wired up.

Linux: AppImage and deb are unsigned. `SHA256SUMS.txt` is the integrity check; a detached signature (minisign or GPG) can be added in the `release` job.

## Verified here and not

Verified on the development machine (Ubuntu 24.04, x86_64):

- `scripts/package-linux.sh` end to end: tar.gz, deb (`dpkg-deb --info/--contents`), AppImage (built, run with `--version`, and `DRAFFTINK_APPIMAGE_RUN=drafftink-mcp`), and the server archive with the real web build; the unpacked server answered `/health`, `/` and the wasm with 200.
- A clean `ubuntu:22.04` container (no `-dev` packages) running the same script.
- `scripts/package-web.sh` with the real web build (`scripts/build-web.sh`).
- `cargo check` of `drafftink`, `drafftink-mcp`, `drafftink-server` for `aarch64-unknown-linux-gnu` (C code via `zig cc`) and `x86_64-pc-windows-gnu` (zig cc plus a stand-in `windres`; the generated `.rc` was inspected). These are type checks, not links.
- YAML syntax of the workflows, XML syntax of `Info.plist.in`, `drafftink.wxs`, the entitlements.

Only in CI (never run by us): the macOS, Windows and aarch64 jobs, `lipo`, `hdiutil`, `codesign`, `wix build`, the real `embed-resource` compile with `rc.exe`, the `release` job and `gh release create`. Expect to fix small things on the first tag; use the manual run first.

## Not done, on purpose

rpm, Flatpak, Snap, WinGet, Homebrew, Nix, Docker image, update channels, an in-app updater and the PWA offline hardening that solidcraft has. Each can be added as another script plus a job once there are users asking. A Docker image for the server (`FROM scratch` + the server archive) is the cheapest next step.
