//! Output set, hashing, deterministic gzip, and the compact-but-readable JSON writer.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Every file the generator produces, keyed by path relative to the fixtures dir.
#[derive(Default)]
pub struct Out {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Out {
    pub fn put(&mut self, rel: &str, bytes: Vec<u8>) {
        self.files.insert(rel.to_string(), bytes);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Gzip with a fixed header (mtime 0, no name) so the bytes are reproducible.
pub fn gzip(bytes: &[u8]) -> Res<Vec<u8>> {
    let mut enc = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::new(9));
    enc.write_all(bytes)?;
    Ok(enc.finish()?)
}

/// Pretty JSON where any object or array whose compact form fits in 110 columns stays on one line.
pub fn fmt_json(v: &Value) -> String {
    let mut s = String::new();
    fmt_rec(v, 0, &mut s);
    s.push('\n');
    s
}

fn fmt_rec(v: &Value, indent: usize, out: &mut String) {
    let compact = serde_json::to_string(v).unwrap_or_default();
    let is_container = matches!(v, Value::Object(_) | Value::Array(_));
    if !is_container || compact.len() + indent <= 110 {
        out.push_str(&compact);
        return;
    }
    let pad = " ".repeat(indent + 2);
    match v {
        Value::Object(m) => {
            out.push_str("{\n");
            let n = m.len();
            for (i, (k, val)) in m.iter().enumerate() {
                out.push_str(&pad);
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push_str(": ");
                fmt_rec(val, indent + 2, out);
                if i + 1 < n {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
        Value::Array(a) => {
            out.push_str("[\n");
            let n = a.len();
            for (i, val) in a.iter().enumerate() {
                out.push_str(&pad);
                fmt_rec(val, indent + 2, out);
                if i + 1 < n {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&" ".repeat(indent));
            out.push(']');
        }
        _ => {}
    }
}

/// Tiny deterministic PRNG (splitmix64) so generated documents never depend on a crate version.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in [lo, hi), rounded to 0.01 so the JSON stays short and exact.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        ((lo + (hi - lo) * self.f()) * 100.0).round() / 100.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

pub fn fnv32(s: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}
