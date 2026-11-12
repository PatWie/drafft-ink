# drafftink-rex

A vendored fork of [ReX](https://github.com/KenyC/ReX) (MIT), the Rust TeX math layout engine, published so that
[Drafft.ink](https://drafft.ink) can depend on it from crates.io (a crate cannot have a Git dependency). The library
is still imported as `rex`.

Changes against upstream commit `aeccdba38f3fa54195c469319b65c423e17a77ae`:

- Only the `ttf-parser` font backend (feature `ttfparser-fontparser`) and the `bbox` render backend are kept.
  The Cairo, FemtoVG, Raqote, Tiny-Skia and Pathfinder renderers and the `font` crate backend are removed, because
  they need Git dependencies.
- The unit tests are removed (they need upstream's renderers, dev-dependencies and font files).
- `unicode-math` is the sibling crate `drafftink-unicode-math`.

Use the upstream crate if you need a renderer. The main project is at <https://github.com/PatWie/drafft-ink>.
