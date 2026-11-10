//! A measure without fonts, for deterministic tests.

use std::sync::Arc;

use drafftink_edit::text_bind::{MonospaceMeasure, TextMeasure as EditMeasure};
use drafftink_io::svg::MathSvg;
use drafftink_io::{EstimateMeasure, MathBox, TextMeasure as IoMeasure};
use drafftink_scene::{FontFamily, FontWeight, TextStyle};
use drafftink_typeset::{MathError, MathErrorKind, MathLayout};

use super::Measure;

/// `edit` measures with [`MonospaceMeasure`] (every character is half the font size wide), `io` with
/// [`EstimateMeasure`], and a formula is drawn as an empty box of its estimated size. No formula layout exists.
#[derive(Debug, Clone, Copy, Default)]
pub struct FakeMeasure {
    edit: MonospaceMeasure,
    io: EstimateMeasure,
}

impl EditMeasure for FakeMeasure {
    fn line_width(&self, line: &str, font: &TextStyle) -> f64 {
        self.edit.line_width(line, font)
    }
}

impl IoMeasure for FakeMeasure {
    fn text_size(&self, text: &str, family: FontFamily, weight: FontWeight, size: f64, line_height: f64) -> (f64, f64) {
        self.io.text_size(text, family, weight, size, line_height)
    }

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        self.io.math_size(latex, size)
    }
}

impl MathSvg for FakeMeasure {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        let b = self.io.math_size(latex, size)?;
        Some(format!(
            r#"<rect x="0" y="{}" width="{}" height="{}" fill="none" stroke="currentColor"/>"#,
            -b.height,
            b.width,
            b.height + b.depth
        ))
    }
}

impl Measure for FakeMeasure {
    fn math_layout(&self, _latex: &str, _size: f64) -> Result<Arc<MathLayout>, MathError> {
        Err(MathError {
            kind: MathErrorKind::Layout("the fake measure has no formula engine".to_owned()),
            span: 0..0,
            message: "This session measures without a formula engine. Use Services::new() for real layouts.".to_owned(),
        })
    }
}
