# drafftink-core (deprecated)

**This crate is deprecated and has no code.** Version 0.1.0 was the core of the first Drafft.ink app. The project was
rewritten as a set of smaller crates, and `drafftink-core` was split up. Use the new crates instead:

| You want | Use |
|---|---|
| The editor: commands, `Session`, undo, selection | [`drafftink-engine`](https://crates.io/crates/drafftink-engine) |
| The element model and the scene store trait | [`drafftink-scene`](https://crates.io/crates/drafftink-scene) |
| Geometry, snapping, hit testing | [`drafftink-geom`](https://crates.io/crates/drafftink-geom), [`drafftink-canvas`](https://crates.io/crates/drafftink-canvas) |
| The CRDT document and collaboration | [`drafftink-doc`](https://crates.io/crates/drafftink-doc), [`drafftink-collab`](https://crates.io/crates/drafftink-collab), [`drafftink-protocol`](https://crates.io/crates/drafftink-protocol) |
| Text and math layout | [`drafftink-typeset`](https://crates.io/crates/drafftink-typeset) |
| File formats (native JSON, Excalidraw, SVG, Mermaid) | [`drafftink-io`](https://crates.io/crates/drafftink-io) |
| Rendering | [`drafftink-render`](https://crates.io/crates/drafftink-render), [`drafftink-gpu`](https://crates.io/crates/drafftink-gpu) |
| The desktop app | `cargo install drafftink` |

The layering of the new crates is in the [architecture notes](https://github.com/PatWie/drafft-ink/blob/main/docs/architecture.md).
The project home is [drafft.ink](https://drafft.ink), the source is on [GitHub](https://github.com/PatWie/drafft-ink).
