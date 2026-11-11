//! Helpers shared by the `clipboard` and `image` families: where something goes (`at`), a document edit that
//! also needs the id source and the text measure, and the selection a paste leaves behind.

use std::collections::BTreeSet;

use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, GroupId, IdSource, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::refusal::Refusal;
use crate::services::Services;
use crate::session::Session;
use crate::store::DocTx;

pub use crate::args::Pt;

/// The words `at` accepts besides a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AtWord {
    /// The middle of the visible canvas.
    #[serde(rename = "viewport_center")]
    ViewportCenter,
    /// The pointer position. A shell replaces this word with a point; if it is left in, the middle of the visible
    /// canvas is used (the pointer is outside the canvas).
    #[serde(rename = "$cursor")]
    Cursor,
}

/// Where the middle of what is inserted goes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum At {
    /// A word.
    Word(AtWord),
    /// A world point `[x, y]`.
    Point(Pt),
}

impl Default for At {
    fn default() -> Self {
        At::Word(AtWord::ViewportCenter)
    }
}

/// The world point `at` names: a given point (checked), else the middle of the visible canvas.
pub(crate) fn resolve_at(s: &Session, at: &At) -> Result<Point, Refusal> {
    let id = s.cmd();
    match at {
        At::Point(p) => Ok(Point::new(args::world(id, "at[0]", p[0])?, args::world(id, "at[1]", p[1])?)),
        At::Word(_) => Ok(s.view().scroll),
    }
}

/// Runs `f` in one document transaction with the id source and the text measure at hand. The active tab, the
/// ports and the services are separate fields of the session, so they can be borrowed together here.
pub(crate) fn edit_with<R, E: From<StoreError>>(
    s: &mut Session,
    f: impl FnOnce(&mut DocTx<'_, '_>, &mut dyn IdSource, &Services) -> Result<R, E>,
) -> Result<R, E> {
    let idx = s.active_index();
    let Session { tabs, ports, services, .. } = s;
    let Some(tab) = tabs.get_mut(idx) else {
        return Err(StoreError::backend("the session has no active tab").into());
    };
    let services: &Services = services;
    let (r, _) = tab.doc.edit::<R, E>(|tx| f(tx, &mut *ports.ids, services))?;
    Ok(r)
}

/// Makes `ids` the selection: the outermost group of each is selected as a whole, and no group is being edited.
pub(crate) fn select_only(s: &mut Session, ids: &[ElementId]) {
    let groups: BTreeSet<GroupId> =
        ids.iter().filter_map(|i| s.element(i)).filter_map(|e| e.group_ids.last().cloned()).collect();
    let obs = &mut s.tab_mut().observed;
    obs.selection = ids.iter().cloned().collect();
    obs.selected_groups = groups;
    obs.editing_group = None;
    obs.linear_editor = None;
}
