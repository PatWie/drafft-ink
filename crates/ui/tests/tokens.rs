#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The type scale is the only source of font sizes in `src/surfaces`: a surface asks for `TextSize::Body`, never
//! for `13.0`. A new literal size there fails this test; use a `TextSize` (`src/theme/tokens.rs`).

use std::path::{Path, PathBuf};

/// Calls that take a size as a number.
const SIZE_CALLS: [&str; 6] = [".size(", "::proportional(", "::monospace(", "::hand(", "::named(", "FontId::new("];

/// The surfaces that still hold literal sizes, with how many each. A count may go down, never up.
///
/// * `library.rs` and the rows of `menu.rs` belong to work in parallel with the top strip (the shared menu-row
///   painter, the library panel); they move to `TextSize` when that work lands.
const ALLOWED: [(&str, usize); 2] = [("library.rs", 6), ("menu.rs", 3)];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// How many calls in `text` pass a number where a size goes: `.size(11.0)`, `proportional(13.0)`,
/// `named(family, 13.0)`, `FontId::new(18.0, ..)`.
fn literal_sizes(text: &str) -> usize {
    let is_number = |rest: &str| rest.trim_start().starts_with(|c: char| c.is_ascii_digit());
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| {
            SIZE_CALLS
                .iter()
                .flat_map(|call| line.match_indices(call).map(move |(at, _)| (*call, &line[at + call.len()..])))
                .filter(|(call, rest)| match *call {
                    "::named(" => rest.split_once(',').is_some_and(|(_, size)| is_number(size)),
                    _ => is_number(rest),
                })
                .count()
        })
        .sum()
}

#[test]
fn surfaces_take_font_sizes_from_the_type_scale() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces"), &mut files);
    assert!(files.len() > 20, "found the surfaces");
    let mut over = Vec::new();
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let found = literal_sizes(&std::fs::read_to_string(file).unwrap());
        let allowed = ALLOWED.iter().find(|(n, _)| *n == name).map_or(0, |(_, count)| *count);
        if found > allowed {
            over.push(format!("{name}: {found} literal font sizes, {allowed} allowed"));
        }
    }
    assert!(over.is_empty(), "use TextSize instead of a number:\n{}", over.join("\n"));
}

#[test]
fn the_scanner_sees_the_calls_it_is_meant_to_catch() {
    assert_eq!(literal_sizes("RichText::new(t).size(11.0)"), 1);
    assert_eq!(literal_sizes("egui::FontId::proportional(13.0)"), 1);
    assert_eq!(literal_sizes("theme::hand( 14.0 )"), 1);
    assert_eq!(literal_sizes("theme::named(theme::PREVIEW_SERIF, 13.0)"), 1);
    assert_eq!(literal_sizes("FontId::new(18.0, family)"), 1);
    assert_eq!(literal_sizes("RichText::new(t).size(TextSize::Small.points())"), 0);
    assert_eq!(literal_sizes("theme::named(theme::PREVIEW_SERIF, TextSize::Body.points())"), 0);
    assert_eq!(literal_sizes("// .size(11.0) in a comment"), 0);
}
