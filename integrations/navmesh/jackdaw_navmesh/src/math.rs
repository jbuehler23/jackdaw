use glam::{U16Vec2, UVec3, Vec2, Vec3, Vec3A};

/// A 3D axis-aligned bounding box
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Aabb3d {
    /// The minimum point of the box
    pub min: Vec3,
    /// The maximum point of the box
    pub max: Vec3,
}

impl Aabb3d {
    /// Constructs an AABB from its center and half-size.
    #[inline]
    pub fn new(center: impl Into<Vec3>, half_size: impl Into<Vec3>) -> Self {
        let (center, half_size) = (center.into(), half_size.into());
        debug_assert!(half_size.x >= 0.0 && half_size.y >= 0.0 && half_size.z >= 0.0);
        Self {
            min: center - half_size,
            max: center + half_size,
        }
    }

    /// Constructs an AABB from a list of vertices.
    ///
    /// Returns `None` if the list of vertices is empty.
    #[inline]
    pub(crate) fn from_verts(verts: &[Vec3A]) -> Option<Self> {
        if verts.is_empty() {
            return None;
        }
        let min = verts
            .iter()
            .fold(Vec3A::splat(f32::MAX), |acc, &v| acc.min(v));
        let max = verts
            .iter()
            .fold(Vec3A::splat(f32::MIN), |acc, &v| acc.max(v));
        Some(Self {
            min: min.into(),
            max: max.into(),
        })
    }

    /// Checks if this AABB intersects with another AABB.
    #[inline]
    pub(crate) fn intersects(&self, other: &Aabb3d) -> bool {
        self.min.cmple(other.max).all() && self.max.cmpge(other.min).all()
    }
}

pub(crate) trait TriangleIndices {
    fn normal(&self, vertices: &[Vec3A]) -> Vec3A;
}

impl TriangleIndices for UVec3 {
    #[inline]
    fn normal(&self, vertices: &[Vec3A]) -> Vec3A {
        let a = vertices[self[0] as usize];
        let b = vertices[self[1] as usize];
        let c = vertices[self[2] as usize];
        let ab = b - a;
        let ac = c - a;
        ab.cross(ac).normalize_or_zero()
    }
}

pub(crate) trait TriangleVertices {
    fn aabb(&self) -> Aabb3d;
}

impl TriangleVertices for [Vec3A; 3] {
    #[inline]
    fn aabb(&self) -> Aabb3d {
        let min = self[0].min(self[1]).min(self[2]).into();
        let max = self[0].max(self[1]).max(self[2]).into();
        Aabb3d { min, max }
    }
}

impl TriangleVertices for [Vec3; 3] {
    #[inline]
    fn aabb(&self) -> Aabb3d {
        let min = self[0].min(self[1]).min(self[2]);
        let max = self[0].max(self[1]).max(self[2]);
        Aabb3d { min, max }
    }
}

/// Gets the standard width (x-axis) offset for the specified direction.
/// # Arguments
/// - `direction`: The direction. [Limits: 0 <= value < 4]
/// # Returns
///
/// The width offset to apply to the current cell position to move in the direction.
#[inline]
pub(crate) fn dir_offset_x(direction: u8) -> i8 {
    const OFFSET: [i8; 4] = [-1, 0, 1, 0];
    OFFSET[direction as usize & 0x03]
}

/// Gets the standard height (z-axis) offset for the specified direction.
/// # Arguments
/// - `direction`: The direction. [Limits: 0 <= value < 4]
/// # Returns
///
/// The height offset to apply to the current cell position to move in the direction.
#[inline]
pub(crate) fn dir_offset_z(direction: u8) -> i8 {
    const OFFSET: [i8; 4] = [0, 1, 0, -1];
    OFFSET[direction as usize & 0x03]
}

#[inline]
pub(crate) fn dir_offset(offset_x: i32, offset_z: i32) -> i8 {
    const DIRS: [i8; 5] = [3, 0, -1, 2, 1];
    DIRS[(((offset_z + 1) << 1) + offset_x) as usize]
}

#[inline]
pub(crate) fn prev(i: usize, n: usize) -> usize {
    (i + n - 1) % n
}

#[inline]
pub(crate) fn next(i: usize, n: usize) -> usize {
    (i + 1) % n
}

pub(crate) fn distance_squared_between_point_and_line_u16vec2(
    point: U16Vec2,
    (p, q): (U16Vec2, U16Vec2),
) -> f32 {
    distance_squared_between_point_and_line_vec2(point.as_vec2(), (p.as_vec2(), q.as_vec2()))
}

pub(crate) fn distance_squared_between_point_and_line_vec2(pt: Vec2, (p, q): (Vec2, Vec2)) -> f32 {
    let pq = q - p;
    let dt = pt - p;
    let d = pq.length_squared();
    let mut t = pq.dot(dt);
    if d > 0.0 {
        t /= d;
    } else {
        tracing::error!(
            "distance_squared_between_point_and_line_vec2 was called with identical points as a line segment. The result might be unexpected."
        );
    }
    t = t.clamp(0.0, 1.0);
    let dt = p + t * pq - pt;
    dt.length_squared()
}

pub(crate) fn distance_squared_between_point_and_line_vec3(
    pt: Vec3A,
    (p, q): (Vec3A, Vec3A),
) -> f32 {
    let pq = q - p;
    let dt = pt - p;
    let d = pq.length_squared();
    let mut t = pq.dot(dt);
    if d > 0.0 {
        t /= d;
    } else {
        tracing::error!(
            "distance_squared_between_point_and_line_vec3 was called with identical points as a line segment. The result might be unexpected."
        );
    }
    t = t.clamp(0.0, 1.0);
    let dt = p + t * pq - pt;
    dt.length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prev() {
        assert_eq!(prev(0, 4), 3);
        assert_eq!(prev(1, 4), 0);
        assert_eq!(prev(2, 4), 1);
        assert_eq!(prev(3, 4), 2);
    }

    #[test]
    fn test_next() {
        assert_eq!(next(0, 4), 1);
        assert_eq!(next(1, 4), 2);
        assert_eq!(next(2, 4), 3);
        assert_eq!(next(3, 4), 0);
    }
}
