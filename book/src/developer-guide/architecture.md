# Architecture

Jackdaw is a Bevy 0.18 plugin set. The editor and the standalone
runtime share the same scene format (JSN) and the same component
reflection. There's no separate engine; if you can write a Bevy
plugin, you can write a jackdaw extension.

## Plugin structure

The editor is delivered as `EditorPlugins`, a Bevy `PluginGroup`.
A typical editor binary looks like:

```rust
App::new()
    .add_plugins(DefaultPlugins)
    .add_plugins((PhysicsPlugins::default(), EnhancedInputPlugin))
    .add_plugins(EditorPlugins::default())
    .add_plugins(your_crate::MyGamePlugin)
    .run()
```

`EditorPlugins` pulls in everything jackdaw needs: the launcher,
viewport, hierarchy, inspector, brush tools, asset browser,
scene IO, and the extension loader. Your `MyGamePlugin` is added
on top of that.

The same `MyGamePlugin` is used by the standalone runtime. The
standalone version doesn't add `EditorPlugins`; it adds
`JackdawPlugin` from `jackdaw_runtime`, which is a much smaller
plugin that knows how to load `.jsn` scenes but doesn't include
any UI.

## App states

The launcher and the editor are the same binary. The state
machine is:

- `AppState::ProjectSelect` is the launcher screen. Recent
  projects, new project, open existing.
- `AppState::Editor` is the editor proper. Once you pick a
  project, you stay here for the session.

You can read the transitions in `src/lib.rs` and
`src/project_select.rs`.

## Per-project editor binary

When you scaffold a project from the launcher, the project
gets its own `editor` binary at `src/bin/editor.rs`. This is
what `cargo editor` runs. The editor binary statically links
both `EditorPlugins` and your `MyGamePlugin`, which means your
gameplay code is always available to the editor's reflection
without any dynamic loading.

There's an experimental dylib path that loads game code as a
hot-reloadable `.so` instead, but it's off by default. Static
is the recommended path. See [Open Challenges](open-challenges.md)
for the dylib story.

## Scene format

Scenes are stored in `assets/scene.jsn`. The format is JSON;
each entity is a list of reflected components keyed by type
path. The serializer skips types tagged with `@EditorHidden`,
the entity-level `EditorHidden` marker, `NonSerializable`, and
`EditorOnly`.

The runtime loader processes JSN entities in topological order
(parents before children) and bundles `Transform`, `Visibility`,
`GlobalTransform`, `InheritedVisibility`, and `ChildOf` into a
single `world.spawn` per entity. User components go in
afterwards, so `On<Insert, T>` observers see correct
hierarchy-derived state. The relevant code is at
`crates/jackdaw_runtime/src/lib.rs::spawn_scene_entities`.

JSN is the current format. The plan is to swap to BSN (Cart's
Bevy 0.19 scene-document work) once that lands; jackdaw's
JSN-first refactor was deliberately shaped to make that swap
mechanical. See [Open Challenges](open-challenges.md).

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

The editor can be extended by writing a separate crate, building
it as a dylib, and dropping the `.so` in the editor's extensions
folder. Extensions can register operators, windows, menu
entries, and keybinds. See [Extending the Editor](extending-the-editor.md)
for the full story.

The extension loader is `crates/jackdaw_loader`. The proxy
dylib that extensions link against is `crates/jackdaw_sdk`. The
rustc wrapper at `crates/jackdaw_rustc_wrapper` rewrites
`--extern bevy=...` so extensions and the editor share one
compiled copy of bevy types.

## What's not here yet

The architecture page doesn't try to cover every system. The
big unfinished pieces (BSN migration, full PIE, dylib loading on
Windows, animation graph, asset processing pipeline) live in
[Open Challenges](open-challenges.md). The
[Crate Structure](crate-structure.md) page lists the workspace
crates and their roles.
