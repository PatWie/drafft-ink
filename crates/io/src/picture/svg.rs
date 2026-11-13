//! SVG pictures: an SVG file or text becomes a PNG at a fixed resolution.
//!
//! # Decisions
//!
//! * **Rasterise on import, store a PNG.** The image model holds a bitmap (`ImageData` has a pixel size, a crop
//!   in pixels and a blob); the renderer, the PNG and SVG exports, Excalidraw files and the web page all decode a
//!   blob with the same bitmap decoder, and none of them has a rasteriser. Storing the SVG would put an untrusted
//!   document into the file table, to be parsed again at every start and in every collaborator's browser, and it
//!   would need a rasteriser in the renderer and in the page. The price is that the picture is not sharp at any
//!   zoom: it is drawn at twice its size (at least 512 and at most 2048 pixels on the long side), which is sharp up
//!   to about 2x zoom for a picture at its own size and softer beyond. The picture is shown at the size the SVG
//!   gives itself (`width` and `height`, or the `viewBox`).
//! * **An SVG is untrusted input.** Nothing outside the document is read: no file, no URL, no font, no
//!   stylesheet (`ImageHrefResolver`'s string resolver answers nothing, and there is no `resources_dir`). The only
//!   pictures inside an SVG that are drawn are PNG, JPEG, WebP and GIF data URLs, checked by [`super::probe`]
//!   first; an SVG inside an SVG is not drawn. Scripts and event attributes have nothing to run them. Text is not
//!   drawn, because it needs fonts that drafft.ink does not load into a picture; [`Svg::has_text`] lets the caller
//!   say so.
//! * **Limits** stop what a small file can still do to the machine: [`MAX_SVG_BYTES`] of text, [`MAX_NODES`] XML
//!   nodes, [`MAX_DEPTH`] levels of elements (the converter and the renderer recurse), and [`MAX_EXPANDED`] elements
//!   after `<use>` has copied its targets (a few lines of `<use>` chains copy a group exponentially, the "billion
//!   laughs" of SVG). The XML reader's own entity limit stops entity bombs. After the conversion, patterns are
//!   checked: each tile is a bitmap of the pattern's size times the scale it is drawn at, and neither one tile
//!   ([`MAX_PATTERN_PIXELS`]) nor all of them ([`MAX_PATTERN_TOTAL`]) may be huge. What is not limited: time. A
//!   filter or a dash pattern chosen to be slow can still keep the page busy for a while.

use std::sync::Arc;

use image::{DynamicImage, RgbaImage};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::roxmltree::{self, Document, Node, ParsingOptions};
use resvg::usvg::{self, ImageHrefResolver, ImageKind, Paint};

use super::{Kind, MAX_SIDE, PictureError, Prepared, encode_png, sniff};

/// Most bytes of SVG text.
pub(super) const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;
/// Most XML nodes (elements, text and comments).
const MAX_NODES: u32 = 250_000;
/// Deepest nesting of elements.
const MAX_DEPTH: usize = 128;
/// Most elements drawn, counting each copy that `<use>` makes.
const MAX_EXPANDED: usize = 400_000;
/// Most pixels of one pattern tile.
const MAX_PATTERN_PIXELS: f64 = 16.0 * 1024.0 * 1024.0;
/// Most pixels of all pattern tiles together.
const MAX_PATTERN_TOTAL: f64 = 128.0 * 1024.0 * 1024.0;
/// Most patterns inside patterns.
const MAX_PATTERN_DEPTH: usize = 4;
/// Most pixels of a picture inside an SVG.
const MAX_EMBEDDED_PIXELS: u64 = 16 * 1024 * 1024;
/// The long side of the stored bitmap is twice the picture's own, between these.
const RASTER_SIDE: std::ops::RangeInclusive<f64> = 512.0..=2048.0;
/// What to tell the person when the SVG has text.
const TEXT_NOT_DRAWN: &str = "The SVG has text, which is not drawn: an SVG picture is drawn without fonts. Turn the text into outlines (paths) in the program that made the SVG, or retype it here.";
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// Why an SVG was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum SvgError {
    #[error("the SVG is {size} bytes, and the limit is {MAX_SVG_BYTES}")]
    TooLarge { size: usize },
    #[error("the SVG is not UTF-8 text")]
    NotText,
    #[error("the SVG is not well-formed XML: {0}")]
    Xml(String),
    #[error("the SVG has more than {MAX_NODES} elements")]
    TooManyNodes,
    #[error("the SVG nests elements more than {MAX_DEPTH} deep")]
    TooDeep,
    #[error("the SVG draws more than {MAX_EXPANDED} elements once its `use` references are followed")]
    TooManyCopies,
    #[error("the SVG has a pattern that would need a bitmap of more than {MAX_PATTERN_PIXELS} pixels")]
    PatternTooBig,
    #[error("the root element is not `svg`")]
    NotSvg,
    #[error("the SVG gives no usable size: {0}")]
    NoSize(String),
    #[error("the SVG could not be drawn")]
    Raster,
}

impl From<SvgError> for PictureError {
    fn from(e: SvgError) -> Self {
        PictureError::Unreadable(e.to_string())
    }
}

/// Does the text look like the start of an SVG document? Only a guess to choose the reader: [`parse`] decides.
pub(super) fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let head = head.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(head);
    let head = head.trim_ascii_start();
    head.starts_with(b"<") && head.windows(4).any(|w| w == b"<svg")
}

/// A parsed SVG that passed the limits.
pub(super) struct Svg {
    tree: usvg::Tree,
    /// The size the picture is shown at: the SVG's own, scaled down when it is over [`MAX_SIDE`].
    given: (u32, u32),
    /// The size of the bitmap that is stored.
    natural: (u32, u32),
    has_text: bool,
}

impl Svg {
    /// The size the picture is shown at, in pixels.
    pub(super) fn given(&self) -> (u32, u32) {
        self.given
    }
}

/// Reads an SVG and checks its limits.
pub(super) fn parse(bytes: &[u8]) -> Result<Svg, SvgError> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(SvgError::TooLarge { size: bytes.len() });
    }
    let text =
        std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)).map_err(|_| SvgError::NotText)?;
    // The same options as the converter's own reader (a DOCTYPE with entities is common in old files), plus a limit.
    let options = ParsingOptions { allow_dtd: true, nodes_limit: MAX_NODES, ..ParsingOptions::default() };
    let xml = Document::parse_with_options(text, options).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => SvgError::TooManyNodes,
        other => SvgError::Xml(other.to_string()),
    })?;
    let root = xml.root_element();
    if !is_svg(root, "svg") {
        return Err(SvgError::NotSvg);
    }
    check_depth(&xml)?;
    check_copies(&xml, root)?;
    let tree = usvg::Tree::from_xmltree(&xml, &options_without_outside_world())
        .map_err(|e| SvgError::NoSize(e.to_string()))?;
    let size = tree.size();
    let (width, height) = (f64::from(size.width()), f64::from(size.height()));
    let given = given_size(width, height).ok_or_else(|| SvgError::NoSize("it is not a number above 0".into()))?;
    let natural = raster_size(width, height);
    check_patterns(&tree, Transform::from_scale(natural.0 as f32 / size.width(), natural.1 as f32 / size.height()))?;
    let has_text = xml.descendants().any(|n| is_svg(n, "text"));
    Ok(Svg { tree, given, natural, has_text })
}

fn is_svg(node: Node, name: &str) -> bool {
    let tag = node.tag_name();
    tag.name() == name && tag.namespace() == Some(SVG_NS)
}

/// The size a picture of `width` by `height` is shown at: the same, or smaller to fit [`MAX_SIDE`]. Always at least 1.
fn given_size(width: f64, height: f64) -> Option<(u32, u32)> {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return None;
    }
    let k = (f64::from(MAX_SIDE) / width.max(height)).min(1.0);
    Some(((width * k).round().max(1.0) as u32, (height * k).round().max(1.0) as u32))
}

/// The size of the stored bitmap: twice the picture's size on the long side, but at least 512 and at most 2048.
fn raster_size(width: f64, height: f64) -> (u32, u32) {
    let long = width.max(height);
    let k = (long * 2.0).clamp(*RASTER_SIDE.start(), *RASTER_SIDE.end()) / long;
    let pixels = |side: f64| (side * k).round().clamp(1.0, *RASTER_SIDE.end()) as u32;
    (pixels(width), pixels(height))
}

fn check_depth(xml: &Document) -> Result<(), SvgError> {
    // Nodes come in document order, so a parent is always seen before its children.
    let mut depths: Vec<u32> = Vec::new();
    for node in xml.descendants() {
        let depth = node.parent().and_then(|p| depths.get(p.id().get_usize())).map_or(0, |d| d + 1);
        if depth as usize > MAX_DEPTH {
            return Err(SvgError::TooDeep);
        }
        let slot = node.id().get_usize();
        if depths.len() <= slot {
            depths.resize(slot + 1, 0);
        }
        depths[slot] = depth;
    }
    Ok(())
}

/// Counts the elements that are drawn, following every `<use>` to its target, and refuses a document that gets
/// past [`MAX_EXPANDED`]. A `<use>` that points at one of its own ancestors counts until the limit, too.
fn check_copies(xml: &Document, root: Node) -> Result<(), SvgError> {
    let by_id: std::collections::HashMap<&str, Node> = xml
        .descendants()
        .filter(|n| n.is_element())
        .filter_map(|n| n.attribute("id").map(|id| (id, n)))
        .rev()
        .collect();
    let mut left = MAX_EXPANDED;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        left = left.checked_sub(1).ok_or(SvgError::TooManyCopies)?;
        stack.extend(node.children().filter(Node::is_element));
        if is_svg(node, "use") {
            let target = [node.attribute("href"), node.attribute((XLINK_NS, "href"))]
                .into_iter()
                .flatten()
                .find_map(|h| h.strip_prefix('#'))
                .and_then(|id| by_id.get(id));
            stack.extend(target);
        }
    }
    Ok(())
}

/// What the converter may read: nothing outside the document, and only small pictures inside it.
fn options_without_outside_world() -> usvg::Options<'static> {
    usvg::Options {
        image_href_resolver: ImageHrefResolver {
            resolve_data: Box::new(|_mime, data, _options| embedded_picture(data)),
            resolve_string: Box::new(|_href, _options| None),
        },
        ..usvg::Options::default()
    }
}

/// A picture given as a data URL inside an SVG: PNG, JPEG, WebP or GIF, of a size [`super::probe`] accepts and of at
/// most [`MAX_EMBEDDED_PIXELS`]. The content decides, never the media type the URL claims.
fn embedded_picture(data: Arc<Vec<u8>>) -> Option<ImageKind> {
    let kind = sniff(&data)?;
    let probe = super::probe(&data).ok()?;
    if u64::from(probe.width) * u64::from(probe.height) > MAX_EMBEDDED_PIXELS {
        return None;
    }
    match kind {
        Kind::Png => Some(ImageKind::PNG(data)),
        Kind::Jpeg => Some(ImageKind::JPEG(data)),
        Kind::WebP => Some(ImageKind::WEBP(data)),
        Kind::Gif => Some(ImageKind::GIF(data)),
        Kind::Bmp | Kind::Ico | Kind::Svg => None,
    }
}

/// What the pattern checks have seen so far.
struct PatternBudget {
    pixels: f64,
    visits: usize,
}

/// Refuses a pattern whose tile would be a huge bitmap. resvg draws a tile as a bitmap of the pattern's rectangle
/// times the scale it is used at, and does not limit it.
fn check_patterns(tree: &usvg::Tree, to_pixels: Transform) -> Result<(), SvgError> {
    let mut budget = PatternBudget { pixels: 0.0, visits: 0 };
    check_group_patterns(tree.root(), to_pixels, 0, &mut budget)
}

fn check_group_patterns(
    group: &usvg::Group,
    base: Transform,
    depth: usize,
    budget: &mut PatternBudget,
) -> Result<(), SvgError> {
    if depth > 2 * MAX_PATTERN_DEPTH {
        return Err(SvgError::TooManyCopies);
    }
    // A clip path and a mask are drawn like a group, and may use patterns too.
    if let Some(clip) = group.clip_path() {
        check_group_patterns(clip.root(), base, depth + 1, budget)?;
    }
    if let Some(mask) = group.mask() {
        check_mask_patterns(mask, base, depth + 1, budget)?;
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(inner) => check_group_patterns(inner, base, depth, budget)?,
            usvg::Node::Path(path) => {
                let paints = path.fill().map(|f| f.paint()).into_iter().chain(path.stroke().map(|s| s.paint()));
                for paint in paints {
                    let Paint::Pattern(pattern) = paint else { continue };
                    budget.visits += 1;
                    if depth >= MAX_PATTERN_DEPTH || budget.visits > MAX_EXPANDED {
                        return Err(SvgError::TooManyCopies);
                    }
                    let to_pixels = base.pre_concat(path.abs_transform()).pre_concat(pattern.transform());
                    let (sx, sy) = to_pixels.get_scale();
                    let rect = pattern.rect();
                    let pixels = f64::from(rect.width() * sx) * f64::from(rect.height() * sy);
                    budget.pixels += pixels.max(1.0);
                    if pixels.is_nan() || pixels > MAX_PATTERN_PIXELS || budget.pixels > MAX_PATTERN_TOTAL {
                        return Err(SvgError::PatternTooBig);
                    }
                    check_group_patterns(pattern.root(), Transform::from_scale(sx, sy), depth + 1, budget)?;
                }
            }
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
    Ok(())
}

fn check_mask_patterns(
    mask: &usvg::Mask,
    base: Transform,
    depth: usize,
    budget: &mut PatternBudget,
) -> Result<(), SvgError> {
    check_group_patterns(mask.root(), base, depth, budget)?;
    mask.mask().map_or(Ok(()), |inner| check_mask_patterns(inner, base, depth + 1, budget))
}

/// Draws the SVG and stores it as a PNG, see the module documentation for the size.
pub(super) fn prepare(svg: &Svg) -> Result<Prepared, SvgError> {
    let size = svg.tree.size();
    let natural = svg.natural;
    let mut pixmap = Pixmap::new(natural.0, natural.1).ok_or(SvgError::Raster)?;
    let to_pixels = Transform::from_scale(natural.0 as f32 / size.width(), natural.1 as f32 / size.height());
    resvg::render(&svg.tree, to_pixels, &mut pixmap.as_mut());
    // The bitmap is premultiplied. A PNG is not.
    let straight: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    let image = RgbaImage::from_raw(natural.0, natural.1, straight).ok_or(SvgError::Raster)?;
    let bytes = encode_png(&DynamicImage::ImageRgba8(image)).map_err(|_| SvgError::Raster)?;
    let warnings = if svg.has_text { vec![TEXT_NOT_DRAWN.to_owned()] } else { Vec::new() };
    Ok(Prepared { bytes, mime: Kind::Png.mime(), natural, given: svg.given, shrunk: false, warnings })
}
