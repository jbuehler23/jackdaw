# Configuration

Configuration is split across three places: `jackdaw.toml` in the
project root (plugin override and run configurations), the user
config directory (global preferences and extension install dirs),
and `project.jsn` (per-project editor settings).

## jackdaw.toml

The one jackdaw-specific file in a project. Everything in it has a
working default; a project with an empty (or missing) file still
opens and plays.

```toml
# The game plugin type inside your lib crate. Uncomment to
# override source detection.
# plugin = "GamePlugin"

[[run]]
name = "Play"
# instances = 2
# env = { SERVER_ADDR = "127.0.0.1:5000" }
# args = []
# cwd = "some/subdir"
```

Top-level keys:

- `plugin`: the game plugin type inside your lib crate. Defaults
  to source detection, then to `GamePlugin`.

Each `[[run]]` entry is one item in the Play dropdown. Every run
launches the same already-built project library through the game
runner; entries differ only in launch environment, never in what
gets built. Fields:

- `name`: dropdown label. Defaults to `Play`.
- `instances`: number of individually launchable copies of this
  config (`Label #1..#N`). Defaults to 1.
- `env`: environment variables set on the game process. This is
  the game's input surface for config (server address, role, and
  so on).
- `args`: extra command-line arguments appended for the game.
- `cwd`: working directory; defaults to the project root.
- `mode`: engine-execution axis; the default is normal play, and
  `editor-preview` is reserved.

There is no bin or feature selection; runs don't build anything.
If the file is missing, the editor synthesizes a single default
run.

## The .jackdaw/ directory

`.jackdaw/` is the editor's build root inside the project:
a generated shim crate and a `target/` directory where the editor
builds the project as a dynamic library against its SDK. It is
gitignored (the scaffold and import both add the entry), owned
entirely by the editor, and safe to delete; the next project open
rebuilds it. The editor never touches the project's own
`Cargo.toml`, `Cargo.lock`, `target/`, or toolchain.

## Cargo features

These are features of the `jackdaw` crate itself, relevant if you
build the editor from source. Projects have no jackdaw-related
features.

- `default = ["multiplayer", "camera_rig"]`. The base editor plus
  the networking-authoring and camera-rig extensions.
- `dylib`. The SDK-backed project flow: builds the proxy dylib
  that project and extension builds link against. Off by default
  because linking it slows plain dev builds; release editor
  builds enable it.
- `runner`. Builds the prebuilt game runner used by Play.

## User config directory

Resolved via `dirs::config_dir()` joined with `jackdaw`. On
Linux that lands at `~/.config/jackdaw/`. The directory
holds:

- `recent.json`: launcher's recent-projects list. Filtered
  to existing folders at startup.
- `keybinds.json`: user-overridden keybinds. Defaults live
  in code; the file only contains overrides.
- `extensions.json`: catalog of installed extensions
  (enabled/disabled, install state).
- `extensions/`: installed extension dylibs. The editor's
  Extensions dialog installs into here; a compatible prebuilt
  `.so` / `.dylib` / `.dll` dropped here loads on the next
  editor start.
- `games/`: same idea, for game dylibs.

You can edit any of these by hand; the editor watches the
directory and reloads when files change.

## Project file

`project.jsn` (see [JSN Format](../developer-guide/jsn-format.md))
holds project-scoped editor settings:

- `default_scene`: scene to open on project load.
- `layout`: persisted dock layout, parsed as
  `jackdaw_panels::LayoutState`. Editing this by hand is
  not recommended; let the editor write it.
- `name`, `description`: free-form metadata, shown in the
  launcher.

## EditorPlugins builder

Programmatic config goes through the `EditorPlugins` plugin
group. The default form is enough for most embedders:

```rust
App::new()
    .add_plugins(EnhancedInputPlugin)
    .add_plugins(jackdaw::EditorPlugins::default())
    .run();
```

Notes:

- `EnhancedInputPlugin` must be added before `EditorPlugins`.
  We do this rather than adding it ourselves so user game
  plugins can also add it without a duplicate-plugin panic.
- `DylibLoaderPlugin` is intentionally not in the group. The
  launcher binary opts in by adding it directly.

The builder API for swapping out built-in extensions or
adding statically linked ones is documented in
[Extending the Editor](../developer-guide/extending-the-editor.md).

## Toolchain

The repo CI pins to a specific nightly in
`.github/workflows/ci.yaml` (currently `nightly-2026-03-05`,
matched against bevy_cli's `rust-toolchain.toml`). We don't
ship a `rust-toolchain.toml` yet, so your local toolchain is
whatever rustup has selected. If you see compiler errors
that match no obvious code change, check the CI pin first.
This affects building the editor only; your project's own
builds use your own toolchain, untouched.
