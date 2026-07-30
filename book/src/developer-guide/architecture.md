# Architecture

Jackdaw is a standalone editor built from Bevy 0.19 plugin sets. The editor and the standalone
runtime share the same scene format and the same component
reflection. There's no separate engine; if you can write a Bevy
plugin, you can write a jackdaw extension.

## Plugin structure

The composable editor is delivered by `jackdaw_editor` as
`JackdawEditorPlugins`, a Bevy `PluginGroup`.
The editor binary looks like:

```rust
App::new()
    .add_plugins(DefaultPlugins.set(editor_window_plugin()))
    .add_plugins((PhysicsPlugins::default(), EnhancedInputPlugin))
    .add_plugins(JackdawEditorPlugins::default())
    .run()
```

`JackdawEditorPlugins` pulls in everything jackdaw needs: the launcher,
viewport, hierarchy, inspector, brush tools, asset browser,
scene IO, and the extension loader. Project code is not compiled
into this binary; it arrives as a dynamic library the editor
builds and loads per project (see below).

The user's `GamePlugin` is used by the standalone game. The game
doesn't add `JackdawEditorPlugins`; it adds
`JackdawPlugin` from `jackdaw_runtime`, which is a much smaller
plugin that knows how to load authored scenes but doesn't include
any UI.

`WindowPlugin` is set by `editor_window_plugin()`.

## App states

The launcher and the editor are the same binary. The state
machine is:

- `AppState::ProjectSelect` is the launcher screen. Recent
  projects, new project, open existing.
- `AppState::Editor` is the editor proper. Once you pick a
  project, you stay here for the session.

You can read the transitions in `src/lib.rs` and
`src/project_select.rs`.

## Project code in the editor

A jackdaw project is a normal Bevy crate. When you open one, the
editor generates a shim crate into the project's gitignored
`.jackdaw/` directory and builds the project's library as a
dynamic library against the editor's SDK (a proxy dylib carrying
the one compiled copy of bevy + jackdaw types the editor and
everything it loads share). The build runs in the background;
when it finishes, an out-of-process extractor reports the project's reflected
schema. The editor represents those types as data rather than mapping game
code into its process.

The editor owns this build completely: it lives in
`.jackdaw/target/`, and the user's own `Cargo.toml`,
`Cargo.lock`, `target/`, and toolchain are never touched. A plain
`cargo run` in the project compiles ordinary crates.io Bevy.

Play is out of process: the editor launches a prebuilt
`jackdaw-runner` binary that loads the same already-built project
dylib and talks to the editor over IPC. Nothing compiles at play
time, and a game crash cannot take down the editor.

## Scene format

Scenes are stored as `.bsn` files under `assets/`. Each entity
lists its reflected components inline. The live in-editor document
is the BSN AST (`SceneBsnAst`); saving writes it back out as
`.bsn` text, and that is the only format anything writes. The
serializer skips types tagged with `@EditorHidden`, the
entity-level `EditorHidden` marker, `NonSerializable`, and
`EditorOnly`. Legacy `.jsn` scenes can still be imported; see
[BSN Format](bsn-format.md).

Outside the editor, `jackdaw_runtime` registers a Bevy
`AssetLoader` for the `bsn` extension, since Bevy has no built-in
loader for the format. The loader processes scene entities in
topological order (parents before children) and bundles
`Transform`, `Visibility`, `GlobalTransform`,
`InheritedVisibility`, and `ChildOf` into a single `world.spawn`
per entity. User components go in afterwards, so `On<Insert, T>`
observers see correct hierarchy-derived state.

## Brushes

Brushes are jackdaw's CSG primitives, used for level geometry.
The data lives on the brush entity as a `Brush` component
(`faces: Vec<BrushFaceData>`, where each face carries a plane,
texture, material, and per-face UVs). Each face becomes a
child entity with a generated mesh; those children carry
`EditorHidden` and `NonSerializable` so they don't show in
the outliner and aren't saved (they're rebuilt from the
parent's `Brush` data on load).

Code:

- `src/brush/mod.rs` is the resource and component layer.
- `src/brush/mesh.rs` rebuilds face meshes when the brush
  changes.
- `src/brush/interaction.rs` is the editing state machine
  (face drag, vertex drag, edge drag).

## Inspector and picker

The inspector is modular. Each component type renders through a
display function that walks its reflected fields. The picker
that shows on `+ Add Component` enumerates the type registry,
filters out anything tagged `@EditorHidden`, and sorts by
category.

Code:

- `src/inspector/mod.rs` is the dispatcher.
- `src/inspector/component_picker.rs` is the `+ Add Component`
  flow.
- `src/inspector/reflect_fields.rs` renders primitive fields.

## Extensions

The editor can be extended by writing a normal Bevy library that
depends on `jackdaw_api` and implements the `JackdawExtension`
trait. Opening the extension project in jackdaw builds and loads
it; the Extensions dialog installs prebuilt extension dylibs.
Extensions can register operators, windows, menu entries, and
keybinds. See [Extending the Editor](extending-the-editor.md)
for the full story.

The dylib loader is `crates/jackdaw_loader`. The proxy dylib
that projects and extensions link against is
`crates/jackdaw_sdk`. The rustc wrapper at
`crates/jackdaw_rustc_wrapper` rewrites `--extern bevy=...` so
loaded code and the editor share one compiled copy of bevy types.

## What's not here yet

The architecture page doesn't try to cover every system. The
big unfinished pieces (animation graph, asset processing
pipeline, and the rest) live in
[Open Challenges](open-challenges.md). The
[Crate Structure](crate-structure.md) page lists the workspace
crates and their roles.
