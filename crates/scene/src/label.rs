//! Which text is the label of a container.
//!
//! A text names its container (`container_id`), and nothing stops two live texts from naming the same one: two
//! peers can each add a label while offline. Every reader has to pick the same one, so the rule is here and
//! nowhere else: **the live text with the smallest id is the label.** The others are claims that lose. (The repair
//! pass of `drafftink-edit` unbinds them once it sees them; until then every reader agrees on the winner.)
//!
//! * [`winner`] is the rule over the ids of the claims.
//! * [`by_container`] applies it to a list of elements: every container that has a label, with its label.
//! * [`SceneRead::label_of`](crate::SceneRead::label_of) asks one container, from the derived index when the
//!   reader has one ([`DerivedIndex::label_of`](crate::DerivedIndex::label_of)) and by a scan otherwise.

use std::collections::HashMap;

use crate::element::Element;
use crate::id::ElementId;

/// The claim that wins among the ids of the live texts that name one container: the smallest.
pub fn winner<'a>(claims: impl IntoIterator<Item = &'a ElementId>) -> Option<&'a ElementId> {
    claims.into_iter().min()
}

/// The container a live element claims as its label, if it does.
pub(crate) fn claimed_by(e: &Element) -> Option<&ElementId> {
    if e.deleted { None } else { e.container_id() }
}

/// The label of every container that has one: of the live texts in `elements`, the winner of each container (see
/// [`winner`]). A container that no live text names is not in the map. The container itself need not be among
/// `elements` or be alive: that is for the caller to check.
pub fn by_container<'a>(elements: impl IntoIterator<Item = &'a Element>) -> HashMap<&'a ElementId, &'a Element> {
    let mut claims: HashMap<&ElementId, Vec<&Element>> = HashMap::new();
    for e in elements {
        if let Some(container) = claimed_by(e) {
            claims.entry(container).or_default().push(e);
        }
    }
    claims
        .into_iter()
        .filter_map(|(container, texts)| {
            let id = winner(texts.iter().map(|t| &t.id))?;
            texts.iter().find(|t| t.id == *id).map(|t| (container, *t))
        })
        .collect()
}

/// The label of `container` among `elements`, by a scan: the winner of the live texts that name it.
pub(crate) fn scan<'a>(
    elements: impl IntoIterator<Item = &'a Element>,
    container: &ElementId,
) -> Option<&'a ElementId> {
    winner(elements.into_iter().filter(|e| claimed_by(e) == Some(container)).map(|e| &e.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{Element, ElementKind, TextData};
    use crate::memory::MemoryStore;
    use crate::store::{SceneRead, SceneStore, SceneTx};
    use crate::zindex::ZIndex;

    fn id(s: &str) -> ElementId {
        ElementId::new(s).unwrap()
    }

    fn rect(name: &str) -> Element {
        Element::new(id(name), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 100.0, 50.0)
    }

    /// A text that names `container`, with the z-order key `z`.
    fn text(name: &str, container: &str, z: ZIndex) -> Element {
        let mut t = TextData::new(name);
        t.container_id = Some(id(container));
        Element::new(id(name), ElementKind::Text(t), z).with_bounds(0.0, 0.0, 10.0, 10.0)
    }

    fn keys(n: usize) -> Vec<ZIndex> {
        ZIndex::rebalance(n).unwrap()
    }

    #[test]
    fn the_smallest_id_wins_whatever_the_order_the_claims_come_in() {
        let (a, b, c) = (id("a"), id("b"), id("c"));
        for claims in [[&a, &b, &c], [&c, &b, &a], [&b, &c, &a]] {
            assert_eq!(winner(claims), Some(&a));
        }
        assert_eq!(winner([&b]), Some(&b));
        assert_eq!(winner([]), None);
    }

    #[test]
    fn a_container_with_several_texts_has_the_smallest_id_as_its_label_whatever_their_z_order() {
        let z = keys(4);
        // "t2" is lowest in the z-order and "t1" is on top: the order does not decide, the id does.
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        for e in [
            rect("box"),
            text("t2", "box", z[1].clone()),
            text("t3", "box", z[2].clone()),
            text("t1", "box", z[3].clone()),
        ] {
            tx.insert(e).unwrap();
        }
        tx.commit().unwrap();
        assert_eq!(store.label_of(&id("box")), Some(&id("t1")), "with the derived index");
        assert_eq!(by_container(store.iter()).get(&id("box")).map(|e| &e.id), Some(&id("t1")), "by container");
        assert_eq!(scan(store.iter(), &id("box")), Some(&id("t1")), "by a scan");
        // An open transaction has no index after its first write: the reader scans, and agrees.
        let mut tx = store.begin();
        tx.insert(text("t0", "box", z[0].clone())).unwrap();
        assert!(tx.derived_index().is_none());
        assert_eq!(tx.label_of(&id("box")), Some(&id("t0")), "inside a transaction that has written");
    }

    #[test]
    fn a_deleted_text_is_no_claim_and_a_container_without_texts_has_no_label() {
        let z = keys(3);
        let mut dead = text("a", "box", z[1].clone());
        dead.deleted = true;
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        for e in [rect("box"), rect("other"), dead, text("b", "box", z[2].clone())] {
            tx.insert(e).unwrap();
        }
        tx.commit().unwrap();
        assert_eq!(store.label_of(&id("box")), Some(&id("b")), "the deleted `a` does not win");
        assert_eq!(store.label_of(&id("other")), None);
        let map = by_container(store.iter_all());
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&id("box")).map(|e| &e.id), Some(&id("b")));
        assert_eq!(scan(store.iter_all(), &id("other")), None);
    }

    #[test]
    fn every_container_gets_its_own_winner() {
        let z = keys(5);
        let texts = [text("p2", "one", z[1].clone()), text("p1", "one", z[2].clone()), text("q", "two", z[3].clone())];
        let all: Vec<Element> = [rect("one"), rect("two")].into_iter().chain(texts).collect();
        let map = by_container(&all);
        let mut pairs: Vec<(&str, &str)> = map.iter().map(|(c, t)| (c.as_str(), t.id.as_str())).collect();
        pairs.sort_unstable();
        assert_eq!(pairs, [("one", "p1"), ("two", "q")]);
    }
}
