# JSN format

JSN ("Jackdaw Scene Notation") is the on-disk format for
scenes, projects, and the asset catalog. It is JSON with a
fixed schema, designed to be human-readable, line-diffable in
git, and to round-trip through Bevy's reflect system without
losing information.

The types live in `crates/jackdaw_jsn/src/format.rs`. Source
of truth for the field list is the structs there; this page
is the orientation.

## Three file kinds

- `*.jsn` (scene): one entity tree. Authored in the editor,
  loaded by the standalone runtime.
- `.jsn/project.jsn` (project config, legacy fallback at
  `<root>/project.jsn`): default scene, persisted dock
  layout, project metadata.
- `.jsn/catalog.jsn` (project-wide assets, legacy fallback
  at `assets/catalog.jsn`): named materials, prefab-ish
  assets shared across scenes in the project.

All three start with the same header struct
(`format_version`, `editor_version`, `bevy_version`) so
loaders can route on it.

## Scene shape

A scene file is one `JsnScene`:

```json
{
  "jsn":     { "format_version": [3, 0, 0], "editor_version": "0.4.0", "bevy_version": "0.18" },
  "metadata":{ "name": "Level 1", "author": "you", "created": "...", "modified": "..." },
  "assets":  { /* per-type-path tables */ },
  "editor":  null,
  "scene":   [ /* entities */ ]
}
```

### Entities

Each entry in `scene` is a `JsnEntity`:

```json
{
  "parent": 0,
  "components": {
    "bevy_transform::components::transform::Transform": { "translation": [...], "rotation": [...], "scale": [...] },
    "bevy_ecs::name::Name": "Player",
    "my_game::SpinningCube": { "speed": 1.5 }
  }
}
```

`parent` is the index of another entity in the same `scene`
array, or absent for roots. Component values are reflect-
serialized: whatever Bevy's reflect produces for that type.
Component keys are full type paths (the same string the
inspector shows under "type path").

The order of entries matters for parent / child resolution
(parents must come before children), but is otherwise just
authoring order.

## Asset references

`assets` is a two-level map: type path, then named asset.

```json
"assets": {
  "bevy_pbr::StandardMaterial": {
    "BrickWall": { "base_color": [0.6, 0.3, 0.2, 1.0] }
  }
}
```

Components reference these by name. The convention is:

- `#Name` for a scene-local asset (must exist in the same
  file's `assets` table).
- `@Name` for a project-wide asset (resolved from
  `catalog.jsn`).

The serializer resolves the prefix at load time. If neither
file has the named asset, the component falls back to its
default value and the loader logs a warning.

## Project file

`project.jsn` looks like:

```json
{
  "jsn":     { "format_version": [3, 0, 0], ... },
  "project": {
    "name": "My Game",
    "description": "",
    "default_scene": "assets/scene.jsn",
    "layout": { /* opaque LayoutState blob, owned by jackdaw_panels */ }
  }
}
```

`default_scene` is relative to the project root, so it works
when the user moves the folder around. `layout` is parsed as
`jackdaw_panels::LayoutState` and is intentionally opaque to
the JSN crate (so layout schema changes don't ripple into
format versioning).

## Catalog file

`catalog.jsn`:

```json
{
  "jsn":    { "format_version": [3, 0, 0], ... },
  "assets": { /* same shape as JsnScene.assets */ }
}
```

Just the assets table at the top level, plus the header.

## Versioning

`format_version` is a `[major, minor, patch]` triple. The
loader has a hand-written migration from v2 (the format
before brush data lived in components) to v3. v1 is no
longer supported. New format changes will go through the
same path: a `JsnSceneVN` deserializer plus a `migrate_to_v(N+1)`
method, called inside the loader before handing the scene to
the rest of the editor.

The migration is intentionally one-way. We do not keep older
versions on the read path; once a file has been opened and
saved by a newer editor, it is in the new shape.

## What is not in JSN

- Mesh data. Brushes serialize as their face planes; the
  mesh rebuilds from those at load. `.glb` imports
  reference the `.glb` file path, not its contents.
- Textures. Same; references only.
- Editor-internal entities. Brush face entities, gizmo
  helpers, picker panels, etc., carry an `EditorOnly` or
  `NonSerializable` marker that the saver skips.
