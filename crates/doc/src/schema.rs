//! Schema constants, the schema version and the migration hook.
//!
//! The version lives in the document (`meta.schema`), so a document says which layout it has. A build opens
//! its own version and older ones (after migrating), and refuses newer ones with "Update the app".

use loro::{LoroDoc, LoroValue};

use crate::error::DocError;

/// Root map: elements by id.
pub(crate) const ROOT_ELEMENTS: &str = "elements";
/// Root map: schema version, name, background.
pub(crate) const ROOT_META: &str = "meta";
/// Root map: image blobs by content hash.
pub(crate) const ROOT_FILES: &str = "files";
/// Key in `meta`: schema version (integer).
pub(crate) const META_SCHEMA: &str = "schema";
/// Key in `meta`: document name.
pub(crate) const META_NAME: &str = "name";
/// Key in `meta`: background colour, `#rrggbb[aa]`.
pub(crate) const META_BACKGROUND: &str = "background";

/// The schema version this build writes and reads without migration.
pub const SCHEMA_VERSION: u32 = 1;

/// Commit origins starting with this prefix are not recorded by the undo manager.
pub(crate) const SYSTEM_ORIGIN_PREFIX: &str = "sys:";

/// One migration step: upgrades a document whose schema version is `from` to `from + 1`, in place, with plain
/// Loro operations. The store commits all steps of one open as a single system commit (not undoable).
///
/// A step must be idempotent for concurrent runs: two peers that open the same old document both migrate it, and
/// the CRDT merges their writes. Write the same values for the same keys (derive them from the data, never from
/// the clock or from randomness), and prefer adding keys to rewriting them.
#[derive(Clone, Copy)]
pub struct Migration {
    /// The version this step upgrades from.
    pub from: u32,
    /// The step.
    pub apply: fn(&LoroDoc) -> Result<(), DocError>,
}

/// The migrations this build knows. Empty: version 1 is the first schema. When version 2 exists, a step with
/// `from: 1` goes here, and `SCHEMA_VERSION` is raised.
pub const MIGRATIONS: &[Migration] = &[];

/// Reads the schema version of a document. `None` when there is none.
pub(crate) fn read_version(doc: &LoroDoc) -> Option<u32> {
    let meta = doc.get_map(ROOT_META);
    match meta.get(META_SCHEMA)?.into_value().ok()? {
        LoroValue::I64(v) => Some(u32::try_from(v).unwrap_or(u32::MAX)),
        LoroValue::Double(v) if v >= 0.0 && v <= f64::from(u32::MAX) && v.fract() == 0.0 => Some(v as u32),
        _ => Some(u32::MAX),
    }
}

/// True when the document has no data at all (a fresh `LoroDoc`).
pub(crate) fn is_blank(doc: &LoroDoc) -> bool {
    match doc.get_deep_value() {
        LoroValue::Map(m) => m.values().all(|v| match v {
            LoroValue::Map(inner) => inner.is_empty(),
            LoroValue::List(l) => l.is_empty(),
            LoroValue::String(s) => s.is_empty(),
            LoroValue::Null => true,
            _ => false,
        }),
        _ => true,
    }
}

/// Brings `doc` from `found` up to [`SCHEMA_VERSION`] using `table`. Returns whether anything was changed.
pub(crate) fn migrate(doc: &LoroDoc, found: u32, table: &[Migration]) -> Result<bool, DocError> {
    migrate_to(doc, found, SCHEMA_VERSION, table)
}

/// Like [`migrate`] with an explicit target version (tests use it to exercise the hook with a fake version 2).
pub(crate) fn migrate_to(doc: &LoroDoc, found: u32, target: u32, table: &[Migration]) -> Result<bool, DocError> {
    if found > target {
        return Err(DocError::NewerSchema { found, max: target });
    }
    if found == target {
        return Ok(false);
    }
    let mut v = found;
    while v < target {
        let step = table.iter().find(|m| m.from == v).ok_or_else(|| DocError::Migration {
            from: v,
            reason: format!("no migration from version {v} is known"),
        })?;
        (step.apply)(doc).map_err(|e| DocError::Migration { from: v, reason: e.to_string() })?;
        v += 1;
        doc.get_map(ROOT_META)
            .insert(META_SCHEMA, i64::from(v))
            .map_err(|e| DocError::Migration { from: v - 1, reason: e.to_string() })?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_at(version: i64) -> LoroDoc {
        let doc = LoroDoc::new();
        doc.get_map(ROOT_META).insert(META_SCHEMA, version).unwrap();
        doc.get_map(ROOT_ELEMENTS).ensure_mergeable_map("a").unwrap().insert("x", 1.0).unwrap();
        doc.commit();
        doc
    }

    fn v1_to_v2(doc: &LoroDoc) -> Result<(), DocError> {
        // A pretend change: version 2 renames the key `x` to `pos_x`.
        let m = doc.get_map(ROOT_ELEMENTS).ensure_mergeable_map("a")?;
        if let Some(loro::ValueOrContainer::Value(v)) = m.get("x") {
            m.insert("pos_x", v)?;
            m.delete("x")?;
        }
        Ok(())
    }

    #[test]
    fn steps_run_in_order_and_the_version_is_bumped() {
        let doc = doc_at(1);
        let table = [Migration { from: 1, apply: v1_to_v2 }];
        assert!(migrate_to(&doc, 1, 2, &table).unwrap());
        assert_eq!(read_version(&doc), Some(2));
        let a = doc.get_map(ROOT_ELEMENTS).ensure_mergeable_map("a").unwrap();
        assert!(a.get("x").is_none() && a.get("pos_x").is_some());
        // Already current: nothing to do.
        assert!(!migrate_to(&doc, 2, 2, &table).unwrap());
    }

    #[test]
    fn a_missing_step_and_a_newer_document_are_errors() {
        let doc = doc_at(0);
        assert!(matches!(migrate_to(&doc, 0, 1, &[]), Err(DocError::Migration { from: 0, .. })));
        assert_eq!(read_version(&doc), Some(0), "version is untouched after a failed migration");
        assert!(matches!(migrate_to(&doc, 3, 2, &[]), Err(DocError::NewerSchema { found: 3, max: 2 })));
    }

    #[test]
    fn a_failing_step_names_the_version_it_failed_on() {
        let doc = doc_at(1);
        let table = [Migration { from: 1, apply: |_| Err(DocError::Backend("boom".into())) }];
        match migrate_to(&doc, 1, 2, &table) {
            Err(DocError::Migration { from: 1, reason }) => assert!(reason.contains("boom")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn versions_are_read_defensively() {
        let doc = LoroDoc::new();
        assert_eq!(read_version(&doc), None);
        assert!(is_blank(&doc));
        doc.get_map(ROOT_META).insert(META_SCHEMA, "two").unwrap();
        assert_eq!(read_version(&doc), Some(u32::MAX), "garbage counts as newer, never as current");
        doc.get_map(ROOT_META).insert(META_SCHEMA, -1i64).unwrap();
        assert_eq!(read_version(&doc), Some(u32::MAX));
    }
}
