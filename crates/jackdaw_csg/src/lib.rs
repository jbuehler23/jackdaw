//! Mesh-CSG glue for jackdaw brushes.
//!
//! Wraps the
//! [`manifold-csg`](https://crates.io/crates/manifold-csg) Rust bindings to
//! [`manifold3d`](https://github.com/elalish/manifold) so brush boolean
//! operations (union, difference, intersection) work on arbitrary brush
//! topologies, not just convex half-space sets.
//!
//! ## Build prerequisites
//!
//! `manifold-csg` builds `manifold3d` from source on first compile. That
//! pulls in:
//!
//! - **cmake** (any recent version; pacman: `cmake`, apt: `cmake`)
//! - **a C++ compiler** (g++ or clang++)
//! - **git** (the build script clones manifold3d)
//!
//! Subsequent builds are cached. The C++ build takes around a minute on
//! a workstation; CI should cache `target/` accordingly.
//!
//! ## Conversion approach
//!
//! 1. **Brush -> Manifold:** triangulate each polygon face into a flat
//!    `(positions, triangles)` pair, then build a `Manifold` from those
//!    via `Manifold::from_mesh_f64`. No property channels are used; face
//!    identity is recovered on the way out by matching triangle planes
//!    to the union of the two source brushes' face planes.
//! 2. **Boolean op:** call `Manifold::{union,difference,intersection}`.
//! 3. **Manifold -> Brush:** read back the result mesh, group triangles
//!    by coplanar plane, match each plane to whichever source face it
//!    came from, then rebuild topology via
//!    `jackdaw_geometry::build_topology_from_face_polygons` so the result
//!    feeds back cleanly into later boolean ops.
//!
//! ## Status
//!
//! Used by `src/draw_brush.rs` for the four paradigm boolean sites
//! (Join / CSG Subtract cutters / CSG Subtract targets / CSG Intersect).
//! See `project_remote_game_integration.md` and the concave-by-default
//! audit doc in MEMORY.md for the broader rollout plan.

use bevy::math::{Quat, Vec2, Vec3};
use bevy::prelude::{Handle, StandardMaterial};
use jackdaw_geometry::{
    BrushFaceData, BrushPlane, BrushTopology, EPSILON, compute_face_tangent_axes, newell_normal,
    triangulate_face_polygon, triangulate_polygon_with_holes,
};
use manifold_csg::manifold::Manifold;

/// Which boolean op to apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BooleanOp {
    Union,
    Difference,
    Intersection,
}

/// Failure modes for the brush <-> manifold round-trip.
#[derive(Debug, Clone)]
pub enum CsgError {
    /// Brush had fewer than 4 vertices or 4 faces. Not a closed 3D solid.
    DegenerateBrush,
    /// The manifold3d kernel rejected the input. Wrapped string is its
    /// debug-display.
    ManifoldRejected(String),
    /// Boolean result was empty (no overlap, or full cancellation).
    /// Caller decides whether that's an error or expected.
    EmptyResult,
}

impl std::fmt::Display for CsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsgError::DegenerateBrush => {
                write!(f, "brush is degenerate (need >= 4 verts and faces)")
            }
            CsgError::ManifoldRejected(s) => write!(f, "manifold3d rejected input: {s}"),
            CsgError::EmptyResult => write!(f, "boolean produced empty result"),
        }
    }
}

impl std::error::Error for CsgError {}

/// Subset of a face's per-face state that must round-trip through the
/// manifold kernel. Owned copy of the source `BrushFaceData` so we can
/// reproject UVs and reattach materials after the boolean.
#[derive(Clone, Debug)]
struct FaceSlot {
    plane: BrushPlane,
    material: Handle<StandardMaterial>,
    uv_offset: Vec2,
    uv_scale: Vec2,
    uv_rotation: f32,
    uv_u_axis: Vec3,
    uv_v_axis: Vec3,
    is_cap: bool,
}

impl FaceSlot {
    fn from_face(face: &BrushFaceData) -> Self {
        Self {
            plane: face.plane.clone(),
            material: face.material.clone(),
            uv_offset: face.uv_offset,
            uv_scale: face.uv_scale,
            uv_rotation: face.uv_rotation,
            uv_u_axis: face.uv_u_axis,
            uv_v_axis: face.uv_v_axis,
            is_cap: face.is_cap,
        }
    }

    fn to_face(&self, plane: BrushPlane) -> BrushFaceData {
        BrushFaceData {
            plane,
            material: self.material.clone(),
            uv_offset: self.uv_offset,
            uv_scale: self.uv_scale,
            uv_rotation: self.uv_rotation,
            uv_u_axis: self.uv_u_axis,
            uv_v_axis: self.uv_v_axis,
            is_cap: self.is_cap,
        }
    }
}

/// A complete brush (faces + topology), the form `manifold_to_brush` produces.
#[derive(Clone, Debug, Default)]
pub struct CsgBrush {
    pub faces: Vec<BrushFaceData>,
    pub topology: BrushTopology,
}

/// World-space inputs to a boolean op. Each input owns its faces and
/// topology; the caller is expected to have already transformed them out
/// of local space if needed (see `brush_to_world`).
#[derive(Clone, Debug)]
pub struct CsgInput<'a> {
    pub faces: &'a [BrushFaceData],
    pub topology: &'a BrushTopology,
}

impl<'a> CsgInput<'a> {
    pub fn new(faces: &'a [BrushFaceData], topology: &'a BrushTopology) -> Self {
        Self { faces, topology }
    }
}

/// Output of `brush_to_manifold`: the kernel handle plus the source slot
/// table.
struct ConvertedInput {
    manifold: Manifold,
    /// Per-input-face slot data. After a boolean we use the plane of
    /// each output triangle to look up the matching FaceSlot from this
    /// vec (or the partner brush's vec).
    face_slots: Vec<FaceSlot>,
}

/// Tolerance for matching an output triangle's plane to a source face's
/// plane. Manifold's coplanar epsilon is relative to the bounding box;
/// we allow a slightly looser fixed tolerance since we operate in
/// editor-space (usually meters / units, not microns).
const PLANE_NORMAL_EPS: f32 = 1e-3;
const PLANE_DISTANCE_EPS: f32 = 1e-2;
const COPLANAR_NORMAL_EPS: f32 = 1e-3;
const COPLANAR_DISTANCE_EPS: f32 = 1e-2;
/// Snap-radius for welding output vertices that share a position.
const POS_DEDUP_EPS: f32 = 1e-4;
/// Tolerance applied to every manifold before a boolean op. Manifold uses
/// this distance (in world-space units) as the merge / coincidence radius
/// for vertices and edges at the cut boundary. Without setting it the
/// kernel falls back to a bounds-relative epsilon that, for a brush a
/// few units across, lands around 1e-7. That is far below `from_mesh_f64`
/// rounding noise (planes saved as f32 round into f64 with ~1e-7 slop),
/// which lets near-coincident verts produce sliver fragments and concave
/// "seep-through" artifacts at the cut plane. 1e-4 (0.1mm at meter scale)
/// is well below visible level-editor precision and well above f64 noise.
const MANIFOLD_TOLERANCE: f64 = 1e-4;

/// Convert one brush (faces + topology) into a manifold3d `Manifold`.
/// Face identity is preserved by recording each input face's plane in
/// `face_slots`; the reverse mapping is done by matching output triangle
/// planes to those slots.
fn brush_to_manifold(input: &CsgInput<'_>) -> Result<ConvertedInput, CsgError> {
    if input.topology.vertices.len() < 4 || input.faces.is_empty() {
        return Err(CsgError::DegenerateBrush);
    }

    // Flat vertex property buffer: just x, y, z (3 props per vert).
    // manifold-csg requires f64.
    let mut vert_props: Vec<f64> = Vec::with_capacity(input.topology.vertices.len() * 3);
    for v in &input.topology.vertices {
        vert_props.push(v.position.x as f64);
        vert_props.push(v.position.y as f64);
        vert_props.push(v.position.z as f64);
    }

    let positions: Vec<Vec3> = input.topology.vertices.iter().map(|v| v.position).collect();

    // Triangulate every face. We do not pack per-vertex property data
    // for face id (manifold merges vertices across faces, which makes
    // a per-vert slot lookup unreliable). Instead we match each output
    // triangle back to a source face by plane equation.
    let mut tri_indices: Vec<u64> = Vec::new();
    let mut face_slots: Vec<FaceSlot> = Vec::with_capacity(input.faces.len());

    for (face_idx, face) in input.faces.iter().enumerate() {
        face_slots.push(FaceSlot::from_face(face));

        if face_idx >= input.topology.polygons.len() {
            continue;
        }
        let poly = &input.topology.polygons[face_idx];
        let loop_start = poly.loop_start as usize;
        let loop_total = poly.loop_total as usize;
        if loop_total < 3 {
            continue;
        }
        let ring: Vec<u32> = (0..loop_total)
            .map(|i| input.topology.loops[loop_start + i].vert)
            .collect();
        let ring_positions: Vec<Vec3> = ring.iter().map(|&vi| positions[vi as usize]).collect();
        let normal = newell_normal(&ring_positions);
        if normal.length_squared() < 0.5 {
            continue;
        }
        // Triangulate using the polygon triangulator (handles concave rings).
        let local_tris = triangulate_face_polygon(&ring_positions, normal);
        for tri in &local_tris {
            let a = ring[tri[0] as usize] as u64;
            let b = ring[tri[1] as usize] as u64;
            let c = ring[tri[2] as usize] as u64;
            tri_indices.push(a);
            tri_indices.push(b);
            tri_indices.push(c);
        }
    }

    if tri_indices.is_empty() {
        return Err(CsgError::DegenerateBrush);
    }

    let manifold = Manifold::from_mesh_f64(&vert_props, 3, &tri_indices)
        .map_err(|e| CsgError::ManifoldRejected(format!("{e:?}")))?;

    // Set a fixed merge/coincidence tolerance before any boolean. See
    // `MANIFOLD_TOLERANCE` for the rationale. The kernel's bounds-relative
    // default is too tight for brush-scale geometry once f32 plane
    // distances have been round-tripped through f64.
    let manifold = manifold.set_tolerance(MANIFOLD_TOLERANCE);

    Ok(ConvertedInput {
        manifold,
        face_slots,
    })
}

/// Match the plane of an output triangle to a source `FaceSlot`. Both
/// runs are searched; the first slot whose plane lies within the
/// tolerance wins.
fn match_plane_to_slot<'a>(plane: &BrushPlane, runs: &[&'a [FaceSlot]]) -> Option<&'a FaceSlot> {
    for run in runs {
        for slot in run.iter() {
            // Manifold preserves orientation, so we expect normals to
            // match in sign as well as direction. But for a difference,
            // the cutter's faces become inward-facing surfaces of the
            // target; their normal flips. So accept both signs.
            let n_dot = slot.plane.normal.dot(plane.normal);
            if n_dot.abs() < 1.0 - PLANE_NORMAL_EPS {
                continue;
            }
            let expected_distance = if n_dot > 0.0 {
                slot.plane.distance
            } else {
                -slot.plane.distance
            };
            if (expected_distance - plane.distance).abs() < PLANE_DISTANCE_EPS {
                return Some(slot);
            }
        }
    }
    None
}

/// Recover a `CsgBrush` from a manifold3d result, using the source
/// brushes' face slots to reproject UVs.
fn manifold_to_brush(
    manifold: &Manifold,
    source_face_slot_runs: &[&[FaceSlot]],
) -> Result<CsgBrush, CsgError> {
    if manifold.is_empty() {
        return Err(CsgError::EmptyResult);
    }
    let (props, nprops, tris) = manifold.to_mesh_f64();
    if tris.len() < 3 {
        return Err(CsgError::EmptyResult);
    }

    let nverts = props.len() / nprops;
    let raw_positions: Vec<Vec3> = (0..nverts)
        .map(|vi| {
            let base = vi * nprops;
            Vec3::new(
                props[base] as f32,
                props[base + 1] as f32,
                props[base + 2] as f32,
            )
        })
        .collect();

    // Weld coincident verts. manifold can hand back duplicate positions
    // for verts that lie on a cut boundary; merging them on the way out
    // keeps the rebuilt topology clean enough to feed back through the
    // kernel on the next op.
    let (positions, remap) = dedup_positions(&raw_positions, POS_DEDUP_EPS);

    // Collect (plane, triangle) records, grouped by coplanar plane.
    struct TriRec {
        a: u32,
        b: u32,
        c: u32,
        plane: BrushPlane,
    }
    let mut tri_recs: Vec<TriRec> = Vec::with_capacity(tris.len() / 3);
    for tc in tris.chunks_exact(3) {
        let a = remap[tc[0] as usize];
        let b = remap[tc[1] as usize];
        let c = remap[tc[2] as usize];
        if a == b || b == c || a == c {
            continue;
        }
        let pa = positions[a as usize];
        let pb = positions[b as usize];
        let pc = positions[c as usize];
        let raw_normal = (pb - pa).cross(pc - pa);
        if raw_normal.length_squared() < 1e-12 {
            continue;
        }
        let normal = raw_normal.normalize();
        let distance = normal.dot(pa);
        tri_recs.push(TriRec {
            a,
            b,
            c,
            plane: BrushPlane { normal, distance },
        });
    }

    if tri_recs.is_empty() {
        return Err(CsgError::EmptyResult);
    }

    // Group triangles by coplanar plane (normal + distance with tolerance).
    struct FaceGroup {
        plane: BrushPlane,
        tris: Vec<(u32, u32, u32)>,
    }
    let mut groups: Vec<FaceGroup> = Vec::new();
    for tr in &tri_recs {
        let mut placed = false;
        for g in groups.iter_mut() {
            if g.plane.normal.dot(tr.plane.normal) > 1.0 - COPLANAR_NORMAL_EPS
                && (g.plane.distance - tr.plane.distance).abs() < COPLANAR_DISTANCE_EPS
            {
                g.tris.push((tr.a, tr.b, tr.c));
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(FaceGroup {
                plane: tr.plane.clone(),
                tris: vec![(tr.a, tr.b, tr.c)],
            });
        }
    }

    // For each coplanar group: recover all rings, classify nested rings as
    // holes via point-in-polygon containment, then:
    //   - simple ring (no holes): emit as a single face;
    //   - annulus (outer + holes): triangulate via earcut-with-holes and
    //     emit each triangle as its own simply-connected face (every face has a single boundary cycle, so all editmesh
    //     ops work without multi-cycle awareness).
    // Sub-faces of the same source coplanar group share material + UV via
    // `match_plane_to_slot`, so the visual is one continuous textured
    // surface even though it's stored as multiple polygons.
    let mut faces: Vec<BrushFaceData> = Vec::new();
    let mut face_rings: Vec<Vec<usize>> = Vec::new();
    for g in &groups {
        let raw_rings = recover_polygon_boundaries(&g.tris);
        if raw_rings.is_empty() {
            continue;
        }

        // Normalize every ring's winding to match the plane normal (CCW
        // viewed from +N). After this step, signed-area can't distinguish
        // outers from holes, so we use containment instead.
        let normalized_rings: Vec<Vec<u32>> = raw_rings
            .into_iter()
            .filter_map(|mut r| {
                if r.len() < 3 {
                    return None;
                }
                let ring_positions: Vec<Vec3> =
                    r.iter().map(|&vi| positions[vi as usize]).collect();
                let ring_normal = newell_normal(&ring_positions);
                if ring_normal.dot(g.plane.normal) < 0.0 {
                    r.reverse();
                }
                Some(r)
            })
            .collect();
        if normalized_rings.is_empty() {
            continue;
        }

        let (u_axis, v_axis) = compute_face_tangent_axes(g.plane.normal);

        // For each ring, find its TOPMOST container (the outer ring it's
        // ultimately nested inside). `None` => the ring is itself an outer.
        let n = normalized_rings.len();
        let mut topmost_container: Vec<Option<usize>> = vec![None; n];
        for i in 0..n {
            let test_3d = positions[normalized_rings[i][0] as usize];
            let test_2d = Vec2::new(test_3d.dot(u_axis), test_3d.dot(v_axis));
            let mut best: Option<usize> = None;
            let mut best_area = f32::MAX;
            for j in 0..n {
                if i == j {
                    continue;
                }
                if point_in_ring_2d(test_2d, &normalized_rings[j], &positions, u_axis, v_axis)
                {
                    let area =
                        ring_area_2d(&normalized_rings[j], &positions, u_axis, v_axis).abs();
                    if area < best_area {
                        best_area = area;
                        best = Some(j);
                    }
                }
            }
            topmost_container[i] = best;
        }

        let mut groupings: Vec<(usize, Vec<usize>)> = Vec::new();
        for i in 0..n {
            if topmost_container[i].is_none() {
                groupings.push((i, Vec::new()));
            }
        }
        for i in 0..n {
            if let Some(parent) = topmost_container[i] {
                if let Some(entry) = groupings.iter_mut().find(|(o, _)| *o == parent) {
                    entry.1.push(i);
                }
            }
        }

        let make_face = |plane: &BrushPlane| -> BrushFaceData {
            let slot = match_plane_to_slot(plane, source_face_slot_runs);
            if let Some(slot) = slot {
                slot.to_face(plane.clone())
            } else {
                let (u, v) = compute_face_tangent_axes(plane.normal);
                BrushFaceData {
                    plane: plane.clone(),
                    uv_scale: Vec2::ONE,
                    uv_u_axis: u,
                    uv_v_axis: v,
                    is_cap: true,
                    ..Default::default()
                }
            }
        };

        for (outer_idx, hole_indices) in groupings {
            let outer = &normalized_rings[outer_idx];
            if hole_indices.is_empty() {
                if outer.len() < 3 {
                    continue;
                }
                faces.push(make_face(&g.plane));
                face_rings.push(outer.iter().map(|&v| v as usize).collect());
            } else {
                let holes: Vec<Vec<u32>> = hole_indices
                    .iter()
                    .map(|&hi| normalized_rings[hi].clone())
                    .collect();
                let tris = triangulate_polygon_with_holes(
                    &positions,
                    outer,
                    &holes,
                    g.plane.normal,
                );
                // Greedy tris-to-quads merge: matches the common post-boolean
                // cleanup. Adjacent coplanar tri pairs combine into a convex
                // quad when the merge is geometrically valid; otherwise the
                // tri stays. For a rectangular frame around a rectangular
                // hole, this collapses ~8 tris into 4 trapezoidal quads.
                let polys = merge_coplanar_tris_to_quads(
                    &tris,
                    &positions,
                    u_axis,
                    v_axis,
                );
                for poly in polys {
                    if poly.len() < 3 {
                        continue;
                    }
                    faces.push(make_face(&g.plane));
                    face_rings.push(poly.iter().map(|&v| v as usize).collect());
                }
            }
        }
    }

    if faces.len() < 4 {
        return Err(CsgError::EmptyResult);
    }

    // Rebuild topology using the canonical builder. This produces the
    // same topology format the rest of the editor expects.
    let topology =
        jackdaw_geometry::build_topology_from_face_polygons(positions.clone(), face_rings);

    if topology.vertices.len() < 4 || topology.polygons.is_empty() {
        return Err(CsgError::EmptyResult);
    }

    Ok(CsgBrush { faces, topology })
}

/// Dedup positions within `eps` and return a remap from old index to new
/// index. The output positions are the survivors of the dedup (i.e., the
/// first vertex encountered at each unique position).
fn dedup_positions(positions: &[Vec3], eps: f32) -> (Vec<Vec3>, Vec<u32>) {
    let eps2 = eps * eps;
    let mut out: Vec<Vec3> = Vec::with_capacity(positions.len());
    let mut remap: Vec<u32> = Vec::with_capacity(positions.len());
    for &p in positions {
        let mut found: Option<u32> = None;
        for (i, q) in out.iter().enumerate() {
            if (*q - p).length_squared() < eps2 {
                found = Some(i as u32);
                break;
            }
        }
        if let Some(i) = found {
            remap.push(i);
        } else {
            remap.push(out.len() as u32);
            out.push(p);
        }
    }
    (out, remap)
}

/// Given a triangle fan that all lies on one plane, recover the polygon
/// boundary as a list of CCW rings matching the triangle winding.
///
/// Uses *directed* edge counting: an interior edge appears once in each
/// direction (A->B in one triangle, B->A in the neighbor), so the boundary
/// edges are those whose reverse-direction partner is absent. Following
/// those directed edges produces rings with the same orientation as the
/// triangles, which is what `build_topology_from_face_polygons` expects.
///
/// Returns multiple rings when a coplanar face group has disconnected
/// components (common after multiple cuts on the same face: each cut can
/// leave its own coplanar region disjoint from the others).
fn recover_polygon_boundaries(tris: &[(u32, u32, u32)]) -> Vec<Vec<u32>> {
    use std::collections::{HashMap, HashSet};

    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for &(a, b, c) in tris {
        for &(u, v) in &[(a, b), (b, c), (c, a)] {
            directed.insert((u, v));
        }
    }
    let boundary: Vec<(u32, u32)> = directed
        .iter()
        .filter(|(u, v)| !directed.contains(&(*v, *u)))
        .copied()
        .collect();
    if boundary.is_empty() {
        return Vec::new();
    }

    let mut next_of: HashMap<u32, u32> = HashMap::new();
    for &(u, v) in &boundary {
        next_of.entry(u).or_insert(v);
    }

    // Walk every starting vert, marking visited entries in `next_of` so we
    // collect every disjoint ring on this plane.
    let mut rings: Vec<Vec<u32>> = Vec::new();
    let mut visited: HashSet<u32> = HashSet::new();
    for &(seed_from, _) in &boundary {
        if visited.contains(&seed_from) {
            continue;
        }
        let mut ring = vec![seed_from];
        visited.insert(seed_from);
        let mut cur = seed_from;
        loop {
            let Some(&n) = next_of.get(&cur) else {
                break;
            };
            if n == seed_from {
                break;
            }
            if visited.contains(&n) {
                break;
            }
            ring.push(n);
            visited.insert(n);
            cur = n;
            if ring.len() > 10_000 {
                break;
            }
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

/// 2D signed area of a 3D ring projected onto the (u_axis, v_axis) plane.
/// Positive when the ring is wound CCW viewed from +N (with N = u x v).
fn ring_area_2d(ring: &[u32], positions: &[Vec3], u: Vec3, v: Vec3) -> f32 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    let n = ring.len();
    for i in 0..n {
        let pi = positions[ring[i] as usize];
        let pj = positions[ring[(i + 1) % n] as usize];
        let xi = pi.dot(u);
        let yi = pi.dot(v);
        let xj = pj.dot(u);
        let yj = pj.dot(v);
        area += xi * yj - xj * yi;
    }
    area * 0.5
}

/// Point-in-polygon test in 2D (ray casting / parity), with the polygon
/// supplied as a 3D ring projected via the (u, v) axes.
fn point_in_ring_2d(p: Vec2, ring: &[u32], positions: &[Vec3], u: Vec3, v: Vec3) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let pi_3d = positions[ring[i] as usize];
        let pj_3d = positions[ring[j] as usize];
        let pi = Vec2::new(pi_3d.dot(u), pi_3d.dot(v));
        let pj = Vec2::new(pj_3d.dot(u), pj_3d.dot(v));
        if ((pi.y > p.y) != (pj.y > p.y))
            && (p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Greedy coplanar tris-to-quads merge. For each interior edge shared by
/// two triangles, attempt to merge them into a convex quad. Triangles that
/// can't be merged into a convex quad stay as triangles. Matches the common "Tris to Quads" cleanup after a boolean. Returns a mix of quads and
/// triangles, each represented as a CCW vertex-index ring (indices into
/// `positions`).
fn merge_coplanar_tris_to_quads(
    tris: &[[u32; 3]],
    positions: &[Vec3],
    u_axis: Vec3,
    v_axis: Vec3,
) -> Vec<Vec<u32>> {
    let n = tris.len();
    if n == 0 {
        return Vec::new();
    }

    // Build canonical-edge -> [tri_idx] map.
    let mut edge_to_tris: std::collections::BTreeMap<(u32, u32), Vec<usize>> =
        std::collections::BTreeMap::new();
    for (ti, tri) in tris.iter().enumerate() {
        for k in 0..3 {
            let a = tri[k];
            let b = tri[(k + 1) % 3];
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            edge_to_tris.entry((lo, hi)).or_default().push(ti);
        }
    }

    let to_2d = |i: u32| -> Vec2 {
        let p = positions[i as usize];
        Vec2::new(p.dot(u_axis), p.dot(v_axis))
    };

    let mut merged: Vec<bool> = vec![false; n];
    let mut output: Vec<Vec<u32>> = Vec::new();

    for (&(e_lo, e_hi), tri_idxs) in &edge_to_tris {
        if tri_idxs.len() != 2 {
            continue;
        }
        let t1 = tri_idxs[0];
        let t2 = tri_idxs[1];
        if merged[t1] || merged[t2] {
            continue;
        }
        if let Some(quad) = try_merge_tris_to_quad(tris[t1], tris[t2], e_lo, e_hi, &to_2d) {
            merged[t1] = true;
            merged[t2] = true;
            output.push(quad);
        }
    }

    for (ti, tri) in tris.iter().enumerate() {
        if !merged[ti] {
            output.push(tri.to_vec());
        }
    }

    output
}

/// Try to merge two CCW triangles sharing edge `(e_lo, e_hi)` into a single
/// convex quad. Returns `Some(ring)` if the merge produces a convex polygon
/// in the supplied 2D projection; `None` otherwise (e.g., the resulting
/// quad would be concave or self-intersecting).
fn try_merge_tris_to_quad(
    tri1: [u32; 3],
    tri2: [u32; 3],
    e_lo: u32,
    e_hi: u32,
    to_2d: &dyn Fn(u32) -> Vec2,
) -> Option<Vec<u32>> {
    let c = tri1.iter().copied().find(|&v| v != e_lo && v != e_hi)?;
    let d = tri2.iter().copied().find(|&v| v != e_lo && v != e_hi)?;

    // Determine the direction of the shared edge in tri1 (lo->hi or hi->lo).
    // Tri1 is CCW, so its third vertex `c` lies to the left of the edge as
    // traversed in tri1's order. We use that direction to build the merged
    // quad in CCW order: [c, edge_start, d, edge_end].
    let mut edge_start = e_lo;
    let mut edge_end = e_hi;
    for k in 0..3 {
        let a = tri1[k];
        let b = tri1[(k + 1) % 3];
        if a == e_lo && b == e_hi {
            edge_start = e_lo;
            edge_end = e_hi;
            break;
        }
        if a == e_hi && b == e_lo {
            edge_start = e_hi;
            edge_end = e_lo;
            break;
        }
    }

    let quad = vec![c, edge_start, d, edge_end];

    // Convex check: all four consecutive cross products must share a sign.
    let p: Vec<Vec2> = quad.iter().map(|&v| to_2d(v)).collect();
    let mut signs: [f32; 4] = [0.0; 4];
    for i in 0..4 {
        let a = p[i];
        let b = p[(i + 1) % 4];
        let c2 = p[(i + 2) % 4];
        let ab = b - a;
        let bc = c2 - b;
        signs[i] = ab.x * bc.y - ab.y * bc.x;
    }
    const TOL: f32 = 1e-6;
    let all_pos = signs.iter().all(|&s| s > TOL);
    let all_neg = signs.iter().all(|&s| s < -TOL);
    if !all_pos && !all_neg {
        return None;
    }
    Some(quad)
}

/// Full pipeline: take two brush inputs, run a boolean op, return the
/// result as a single `CsgBrush`.
pub fn brush_boolean(
    a: &CsgInput<'_>,
    b: &CsgInput<'_>,
    op: BooleanOp,
) -> Result<CsgBrush, CsgError> {
    let a_conv = brush_to_manifold(a)?;
    let b_conv = brush_to_manifold(b)?;

    let result = match op {
        BooleanOp::Union => a_conv.manifold.union(&b_conv.manifold),
        BooleanOp::Difference => a_conv.manifold.difference(&b_conv.manifold),
        BooleanOp::Intersection => a_conv.manifold.intersection(&b_conv.manifold),
    };

    let runs: Vec<&[FaceSlot]> = vec![&a_conv.face_slots, &b_conv.face_slots];
    manifold_to_brush(&result, &runs)
}

/// Difference variant that returns the (potentially multiple) connected
/// components of the result. Matches the existing
/// `subtract_brush` shape, where a target cut by a cutter that splits
/// it in two yields multiple fragments.
pub fn brush_difference_split(
    target: &CsgInput<'_>,
    cutter: &CsgInput<'_>,
) -> Result<Vec<CsgBrush>, CsgError> {
    let t_conv = brush_to_manifold(target)?;
    let c_conv = brush_to_manifold(cutter)?;

    let result = t_conv.manifold.difference(&c_conv.manifold);
    if result.is_empty() {
        return Err(CsgError::EmptyResult);
    }
    let runs: Vec<&[FaceSlot]> = vec![&t_conv.face_slots, &c_conv.face_slots];
    let components = result.decompose();
    if components.is_empty() {
        // No disconnected components. Treat the whole thing as one.
        return Ok(vec![manifold_to_brush(&result, &runs)?]);
    }
    let mut out: Vec<CsgBrush> = Vec::with_capacity(components.len());
    for comp in &components {
        match manifold_to_brush(comp, &runs) {
            Ok(brush) => out.push(brush),
            Err(CsgError::EmptyResult) => continue,
            Err(e) => return Err(e),
        }
    }
    if out.is_empty() {
        return Err(CsgError::EmptyResult);
    }
    Ok(out)
}

/// Union of an arbitrary number of brushes (used by Join's mesh-CSG
/// path; the parry convex_hull path remains for true convex inputs).
pub fn brush_batch_union(inputs: &[CsgInput<'_>]) -> Result<CsgBrush, CsgError> {
    if inputs.is_empty() {
        return Err(CsgError::DegenerateBrush);
    }
    if inputs.len() == 1 {
        // Trivial: just round-trip through manifold to canonicalise.
        let conv = brush_to_manifold(&inputs[0])?;
        let runs: Vec<&[FaceSlot]> = vec![&conv.face_slots];
        return manifold_to_brush(&conv.manifold, &runs);
    }
    let mut converted: Vec<ConvertedInput> = Vec::with_capacity(inputs.len());
    for input in inputs {
        converted.push(brush_to_manifold(input)?);
    }
    let manifolds: Vec<Manifold> = converted.iter().map(|c| c.manifold.clone()).collect();
    let result = Manifold::batch_union(&manifolds);
    let runs: Vec<&[FaceSlot]> = converted.iter().map(|c| c.face_slots.as_slice()).collect();
    manifold_to_brush(&result, &runs)
}

/// Translate a brush's topology positions and face planes by a world
/// transform (rotation + translation). UV axes are also rotated.
///
/// This is the topology-aware companion of
/// `jackdaw_geometry::brush_planes_to_world`. Mesh-CSG needs vertices in
/// world space, not just planes.
pub fn brush_to_world(
    faces: &[BrushFaceData],
    topology: &BrushTopology,
    rotation: Quat,
    translation: Vec3,
) -> (Vec<BrushFaceData>, BrushTopology) {
    let world_faces: Vec<BrushFaceData> = faces
        .iter()
        .map(|f| {
            let world_normal = (rotation * f.plane.normal).normalize();
            let world_distance = f.plane.distance + world_normal.dot(translation);
            BrushFaceData {
                plane: BrushPlane {
                    normal: world_normal,
                    distance: world_distance,
                },
                material: f.material.clone(),
                uv_offset: f.uv_offset,
                uv_scale: f.uv_scale,
                uv_rotation: f.uv_rotation,
                uv_u_axis: (rotation * f.uv_u_axis).normalize_or_zero(),
                uv_v_axis: (rotation * f.uv_v_axis).normalize_or_zero(),
                is_cap: f.is_cap,
            }
        })
        .collect();
    let mut world_topo = topology.clone();
    for v in &mut world_topo.vertices {
        v.position = rotation * v.position + translation;
    }
    (world_faces, world_topo)
}

/// Recentre a brush so its topology is around the origin. Returns the
/// centroid that was subtracted.
pub fn brush_recentre(brush: &mut CsgBrush) -> Vec3 {
    if brush.topology.vertices.is_empty() {
        return Vec3::ZERO;
    }
    let centroid: Vec3 = brush
        .topology
        .vertices
        .iter()
        .map(|v| v.position)
        .sum::<Vec3>()
        / brush.topology.vertices.len() as f32;
    for v in &mut brush.topology.vertices {
        v.position -= centroid;
    }
    for face in &mut brush.faces {
        // d' = d - n . centroid
        face.plane.distance -= face.plane.normal.dot(centroid);
    }
    centroid
}

#[allow(dead_code)]
const _: f32 = EPSILON;

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_geometry::{
        BrushFaceData, BrushPlane, MeshEdge, MeshLoop, MeshPoly, MeshVert,
        compute_face_tangent_axes,
    };

    fn cube_brush(half: f32) -> (Vec<BrushFaceData>, BrushTopology) {
        let h = half;
        let normals = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ];
        let distances = [h, h, h, h, h, h];
        let faces: Vec<BrushFaceData> = normals
            .iter()
            .zip(distances.iter())
            .map(|(&normal, &distance)| {
                let (u, v) = compute_face_tangent_axes(normal);
                BrushFaceData {
                    plane: BrushPlane { normal, distance },
                    uv_scale: Vec2::ONE,
                    uv_u_axis: u,
                    uv_v_axis: v,
                    ..Default::default()
                }
            })
            .collect();
        let vertices = vec![
            MeshVert {
                position: Vec3::new(-h, -h, -h),
            },
            MeshVert {
                position: Vec3::new(h, -h, -h),
            },
            MeshVert {
                position: Vec3::new(h, h, -h),
            },
            MeshVert {
                position: Vec3::new(-h, h, -h),
            },
            MeshVert {
                position: Vec3::new(-h, -h, h),
            },
            MeshVert {
                position: Vec3::new(h, -h, h),
            },
            MeshVert {
                position: Vec3::new(h, h, h),
            },
            MeshVert {
                position: Vec3::new(-h, h, h),
            },
        ];
        let edges = vec![
            MeshEdge {
                v: [0, 1],
                ..Default::default()
            },
            MeshEdge {
                v: [1, 2],
                ..Default::default()
            },
            MeshEdge {
                v: [2, 3],
                ..Default::default()
            },
            MeshEdge {
                v: [0, 3],
                ..Default::default()
            },
            MeshEdge {
                v: [4, 5],
                ..Default::default()
            },
            MeshEdge {
                v: [5, 6],
                ..Default::default()
            },
            MeshEdge {
                v: [6, 7],
                ..Default::default()
            },
            MeshEdge {
                v: [4, 7],
                ..Default::default()
            },
            MeshEdge {
                v: [0, 4],
                ..Default::default()
            },
            MeshEdge {
                v: [1, 5],
                ..Default::default()
            },
            MeshEdge {
                v: [2, 6],
                ..Default::default()
            },
            MeshEdge {
                v: [3, 7],
                ..Default::default()
            },
        ];
        // Face order matches +X, -X, +Y, -Y, +Z, -Z.
        // Each ring is CCW when viewed from outside.
        let rings: [&[u32]; 6] = [
            &[1, 2, 6, 5], // +X
            &[0, 4, 7, 3], // -X
            &[3, 7, 6, 2], // +Y
            &[0, 1, 5, 4], // -Y
            &[4, 5, 6, 7], // +Z
            &[0, 3, 2, 1], // -Z
        ];
        // Edge lookup: same indices as edges vec.
        let edge_idx = |a: u32, b: u32| -> u32 {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            edges
                .iter()
                .position(|e| e.v == [lo, hi])
                .map(|i| i as u32)
                .unwrap_or(0)
        };
        let mut loops: Vec<MeshLoop> = Vec::new();
        let mut polygons: Vec<MeshPoly> = Vec::new();
        for ring in &rings {
            let start = loops.len() as u32;
            for i in 0..ring.len() {
                let v = ring[i];
                let vn = ring[(i + 1) % ring.len()];
                loops.push(MeshLoop {
                    vert: v,
                    edge: edge_idx(v, vn),
                });
            }
            polygons.push(MeshPoly {
                loop_start: start,
                loop_total: ring.len() as u32,
            });
        }
        let topology = BrushTopology {
            vertices,
            edges,
            polygons,
            loops,
            attributes: Default::default(),
        };
        (faces, topology)
    }

    fn translated_cube_brush(half: f32, offset: Vec3) -> (Vec<BrushFaceData>, BrushTopology) {
        let (faces, mut topo) = cube_brush(half);
        for v in &mut topo.vertices {
            v.position += offset;
        }
        let mut faces = faces;
        for f in &mut faces {
            f.plane.distance += f.plane.normal.dot(offset);
        }
        (faces, topo)
    }

    #[test]
    fn cube_minus_cube_produces_expected_topology() {
        let (a_faces, a_topo) = cube_brush(1.0);
        let (b_faces, b_topo) = translated_cube_brush(0.5, Vec3::new(0.75, 0.0, 0.0));
        let a = CsgInput::new(&a_faces, &a_topo);
        let b = CsgInput::new(&b_faces, &b_topo);
        let result = brush_boolean(&a, &b, BooleanOp::Difference).expect("subtract should succeed");
        // The cutter pokes a notch through the +X face. That face becomes
        // an annulus (rectangle-with-rectangular-hole), which our pipeline splits into simply-connected triangles, so the
        // total face count is higher than a pure n-gon scheme. The exact
        // count varies with manifold3d's triangulation; just bound it.
        assert!(
            result.faces.len() >= 4 && result.faces.len() <= 32,
            "expected reasonable face count, got {}",
            result.faces.len()
        );
        assert!(
            result.topology.vertices.len() >= 6,
            "expected >= 6 verts, got {}",
            result.topology.vertices.len()
        );
        for poly in &result.topology.polygons {
            assert!(poly.loop_total >= 3);
        }
    }

    #[test]
    fn boolean_returns_empty_on_disjoint_inputs() {
        let (a_faces, a_topo) = cube_brush(1.0);
        let (b_faces, b_topo) = translated_cube_brush(1.0, Vec3::new(100.0, 0.0, 0.0));
        let a = CsgInput::new(&a_faces, &a_topo);
        let b = CsgInput::new(&b_faces, &b_topo);

        // Intersection of disjoint cubes is empty.
        let intersect = brush_boolean(&a, &b, BooleanOp::Intersection);
        assert!(
            matches!(intersect, Err(CsgError::EmptyResult)),
            "intersection of disjoint cubes should be empty, got {intersect:?}"
        );

        // Subtraction of an unrelated faraway cube should leave the
        // target unchanged: brush_difference_split returns the original.
        let sub = brush_difference_split(&a, &b).expect("disjoint difference should succeed");
        assert_eq!(sub.len(), 1, "expected single fragment, got {}", sub.len());
        assert!(
            sub[0].topology.vertices.len() == 8,
            "expected 8 verts (unchanged cube), got {}",
            sub[0].topology.vertices.len()
        );
    }

    #[test]
    fn material_idx_propagates_through_boolean() {
        // Build a cube where the +X face has a distinguishable sentinel
        // value on its uv_scale, then subtract a cube that overlaps the
        // origin (so the +X face is left mostly intact).
        let (mut a_faces, a_topo) = cube_brush(1.0);
        a_faces[0].uv_scale = Vec2::new(7.0, 13.0); // sentinel value
        let (b_faces, b_topo) = translated_cube_brush(0.4, Vec3::new(0.0, 0.0, 0.0));
        let a = CsgInput::new(&a_faces, &a_topo);
        let b = CsgInput::new(&b_faces, &b_topo);
        let result = brush_boolean(&a, &b, BooleanOp::Difference).expect("subtract should succeed");
        // The +X face's sentinel uv_scale should have survived on the
        // output face that lies on x = 1 (the original +X plane).
        let plus_x = result.faces.iter().find(|f| {
            (f.plane.normal - Vec3::X).length() < 1e-3 && (f.plane.distance - 1.0).abs() < 1e-3
        });
        assert!(plus_x.is_some(), "expected +X face to survive");
        let plus_x = plus_x.unwrap();
        assert!(
            (plus_x.uv_scale - Vec2::new(7.0, 13.0)).length() < 1e-3,
            "sentinel uv_scale should propagate; got {:?}",
            plus_x.uv_scale
        );
    }

    #[test]
    fn concave_brush_subtract_works() {
        // Build a "concave" L-shape by subtracting a corner from a cube
        // first, then subtract a small cube from the L.
        let (base_faces, base_topo) = cube_brush(1.0);
        let (corner_faces, corner_topo) = translated_cube_brush(0.5, Vec3::new(0.75, 0.75, 0.75));
        let l_shape = brush_boolean(
            &CsgInput::new(&base_faces, &base_topo),
            &CsgInput::new(&corner_faces, &corner_topo),
            BooleanOp::Difference,
        )
        .expect("first subtract should succeed");
        // L-shape is now concave. Subtract another small box from it.
        let (cut_faces, cut_topo) = translated_cube_brush(0.2, Vec3::new(-0.7, -0.7, -0.7));
        let result = brush_boolean(
            &CsgInput::new(&l_shape.faces, &l_shape.topology),
            &CsgInput::new(&cut_faces, &cut_topo),
            BooleanOp::Difference,
        )
        .expect("subtract from concave brush should succeed");
        assert!(
            result.topology.vertices.len() >= 8,
            "expected >= 8 verts, got {}",
            result.topology.vertices.len()
        );
        assert!(
            !result.topology.polygons.is_empty(),
            "expected polygons in result"
        );
    }

    #[test]
    fn concave_output_round_trips_through_kernel() {
        // The L-shape produced by `cube minus corner cube` is concave.
        // Verify it survives a second conversion through the kernel.
        let (base_faces, base_topo) = cube_brush(1.0);
        let (corner_faces, corner_topo) = translated_cube_brush(0.5, Vec3::new(0.75, 0.75, 0.75));
        let l_shape = brush_boolean(
            &CsgInput::new(&base_faces, &base_topo),
            &CsgInput::new(&corner_faces, &corner_topo),
            BooleanOp::Difference,
        )
        .expect("first subtract should succeed");

        let input = CsgInput::new(&l_shape.faces, &l_shape.topology);
        let conv = brush_to_manifold(&input);
        if let Err(e) = conv {
            panic!("L-shape should round-trip into manifold: {e:?}");
        }
    }
}
