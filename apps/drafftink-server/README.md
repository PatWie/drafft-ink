# drafftink-server

Collaboration server library for Drafft.ink: WebSocket sync, room storage and a static web client.

**This crate has no binary since 0.2.0.** The program is `drafftink-headless serve`:

```sh
cargo install drafftink-headless --features sqlite,s3
drafftink-headless serve --web-dir dist
```

Use this crate to embed the server in your own program (`drafftink_server::start`, `Args`, `Config`). Version 0.1.0 was the standalone binary.

Part of [Drafft.ink](https://drafft.ink), an infinite canvas whiteboard written in Rust. See the [main README](https://github.com/PatWie/drafft-ink#readme) and the [documentation](https://github.com/PatWie/drafft-ink/tree/main/docs) for the whole project and how the crates fit together.
