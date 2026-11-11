//! What a new arrow looks like before it is drawn: its path type and the heads at its two ends.
//!
//! These are the tool's defaults for new arrows (Excalidraw's `currentItemArrowType`, `currentItemStartArrowhead`
//! and `currentItemEndArrowhead`). The arrow tool reads them when it creates an arrow. `element.create` does not:
//! it takes its own arguments, so a script gets the same arrow whatever the tool is set to.

use drafftink_scene::{Head, Kind, PathType};
use serde::Serialize;

use crate::create::{DEFAULT_ARROW_END_HEAD, LinearSpec};

/// Every head, in the order of the catalog. The match in [`name_of`] has no wildcard, so a new head cannot be
/// added without extending this list too.
pub const HEADS: [Head; 15] = [
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
];

/// Every path type.
pub const PATH_TYPES: [PathType; 3] = [PathType::Sharp, PathType::Round, PathType::Elbow];

/// The name of a head as it is written in JSON and in settings.
pub fn name_of(head: Head) -> &'static str {
    match head {
        Head::None => "none",
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
    }
}

/// The head called `name`, as [`name_of`] writes it.
pub fn head_named(name: &str) -> Option<Head> {
    HEADS.into_iter().find(|h| name_of(*h) == name)
}

/// The name of a path type as it is written in JSON and in settings.
pub fn path_name(path: PathType) -> &'static str {
    match path {
        PathType::Sharp => "sharp",
        PathType::Round => "round",
        PathType::Elbow => "elbow",
    }
}

/// The path type called `name`, as [`path_name`] writes it.
pub fn path_named(name: &str) -> Option<PathType> {
    PATH_TYPES.into_iter().find(|p| path_name(*p) == name)
}

/// The path type and the two heads a new arrow gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ArrowDefaults {
    /// How the points are joined.
    pub path: PathType,
    /// The head at the first point.
    pub start_head: Head,
    /// The head at the last point.
    pub end_head: Head,
}

impl Default for ArrowDefaults {
    /// Straight segments, no start head and an open V at the end (the old app's arrow).
    fn default() -> Self {
        Self { path: PathType::Sharp, start_head: Head::None, end_head: DEFAULT_ARROW_END_HEAD }
    }
}

impl ArrowDefaults {
    /// The spec of a new arrow with these defaults.
    pub fn spec(&self) -> LinearSpec {
        LinearSpec {
            path: self.path,
            start_head: self.start_head,
            end_head: self.end_head,
            ..LinearSpec::defaults(Kind::Arrow)
        }
    }

    /// Sets the parts that are `Some`. Returns whether anything changed.
    pub fn set(&mut self, path: Option<PathType>, start_head: Option<Head>, end_head: Option<Head>) -> bool {
        let before = *self;
        self.path = path.unwrap_or(self.path);
        self.start_head = start_head.unwrap_or(self.start_head);
        self.end_head = end_head.unwrap_or(self.end_head);
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_old_arrow() {
        let d = ArrowDefaults::default();
        assert_eq!((d.path, d.start_head, d.end_head), (PathType::Sharp, Head::None, Head::Arrow));
        assert_eq!(d.spec(), LinearSpec::defaults(Kind::Arrow));
    }

    #[test]
    fn names_match_the_serde_names_and_round_trip() {
        for h in HEADS {
            let json = serde_json::to_value(h).unwrap();
            assert_eq!(json.as_str(), Some(name_of(h)));
            assert_eq!(head_named(name_of(h)), Some(h));
        }
        for p in PATH_TYPES {
            assert_eq!(serde_json::to_value(p).unwrap().as_str(), Some(path_name(p)));
            assert_eq!(path_named(path_name(p)), Some(p));
        }
        assert_eq!(head_named("spiky"), None);
        assert_eq!(path_named("wavy"), None);
    }

    #[test]
    fn set_reports_whether_anything_changed() {
        let mut d = ArrowDefaults::default();
        assert!(!d.set(None, None, None));
        assert!(!d.set(Some(PathType::Sharp), Some(Head::None), Some(Head::Arrow)));
        assert!(d.set(Some(PathType::Elbow), None, Some(Head::Triangle)));
        assert_eq!((d.path, d.start_head, d.end_head), (PathType::Elbow, Head::None, Head::Triangle));
        assert_eq!(d.spec().path, PathType::Elbow);
    }
}
