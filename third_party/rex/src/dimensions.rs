//! Types for quantities with units (em, pixels, etc.)
//!
//! This allows for compile-time checking of unit errors.
//! A function requiring an input to be in px units would for instance ask an argument of type [`Unit<Px>`]. 


use std::ops::{Add, Sub, Mul, AddAssign, SubAssign, Div, Neg};
use std::cmp::{PartialEq, PartialOrd};
use std::fmt::{Display, Debug};
use std::iter::Sum;

use self::units::{Em, Inch, Pt, Px, Ratio};
pub mod units;

/// A f64 value with its unit represented in the type
#[derive(Serialize, Deserialize)]
pub struct Unit<U> {
    value : f64,
    _phantom : std::marker::PhantomData<U>,
}


impl<U> PartialEq for Unit<U> {
    fn eq(&self, other: &Self) -> bool {
        self.value.eq(&other.value)
    }

    fn ne(&self, other: &Self) -> bool {
        self.value.ne(&other.value)
    }
}

impl<U> PartialOrd for Unit<U> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(&other.value)
    }

    fn lt(&self, other: &Self) -> bool {
        self.value.lt(&other.value)
    }

    fn le(&self, other: &Self) -> bool {
        self.value.le(&other.value)
    }

    fn gt(&self, other: &Self) -> bool {
        self.value.gt(&other.value)
    }

    fn ge(&self, other: &Self) -> bool {
        self.value.ge(&other.value)
    }
}

impl<U> Clone for Unit<U> {
    fn clone(&self) -> Self {
        let Self { value, _phantom } = *self;
        Self { value, _phantom }
    }
}
impl<U> Copy for Unit<U> {}
impl<U> Debug for Unit<U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let type_name = std::any::type_name::<U>();
        write!(f, "Unit::<{}>::new({})", type_name, self.value)
    }
}

impl<U> Display for Unit<U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.value, f)
    }
}



impl Unit<Ratio<Px, Inch>> {
    /// Standard value used for pixel per inch. Cf [`Inch`] for more explanation
    pub const STANDARD_PPI : Self = Self::new(96.);
}

impl Unit<Ratio<Inch, Pt>> {
    /// 1 pt = 1 / 72 in. Cf [`Pt`] for more explanation
    pub const DTP : Self = Self::new(0.0138888888888888888888); // 1 / 72
}

impl Unit<Ratio<Px, Pt>> {
    // FIXME: not leveraging the type system to enforce correctness of this multiplication
    /// A standard conversion between points and vpixels.
    /// This is simply [`Unit::DTP`] multiplied by [`Unit::STANDARD_PPI`]
    pub fn standard_pt_to_px() -> Self {
        Unit::DTP * Unit::STANDARD_PPI.lift::<Pt>()
    } 
}

impl Unit<Ratio<Pt, Px>> {
    // FIXME: not leveraging the type system to enforce correctness of this multiplication
    /// A standard conversion between vpixels and points. The inverse of [`Unit::standard_pt_to_vpx`]
    pub fn standard_px_to_pt() -> Self {
        Unit::standard_pt_to_px().recip()
    } 
}

impl<U> Unit<U> {
    /// The zero value
    pub const ZERO : Self = Self::new(0.);

    /// Creates a value with unit from a unit-less value.
    /// To be used with care: you need to manually check that the value you pass is indeed in the right dimension.
    pub const fn new(value: f64) -> Self { 
        Self { 
            value, 
            _phantom: std::marker::PhantomData 
        } 
    }

    /// Is quantity equal to zero?
    pub fn is_zero(self) -> bool {
        self.value == 0.0
    }

    /// Converts a value to a unit-less value ("unsafe" since it removes information about dimensions)
    #[inline]
    pub const fn to_unitless(self) -> f64 
    { self.value }

    /// Like [`Unit::to_unitles`] but explicitly asks for the dimension to avoid errors
    #[inline]
    pub fn unitless(self, _unit : U) -> f64 
    { self.to_unitless() }

    /// Multiply value by a unitless value
    pub fn scale(self, scale : f64) -> Self {
        Self::new(self.value * scale)
    }

    /// Equivalent to [`f64::min`] for values with units
    pub fn min(self, other : Self) -> Self {
        Self::new(self.value.min(other.value))
    }

    /// Equivalent to [`f64::max`] for values with units
    pub fn max(self, other : Self) -> Self {
        Self::new(self.value.max(other.value))
    }

    /// Equivalent to [`f64::abs`] for values with units
    pub fn abs(self) -> Self {
        Self::new(self.value.abs())
    }
}

impl<U> Unit<Ratio<U, U>> {
    /// Converts a unitless value to a float
    pub const fn as_unitless(self) -> f64 {
        self.to_unitless()
    }
}

impl<U, V> Unit<Ratio<U, V>> {
    /// Inverts a ratio going from `x` (unit: U/V) to `1/x` (unit: V/U)
    #[inline]
    pub fn recip(self) -> Unit<Ratio<V, U>> {
        Unit::<Ratio<V, U>>::new(self.value.recip())
    }

    /// Converts from U / V to (U / W) / (V / W).  
    ///
    /// This can be used to multiply two ratios together, for instance:
    ///
    /// ```
    /// # use rex::dimensions::units::{Ratio, Pt, Em, Inch};
    /// # use rex::dimensions::Unit;
    /// let x : Unit<Ratio<Pt, Em>>   = Unit::new(4. / 3.);
    /// let y : Unit<Ratio<Inch, Pt>> = Unit::new(5. / 4.);
    ///
    /// // let z : Unit<Ratio<Inch, Em>> = x * y; // <- mismatched type error 
    /// let z : Unit<Ratio<Inch, Em>> = x * y.lift::<Em>(); // ok 
    /// ```
    #[inline]
    pub const fn lift<W>(self) -> Unit<Ratio<Ratio<U, W>, Ratio<V, W>>> {
        Unit::new(self.value)
    }
}

impl<U, V, W> Unit<Ratio<Ratio<U, W>, Ratio<V, W>>> {
    /// Converts from (U / W) / (V / W) to U / V. Inverse of [`Unit::lift`].  
    #[inline]
    pub const fn unlift(self) -> Unit<Ratio<U, V>> {
        Unit::new(self.value)
    }
}

impl<U> Add for Unit<U> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.value + rhs.value)
    }
}

impl<U> Sub for Unit<U> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.value - rhs.value)
    }
}

impl<U> Neg for Unit<U> {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(- self.value)
    }
    // add code here
}

impl<U> AddAssign for Unit<U> {
    fn add_assign(&mut self, rhs: Self) {
        self.value += rhs.value;
    }
}

impl<U> SubAssign for Unit<U> {
    fn sub_assign(&mut self, rhs: Self) {
        self.value -= rhs.value;
    }
}


impl<U, V> Mul<Unit<Ratio<U, V>>> for Unit<V> {
    type Output = Unit<U>;

    fn mul(self, rhs: Unit<Ratio<U, V>>) -> Self::Output {
        Unit::<U>::new(self.value * rhs.value)
    }
}





impl<U, V> Div<Unit<V>> for Unit<U> {
    type Output = Unit<Ratio<U, V>>;

    fn div(self, rhs: Unit<V>) -> Self::Output {
        Unit::<Ratio<U, V>>::new(self.value / rhs.value)
    }
}



impl<U> From<f64> for Unit<U> {
    fn from(x: f64) -> Self 
    { Unit { value: x, _phantom: std::marker::PhantomData } }
}

impl<U> From<i32> for Unit<U> {
    fn from(x: i32) -> Self 
    { Unit { value: x.into(), _phantom: std::marker::PhantomData } }
}

impl<U> From<i16> for Unit<U> {
    fn from(x: i16) -> Self 
    { Unit { value: x.into(), _phantom: std::marker::PhantomData } }
}

impl<U> From<u32> for Unit<U> {
    fn from(x: u32) -> Self 
    { Unit { value: x.into(), _phantom: std::marker::PhantomData } }
}

impl<U> From<u16> for Unit<U> {
    fn from(x: u16) -> Self 
    { Unit { value: x.into(), _phantom: std::marker::PhantomData } }
}


impl<U> Sum for Unit<U> {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(
            Unit::ZERO,
            |a, b| a + b
        )
    }
}

/// A type for quantities along with their unit
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum AnyUnit {
    /// em
    Em(f64),
    /// pixels
    Px(f64)
}

impl AnyUnit {
    /// Check if the length represented is negative. 
    /// (We implicitly assume that all units represented within this type are *positive* multiples of each other)
    pub fn is_negative(&self) -> bool {
        match self {
            AnyUnit::Em(val) => val.is_sign_negative(),
            AnyUnit::Px(val) => val.is_sign_negative(),
        }
    }
}

impl Display for AnyUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnyUnit::Em(value) => write!(f, "{}em", value),
            AnyUnit::Px(value) => write!(f, "{}px", value),
        }
    }
}

impl From<Unit<Em>> for AnyUnit {
    fn from(value: Unit<Em>) -> Self {
        Self::Em(value.unitless(Em))
    }
}

impl From<Unit<Px>> for AnyUnit {
    fn from(value: Unit<Px>) -> Self {
        Self::Px(value.unitless(Px))
    }
}