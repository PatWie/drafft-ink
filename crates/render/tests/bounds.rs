#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `paint_bounds` is what the spatial index holds, so everything an element paints must lie inside it.

mod common;

use common::*;
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use kurbo::{Point, Rect};
use proptest::prelude::*;

fn sloppiness(i: u8) -> Sloppiness {
    [Sloppiness::Architect, Sloppiness::Artist, Sloppiness::Cartoonist, Sloppiness::Drunk][usize::from(i % 4)]
}

fn pattern(i: u8) -> FillPattern {
    [
        FillPattern::Solid,
        FillPattern::Hachure,
        FillPattern::ZigZag,
        FillPattern::CrossHatch,
        FillPattern::Dots,
        FillPattern::Dashed,
        FillPattern::ZigZagLine,
    ][usize::from(i % 7)]
}

fn head(i: u8) -> Head {
    [
        Head::None,
        Head::Arrow,
        Head::Bar,
        Head::Circle,
        Head::CircleOutline,
        Head::Triangle,
        Head::TriangleOutline,
        Head::Diamond,
        Head::DiamondOutline,
        Head::CardinalityOne,
        Head::CardinalityMany,
        Head::CardinalityOneOrMany,
        Head::CardinalityExactlyOne,
        Head::CardinalityZeroOrOne,
        Head::CardinalityZeroOrMany,
    ][usize::from(i % 15)]
}

fn contained(e: &Element, painter: &mut ElementPainter, ts: &mut Typesetter) -> Result<(), String> {
    let frag = painter.paint(e, &RenderOptions::default(), ts, &|_| None);
    let m = paint_margin(e);
    let a = Rect::new(-m.left, -m.top, e.transform.w + m.right, e.transform.h + m.bottom);
    let b = frag.bounds;
    if b.x0 >= a.x0 - 1e-6 && b.y0 >= a.y0 - 1e-6 && b.x1 <= a.x1 + 1e-6 && b.y1 <= a.y1 + 1e-6 {
        Ok(())
    } else {
        Err(format!("{:?} drew {b:?} outside {a:?}", e.tag()))
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn shapes_stay_inside_their_paint_bounds(
        kind in 0u8..4, w in 1.0f64..400.0, h in 1.0f64..300.0, sw in 0.0f64..24.0, rough in 0u8..4, pat in 0u8..7,
        seed in any::<u32>(), fill in any::<bool>(), radius in 0.0f64..40.0, dashed in 0u8..3,
    ) {
        let z = ZIndex::first();
        let mut e = match kind {
            0 => rect_el("a", &z, 5.0, 5.0, w, h),
            1 => ellipse_el("a", &z, 5.0, 5.0, w, h),
            2 => Element::new(id("a"), ElementKind::Diamond, z.clone()).with_bounds(5.0, 5.0, w, h),
            _ => rect_el("a", &z, 5.0, 5.0, w, h),
        };
        e.style.stroke_width = sw;
        e.style.sloppiness = sloppiness(rough);
        e.style.fill_pattern = pattern(pat);
        e.style.fill_color = fill.then(|| Color::rgb(10, 200, 50));
        e.style.seed = seed;
        e.style.corner_radius = radius;
        e.style.stroke_style = [StrokeStyle::Solid, StrokeStyle::Dashed, StrokeStyle::Dotted][usize::from(dashed)];
        let mut painter = ElementPainter::new();
        let mut ts = Typesetter::new();
        prop_assert_eq!(contained(&e, &mut painter, &mut ts), Ok(()));
    }

    #[test]
    fn lines_and_arrows_stay_inside_their_paint_bounds(
        pts in proptest::collection::vec((0.0f64..500.0, 0.0f64..400.0), 2..9), arrow in any::<bool>(),
        round in any::<bool>(), closed in any::<bool>(), sw in 0.0f64..16.0, rough in 0u8..4, seed in any::<u32>(),
        sh in 0u8..15, eh in 0u8..15, hs in proptest::option::of(4.0f64..60.0), fill in any::<bool>(),
    ) {
        let world: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
        prop_assume!(world.windows(2).any(|p| p[0] != p[1]));
        let z = ZIndex::first();
        let kind = if arrow { LinearKind::Arrow } else { LinearKind::Line };
        let Some(mut e) = Element::linear(id("l"), kind, z, &world) else { return Ok(()) };
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
            if round { d.path = PathType::Round; }
            d.closed = closed && !arrow && world.len() >= 3;
            if arrow { d.start_head = head(sh); d.end_head = head(eh); }
            d.head_size = hs;
        }
        e.style.stroke_width = sw;
        e.style.sloppiness = sloppiness(rough);
        e.style.seed = seed;
        e.style.fill_color = fill.then(|| Color::rgb(10, 200, 50));
        e.style.fill_pattern = FillPattern::Hachure;
        let mut painter = ElementPainter::new();
        let mut ts = Typesetter::new();
        prop_assert_eq!(contained(&e, &mut painter, &mut ts), Ok(()));
    }

    #[test]
    fn freehand_stays_inside_its_paint_bounds(
        n in 1usize..80, sw in 0.0f64..24.0, pressure in any::<bool>(), highlighter in any::<bool>(), closed in any::<bool>(),
        seed in any::<u32>(), rough in 0u8..4,
    ) {
        let mut rng = Rng(u64::from(seed));
        let mut e = freehand_el("f", &ZIndex::first(), 3.0, 3.0, n, &mut rng, pressure);
        if let ElementKind::Freehand(d) = &mut e.kind {
            d.highlighter = highlighter;
            d.closed = closed && n >= 3;
        }
        e.style.stroke_width = sw;
        e.style.seed = seed;
        e.style.sloppiness = sloppiness(rough);
        let mut painter = ElementPainter::new();
        let mut ts = Typesetter::new();
        prop_assert_eq!(contained(&e, &mut painter, &mut ts), Ok(()));
    }

    #[test]
    fn world_bounds_hold_a_rotated_element(
        x in -1000.0f64..1000.0, y in -1000.0f64..1000.0, w in 1.0f64..300.0, h in 1.0f64..300.0, angle in -7.0f64..7.0,
    ) {
        let mut e = rect_el("a", &ZIndex::first(), x, y, w, h);
        e.transform.angle = angle;
        let b = paint_bounds(&e);
        let to_world = element_to_world(&e);
        for (px, py) in [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)] {
            let p = to_world * Point::new(px, py);
            prop_assert!(p.x >= b.x0 - 1e-6 && p.x <= b.x1 + 1e-6 && p.y >= b.y0 - 1e-6 && p.y <= b.y1 + 1e-6);
        }
    }
}

#[test]
fn text_math_and_frames_stay_inside_their_paint_bounds() {
    let mut ts = Typesetter::new();
    let mut painter = ElementPainter::new();
    let z = ZIndex::first();
    let mut texts = vec![
        text_el(&mut ts, "t1", &z, 0.0, 0.0, "Hello world"),
        text_el(&mut ts, "t2", &z, 0.0, 0.0, "line one\nline two is longer\nthree"),
        text_el(&mut ts, "t3", &z, 0.0, 0.0, "Ägypten Ωmega"),
    ];
    if let ElementKind::Text(t) = &mut texts[1].kind {
        t.font.family = FontFamily::NotoSans;
        t.font.weight = FontWeight::Light; // the italic face
        t.font.size = 31.0;
    }
    measure_text(&mut ts, &mut texts[1]);
    let mut all: Vec<Element> = texts;
    for (i, f) in
        ["x^2", "\\frac{a+b}{c}", "\\sqrt{\\sum_{i=0}^{n} i^2}", "\\int_0^1 x dx", "\\notacommand"].iter().enumerate()
    {
        all.push(math_el(&mut ts, &format!("m{i}"), &z, 0.0, 0.0, f));
    }
    let mut frame = frame_el("fr", &z, 0.0, 0.0, 200.0, 100.0);
    frame.kind =
        ElementKind::Frame(FrameData { name: Some("A rather long frame name that does not fit".into()), clip: true });
    all.push(frame);
    all.push(frame_el("fr2", &z, 0.0, 0.0, 10.0, 10.0));
    for e in &all {
        assert_eq!(contained(e, &mut painter, &mut ts), Ok(()), "{:?}", e.id);
    }
}
