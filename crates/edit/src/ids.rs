//! Fresh ids: ask the id source until it returns one that is not taken.

use drafftink_scene::{ElementId, GroupId, IdSource, SceneRead};

/// How often the id source is asked before its ids count as used up.
const ATTEMPTS: usize = 64;

/// The id source kept returning ids that are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Exhausted;

/// An element id for which `taken` is false.
pub(crate) fn fresh_element_id(
    src: &mut dyn IdSource,
    taken: impl Fn(&ElementId) -> bool,
) -> Result<ElementId, Exhausted> {
    (0..ATTEMPTS).map(|_| src.element_id()).find(|id| !taken(id)).ok_or(Exhausted)
}

/// A group id for which `taken` is false.
pub(crate) fn fresh_group_id(src: &mut dyn IdSource, taken: impl Fn(&GroupId) -> bool) -> Result<GroupId, Exhausted> {
    (0..ATTEMPTS).map(|_| src.group_id()).find(|id| !taken(id)).ok_or(Exhausted)
}

/// An element id that no element of `scene` uses (live or deleted). A source that keeps returning used ids
/// makes this return one of them after a bound; the insert then reports it.
pub fn unused_element_id<R: SceneRead>(scene: &R, src: &mut dyn IdSource) -> ElementId {
    fresh_element_id(src, |id| scene.contains(id)).unwrap_or_else(|_| src.element_id())
}
