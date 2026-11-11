//! Pictures of the library items, for the library drawer.
//!
//! The drawer shows one picture per item. It does not draw anything itself: the picture is the answer of the
//! command `library.thumbnail`, which the engine draws with the `Renderer` port (a GPU on the desktop, the
//! browser's GPU or WebGL2 on the web). This module keeps the answers:
//!
//! * one cache entry per item, valid for as long as the item's `content` fingerprint and the palette stay the
//!   same (renaming an item keeps its pictures, a new drawing or the dark theme draws them again);
//! * two sizes per item, the tile and the larger picture of the hover tooltip;
//! * a budget: [`Thumbnails::jobs`] hands out at most [`PER_FRAME`] pictures to ask for, the tiles on screen
//!   first, so that a drawer with a thousand items never stalls a frame and draws only what is looked at;
//! * the browser's way of drawing: a picture that is not ready is `render_pending`. The entry waits and the
//!   next frame asks again, the way the export dialog does.
//!
//! The model asks the session (`Model::read_thumbnails`); this module never sees it.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use drafftink_engine::Refusal;
use serde_json::{Value, json};

use crate::model::LibraryItem;

/// Most pictures asked for in one frame.
pub const PER_FRAME: usize = 2;
/// Pictures kept for items that are off screen; past this the ones nobody looks at are dropped.
const MAX_KEPT: usize = 200;
/// Most pixels a picture may have (a hostile answer must not allocate without limit).
const MAX_PIXELS: u64 = 1100 * 1100;

/// Which picture of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThumbSize {
    /// The picture in the item's tile.
    Tile,
    /// The larger picture in the tooltip.
    Preview,
}

impl ThumbSize {
    /// Pixels along the longer side: twice the points the picture is shown at, for sharp edges on a 2x screen.
    pub const fn max_side(self) -> u32 {
        match self {
            Self::Tile => 224,
            Self::Preview => 480,
        }
    }
}

/// A decoded picture: straight (not premultiplied) 8-bit RGBA, rows from the top.
#[derive(PartialEq, Eq)]
pub struct ThumbImage {
    /// Width and height in pixels.
    pub size: [usize; 2],
    /// `size[0] * size[1] * 4` bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for ThumbImage {
    /// The size only: a picture's bytes are not something to read in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThumbImage({} by {})", self.size[0], self.size[1])
    }
}

impl ThumbImage {
    /// The RGBA bytes of one pixel, `None` outside the picture.
    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        if x >= self.size[0] || y >= self.size[1] {
            return None;
        }
        let at = (y * self.size[0] + x) * 4;
        self.rgba.get(at..at + 4)?.try_into().ok()
    }
}

/// How far one picture has come.
#[derive(Debug, Clone)]
pub enum Thumb {
    /// Asked for, and the renderer is still drawing (the browser).
    Pending,
    /// Drawn.
    Ready(Arc<ThumbImage>),
    /// Not drawn, and asking again would not help; the text says why.
    Failed(String),
}

/// Why an answer of `library.thumbnail` is not a picture.
#[derive(Debug, thiserror::Error)]
enum DecodeError {
    #[error("the answer has no picture in it")]
    NoPicture,
    #[error("the picture is not base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("the picture is not a PNG: {0}")]
    Png(#[from] png::DecodingError),
    #[error("the picture is {0} by {1} pixels, which is too many")]
    TooBig(u32, u32),
    #[error("the picture is not 8-bit RGBA")]
    Format,
}

/// Decodes the output of `library.thumbnail`.
fn decode(answer: &Value) -> Result<ThumbImage, DecodeError> {
    let text = answer.pointer("/bytes/base64").and_then(Value::as_str).ok_or(DecodeError::NoPicture)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(text)?;
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info()?;
    let info = reader.info();
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
        return Err(DecodeError::TooBig(info.width, info.height));
    }
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return Err(DecodeError::Format);
    }
    let mut rgba = vec![0; reader.output_buffer_size().ok_or(DecodeError::Format)?];
    let frame = reader.next_frame(&mut rgba)?;
    rgba.truncate(frame.buffer_size());
    let size = [usize::try_from(frame.width).unwrap_or(0), usize::try_from(frame.height).unwrap_or(0)];
    Ok(ThumbImage { size, rgba })
}

/// What the pictures of one item were drawn for.
#[derive(Debug, Clone)]
struct Pictures {
    content: String,
    dark: bool,
    tile: Option<Thumb>,
    preview: Option<Thumb>,
}

impl Pictures {
    fn slot(&self, size: ThumbSize) -> Option<&Thumb> {
        match size {
            ThumbSize::Tile => self.tile.as_ref(),
            ThumbSize::Preview => self.preview.as_ref(),
        }
    }

    fn slot_mut(&mut self, size: ThumbSize) -> &mut Option<Thumb> {
        match size {
            ThumbSize::Tile => &mut self.tile,
            ThumbSize::Preview => &mut self.preview,
        }
    }
}

/// What the drawer looks at, for the order in which pictures are asked for.
#[derive(Debug, Clone, Copy)]
pub struct Want<'a> {
    /// The canvas is in the dark theme, so pictures are drawn with the dark palette.
    pub dark: bool,
    /// The items whose tiles are on screen.
    pub visible: &'a HashSet<String>,
    /// The item under the pointer, whose larger picture the tooltip shows.
    pub hovered: Option<&'a str>,
}

/// One picture to ask the session for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    item: String,
    content: String,
    size: ThumbSize,
    dark: bool,
}

impl Job {
    /// The arguments of `library.thumbnail`.
    pub fn args(&self) -> Value {
        json!({ "item": self.item, "max_side": self.size.max_side(), "dark": self.dark })
    }
}

/// What a round of asking did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// A picture arrived (or failed for good): the drawer should be drawn again.
    pub changed: bool,
    /// The budget ended with pictures still to ask for: ask again next frame.
    pub more: bool,
    /// A picture is still being drawn by the renderer, which wakes the app when it is done.
    pub waiting: bool,
}

/// The pictures of the library, by item id.
#[derive(Debug, Clone, Default)]
pub struct Thumbnails {
    pictures: HashMap<String, Pictures>,
}

impl Thumbnails {
    /// The picture of `item` in this size and palette, if it was asked for and is still good.
    pub fn picture(&self, item: &LibraryItem, size: ThumbSize, dark: bool) -> Option<&Thumb> {
        let pictures = self.pictures.get(&item.id)?;
        (pictures.content == item.content && pictures.dark == dark).then(|| pictures.slot(size)).flatten()
    }

    /// The number of items that have a picture or a request.
    pub fn len(&self) -> usize {
        self.pictures.len()
    }

    /// True when nothing was asked for.
    pub fn is_empty(&self) -> bool {
        self.pictures.is_empty()
    }

    /// Forgets everything (the library is not read any more).
    pub fn clear(&mut self) {
        self.pictures.clear();
    }

    /// The pictures to ask for now: at most [`PER_FRAME`], the hovered item's larger one first, then the tiles
    /// on screen, in library order. Also drops what is no good any more: items that are gone or changed, and
    /// the other palette. The second value is true when more work is left.
    pub fn jobs(&mut self, items: &[LibraryItem], want: &Want<'_>) -> (Vec<Job>, bool) {
        let content: HashMap<&str, &str> = items.iter().map(|i| (i.id.as_str(), i.content.as_str())).collect();
        self.pictures.retain(|id, p| p.dark == want.dark && content.get(id.as_str()) == Some(&p.content.as_str()));
        let hovered = want.hovered.and_then(|id| items.iter().find(|i| i.id == id)).map(|i| (i, ThumbSize::Preview));
        let tiles = items.iter().filter(|i| want.visible.contains(&i.id)).map(|i| (i, ThumbSize::Tile));
        let mut todo = hovered.into_iter().chain(tiles).filter(|(item, size)| self.needs(item, *size, want.dark));
        let jobs: Vec<Job> = todo
            .by_ref()
            .take(PER_FRAME)
            .map(|(item, size)| Job { item: item.id.clone(), content: item.content.clone(), size, dark: want.dark })
            .collect();
        (jobs, todo.next().is_some())
    }

    /// True when the picture was never asked for, or is waiting for the renderer.
    fn needs(&self, item: &LibraryItem, size: ThumbSize, dark: bool) -> bool {
        match self.picture(item, size, dark) {
            None | Some(Thumb::Pending) => true,
            Some(Thumb::Ready(_) | Thumb::Failed(_)) => false,
        }
    }

    /// Takes the session's answer to a [`Job`].
    pub fn finish(&mut self, job: &Job, answer: Result<Value, Refusal>) -> Progress {
        let (state, progress) = match answer {
            Err(refusal) if refusal.code == "render_pending" => {
                (Thumb::Pending, Progress { waiting: true, ..Progress::default() })
            }
            Err(refusal) => (Thumb::Failed(refusal.message), Progress { changed: true, ..Progress::default() }),
            Ok(out) => {
                let state = match decode(&out) {
                    Ok(image) => Thumb::Ready(Arc::new(image)),
                    Err(e) => Thumb::Failed(e.to_string()),
                };
                (state, Progress { changed: true, ..Progress::default() })
            }
        };
        let entry = self.pictures.entry(job.item.clone()).or_insert_with(|| Pictures {
            content: job.content.clone(),
            dark: job.dark,
            tile: None,
            preview: None,
        });
        *entry.slot_mut(job.size) = Some(state);
        progress
    }

    /// Lets go of the pictures of items that are not on screen when there are too many.
    pub fn trim(&mut self, want: &Want<'_>) {
        if self.pictures.len() > MAX_KEPT {
            self.pictures.retain(|id, _| want.visible.contains(id) || want.hovered == Some(id.as_str()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, content: &str) -> LibraryItem {
        LibraryItem {
            id: id.to_owned(),
            name: id.to_owned(),
            element_count: 1,
            width: 10.0,
            height: 10.0,
            content: content.to_owned(),
        }
    }

    fn png_answer(width: u32, height: u32, rgba: &[u8]) -> Value {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
        writer.finish().unwrap();
        json!({ "bytes": { "base64": base64::engine::general_purpose::STANDARD.encode(out), "mime": "image/png" } })
    }

    fn visible(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn an_answer_decodes_to_straight_rgba_with_the_pixels_in_place() {
        let image = decode(&png_answer(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128])).unwrap();
        assert_eq!(image.size, [2, 1]);
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 0, 255, 128]));
        assert_eq!(image.pixel(2, 0), None);
    }

    #[test]
    fn answers_that_are_not_pictures_are_refused_in_words() {
        for (answer, words) in [
            (json!({}), "no picture"),
            (json!({ "bytes": { "base64": "***" } }), "base64"),
            (json!({ "bytes": { "base64": "aGVsbG8=" } }), "not a PNG"),
        ] {
            let why = decode(&answer).unwrap_err().to_string();
            assert!(why.contains(words), "{why}");
        }
    }

    #[test]
    fn a_picture_with_too_many_pixels_is_not_decoded() {
        let huge = png_answer(1200, 1200, &vec![0; 1200 * 1200 * 4]);
        assert!(matches!(decode(&huge), Err(DecodeError::TooBig(1200, 1200))));
    }

    #[test]
    fn jobs_are_the_hovered_preview_first_then_visible_tiles_within_the_budget() {
        let items = [item("a", "1"), item("b", "2"), item("c", "3"), item("d", "4")];
        let seen = visible(&["b", "c", "d"]);
        let mut cache = Thumbnails::default();
        let want = Want { dark: false, visible: &seen, hovered: Some("d") };
        let (jobs, more) = cache.jobs(&items, &want);
        let asked: Vec<_> = jobs.iter().map(|j| (j.item.as_str(), j.size)).collect();
        assert_eq!(asked, [("d", ThumbSize::Preview), ("b", ThumbSize::Tile)]);
        assert!(more, "c and d's tile are still to do");
        assert_eq!(jobs[0].args(), json!({ "item": "d", "max_side": 480, "dark": false }));
    }

    #[test]
    fn an_item_that_is_not_on_screen_is_not_asked_for() {
        let items = [item("a", "1"), item("b", "2")];
        let seen = visible(&["b"]);
        let mut cache = Thumbnails::default();
        let (jobs, more) = cache.jobs(&items, &Want { dark: false, visible: &seen, hovered: None });
        assert_eq!(jobs.iter().map(|j| j.item.as_str()).collect::<Vec<_>>(), ["b"]);
        assert!(!more);
    }

    #[test]
    fn a_finished_picture_is_not_asked_for_again_and_a_pending_one_is() {
        let items = [item("a", "1")];
        let seen = visible(&["a"]);
        let want = Want { dark: false, visible: &seen, hovered: None };
        let mut cache = Thumbnails::default();
        let (jobs, _) = cache.jobs(&items, &want);
        let job = jobs[0].clone();
        let pending = cache.finish(&job, Err(Refusal::render_pending("library.thumbnail", "picture")));
        assert_eq!(pending, Progress { waiting: true, ..Progress::default() });
        assert!(matches!(cache.picture(&items[0], ThumbSize::Tile, false), Some(Thumb::Pending)));
        assert_eq!(cache.jobs(&items, &want).0, vec![job.clone()], "asked again");
        let done = cache.finish(&job, Ok(png_answer(1, 1, &[1, 2, 3, 255])));
        assert_eq!(done, Progress { changed: true, ..Progress::default() });
        assert!(matches!(cache.picture(&items[0], ThumbSize::Tile, false), Some(Thumb::Ready(_))));
        assert!(cache.jobs(&items, &want).0.is_empty());
    }

    #[test]
    fn a_failure_is_kept_with_its_reason_and_not_asked_for_again() {
        let items = [item("a", "1")];
        let seen = visible(&["a"]);
        let want = Want { dark: false, visible: &seen, hovered: None };
        let mut cache = Thumbnails::default();
        let job = cache.jobs(&items, &want).0.remove(0);
        cache.finish(&job, Err(Refusal::bad_args("library.thumbnail", "no good")));
        assert!(matches!(cache.picture(&items[0], ThumbSize::Tile, false), Some(Thumb::Failed(_))));
        assert!(cache.jobs(&items, &want).0.is_empty());
    }

    #[test]
    fn a_new_drawing_the_other_palette_or_a_removed_item_drops_the_picture() {
        let items = [item("a", "1"), item("b", "1")];
        let seen = visible(&["a", "b"]);
        let mut cache = Thumbnails::default();
        let want = Want { dark: false, visible: &seen, hovered: None };
        for job in cache.jobs(&items, &want).0 {
            cache.finish(&job, Ok(png_answer(1, 1, &[0, 0, 0, 255])));
        }
        assert_eq!(cache.len(), 2);
        // The item "a" now draws something else: its picture is not good for the new fingerprint.
        let changed = [item("a", "9"), item("b", "1")];
        assert!(cache.picture(&changed[0], ThumbSize::Tile, false).is_none());
        assert!(cache.picture(&changed[1], ThumbSize::Tile, false).is_some());
        let (jobs, _) = cache.jobs(&changed, &want);
        assert_eq!(jobs.iter().map(|j| j.item.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(cache.len(), 1, "the stale entry of a is gone");
        // The dark theme draws everything again, and a removed item is forgotten.
        let (jobs, _) = cache.jobs(&changed[1..], &Want { dark: true, ..want });
        assert_eq!(jobs.iter().map(|j| (j.item.as_str(), j.dark)).collect::<Vec<_>>(), [("b", true)]);
        assert!(cache.is_empty());
    }

    #[test]
    fn trimming_keeps_what_is_on_screen() {
        let mut cache = Thumbnails::default();
        for n in 0..=MAX_KEPT {
            let job = Job { item: format!("i{n}"), content: "c".to_owned(), size: ThumbSize::Tile, dark: false };
            cache.finish(&job, Ok(png_answer(1, 1, &[0, 0, 0, 255])));
        }
        let seen = visible(&["i3"]);
        cache.trim(&Want { dark: false, visible: &seen, hovered: Some("i4") });
        assert_eq!(cache.len(), 2, "the visible tile and the hovered item");
    }
}
