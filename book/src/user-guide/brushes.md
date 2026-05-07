# Brushes

A brush is jackdaw's primitive for level geometry. Think
TrenchBroom or old-school Hammer: convex polyhedra defined by
their faces, with per-face materials and UVs, edited in place
without a DCC tool. They serialize directly into the scene
`.jsn`, no external mesh files.

## The two ways to make a brush

### Quick add

Hierarchy panel, right-click, `Add > Cube` or `Add > Sphere`.
You get a unit primitive at the origin, selected and ready
to move. This is the fastest path when you just need a
block.

### Draw

Press `B` to enter the draw-brush modal. Click in the
viewport to drop vertices, then press `Enter` to close the
polygon and extrude it to a brush. While drawing:

- `Click` places a vertex.
- `Backspace` removes the last vertex.
- `Enter` closes the polygon.
- `Esc` or right-click cancels.
- `Tab` toggles between additive and subtractive draw mode.
  In subtractive mode (`C` to enter directly), the closed
  polygon CSGs out of the brush you draw against.

The plane you draw on is the closest face under your cursor,
or the world floor if nothing is under it.

## Editing a brush

Select a brush, then pick the edit mode:

- `1` vertex mode
- `2` edge mode
- `3` face mode
- `4` clip mode

Click an element to select it, drag the gizmo to move it.
Multi-select with `Shift+Click`. `Delete` removes the
selected element (vertices collapse the surrounding face,
faces leave a hole jackdaw won't render).

`Esc` exits edit mode and returns to entity-level selection.

### Snap and constrain

- `Ctrl` while dragging toggles snap to grid; the snap step
  follows your current grid size.
- `X` / `Y` / `Z` constrain the drag to that axis.
- `MMB` toggles the global snap mode without holding `Ctrl`.

### Clip

Clip mode (`4`) draws a plane through the brush. Drag the
plane gizmo where you want the cut, press `Enter` to apply.
The brush splits in two; the clipped-off side becomes a new
brush you can immediately delete or move.

## Boolean operations

Select two or more brushes and run one of:

- **CSG Subtract** (`Ctrl+K`): cut the second selection out
  of the first.
- **CSG Intersect** (`Ctrl+Shift+K`): keep only the volume
  both brushes share.
- **Join (Convex Merge)** (`J`): merge two brushes back into
  one convex brush, when their union is itself convex.

All three live under the **Edit** menu. They run through the
CSG code in `crates/jackdaw_geometry`. The result replaces
the inputs with new brushes; the original selection ordering
picks which is the minuend in subtract.

## Faces, materials, and UVs

Selecting a face in face mode (`3`) shows its material and
UV controls in the inspector. You can:

- Set a texture or material from the material browser.
- Tweak UV offset, scale, and rotation per face.

Face data lives on the brush entity as `BrushFaceData`. See
[Materials & Textures](materials-textures.md) for what the
material picker exposes.

## Common gotchas

- **Brush disappears after a CSG op.** The op produced a
  degenerate result (zero-volume intersection, fully
  consumed subtractor). Undo and try a different overlap.
- **Faces look inside-out.** Brushes assume outward normals.
  If you authored vertices in clockwise order while drawing,
  flip the brush via the inspector or redraw.
- **Snap is "wrong"**. Snap follows the grid size shown in
  the status bar, not a fixed unit. Step the grid with `[`
  and `]`.
