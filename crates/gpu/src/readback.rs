//! Pixels of the retained scene image, read back for the eyedropper.
//!
//! A read is started with [`crate::backend::Backend::read_scene`], which only records and submits the copy, and
//! finished with [`SceneRead::image`]: native code waits for the GPU there, the browser awaits the buffer map.
//! The WebGL2 back end reads synchronously and hands the image over already.

use crate::error::GpuError;
use crate::frame::Image;

/// A read of a rectangle of the scene image that has been started.
#[derive(Debug)]
pub enum SceneRead {
    /// The pixels are here already (the WebGL2 back end).
    Ready(Image),
    /// The copy is on the GPU; the pixels come when the buffer is mapped.
    #[cfg(feature = "wgpu")]
    Mapped(crate::compositor::MappedRead),
}

impl SceneRead {
    /// The pixels of the rectangle, RGBA8, tightly packed, top row first.
    pub async fn image(self) -> Result<Image, GpuError> {
        match self {
            Self::Ready(image) => Ok(image),
            #[cfg(feature = "wgpu")]
            Self::Mapped(read) => read.image().await,
        }
    }
}

/// The rectangle `[x, y, width, height]` (physical pixels) of one pixel at `at` (egui points) in a scene image of
/// `size` pixels drawn at `scale` pixels per point, kept inside the image. `None` for a scene image of no area.
pub(crate) fn pixel_area(at: egui::Pos2, scale: f32, size: [u32; 2]) -> Option<[u32; 4]> {
    let [w, h] = size;
    if w == 0 || h == 0 {
        return None;
    }
    // A position in points is not negative or NaN on a pointer event, and a bad one lands on the first pixel.
    let pixel = |point: f32, side: u32| -> u32 {
        let px = (point * scale).floor();
        if px.is_finite() && px > 0.0 { (px as u32).min(side - 1) } else { 0 }
    };
    Some([pixel(at.x, w), pixel(at.y, h), 1, 1])
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn a_point_maps_to_the_pixel_under_it() {
        assert_eq!(pixel_area(pos2(10.4, 20.9), 1.0, [100, 100]), Some([10, 20, 1, 1]));
        assert_eq!(pixel_area(pos2(10.4, 20.9), 2.0, [400, 400]), Some([20, 41, 1, 1]), "scaled by the pixel ratio");
    }

    #[test]
    fn a_point_outside_the_image_or_not_a_number_lands_on_its_edge() {
        assert_eq!(pixel_area(pos2(500.0, 500.0), 1.0, [100, 80]), Some([99, 79, 1, 1]));
        assert_eq!(pixel_area(pos2(-5.0, f32::NAN), 1.0, [100, 80]), Some([0, 0, 1, 1]));
        assert_eq!(pixel_area(pos2(f32::INFINITY, 1.0), 1.0, [100, 80]), Some([0, 1, 1, 1]));
    }

    #[test]
    fn an_image_without_area_has_no_pixels() {
        assert_eq!(pixel_area(pos2(1.0, 1.0), 1.0, [0, 10]), None);
    }
}
