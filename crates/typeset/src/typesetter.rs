//! The typesetting service: layout plus bounded caches.

use std::hash::Hash;

use drafftink_geom::hash::{Bits, Hash128};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use drafftink_profile::{Counter, inc};
use kurbo::{Point, Rect, Size};

use crate::bound::{BoundText, Container, ContainerKind, VerticalAlign};
use crate::cache::Lru;
use crate::layout::{self, Engine, TextLayout};
use crate::math::{MathEngine, MathError, MathLayout, MathSpec};
use crate::style::{LineHeight, Span, SpanStyle, TextSpec, TextStyle, Wrap};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Largest font file [`Typesetter::register_font`] accepts.
pub const MAX_FONT_BYTES: usize = 32 << 20;

/// Cache limits. Each cache evicts its least recently used entries when either limit is passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConfig {
    /// Most text layouts kept.
    pub text_entries: usize,
    /// Most bytes (estimated) of text layouts kept.
    pub text_bytes: usize,
    /// Most math layouts kept (errors included).
    pub math_entries: usize,
    /// Most bytes (estimated) of math layouts kept.
    pub math_bytes: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { text_entries: 8192, text_bytes: 64 << 20, math_entries: 2048, math_bytes: 16 << 20 }
    }
}

/// Cache counters. Read them with [`Typesetter::stats`]; tests assert on the deltas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypesetStats {
    /// Text layouts served from the cache.
    pub text_hits: u64,
    /// Text layouts that had to be computed.
    pub text_misses: u64,
    /// Math layouts (or cached errors) served from the cache.
    pub math_hits: u64,
    /// Math layouts that had to be computed.
    pub math_misses: u64,
    /// Entries now in the text cache.
    pub text_entries: usize,
    /// Entries now in the math cache.
    pub math_entries: usize,
    /// Estimated bytes now held by both caches.
    pub bytes: usize,
    /// Entries evicted so far, both caches.
    pub evictions: u64,
    /// Changes whenever the set of fonts changes; part of every cache key.
    pub font_set_version: u64,
}

impl TypesetStats {
    /// Text hit rate in `0.0..=1.0`, `None` before the first lookup.
    pub fn text_hit_rate(&self) -> Option<f64> {
        let n = self.text_hits + self.text_misses;
        (n > 0).then(|| self.text_hits as f64 / n as f64)
    }
}

/// Why a font could not be registered.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FontError {
    /// Larger than [`MAX_FONT_BYTES`].
    #[error("The font file is larger than {MAX_FONT_BYTES} bytes.")]
    TooLarge,
    /// Not a TrueType or OpenType font.
    #[error("The file is not a usable font: {0}.")]
    Invalid(#[source] ttf_parser::FaceParsingError),
}

/// Cache key: a 128-bit digest of everything that affects the layout, plus the font set version.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    digest: u128,
    fonts: u64,
}

// The `feed_*` functions destructure their input without `..`: a new field is a compile error until it is
// hashed here, so a layout can never be served for a spec that differs in a field the key forgot.

fn feed_text_style(h: &mut Hash128, s: &TextStyle) {
    let TextStyle { family, weight, italic, size, color, underline, strike } = s;
    (family, weight, italic, Bits::from(*size), color, underline, strike).hash(h);
}

fn feed_span_style(h: &mut Hash128, s: &SpanStyle) {
    let SpanStyle { family, weight, italic, size, color, underline, strike } = s;
    (family, weight, italic, size.map(Bits::from), color, underline, strike).hash(h);
}

fn feed_text(spec: &TextSpec) -> u128 {
    let TextSpec { text, style, spans, align, wrap, line_height } = spec;
    let mut h = Hash128::new();
    text.hash(&mut h);
    feed_text_style(&mut h, style);
    spans.len().hash(&mut h);
    for Span { range, style } in spans {
        (range.start, range.end).hash(&mut h);
        feed_span_style(&mut h, style);
    }
    align.hash(&mut h);
    match wrap {
        Wrap::None => 0u8.hash(&mut h),
        Wrap::Width(w) => (1u8, Bits::from(*w)).hash(&mut h),
    }
    match line_height {
        LineHeight::Normal => 0u8.hash(&mut h),
        LineHeight::Metrics(v) => (1u8, Bits::from(*v)).hash(&mut h),
        LineHeight::FontSize(v) => (2u8, Bits::from(*v)).hash(&mut h),
    }
    h.finish128()
}

fn feed_math(spec: &MathSpec) -> u128 {
    let MathSpec { latex, size, text_font_fallback } = spec;
    let mut h = Hash128::new();
    (latex, Bits(*size), text_font_fallback).hash(&mut h);
    h.finish128()
}

type MathResult = Result<Arc<MathLayout>, MathError>;

/// Lays out text and math and caches the results. One per process in the app; cheap to create in
/// tests (it registers the bundled fonts, a few milliseconds).
///
/// Methods take `&mut self` because Parley's shaping contexts are reused between calls. The
/// returned layouts are immutable `Arc`s and can be sent anywhere.
pub struct Typesetter {
    engine: Engine,
    math: Option<Result<MathEngine, MathError>>,
    text_cache: Lru<Key, Arc<TextLayout>>,
    math_cache: Lru<Key, MathResult>,
    font_version: u64,
    text_hits: u64,
    text_misses: u64,
    math_hits: u64,
    math_misses: u64,
}

impl Default for Typesetter {
    fn default() -> Self {
        Self::new()
    }
}

impl Typesetter {
    /// A typesetter with the default cache limits.
    pub fn new() -> Self {
        Self::with_config(CacheConfig::default())
    }

    /// A typesetter with explicit cache limits.
    pub fn with_config(cfg: CacheConfig) -> Self {
        Self {
            engine: Engine::new(),
            math: None,
            text_cache: Lru::new(cfg.text_entries, cfg.text_bytes),
            math_cache: Lru::new(cfg.math_entries, cfg.math_bytes),
            font_version: 1,
            text_hits: 0,
            text_misses: 0,
            math_hits: 0,
            math_misses: 0,
        }
    }

    /// Lay out text. Served from the cache when the same spec was laid out before.
    pub fn text(&mut self, spec: &TextSpec) -> Arc<TextLayout> {
        let key = Key { digest: feed_text(spec), fonts: self.font_version };
        if let Some(hit) = self.text_cache.get(&key) {
            self.text_hits += 1;
            inc(Counter::CacheHit);
            return hit;
        }
        self.text_misses += 1;
        inc(Counter::CacheMiss);
        inc(Counter::Relayouts);
        let _span = drafftink_profile::span("typeset_text");
        let built = Arc::new(layout::build(&mut self.engine, spec, next_id()));
        self.text_cache.insert(key, built.clone(), built.weight());
        built
    }

    /// The size of the text. Same cost as [`Self::text`]; use this for bounds so that nothing is
    /// measured during render.
    pub fn measure(&mut self, spec: &TextSpec) -> Size {
        self.text(spec).size
    }

    /// Lay out a formula. Errors are cached too, so a broken formula costs one parse, not one per
    /// frame.
    pub fn math(&mut self, spec: &MathSpec) -> Result<Arc<MathLayout>, MathError> {
        let key = Key { digest: feed_math(spec), fonts: self.font_version };
        if let Some(hit) = self.math_cache.get(&key) {
            self.math_hits += 1;
            inc(Counter::CacheHit);
            return hit;
        }
        self.math_misses += 1;
        inc(Counter::CacheMiss);
        inc(Counter::Relayouts);
        let _span = drafftink_profile::span("typeset_math");
        let engine = self.math.get_or_insert_with(MathEngine::new);
        let result: MathResult = match engine {
            Ok(e) => e.layout(spec, next_id()).map(Arc::new),
            Err(e) => Err(e.clone()),
        };
        let weight = result.as_ref().map_or(256 + spec.latex.len(), |l| l.weight());
        self.math_cache.insert(key, result.clone(), weight);
        result
    }

    /// Lay out `spec` inside `container`, as Excalidraw does for bound text. `spec.wrap` is
    /// ignored: the wrap width comes from the container.
    pub fn bound_text(&mut self, spec: &TextSpec, container: Container, valign: VerticalAlign) -> BoundText {
        let font_size = f64::from(spec.style.size);
        let wrap_width = container.max_text_width(font_size);
        let mut spec = spec.clone();
        spec.wrap = Wrap::Width(wrap_width as f32);
        let layout = self.text(&spec);
        let m = layout.size;

        let mut size = container.size;
        if container.kind != ContainerKind::Arrow {
            let max_h = container.max_text_height(m.height);
            if m.height > max_h {
                size.height = Container::dimension_for_text(container.kind, m.height);
            }
            if m.width > wrap_width + 0.01 {
                size.width = Container::dimension_for_text(container.kind, m.width);
            }
        }
        let grown = Container { kind: container.kind, size };
        // The wrap width follows the (possibly grown) container.
        let (layout, wrap_width) = if size.width == container.size.width {
            (layout, wrap_width)
        } else {
            let w = grown.max_text_width(font_size);
            spec.wrap = Wrap::Width(w as f32);
            (self.text(&spec), w)
        };
        let max_h = grown.max_text_height(layout.size.height);
        let o = grown.inner_origin();
        let inner = Rect::new(o.x, o.y, o.x + wrap_width, o.y + max_h);
        let h = layout.size.height;
        let y = match valign {
            VerticalAlign::Top => o.y,
            VerticalAlign::Bottom => o.y + (max_h - h),
            VerticalAlign::Middle => o.y + (max_h / 2.0 - h / 2.0),
        };
        BoundText { layout, origin: Point::new(o.x, y), wrap_width, inner, container_size: size }
    }

    /// Smallest container for a text style: one line plus padding, as Excalidraw gives an empty
    /// shape before its first text.
    pub fn min_container_size(&mut self, spec: &TextSpec) -> Size {
        let probe = TextSpec { text: "M".to_owned(), spans: Vec::new(), wrap: Wrap::None, ..spec.clone() };
        let l = self.text(&probe);
        let pad = 2.0 * crate::bound::BOUND_TEXT_PADDING;
        Size::new(l.size.width + pad, l.size.height + pad)
    }

    /// Add a font file. Its families become usable as [`crate::FontFamily::Custom`]. Clears the
    /// text cache and changes the font set version, so no stale layout can be served. Returns the
    /// names of the registered families.
    pub fn register_font(&mut self, bytes: Vec<u8>) -> Result<Vec<String>, FontError> {
        if bytes.len() > MAX_FONT_BYTES {
            return Err(FontError::TooLarge);
        }
        ttf_parser::Face::parse(&bytes, 0).map_err(FontError::Invalid)?;
        let registered = self.engine.font_cx.collection.register_fonts(peniko::Blob::new(Arc::new(bytes)), None);
        let names: Vec<String> = registered
            .iter()
            .filter_map(|(id, _)| self.engine.font_cx.collection.family_name(*id).map(str::to_owned))
            .collect();
        self.font_version += 1;
        self.text_cache.clear();
        Ok(names)
    }

    /// The font set version (changes in [`Self::register_font`]).
    pub fn font_set_version(&self) -> u64 {
        self.font_version
    }

    /// Cache counters and sizes.
    pub fn stats(&self) -> TypesetStats {
        TypesetStats {
            text_hits: self.text_hits,
            text_misses: self.text_misses,
            math_hits: self.math_hits,
            math_misses: self.math_misses,
            text_entries: self.text_cache.len(),
            math_entries: self.math_cache.len(),
            bytes: self.text_cache.weight() + self.math_cache.weight(),
            evictions: self.text_cache.evictions() + self.math_cache.evictions(),
            font_set_version: self.font_version,
        }
    }

    /// Drop every cached layout (counters stay).
    pub fn clear_caches(&mut self) {
        self.text_cache.clear();
        self.math_cache.clear();
    }
}

/// Support for the benchmark only. Not part of the API.
#[doc(hidden)]
pub mod bench_support {
    use crate::math::{MathEngine, MathError, MathLayout, MathSpec};

    /// Lay out a formula the way the old app did on every frame: build the math font, its cmap
    /// map and the layout from scratch, with no cache.
    pub fn math_with_fresh_engine(spec: &MathSpec) -> Result<MathLayout, MathError> {
        MathEngine::new()?.layout(spec, 0)
    }
}
