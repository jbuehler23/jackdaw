//! Newell's method for robust polygon-normal computation.
//! Sums (yi + yi+1)(zi - zi+1) etc. across all edges; works for
//! non-planar polygons and is stable on near-degenerate inputs.

use bevy::math::Vec3;

pub fn newell_normal(ring: &[Vec3]) -> Vec3 {
    let n = ring.len();
    if n < 3 {
        return Vec3::ZERO;
    }
    let mut acc = Vec3::ZERO;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        acc.x += (a.y - b.y) * (a.z + b.z);
        acc.y += (a.z - b.z) * (a.x + b.x);
        acc.z += (a.x - b.x) * (a.y + b.y);
    }
    acc.normalize_or_zero()
}
