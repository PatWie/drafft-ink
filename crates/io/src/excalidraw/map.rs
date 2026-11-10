//! Name tables between Excalidraw's vocabulary and the scene model: fonts, arrowheads, fills, roughness, corner
//! radius. Import and export both use these, so a value that cannot round trip is found by comparing
//! `import(export(x))` with `x` (see `notes`).

use drafftink_scene::{FillPattern, FontFamily, FontWeight, Head, Sloppiness, StrokeStyle};

/// Font family and weight for an Excalidraw `fontFamily` number, or `None` for an unknown number.
///
/// Excalidraw ids: 1 Virgil, 2 Helvetica, 3 Cascadia, 4 (retired), 5 Excalifont, 6 Nunito, 7 Lilita One,
/// 8 Comic Shanns, 9 Liberation Sans, 10 Assistant. The hand-written faces map to GelPen, every plain face to
/// Noto Sans, and the heavy display face (Lilita One) to Noto Sans in the heavy weight.
pub(super) fn font_from_number(n: i64) -> Option<(FontFamily, FontWeight)> {
    Some(match n {
        1 | 5 | 8 => (FontFamily::GelPen, FontWeight::Regular),
        2..=4 | 6 | 9 | 10 => (FontFamily::NotoSans, FontWeight::Regular),
        7 => (FontFamily::NotoSans, FontWeight::Heavy),
        _ => return None,
    })
}

/// The Excalidraw `fontFamily` number written for a font.
pub(super) fn font_to_number(family: FontFamily, weight: FontWeight) -> i64 {
    match (family, weight) {
        (FontFamily::GelPen, _) => 5,
        (FontFamily::GelPenSerif, _) => 1,
        (FontFamily::VanillaExtract, _) => 8,
        (FontFamily::NotoSans, FontWeight::Heavy) => 7,
        (FontFamily::NotoSans, _) => 9,
    }
}

/// Head for an Excalidraw arrowhead name, including the legacy names.
pub(super) fn head_from_name(name: &str) -> Option<Head> {
    Some(match name {
        "arrow" => Head::Arrow,
        "bar" => Head::Bar,
        "dot" | "circle" => Head::Circle,
        "circle_outline" => Head::CircleOutline,
        "triangle" => Head::Triangle,
        "triangle_outline" => Head::TriangleOutline,
        "diamond" => Head::Diamond,
        "diamond_outline" => Head::DiamondOutline,
        "crowfoot_one" | "cardinality_one" => Head::CardinalityOne,
        "crowfoot_many" | "cardinality_many" => Head::CardinalityMany,
        "crowfoot_one_or_many" | "cardinality_one_or_many" => Head::CardinalityOneOrMany,
        "cardinality_exactly_one" => Head::CardinalityExactlyOne,
        "cardinality_zero_or_one" => Head::CardinalityZeroOrOne,
        "cardinality_zero_or_many" => Head::CardinalityZeroOrMany,
        _ => return None,
    })
}

/// The Excalidraw arrowhead name of a head, or `None` for no head.
pub(super) fn head_to_name(head: Head) -> Option<&'static str> {
    Some(match head {
        Head::None => return None,
        Head::Arrow => "arrow",
        Head::Bar => "bar",
        Head::Circle => "circle",
        Head::CircleOutline => "circle_outline",
        Head::Triangle => "triangle",
        Head::TriangleOutline => "triangle_outline",
        Head::Diamond => "diamond",
        Head::DiamondOutline => "diamond_outline",
        Head::CardinalityOne => "cardinality_one",
        Head::CardinalityMany => "cardinality_many",
        Head::CardinalityOneOrMany => "cardinality_one_or_many",
        Head::CardinalityExactlyOne => "cardinality_exactly_one",
        Head::CardinalityZeroOrOne => "cardinality_zero_or_one",
        Head::CardinalityZeroOrMany => "cardinality_zero_or_many",
    })
}

/// Fill pattern for an Excalidraw `fillStyle` (and the rough.js names an older file may carry).
pub(super) fn fill_from_name(name: &str) -> Option<FillPattern> {
    Some(match name {
        "solid" => FillPattern::Solid,
        "hachure" => FillPattern::Hachure,
        "cross-hatch" => FillPattern::CrossHatch,
        "zigzag" => FillPattern::ZigZag,
        "dots" => FillPattern::Dots,
        "dashed" => FillPattern::Dashed,
        "zigzag-line" => FillPattern::ZigZagLine,
        _ => return None,
    })
}

/// The `fillStyle` Excalidraw knows that is nearest to a pattern. Excalidraw has four: hachure, cross-hatch,
/// solid and zigzag.
pub(super) fn fill_to_name(p: FillPattern) -> &'static str {
    match p {
        FillPattern::Solid => "solid",
        FillPattern::Hachure | FillPattern::Dots | FillPattern::Dashed => "hachure",
        FillPattern::CrossHatch => "cross-hatch",
        FillPattern::ZigZag | FillPattern::ZigZagLine => "zigzag",
    }
}

/// Sloppiness for an Excalidraw `roughness` (0, 1, 2 in practice; any number is accepted and snapped).
pub(super) fn sloppiness_from_roughness(r: f64) -> Sloppiness {
    if r < 0.5 {
        Sloppiness::Architect
    } else if r < 1.5 {
        Sloppiness::Artist
    } else if r < 2.75 {
        Sloppiness::Cartoonist
    } else {
        Sloppiness::Drunk
    }
}

/// The `roughness` written for a sloppiness. Excalidraw stops at 2, so the heaviest level is written as 2.
pub(super) fn roughness_of(s: Sloppiness) -> f64 {
    match s {
        Sloppiness::Architect => 0.0,
        Sloppiness::Artist => 1.0,
        Sloppiness::Cartoonist | Sloppiness::Drunk => 2.0,
    }
}

/// Stroke style for a `strokeStyle` name.
pub(super) fn stroke_style_from_name(name: &str) -> Option<StrokeStyle> {
    match name {
        "solid" => Some(StrokeStyle::Solid),
        "dashed" => Some(StrokeStyle::Dashed),
        "dotted" => Some(StrokeStyle::Dotted),
        _ => None,
    }
}

/// The `strokeStyle` name of a stroke style.
pub(super) fn stroke_style_name(s: StrokeStyle) -> &'static str {
    match s {
        StrokeStyle::Solid => "solid",
        StrokeStyle::Dashed => "dashed",
        StrokeStyle::Dotted => "dotted",
    }
}

/// Excalidraw's `roundness` type numbers.
pub(super) const ROUNDNESS_LEGACY: i64 = 1;
pub(super) const ROUNDNESS_PROPORTIONAL: i64 = 2;
pub(super) const ROUNDNESS_ADAPTIVE: i64 = 3;

/// A parsed `roundness` value: its type number and optional `value`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Roundness {
    pub kind: i64,
    pub value: Option<f64>,
}

/// The corner radius Excalidraw draws for a rounded rectangle-like shape. `side` is the shorter side.
///
/// Types 1 (legacy) and 2 (proportional) take a quarter of the side. Type 3 (adaptive) takes a quarter of the side up
/// to a cut-off and a fixed radius (32 unless `value` says otherwise) above it. Any other type draws square corners.
pub(super) fn corner_radius(r: Roundness, side: f64) -> f64 {
    const PROPORTION: f64 = 0.25;
    const ADAPTIVE: f64 = 32.0;
    let side = if side.is_finite() { side.max(0.0) } else { 0.0 };
    match r.kind {
        ROUNDNESS_LEGACY | ROUNDNESS_PROPORTIONAL => side * PROPORTION,
        ROUNDNESS_ADAPTIVE => {
            let fixed = r.value.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(ADAPTIVE);
            if side <= fixed / PROPORTION { side * PROPORTION } else { fixed }
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_radius_is_a_quarter_of_the_side_capped_at_32() {
        let r = Roundness { kind: ROUNDNESS_ADAPTIVE, value: None };
        assert_eq!(corner_radius(r, 20.0), 5.0);
        assert_eq!(corner_radius(r, 128.0), 32.0);
        assert_eq!(corner_radius(r, 1000.0), 32.0);
        let v = Roundness { kind: ROUNDNESS_ADAPTIVE, value: Some(10.0) };
        assert_eq!(corner_radius(v, 100.0), 10.0);
        assert_eq!(corner_radius(v, 20.0), 5.0);
        assert_eq!(corner_radius(Roundness { kind: 9, value: None }, 20.0), 0.0);
        assert_eq!(corner_radius(Roundness { kind: ROUNDNESS_PROPORTIONAL, value: None }, 1000.0), 250.0);
    }

    #[test]
    fn every_head_has_a_name_that_reads_back() {
        let all = [
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
        ];
        for h in all {
            let name = head_to_name(h).expect("named");
            assert_eq!(head_from_name(name), Some(h), "{name}");
        }
        assert_eq!(head_to_name(Head::None), None);
        assert_eq!(head_from_name("dot"), Some(Head::Circle));
        assert_eq!(head_from_name("crowfoot_many"), Some(Head::CardinalityMany));
        assert_eq!(head_from_name("nonsense"), None);
    }

    #[test]
    fn fonts_cover_every_number_and_round_trip_their_own_output() {
        for n in 1..=10 {
            assert!(font_from_number(n).is_some() || n == 0, "{n}");
        }
        assert!(font_from_number(0).is_none());
        assert!(font_from_number(11).is_none());
        for family in [FontFamily::GelPen, FontFamily::NotoSans] {
            let n = font_to_number(family, FontWeight::Regular);
            assert_eq!(font_from_number(n), Some((family, FontWeight::Regular)));
        }
        assert_eq!(
            font_from_number(font_to_number(FontFamily::NotoSans, FontWeight::Heavy)).map(|f| f.1),
            Some(FontWeight::Heavy)
        );
    }

    #[test]
    fn roughness_snaps_to_the_four_levels() {
        assert_eq!(sloppiness_from_roughness(0.0), Sloppiness::Architect);
        assert_eq!(sloppiness_from_roughness(1.0), Sloppiness::Artist);
        assert_eq!(sloppiness_from_roughness(1.6), Sloppiness::Cartoonist);
        assert_eq!(sloppiness_from_roughness(2.0), Sloppiness::Cartoonist);
        assert_eq!(sloppiness_from_roughness(3.5), Sloppiness::Drunk);
        assert_eq!(roughness_of(Sloppiness::Drunk), 2.0);
    }
}
