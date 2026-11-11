//! Fractional indexing: the z-order key of an element.
//!
//! A [`ZIndex`] is a short string that sorts in bottom-to-top order under plain byte comparison. A new key can always
//! be made between two existing keys, so a reorder rewrites only the moved elements, which is what makes z-order a
//! CRDT-friendly field. The scheme is the base-62 variant of the one published by Rocicorp (`fractional-indexing`,
//! CC0), the same one Excalidraw uses, so `.excalidraw` indices are valid keys here. Ties between concurrent inserts
//! are broken by element id; [`repair`] fixes a sequence that still has ties or inversions.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smol_str::SmolStr;

/// Longest key accepted. A key grows by about one character per six inserts at the same spot; past this, rebalance.
pub const MAX_INDEX_LEN: usize = 128;

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const SMALLEST_INTEGER: &str = "A00000000000000000000000000";
const INTEGER_ZERO: &str = "a0";

/// Why a key is invalid or cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    /// The text is not a well-formed order key.
    #[error("{0:?} is not a valid z-order key")]
    Invalid(String),
    /// The lower key is not below the upper key.
    #[error("cannot make a z-order key between {lower:?} and {upper:?}: the lower key is not below the upper key")]
    NotOrdered {
        /// Lower bound given.
        lower: String,
        /// Upper bound given.
        upper: String,
    },
    /// The key would be longer than [`MAX_INDEX_LEN`] or the key space is exhausted. Rebalance the neighbours.
    #[error("the z-order keys here are too long or used up; rebalance them with `ZIndex::rebalance`")]
    Exhausted,
}

/// A z-order key. Compares as a string; smaller is further back.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZIndex(SmolStr);

impl fmt::Debug for ZIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ZIndex({})", self.0)
    }
}

impl fmt::Display for ZIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl ZIndex {
    /// The key of the first element in an empty scene.
    pub fn first() -> Self {
        Self(SmolStr::new_static(INTEGER_ZERO))
    }

    /// Checks `s` and wraps it.
    pub fn parse(s: &str) -> Result<Self, IndexError> {
        if s.len() > MAX_INDEX_LEN || !valid_key(s) {
            return Err(IndexError::Invalid(s.chars().take(40).collect()));
        }
        Ok(Self(SmolStr::new(s)))
    }

    /// The key as text.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// A key strictly between `lower` and `upper`. `None` means "no bound on that side".
    pub fn between(lower: Option<&ZIndex>, upper: Option<&ZIndex>) -> Result<ZIndex, IndexError> {
        let key = key_between(lower.map(ZIndex::as_str), upper.map(ZIndex::as_str))?;
        wrap(key)
    }

    /// `n` increasing keys strictly between `lower` and `upper`.
    pub fn n_between(lower: Option<&ZIndex>, upper: Option<&ZIndex>, n: usize) -> Result<Vec<ZIndex>, IndexError> {
        let keys = n_keys_between(lower.map(ZIndex::as_str), upper.map(ZIndex::as_str), n)?;
        keys.into_iter().map(wrap).collect()
    }

    /// `n` fresh, evenly short keys for a whole sequence. Use it to shorten keys that have grown long.
    pub fn rebalance(n: usize) -> Result<Vec<ZIndex>, IndexError> {
        ZIndex::n_between(None, None, n)
    }
}

fn wrap(key: String) -> Result<ZIndex, IndexError> {
    if key.len() > MAX_INDEX_LEN {
        return Err(IndexError::Exhausted);
    }
    Ok(ZIndex(SmolStr::new(key)))
}

impl Serialize for ZIndex {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for ZIndex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        ZIndex::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ZIndex {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ZIndex".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Fractional index: sorts bottom to top as plain text.",
            "maxLength": MAX_INDEX_LEN,
            "pattern": "^[0-9A-Za-z]+$"
        })
    }
}

// ---- the algorithm (byte-based, never indexes out of range) ----

/// Length of the integer part announced by the head character, or `None` for a bad head.
fn integer_len(head: u8) -> Option<usize> {
    match head {
        b'a'..=b'z' => Some(usize::from(head - b'a') + 2),
        b'A'..=b'Z' => Some(usize::from(b'Z' - head) + 2),
        _ => None,
    }
}

fn integer_part(key: &str) -> Option<&str> {
    let len = integer_len(*key.as_bytes().first()?)?;
    key.get(..len)
}

fn valid_key(key: &str) -> bool {
    if key == SMALLEST_INTEGER || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    match integer_part(key) {
        Some(i) => !key.get(i.len()..).is_some_and(|f| f.ends_with('0')),
        None => false,
    }
}

fn digit_value(b: u8) -> Option<usize> {
    DIGITS.iter().position(|d| *d == b)
}

fn digit_char(i: usize) -> Option<char> {
    DIGITS.get(i).map(|b| char::from(*b))
}

/// A string midway between `a` and `b` (`b` is None for "no upper bound"). `a` is a fraction without a trailing zero.
fn midpoint(a: &str, b: Option<&str>) -> Result<String, IndexError> {
    let bad = || IndexError::Invalid(a.chars().take(40).collect());
    if let Some(b) = b {
        if a >= b {
            return Err(IndexError::NotOrdered { lower: a.into(), upper: b.into() });
        }
        if b.ends_with('0') {
            return Err(bad());
        }
    }
    if a.ends_with('0') {
        return Err(bad());
    }
    if let Some(b) = b {
        // Shared prefix (a is padded with zeros).
        let (ab, bb) = (a.as_bytes(), b.as_bytes());
        let mut n = 0;
        while n < bb.len() && ab.get(n).copied().unwrap_or(b'0') == bb.get(n).copied().unwrap_or(0) {
            n += 1;
        }
        if n > 0 {
            let head = b.get(..n).ok_or_else(bad)?;
            let a_rest = a.get(n.min(a.len())..).ok_or_else(bad)?;
            let b_rest = b.get(n..).ok_or_else(bad)?;
            return Ok(format!("{head}{}", midpoint(a_rest, Some(b_rest))?));
        }
    }
    let digit_a = match a.as_bytes().first() {
        Some(c) => digit_value(*c).ok_or_else(bad)?,
        None => 0,
    };
    let digit_b = match b.and_then(|b| b.as_bytes().first()) {
        Some(c) => digit_value(*c).ok_or_else(bad)?,
        None => DIGITS.len(),
    };
    if digit_b > digit_a + 1 {
        let mid = (digit_a + digit_b).div_ceil(2);
        return digit_char(mid).map(String::from).ok_or_else(bad);
    }
    if let Some(b) = b.filter(|b| b.len() > 1) {
        return b.get(..1).map(str::to_owned).ok_or_else(bad);
    }
    let first = digit_char(digit_a).ok_or_else(bad)?;
    let rest = a.get(1..).unwrap_or("");
    Ok(format!("{first}{}", midpoint(rest, None)?))
}

fn increment_integer(x: &str) -> Result<Option<String>, IndexError> {
    let bad = || IndexError::Invalid(x.chars().take(40).collect());
    let bytes = x.as_bytes();
    let (head, digs) = bytes.split_first().ok_or_else(bad)?;
    let mut digs = digs.to_vec();
    let mut carry = true;
    for d in digs.iter_mut().rev() {
        if !carry {
            break;
        }
        let v = digit_value(*d).ok_or_else(bad)? + 1;
        if v == DIGITS.len() {
            *d = b'0';
        } else {
            *d = *DIGITS.get(v).ok_or_else(bad)?;
            carry = false;
        }
    }
    let mut head = *head;
    if carry {
        if head == b'Z' {
            return Ok(Some(INTEGER_ZERO.to_owned()));
        }
        if head == b'z' {
            return Ok(None);
        }
        head += 1;
        if head > b'a' {
            digs.push(b'0');
        } else {
            digs.pop();
        }
    }
    let mut out = vec![head];
    out.extend(digs);
    String::from_utf8(out).map(Some).map_err(|_| bad())
}

fn decrement_integer(x: &str) -> Result<Option<String>, IndexError> {
    let bad = || IndexError::Invalid(x.chars().take(40).collect());
    let bytes = x.as_bytes();
    let (head, digs) = bytes.split_first().ok_or_else(bad)?;
    let mut digs = digs.to_vec();
    let mut borrow = true;
    let last = *DIGITS.last().ok_or_else(bad)?;
    for d in digs.iter_mut().rev() {
        if !borrow {
            break;
        }
        let v = digit_value(*d).ok_or_else(bad)?;
        if v == 0 {
            *d = last;
        } else {
            *d = *DIGITS.get(v - 1).ok_or_else(bad)?;
            borrow = false;
        }
    }
    let mut head = *head;
    if borrow {
        if head == b'a' {
            return Ok(Some(format!("Z{}", char::from(last))));
        }
        if head == b'A' {
            return Ok(None);
        }
        head -= 1;
        if head < b'Z' {
            digs.push(last);
        } else {
            digs.pop();
        }
    }
    let mut out = vec![head];
    out.extend(digs);
    String::from_utf8(out).map(Some).map_err(|_| bad())
}

fn split_key(key: &str) -> Result<(&str, &str), IndexError> {
    if !valid_key(key) {
        return Err(IndexError::Invalid(key.chars().take(40).collect()));
    }
    let i = integer_part(key).ok_or_else(|| IndexError::Invalid(key.chars().take(40).collect()))?;
    Ok((i, key.get(i.len()..).unwrap_or("")))
}

fn key_between(a: Option<&str>, b: Option<&str>) -> Result<String, IndexError> {
    if let Some(a) = a {
        split_key(a)?;
    }
    if let Some(b) = b {
        split_key(b)?;
    }
    if let (Some(a), Some(b)) = (a, b)
        && a >= b
    {
        return Err(IndexError::NotOrdered { lower: a.into(), upper: b.into() });
    }
    match (a, b) {
        (None, None) => Ok(INTEGER_ZERO.to_owned()),
        (None, Some(b)) => {
            let (ib, fb) = split_key(b)?;
            if ib == SMALLEST_INTEGER {
                return Ok(format!("{ib}{}", midpoint("", Some(fb))?));
            }
            if ib < b {
                return Ok(ib.to_owned());
            }
            decrement_integer(ib)?.ok_or(IndexError::Exhausted)
        }
        (Some(a), None) => {
            let (ia, fa) = split_key(a)?;
            match increment_integer(ia)? {
                Some(i) => Ok(i),
                None => Ok(format!("{ia}{}", midpoint(fa, None)?)),
            }
        }
        (Some(a), Some(b)) => {
            let (ia, fa) = split_key(a)?;
            let (ib, fb) = split_key(b)?;
            if ia == ib {
                return Ok(format!("{ia}{}", midpoint(fa, Some(fb))?));
            }
            match increment_integer(ia)? {
                Some(i) if i.as_str() < b => Ok(i),
                _ => Ok(format!("{ia}{}", midpoint(fa, None)?)),
            }
        }
    }
}

fn n_keys_between(a: Option<&str>, b: Option<&str>, n: usize) -> Result<Vec<String>, IndexError> {
    match n {
        0 => Ok(Vec::new()),
        1 => Ok(vec![key_between(a, b)?]),
        _ => {
            if b.is_none() {
                let mut c = key_between(a, None)?;
                let mut out = vec![c.clone()];
                for _ in 1..n {
                    c = key_between(Some(&c), None)?;
                    out.push(c.clone());
                }
                return Ok(out);
            }
            if a.is_none() {
                let mut c = key_between(None, b)?;
                let mut out = vec![c.clone()];
                for _ in 1..n {
                    c = key_between(None, Some(&c))?;
                    out.push(c.clone());
                }
                out.reverse();
                return Ok(out);
            }
            let mid = n / 2;
            let c = key_between(a, b)?;
            let mut out = n_keys_between(a, Some(&c), mid)?;
            out.push(c.clone());
            out.extend(n_keys_between(Some(&c), b, n - mid - 1)?);
            Ok(out)
        }
    }
}

impl ZIndex {
    /// Repairs a sequence that should be strictly increasing but has ties or inversions (after a merge, or an import).
    ///
    /// Elements whose key does not sit strictly between its neighbours get new keys, run by run, so well-ordered
    /// elements keep their keys. If a run cannot be filled, the whole sequence is rebalanced. Returns the positions that
    /// changed. The slice must already be in the intended order (for ties, order by element id first).
    pub fn repair(keys: &mut [ZIndex]) -> Result<Vec<usize>, IndexError> {
        let n = keys.len();
        let invalid: Vec<bool> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let prev = i.checked_sub(1).and_then(|p| keys.get(p));
                let next = keys.get(i + 1);
                prev.is_some_and(|p| p >= k) || next.is_some_and(|nx| k >= nx)
            })
            .collect();
        if !invalid.iter().any(|b| *b) {
            return Ok(Vec::new());
        }
        let mut new_keys: Vec<ZIndex> = keys.to_vec();
        let mut changed = Vec::new();
        let mut i = 0;
        let mut ok = true;
        while i < n {
            if !invalid.get(i).copied().unwrap_or(false) {
                i += 1;
                continue;
            }
            let start = i;
            while i < n && invalid.get(i).copied().unwrap_or(false) {
                i += 1;
            }
            let lower = start.checked_sub(1).and_then(|p| keys.get(p));
            let upper = keys.get(i);
            match ZIndex::n_between(lower, upper, i - start) {
                Ok(fresh) => {
                    for (offset, k) in fresh.into_iter().enumerate() {
                        if let Some(slot) = new_keys.get_mut(start + offset) {
                            *slot = k;
                            changed.push(start + offset);
                        }
                    }
                }
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            let fresh = ZIndex::rebalance(n)?;
            let all: Vec<usize> = (0..n).filter(|i| keys.get(*i) != fresh.get(*i)).collect();
            for (slot, k) in keys.iter_mut().zip(fresh) {
                *slot = k;
            }
            return Ok(all);
        }
        for (slot, k) in keys.iter_mut().zip(new_keys) {
            *slot = k;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn z(s: &str) -> ZIndex {
        ZIndex::parse(s).unwrap()
    }

    #[test]
    fn known_vectors_from_the_reference_implementation() {
        let k = |a: Option<&str>, b: Option<&str>| key_between(a, b).unwrap();
        assert_eq!(k(None, None), "a0");
        assert_eq!(k(None, Some("a0")), "Zz");
        assert_eq!(k(Some("a0"), None), "a1");
        assert_eq!(k(Some("a0"), Some("a1")), "a0V");
        assert_eq!(k(Some("a1"), Some("a2")), "a1V");
        assert_eq!(k(Some("a0V"), Some("a1")), "a0l");
        assert_eq!(k(Some("Zz"), Some("a0")), "ZzV");
        assert_eq!(k(Some("Zz"), Some("a1")), "a0");
        assert_eq!(k(None, Some("Y00")), "Xzzz");
        assert_eq!(k(Some("bzz"), None), "c000");
    }

    #[test]
    fn n_keys_are_increasing_and_between() {
        let keys = ZIndex::n_between(Some(&z("a0")), Some(&z("a1")), 10).unwrap();
        assert_eq!(keys.len(), 10);
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        assert!(keys[0] > z("a0") && keys[9] < z("a1"));
        let fresh = ZIndex::rebalance(200).unwrap();
        assert!(fresh.windows(2).all(|w| w[0] < w[1]));
        assert!(fresh.iter().all(|k| k.as_str().len() <= 4));
        assert!(ZIndex::rebalance(0).unwrap().is_empty());
    }

    #[test]
    fn appending_and_prepending_many_stays_short_and_ordered() {
        let mut top = ZIndex::first();
        for _ in 0..5000 {
            let next = ZIndex::between(Some(&top), None).unwrap();
            assert!(next > top);
            top = next;
        }
        assert!(top.as_str().len() < 10);
        let mut bottom = ZIndex::first();
        for _ in 0..5000 {
            let next = ZIndex::between(None, Some(&bottom)).unwrap();
            assert!(next < bottom);
            bottom = next;
        }
    }

    #[test]
    fn inserting_at_the_same_spot_grows_then_reports_exhaustion() {
        let lo = z("a0");
        let mut hi = z("a1");
        let mut grew = false;
        for _ in 0..2000 {
            match ZIndex::between(Some(&lo), Some(&hi)) {
                Ok(k) => {
                    assert!(k > lo && k < hi);
                    hi = k;
                }
                Err(e) => {
                    assert_eq!(e, IndexError::Exhausted);
                    grew = true;
                    break;
                }
            }
        }
        assert!(grew, "the key length limit must be reached and reported");
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        for bad in ["", "0", "a", "a00", "A00000000000000000000000000", "a0 ", "é", "a0\u{0}", "!!", "zz"] {
            assert!(ZIndex::parse(bad).is_err(), "{bad:?}");
        }
        assert!(matches!(ZIndex::between(Some(&z("a1")), Some(&z("a0"))), Err(IndexError::NotOrdered { .. })));
        assert!(matches!(ZIndex::between(Some(&z("a1")), Some(&z("a1"))), Err(IndexError::NotOrdered { .. })));
        assert!(ZIndex::parse(&"a1".repeat(200)).is_err());
        assert!(serde_json::from_str::<ZIndex>("\"a0\"").is_ok());
        assert!(serde_json::from_str::<ZIndex>("\"\"").is_err());
        assert!(serde_json::from_str::<ZIndex>("5").is_err());
    }

    #[test]
    fn excalidraw_style_indices_parse() {
        for k in ["a0", "a1", "a0V", "Zz", "b00", "aF"] {
            assert!(ZIndex::parse(k).is_ok(), "{k}");
        }
    }

    #[test]
    fn repair_fixes_ties_and_inversions_and_keeps_good_keys() {
        let mut keys = vec![z("a1"), z("a1"), z("a2"), z("a3")];
        let changed = ZIndex::repair(&mut keys).unwrap();
        assert!(!changed.is_empty());
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(keys[2], z("a2"));
        assert_eq!(keys[3], z("a3"));

        let mut inverted = vec![z("a5"), z("a3"), z("a9"), z("a4")];
        ZIndex::repair(&mut inverted).unwrap();
        assert!(inverted.windows(2).all(|w| w[0] < w[1]));

        let mut fine = vec![z("a0"), z("a1")];
        assert!(ZIndex::repair(&mut fine).unwrap().is_empty());
        assert!(ZIndex::repair(&mut []).unwrap().is_empty());
    }

    proptest! {
        #[test]
        fn between_is_strictly_between(seq in proptest::collection::vec(0usize..40, 1..60)) {
            // Build a sorted list by inserting at pseudo-random positions, and check the invariants hold.
            let mut list: Vec<ZIndex> = Vec::new();
            for pos in seq {
                let at = if list.is_empty() { 0 } else { pos % (list.len() + 1) };
                let lower = at.checked_sub(1).and_then(|i| list.get(i));
                let upper = list.get(at);
                if let Ok(k) = ZIndex::between(lower, upper) {
                    list.insert(at, k);
                }
            }
            prop_assert!(list.windows(2).all(|w| w[0] < w[1]));
            prop_assert!(list.iter().all(|k| ZIndex::parse(k.as_str()).is_ok()));
        }

        #[test]
        fn arbitrary_text_never_panics(s in "\\PC{0,40}") {
            if let Ok(a) = ZIndex::parse(&s) {
                let _ = ZIndex::between(Some(&a), None);
                let _ = ZIndex::between(None, Some(&a));
                let _ = ZIndex::between(Some(&a), Some(&a));
            }
        }

        #[test]
        fn repair_always_yields_increasing(raw in proptest::collection::vec(0u8..6, 0..30)) {
            let mut keys: Vec<ZIndex> = raw.iter().filter_map(|n| ZIndex::parse(&format!("a{n}")).ok()).collect();
            ZIndex::repair(&mut keys).unwrap();
            prop_assert!(keys.windows(2).all(|w| w[0] < w[1]));
        }
    }
}
