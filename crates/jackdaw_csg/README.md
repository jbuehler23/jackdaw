# jackdaw_csg

Mesh-CSG glue between jackdaw brushes and
[`manifold-rust`](https://crates.io/crates/manifold-rust), a pure-Rust port
of the [`manifold3d`](https://github.com/elalish/manifold) boolean kernel.

Replaces the convex-only half-space CSG path used by Join, CSG
Subtract, and CSG Intersect with mesh-CSG that works on arbitrary
(concave) topologies. The kernel is plain Rust, so the crate needs no
cmake, C++ compiler or source checkout to build.

## API

- `BooleanOp` -- enum of `Union`, `Difference`, `Intersection`.
- `CsgInput` -- borrowed view of `(faces, topology)` pair.
- `CsgBrush` -- owned result of a boolean op.
- `brush_boolean(a, b, op)` -- single-result pipeline.
- `brush_difference_split(target, cutter)` -- `Difference` that returns
  each connected component as a separate `CsgBrush`, matching the
  existing `subtract_brush` shape.
- `brush_batch_union(inputs)` -- N-way union.
- `brush_to_world(faces, topology, transform)` -- affine transform
  helper (scale, rotation, translation) for going into world space
  before a boolean op.
- `brush_recentre(brush)` -- subtract the centroid so the result sits at
  the origin in local space.

## Conversion approach

1. **Brush -> solid:** triangulate each polygon face with
   `triangulate_face_polygon` (handles concave rings via earcut) and
   build a kernel solid from the flat positions and triangle indices.
2. **Boolean:** union, difference or intersection in the kernel.
3. **Solid -> brush:** read back the result mesh, group triangles by
   coplanar plane (the kernel may retriangulate cut faces), match each
   plane to its source face to reproject UV axes and material, rebuild
   `BrushTopology` with canonical edge / loop / poly arrays.

## Tests

`cargo test -p jackdaw_csg`:

- `cube_minus_cube_produces_expected_topology` -- a smaller cube
  subtracted from a larger one produces a valid closed solid.
- `concave_brush_subtract_works` -- subtract a corner from a cube to
  make it concave, then subtract a small cube from the concave shape;
  expects no panic and a non-empty result.
- `material_idx_propagates_through_boolean` -- a sentinel `uv_scale`
  set on the input's +X face survives on the corresponding output face.
- `boolean_returns_empty_on_disjoint_inputs` -- intersect/subtract with
  a faraway cube returns `EmptyResult` / unchanged target.
