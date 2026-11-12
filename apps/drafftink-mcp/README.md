# drafftink-mcp

MCP server library for Drafft.ink: headless, attached to the desktop app, or relayed to the web page.

**This crate has no binary.** The programs are `drafftink mcp` (the desktop app's bridge to the window that is open, or to a browser tab with `--relay`) and `drafftink-headless mcp` (its own document, no app needed; `cargo install drafftink-headless`).

Part of [Drafft.ink](https://drafft.ink), an infinite canvas whiteboard written in Rust. See the [main README](https://github.com/PatWie/drafft-ink#readme) and the [documentation](https://github.com/PatWie/drafft-ink/tree/main/docs) for the whole project and how the crates fit together.
