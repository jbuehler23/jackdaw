# Crate structure

Jackdaw is a workspace with one editor binary, a handful of
runtime / format crates that user games depend on, and a
larger group of internal-only crates that the editor
consumes. The split exists so a shipped game pulls in only
what it needs.

## What a user game depends on

One direct dependency, no editor in the dependency graph:

- `jackdaw_runtime`: the standalone scene loader for authored
  `.bsn` scenes, the optional `physics` feature that builds
  avian colliders from authored data, and the `EditorMeta` /
  `ReflectEditorMeta` reflect attributes (`EditorCategory`,
  `EditorDescription`, `EditorHidden`) that user game crates
  use on their components.

`JackdawPlugin` registers a Bevy `AssetLoader` for the `bsn`
extension. Bevy ships no loader for that format, so a game
without `jackdaw_runtime` cannot open an authored scene.

It pulls in the scene and geometry crates:

- `jackdaw_bsn`: the `.bsn` scene format, its parser, and the
  scene document.
- `jackdaw_scene_types`: the shared components (`Brush`,
  scene node ids, custom properties).
- `jackdaw_geometry`: brush data structures (`BrushFaceData`,
  CSG, triangulation). Needed at runtime because the
  standalone game has to rebuild brush meshes from the
  serialized planes.

`jackdaw_jsn` is not in this graph. It is a read-only importer
for the legacy `.jsn` format and only the editor depends on it.

The game template's `Cargo.toml` shows the canonical shape: a
normal Bevy crate with `bevy`, `jackdaw_runtime`, and a physics
crate, and nothing editor-related.

## What the editor adds on top

The `jackdaw` package is the official editor installation. The public
`jackdaw_editor` crate exposes the `JackdawEditorPlugins` composition seam.
They depend on nearly everything
else in the workspace. The interesting layers:

- `jackdaw_feathers` / `jackdaw_widgets` / `jackdaw_panels`:
  the UI layer. Feathers is the styled-widget primitives,
  widgets are the higher-level pieces (split panels, dock,
  picker), panels is the docking system.
- `jackdaw_camera`: viewport camera plugin (fly camera,
  orbit, bookmarks). Standalone games can use it too, since
  it doesn't depend on anything editor-specific.
- `jackdaw_commands`: the undo/redo command stack. Editor
  operations push `EditorCommand`s here.
- `jackdaw_terrain`: heightmap data + sculpt + erosion.
- `jackdaw_avian_integration`: physics overlays and the
  Physics tool. Glue between the editor and Avian.
- `jackdaw_animation`: animation graph editing, clip
  authoring.
- `jackdaw_node_graph`: node-graph primitives shared between
  the animation editor and the (planned) signal editor.
- `jackdaw_remote`: the Bevy Remote Protocol (BRP) client
  used by the remote inspector when talking to a running
  game.
- `jackdaw_camera_rig`: authorable first/third-person camera
  rig components plus the runtime driver that moves them.
  Optional, behind the default-on `camera_rig` feature.
- `jackdaw_csg`: the glue between brushes and the manifold3d
  mesh-boolean kernel.
- `jackdaw_snap`, `jackdaw_select`, `jackdaw_uv`,
  `jackdaw_pick`, `jackdaw_hull`, `jackdaw_material`:
  engine-agnostic editing math (snapping, half-edge selection
  traversal, UV projection, ray and point queries, convex
  hulls, PBR texture-set detection). No bevy dependency; the
  editor is a thin adapter over them.
- `jackdaw_multiplayer`, `jackdaw_multiplayer_editor`,
  `jackdaw_multiplayer_lightyear`: networking authoring. The
  editor writes replication metadata only; the lightyear
  backend lives game-side.
- `jackdaw_localization`: editor string catalogue.
- `bevy_window_chrome`: custom title bar window chrome for Bevy.

## Play and the command line

- `jackdaw_project_build`: the build pipeline. Generates the
  shim crate, drives cargo through the rustc wrapper, extracts
  the component schema, and owns SDK path resolution and the
  first-run SDK bootstrap. Deliberately bevy-light so the CLI
  can link it without dragging in a renderer.
- `jackdaw_cli_internal`: bevy-light command implementations used by `jd`.
  Release-only packaging is invoked through `cargo xtask`.
- `jackdaw_runner`: the prebuilt game runner. Play dlopens the
  already-built project dylib through it, so nothing compiles
  at play time.
- `jackdaw_pie_protocol`: the IPC message types and the
  `jackdaw.toml` run-configuration manifest shared by the
  editor and the runner.

## Project and extension dylib plumbing

Seven crates exist for building and loading project and
extension dylibs:

- `jackdaw_api`: the public surface extensions link against.
  Re-exports bevy plus the operator / extension traits
  (including `JackdawExtension`). Has a `dynamic_linking`
  feature that flips bevy to its dylib build.
- `jackdaw_api_internal`: host-side plumbing (loader plugin,
  catalog, enable/disable helpers, internal markers).
  `jackdaw_api` deliberately does not re-export this.
- `jackdaw_api_macros`: proc-macros backing the extension
  API.
- `jackdaw_sdk`: the proxy dylib that project and extension
  builds link against via `--extern bevy=libjackdaw_sdk.so`.
  Holds the single compiled copy of bevy + jackdaw types
  shared between both sides.
- `jackdaw_dylib`: the dynamic-loader shim that dlopens
  dylibs at runtime.
- `jackdaw_loader`: the host-side resource that tracks
  loaded dylibs, plus the crash quarantine.
- `jackdaw_rustc_wrapper`: the rustc interceptor crate.
  Ships its `jackdaw-rustc-wrapper` binary, which the
  editor's build pipeline invokes to inject the right
  `--extern` flags. User projects never configure it; the
  editor drives it from the generated `.jackdaw/` build
  root.

## Other crates

- `jackdaw_fuzzy`: fuzzy-match scoring for the picker /
  command palette. Tiny.
- `jackdaw_jsn`: read-only importer for the legacy `.jsn`
  format. Nothing writes `.jsn`; opening one converts it to
  `.bsn`.

## How to find things

If you are looking for a specific feature: search the editor
crate first (`src/`). If you find a `Plugin`, follow its
imports back to the crate that owns the underlying logic.
The editor crate is mostly orchestration; real work lives in
the workspace crates.

## What needs splitting

`src/` is over 100 files. The brush, animation, and remote
inspector subsystems are the obvious candidates for
extraction into their own crates. Not blocking on it.
