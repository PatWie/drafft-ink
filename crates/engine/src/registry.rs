//! The registry: every command spec, lookup by id, listing with live availability.
//!
//! The global registry is the plain list in `cmd::FAMILIES`. Tests and tools can build a [`Registry`]
//! from any slices, which is how the executor is tested with synthetic commands.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::Serialize;

use crate::refusal::{Refusal, closest};
use crate::session::Session;
use crate::spec::CommandSpec;

/// A set of command specs with an id index.
pub struct Registry {
    specs: Vec<&'static CommandSpec>,
    by_id: HashMap<&'static str, usize>,
}

impl Registry {
    /// A registry over family slices. A repeated id keeps the first (a test fails on duplicates).
    pub fn from_families(families: &[&'static [CommandSpec]]) -> Self {
        let mut specs = Vec::new();
        let mut by_id = HashMap::new();
        for fam in families {
            for spec in fam.iter() {
                by_id.entry(spec.id).or_insert(specs.len());
                specs.push(spec);
            }
        }
        Self { specs, by_id }
    }

    /// The shared registry of all families.
    pub fn global() -> Arc<Registry> {
        static GLOBAL: OnceLock<Arc<Registry>> = OnceLock::new();
        GLOBAL.get_or_init(|| Arc::new(Registry::from_families(crate::cmd::FAMILIES))).clone()
    }

    /// The spec with this id.
    pub fn find(&self, id: &str) -> Option<&'static CommandSpec> {
        self.by_id.get(id).and_then(|&i| self.specs.get(i)).copied()
    }

    /// Every spec in registration order (duplicates included, so a test can see them).
    pub fn all(&self) -> &[&'static CommandSpec] {
        &self.specs
    }

    /// Number of specs.
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Distinct family names in registration order.
    pub fn families(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for s in &self.specs {
            if !out.contains(&s.family) {
                out.push(s.family);
            }
        }
        out
    }

    /// Up to three ids that look like `wanted`.
    pub fn suggestions(&self, wanted: &str) -> Vec<String> {
        closest(wanted, self.specs.iter().map(|s| s.id), 3)
    }

    /// The refusal for an unknown id.
    pub fn unknown(&self, id: &str) -> Refusal {
        Refusal::unknown_command(id, &self.suggestions(id))
    }
}

/// A command with its availability right now, for palettes, toolbars and `engine.commands`.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    /// The id.
    pub id: &'static str,
    /// The title.
    pub title: &'static str,
    /// The family.
    pub family: &'static str,
    /// The summary.
    pub summary: &'static str,
    /// `Ok(())` when the command can run now, else the refusal that `execute` would return.
    #[serde(serialize_with = "ser_availability")]
    pub available: Result<(), Refusal>,
}

fn ser_availability<S: serde::Serializer>(a: &Result<(), Refusal>, s: S) -> Result<S::Ok, S::Error> {
    match a {
        Ok(()) => s.serialize_bool(true),
        Err(r) => {
            use serde::ser::SerializeMap;
            let mut m = s.serialize_map(Some(2))?;
            m.serialize_entry("code", r.code)?;
            m.serialize_entry("message", &r.message)?;
            m.end()
        }
    }
}

impl Session {
    /// Every command with its availability for the current state.
    pub fn commands(&self) -> Vec<CommandInfo> {
        self.registry()
            .all()
            .iter()
            .map(|s| CommandInfo {
                id: s.id,
                title: s.title,
                family: s.family,
                summary: s.summary,
                available: self.availability_of(s),
            })
            .collect()
    }
}
