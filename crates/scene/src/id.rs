//! Identifiers: element, group and file ids, and the injected sources that create them.
//!
//! Ids are short strings so that foreign ids (old UUIDs, Excalidraw nanoids) survive a migration unchanged and so that
//! tests and agents can read them. They are never derived from the clock or from global randomness: creation goes
//! through an [`IdSource`] that the caller supplies.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use smol_str::SmolStr;

/// Longest accepted element or group id, in bytes.
pub const MAX_ID_LEN: usize = 64;

/// Why a string is not a valid id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The id has no characters.
    #[error("an id cannot be empty")]
    Empty,
    /// The id is longer than [`MAX_ID_LEN`].
    #[error("an id is at most {MAX_ID_LEN} bytes, and this one has {0}")]
    TooLong(usize),
    /// The id contains a character outside `A-Z a-z 0-9 _ -`.
    #[error("an id may only contain A-Z, a-z, 0-9, '_' and '-', and {0:?} is not allowed")]
    BadChar(char),
    /// A file id is not 64 lowercase hex digits.
    #[error("a file id is 64 lowercase hex digits (a SHA-256 of the content)")]
    BadFileId,
}

fn check_id(s: &str) -> Result<(), IdError> {
    if s.is_empty() {
        return Err(IdError::Empty);
    }
    if s.len() > MAX_ID_LEN {
        return Err(IdError::TooLong(s.len()));
    }
    match s.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-')) {
        Some(c) => Err(IdError::BadChar(c)),
        None => Ok(()),
    }
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident, $schema_name:literal) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(SmolStr);

        impl $name {
            /// Checks `s` and wraps it.
            pub fn new(s: &str) -> Result<Self, IdError> {
                check_id(s)?;
                Ok(Self(SmolStr::new(s)))
            }

            /// The id as text.
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, IdError> {
                Self::new(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.0.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = std::borrow::Cow::<str>::deserialize(d)?;
                Self::new(&s).map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                $schema_name.into()
            }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_ID_LEN,
                    "pattern": "^[A-Za-z0-9_-]+$"
                })
            }
        }
    };
}

string_id!(
    /// Stable identifier of an element. Never changes for the life of the element, across group, ungroup, move and
    /// undo. Only duplicate and paste create new ids.
    ElementId,
    "ElementId"
);
string_id!(
    /// Identifier of a group. A group is not an element: it is an id shared by the `group_ids` of its members.
    GroupId,
    "GroupId"
);

/// Content hash of an image blob: 64 lowercase hex digits of its SHA-256.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(SmolStr);

impl FileId {
    /// The id of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let digest = Sha256::digest(bytes);
        let mut out = String::with_capacity(64);
        for b in digest {
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
        Self(SmolStr::new(out))
    }

    /// Checks that `s` has the shape of a content hash. It does not prove that any blob has this hash.
    pub fn new(s: &str) -> Result<Self, IdError> {
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            Ok(Self(SmolStr::new(s)))
        } else {
            Err(IdError::BadFileId)
        }
    }

    /// The hash as text.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileId({})", self.0)
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl Serialize for FileId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for FileId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for FileId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FileId".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^[0-9a-f]{64}$" })
    }
}

/// A source of ids and seeds. The engine's `Ids` port implements it: random in the app, sequential in tests.
pub trait IdSource {
    /// A fresh element id.
    fn element_id(&mut self) -> ElementId;
    /// A fresh group id.
    fn group_id(&mut self) -> GroupId;
    /// A fresh seed for the hand-drawn jitter of one element.
    fn seed(&mut self) -> u32;
}

/// A 64-bit random generator supplied by the caller (OS randomness in the app, a fixed seed in tests).
pub trait Rng64 {
    /// The next 64 random bits.
    fn next_u64(&mut self) -> u64;
}

/// SplitMix64: a tiny deterministic generator. Good for ids and seeds, not for secrets.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator that always produces the same sequence for the same `seed`.
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl Rng64 for SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Random 16-character ids (96 bits) from an injected generator.
#[derive(Debug, Clone)]
pub struct RandomIds<R: Rng64>(pub R);

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

fn random_text<R: Rng64>(rng: &mut R) -> SmolStr {
    let (a, b) = (rng.next_u64(), rng.next_u64());
    let mut out = String::with_capacity(16);
    for i in 0..16u32 {
        let bits = if i < 10 { a >> (i * 6) } else { b >> ((i - 10) * 6) };
        let idx = usize::try_from(bits & 63).unwrap_or(0);
        out.push(char::from(ALPHABET.get(idx).copied().unwrap_or(b'A')));
    }
    SmolStr::new(out)
}

impl<R: Rng64> IdSource for RandomIds<R> {
    fn element_id(&mut self) -> ElementId {
        ElementId(random_text(&mut self.0))
    }
    fn group_id(&mut self) -> GroupId {
        GroupId(random_text(&mut self.0))
    }
    fn seed(&mut self) -> u32 {
        // Truncation is the point: the low 32 bits of a uniform u64 are uniform.
        (self.0.next_u64() & u64::from(u32::MAX)) as u32
    }
}

/// Sequential ids for tests and scripts: `e1`, `e2`, ... for elements, `g1`, `g2`, ... for groups, seeds 1, 2, ...
#[derive(Debug, Clone, Default)]
pub struct SeqIds {
    elements: u64,
    groups: u64,
    seeds: u32,
}

impl SeqIds {
    /// A fresh counter starting at 1.
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdSource for SeqIds {
    fn element_id(&mut self) -> ElementId {
        self.elements += 1;
        ElementId(SmolStr::new(format!("e{}", self.elements)))
    }
    fn group_id(&mut self) -> GroupId {
        self.groups += 1;
        GroupId(SmolStr::new(format!("g{}", self.groups)))
    }
    fn seed(&mut self) -> u32 {
        self.seeds = self.seeds.wrapping_add(1);
        self.seeds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_and_invalid_ids() {
        assert!(ElementId::new("abc-DEF_123").is_ok());
        assert_eq!(ElementId::new(""), Err(IdError::Empty));
        assert_eq!(ElementId::new("a b"), Err(IdError::BadChar(' ')));
        assert_eq!(ElementId::new("é"), Err(IdError::BadChar('é')));
        assert_eq!(ElementId::new(&"a".repeat(65)), Err(IdError::TooLong(65)));
        assert!(ElementId::new(&"a".repeat(64)).is_ok());
        // An old UUID is a valid id.
        assert!(ElementId::new("44093825-1401-4f08-9fd9-84d6a593a5d2").is_ok());
    }

    #[test]
    fn random_ids_are_deterministic_for_a_seed_and_valid() {
        let mut a = RandomIds(SplitMix64::new(7));
        let mut b = RandomIds(SplitMix64::new(7));
        let ia = a.element_id();
        assert_eq!(ia, b.element_id());
        assert_eq!(ia.as_str().len(), 16);
        assert!(ElementId::new(ia.as_str()).is_ok());
        assert_ne!(ia, a.element_id());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            assert!(seen.insert(a.element_id()));
        }
    }

    #[test]
    fn sequential_ids() {
        let mut s = SeqIds::new();
        assert_eq!(s.element_id().as_str(), "e1");
        assert_eq!(s.element_id().as_str(), "e2");
        assert_eq!(s.group_id().as_str(), "g1");
        assert_eq!(s.seed(), 1);
    }

    #[test]
    fn file_id_is_the_sha256_hex() {
        // SHA-256("abc")
        assert_eq!(FileId::of(b"abc").as_str(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(FileId::new("ABC").is_err());
        assert!(FileId::new(FileId::of(b"x").as_str()).is_ok());
    }

    #[test]
    fn serde_rejects_bad_ids() {
        assert!(serde_json::from_str::<ElementId>("\"ok_1\"").is_ok());
        assert!(serde_json::from_str::<ElementId>("\"no way\"").is_err());
        assert!(serde_json::from_str::<ElementId>("12").is_err());
        assert!(serde_json::from_str::<FileId>("\"abc\"").is_err());
    }
}
