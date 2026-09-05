# Your first scene

This page walks you from a blank project to a saved scene
with one cube in it. Five minutes, give or take.

## Pick a starting point

Two starting paths:

- **New Project > Game** on the launcher (or `jd new
  my-game` from the terminal). You get a normal Bevy crate: a
  `lib.rs` with a `GamePlugin`, a `main.rs` that runs the
  standalone game, a starter scene, and a `jackdaw.toml`. Pick
  this if you want to ship a real binary later.
- **New Scene** inside an already-open project. Use this if you
  just want to author a scene next to ones you have.

A new project opens immediately. The editor builds the project's
cargo binary in the background (same as `cargo build` in the
project root) and asks it for its reflected type schema. Your
own components show up in the inspector once that finishes.
Placing brushes and saving scenes works right away, so you do
not have to wait for it.

Expect that first build to take around nine minutes: it compiles
Bevy from source, the same as any Bevy project. Every project
pays it once. Rebuilds after that are 1 to 4 seconds, so this
is the only time you will sit through it.

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

`File > Save` (or `Ctrl+S`). A project from the Game template
already has `assets/scene.bsn` open, so this writes straight
back to it. A scene created with `File > New Scene` asks where
to put the file the first time; pick `assets/scene.bsn` to
match what the template loads.

Open that `.bsn` in your text editor if you want to peek. It
is plain text, with one entry per entity and reflected
component data inline. See
[BSN Format](../developer-guide/bsn-format.md) for the syntax.

## See it run outside the editor

From the project folder:

```sh
cargo run
```

This launches the standalone binary. `main.rs` adds
`jackdaw_runtime::JackdawPlugin`, which registers the asset
loader for `.bsn` files, and the template's `GamePlugin`
spawns a `JackdawSceneRoot` pointing at `scene.bsn`. No editor
in the loop. The cube sits where you placed it, and any
components you attached in the inspector are alive on the
entity.

Bevy cannot load `.bsn` on its own; the loader ships in
`jackdaw_runtime`, which is an ordinary dependency of your
crate.

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
