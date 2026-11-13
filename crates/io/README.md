# drafftink-io

Every Drafft.ink file format (L4), pure: bytes in, scene Document out. Native JSON, legacy migration, Excalidraw, Mermaid, SVG and PNG metadata.

Part of [Drafft.ink](https://drafft.ink), an infinite canvas whiteboard written in Rust. See the [main README](https://github.com/PatWie/drafft-ink#readme) and the [documentation](https://github.com/PatWie/drafft-ink/tree/main/docs) for the whole project and how the crates fit together.

## Pictures

`picture::prepare` is the one way a picture gets into a document (`image.insert`, paste, drop, the pickers and the `drafftink-canvas` package all use it). It reads the format from the first bytes, never from a name or a media type, and stores:

| Input | Stored | Shown at |
|---|---|---|
| PNG, JPEG, WebP | the bytes as they are (a picture over 2 MiB and 2048 px is re-encoded smaller) | its pixel size |
| GIF | a PNG of the first frame | its pixel size |
| BMP | a PNG | its pixel size |
| ICO | a PNG of the image with the most pixels | its pixel size |
| SVG | a PNG drawn by resvg at twice the SVG's size, at least 512 and at most 2048 px on the long side | the size the SVG gives itself (`width` and `height`, else the `viewBox`) |

An SVG is stored as a bitmap because every consumer of an image blob (the renderer, the PNG and SVG exports, Excalidraw files, the page) decodes bitmaps, and none has a rasteriser. The price is a fixed resolution: sharp up to about twice the zoom, soft beyond. Text in an SVG is not drawn (the picture has no fonts) and the paste says so.

An SVG is untrusted input. Nothing outside the document is read: no file, URL, font or stylesheet; a picture inside an SVG is drawn only when it is a PNG, JPEG, WebP or GIF data URL of at most 16 megapixels, and an SVG inside an SVG is not drawn. It is refused when it is over 8 MiB, has more than 250,000 XML nodes, nests elements deeper than 128, draws more than 400,000 elements once its `<use>` references are followed (the exponential `<use>` bomb), or has a pattern whose tile would be a bitmap of more than 16 megapixels. Not limited: the time a filter or a dash pattern can take. `crates/io/tests/pictures.rs` has the hostile cases.
