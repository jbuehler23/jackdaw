# Open challenges

This is the honest list. Stuff that's not done, or is partly
done, or is genuinely hard. Nothing here is shipped. If you
want to take a swing at any of it, please file an issue first
so we can talk through the approach.

## Windows dylib hardening

The editor loads project and extension code as dylibs built
against its SDK proxy. On Windows, the PE binary format has a
65,535 export cap, and bevy + jackdaw types together push close
to it; the SDK build routes through `rust-lld` and disables
incremental codegen to stay linkable. That works, but the export
count grows with every API surface addition, and the failure
mode when the cap is hit is a link error deep in the SDK build.

Where to dig in: trimming what the SDK proxy re-exports, and a
CI check that tracks the export count so a regression is caught
before it ships.

## Play-In-Editor (PIE) depth

PIE is the "click play to run your game" flow. The process model
is settled: the game always runs out of process, launched by the
prebuilt runner over IPC, with zero play-time compilation. Frame
streaming into the Game panel, input capture, click-to-select
picking, and the Live entity tree all shipped; see
[Play-in-editor](../user-guide/play-in-editor.md).

What's not done: deeper live editing (a broader set of
component edits riding back into the running game and into the
authored scene), richer widget metadata for live values, and
protocol maturity across more component types.

Where to dig in: pick one component family that doesn't
round-trip yet and follow it through the IPC lanes.

## Upstream BSN alignment

Jackdaw's scene document is BSN: `.bsn` files are the authored
format, the live in-editor document is the BSN AST, and `.jsn`
survives only as a read-only importer. What remains is staying
aligned with the upstream Bevy scene work as its APIs settle,
and upstreaming the pieces of jackdaw's writer that make sense
there.

Where to dig in: track the upstream scene-notation APIs and
diff them against `crates/jackdaw_bsn` as they move.

## Engine-feature gaps

Compared to other game engines, jackdaw is missing a bunch.
None of these are blockers; they're places where someone with
taste in the area could lead. One line each:

- Animation graph editor. Started in `crates/jackdaw_animation`,
  not finished.
- Particle / VFX editor. Not started.
- Material graph editor (shader-graph style). Not started.
- Light baking and lightmap pipeline. Not started.
- Navmesh debug overlay. We have a navmesh component but no
  visualisation.
- Cinematics / cutscene editor. Not started.
- Audio mixer. Not started.
- Localization (i18n). Not started.
- In-editor profiler / frame-time inspector. Not started.
- Asset import beyond GLTF (FBX, USD, batch texture
  compression). Not started.
- Level streaming for large open worlds. Not started.

If you care about any of these, opening a small "here's what
I'd do" issue is the best starting point. We don't want to
solo-design any of them.

## Asset processing pipeline

Right now asset processing only happens at editor runtime. If
you want to pre-process textures or bake meshes for a CI
build, you have to start the editor headlessly, which is not
great.

Half of the second shape below now exists: `jackdaw-cli` drives
the editor's build machinery from a terminal with `build` and
`run`. What is missing is a `process` step and the
asset-processing pipeline behind it.
The remaining shapes:

- Split the user's game into a library plus multiple binaries
  (run, process), with processing driven from the project's own
  binaries. Invasive for the project template.
- Extend `jackdaw-cli` with a `process` subcommand alongside
  `build`. Less invasive but more code in jackdaw.

Where to dig in: pick one shape and prototype it against a
small game. We'd like to see the workflow before locking in
the design.

## Single-entity editor-only ergonomics

Today `EditorOnly` skips the whole entity from save, so to
have a `PlayerSpawn` marker that ships and a visual indicator
that doesn't, you author a parent (with `PlayerSpawn`) and a
child (with `EditorOnly` + a mesh).

A single entity cannot carry both today, because the save
filter is at entity granularity. A
future `EditorOnlyVisuals` marker that strips visual components
(`Mesh3d`, `MeshMaterial3d`, etc) at save time but keeps the
entity and its non-visual components would enable single-
entity authoring. The cost is a small allowlist of "visual"
component types that grows as bevy adds new ones.

Where to dig in: design the allowlist, file an issue, then
implement. The semantics decision is the harder part than
the code.

## Brush face children as a custom relationship

Each brush spawns N face child entities for rendering. They
carry `EditorHidden` (so they're not in the outliner) and
`NonSerializable` (so they're not in the save). But
`Children` queries on the brush still enumerate them, which
means user code that walks brush children sees jackdaw's
implementation detail.

A custom Bevy relationship (not `ChildOf`) for face entities
would solve this cleanly. The face entities would be reachable
through the relationship but invisible to standard `Children`
queries. The cost is a small per-frame propagation system that
reads the brush's `GlobalTransform` and writes the face's.

Where to dig in: the relationship API in Bevy 0.19, and
whether we can do this without breaking `BrushFaceEntity`
queries that already work.
