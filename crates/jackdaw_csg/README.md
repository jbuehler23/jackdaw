# jackdaw_csg

Mesh-CSG glue between jackdaw brushes and the
[`manifold3d`](https://github.com/elalish/manifold) C++ kernel via the
[`manifold-csg`](https://crates.io/crates/manifold-csg) Rust bindings.

Replaces the convex-only half-space CSG path used by Join, CSG
Subtract, and CSG Intersect with mesh-CSG that works on arbitrary
(concave) topologies.

## Status

Currently **excluded from the workspace** because building
`manifold-csg-sys` requires `cmake`, and `cmake` is not installed in the
default jackdaw dev environment.

To enable:

1. Install the system prerequisites listed below.
2. Re-add `"crates/jackdaw_csg"` to the workspace `members` list (and
   remove the `exclude` entry for it) in the root `Cargo.toml`.
3. Uncomment the `jackdaw_csg` line in `[workspace.dependencies]`.
4. Run `cargo build -p jackdaw_csg` to verify the C++ kernel compiles.

## Build prerequisites

| Tool | Purpose | Arch | Debian/Ubuntu |
|------|---------|------|---------------|
| cmake | configures manifold3d | `pacman -S cmake` | `apt install cmake` |
| C++ compiler | builds manifold3d | `pacman -S gcc` | `apt install g++` |
| git | clones manifold3d source | usually present | usually present |
| python3 | optional, used by some build helpers | usually present | usually present |

The first build clones manifold3d into the `target/` cache and compiles
it; expect ~60 seconds on a workstation. Subsequent builds reuse the
cache.

## API

- `BooleanOp` -- enum of `Union`, `Difference`, `Intersection`.
- `CsgInput` -- borrowed view of `(faces, topology)` pair.
- `CsgBrush` -- owned result of a boolean op.
- `brush_boolean(a, b, op)` -- single-result pipeline.
- `brush_difference_split(target, cutter)` -- `Difference` that returns
  each connected component as a separate `CsgBrush`, matching the
  existing `subtract_brush` shape.
- `brush_batch_union(inputs)` -- N-way union.
- `brush_to_world(faces, topology, rotation, translation)` -- transform
  helper for going into world space before a boolean op.
- `brush_recentre(brush)` -- subtract the centroid so the result sits at
  the origin in local space.

## Conversion approach

1. **Brush -> Manifold:** triangulate each polygon face with
   `triangulate_face_polygon` (handles concave rings via earcut), pack
   into `vert_properties` with a 4th property channel for source face
   index, build via `Manifold::from_mesh_f64`.
2. **Boolean:** `Manifold::union` / `difference` / `intersection`.
3. **Manifold -> Brush:** read back `to_mesh_f64`, group triangles by
   coplanar plane (manifold3d may retriangulate cut faces), reproject
   UV axes from the source face slot indexed by the recovered material
   channel, rebuild `BrushTopology` with canonical edge / loop / poly
   arrays.

## Tests

`cargo test -p jackdaw_csg`:

- `cube_minus_cube_produces_expected_topology` -- a smaller cube
  subtracted from a larger one produces a valid manifold solid.
- `concave_brush_subtract_works` -- subtract a corner from a cube to
  make it concave, then subtract a small cube from the concave shape;
  expects no panic and a non-empty result.
- `material_idx_propagates_through_boolean` -- a sentinel `uv_scale`
  set on the input's +X face survives on the corresponding output face.
- `boolean_returns_empty_on_disjoint_inputs` -- intersect/subtract with
  a faraway cube returns `EmptyResult` / unchanged target.
