Drafft.ink {version}: an infinite canvas whiteboard. No account, no telemetry.

These builds are **not code-signed** (see "Unsigned builds" below). Verify downloads with `SHA256SUMS.txt`:
`sha256sum --check --ignore-missing SHA256SUMS.txt`.

## Which file?

| You want | Download |
|---|---|
| Windows installer | `drafftink-{version}-windows-x64.msi` |
| Windows, no install | `drafftink-{version}-windows-x64.zip` |
| macOS (Apple silicon and Intel) | `drafftink-{version}-macos-universal.dmg` |
| Linux, one file for any distro | `drafftink-{version}-linux-x86_64.AppImage` or `...-aarch64.AppImage` |
| Debian, Ubuntu, Mint | `drafftink-{version}-linux-x86_64.deb` or `...-aarch64.deb` |
| Linux, plain archive | `drafftink-{version}-linux-x86_64.tar.gz` or `...-aarch64.tar.gz` |
| Host your own server and web app | `drafftink-server-{version}-<os>-<arch>` archive (the binary and the web build in one folder) |
| The web build only (any static host) | `drafftink-web-{version}.tar.gz` or `.zip` |

Every desktop package also contains `drafftink-mcp`, the MCP server that lets an AI agent draw in the window.
Each file is also attached without the version in its name (`drafftink-windows-x64.msi`, `drafftink-macos-universal.dmg`,
...), so `releases/latest/download/<name>` works.

## Install

- **Windows:** run the `.msi`, or unzip and run `drafftink.exe`. SmartScreen shows "Windows protected your PC" for an unsigned
  file: choose "More info", then "Run anyway".
- **macOS:** open the `.dmg`, drag Drafftink to Applications. The first time, right-click the app and choose "Open"
  (or run `xattr -dr com.apple.quarantine /Applications/Drafftink.app`).
- **Linux AppImage:** `chmod +x drafftink-*.AppImage && ./drafftink-*.AppImage`.
- **Linux deb:** `sudo apt install ./drafftink-{version}-linux-x86_64.deb`.
- **Server:** unpack the `drafftink-server` archive and run `./drafftink-server` in that folder, then open http://127.0.0.1:3030.
  `--help` lists the options (bind address, storage, auth tokens).

The app needs a GPU with Vulkan (Linux), Metal (macOS) or DirectX 12 (Windows).

## Unsigned builds

Nothing here is signed or notarized. The `.dmg`, `.msi` and `.exe` files will trigger the OS warnings described above.
