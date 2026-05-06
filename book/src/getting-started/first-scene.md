# Your first scene

This page walks you from a blank project to a saved scene
with one cube in it. Five minutes, give or take.

## Pick a starting point

The launcher has two starting paths:

- **+ New Project** with the `game-static` template. You get
  a `lib.rs` with a `MyGamePlugin`, a `bin/editor.rs` that
  hosts the editor, and a `main.rs` that runs the standalone
  game. Pick this if you want to ship a real binary later.
- **+ New Scene** inside an already-open project. Use this
  if you just want to author a `.jsn` next to ones you have.

If you used the static template, the launcher offers to build
the editor binary on first open. Say yes; subsequent opens are
fast incremental rebuilds.

## Place a cube

Once the editor is open:

1. In the **Hierarchy** panel, right-click and pick
   `Add > Cube`.
2. The cube appears at the origin. Click it in the viewport
   or in the hierarchy to select.
3. With the cube selected, drag a translation arrow on the
   gizmo. The default mode is translate; press `R` for
   rotate, `T` for scale, `Esc` to return to translate.
   Arrow keys nudge on the grid.

That cube is a brush, not a `.glb` import, so you can edit
its faces in place. See the
[Brushes](../user-guide/brushes.md) chapter when you want to
do that.

## Save the scene

`File > Save` (or `Ctrl+S`). The first save asks where to put
the file; pick `assets/scene.jsn` to match what the static
template's standalone binary expects.

Open that `.jsn` in your text editor if you want to peek. It
is plain JSON-ish text, with one entry per entity and reflect
component data inline. The format is documented in
[JSN Format](../developer-guide/jsn-format.md).

## See it run outside the editor

If you scaffolded with `game-static`:

```sh
cargo run
```

This launches the standalone binary. It loads
`assets/scene.jsn` from disk and runs your `MyGamePlugin`. No
editor in the loop. The cube sits where you placed it, and
any components you attached in the inspector are alive on
the entity.

## What you have now

A project with one scene, one cube, and a save/load round
trip you can iterate on. Next steps:

- [Viewport Navigation](../user-guide/viewport-navigation.md)
  for getting around the 3D view.
- [Custom Components](../developer-guide/custom-components.md)
  to attach your own behaviour to the cube.
- [Migrating an Existing Project](migrating-an-existing-project.md)
  if you already have a Bevy game and want to wire jackdaw
  into it.
