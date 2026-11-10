//! Reading loosely typed JSON: every getter returns `None` for a missing or `null` field, and for a field of the wrong
//! type it also records a warning. Callers pick the default.

use serde_json::{Map, Value};

use crate::ImportReport;
use crate::limits::MAX_WARNINGS;

/// Records a warning. The message is only built while the report still has room, so a hostile file with millions of
/// bad fields costs no formatting.
pub(super) fn warn(rep: &mut ImportReport, msg: impl FnOnce() -> String) {
    if rep.warnings.len() < MAX_WARNINGS {
        rep.warnings.push(msg());
    } else {
        rep.omitted += 1;
    }
}

/// A JSON object being read, with enough context to word warnings ("item 2: element 5: ...").
#[derive(Clone, Copy)]
pub(super) struct Rd<'a> {
    pub m: &'a Map<String, Value>,
    pub prefix: &'a str,
    pub n: usize,
}

impl<'a> Rd<'a> {
    pub(super) fn new(m: &'a Map<String, Value>, prefix: &'a str, n: usize) -> Self {
        Self { m, prefix, n }
    }

    fn bad(&self, rep: &mut ImportReport, key: &str, want: &str) {
        warn(rep, || format!("{}element {}: `{key}` is not {want}, so the default is used", self.prefix, self.n));
    }

    /// A warning about this element.
    pub(super) fn warn(&self, rep: &mut ImportReport, msg: impl FnOnce() -> String) {
        warn(rep, || format!("{}element {}: {}", self.prefix, self.n, msg()));
    }

    pub(super) fn get(&self, key: &str) -> Option<&'a Value> {
        self.m.get(key).filter(|v| !v.is_null())
    }

    pub(super) fn num(&self, key: &str, rep: &mut ImportReport) -> Option<f64> {
        let v = self.get(key)?;
        match v.as_f64().filter(|f| f.is_finite()) {
            Some(f) => Some(f),
            None => {
                self.bad(rep, key, "a number");
                None
            }
        }
    }

    pub(super) fn int(&self, key: &str, rep: &mut ImportReport) -> Option<i64> {
        let v = self.get(key)?;
        // Floats that hold a whole number are fine (JavaScript does not tell them apart).
        let n = v
            .as_i64()
            .or_else(|| v.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64));
        if n.is_none() {
            self.bad(rep, key, "a whole number");
        }
        n
    }

    pub(super) fn str(&self, key: &str, rep: &mut ImportReport) -> Option<&'a str> {
        let v = self.get(key)?;
        match v.as_str() {
            Some(s) => Some(s),
            None => {
                self.bad(rep, key, "text");
                None
            }
        }
    }

    pub(super) fn bool(&self, key: &str, rep: &mut ImportReport) -> Option<bool> {
        let v = self.get(key)?;
        match v.as_bool() {
            Some(b) => Some(b),
            None => {
                self.bad(rep, key, "true or false");
                None
            }
        }
    }

    pub(super) fn array(&self, key: &str, rep: &mut ImportReport) -> Option<&'a Vec<Value>> {
        let v = self.get(key)?;
        match v.as_array() {
            Some(a) => Some(a),
            None => {
                self.bad(rep, key, "a list");
                None
            }
        }
    }

    pub(super) fn object(&self, key: &str, rep: &mut ImportReport) -> Option<&'a Map<String, Value>> {
        let v = self.get(key)?;
        match v.as_object() {
            Some(o) => Some(o),
            None => {
                self.bad(rep, key, "an object");
                None
            }
        }
    }
}

/// A pair of finite numbers `[a, b]`.
pub(super) fn pair(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    if a.len() != 2 {
        return None;
    }
    let x = a.first()?.as_f64().filter(|f| f.is_finite())?;
    let y = a.get(1)?.as_f64().filter(|f| f.is_finite())?;
    Some([x, y])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn getters_default_quietly_on_missing_and_warn_on_wrong_types() {
        let v = json!({"a": 1.5, "b": "x", "c": null, "d": 3.0, "e": true});
        let m = v.as_object().unwrap();
        let rd = Rd::new(m, "", 1);
        let mut rep = ImportReport::default();
        assert_eq!(rd.num("a", &mut rep), Some(1.5));
        assert_eq!(rd.num("missing", &mut rep), None);
        assert_eq!(rd.num("c", &mut rep), None);
        assert!(rep.is_clean());
        assert_eq!(rd.num("b", &mut rep), None);
        assert_eq!(rd.int("d", &mut rep), Some(3));
        assert_eq!(rd.int("a", &mut rep), None);
        assert_eq!(rd.str("e", &mut rep), None);
        assert_eq!(rep.warnings.len(), 3);
        assert!(rep.warnings[0].contains("element 1") && rep.warnings[0].contains("`b`"));
    }

    #[test]
    fn pairs_must_be_two_finite_numbers() {
        assert_eq!(pair(&json!([1, 2.5])), Some([1.0, 2.5]));
        assert_eq!(pair(&json!([1])), None);
        assert_eq!(pair(&json!([1, "2"])), None);
        assert_eq!(pair(&json!([1, 2, 3])), None);
    }
}
