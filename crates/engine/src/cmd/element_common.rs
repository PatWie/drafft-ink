//! Helpers shared by the `element` and `selection` command families (and by other families that act on
//! elements and the selection). Nothing here holds a domain rule; the rules are in `drafftink-edit`.

use drafftink_edit::group::selection::{Selection as EditSelection, select_ids};
use drafftink_scene::ElementId;
use serde_json::json;

use crate::refusal::Refusal;
use crate::session::Session;

/// `locked` for several elements: the catalog text names the first one, `details.ids` lists all of them.
pub(crate) fn locked_refusal(cmd: &str, ids: &[ElementId]) -> Refusal {
    let first = ids.first().map_or("", ElementId::as_str);
    Refusal::locked(cmd, first)
        .with_details(json!({ "ids": ids.iter().map(|i| i.as_str().to_owned()).collect::<Vec<_>>() }))
}

/// The observed selection of the active tab in the form the edit crate takes.
pub(crate) fn edit_selection(s: &Session) -> EditSelection {
    let o = &s.tab().observed;
    EditSelection {
        ids: o.selection.clone(),
        groups: o.selected_groups.clone(),
        editing_group: o.editing_group.clone(),
    }
}

/// Writes an edit-crate selection into the active tab's observed state. The point editor is left when its
/// element is no longer selected.
pub(crate) fn apply_selection(s: &mut Session, sel: &EditSelection) {
    let o = &mut s.tab_mut().observed;
    o.selection = sel.ids.clone();
    o.selected_groups = sel.groups.clone();
    o.editing_group = sel.editing_group.clone();
    if o.linear_editor.as_ref().is_some_and(|l| !o.selection.contains(&l.element)) {
        o.linear_editor = None;
    }
}

/// Selects exactly these elements as the normalised selection (bound text becomes its container, the
/// children of a selected frame are dropped, groups are derived). The editing group is kept when `keep_editing`.
pub(crate) fn select_exactly(s: &mut Session, ids: &[ElementId], keep_editing: bool) {
    let editing = if keep_editing { s.tab().observed.editing_group.clone() } else { None };
    let sel = select_ids(s.store(), ids, editing.as_ref());
    apply_selection(s, &sel);
}

/// Re-normalises the current selection after a document change (deleted and locked elements drop out, an
/// editing group that is no longer a group is left).
pub(crate) fn prune_selection(s: &mut Session) {
    let cur = edit_selection(s);
    let pruned = drafftink_edit::group::selection::prune(s.store(), &cur);
    apply_selection(s, &pruned);
}

/// Ids as strings, for command output.
pub(crate) fn id_strings<'a>(ids: impl IntoIterator<Item = &'a ElementId>) -> Vec<String> {
    ids.into_iter().map(|i| i.as_str().to_owned()).collect()
}

/// Runs `f` in one document transaction of the active tab together with the session's id source, so that a
/// command can draw ids and seeds inside the transaction (the session cannot be borrowed twice through
/// `tab_mut()`). Returns what `f` returns; an `Err` leaves the document unchanged.
pub(crate) fn edit_with_ids<R, E: From<drafftink_scene::StoreError>>(
    s: &mut Session,
    f: impl FnOnce(&mut crate::store::DocTx<'_, '_>, &mut dyn drafftink_scene::IdSource) -> Result<R, E>,
) -> Result<R, E> {
    let idx = s.active_index();
    let ids: &mut dyn drafftink_scene::IdSource = &mut *s.ports.ids;
    let Some(tab) = s.tabs.get_mut(idx) else {
        return Err(drafftink_scene::StoreError::Backend("there is no active tab".to_owned()).into());
    };
    tab.doc.edit(|tx| f(tx, ids)).map(|(r, _)| r)
}

/// Like [`edit_with_ids`], and also hands out the text measure of the session (`Session::services`), which a
/// borrow of the whole session could not do next to the transaction.
pub(crate) fn edit_with_measure<R, E: From<drafftink_scene::StoreError>>(
    s: &mut Session,
    f: impl FnOnce(
        &mut crate::store::DocTx<'_, '_>,
        &dyn drafftink_edit::text_bind::TextMeasure,
        &mut dyn drafftink_scene::IdSource,
    ) -> Result<R, E>,
) -> Result<R, E> {
    let idx = s.active_index();
    let measure = s.services.edit();
    let ids: &mut dyn drafftink_scene::IdSource = &mut *s.ports.ids;
    let Some(tab) = s.tabs.get_mut(idx) else {
        return Err(drafftink_scene::StoreError::Backend("there is no active tab".to_owned()).into());
    };
    tab.doc.edit(|tx| f(tx, measure, ids)).map(|(r, _)| r)
}
