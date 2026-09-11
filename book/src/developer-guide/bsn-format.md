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

A material or any other asset file is referenced by its path
under `assets/`:

```
my_game::Signpost { board: "materials/slate.material.bsn" }
```

A scene-local asset, defined inline in the same `.bsn` file, is
referenced as `#Name`. The `@Name` spelling of a project-wide
asset is what files written before paths use; it still resolves
while a project catches up, as long as one file answers to that
name, and the next save writes the path.

Every reference resolves against the same table at load time:
the project's asset files under their paths, plus the scene's
own inline definitions. A reference that resolves to nothing is
left as the file spelled it and falls back to a default handle
rather than failing the load, so a missing material shows up as
untextured geometry, not an error, and a save does not quietly
drop what could not be found.

A reference no asset file answers to is loaded through the asset
server as a file path, which is how an image, a mesh or any other
file the engine loads for itself is named.

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

An asset file is a `.bsn` naming the type it holds, and it can
sit in any folder under `assets/`; the editor indexes every one
it finds by the path it sits at. `assets/catalog.bsn` holds only
what has no file of its own. Any scene in the project can
reference either with `@Name`. Legacy
catalogs at `.jsn/catalog.jsn` or `assets/catalog.jsn` are read
for migration and rewritten to `assets/catalog.bsn` on the
next save.

## Asset files in a game

The runtime reads the project's asset files itself: at startup it walks
every `.bsn` under the asset folder, takes the type each file holds
from its header or its first root, and loads the ones whose type the
game registered as a reflected asset into that type's store. A file is
reachable by the path it sits at, and, while a project still spells
references by name, by its stem where only one file carries that stem.
A file naming a type the game has not registered is skipped with a
warning; scenes and prefabs are left to the loaders that own them.

This is a walk rather than a Bevy `AssetLoader` for `.bsn`, because a
typed handle needs one loader per concrete asset type the game
registers, while the walk is generic over reflection and matches the
index the editor builds for the same folder. Loading a type
asynchronously through the asset server can come later without changing
how a file is written or referenced.

A game that keeps its definitions as rows rather than as assets reads
the same files directly:

```rust,ignore
use jackdaw_bsn::{read_asset_file, walk_asset_files};

for (path, type_path) in walk_asset_files(Path::new("assets")) {
    if type_path == ItemDef::type_path() {
        let item: ItemDef = read_asset_file(&path)?;
        items.insert(item.id.clone(), item);
    }
}
```

`walk_asset_files` yields one entry per asset file with the type it
holds, leaving scenes and prefabs out. `read_asset_file` reads the
value the file's first root holds, refusing a file whose root is
another type; fields naming other assets by path stay at their
defaults, since resolving those takes an asset server.

## What is not in BSN

- Mesh data. Brushes serialize as their face planes; the mesh
  rebuilds from those at load. `.glb` imports reference the
  file path, not its contents.
- Textures. References only.
- Editor-internal entities. Brush face entities, gizmo
  helpers, picker panels, and similar carry an `EditorOnly` or
  `NonSerializable` marker that the saver skips.
