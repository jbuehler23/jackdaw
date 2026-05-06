# Open challenges

This is the honest list. Stuff that's not done, or is partly
done, or is genuinely hard. Nothing here is shipped. If you
want to take a swing at any of it, please file an issue first
so we can talk through the approach.

## Windows dylib loading

The dylib path (where the editor `dlopen`s a hot-reloadable
game `.so`) doesn't currently work on Windows. The PE binary
format has a 65,535 export cap, and bevy + jackdaw types
together blow past that.

It's a binary-format property, not a linker setting. Switching
to `rust-lld` instead of MSVC `link.exe` was tried and doesn't
help. eugineerd attempted splitting bevy into multiple
per-subcrate dylibs and ran into the diamond dependency problem
described
[here](https://robert.kra.hn/posts/2022-09-09-speeding-up-incremental-rust-compilation-with-dylibs/#limitation-the-diamond-dependency-problem).

Where to dig in: making per-bevy-subcrate dylibs work needs
upstream bevy crate-type changes; we can't fix this entirely
inside jackdaw. The static-template path works on Windows
today and is the recommended setup. If you have a clean idea
for the multi-dylib split (especially one that avoids the
diamond), let us know.

## Play-In-Editor (PIE) maturity

PIE is the "click play to run your game inside the editor"
flow. The first three phases shipped:

- BRP foundation (the Bevy Remote Protocol plumbing).
- Entity browser for the live game state.
- Read-only remote inspector.

What's not done: bidirectional editing (changing a value in
the inspector and having it ride back to the game),
WebSocket streaming instead of HTTP polling, full component
widget metadata, and a clean story for spawning the game in
its own process versus loading it as a dylib.

The big open question is in-process vs. out-of-process.
In-process is cheap (no IPC) but a panicking game can take
down the editor. Out-of-process is safer but needs a real
protocol for state sync. Discussion lives in the GitHub
issues and on Discord.

Where to dig in: pick a small slice (like "round-trip a
single Transform edit") and prototype the wire format.

## Migration to BSN

Cart's BSN PR for Bevy 0.19 establishes scene-document-as-
source-of-truth at the engine level. Jackdaw's current
JSN-first refactor was deliberately shaped to mirror BSN, so
the swap should be mostly mechanical.

What's not done: the actual swap. We need to wait for BSN to
land, then port. The risk is timing; BSN might land between a
jackdaw release cycle, and we want to stay shippable
throughout.

Where to dig in: track the BSN PR upstream, run the
JSN-to-BSN diff on a test scene as soon as the upstream
format is stable, and prototype the loader changes against a
local bevy fork.

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

andriyDev raised this in editor-dev. The natural shapes are:

- Split the user's game into a library plus multiple binaries
  (run, process). Closer to Unreal's UAT model. Invasive for
  the static template.
- Add a `cargo jackdaw` subcommand with `process`, `build`,
  etc. Closer to Unity's CLI. Less invasive but more code in
  jackdaw.

Where to dig in: pick one shape and prototype it against a
small game. We'd like to see the workflow before locking in
the design.

## Single-entity editor-only ergonomics

Today `EditorOnly` skips the whole entity from save, so to
have a `PlayerSpawn` marker that ships and a visual indicator
that doesn't, you author a parent (with `PlayerSpawn`) and a
child (with `EditorOnly` + a mesh).

Jan asked whether the same entity could carry both. It can't
today, because the save filter is at entity granularity. A
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

Where to dig in: the relationship API in Bevy 0.18, and
whether we can do this without breaking `BrushFaceEntity`
queries that already work.
