//! The document schema this server serves (docs/crdt.md §8).
//!
//! The server keeps documents as opaque Loro data, but it reads one number, `meta.schema`, to refuse an update
//! that raises it beyond [`SERVED_SCHEMA`]: a client that understands only the lower number would otherwise
//! receive a document it can only show read-only.

use loro::{LoroDoc, LoroValue, ValueOrContainer};

/// The highest `meta.schema` the server relays and stores. It is the `SCHEMA_VERSION` of `drafftink-doc`
/// (the `served_schema_is_the_one_of_the_document_crate` test keeps them equal).
pub const SERVED_SCHEMA: u32 = 1;

/// The `meta.schema` of `doc` when it is above [`SERVED_SCHEMA`]. A missing or unreadable value is not above.
pub fn beyond_served(doc: &LoroDoc) -> Option<i64> {
    // `get_by_str_path` does not create the `meta` container the way `get_map` does, so a room that has none
    // stays as it was.
    let found = match doc.get_by_str_path("meta/schema")? {
        ValueOrContainer::Value(LoroValue::I64(n)) => n,
        _ => return None,
    };
    (found > i64::from(SERVED_SCHEMA)).then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_schema(v: LoroValue) -> LoroDoc {
        let doc = LoroDoc::new();
        doc.get_map("meta").insert("schema", v).unwrap_or_default();
        doc.commit();
        doc
    }

    #[test]
    fn served_schema_is_the_one_of_the_document_crate() {
        assert_eq!(SERVED_SCHEMA, drafftink_doc::SCHEMA_VERSION);
    }

    #[test]
    fn only_a_number_above_the_served_schema_counts() {
        assert_eq!(beyond_served(&LoroDoc::new()), None);
        assert_eq!(beyond_served(&with_schema(LoroValue::from(i64::from(SERVED_SCHEMA)))), None);
        assert_eq!(beyond_served(&with_schema(LoroValue::from(0i64))), None);
        assert_eq!(beyond_served(&with_schema(LoroValue::from(99i64))), Some(99));
        assert_eq!(beyond_served(&with_schema(LoroValue::from("99"))), None);
    }
}
