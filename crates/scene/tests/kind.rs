#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `Kind::ALL`, `Display` and `FromStr`.

use drafftink_scene::Kind;

/// Fails to compile when a kind is added and not listed in `Kind::ALL`.
fn index(k: Kind) -> usize {
    match k {
        Kind::Rectangle => 0,
        Kind::Diamond => 1,
        Kind::Ellipse => 2,
        Kind::Line => 3,
        Kind::Arrow => 4,
        Kind::Freehand => 5,
        Kind::Text => 6,
        Kind::Math => 7,
        Kind::Image => 8,
        Kind::Frame => 9,
    }
}

#[test]
fn all_lists_every_kind_once_in_enum_order() {
    for (i, k) in Kind::ALL.into_iter().enumerate() {
        assert_eq!(index(k), i);
    }
}

#[test]
fn display_and_from_str_round_trip_for_every_kind() {
    for k in Kind::ALL {
        assert_eq!(k.to_string(), k.name());
        assert_eq!(k.name().parse::<Kind>(), Ok(k));
    }
    assert!("circle".parse::<Kind>().is_err());
    assert!("Rectangle".parse::<Kind>().is_err());
}
