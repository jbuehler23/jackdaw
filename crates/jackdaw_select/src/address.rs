//! Rule-based addressing of brush elements: which faces, edges or vertices of a
//! [`BrushTopology`] a [`Selector`] picks. Faces are polygon indices, vertices
//! vertex indices and edges indices into `BrushTopology::edges`, the same index
//! spaces a topology read reports. Every rule in a selector must hold for an
//! element to be picked.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;

use glam::{Affine3A, Vec3};
use jackdaw_geometry::BrushTopology;
use jackdaw_geometry::halfedge::HalfedgeBinding;

/// Default angle between a wanted normal or direction and an element's, in
/// degrees.
pub const DEFAULT_TOLERANCE_DEG: f32 = 5.0;

/// Default distance from the extreme an element may sit and still count.
pub const DEFAULT_EPSILON: f32 = 1e-3;

/// Slack for an element sitting exactly on a box face.
const BOX_SLACK: f32 = 1e-5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementKind {
    Face,
    Edge,
    Vertex,
}

impl ElementKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Face => "face",
            Self::Edge => "edge",
            Self::Vertex => "vertex",
        }
    }
}

/// A direction and how far from it an element may point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    pub direction: Vec3,
    pub tolerance_deg: f32,
}

/// The elements at the low or high end of one local axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extreme {
    /// 0 for x, 1 for y, 2 for z.
    pub axis: usize,
    pub max: bool,
    pub epsilon: f32,
}

/// An axis-aligned box. `world` boxes are tested against the brush's
/// vertices placed by the transform handed to [`resolve`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InsideBox {
    pub min: Vec3,
    pub max: Vec3,
    pub world: bool,
}

/// The rules an element must all satisfy. Directions, extremes and local boxes
/// are in the brush's own space.
///
/// - `normal`: a face whose normal lies in the cone; an edge or vertex of such
///   a face.
/// - `direction`: an edge running along the cone's axis, either way round.
/// - `extreme`: an element lying wholly within `epsilon` of the lowest or
///   highest vertex along the axis.
/// - `inside_box`: an element whose every vertex is inside the box.
/// - `loop_of`: the edge loop through an edge, or the vertices on it.
/// - `all`: every element.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selector {
    pub indices: Option<Vec<usize>>,
    pub normal: Option<Cone>,
    pub direction: Option<Cone>,
    pub extreme: Option<Extreme>,
    pub inside_box: Option<InsideBox>,
    pub loop_of: Option<usize>,
    pub all: bool,
}

impl Selector {
    /// Whether the selector names no rule, so it would pick nothing meaningful.
    pub fn is_empty(&self) -> bool {
        self.indices.is_none()
            && self.normal.is_none()
            && self.direction.is_none()
            && self.extreme.is_none()
            && self.inside_box.is_none()
            && self.loop_of.is_none()
            && !self.all
    }
}

/// Why a selector picks nothing meaningful.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectError {
    /// The selector names no rule at all.
    NoRule,
    /// An index past the end of the elements of its kind.
    OutOfRange {
        kind: ElementKind,
        index: usize,
        count: usize,
    },
    /// A rule that does not apply to elements of this kind.
    NotForKind {
        rule: &'static str,
        kind: ElementKind,
    },
    /// A direction of zero length.
    ZeroDirection { rule: &'static str },
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRule => f.write_str("give `indices` or at least one `where` rule"),
            Self::OutOfRange { kind, index, count } => write!(
                f,
                "{} index {index} is out of range; the brush has {count}",
                kind.name()
            ),
            Self::NotForKind { rule, kind } => {
                write!(f, "`{rule}` does not select {}s", kind.name())
            }
            Self::ZeroDirection { rule } => write!(f, "`{rule}` needs a non-zero direction"),
        }
    }
}

impl std::error::Error for SelectError {}

/// How a resolved set combines with the current selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Combine {
    #[default]
    Set,
    Add,
    Remove,
}

/// Combine `resolved` with `current`. `Set` replaces, `Add` appends what is
/// new in resolved order, `Remove` drops what was resolved.
pub fn combine<T: Eq + Hash + Copy>(current: &[T], resolved: &[T], how: Combine) -> Vec<T> {
    match how {
        Combine::Set => {
            let mut seen = HashSet::with_capacity(resolved.len());
            resolved
                .iter()
                .copied()
                .filter(|item| seen.insert(*item))
                .collect()
        }
        Combine::Add => {
            let mut seen: HashSet<T> = current.iter().copied().collect();
            let mut out = current.to_vec();
            out.extend(resolved.iter().copied().filter(|item| seen.insert(*item)));
            out
        }
        Combine::Remove => {
            let dropped: HashSet<T> = resolved.iter().copied().collect();
            current
                .iter()
                .copied()
                .filter(|item| !dropped.contains(item))
                .collect()
        }
    }
}

/// The sorted indices of the `kind` elements of `topology` that every rule
/// of `selector` admits. `to_world` places the brush for world boxes.
pub fn resolve(
    topology: &BrushTopology,
    kind: ElementKind,
    selector: &Selector,
    to_world: Affine3A,
) -> Result<Vec<usize>, SelectError> {
    if selector.is_empty() {
        return Err(SelectError::NoRule);
    }
    if selector.direction.is_some() && kind != ElementKind::Edge {
        return Err(SelectError::NotForKind {
            rule: "direction",
            kind,
        });
    }
    if selector.loop_of.is_some() && kind == ElementKind::Face {
        return Err(SelectError::NotForKind {
            rule: "loop_of",
            kind,
        });
    }
    for (rule, cone) in [
        ("normal", selector.normal),
        ("direction", selector.direction),
    ] {
        if cone.is_some_and(|cone| cone.direction.length_squared() < 1e-12) {
            return Err(SelectError::ZeroDirection { rule });
        }
    }

    let count = element_count(topology, kind);
    if let Some(indices) = &selector.indices
        && let Some(&index) = indices.iter().find(|&&index| index >= count)
    {
        return Err(SelectError::OutOfRange { kind, index, count });
    }
    let edge_count = topology.edges.len();
    if let Some(edge) = selector.loop_of
        && edge >= edge_count
    {
        return Err(SelectError::OutOfRange {
            kind: ElementKind::Edge,
            index: edge,
            count: edge_count,
        });
    }

    let elements = Elements::new(topology, to_world);
    let mut keep = vec![true; count];
    if let Some(indices) = &selector.indices {
        let mut named = vec![false; count];
        for &index in indices {
            named[index] = true;
        }
        and(&mut keep, &named);
    }
    if let Some(cone) = selector.normal {
        let facing: Vec<bool> = elements
            .face_normals
            .iter()
            .map(|normal| within(*normal, cone, false))
            .collect();
        let admitted = elements.through_faces(kind, &facing);
        and(&mut keep, &admitted);
    }
    if let Some(cone) = selector.direction {
        let along: Vec<bool> = topology
            .edges
            .iter()
            .map(|edge| {
                let a = elements.local[edge.v[0] as usize];
                let b = elements.local[edge.v[1] as usize];
                within(b - a, cone, true)
            })
            .collect();
        and(&mut keep, &along);
    }
    if let Some(extreme) = selector.extreme {
        let axis = extreme.axis.min(2);
        let values = elements.local.iter().map(|p| p[axis]);
        let end = if extreme.max {
            values.fold(f32::MIN, f32::max)
        } else {
            values.fold(f32::MAX, f32::min)
        };
        let at_end: Vec<bool> = elements
            .local
            .iter()
            .map(|p| (p[axis] - end).abs() <= extreme.epsilon)
            .collect();
        let admitted = elements.wholly(kind, &at_end);
        and(&mut keep, &admitted);
    }
    if let Some(region) = selector.inside_box {
        let points = if region.world {
            &elements.world
        } else {
            &elements.local
        };
        let inside: Vec<bool> = points
            .iter()
            .map(|p| p.cmpge(region.min - BOX_SLACK).all() && p.cmple(region.max + BOX_SLACK).all())
            .collect();
        let admitted = elements.wholly(kind, &inside);
        and(&mut keep, &admitted);
    }
    if let Some(edge) = selector.loop_of {
        let on_loop = loop_through(topology, edge);
        let admitted = match kind {
            ElementKind::Edge => on_loop,
            _ => elements.vertices_of_edges(&on_loop),
        };
        and(&mut keep, &admitted);
    }

    Ok(keep
        .iter()
        .enumerate()
        .filter_map(|(index, kept)| kept.then_some(index))
        .collect())
}

fn and(keep: &mut [bool], admitted: &[bool]) {
    for (kept, admitted) in keep.iter_mut().zip(admitted) {
        *kept &= *admitted;
    }
}

fn element_count(topology: &BrushTopology, kind: ElementKind) -> usize {
    match kind {
        ElementKind::Face => topology.polygons.len(),
        ElementKind::Edge => topology.edges.len(),
        ElementKind::Vertex => topology.vertices.len(),
    }
}

/// Whether `vector` lies within `cone`; `either_way` also accepts the
/// opposite direction.
fn within(vector: Vec3, cone: Cone, either_way: bool) -> bool {
    let (Some(vector), Some(wanted)) = (vector.try_normalize(), cone.direction.try_normalize())
    else {
        return false;
    };
    let cos = vector.dot(wanted);
    let cos = if either_way { cos.abs() } else { cos };
    let limit = cone.tolerance_deg.clamp(0.0, 180.0).to_radians().cos();
    cos >= limit - 1e-6
}

/// The edges of the loop through `edge`, the seed included.
fn loop_through(topology: &BrushTopology, edge: usize) -> Vec<bool> {
    let mut on_loop = vec![false; topology.edges.len()];
    on_loop[edge] = true;
    let seed = topology.edges[edge].v;
    let binding = HalfedgeBinding::lift_from_topology(topology);
    let walked = crate::loop_edges(
        &binding.mesh,
        &binding.vert_keys,
        &[(seed[0] as usize, seed[1] as usize)],
    );
    let index_of = edge_indices(topology);
    for pair in walked {
        if let Some(&index) = index_of.get(&pair) {
            on_loop[index] = true;
        }
    }
    on_loop
}

/// Each edge's index by its ordered vertex pair.
pub fn edge_indices(topology: &BrushTopology) -> HashMap<(usize, usize), usize> {
    topology
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| ((edge.v[0] as usize, edge.v[1] as usize), index))
        .collect()
}

/// The geometry the rules read, computed once per call.
struct Elements<'a> {
    topology: &'a BrushTopology,
    local: Vec<Vec3>,
    world: Vec<Vec3>,
    face_normals: Vec<Vec3>,
}

impl<'a> Elements<'a> {
    fn new(topology: &'a BrushTopology, to_world: Affine3A) -> Self {
        let local: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();
        let world = local
            .iter()
            .map(|p| to_world.transform_point3(*p))
            .collect();
        let face_normals = (0..topology.polygons.len())
            .map(|face| {
                if topology.polygons[face].loop_total < 3 {
                    Vec3::ZERO
                } else {
                    topology.face_normal_with(&local, face)
                }
            })
            .collect();
        Self {
            topology,
            local,
            world,
            face_normals,
        }
    }

    fn face_loops(&self, face: usize) -> &[jackdaw_geometry::MeshLoop] {
        let poly = &self.topology.polygons[face];
        let start = poly.loop_start as usize;
        &self.topology.loops[start..start + poly.loop_total as usize]
    }

    /// Faces flagged in `faces`, or the edges or vertices of any of them.
    fn through_faces(&self, kind: ElementKind, faces: &[bool]) -> Vec<bool> {
        if kind == ElementKind::Face {
            return faces.to_vec();
        }
        let mut out = vec![false; element_count(self.topology, kind)];
        for (face, _) in faces.iter().enumerate().filter(|(_, flagged)| **flagged) {
            for corner in self.face_loops(face) {
                let index = match kind {
                    ElementKind::Edge => corner.edge,
                    _ => corner.vert,
                } as usize;
                if let Some(slot) = out.get_mut(index) {
                    *slot = true;
                }
            }
        }
        out
    }

    /// Elements of `kind` all of whose vertices are flagged in `verts`.
    fn wholly(&self, kind: ElementKind, verts: &[bool]) -> Vec<bool> {
        let flagged = |vert: u32| verts.get(vert as usize).copied().unwrap_or(false);
        match kind {
            ElementKind::Vertex => verts.to_vec(),
            ElementKind::Edge => self
                .topology
                .edges
                .iter()
                .map(|edge| flagged(edge.v[0]) && flagged(edge.v[1]))
                .collect(),
            ElementKind::Face => (0..self.topology.polygons.len())
                .map(|face| {
                    let corners = self.face_loops(face);
                    !corners.is_empty() && corners.iter().all(|corner| flagged(corner.vert))
                })
                .collect(),
        }
    }

    fn vertices_of_edges(&self, edges: &[bool]) -> Vec<bool> {
        let mut out = vec![false; self.local.len()];
        for (edge, _) in edges.iter().enumerate().filter(|(_, flagged)| **flagged) {
            for vert in self.topology.edges[edge].v {
                out[vert as usize] = true;
            }
        }
        out
    }
}

/// A fingerprint of `topology`'s structure: vertex count, edges and polygon
/// rings. Moving vertices leaves it alone; anything that renumbers elements
/// changes it, so a caller holding indices can tell they went stale. Kept to
/// 53 bits so a JSON number carries it exactly.
pub fn topology_revision(topology: &BrushTopology) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut feed = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    feed(topology.vertices.len() as u64);
    feed(topology.edges.len() as u64);
    for edge in &topology.edges {
        feed(u64::from(edge.v[0]) << 32 | u64::from(edge.v[1]));
    }
    feed(topology.polygons.len() as u64);
    for face in 0..topology.polygons.len() {
        let poly = &topology.polygons[face];
        feed(u64::from(poly.loop_total));
        let start = poly.loop_start as usize;
        for corner in &topology.loops[start..start + poly.loop_total as usize] {
            feed(u64::from(corner.vert));
        }
    }
    hash & ((1 << 53) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_geometry::{
        build_topology_from_face_polygons, compute_brush_topology, cuboid_faces,
    };

    fn cube() -> BrushTopology {
        compute_brush_topology(&cuboid_faces(Vec3::splat(0.5)))
    }

    /// An upright prism of `sides` around y, radius 1, from y=-1 to y=1.
    fn cylinder(sides: usize) -> BrushTopology {
        let mut positions = Vec::new();
        for y in [-1.0, 1.0] {
            for i in 0..sides {
                let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                positions.push(Vec3::new(a.cos(), y, -a.sin()));
            }
        }
        let mut rings = vec![
            (0..sides).rev().collect::<Vec<_>>(),
            (sides..2 * sides).collect::<Vec<_>>(),
        ];
        for i in 0..sides {
            let j = (i + 1) % sides;
            rings.push(vec![i, j, sides + j, sides + i]);
        }
        build_topology_from_face_polygons(positions, rings)
    }

    fn select(topology: &BrushTopology, kind: ElementKind, selector: Selector) -> Vec<usize> {
        resolve(topology, kind, &selector, Affine3A::IDENTITY).expect("resolves")
    }

    fn up(tolerance_deg: f32) -> Option<Cone> {
        Some(Cone {
            direction: Vec3::Y,
            tolerance_deg,
        })
    }

    fn face_with_normal(topology: &BrushTopology, normal: Vec3) -> usize {
        (0..topology.polygons.len())
            .find(|&face| topology.face_normal(face).abs_diff_eq(normal, 1e-4))
            .expect("a face with that normal")
    }

    fn verts_where(topology: &BrushTopology, keep: impl Fn(Vec3) -> bool) -> Vec<usize> {
        (0..topology.vertices.len())
            .filter(|&v| keep(topology.vertices[v].position))
            .collect()
    }

    #[test]
    fn the_fixtures_have_the_expected_counts() {
        let cube = cube();
        assert_eq!(
            (cube.vertices.len(), cube.edges.len(), cube.polygons.len()),
            (8, 12, 6)
        );
        let cylinder = cylinder(16);
        assert_eq!(
            (
                cylinder.vertices.len(),
                cylinder.edges.len(),
                cylinder.polygons.len()
            ),
            (32, 48, 18)
        );
    }

    #[test]
    fn normal_up_picks_the_top_face_of_a_cube() {
        let cube = cube();
        let top = face_with_normal(&cube, Vec3::Y);
        let picked = select(
            &cube,
            ElementKind::Face,
            Selector {
                normal: up(DEFAULT_TOLERANCE_DEG),
                ..Default::default()
            },
        );
        assert_eq!(picked, vec![top]);
    }

    #[test]
    fn normal_picks_the_edges_and_vertices_of_the_matching_faces() {
        let cube = cube();
        let selector = Selector {
            normal: up(DEFAULT_TOLERANCE_DEG),
            ..Default::default()
        };
        let verts = select(&cube, ElementKind::Vertex, selector.clone());
        assert_eq!(verts, verts_where(&cube, |p| p.y > 0.0));
        let edges = select(&cube, ElementKind::Edge, selector);
        assert_eq!(edges.len(), 4);
        for edge in edges {
            let [a, b] = cube.edges[edge].v;
            assert!(cube.vertices[a as usize].position.y > 0.0);
            assert!(cube.vertices[b as usize].position.y > 0.0);
        }
    }

    #[test]
    fn the_tolerance_widens_the_cone_on_a_cylinder() {
        let cylinder = cylinder(16);
        let side = Some(Cone {
            direction: Vec3::X,
            tolerance_deg: 5.0,
        });
        let narrow = select(
            &cylinder,
            ElementKind::Face,
            Selector {
                normal: side,
                ..Default::default()
            },
        );
        assert!(
            narrow.is_empty(),
            "no side faces straight along +x: {narrow:?}"
        );

        // Side normals sit at 11.25 + k * 22.5 degrees from +x.
        let wide = select(
            &cylinder,
            ElementKind::Face,
            Selector {
                normal: Some(Cone {
                    direction: Vec3::X,
                    tolerance_deg: 45.0,
                }),
                ..Default::default()
            },
        );
        assert_eq!(wide.len(), 4, "{wide:?}");
        for face in wide {
            assert!(cylinder.face_normal(face).x > 0.7);
        }

        let caps = select(
            &cylinder,
            ElementKind::Face,
            Selector {
                normal: up(45.0),
                ..Default::default()
            },
        );
        assert_eq!(caps, vec![1]);
    }

    #[test]
    fn a_zero_tolerance_still_matches_an_exact_normal() {
        let cube = cube();
        let picked = select(
            &cube,
            ElementKind::Face,
            Selector {
                normal: up(0.0),
                ..Default::default()
            },
        );
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn a_ninety_degree_tolerance_takes_the_sides_but_not_the_bottom() {
        let cube = cube();
        let picked = select(
            &cube,
            ElementKind::Face,
            Selector {
                normal: up(90.0),
                ..Default::default()
            },
        );
        assert_eq!(picked.len(), 5);
        assert!(!picked.contains(&face_with_normal(&cube, Vec3::NEG_Y)));
    }

    #[test]
    fn direction_picks_edges_along_an_axis_either_way() {
        let cylinder = cylinder(16);
        let vertical = select(
            &cylinder,
            ElementKind::Edge,
            Selector {
                direction: Some(Cone {
                    direction: Vec3::NEG_Y,
                    tolerance_deg: DEFAULT_TOLERANCE_DEG,
                }),
                ..Default::default()
            },
        );
        assert_eq!(vertical.len(), 16);
        for edge in vertical {
            let [a, b] = cylinder.edges[edge].v;
            let run =
                cylinder.vertices[b as usize].position - cylinder.vertices[a as usize].position;
            assert!(run.x.abs() < 1e-5 && run.z.abs() < 1e-5);
        }
    }

    #[test]
    fn direction_is_refused_for_faces_and_vertices() {
        let cube = cube();
        let selector = Selector {
            direction: up(5.0),
            ..Default::default()
        };
        for kind in [ElementKind::Face, ElementKind::Vertex] {
            assert_eq!(
                resolve(&cube, kind, &selector, Affine3A::IDENTITY),
                Err(SelectError::NotForKind {
                    rule: "direction",
                    kind
                })
            );
        }
    }

    #[test]
    fn extreme_max_y_picks_the_top_of_a_cube() {
        let cube = cube();
        let selector = Selector {
            extreme: Some(Extreme {
                axis: 1,
                max: true,
                epsilon: DEFAULT_EPSILON,
            }),
            ..Default::default()
        };
        assert_eq!(
            select(&cube, ElementKind::Vertex, selector.clone()),
            verts_where(&cube, |p| p.y > 0.0)
        );
        assert_eq!(
            select(&cube, ElementKind::Face, selector.clone()),
            vec![face_with_normal(&cube, Vec3::Y)]
        );
        assert_eq!(select(&cube, ElementKind::Edge, selector).len(), 4);
    }

    #[test]
    fn extreme_epsilon_reaches_vertices_just_below_the_top() {
        let mut cube = cube();
        let dipped = verts_where(&cube, |p| p.y > 0.0)[0];
        cube.vertices[dipped].position.y -= 0.05;
        let at_top = |epsilon| {
            select(
                &cube,
                ElementKind::Vertex,
                Selector {
                    extreme: Some(Extreme {
                        axis: 1,
                        max: true,
                        epsilon,
                    }),
                    ..Default::default()
                },
            )
        };
        assert_eq!(at_top(0.01).len(), 3);
        assert_eq!(at_top(0.1).len(), 4);
    }

    #[test]
    fn inside_box_takes_only_wholly_enclosed_elements() {
        let cube = cube();
        let upper_half = Selector {
            inside_box: Some(InsideBox {
                min: Vec3::new(-1.0, 0.0, -1.0),
                max: Vec3::splat(1.0),
                world: false,
            }),
            ..Default::default()
        };
        assert_eq!(
            select(&cube, ElementKind::Vertex, upper_half.clone()).len(),
            4
        );
        assert_eq!(
            select(&cube, ElementKind::Edge, upper_half.clone()).len(),
            4
        );
        assert_eq!(
            select(&cube, ElementKind::Face, upper_half),
            vec![face_with_normal(&cube, Vec3::Y)]
        );
    }

    #[test]
    fn a_world_box_reads_the_placed_brush() {
        let cube = cube();
        let lifted = Affine3A::from_translation(Vec3::new(0.0, 10.0, 0.0));
        let selector = Selector {
            inside_box: Some(InsideBox {
                min: Vec3::new(-1.0, 10.0, -1.0),
                max: Vec3::new(1.0, 11.0, 1.0),
                world: true,
            }),
            ..Default::default()
        };
        let picked = resolve(&cube, ElementKind::Vertex, &selector, lifted).expect("resolves");
        assert_eq!(picked, verts_where(&cube, |p| p.y > 0.0));
        let unplaced =
            resolve(&cube, ElementKind::Vertex, &selector, Affine3A::IDENTITY).expect("resolves");
        assert!(unplaced.is_empty());
    }

    #[test]
    fn loop_of_matches_the_loop_walk_and_keeps_its_seed() {
        let cylinder = cylinder(16);
        let seed = (0..cylinder.edges.len())
            .find(|&edge| {
                let [a, b] = cylinder.edges[edge].v;
                (a as usize) < 16 && (b as usize) >= 16
            })
            .expect("a vertical edge");
        let looped = select(
            &cylinder,
            ElementKind::Edge,
            Selector {
                loop_of: Some(seed),
                ..Default::default()
            },
        );
        assert!(looped.contains(&seed));

        let binding = HalfedgeBinding::lift_from_topology(&cylinder);
        let [a, b] = cylinder.edges[seed].v;
        let mut walked: Vec<usize> = crate::loop_edges(
            &binding.mesh,
            &binding.vert_keys,
            &[(a as usize, b as usize)],
        )
        .into_iter()
        .filter_map(|(a, b)| cylinder.edge_id(a as u32, b as u32))
        .map(|edge| edge as usize)
        .collect();
        walked.push(seed);
        walked.sort_unstable();
        walked.dedup();
        assert_eq!(looped, walked);

        let verts = select(
            &cylinder,
            ElementKind::Vertex,
            Selector {
                loop_of: Some(seed),
                ..Default::default()
            },
        );
        assert!(verts.contains(&(a as usize)) && verts.contains(&(b as usize)));
    }

    #[test]
    fn all_takes_every_element() {
        let cube = cube();
        let all = Selector {
            all: true,
            ..Default::default()
        };
        assert_eq!(select(&cube, ElementKind::Face, all.clone()).len(), 6);
        assert_eq!(select(&cube, ElementKind::Edge, all.clone()).len(), 12);
        assert_eq!(select(&cube, ElementKind::Vertex, all).len(), 8);
    }

    #[test]
    fn rules_combine_as_and() {
        let cylinder = cylinder(16);
        let top_rim = select(
            &cylinder,
            ElementKind::Edge,
            Selector {
                normal: up(5.0),
                extreme: Some(Extreme {
                    axis: 0,
                    max: true,
                    epsilon: 0.5,
                }),
                ..Default::default()
            },
        );
        assert!(!top_rim.is_empty());
        for edge in &top_rim {
            for vert in cylinder.edges[*edge].v {
                let p = cylinder.vertices[vert as usize].position;
                assert!(p.y > 0.0 && p.x >= 0.5, "{p}");
            }
        }

        let named = select(
            &cylinder,
            ElementKind::Face,
            Selector {
                indices: Some(vec![0, 1, 2]),
                normal: up(5.0),
                ..Default::default()
            },
        );
        assert_eq!(named, vec![1]);
    }

    #[test]
    fn indices_alone_select_exactly_those() {
        let cube = cube();
        let picked = select(
            &cube,
            ElementKind::Edge,
            Selector {
                indices: Some(vec![7, 2, 2]),
                ..Default::default()
            },
        );
        assert_eq!(picked, vec![2, 7]);
    }

    #[test]
    fn out_of_range_indices_and_loop_seeds_are_refused() {
        let cube = cube();
        let selector = Selector {
            indices: Some(vec![0, 6]),
            ..Default::default()
        };
        assert_eq!(
            resolve(&cube, ElementKind::Face, &selector, Affine3A::IDENTITY),
            Err(SelectError::OutOfRange {
                kind: ElementKind::Face,
                index: 6,
                count: 6
            })
        );
        let selector = Selector {
            loop_of: Some(12),
            ..Default::default()
        };
        assert!(matches!(
            resolve(&cube, ElementKind::Edge, &selector, Affine3A::IDENTITY),
            Err(SelectError::OutOfRange { index: 12, .. })
        ));
    }

    #[test]
    fn an_empty_selector_and_a_zero_direction_are_refused() {
        let cube = cube();
        assert_eq!(
            resolve(
                &cube,
                ElementKind::Face,
                &Selector::default(),
                Affine3A::IDENTITY
            ),
            Err(SelectError::NoRule)
        );
        let selector = Selector {
            normal: Some(Cone {
                direction: Vec3::ZERO,
                tolerance_deg: 5.0,
            }),
            ..Default::default()
        };
        assert_eq!(
            resolve(&cube, ElementKind::Face, &selector, Affine3A::IDENTITY),
            Err(SelectError::ZeroDirection { rule: "normal" })
        );
    }

    #[test]
    fn combine_sets_adds_and_removes() {
        assert_eq!(combine(&[1, 2], &[3], Combine::Set), vec![3]);
        assert_eq!(combine(&[1, 2], &[2, 3], Combine::Add), vec![1, 2, 3]);
        assert_eq!(combine(&[1, 2, 3], &[2], Combine::Remove), vec![1, 3]);
    }

    #[test]
    fn the_revision_follows_structure_not_positions() {
        let cube = cube();
        let base = topology_revision(&cube);
        assert_eq!(base, topology_revision(&cube.clone()));
        let mut moved = cube.clone();
        moved.vertices[0].position += Vec3::X;
        assert_eq!(base, topology_revision(&moved));
        assert_ne!(base, topology_revision(&cylinder(4)));
    }
}
