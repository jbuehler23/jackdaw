# Crate structure

Jackdaw is a workspace with one editor binary, a handful of
runtime / format crates that user games depend on, and a
larger group of internal-only crates that the editor
consumes. The split exists so a shipped game pulls in only
what it needs.

## What a user game depends on

Three crates, no editor in the dependency graph:

- `jackdaw_jsn`: the `.jsn` format, types, loader, and the
  Bevy plugin that wires the loader into Bevy's asset
  pipeline. Everyone needs this.
- `jackdaw_runtime`: the standalone scene loader, plus the
  `EditorMeta` / `ReflectEditorMeta` reflect attributes
  (`EditorCategory`, `EditorDescription`, `EditorHidden`)
  that user game crates use on their components.
- `jackdaw_geometry`: brush data structures (`BrushFaceData`,
  CSG, triangulation). Needed at runtime because the
  standalone game has to rebuild brush meshes from the
  serialized planes.

`game-static` template's `Cargo.toml` shows the canonical
shape.

## What the editor adds on top

The `jackdaw` crate (top-level) is the editor binary plus the
plugin group `EditorPlugins`. It depends on every other crate
in the workspace. The interesting layers:

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

## Extension and dylib plumbing

Seven crates exist for the extension story:

- `jackdaw_api`: the public surface third-party extensions
  link against. Re-exports bevy plus the operator /
  extension traits. Has a `dynamic_linking` feature that
  flips bevy to its dylib build.
- `jackdaw_api_internal`: host-side plumbing (loader plugin,
  catalog, enable/disable helpers, internal markers).
  `jackdaw_api` deliberately does not re-export this.
- `jackdaw_api_macros`: proc-macros backing the extension
  API.
- `jackdaw_sdk`: the proxy dylib that scaffolded extension
  projects link against via
  `--extern bevy=libjackdaw_sdk.so`. Carries the one
  compiled copy of bevy + jackdaw types both sides share.
- `jackdaw_dylib`: the dynamic-loader shim that dlopens
  extension dylibs at runtime.
- `jackdaw_loader`: the host-side resource that tracks
  loaded dylibs.
- `jackdaw_rustc_wrapper`: the rustc interceptor crate.
  Ships its `jackdaw-rustc-wrapper` binary, which scaffolded
  dylib projects invoke through `.cargo/config.toml` to
  inject the right `--extern` flags.

## Other crates

- `jackdaw_fuzzy`: fuzzy-match scoring for the picker /
  command palette. Tiny.
- `jackdaw_widgets`: project-specific widgets that don't
  belong in `jackdaw_feathers` (history view, color picker,
  etc.).

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
