//! Floating-point operations under the names the standard library gives
//! them, adapted from `bevy_math::ops`.

/// Computes the cosine of `x` (in radians).
#[inline(always)]
pub(crate) fn cos(x: f32) -> f32 {
    f32::cos(x)
}

/// Computes the absolute value of `x`.
#[inline(always)]
pub(crate) fn abs(x: f32) -> f32 {
    f32::abs(x)
}

/// Returns the largest integer less than or equal to `x`.
#[inline(always)]
pub(crate) fn floor(x: f32) -> f32 {
    f32::floor(x)
}

/// Returns the smallest integer greater than or equal to `x`.
#[inline(always)]
pub(crate) fn ceil(x: f32) -> f32 {
    f32::ceil(x)
}
