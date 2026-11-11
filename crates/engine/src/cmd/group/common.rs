//! Helpers shared by the `group`, `frame` and `arrange` families: reading and writing the selection of the
//! session in the form the `edit` crate uses, the document edit with typed domain errors, and the refusals
//! that every structure command maps its domain errors to.

use drafftink_edit::group::selection::Selection as EditSelection;
use drafftink_edit::group::units::Relations;
use drafftink_edit::zorder::ZOrderError;
use drafftink_scene::{ElementId, GroupId, IdSource, StoreError};
use serde_json::json;

use crate::refusal::Refusal;
use crate::session::Session;
use crate::store::DocTx;

/// The selection of the active tab in the form of the `edit` crate.
pub(crate) fn current_selection(s: &Session) -> EditSelection {
    let o = &s.tab().observed;
    EditSelection {
        ids: o.selection.clone(),
        groups: o.selected_groups.clone(),
        editing_group: o.editing_group.clone(),
    }
}

/// Makes `sel` the selection, the selected groups and the editing group of the active tab.
pub(crate) fn apply_selection(s: &mut Session, sel: EditSelection) {
    let o = &mut s.tab_mut().observed;
    o.selection = sel.ids;
    o.selected_groups = sel.groups;
    o.editing_group = sel.editing_group;
}

/// The editing group of the active tab, if it still is a group (a group needs two live members).
pub(crate) fn editing_group(s: &Session) -> Option<GroupId> {
    let g = s.tab().observed.editing_group.clone()?;
    Relations::build(s.store()).is_group(&g).then_some(g)
}

/// The selected ids as a list, live elements only.
pub(crate) fn selected_ids(s: &Session) -> Vec<ElementId> {
    s.selection().iter().filter(|i| s.element(i).is_some()).cloned().collect()
}

/// The id port as an [`IdSource`] value (the domain functions are generic over `I: IdSource`).
pub(crate) struct Ids<'a>(pub &'a mut dyn IdSource);

impl IdSource for Ids<'_> {
    fn element_id(&mut self) -> ElementId {
        self.0.element_id()
    }
    fn group_id(&mut self) -> GroupId {
        self.0.group_id()
    }
    fn seed(&mut self) -> u32 {
        self.0.seed()
    }
}

/// Why a document edit failed: the store (a bug or a full scene) or a domain rule.
pub(crate) enum Failed<E> {
    /// The store refused a write.
    Store(StoreError),
    /// A rule of the `edit` crate refused.
    Domain(E),
}

impl<E> From<StoreError> for Failed<E> {
    fn from(e: StoreError) -> Self {
        Failed::Store(e)
    }
}

impl<E> Failed<E> {
    /// The refusal: `map` for a domain error, `internal` for a store error.
    pub(crate) fn refusal(self, cmd: &str, map: impl FnOnce(E) -> Refusal) -> Refusal {
        match self {
            Failed::Domain(e) => map(e),
            Failed::Store(e) => Refusal::internal(cmd, e.to_string()),
        }
    }
}

/// Runs `f` in one transaction on the active tab's document, with the id port at hand. Domain errors come
/// back as [`Failed::Domain`]; nothing is changed then.
pub(crate) fn edit<R, E>(
    s: &mut Session,
    f: impl FnOnce(&mut DocTx<'_, '_>, &mut Ids<'_>) -> Result<R, E>,
) -> Result<R, Failed<E>> {
    let idx = s.active_index();
    let Some(tab) = s.tabs.get_mut(idx) else {
        return Err(Failed::Store(StoreError::Backend("the session has no active tab".to_owned())));
    };
    let mut ids = Ids(&mut *s.ports.ids);
    let (r, _) = tab.doc.edit::<R, Failed<E>>(|tx| f(tx, &mut ids).map_err(Failed::Domain))?;
    Ok(r)
}

/// `locked` with every offending id in `details.ids`.
pub(crate) fn locked(cmd: &str, ids: &[ElementId]) -> Refusal {
    let first = ids.first().map_or("", ElementId::as_str);
    let all: Vec<&str> = ids.iter().map(ElementId::as_str).collect();
    Refusal::locked(cmd, first).with_details(json!({ "ids": all }))
}

/// `not_found` for an element id.
pub(crate) fn not_found(cmd: &str, id: &ElementId) -> Refusal {
    Refusal::not_found(cmd, id.as_str())
}

/// The refusal for a z-order error.
pub(crate) fn zorder_refusal(cmd: &str, e: ZOrderError) -> Refusal {
    match e {
        ZOrderError::NotFound(id) => not_found(cmd, &id),
        ZOrderError::Locked(ids) => locked(cmd, &ids),
        other => Refusal::internal(cmd, other.to_string()),
    }
}

/// `no_group`, the text of the catalog.
pub(crate) fn no_group(cmd: &str) -> Refusal {
    Refusal::custom(
        "no_group",
        cmd,
        "No group is selected. Select a group first (click it, or `selection.select` with `group`), or pass `group`.",
    )
    .suggesting("selection.select", None)
}
