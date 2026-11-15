//! Decoded pictures, kept in a bounded cache.
//!
//! The old renderer decoded an image inside the render pass, on first sight, and never freed it. Here decoding
//! is a separate step that a host can run where it likes:
//!
//! * [`DecodePolicy::Inline`]: the frame decodes up to a pixel budget itself (simple, fine for native).
//! * [`DecodePolicy::Deferred`]: the frame only records a [`DecodeRequest`]. The host calls
//!   [`ImageCache::take_requests`], decodes with [`decode_image`] on a worker or a task, and hands the result back
//!   with [`ImageCache::complete`]. Until then the element shows a placeholder.
//!
//! Either way a failed decode is remembered (a bad picture is not decoded again every frame), and the decoded
//! pixels are bounded by bytes with least-recently-used eviction.
//!
//! A picture with paint on it is drawn from its *baked* pixels: the decoded picture with the strokes painted over
//! it ([`drafftink_scene::paint::bake`]). The cache keeps them next to the decoded pictures, by file and a digest
//! of the strokes, under the same byte budget. The original stays decoded as well, so a stroke that is erased,
//! undone or reset needs no new decode.

use std::collections::HashMap;
use std::error::Error;
use std::io::Cursor;
use std::sync::Arc;

use drafftink_scene::paint::{bake, bake_over};
use drafftink_scene::{FileId, Paint};
use image::ImageReader;
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

/// Widest or tallest picture that is decoded, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 16_384;
/// Most memory a single decode may allocate, in bytes.
pub const MAX_DECODE_BYTES: u64 = 256 << 20;

/// Why a picture could not be decoded.
///
/// Two errors are equal when their messages are, because the decoder's errors have no equality.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ImageError {
    /// The bytes are not a picture format the decoder reads (PNG, JPEG, WebP, GIF, BMP, ICO). The source is the decoder's error.
    #[error("The picture could not be decoded: {0}.")]
    Decode(#[source] Arc<dyn Error + Send + Sync>),
    /// The picture is larger than the decoder accepts.
    #[error(
        "The picture is larger than {MAX_IMAGE_SIDE} pixels on a side or needs more than {MAX_DECODE_BYTES} bytes."
    )]
    TooLarge,
}

impl PartialEq for ImageError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImageError::Decode(a), ImageError::Decode(b)) => a.to_string() == b.to_string(),
            (ImageError::TooLarge, ImageError::TooLarge) => true,
            _ => false,
        }
    }
}

impl Eq for ImageError {}

impl ImageError {
    fn decode(source: impl Error + Send + Sync + 'static) -> Self {
        ImageError::Decode(Arc::new(source))
    }
}

/// Decodes encoded picture bytes (PNG, JPEG, WebP, GIF, BMP, ICO) into straight-alpha RGBA. Pure: run it on any thread.
pub fn decode_image(bytes: &[u8]) -> Result<ImageData, ImageError> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(ImageError::decode)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        other => ImageError::decode(other),
    })?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(ImageData {
        data: Blob::new(Arc::new(rgba.into_vec())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

/// Who decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodePolicy {
    /// The frame decodes at most `pixels_per_frame` pixels of pictures (at least one picture per frame).
    Inline {
        /// Budget per frame.
        pixels_per_frame: u64,
    },
    /// The frame only records requests; the host decodes and calls [`ImageCache::complete`].
    Deferred,
}

impl Default for DecodePolicy {
    fn default() -> Self {
        DecodePolicy::Inline { pixels_per_frame: 8_000_000 }
    }
}

/// A picture the host is asked to decode.
#[derive(Clone, Debug)]
pub struct DecodeRequest {
    /// Which picture.
    pub file: FileId,
    /// The encoded bytes.
    pub bytes: Arc<[u8]>,
}

/// What the cache knows about a picture.
#[derive(Clone, Debug)]
pub enum ImageState {
    /// A decode was requested and has not finished.
    Pending,
    /// Decoded and ready to draw.
    Ready(ImageData),
    /// Decoding failed; the picture is shown as broken.
    Failed(ImageError),
}

struct Slot {
    state: ImageState,
    tick: u64,
    bytes: usize,
    /// Encoded bytes of a picture waiting for an inline decode.
    source: Option<Arc<[u8]>>,
}

/// A decoded picture with strokes painted over it.
struct Baked {
    image: ImageData,
    /// The strokes that were painted, and the stored size they are in pixels of: what a later bake can build on.
    paint: Paint,
    natural: (u32, u32),
    tick: u64,
}

impl Baked {
    fn bytes(&self) -> usize {
        self.image.data.data().len()
    }
}

/// What the byte budget can evict.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Held {
    Picture(FileId),
    Baked(BakedKey),
}

/// A baked picture: the file and the digest of its strokes (and stored size).
type BakedKey = (FileId, u128);

/// Counters of the image cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageStats {
    /// Pictures decoded successfully.
    pub decoded: u64,
    /// Decodes that failed.
    pub failed: u64,
    /// Pictures evicted to stay under the byte budget.
    pub evictions: u64,
    /// Entries now held.
    pub entries: usize,
    /// Decoded and baked pixel bytes now held.
    pub bytes: usize,
    /// Painted pictures baked (strokes laid over the decoded pixels).
    pub baked: u64,
    /// Of those, the ones that only painted the strokes added to an earlier bake of the same picture.
    pub baked_on_earlier: u64,
    /// Baked pictures now held.
    pub baked_entries: usize,
}

/// Decoded pictures by file id, bounded by bytes.
pub struct ImageCache {
    slots: HashMap<FileId, Slot>,
    baked: HashMap<BakedKey, Baked>,
    requests: Vec<DecodeRequest>,
    policy: DecodePolicy,
    max_bytes: usize,
    bytes: usize,
    tick: u64,
    frame_pixels: u64,
    stats: ImageStats,
}

/// Failed and pending entries are tiny but still bounded.
const MAX_SMALL_ENTRIES: usize = 4096;
/// Most baked versions of one picture kept at once: the shown paint, the one before an undo, and a stroke being drawn.
const MAX_BAKED_PER_FILE: usize = 3;

impl ImageCache {
    /// A cache holding at most `max_bytes` of decoded pixels.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            slots: HashMap::new(),
            baked: HashMap::new(),
            requests: Vec::new(),
            policy: DecodePolicy::default(),
            max_bytes: max_bytes.max(1),
            bytes: 0,
            tick: 0,
            frame_pixels: 0,
            stats: ImageStats::default(),
        }
    }

    /// Chooses who decodes.
    pub fn set_policy(&mut self, policy: DecodePolicy) {
        self.policy = policy;
    }

    /// The current policy.
    pub fn policy(&self) -> DecodePolicy {
        self.policy
    }

    /// Starts a frame: resets the inline decode budget.
    pub fn begin_frame(&mut self) {
        self.frame_pixels = 0;
    }

    /// Requests the host has not taken yet.
    pub fn pending_requests(&self) -> usize {
        self.requests.len()
    }

    /// Takes the recorded decode requests (deferred mode). The host decodes them and calls [`Self::complete`].
    pub fn take_requests(&mut self) -> Vec<DecodeRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Hands back the result of a decode the host ran.
    pub fn complete(&mut self, file: &FileId, result: Result<ImageData, ImageError>) {
        self.finish(file.clone(), result);
    }

    /// The state of a picture, or `None` when it was never asked for. Counts as a use.
    pub fn state(&mut self, file: &FileId) -> Option<&ImageState> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.slots.get_mut(file)?;
        slot.tick = tick;
        Some(&slot.state)
    }

    /// Looks the picture up and starts a decode when needed. Returns the decoded image when it is ready now.
    ///
    /// `bytes` gives the encoded file (the store's blob) and is called only when a decode has to start.
    pub(crate) fn lookup(&mut self, file: &FileId, bytes: impl FnOnce() -> Option<Arc<[u8]>>) -> Lookup {
        self.tick += 1;
        let tick = self.tick;
        let source = if let Some(slot) = self.slots.get_mut(file) {
            slot.tick = tick;
            match &slot.state {
                ImageState::Ready(img) => return Lookup::Ready(img.clone()),
                ImageState::Failed(_) => return Lookup::Failed,
                ImageState::Pending => match slot.source.clone() {
                    Some(src) => src,
                    None => return Lookup::Waiting,
                },
            }
        } else {
            let Some(bytes) = bytes() else { return Lookup::Missing };
            bytes
        };
        match self.policy {
            DecodePolicy::Inline { pixels_per_frame } => {
                if self.frame_pixels > 0 && self.frame_pixels >= pixels_per_frame {
                    self.mark_pending(file.clone(), source, false);
                    return Lookup::Waiting;
                }
                let result = decode_image(&source);
                self.frame_pixels += match &result {
                    Ok(img) => u64::from(img.width) * u64::from(img.height),
                    Err(_) => 1,
                };
                self.finish(file.clone(), result);
                match self.slots.get(file).map(|s| &s.state) {
                    Some(ImageState::Ready(img)) => Lookup::Ready(img.clone()),
                    Some(ImageState::Failed(_)) => Lookup::Failed,
                    _ => Lookup::Waiting,
                }
            }
            DecodePolicy::Deferred => {
                if !self.slots.contains_key(file) {
                    self.mark_pending(file.clone(), source, true);
                }
                Lookup::Waiting
            }
        }
    }

    /// True when drawing the picture again would give a different result than the placeholder drawn earlier:
    /// it is decoded (or failed) now, or its bytes arrived and no decode has started.
    pub(crate) fn changed_since_placeholder(&self, file: &FileId, has_bytes: impl FnOnce() -> bool) -> bool {
        match self.slots.get(file).map(|s| &s.state) {
            Some(ImageState::Ready(_) | ImageState::Failed(_)) => true,
            Some(ImageState::Pending) => {
                // An inline decode waits for the next frame's budget.
                matches!(self.policy, DecodePolicy::Inline { .. })
            }
            None => has_bytes(),
        }
    }

    fn mark_pending(&mut self, file: FileId, bytes: Arc<[u8]>, record: bool) {
        let source = if record { None } else { Some(Arc::clone(&bytes)) };
        self.slots.insert(file.clone(), Slot { state: ImageState::Pending, tick: self.tick, bytes: 0, source });
        if record {
            self.requests.push(DecodeRequest { file, bytes });
        }
        self.trim_small();
    }

    fn finish(&mut self, file: FileId, result: Result<ImageData, ImageError>) {
        self.tick += 1;
        if let Some(old) = self.slots.remove(&file) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        let (state, bytes) = match result {
            Ok(img) => {
                self.stats.decoded += 1;
                let n = img.data.data().len();
                (ImageState::Ready(img), n)
            }
            Err(e) => {
                self.stats.failed += 1;
                (ImageState::Failed(e), 0)
            }
        };
        self.bytes += bytes;
        self.slots.insert(file, Slot { state, tick: self.tick, bytes, source: None });
        self.evict();
    }

    fn evict(&mut self) {
        if self.bytes <= self.max_bytes {
            return;
        }
        // Drop the least recently used pixels (decoded or baked) until 10% under the budget. The newest stays.
        let target = self.max_bytes - self.max_bytes / 10;
        let mut order: Vec<(u64, Held)> = self
            .slots
            .iter()
            .filter(|(_, s)| s.bytes > 0)
            .map(|(k, s)| (s.tick, Held::Picture(k.clone())))
            .chain(self.baked.iter().map(|(k, b)| (b.tick, Held::Baked(k.clone()))))
            .collect();
        order.sort_unstable();
        order.pop();
        for (_, held) in order {
            if self.bytes <= target {
                break;
            }
            let freed = match held {
                Held::Picture(file) => self.slots.remove(&file).map(|s| s.bytes),
                Held::Baked(key) => self.baked.remove(&key).map(|b| b.bytes()),
            };
            if let Some(freed) = freed {
                self.bytes = self.bytes.saturating_sub(freed);
                self.stats.evictions += 1;
            }
        }
    }

    /// The picture with the strokes of `paint` painted over `original` (the decoded picture of `file`).
    ///
    /// `natural` is the stored size of the picture the strokes are in pixels of. The result is baked once per
    /// distinct paint and kept under the byte budget; asking again is a lookup. Without strokes, or when the
    /// strokes cannot be painted onto this picture, it is `original`.
    pub fn baked(&mut self, file: &FileId, natural: (u32, u32), paint: &Paint, original: &ImageData) -> ImageData {
        if paint.is_empty() || natural.0 == 0 || natural.1 == 0 {
            return original.clone();
        }
        let key = (file.clone(), crate::hash::paint_key(natural, paint));
        self.tick += 1;
        let tick = self.tick;
        if let Some(hit) = self.baked.get_mut(&key) {
            hit.tick = tick;
            return hit.image.clone();
        }
        let scale =
            (f64::from(original.width) / f64::from(natural.0), f64::from(original.height) / f64::from(natural.1));
        // A picture that gains strokes (the paint editor shows the stroke being drawn as one more stroke) only needs
        // the new ones painted over the bake of the strokes it already had.
        let earlier = self.longest_bake_within(file, natural, paint);
        let pixels = match &earlier {
            Some((base, done)) => bake_over(
                base.data.data(),
                original.data.data(),
                original.width,
                original.height,
                scale,
                paint.values().skip(*done),
            ),
            None => bake(original.data.data(), original.width, original.height, scale, paint.values()),
        };
        let Some(pixels) = pixels else { return original.clone() };
        let image = ImageData { data: Blob::new(Arc::new(pixels)), ..original.clone() };
        self.stats.baked += 1;
        self.stats.baked_on_earlier += u64::from(earlier.is_some());
        self.forget_oldest_bake_of(file);
        let entry = Baked { image: image.clone(), paint: paint.clone(), natural, tick };
        self.bytes += entry.bytes();
        self.baked.insert(key, entry);
        self.evict();
        image
    }

    /// The bake of `file` whose strokes are the first strokes of `paint` (as many as possible), and how many.
    /// It counts as a use.
    fn longest_bake_within(&mut self, file: &FileId, natural: (u32, u32), paint: &Paint) -> Option<(ImageData, usize)> {
        let (key, done) = self
            .baked
            .iter()
            .filter(|(k, b)| &k.0 == file && b.natural == natural && b.paint.len() < paint.len())
            .filter(|(_, b)| b.paint.iter().zip(paint).all(|(old, new)| old == new))
            .map(|(k, b)| (k.clone(), b.paint.len()))
            .max_by_key(|(_, n)| *n)?;
        let entry = self.baked.get_mut(&key)?;
        entry.tick = self.tick;
        Some((entry.image.clone(), done))
    }

    /// Makes room for one more baked version of `file` when it already has [`MAX_BAKED_PER_FILE`].
    fn forget_oldest_bake_of(&mut self, file: &FileId) {
        let mut mine: Vec<(u64, BakedKey)> =
            self.baked.iter().filter(|(k, _)| &k.0 == file).map(|(k, b)| (b.tick, k.clone())).collect();
        if mine.len() < MAX_BAKED_PER_FILE {
            return;
        }
        mine.sort_unstable();
        let excess = mine.len() + 1 - MAX_BAKED_PER_FILE;
        for (_, key) in mine.into_iter().take(excess) {
            if let Some(old) = self.baked.remove(&key) {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
        }
    }

    fn trim_small(&mut self) {
        let small = self.slots.values().filter(|s| s.bytes == 0).count();
        if small <= MAX_SMALL_ENTRIES {
            return;
        }
        let mut order: Vec<(u64, FileId)> =
            self.slots.iter().filter(|(_, s)| s.bytes == 0).map(|(k, s)| (s.tick, k.clone())).collect();
        order.sort_unstable();
        for (_, key) in order.into_iter().take(small - MAX_SMALL_ENTRIES / 2) {
            self.slots.remove(&key);
        }
    }

    /// Forgets everything (a different document was loaded).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.baked.clear();
        self.requests.clear();
        self.bytes = 0;
    }

    /// Counters.
    pub fn stats(&self) -> ImageStats {
        ImageStats { entries: self.slots.len(), baked_entries: self.baked.len(), bytes: self.bytes, ..self.stats }
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new(192 << 20)
    }
}

/// The answer of [`ImageCache::lookup`].
pub(crate) enum Lookup {
    Ready(ImageData),
    /// A decode is queued or running.
    Waiting,
    /// The picture could not be decoded.
    Failed,
    /// The bytes are not in the store (yet).
    Missing,
}
