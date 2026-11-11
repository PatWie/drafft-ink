//! The typesetting service: layout plus bounded caches.

use std::collections::BTreeSet;
use std::hash::Hash;

use drafftink_geom::hash::{Bits, Hash128};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use drafftink_profile::{Counter, inc};
use kurbo::{Point, Rect, Size};

use crate::bound::{BoundText, Container, ContainerKind, VerticalAlign};
use crate::cache::Lru;
use crate::fonts::ExtraFont;
use crate::layout::{self, Engine, TextLayout};
use crate::math::{MathEngine, MathError, MathLayout, MathSpec};
use crate::style::{FontFamily, LineHeight, Span, SpanStyle, TextSpec, TextStyle, Wrap};

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
    let TextSpec { text, style, spans, align, wrap, line_height, math, reveal } = spec;
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
    // The formulas are in the text, so the text and its size cover them; the mode and the revealed one change the layout.
    (math, reveal).hash(&mut h);
    h.finish128()
}

/// True when the text asks for `family` itself (in its base style or in a span), not only as the glyph fallback.
fn uses_family(spec: &TextSpec, family: &FontFamily) -> bool {
    spec.style.family == *family || spec.spans.iter().any(|s| s.style.family.as_ref() == Some(family))
}

fn feed_math(spec: &MathSpec) -> u128 {
    let MathSpec { latex, size, text_font_fallback } = spec;
    let mut h = Hash128::new();
    (latex, Bits(*size), text_font_fallback).hash(&mut h);
    h.finish128()
}

type MathResult = Result<Arc<MathLayout>, MathError>;

/// Math layout with its cache: the ReX engine (built on first use), the cache and the counters.
struct MathService {
    /// The bytes of STIX Two Math, once this build has them (built in, or given to `provide_font`).
    font: Option<&'static [u8]>,
    /// A formula was asked for while `font` was missing.
    wants_font: bool,
    engine: Option<Result<MathEngine, MathError>>,
    cache: Lru<Key, MathResult>,
    hits: u64,
    misses: u64,
}

impl MathService {
    fn new(cfg: CacheConfig, extras: bool) -> Self {
        Self {
            font: ExtraFont::StixTwoMath.embedded().filter(|_| extras),
            wants_font: false,
            engine: None,
            cache: Lru::new(cfg.math_entries, cfg.math_bytes),
            hits: 0,
            misses: 0,
        }
    }

    /// The layout of `spec`, from the cache when possible. Errors are cached too.
    fn layout(&mut self, spec: &MathSpec, fonts: u64) -> MathResult {
        let Some(font) = self.font else {
            // Not cached: the answer changes when the font arrives.
            self.wants_font = true;
            return Err(MathError::font_loading());
        };
        let key = Key { digest: feed_math(spec), fonts };
        if let Some(hit) = self.cache.get(&key) {
            self.hits += 1;
            inc(Counter::CacheHit);
            return hit;
        }
        self.misses += 1;
        inc(Counter::CacheMiss);
        inc(Counter::Relayouts);
        let _span = drafftink_profile::span("typeset_math");
        let engine = self.engine.get_or_insert_with(|| MathEngine::new(font));
        let result: MathResult = match engine {
            Ok(e) => e.layout(spec, next_id()).map(Arc::new),
            Err(e) => Err(e.clone()),
        };
        let weight = result.as_ref().map_or(256 + spec.latex.len(), |l| l.weight());
        self.cache.insert(key, result.clone(), weight);
        result
    }
}

/// Lays out text and math and caches the results. One per process in the app; cheap to create in
/// tests (it registers the bundled fonts, a few milliseconds).
///
/// Methods take `&mut self` because Parley's shaping contexts are reused between calls. The
/// returned layouts are immutable `Arc`s and can be sent anywhere.
pub struct Typesetter {
    engine: Engine,
    math: MathService,
    /// The extra fonts that are loaded, built in or provided.
    loaded: BTreeSet<ExtraFont>,
    /// Extra fonts that a layout needed and that are not loaded.
    wanted: BTreeSet<ExtraFont>,
    text_cache: Lru<Key, Arc<TextLayout>>,
    font_version: u64,
    text_hits: u64,
    text_misses: u64,
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
        Self::build(cfg, true)
    }

    /// A typesetter that starts without the [`ExtraFont`]s even where the build has them, as a build without
    /// them does. For tests of the loading path.
    pub fn without_extra_fonts() -> Self {
        Self::build(CacheConfig::default(), false)
    }

    fn build(cfg: CacheConfig, extras: bool) -> Self {
        Self {
            engine: Engine::new(extras),
            math: MathService::new(cfg, extras),
            loaded: ExtraFont::ALL.into_iter().filter(|f| extras && f.embedded().is_some()).collect(),
            wanted: BTreeSet::new(),
            text_cache: Lru::new(cfg.text_entries, cfg.text_bytes),
            font_version: 1,
            text_hits: 0,
            text_misses: 0,
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
        let fonts = self.font_version;
        let math = &mut self.math;
        let built = Arc::new(layout::build(&mut self.engine, spec, next_id(), &mut |m| math.layout(m, fonts)));
        if built.has_missing_glyphs() || uses_family(spec, &FontFamily::NotoSans) {
            self.want(&ExtraFont::NOTO_SANS);
        }
        if uses_family(spec, &FontFamily::PlexSans) {
            self.want(&[ExtraFont::PlexSans]);
        }
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
        self.math.layout(spec, self.font_version)
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
        let names = self.engine.register(Arc::new(bytes));
        self.font_version += 1;
        self.text_cache.clear();
        Ok(names)
    }

    fn want(&mut self, fonts: &[ExtraFont]) {
        self.wanted.extend(fonts.iter().copied().filter(|f| !self.loaded.contains(f)));
    }

    /// The extra fonts that a layout needed and that are not loaded yet, each listed until it is provided.
    /// Empty on a build that has the fonts built in. The host fetches them and calls [`Self::provide_font`].
    pub fn wanted_fonts(&mut self) -> Vec<ExtraFont> {
        if std::mem::take(&mut self.math.wants_font) {
            self.want(&[ExtraFont::StixTwoMath]);
        }
        self.wanted.iter().copied().collect()
    }

    /// Gives the typesetter the file of an [`ExtraFont`]. Layouts made without it are dropped from the cache and
    /// the font set version changes, so the next layout uses the font. Providing a font twice, or a font
    /// that is built in, changes nothing.
    pub fn provide_font(&mut self, font: ExtraFont, bytes: Vec<u8>) -> Result<(), FontError> {
        if self.loaded.contains(&font) {
            return Ok(());
        }
        if bytes.len() > MAX_FONT_BYTES {
            return Err(FontError::TooLarge);
        }
        ttf_parser::Face::parse(&bytes, 0).map_err(FontError::Invalid)?;
        if font == ExtraFont::StixTwoMath {
            // ReX borrows the file for as long as the engine lives, which is the rest of the process.
            self.math.font = Some(Box::leak(bytes.into_boxed_slice()));
            self.math.engine = None;
            self.math.wants_font = false;
        } else {
            self.engine.register(Arc::new(bytes));
        }
        self.loaded.insert(font);
        self.wanted.remove(&font);
        self.font_version += 1;
        self.text_cache.clear();
        self.math.cache.clear();
        Ok(())
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
            math_hits: self.math.hits,
            math_misses: self.math.misses,
            text_entries: self.text_cache.len(),
            math_entries: self.math.cache.len(),
            bytes: self.text_cache.weight() + self.math.cache.weight(),
            evictions: self.text_cache.evictions() + self.math.cache.evictions(),
            font_set_version: self.font_version,
        }
    }

    /// Drop every cached layout (counters stay).
    pub fn clear_caches(&mut self) {
        self.text_cache.clear();
        self.math.cache.clear();
    }
}

/// Support for the benchmark only (feature `bench`). Not part of the API.
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench_support {
    use crate::math::{MathEngine, MathError, MathLayout, MathSpec};

    /// Lay out a formula the way the old app did on every frame: build the math font, its cmap
    /// map and the layout from scratch, with no cache.
    pub fn math_with_fresh_engine(spec: &MathSpec) -> Result<MathLayout, MathError> {
        let font = crate::fonts::ExtraFont::StixTwoMath.embedded().ok_or_else(MathError::font_loading)?;
        MathEngine::new(font)?.layout(spec, 0)
    }
}
