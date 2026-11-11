//! Collaborators as the page describes them (`stage.setRemoteUsers([...])`): the stage draws a pointer with a name,
//! an outline round what each person has selected, and their laser trail, all in the person's colour.

use drafftink_canvas::{RemoteUser, TrailPoint};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Color, ElementId};
use serde::Deserialize;

use crate::error::ApiError;

/// One person in the room.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RemoteUserSpec {
    /// A stable id for the person.
    pub id: String,
    /// Shown in a label beside the pointer.
    #[serde(default)]
    pub name: Option<String>,
    /// `#rrggbb` or `#rrggbbaa`.
    pub color: String,
    /// The pointer in world units, `[x, y]`. Absent when the pointer is outside the canvas.
    #[serde(default)]
    pub cursor: Option<[f64; 2]>,
    /// Ids of the shapes they have selected. Unknown ids are skipped.
    #[serde(default)]
    pub selection: Vec<String>,
    /// The recent path of the pointer, oldest first.
    #[serde(default)]
    pub trail: Vec<TrailSpec>,
}

/// A point of a trail.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TrailSpec {
    /// World x.
    pub x: f64,
    /// World y.
    pub y: f64,
    /// Milliseconds since the point was recorded.
    pub age_ms: f64,
}

impl RemoteUserSpec {
    /// The canvas crate's description of this person.
    pub(crate) fn into_user(self) -> Result<RemoteUser, ApiError> {
        let color = Color::parse(&self.color)
            .map_err(|e| ApiError::invalid(format!("the colour of remote user {}: {e}", self.id)))?;
        let finite =
            |v: f64| if v.is_finite() { Ok(v) } else { Err(ApiError::invalid("remote positions must be finite")) };
        let cursor = self.cursor.map(|[x, y]| Ok::<_, ApiError>(Point::new(finite(x)?, finite(y)?))).transpose()?;
        let trail = self
            .trail
            .iter()
            .map(|p| Ok(TrailPoint { pos: Point::new(finite(p.x)?, finite(p.y)?), age_ms: finite(p.age_ms)? }))
            .collect::<Result<Vec<_>, ApiError>>()?;
        // An id that is not an element id cannot name an element: it is skipped, not an error.
        let selection = self.selection.iter().filter_map(|s| ElementId::new(s).ok()).collect();
        Ok(RemoteUser { id: self.id, name: self.name, color, cursor, selection, trail })
    }
}
