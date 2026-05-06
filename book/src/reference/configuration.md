# Configuration

Configuration is split across three places: the workspace
`Cargo.toml` (feature flags), the user config directory
(global preferences and dylib install dirs), and
`project.jsn` (per-project settings).

## Cargo features

The top-level `jackdaw` crate exposes:

- `default = []`. The base editor.
- `dylib`. Experimental dynamic-linking flow for
  hot-loadable extensions. Pulls in `jackdaw_sdk` plus
  Bevy's `dynamic_linking`. Off by default; see
  [Extending the Editor](../developer-guide/extending-the-editor.md)
  for what it gates.
- `hot-reload`. Adds `bevy_simple_subsecond_system` for
  iterating on the editor itself. Editor-developer convenience,
  not for shipping games.

Templates ship with their own feature set. The `game-static`
template's `Cargo.toml` declares an `editor` feature that
guards the editor binary, so a release build can drop it
out:

```toml
[features]
default = []
editor = ["dep:jackdaw"]
```

`cargo run` builds the standalone game; `cargo editor` (or
`cargo run --bin editor --features editor`) builds the
editor host.

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
- `extensions/`: drop a built `.so` / `.dylib` / `.dll`
  here to load it on the next editor start (only when the
  launcher binary is running with `DylibLoaderPlugin`).
- `games/`: same idea, for game extensions.

You can edit any of these by hand; the editor watches the
directory and reloads when files change.

## Project file

`project.jsn` (see [JSN Format](../developer-guide/jsn-format.md))
holds project-scoped settings:

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
  launcher binary opts in by adding it directly. Per-project
  static editor binaries should not add it; the dylibs in
  `~/.config/jackdaw/` were built against a different bevy
  compilation and panic at the FFI boundary if loaded into a
  static editor.

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
