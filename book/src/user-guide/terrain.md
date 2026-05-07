# Terrain

Jackdaw's terrain is a heightmap-backed mesh, chunked for
streaming and edited with brush-style sculpt tools. The crate
that does the work is `jackdaw_terrain`; if you want the
actual data structures, the entry points are
`Heightmap`, `apply_brush`, and `build_chunk_mesh_data`.

## Add a terrain

`Add > Terrain` in the hierarchy. You get a flat heightmap
component on a new entity, with a chunked mesh underneath.
Resolution and physical size are properties on the
`Terrain` component, editable in the inspector.

## Sculpt

Select the terrain, then pick a sculpt tool from the toolbar
or the terrain panel. Available tools:

- **Raise / lower**. Add or subtract height under the cursor.
- **Flatten**. Drag heights toward the height under the
  click point.
- **Smooth**. Average heights inside the brush radius.
- **Noise**. Add procedural noise inside the brush radius;
  good for breaking up flat areas without sculpting by hand.

Brush radius and strength sit in the toolbar. The brush
preview ring tracks the cursor so you can aim before
committing.

`Ctrl+Z` undoes the last stroke. Each contiguous drag is one
undo entry, not one entry per heightmap sample.

## Erosion

The erosion pass simulates hydraulic erosion across the whole
heightmap. Adjust iteration count, evaporation rate, and
sediment capacity in the panel; click `Run`. It is a
one-shot operation, not a real-time tool.

This is the slowest thing in the terrain workflow, since it
runs on the CPU and rebuilds every chunk mesh when it
finishes. Save before you click. We do not have a cancel
button yet.

## Texture painting

Texture painting on terrain is on the roadmap but not built.
Today the terrain takes a single material applied to the
whole surface. If you need varied surface textures, blend in
your fragment shader or split the heightmap into multiple
terrains, each with its own material.

## Chunking

Chunks are 32 cells per edge (`src/terrain/mod.rs::CHUNK_SIZE`).
Edits only rebuild the chunks that overlap the brush,
which is what keeps sculpting fast on large heightmaps.
There is no LOD or frustum streaming yet; every chunk
renders at full resolution.

## Common gotchas

- **Mesh shows seams between chunks.** Normals are computed
  per chunk. The boundary samples should match across
  chunks; if they don't, an edit straddled the boundary and
  one side never rebuilt. Touch both sides with the smooth
  tool to force the rebuild.
- **Erosion result looks wrong.** Iteration count is the
  knob to tune first. Defaults aim for a generic mountain;
  rolling hills want fewer iterations and a higher
  evaporation rate.
- **Standalone game shows no terrain.** `jackdaw_runtime`
  doesn't pull in `jackdaw_terrain`. If your game needs
  terrain at runtime, add `jackdaw_terrain` to your
  standalone `Cargo.toml` and bring whatever plugin /
  systems you want into your game's plugin alongside
  `JackdawPlugin`.
