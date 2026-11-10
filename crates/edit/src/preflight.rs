//! Pre-checks that a control can run before the operation: "are there enough units to group, align or
//! distribute", "is there a group among the selection".
//!
//! The operations in [`crate::group`] and [`crate::arrange`] use the same functions and constants, so a greyed-out
//! button and the refusal of the command always agree, and the thresholds exist once.

use drafftink_scene::{ElementId, GroupId, SceneRead};

use crate::arrange::ArrangeError;
use crate::group::GroupError;
use crate::group::units::{Relations, TargetError, Unit, UnitKey, primaries, units};

/// Units needed to group.
pub const MIN_GROUP_UNITS: usize = 2;
/// Units needed to align without a reference element.
pub const MIN_ALIGN_UNITS: usize = 2;
/// Units needed to distribute.
pub const MIN_DISTRIBUTE_UNITS: usize = 3;

/// The units of `prim` (already primaries) when they can form a new group: at least [`MIN_GROUP_UNITS`], and not
/// one single existing group.
pub fn groupable_units<R: SceneRead>(
    scene: &R,
    rel: &Relations,
    prim: &[ElementId],
    editing: Option<&GroupId>,
) -> Result<Vec<Unit>, GroupError> {
    let us = units(scene, rel, prim, editing)?;
    if us.len() < MIN_GROUP_UNITS {
        return match us.first().map(|u| &u.key) {
            Some(UnitKey::Group(_)) => Err(GroupError::AlreadyGrouped),
            _ => Err(GroupError::TooFew { have: us.len() }),
        };
    }
    Ok(us)
}

/// Checks that `ids` can be grouped (enough units, not one group already), without changing anything.
pub fn check_group<R: SceneRead>(scene: &R, ids: &[ElementId], editing: Option<&GroupId>) -> Result<(), GroupError> {
    let rel = Relations::build(scene);
    let prim = primaries(scene, ids)?;
    groupable_units(scene, &rel, &prim, editing).map(|_| ())
}

/// The groups that `ids` stand for at the level of `editing`: an element of a group stands for its group.
pub fn groups_of<R: SceneRead>(
    scene: &R,
    rel: &Relations,
    ids: &[ElementId],
    editing: Option<&GroupId>,
) -> Result<Vec<GroupId>, TargetError> {
    let prim = primaries(scene, ids)?;
    Ok(units(scene, rel, &prim, editing)?
        .into_iter()
        .filter_map(|u| match u.key {
            UnitKey::Group(g) => Some(g),
            UnitKey::Element(_) => None,
        })
        .collect())
}

/// Checks that `ids` give at least [`MIN_ALIGN_UNITS`] units to align.
pub fn check_align<R: SceneRead>(scene: &R, ids: &[ElementId], editing: Option<&GroupId>) -> Result<(), ArrangeError> {
    check_count(scene, ids, editing, "align", MIN_ALIGN_UNITS)
}

/// Checks that `ids` give at least [`MIN_DISTRIBUTE_UNITS`] units to distribute.
pub fn check_distribute<R: SceneRead>(
    scene: &R,
    ids: &[ElementId],
    editing: Option<&GroupId>,
) -> Result<(), ArrangeError> {
    check_count(scene, ids, editing, "distribute", MIN_DISTRIBUTE_UNITS)
}

fn check_count<R: SceneRead>(
    scene: &R,
    ids: &[ElementId],
    editing: Option<&GroupId>,
    what: &'static str,
    need: usize,
) -> Result<(), ArrangeError> {
    let rel = Relations::build(scene);
    let have = units(scene, &rel, ids, editing)?.len();
    if have < need { Err(ArrangeError::TooFew { what, need, have }) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{Element, ElementKind, MemoryStore, SceneStore, SceneTx, Transform, ZIndex};

    use super::*;

    fn scene(n: usize) -> (MemoryStore, Vec<ElementId>) {
        let mut st = MemoryStore::new();
        let mut ids = Vec::new();
        let mut key: Option<ZIndex> = None;
        for i in 0..n {
            let id = ElementId::new(&format!("r{i}")).unwrap();
            let z = ZIndex::between(key.as_ref(), None).unwrap();
            key = Some(z.clone());
            let mut e = Element::new(id.clone(), ElementKind::Rectangle, z);
            e.transform = Transform::new(i as f64 * 50.0, 0.0, 40.0, 40.0);
            let mut tx = st.begin();
            tx.insert(e).unwrap();
            tx.commit().unwrap();
            ids.push(id);
        }
        (st, ids)
    }

    #[test]
    fn counts_are_checked_with_the_shared_thresholds() {
        let (st, ids) = scene(3);
        assert!(matches!(check_group(&st, &ids[..1], None), Err(GroupError::TooFew { have: 1 })));
        assert!(check_group(&st, &ids[..2], None).is_ok());
        assert!(matches!(check_align(&st, &ids[..1], None), Err(ArrangeError::TooFew { need: 2, have: 1, .. })));
        assert!(check_align(&st, &ids[..2], None).is_ok());
        assert!(matches!(check_distribute(&st, &ids[..2], None), Err(ArrangeError::TooFew { need: 3, have: 2, .. })));
        assert!(check_distribute(&st, &ids, None).is_ok());
    }
}
