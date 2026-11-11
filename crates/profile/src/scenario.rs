//! The names of the scenarios (`docs/performance.md` section 4.1) and their keys.
//!
//! A scenario key is `name@size` (`pan@10k`) or just `name` (`idle`). The app drivers parse the key the same
//! way on every platform, so a native run and a web run of `pan@10k` are the same scenario.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::report::{ScenarioReport, scenario_key};
use crate::stats::PerfStats;

/// What a scenario does while the window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScenarioKind {
    /// No input at all.
    Idle,
    /// A sinusoidal pan with the middle mouse button held.
    Pan,
    /// Mouse wheel ticks that scroll the canvas back and forth.
    Wheel,
    /// A ctrl+wheel sweep in and out.
    Zoom,
    /// A drag of a selection of 100 elements.
    Drag,
    /// Pointer moves over the canvas with no button pressed.
    Hover,
}

impl ScenarioKind {
    /// Every kind, in the order the docs list them.
    pub const ALL: [Self; 6] = [Self::Idle, Self::Pan, Self::Wheel, Self::Zoom, Self::Drag, Self::Hover];

    /// The name used in keys and reports: `idle`, `pan`, `wheel`, `zoom`, `drag`, `hover`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pan => "pan",
            Self::Wheel => "wheel",
            Self::Zoom => "zoom",
            Self::Drag => "drag",
            Self::Hover => "hover",
        }
    }

    /// How long the window is open unless the driver says otherwise, in seconds.
    pub const fn default_secs(self) -> f64 {
        match self {
            Self::Hover => 8.0,
            _ => 10.0,
        }
    }

    /// Does the scenario need a generated scene? Only `idle` runs without one.
    pub const fn needs_scene(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

impl fmt::Display for ScenarioKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A scenario key could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum KeyError {
    /// The name before `@` is not a scenario.
    #[error("`{0}` is not a scenario; the scenarios are idle, pan, wheel, zoom, drag and hover")]
    UnknownScenario(String),
    /// The size after `@` is not a number of elements.
    #[error("`{0}` is not a scene size; write 1k, 10k, 100k or a number of elements")]
    BadSize(String),
    /// The scenario draws on a scene but the key has no size.
    #[error("`{0}` needs a scene size, like `{0}@10k`")]
    MissingSize(ScenarioKind),
}

/// A scenario and the size of its scene: the key of a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScenarioKey {
    /// What it does.
    pub kind: ScenarioKind,
    /// Elements in the generated scene. `None` keeps the document the app started with (`idle` only).
    pub size: Option<u64>,
}

impl ScenarioKey {
    /// An empty report for this scenario around `stats`, tagged with the platform that ran it.
    pub fn report(&self, platform: &str, stats: PerfStats) -> ScenarioReport {
        let mut report = ScenarioReport::new(self.kind.name(), self.size, stats);
        report.meta.insert("platform".into(), platform.into());
        report
    }
}

impl fmt::Display for ScenarioKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&scenario_key(self.kind.name(), self.size))
    }
}

fn parse_size(text: &str) -> Result<u64, KeyError> {
    let bad = || KeyError::BadSize(text.to_owned());
    let lower = text.to_ascii_lowercase();
    let (digits, factor) = match lower.strip_suffix('k') {
        Some(d) => (d, 1_000),
        None => match lower.strip_suffix('m') {
            Some(d) => (d, 1_000_000),
            None => (lower.as_str(), 1),
        },
    };
    let n: u64 = digits.parse().map_err(|_| bad())?;
    n.checked_mul(factor).filter(|n| *n > 0).ok_or_else(bad)
}

impl FromStr for ScenarioKey {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, KeyError> {
        let (name, size) = match text.split_once('@') {
            Some((name, size)) => (name, Some(parse_size(size)?)),
            None => (text, None),
        };
        let kind = ScenarioKind::ALL
            .into_iter()
            .find(|k| k.name() == name)
            .ok_or_else(|| KeyError::UnknownScenario(name.to_owned()))?;
        if kind.needs_scene() && size.is_none() {
            return Err(KeyError::MissingSize(kind));
        }
        Ok(Self { kind, size })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_through_text() {
        for text in ["idle", "idle@10k", "pan@1k", "zoom@100k", "hover@10k", "drag@1k", "wheel@10k"] {
            let key: ScenarioKey = text.parse().unwrap();
            assert_eq!(key.to_string(), text);
        }
        assert_eq!("pan@2500".parse::<ScenarioKey>().unwrap().size, Some(2500));
        assert_eq!("pan@1m".parse::<ScenarioKey>().unwrap().size, Some(1_000_000));
    }

    #[test]
    fn bad_keys_say_what_is_wrong() {
        assert_eq!("fly@1k".parse::<ScenarioKey>(), Err(KeyError::UnknownScenario("fly".into())));
        assert_eq!("pan@lots".parse::<ScenarioKey>(), Err(KeyError::BadSize("lots".into())));
        assert_eq!("pan@0".parse::<ScenarioKey>(), Err(KeyError::BadSize("0".into())));
        assert_eq!("pan".parse::<ScenarioKey>(), Err(KeyError::MissingSize(ScenarioKind::Pan)));
        assert!("pan@1k".parse::<ScenarioKey>().is_ok());
    }

    #[test]
    fn the_report_key_is_the_scenario_key() {
        let key: ScenarioKey = "pan@10k".parse().unwrap();
        let report = key.report("web", PerfStats::empty());
        assert_eq!(report.key(), "pan@10k");
        assert_eq!(report.meta.get("platform").map(String::as_str), Some("web"));
    }
}
