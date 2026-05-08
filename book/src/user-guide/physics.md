# Physics and prop placement

Jackdaw uses [avian3d](https://github.com/Jondolf/avian) for
physics. There's no global "enable physics" toggle: you opt an
entity in by adding the components it needs, and the editor's
Physics Tool lets you drop dynamic bodies into the scene
hammer-style.

## Adding physics to a brush or entity

A physics-enabled entity needs two things:

- `AvianCollider`: jackdaw's wrapper around avian's
  `ColliderConstructor`. Picks the collider shape (cuboid,
  sphere, trimesh-from-mesh, etc.) and rebuilds the actual
  `Collider` whenever you change it.
- `RigidBody`: dynamic / static / kinematic. Dynamic bodies
  fall under gravity; static bodies are immovable collision
  surfaces.

Workflow:

1. Select the brush (or any entity with a mesh).
2. Inspector panel: click `+ Add Component`.
3. Search "AvianCollider" and pick it (it lives under the
   **Avian3d** category).
4. Picking `AvianCollider` auto-adds `RigidBody` via the
   require chain. Default body is `Dynamic`.

The collider builds from the entity's geometry on the next
tick. For brushes, jackdaw triangulates the brush faces and
hands them to avian. For mesh entities (`Mesh3d`), avian
reads the loaded mesh asset.

You'll see the green wireframe overlay on the brush once the
collider is up. If you don't, the collider build failed
silently; check the brush has finite volume.

### Switching collider shape

`AvianCollider` is a single-field tuple struct holding a
`ColliderConstructor`. The inspector renders it as an enum
dropdown. Common picks:

- `Cuboid` / `Sphere` / `Capsule` / `Cylinder` / `Cone`:
  primitive shapes, parameters are the half-extents / radii.
- `TrimeshFromMesh`: builds a triangle mesh collider from
  the entity's mesh. Best for brushes and detailed props
  where shape fidelity matters; expensive for rigid bodies.
- `ConvexDecompositionFromMesh`: V-HACD decomposes the mesh
  into convex hulls. Use for dynamic props where trimesh
  isn't valid.

Trimesh colliders are static-only in practice (avian rejects
trimesh colliders on dynamic bodies). For dynamic props, use
a primitive or convex decomposition.

### Static level geometry

For platforms, walls, and floors: set `RigidBody` to `Static`
in the inspector. Static bodies don't fall, can't be moved by
forces, and serve as the collision surface other bodies land
on.

Default is `Dynamic`; switch to `Static` after adding the
bundle if the brush is meant to be level geometry.

## Physics Tool: dropping props into place

Once entities have colliders, you'd usually want to author
their resting positions by simulating instead of guessing
poses. That's what the Physics Tool is for.

### Workflow

1. Select the props you want to place (one or many).
2. Press `Shift+P` to enter the Physics Tool.
3. The status bar reads `Physics Tool | drag selected to
   release | Space commit | Esc cancel`.
4. Click and drag a selected entity. Release. Gravity takes
   over and the body falls / settles.
5. Drag again to nudge.
6. Press `Space` to commit and exit. Settled positions are
   pushed onto the undo stack as a single entry, so `Ctrl+Z`
   returns you to physics mode at those positions for
   another pass.
7. `Esc` instead of `Space` cancels and reverts to the
   pre-tool poses.

### Selected vs non-selected

The tool only simulates the **selected** entities. Every
other dynamic / kinematic body in the scene gets paused
(`RigidBodyDisabled`) so it acts as a static obstacle while
the selection settles. Static bodies are always solid; the
tool never disables them.

This is the key UX: select what you want to place, ignore
the rest, drop them in. Re-select a different group to
place that one without disturbing the first.

### Visual cues

- Green wireframe: collider visible while a body is around.
- Orange: collider visible on a selected body.
- Cyan / blue: a sensor.
- Hierarchy arrows (toggle in `View` menu): show the body
  to collider parent / child links.

## Common gotchas

- **Dynamic body falls through the floor.** The floor
  isn't a static body, or the floor entity has no collider.
  Add `AvianCollider` to the floor and set its `RigidBody`
  to `Static`.
- **Collider wireframe is the wrong shape after rescaling.**
  Should track scale gizmo edits since the avian-integration
  fix in this branch. If you still see drift, file an issue
  with the collider type and the resize gesture.
- **`ColliderConstructor` panic when added directly.**
  Picking the raw `ColliderConstructor` (not `AvianCollider`)
  on an entity without a `Mesh3d` panics avian's auto-init.
  The picker hides standalone `ColliderConstructor` for this
  reason; pick `AvianCollider` instead.
- **Body can't be selected in physics mode.** Selection
  works the same as Object mode (LMB-click). If clicks land
  on the wrong body, check the cursor is over the body's
  collider, not just its visual mesh.
- **Body doesn't move when I drag.** The first drag in a
  physics session unpauses `Time<Physics>`; if your drag is
  too short to clear the threshold, the sim never starts.
  Drag a few pixels.
