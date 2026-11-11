//! Helpers shared by the integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_typeset::*;
use kurbo::{Rect, Shape};

pub fn plain(text: &str, size: f32) -> TextSpec {
    TextSpec::styled(text, TextStyle::with_size(size))
}

/// Bounding box of all outline paths, `None` when there is no ink.
pub fn ink_bbox(paths: &[ColouredPath]) -> Option<Rect> {
    paths.iter().filter(|p| !p.path.elements().is_empty()).map(|p| p.path.bounding_box()).reduce(|a, b| a.union(b))
}

pub fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(rel)
}

/// A tiny deterministic generator, so tests need no `rand`.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}
