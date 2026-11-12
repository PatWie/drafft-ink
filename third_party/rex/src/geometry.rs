//! Defines geometric primitives
//! Currently only defines bbox

use std::ops::Mul;

use crate::dimensions::Unit;

/// A generic bounding box for a 2D coordinate system, parameterized by unit type.
/// When used to store typographical measurements, i.e. a glyph's bounding box, it is assumed the origin is at (0, 0). In particular, the baseline on which glyphs sit is at y=0
/// This is relevant for [`BBox::typo_height`] and [`BBox::typo_depth`].
#[derive(Debug, Clone)]
pub struct BBox<U> {
    /// minimal x-value
    pub x_min : Unit<U>,
    /// maximal x-value
    pub x_max : Unit<U>,
    /// minimal y-value
    pub y_min : Unit<U>,
    /// maximal y-value
    pub y_max : Unit<U>,
}

impl<U> BBox<U> {
    /// Creates new bbox from coordinates of extremal points
    /// Does not check the invariant that `x_min <= x_max` and `y_min <= y_max`
    pub fn new(x_min: Unit<U>, y_min: Unit<U>, x_max: Unit<U>, y_max: Unit<U>) -> Self {
        debug_assert!(x_min <= x_max);
        debug_assert!(y_min <= y_max);
        Self { x_min, x_max, y_min, y_max }
    }

    /// Creates a bbox, given a position for top-left corner, width and typogrpahical height and typographical height.
    pub fn from_typo(x : Unit<U>, width : Unit<U>, height : Unit<U>, depth : Unit<U>) -> Self {
        debug_assert!(width >= Unit::ZERO);
        debug_assert!(depth <= height);
        Self {
            x_min : x, x_max : x + width,
            y_min : depth, y_max : height,
        }
    }

    /// Creates a bbox, given a position for top-left corner, width and height.
    pub fn from_dims(x : Unit<U>, y : Unit<U>, width : Unit<U>, height : Unit<U>) -> Self {
        debug_assert!(height >= Unit::ZERO);
        debug_assert!(width >= Unit::ZERO);
        Self {
            x_min : x, x_max : x + width,
            y_min : y, y_max : y + height,
        }
    }

    pub fn translate(&self, t_x : Unit<U>, t_y : Unit<U>) -> Self {
        Self {
            x_min: self.x_min + t_x,
            x_max: self.x_max + t_x,
            y_min: self.y_min + t_y,
            y_max: self.y_max + t_y,
        }
    }

    /// Creates a bbox corresponding to a zero-width zero-height point
    pub fn single_point(x : Unit<U>, y : Unit<U>) -> Self {
        Self {
            x_min : x, x_max : x,
            y_min : y, y_max : y,
        }
    }

    /// Creates the smallest bbox containing `self` and the point with coordinates `x` and `y`
    pub fn enclose(&self, x : Unit<U>, y: Unit<U>) -> Self {
        self.union(Self::single_point(x, y))
    }

    /// Creates the smallest bbox containing `self` and `other`
    pub fn union(&self, other: Self) -> Self {
        Self {
            x_min : Unit::min(self.x_min, other.x_min),
            x_max : Unit::max(self.x_max, other.x_max),
            y_min : Unit::min(self.y_min, other.y_min),
            y_max : Unit::max(self.y_max, other.y_max),
        }
    }

    /// Signed distance between the baseline and the highest points of the bounding box
    /// If the glyph extends above the baseline (and most do), this is positive.
    /// NB: this is not be the same as the "geometric height" of the bounding box (i.e. unsigned distance between top and bottom of box)
    pub fn typo_height(&self) -> Unit<U> {
        self.y_max
    }

    /// Signed distance between the baseline and the lowest points of the bounding box
    /// If the glyph extends below the baseline (and most do), this is negative.
    pub fn typo_depth(&self) -> Unit<U> {
        self.y_min
    }

    /// Width of the bounding box
    pub fn width(&self) -> Unit<U> {
        self.x_max - self.x_min
    }

    /// The geometric height: the unsigned distance between the top of the box and the bottom of the box
    pub fn total_height(&self) -> Unit<U> {
        self.y_max - self.y_min
    }

    pub fn scale<V, W>(&self, scale : Unit<V>) -> BBox<W> 
    where Unit<U> : Mul<Unit<V>, Output = Unit<W>>
    {
        BBox::<W> { 
            x_min: self.x_min * scale, 
            x_max: self.x_max * scale, 
            y_min: self.y_min * scale, 
            y_max: self.y_max * scale, 
        }
    }

    /// Checks if 2 bboxes are approximately the same, more specifically if the two corners' four coordinates are the same up to `epsilon` of their initial value.
    pub fn close_to(&self, other: &Self, epsilon: f64) -> bool {
        (self.x_min - other.x_min).to_unitless().abs() * 2. <= epsilon * (self.x_min.abs() + other.x_min.abs()).to_unitless() &&
        (self.x_max - other.x_max).to_unitless().abs() * 2. <= epsilon * (self.x_max.abs() + other.x_max.abs()).to_unitless() &&
        (self.y_min - other.y_min).to_unitless().abs() * 2. <= epsilon * (self.y_min.abs() + other.y_min.abs()).to_unitless() &&
        (self.y_max - other.y_max).to_unitless().abs() * 2. <= epsilon * (self.y_max.abs() + other.y_max.abs()).to_unitless() 
    }


}

