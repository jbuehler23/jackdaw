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

## Paint channels

Choose the paintbrush in the terrain toolbar to edit integer
channels such as biome, ground type, or buildability. Add a
channel and one or more palette values in the **Paint
Channels** section, select the value to write, and drag over
the terrain. Hold `Ctrl` while painting to restore value `0`.

**Show Painted Values** tints the terrain with the active
palette so the stored data is visible even before a game
material consumes it. New channel and palette entries receive
generated names and colours; project-specific descriptor
names, integer widths, labels, values, and colours can also be
authored directly in the scene document.

## Quantization

Enable quantization when the game needs a fixed metric grid or
terraced elevations. **Cell Size** controls the world-space
distance between samples and **Height Step** controls the
elevation interval. Sculpt, generation, and erosion snap new
changes while quantization is enabled. Click **Apply** once to
snap heights that existed before it was enabled.

Turning quantization off stops future snapping but does not
alter heights that are already stored.

## Scatter

The **Scatter** section places model instances across the
selected terrain. Add one or more model assets, then configure
density, spacing, scale, yaw, normal alignment, and an optional
paint-channel mask. A seed produces the same placement for the
same terrain and channel data.

Re-running the same scatter group replaces untouched generated
instances. Instances moved, rotated, or scaled by hand are
preserved. The whole run is a single undoable edit.

## Sidecars and export

Scene files keep the small terrain descriptor, while heights
and per-cell channel values are stored beside the scene in
versioned `.jdterrain` files. Save and move those sidecars with
the `.bsn` scene that references them.

For a headless runtime or another engine, export the authored
terrain with:

```text
jd export-terrain path/to/scene.bsn --out path/to/export
```

The export contains height and channel images, a manifest, and
placed-scene data. Quantized projects normally keep cell size
and elevation step on the terrain; unquantized scenes can pass
`--cell-size` and `--elevation-step` together for an export-only
grid. Add `--raw-heights` when the consumer also needs the raw
height buffer.

### Export format contract

This is a cross-repo contract: an importer in another repo is
built against it, so treat the shapes below as stable.

- `manifest.json`, `format_version: 2`. Bumped whenever a field
  is added, removed, or reinterpreted; an importer should check
  it and refuse (or degrade explicitly) on a version it does
  not understand, rather than assume the shape it expects.
- `heightmap.png` is a 16-bit grayscale PNG. Every pixel decodes
  to a world-space height via
  `height = manifest.heightmap.base_m + pixel * manifest.heightmap.step_m`
  (`encoding: "unsigned-steps-from-base"`). Quantized exports
  set `step_m` to the terrain's elevation step; unquantized
  exports derive `step_m` from the actual authored height span
  (not from `max_height_m`, which is a configured ceiling and
  can differ from the real data range).
- Each paint channel is its own PNG (`channels/<name>.png`, 8-
  or 16-bit depending on the channel's element width) plus a
  manifest entry: `name`, `file`, `bit_depth`, `element`
  (`"u8"` / `"u16"`), and `palette` -- a list of
  `{ value, label, color }` entries, `color` as `#rrggbb`.
  Channel names are guaranteed unique in one export: the writer
  refuses the whole export if the scene's channel names collide,
  either exactly or after filename sanitization.
- `placements.json`, its own `format_version: 1`, lists every
  scattered / placed instance: `name`, `asset` (nullable),
  `translation_m` / `rotation_quat` / `scale`, and `components`
  (a free-form JSON map of any extra authored component data on
  that instance).
- `heights.f32`, present only with `--raw-heights`: the raw
  height buffer as little-endian `f32`, row-major, unquantized
  and unscaled -- for a consumer that wants the source values
  rather than the quantized PNG encoding.

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
- **Standalone game shows no terrain.** Two separate causes
  land on the same symptom. First, `jackdaw_runtime` doesn't
  pull in `jackdaw_terrain`: if your game needs terrain at
  runtime, add `jackdaw_terrain` to your standalone
  `Cargo.toml` and bring whatever plugin / systems you want
  into your game's plugin alongside `JackdawPlugin`. Second,
  even with the crate present, a terrain's heights and paint
  channels live in a `.jdterrain` sidecar next to the `.bsn`
  scene, not in the scene file itself (see "Sidecars and
  export" above) -- if you load a `.bsn` scene directly at
  runtime rather than through the `jd export-terrain` pipeline,
  every referenced `.jdterrain` sidecar has to ship and load
  alongside it, or the terrain reads as flat.
