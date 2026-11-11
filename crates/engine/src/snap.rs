//! The `snap` argument shared by the commands that drag things: `arrow.move_point`, `element.move` and
//! `element.resize` (docs/commands.md `Snap`).

use drafftink_edit::arrow::SnapFlags;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::Session;

/// The words `snap` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapWord {
    /// Follow the settings `snap.grid`, `snap.smart_guides` and `snap.angle`.
    Auto,
    /// No snapping.
    None,
}

/// Snap switches; a missing key follows its setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SnapSwitches {
    /// Snap to the 20 unit grid.
    pub grid: Option<bool>,
    /// Smart guides: snap to the edges and centres of other elements, and to equal spacing.
    pub smart: Option<bool>,
    /// Lock the direction to a multiple of the angle step (`snap.angle_step`, 15 degrees). Used by commands
    /// that have a direction.
    pub angle: Option<bool>,
}

/// `auto`, `none`, or switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Snap {
    /// `"auto"` or `"none"`.
    Word(SnapWord),
    /// `{grid?, smart?, angle?}`.
    Switches(SnapSwitches),
}

impl Default for Snap {
    fn default() -> Self {
        Snap::Word(SnapWord::Auto)
    }
}

impl Snap {
    /// The snaps that are on after the settings are read.
    pub(crate) fn resolve(self, s: &Session) -> SnapFlags {
        let setting = SnapFlags {
            grid: s.settings.bool_or("snap.grid", false),
            smart: s.settings.bool_or("snap.smart_guides", false),
            angle: s.settings.bool_or("snap.angle", false),
        };
        match self {
            Snap::Word(SnapWord::Auto) => setting,
            Snap::Word(SnapWord::None) => SnapFlags::default(),
            Snap::Switches(w) => SnapFlags {
                grid: w.grid.unwrap_or(setting.grid),
                smart: w.smart.unwrap_or(setting.smart),
                angle: w.angle.unwrap_or(setting.angle),
            },
        }
    }
}
