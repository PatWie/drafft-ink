//! Sticky notes: a rectangle with a preset look that holds a centred label (`element.create_sticky`).
//!
//! A note is not an element kind. It is an ordinary rectangle that carries the preset of Excalidraw's sticky note
//! (`element/src/stickyNote.ts`, `common/src/colors.ts`) and an optional bound label, so every command, the
//! renderer, the file formats and Excalidraw interchange keep working on it.
//!
//! * **Size.** A new note is [`DEFAULT_SIZE`] square and never smaller than [`MIN_SIZE`] in either direction.
//! * **Look.** Solid fill in one of the [`NoteColor`]s (yellow by default), near-black ink ([`INK`]), rounded corners
//!   (a quarter of the shorter side, at most [`MAX_RADIUS`]). The line width, dash style, hand-drawn roughness and
//!   opacity are the caller's base style.
//! * **Label.** Centred both ways. Its font is the largest size on the grid `ceiling, ceiling - 2, ..` down to
//!   [`FONT_MIN`] at which the wrapped text fits the note ([`fit_font_size`]); when even the smallest size does not
//!   fit, the note grows downwards like any other label container.
//!
//! Left out on purpose: the creation-date footer and the re-fit of the font when the note is resized or its text
//! edited later. The renderer has no shadow, so the note has no drop shadow or fold.
//!
//! Callers run these functions inside one transaction.

use drafftink_geom::kurbo::Size;
use drafftink_scene::{Color, ElementId, IdSource, Kind, SceneTx, StoreError, Style, TextStyle};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::create::{self, CreateError, FrameChoice, Joined};
use crate::text_bind::{self, TextMeasure, inner_size, measure_lines, wrap_text};

/// Side of a new note, in world units.
pub const DEFAULT_SIZE: f64 = 250.0;
/// Smallest width or height of a note.
pub const MIN_SIZE: f64 = 75.0;
/// Largest label font size a note starts from when the caller names none.
pub const FONT_CEILING: f64 = 28.0;
/// Smallest label font size the fit goes down to.
pub const FONT_MIN: f64 = 16.0;
/// Largest label font size a caller may ask for.
pub const FONT_MAX: f64 = 512.0;
/// Distance between two sizes on the fit grid.
pub const FONT_STEP: f64 = 2.0;
/// Corner radius as a fraction of the shorter side.
pub const RADIUS_FRACTION: f64 = 0.25;
/// Largest corner radius.
pub const MAX_RADIUS: f64 = 32.0;
/// The ink of a note: its outline and its label (Excalidraw's default stroke).
pub const INK: Color = Color::rgb(0x1e, 0x1e, 0x1e);

/// The paper colours of a note (Excalidraw's sticky note picks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteColor {
    /// `#ffdf6b`, the default.
    #[default]
    Yellow,
    /// `#fcc2d7`.
    Pink,
    /// `#b2f2bb`.
    Green,
    /// `#a5d8ff`.
    Blue,
    /// `#ffd8a8`.
    Orange,
}

impl NoteColor {
    /// The fill colour.
    pub const fn fill(self) -> Color {
        match self {
            Self::Yellow => Color::rgb(0xff, 0xdf, 0x6b),
            Self::Pink => Color::rgb(0xfc, 0xc2, 0xd7),
            Self::Green => Color::rgb(0xb2, 0xf2, 0xbb),
            Self::Blue => Color::rgb(0xa5, 0xd8, 0xff),
            Self::Orange => Color::rgb(0xff, 0xd8, 0xa8),
        }
    }
}

/// Why a note was not made.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StickyError {
    /// The note would be under [`MIN_SIZE`].
    #[error("the sticky note is smaller than {MIN_SIZE} units in {}", dims(*width, *height))]
    TooSmall {
        /// The width is under the minimum.
        width: bool,
        /// The height is under the minimum.
        height: bool,
    },
    /// The label font size is outside [`FONT_MIN`] to [`FONT_MAX`] or not a number.
    #[error("the font size must be a number from {FONT_MIN} to {FONT_MAX}")]
    BadFontSize,
    /// A rule of creating elements refused (id, name, frame, label text).
    #[error(transparent)]
    Create(#[from] CreateError),
}

impl From<StoreError> for StickyError {
    fn from(e: StoreError) -> Self {
        Self::Create(CreateError::from(e))
    }
}

fn dims(width: bool, height: bool) -> &'static str {
    match (width, height) {
        (true, true) => "width and height",
        (true, false) => "width",
        _ => "height",
    }
}

/// The corner radius of a note of this size.
pub fn corner_radius(w: f64, h: f64) -> f64 {
    (w.min(h) * RADIUS_FRACTION).clamp(0.0, MAX_RADIUS)
}

/// The style of a note: the preset over `base`. The seed of `base` is kept.
pub fn note_style(color: NoteColor, (w, h): (f64, f64), base: &Style) -> Style {
    Style {
        stroke_color: INK,
        fill_color: Some(color.fill()),
        fill_pattern: drafftink_scene::FillPattern::Solid,
        corner_radius: corner_radius(w, h),
        ..base.clone()
    }
}

/// Refuses a box under [`MIN_SIZE`].
pub fn check_size(w: f64, h: f64) -> Result<(), StickyError> {
    let (width, height) = (w < MIN_SIZE, h < MIN_SIZE);
    if width || height { Err(StickyError::TooSmall { width, height }) } else { Ok(()) }
}

/// Checks a requested font ceiling.
pub fn check_ceiling(size: f64) -> Result<f64, StickyError> {
    if size.is_finite() && (FONT_MIN..=FONT_MAX).contains(&size) { Ok(size) } else { Err(StickyError::BadFontSize) }
}

/// The sizes the fit tries: `ceiling`, `ceiling - 2`, and so on while above [`FONT_MIN`], then [`FONT_MIN`] itself.
fn grid(ceiling: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut size = ceiling.max(FONT_MIN);
    while size > FONT_MIN {
        out.push(size);
        size -= FONT_STEP;
    }
    out.push(FONT_MIN);
    out
}

/// The largest font size on the grid below `ceiling` at which `text` wrapped to the inner width of a note of
/// `outer` size fits its inner height; [`FONT_MIN`] when none does.
pub fn fit_font_size<M: TextMeasure + ?Sized>(m: &M, text: &str, font: &TextStyle, outer: Size, ceiling: f64) -> f64 {
    let Some(inner) = inner_size(Kind::Rectangle, outer) else { return FONT_MIN };
    let mut probe = font.clone();
    grid(ceiling)
        .into_iter()
        .find(|size| {
            probe.size = *size;
            let lines = wrap_text(m, text, &probe, inner.width);
            let used = measure_lines(m, &lines, &probe);
            used.width <= inner.width && used.height <= inner.height
        })
        .unwrap_or(FONT_MIN)
}

/// What to make.
#[derive(Debug, Clone)]
pub struct NoteSpec {
    /// The id of the note; `None` draws a fresh one.
    pub id: Option<ElementId>,
    /// Left, top, width, height.
    pub bounds: (f64, f64, f64, f64),
    /// The paper colour.
    pub color: NoteColor,
    /// Text of the label; `None` makes an empty note.
    pub text: Option<String>,
    /// The largest label font size ([`FONT_CEILING`] when `None`).
    pub font_ceiling: Option<f64>,
    /// Line width, dash style, roughness and opacity come from here.
    pub base: Style,
    /// Family, weight and line height of the label.
    pub font: TextStyle,
    /// A unique name for the note.
    pub name: Option<String>,
    /// Which frame the note joins.
    pub frame: FrameChoice,
}

/// A note that was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// The rectangle.
    pub id: ElementId,
    /// Its label, when the spec had text.
    pub label: Option<ElementId>,
    /// The frame it joined.
    pub joined: Joined,
}

/// Makes the note: a rectangle on top of the z-order with the preset style, its name and frame, and the label.
pub fn create_note<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    spec: &NoteSpec,
) -> Result<Note, StickyError> {
    let (_, _, w, h) = spec.bounds;
    check_size(w, h)?;
    let ceiling = check_ceiling(spec.font_ceiling.unwrap_or(FONT_CEILING))?;
    let mut style = note_style(spec.color, (w, h), &spec.base);
    style.seed = ids.seed();
    let wanted = match &spec.id {
        Some(id) => id.clone(),
        None => crate::ids::unused_element_id(&*tx, ids),
    };
    let id = create::insert_on_top(tx, |z| create::shape_element(wanted, Kind::Rectangle, z, spec.bounds, style))?;
    if let Some(n) = &spec.name {
        create::set_name(tx, &id, Some(n))?;
    }
    let joined = create::join_frame(tx, &id, &spec.frame)?;
    let label = match spec.text.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(text) => {
            let mut font = spec.font.clone();
            font.size = fit_font_size(m, text, &font, Size::new(w, h), ceiling);
            let made = text_bind::create_label(tx, m, ids, &id, text, Some(&font)).map_err(CreateError::from)?;
            Some(made.label)
        }
        None => None,
    };
    Ok(Note { id, label, joined })
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{ElementKind, MemoryStore, SceneRead, SceneStore, SeqIds, TextAlign, VerticalAlign};

    use super::*;
    use crate::text_bind::MonospaceMeasure;

    fn spec(text: Option<&str>, size: f64) -> NoteSpec {
        NoteSpec {
            id: None,
            bounds: (10.0, 20.0, size, size),
            color: NoteColor::Yellow,
            text: text.map(str::to_owned),
            font_ceiling: None,
            base: Style::default(),
            font: TextStyle::default(),
            name: None,
            frame: FrameChoice::Auto,
        }
    }

    fn measure() -> MonospaceMeasure {
        MonospaceMeasure::default()
    }

    #[test]
    fn the_grid_runs_from_the_ceiling_down_to_the_minimum() {
        assert_eq!(grid(28.0), [28.0, 26.0, 24.0, 22.0, 20.0, 18.0, 16.0]);
        assert_eq!(grid(27.0), [27.0, 25.0, 23.0, 21.0, 19.0, 17.0, 16.0]);
        assert_eq!(grid(16.0), [16.0]);
    }

    #[test]
    fn the_radius_is_a_quarter_of_the_short_side_up_to_the_cap() {
        assert_eq!(corner_radius(250.0, 250.0), 32.0);
        assert_eq!(corner_radius(80.0, 200.0), 20.0);
    }

    #[test]
    fn sizes_under_the_minimum_are_refused_by_name() {
        assert_eq!(check_size(74.0, 80.0), Err(StickyError::TooSmall { width: true, height: false }));
        assert_eq!(check_size(80.0, 10.0), Err(StickyError::TooSmall { width: false, height: true }));
        assert!(check_size(75.0, 75.0).is_ok());
    }

    #[test]
    fn a_note_with_text_is_a_styled_rectangle_with_a_centred_label() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let mut ids = SeqIds::default();
        let note = create_note(&mut tx, &measure(), &mut ids, &spec(Some("buy milk"), 250.0)).unwrap();
        let rect = tx.get(&note.id).unwrap().clone();
        assert!(matches!(rect.kind, ElementKind::Rectangle));
        assert_eq!(rect.style.fill_color, Some(NoteColor::Yellow.fill()));
        assert_eq!(rect.style.stroke_color, INK);
        assert_eq!(rect.style.corner_radius, 32.0);
        let label = tx.get(note.label.as_ref().unwrap()).unwrap().clone();
        let ElementKind::Text(t) = &label.kind else { panic!("the label is text") };
        assert_eq!(t.container_id.as_ref(), Some(&note.id));
        assert_eq!((t.font.align, t.font.vertical_align), (TextAlign::Center, VerticalAlign::Middle));
        assert_eq!(t.font.size, FONT_CEILING);
    }

    #[test]
    fn long_text_gets_a_smaller_font_and_the_note_keeps_its_size_when_the_minimum_fits() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let mut ids = SeqIds::default();
        let text = "word ".repeat(40);
        let note = create_note(&mut tx, &measure(), &mut ids, &spec(Some(&text), 250.0)).unwrap();
        let label = tx.get(note.label.as_ref().unwrap()).unwrap().clone();
        let ElementKind::Text(t) = &label.kind else { panic!("the label is text") };
        assert!(t.font.size < FONT_CEILING && t.font.size >= FONT_MIN, "{}", t.font.size);
        assert_eq!(tx.get(&note.id).unwrap().transform.h, 250.0);
    }

    #[test]
    fn text_that_does_not_fit_even_at_the_minimum_grows_the_note() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let mut ids = SeqIds::default();
        let text = "word ".repeat(400);
        let note = create_note(&mut tx, &measure(), &mut ids, &spec(Some(&text), 100.0)).unwrap();
        assert!(tx.get(&note.id).unwrap().transform.h > 100.0);
    }

    #[test]
    fn a_blank_text_makes_an_empty_note() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let mut ids = SeqIds::default();
        let note = create_note(&mut tx, &measure(), &mut ids, &spec(Some("   "), 250.0)).unwrap();
        assert_eq!(note.label, None);
    }

    #[test]
    fn a_too_small_note_leaves_nothing_behind() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let mut ids = SeqIds::default();
        let before = tx.len_all();
        let err = create_note(&mut tx, &measure(), &mut ids, &spec(Some("x"), 40.0)).unwrap_err();
        assert!(matches!(err, StickyError::TooSmall { width: true, height: true }));
        assert_eq!(tx.len_all(), before);
    }

    #[test]
    fn the_font_ceiling_is_range_checked() {
        assert_eq!(check_ceiling(15.0), Err(StickyError::BadFontSize));
        assert_eq!(check_ceiling(f64::NAN), Err(StickyError::BadFontSize));
        assert_eq!(check_ceiling(600.0), Err(StickyError::BadFontSize));
        assert_eq!(check_ceiling(40.0), Ok(40.0));
    }
}
