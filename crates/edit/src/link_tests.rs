use drafftink_scene::{Element, ElementId, ElementKind, MemoryStore, SceneRead, SceneStore, SceneTx, ZIndex};

use super::*;

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn url(text: &str) -> String {
    match parse_input(text) {
        Ok(Input::Url(u)) => u,
        other => panic!("{text:?} gave {other:?}"),
    }
}

fn err(text: &str) -> LinkError {
    parse_input(text).unwrap_err()
}

#[test]
fn web_links_are_kept_and_the_scheme_is_lower_cased() {
    assert_eq!(url("https://example.com/a?b=c#d"), "https://example.com/a?b=c#d");
    assert_eq!(url("HTTP://Example.com"), "http://Example.com");
    assert_eq!(url("  https://example.com  "), "https://example.com");
    assert_eq!(url("https://localhost:3030/ws"), "https://localhost:3030/ws");
    assert_eq!(url("http://[::1]:8080/x"), "http://[::1]:8080/x");
    assert_eq!(url("mailto:a@b.example?subject=hi"), "mailto:a@b.example?subject=hi");
    assert_eq!(url("MAILTO:a@b.example"), "mailto:a@b.example");
}

#[test]
fn a_bare_address_gets_https() {
    assert_eq!(url("example.com/page"), "https://example.com/page");
    assert_eq!(url("localhost"), "https://localhost");
    assert_eq!(url("localhost:3000/x"), "https://localhost:3000/x");
    assert_eq!(url("example.com:8080"), "https://example.com:8080");
}

#[test]
fn empty_text_clears_and_element_links_are_passed_on() {
    assert_eq!(parse_input("").unwrap(), Input::Clear);
    assert_eq!(parse_input("   ").unwrap(), Input::Clear);
    assert_eq!(parse_input("#e12").unwrap(), Input::Element("e12".into()));
    assert_eq!(parse_input("@box").unwrap(), Input::Element("@box".into()));
    assert_eq!(parse_input("#@box").unwrap(), Input::Element("@box".into()));
    assert_eq!(err("#"), LinkError::EmptyElementRef);
    assert_eq!(err("@"), LinkError::EmptyElementRef);
    assert_eq!(err("#@"), LinkError::EmptyElementRef);
}

#[test]
fn schemes_that_can_run_code_or_read_files_are_refused() {
    for bad in [
        "javascript:alert(1)",
        "JavaScript:alert(1)",
        "data:text/html,<b>x</b>",
        "file:///etc/passwd",
        "vbscript:x",
        "ftp://example.com",
        "blob:https://example.com/x",
        "about:blank",
    ] {
        assert!(matches!(err(bad), LinkError::BadScheme { .. } | LinkError::BadCharacter), "{bad}");
    }
    let LinkError::BadScheme { scheme } = err("JAVASCRIPT:alert(1)") else { panic!() };
    assert_eq!(scheme, "javascript");
    assert_eq!(
        err("javascript:alert(1)").to_string(),
        "Links can only use http://, https:// or mailto:, not `javascript:`. Use a web address such as https://example.com, or link to an element with `#id` or `@name`."
    );
}

#[test]
fn disguised_schemes_do_not_get_through() {
    // Whitespace and control characters inside the scheme, an encoded colon, a missing slash pair.
    for bad in ["java\nscript:alert(1)", "java script:x", "\u{1}javascript:x", "http:example.com", "https:/example.com"] {
        assert!(parse_input(bad).is_err(), "{bad}");
    }
    assert!(matches!(err("java%73cript:alert(1)"), LinkError::NotAnAddress { .. }));
    assert!(matches!(err("//example.com"), LinkError::NotAnAddress { .. }));
    assert!(matches!(err("/relative/path"), LinkError::NotAnAddress { .. }));
}

#[test]
fn text_that_is_not_an_address_is_refused_in_plain_words() {
    assert!(matches!(err("hello"), LinkError::NotAnAddress { .. }));
    assert!(matches!(err("https://"), LinkError::NotAnAddress { .. }));
    assert!(matches!(err("https://exa mple.com"), LinkError::Whitespace));
    assert!(matches!(err("https://example.com:99999"), LinkError::NotAnAddress { .. }));
    assert!(matches!(err("https://a..b"), LinkError::NotAnAddress { .. }));
    assert!(matches!(err("mailto:nobody"), LinkError::NotAnAddress { .. }));
    assert_eq!(err("https://user:pw@example.com"), LinkError::UserInfo);
    assert_eq!(err("https://example.com/\"x"), LinkError::BadCharacter);
    assert_eq!(err("https://example.com/<x>"), LinkError::BadCharacter);
    assert_eq!(
        err("hello").to_string(),
        "`hello` is not a web address. Write it like https://example.com/page, link to an element with `#id` or `@name`, or pass null to remove the link."
    );
}

#[test]
fn a_long_text_is_refused_and_shown_shortened() {
    let long = format!("https://example.com/{}", "a".repeat(MAX_LINK_BYTES));
    assert!(matches!(err(&long), LinkError::TooLong { max: MAX_LINK_BYTES, .. }));
    let LinkError::NotAnAddress { shown } = err(&"x".repeat(200)) else { panic!() };
    assert_eq!(shown.chars().count(), 63);
    assert!(shown.ends_with("..."));
}

#[test]
fn a_stored_link_is_checked_again() {
    assert_eq!(classify_stored("https://example.com").unwrap(), Stored::Url("https://example.com".into()));
    assert_eq!(classify_stored("#e3").unwrap(), Stored::Element("e3".into()));
    assert!(matches!(classify_stored("javascript:alert(1)"), Err(LinkError::BadScheme { .. })));
    assert_eq!(classify_stored("#"), Err(LinkError::EmptyElementRef));
    assert_eq!(element_link(&id("e7")), "#e7");
}

fn store_with(locked: bool) -> MemoryStore {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let mut e = Element::new(id("a"), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 10.0, 10.0);
    e.locked = locked;
    tx.insert(e).unwrap();
    tx.commit().unwrap();
    store
}

#[test]
fn set_writes_clears_and_reports_no_change() {
    let mut store = store_with(false);
    let mut tx = store.begin();
    assert_eq!(set(&mut tx, &id("a"), Some("https://example.com".into())), Ok(true));
    assert_eq!(set(&mut tx, &id("a"), Some("https://example.com".into())), Ok(false));
    assert_eq!(tx.get(&id("a")).unwrap().link.as_deref(), Some("https://example.com"));
    assert_eq!(set(&mut tx, &id("a"), None), Ok(true));
    assert_eq!(set(&mut tx, &id("a"), None), Ok(false));
    assert_eq!(tx.get(&id("a")).unwrap().link, None);
    assert_eq!(set(&mut tx, &id("zz"), None), Err(LinkError::NotFound(id("zz"))));
}

#[test]
fn set_refuses_a_locked_element() {
    let mut store = store_with(true);
    let mut tx = store.begin();
    assert_eq!(set(&mut tx, &id("a"), Some("https://example.com".into())), Err(LinkError::Locked(id("a"))));
    assert!(target_exists(&tx, &id("a")));
    assert!(!target_exists(&tx, &id("zz")));
}

fn linked(x: f64, y: f64, w: f64, h: f64, angle: f64) -> Element {
    let mut e = Element::new(id("a"), ElementKind::Rectangle, ZIndex::first()).with_bounds(x, y, w, h);
    e.transform.angle = angle;
    e.link = Some("https://example.com".into());
    e
}

#[test]
fn the_icon_sits_beside_the_top_right_corner_and_keeps_its_screen_size_when_zoomed_out() {
    let e = linked(0.0, 0.0, 100.0, 50.0, 0.0);
    let r = icon_rect(&e, 1.0);
    assert!((r.width() - 12.0).abs() < 1e-9 && (r.height() - 12.0).abs() < 1e-9);
    // x = 100 + 4 - 2 = 102, y = -4 - 12 + 2 = -14.
    assert!((r.x0 - 102.0).abs() < 1e-9 && (r.y0 - -14.0).abs() < 1e-9, "{r:?}");
    assert_eq!(icon_rect(&e, 0.25), r, "below zoom 1 the icon is as big in world units as at zoom 1");
    assert!((icon_rect(&e, 2.0).width() - 6.0).abs() < 1e-9, "above 1 it shrinks with the zoom");
}

#[test]
fn the_icon_turns_with_the_element() {
    let quarter = std::f64::consts::FRAC_PI_2;
    let e = linked(0.0, 0.0, 100.0, 100.0, quarter);
    let r = icon_rect(&e, 1.0);
    // The unrotated icon centre (108, -8) turned a quarter about (50, 50) lands at (108, 108).
    assert!(((r.x0 + r.x1) / 2.0 - 108.0).abs() < 1e-9 && ((r.y0 + r.y1) / 2.0 - 108.0).abs() < 1e-9, "{r:?}");
}

#[test]
fn a_point_hits_the_icon_only_when_the_element_has_a_link() {
    let mut e = linked(0.0, 0.0, 100.0, 50.0, 0.0);
    assert!(hits_icon(&e, Point::new(108.0, -8.0), 1.0));
    assert!(hits_icon(&e, Point::new(113.0, -8.0), 1.0), "the reach is 4 px beyond the 12 px square");
    assert!(!hits_icon(&e, Point::new(130.0, -8.0), 1.0));
    assert!(!hits_icon(&e, Point::new(50.0, 25.0), 1.0), "the body is not the icon");
    assert!(!hits_icon(&e, Point::new(f64::NAN, 0.0), 1.0));
    e.link = None;
    assert!(!hits_icon(&e, Point::new(108.0, -8.0), 1.0));
}
