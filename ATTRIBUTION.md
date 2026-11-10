# Attribution

Third-party works that are copied into this repository, or whose code is ported or adapted into it.
Dependencies fetched by Cargo are not listed here: `cargo xtask deny` checks their licences.

Fonts are separate works. The AGPL-3.0 of this repository does not cover them, and each keeps its
own terms. The copies of the licence texts are next to the files, in `crates/typeset/assets/licenses/`.

## Fonts (`crates/typeset/assets/fonts/`)

Carried over from the old app (`drafft-ink` at `8ce40ab`, `crates/drafftink-render/assets/`).

| File | Font | Author and copyright | Terms | Status |
|---|---|---|---|---|
| `GelPen.ttf`, `GelPenLight.ttf`, `GelPenHeavy.ttf` | GelPen | Shara Weber, sharasfonts.wordpress.com. The font files say only "SharaWeber". | The author's site says GelPen is free to download and DaFont lists it as "100% Free". The old repo's readme (`GelPen-README.txt`) says only "The font GelPen is beautiful and from sharasfonts.wordpress.com". The OFL claim in the old research notes came from another font's readme (Kimberly Geswein's Architects Daughter) and does not apply. | **Redistribution terms not written down by the author.** Free to use; permission to redistribute inside an AGPL project and binaries is not confirmed. Ask the author in writing. |
| `GelPenSerifLight.ttf`, `GelPenSerifMedium.ttf`, `GelPenSerifHeavy.ttf` | GelPen Serif | Shara Weber | Same as GelPen (the author's site: "Free to download"; DaFont "100% Free"). | Same open item as GelPen. |
| `VanillaExtract.ttf` | Vanilla Extract | Shara Weber, 2016 (sharasfonts.com) | The font's name table says "Personal and Commercial Use Allowed". That grants use, not explicit redistribution or modification. | **Open item, decided in `docs/plan.md` §10.5:** keep the font, record it here, ask the author for written permission. |
| `NotoSans-Regular.ttf`, `NotoSans-Bold.ttf`, `NotoSans-Italic.ttf` | Noto Sans | Copyright 2022 The Noto Project Authors (https://github.com/notofonts/latin-greek-cyrillic). Noto is a trademark of Google LLC. | SIL Open Font License 1.1 (`OFL-1.1.txt`). Unmodified. | OK. Optional: the `noto-sans` feature of `drafftink-typeset` (on by default) bundles it. It is the glyph fallback for characters the hand-drawn fonts lack. |
| `STIXTwoMath-Regular.otf` | STIX Two Math 2.00 b137 (file called `rex-xits.otf` in the old app and in ReX) | Copyright (c) 2001-2016 STI Pub Companies; portions MicroPress, Inc. and Elsevier, Inc. STIX Fonts is a trademark of IEEE. | STIX Fonts user licence, based on the SIL OFL (`STIX-user-license.txt`, from the font's own name table). Unmodified; the file was renamed only. | OK. Later STIX Two releases are plain OFL-1.1; upgrading removes the special text. |

Not carried over: `Excalifont-Regular.woff2` (unused in the old app) and the leftover Architects Daughter readme.

## Code

| What | Where | Source | Licence |
|---|---|---|---|
| Bound-text sizing: inner box per container kind, the inverse container dimension, vertical placement, grow-on-overflow rule | `crates/typeset/src/bound.rs`, `Typesetter::bound_text` | Excalidraw `packages/element/src/textElement.ts` (`getBoundTextMaxWidth`, `getBoundTextMaxHeight`, `getContainerCoords`, `computeContainerDimensionForBoundText`, `redrawTextBoundingBox`), described in `research/parts/02c-bound-text-flowchart.md` §3. Reimplemented in Rust; the constants and formulas are the same. | MIT, Copyright (c) 2020 Excalidraw |
| Math glyph fallback to the text font and the Unicode math-alphanumeric to ASCII map | `crates/typeset/src/math.rs` (`math_to_ascii`, `Recorder::symbol`) | The old app's `rex_backend.rs` (this project's own earlier code, AGPL-3.0) | AGPL-3.0 |
| ReX (TeX math layout) is a Git dependency, not copied code | `crates/typeset/Cargo.toml` | https://github.com/KenyC/ReX at `aeccdba38f3fa54195c469319b65c423e17a77ae` | MIT, Copyright (c) 2023 Keny C |
| Colour palette: 22 families by 12 shades, named `blue-500` in command arguments | `crates/scene/src/palette.rs` | The old app's `TAILWIND_COLORS` (`drafftink-widgets/src/colors.rs`), which carries the colour values of Tailwind CSS v3 plus an interpolated 25 shade | MIT, Copyright (c) Tailwind Labs, Inc. |
