# drafftink-headless

Drafft.ink without a screen: the collaboration server, an MCP server for AI agents and command line tools for documents. It links no GPU, window, UI or file-dialog code, so it runs over SSH, in CI and in a container.

```sh
cargo install drafftink-headless --features sqlite,s3

drafftink-headless serve --web-dir dist                       # the collaboration server (and the web app)
drafftink-headless mcp                                        # MCP on stdio, its own document
drafftink-headless convert diagram.mmd diagram.drafftink      # also: run, outline, inspect, commands, schemas
claude mcp add drafftink -- drafftink-headless mcp
```

Without a subcommand it prints the help and exits with 2. The desktop app is the other program, `drafftink`; its `drafftink mcp` drives the window that is open.

Part of [Drafft.ink](https://drafft.ink), an infinite canvas whiteboard written in Rust. See the [main README](https://github.com/PatWie/drafft-ink#readme) and the [documentation](https://github.com/PatWie/drafft-ink/tree/main/docs) for the whole project and how the crates fit together.
