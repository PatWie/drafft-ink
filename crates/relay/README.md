# drafftink-relay

The blind relay of Drafft.ink's end-to-end encrypted rooms (L1): a sans-IO room core. It takes frames and the time, and it returns frames to send and the time to come back. It reads no payload, links no Loro and no tokio, and never reads a clock.

Part of [Drafft.ink](https://drafft.ink), an infinite canvas whiteboard written in Rust. See the [main README](https://github.com/PatWie/drafft-ink#readme) and the [documentation](https://github.com/PatWie/drafft-ink/tree/main/docs) for the whole project and how the crates fit together.
