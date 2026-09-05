# BSN format

BSN ("Bevy Scene Notation") is the on-disk format for jackdaw
scenes. It is a reflection-based notation: each entity lists
its components by full type path, with values in a compact
struct / enum / tuple syntax that round-trips through Bevy's
reflect system. Scene files are human-readable and
line-diffable in git.

The parser and scene document live in `crates/jackdaw_bsn`.
The live in-editor document is the BSN AST (`SceneBsnAst`);
saving writes it back out as `.bsn` text. Source of truth for
the grammar is that crate; this page is the orientation.

## Legacy JSN import

`.jsn` ("Jackdaw Scene Notation") is the previous scene format:
JSON with a fixed schema, implemented in `crates/jackdaw_jsn`.
It survives as an import-only path. Opening a legacy `.jsn`
scene converts it to `.bsn` on disk (the original is kept as a
`.jsn.bak` backup), and the editor works with the `.bsn` from
then on. Nothing writes `.jsn` any more; `jackdaw_jsn` is a
read-only importer.

## Scene shape

A scene is a list of root entity nodes. Each node names its
components; child entities nest under
`bevy_ecs::hierarchy::Children`.

```
#Root
bevy_transform::components::transform::Transform
bevy_camera::visibility::Visibility::Visible
bevy_ecs::hierarchy::Children [
    #Main Camera
    bevy_camera::components::Camera3d
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 0.0, y: 6.0, z: 12.0 },
        rotation: glam::Quat { x: -0.216, y: 0.0, z: 0.0, w: 0.976 },
    }

    #Sun
    bevy_light::directional_light::DirectionalLight
]
```

- `#Name` labels the entity (its `Name` component).
- A bare type path is a component at its default value.
- `Type { field: value, .. }` sets struct fields; omitted
  fields keep their defaults.
- `Type::Variant` is an enum value; `Type(value)` a tuple
  struct.
- `bevy_ecs::hierarchy::Children [ .. ]` nests child nodes.

Component keys are full type paths (the same string the
inspector shows under "type path"). Values are whatever Bevy's
reflect produces for that type, so nested types spell out
their own paths (`glam::Vec3 { .. }`). Children come after
their parent, so parent / child order is a property of the
nesting, not of a flat entity list.

## Asset references

Materials and other shared assets are referenced by name:

- `#Name` for a scene-local asset, defined inline in the same
  `.bsn` file.
- `@Name` for a project-wide asset, resolved from the project
  catalog.

Both prefixes resolve against the same name-to-handle table at
load time, populated from the scene's own inline definitions and
the project catalog. A name that resolves to nothing falls back
to a default handle rather than failing the load, so a missing
material shows up as untextured geometry, not an error.

A component value that is a plain path string (no prefix) is
loaded through the asset server as a file path instead.

## Project file

Per-project editor settings live in `.jackdaw/project.json`, a
plain JSON file inside the editor's build directory:

```json
{
  "name": "My Game",
  "description": "",
  "default_scene": "assets/scene.bsn",
  "last_open_tabs": ["assets/scene.bsn"],
  "layout": { }
}
```

All scene paths here are relative to the project root, so they
keep working when the folder moves. `last_open_tabs` is what the
editor actually reopens; `default_scene` is reserved and not yet
consulted. `layout` is the persisted dock layout and is
intentionally opaque to the config (consumers parse it as the
`jackdaw_panels` workspace state). Legacy projects that keep a
`.jsn/project.jsn` or root `project.jsn` are migrated to
`.jackdaw/project.json` on open.

## Catalog file

Project-wide named assets live in `assets/catalog.bsn`. Any
scene in the project can reference them with `@Name`. Legacy
catalogs at `.jsn/catalog.jsn` or `assets/catalog.jsn` are read
for migration and rewritten to `assets/catalog.bsn` on the
next save.

## What is not in BSN

- Mesh data. Brushes serialize as their face planes; the mesh
  rebuilds from those at load. `.glb` imports reference the
  file path, not its contents.
- Textures. References only.
- Editor-internal entities. Brush face entities, gizmo
  helpers, picker panels, and similar carry an `EditorOnly` or
  `NonSerializable` marker that the saver skips.
