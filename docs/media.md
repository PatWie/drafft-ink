# The start canvas and the README pictures

Two things are built from files that a person edits: the welcome document that a first start shows, and the images in `docs/img` that the README uses. Both are reproducible with one command each.

## The welcome document

`crates/engine/assets/intro/intro.jsonl` is a command script: one engine command per line, run in order on an empty document. `intro.json` next to it is generated from it, and the engine embeds that file (`include_str!` in `cmd/doc/open.rs`). Edit the script, never the JSON.

```sh
cargo xtask intro            # run intro.jsonl, rewrite intro.json
cargo xtask intro --check    # fail when intro.json is not the script's output (also part of `cargo xtask test`)
```

Under the hood this is `drafftink-headless run intro.jsonl --batch --seed 7 --out intro.json` (`apps/drafftink-headless/tests/intro.rs` owns the command line). The seed makes element ids and the hand-drawn jitter the same on every run, so the file only changes when the script does. Lines that start with `//` are comments and blank lines are skipped.

To look at the result without starting the app: `drafftink-headless outline crates/engine/assets/intro/intro.json`, or `cargo xtask media --only canvas-only` and open `docs/img/canvas-only.png`.

### Writing the script

The whole script runs as one batch, so a step can be named with `"as"` and later steps use `"$name"` (`"$name.id"`, and for `diagram.create` also `"$name.<node>"`). Elements that other tools address by name (the media shots below) carry a `name` argument, then `"@name"` finds them. Units are world units; the grid is 20. Colours are `#rrggbb` or palette names such as `blue-100` or `indigo-700`; `diagram.create` takes `#rrggbb` only. The full argument list of any command: `drafftink-cli commands element.create`.

| Need | Command |
|---|---|
| Rectangle, diamond, ellipse | `element.create` with `kind`, `bounds {x,y,w,h}`, `style {fill_color, stroke_color, fill_pattern, sloppiness, stroke_width, stroke_style, opacity, corner_radius}` |
| Label inside a shape or on an arrow | `element.create` `kind: text` with `container: "$shape"` and `text` |
| Free text | `element.create` `kind: text`, `at [x,y]`, `text`, optional `width` (wraps), `style {font_size, font_family, font_weight, text_align, stroke_color}` |
| Formula | `element.create` `kind: math`, `at`, `latex`; `text.set_latex` to change it |
| Lines, arrows, curves | `element.create` `kind: line | arrow | freehand`, `points`, `arrow_type: sharp | round | elbow`, `start`/`end {target}` to bind to shapes, `start_head`/`end_head` |
| Sticky note | `element.create_sticky` with `at` (centre) or `bounds`, `size`, `color: yellow | pink | green | blue | orange`, `text` |
| Whole diagram from nodes and edges | `diagram.create` with `nodes`, `edges`, `layout {algorithm: layered | tree | grid | force | none, direction}`, `at`, optional `frame` (a name) or `group` |
| Frame around elements, or an empty frame | `frame.wrap {ids, name, padding}`; `element.create` `kind: frame` with `bounds` |
| Group | `group.create {ids}`, `group.set_name` |
| Change style afterwards | `style.set {ids, props}` (same keys as `style`, plus `font_*`, `text_align`) |
| Position and order | `element.set_bounds`, `element.move`, `element.rotate`, `element.flip`, `arrange.align`, `arrange.distribute`, `arrange.z_order` |
| Names, links, locking | `element.set_name`, `element.set_link`, `element.lock` |
| Background | `doc.set_background {color}` |
| Clean-up | `selection.clear` as the last step; creating an element selects it |

Pictures: `image.insert {"images": [{"path": "crates/engine/assets/intro/logo.png"}], "at": [x, y], "fit": 200}`. The path is relative to the repository root; the picture is embedded in `intro.json` as base64, so keep it small.

The first view: a document has no saved camera. `doc.show_intro` fits all content and then frames the element named `home` (a frame that holds the tour) when the script has one, so content outside it, such as the far-right box, is a reward for panning and does not shrink the first view. Without a `home` element the whole content is fitted. What the script cannot say: anything the headless command line has no port for (clipboard, network). `diagram.create` styles use `#rrggbb`, not palette names.

## README pictures

```sh
cargo xtask media                     # all shots, about 10 s after the build
cargo xtask media --only canvas-only  # one shot (repeat --only for more)
```

It runs the example `media` of `drafftink-app` (`crates/app/examples/media`), which starts the real app headless: the app reads its intro document, the UI runs through `drafftink_ui::draw`, the canvas is drawn by Vello and composited under the UI, and egui paints over it on an offscreen texture (`drafftink_gpu::headless`, the winit shell without a window). With no GPU it uses Mesa lavapipe (`VK_ICD_FILENAMES`, set for you when `/usr/share/vulkan/icd.d/lvp_icd.json` exists), so the pictures do not depend on the machine. The app gets an empty data folder, so no setting or saved workspace of the person who runs it leaks into a picture.

`docs/media/shots.json` lists the pictures. Each shot has a `name` (the file is `docs/img/<name>.png`), optional `steps` (commands run after the intro is shown, `{"id": "...", "args": {...}}`), and a `kind`:

- `window`: the whole app. `size` in logical pixels, `scale` (physical pixels per logical pixel; the PNG is `size * scale`), `theme` (`light` or `dark`), optional `touch: true` for the phone layout. Use `selection.select` to choose what the ribbon shows and `view.zoom_to_fit` or `view.set` for the camera (the intro already frames `@home`).
- `export`: the document alone through the PNG export (`doc.export`), `width` in pixels, `padding`, `theme`, and optionally `ids` (ids or `@name`s) to export only those elements, with the frames' children (the welcome picture uses `@home` and the arrow that leaves it).

| File | What |
|---|---|
| `docs/img/screenshot-light.png`, `screenshot-dark.png` | 1600x1000, the app with the welcome document and the `box` selected |
| `docs/img/screenshot-phone.png` | 780x1688, touch layout |
| `docs/img/canvas-only.png` | the welcome document through the export path, 1600 px wide |

The shots select elements by `@name`, so keep the `name` arguments in `intro.jsonl` that `shots.json` mentions (`box`, `follower`, `home`). A step that refuses (an unknown name) fails the run and says which shot and step. PNGs are written as 8-bit RGB with the best `png` compression, which keeps each under 300 KB for the current content.

To look at a picture without sending it whole to an image reader, scale it down: `ffmpeg -i docs/img/screenshot-light.png -vf scale=1100:-1 /tmp/preview.jpg`.
