# Terrain

Jackdaw's terrain is a heightmap-backed mesh, rendered as
clipmap LOD levels around the camera and edited with
brush-style sculpt tools. The crate that does the work is
`jackdaw_terrain`; if you want the actual data structures, the
entry points are `Heightmap`, `apply_brush`, and
`build_clipmap_mesh_data`.

## Add a terrain

`Add > Terrain` in the hierarchy. You get a flat heightmap
component on a new entity, rendered as clipmap LOD levels
around the camera. Resolution and physical size are properties
on the `Terrain` component, editable in the inspector.

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
runs on the CPU and rebuilds every LOD level when it
finishes. Save before you click. There is no cancel button.

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

Scene files keep the small terrain descriptor, while heights,
painted texture ids and per-cell channel values are stored beside
the scene in versioned `.jdterrain` files, along with the terrain's
texture-set reference. Save and move those sidecars with the `.bsn`
scene that references them.

A sidecar in an older format migrates when the scene opens and is
rewritten in the current format on the next save. A sidecar this
build cannot read, such as one written by a newer jackdaw or one
whose resolution is not a power of two, refuses edits and is never
overwritten, so nothing is lost while you fix or replace it.

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

## Rendering

Terrain draws as a handful of concentric LOD levels centred on
the camera: the level under the camera samples every grid
point, and each ring out doubles the step and covers four
times the ground, so the vertex budget stays flat as the
terrain grows. Levels snap their outer edge to the coarser
level next to them, so boundaries between levels stay
crack-free however far the camera moves. A level only draws
where the terrain has data; ground no region owns costs
nothing to render. Edits rebuild only the levels whose ground
changed, which keeps sculpting fast on large heightmaps.

Each material slot has its own **Tiling** and **Detiling**
controls, in the terrain panel's **Slot** section. Tiling sets
how many times the texture repeats per world unit; Detiling
breaks up that repetition by turning and shifting individual
tiles -- 0 is off.

## Autoterrain

Autoterrain textures the cells you have not painted from the
slope under them: flat ground draws one of the terrain's
textures, steep ground another, and the band between them
blends by height the same way two painted textures do. Turn it
on in the terrain panel's **Textures** tab, under
**Autoterrain**, where you also pick which texture flat and
steep ground draw and set the slope band in degrees. It is off
per terrain until you turn it on.

Painting a cell claims it: from then on it draws what you
painted, wherever the slope goes. With the paint options bar's
**Restore Auto** checkbox on, the brush hands the cells under it
back to autoterrain without disturbing the paint underneath, so
a later stroke over them brings back what they had.

Autoterrain is evaluated as the terrain draws, so sculpting
re-textures the ground as you go: raise a bank past the slope
band and it takes the steep texture as soon as it is steep. The
settings live in the terrain's sidecar, so a built game shades
the ground the way the editor showed it.

## Common gotchas

- **Erosion result looks wrong.** Iteration count is the
  knob to tune first. Defaults aim for a generic mountain;
  rolling hills want fewer iterations and a higher
  evaporation rate.
- **Standalone game shows no terrain.** `jackdaw_runtime`
  draws terrain behind its `terrain` feature, which is off by
  default so a game without terrain links neither the mesher
  nor the shader. Turn it on in your game's `Cargo.toml`:
  `jackdaw_runtime = { version = "0.19", features = ["terrain"] }`.
  With it on, `JackdawPlugin` reads each `Terrain` entity's
  `.jdterrain` sidecar from beside the `.bsn` scene that spawned
  it and draws the result with the same splat material and
  clipmap mesher the editor uses. The sidecars have to ship
  alongside the scene (see "Sidecars and export" above): a
  missing one reads as flat ground, and one that will not decode
  draws nothing at all.

  Your game also has to have an active `Camera3d`, because the
  LOD levels are laid out around wherever the terrain is being
  looked at from. An authored `.bsn` scene carries none, since
  the editor never saves its viewport camera into one, so spawn
  the camera yourself. With none in the world the terrain does
  not draw, and `jackdaw_runtime` says so in the log once.

  By default the viewer is the active `Camera3d` with the
  highest `order`, which in a single-camera game is the only
  one there is. If your game draws a UI overlay through a
  second camera at a higher `order`, put the `TerrainViewer`
  marker component on your world camera; a marked camera is
  preferred over any unmarked one, so the overlay cannot pull
  the finest LOD ring away from the player.
