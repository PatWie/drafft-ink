//! What the debug panel shows. The app measures (the shell times the frames and the GPU, the renderer counts its
//! work) and hands this plain data to the UI, which only draws it (`surfaces/debug.rs`). Nothing here is read from
//! a session or measured by the UI, and nothing is made up: a number the machine cannot give is left out or says why.

use drafftink_profile::FrameReading;

/// How a value is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowTone {
    /// The normal text colour.
    #[default]
    Normal,
    /// Worth a look: a software rasteriser, a slow frame.
    Warn,
}

/// One line of the panel: a name and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugRow {
    /// What it is.
    pub name: &'static str,
    /// The value, as text.
    pub value: String,
    /// How the value is drawn.
    pub tone: RowTone,
}

impl DebugRow {
    /// A row in the normal tone.
    pub fn new(name: &'static str, value: impl Into<String>) -> Self {
        Self { name, value: value.into(), tone: RowTone::Normal }
    }

    /// The same row marked as worth a look.
    #[must_use]
    pub fn flagged(mut self) -> Self {
        self.tone = RowTone::Warn;
        self
    }
}

/// A titled group of rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugSection {
    /// The caption above the rows.
    pub title: &'static str,
    /// The rows.
    pub rows: Vec<DebugRow>,
}

/// The content of the debug panel. Written by the app through `UiShell::set_debug` in every frame while the panel is
/// open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DebugView {
    /// Frame rate and frame time. `None` until the shell has timed a frame (the first frame after the panel opens).
    pub frames: Option<FrameReading>,
    /// The sections under the frame rate: the renderer and the GPU, the scene, the window.
    pub sections: Vec<DebugSection>,
}

/// A frame time as the panel shows it: `0.42 ms`, `12.3 ms`, `103 ms`.
pub fn ms_label(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.2} ms")
    } else if ms < 100.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{ms:.0} ms")
    }
}

/// A count with a comma between the thousands: `12,345`.
pub fn count_label(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i != 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A byte count in binary units: `812 KiB`, `1.4 GiB`.
pub fn bytes_label(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("TiB");
    if value < 10.0 { format!("{value:.1} {name}") } else { format!("{value:.0} {name}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_times_keep_three_digits() {
        assert_eq!(ms_label(0.4213), "0.42 ms");
        assert_eq!(ms_label(12.34), "12.3 ms");
        assert_eq!(ms_label(103.4), "103 ms");
    }

    #[test]
    fn counts_group_the_thousands() {
        assert_eq!(count_label(0), "0");
        assert_eq!(count_label(999), "999");
        assert_eq!(count_label(1_000), "1,000");
        assert_eq!(count_label(12_345), "12,345");
        assert_eq!(count_label(100_000), "100,000");
        assert_eq!(count_label(1_234_567), "1,234,567");
    }

    #[test]
    fn byte_counts_use_binary_units() {
        assert_eq!(bytes_label(0), "0 B");
        assert_eq!(bytes_label(1023), "1023 B");
        assert_eq!(bytes_label(1024), "1.0 KiB");
        assert_eq!(bytes_label(812 * 1024), "812 KiB");
        assert_eq!(bytes_label(1_503_238_554), "1.4 GiB");
        assert_eq!(bytes_label(u64::MAX), "16777216 TiB", "the largest count stays in the last unit");
    }
}
