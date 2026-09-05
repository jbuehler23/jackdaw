# Configuration

Configuration is split across three places: `jackdaw.toml` in the
project root (package selection and run configurations), the user
config directory (global preferences and extension install dirs),
and `.jackdaw/project.json` (per-project editor settings). A
fourth location, the SDK, is resolved rather than configured; see
[Where the SDK lives](#where-the-sdk-lives).

## jackdaw.toml

The one jackdaw-specific file in a project. Everything in it has a
working default; a project with an empty (or missing) file still
opens and plays.

```toml
# In a cargo workspace, the member jackdaw builds as the game.
# package = "my-game"

[[run]]
name = "Play"
# instances = 2
# env = { SERVER_ADDR = "127.0.0.1:5000" }
# args = []
# cwd = "some/subdir"
```

Top-level keys:

- `package`: which workspace member is the game. Single-package
  projects omit this.
- `plugin`: optional name of the project's root Bevy `Plugin`
  type. Recorded by import/setup and checked by `jd doctor`;
  Play launches your cargo binary, which must add the plugin
  itself in `main.rs`.

Each `[[run]]` entry is one item in the Play dropdown. Every run
launches the same already-built game binary; entries differ only
in launch environment, never in what gets built. Fields:

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

`.jackdaw/` is the editor's per-project scratch space: persisted
editor settings (`project.json`), the extracted type schema from
the game binary, and for extension projects the generated shim
crate and SDK-linked build target. It is gitignored (the scaffold
and import both add the entry), owned entirely by the editor, and
safe to delete; the next project open rebuilds what it needs. The
editor never touches the project's own `Cargo.toml`, `Cargo.lock`,
`target/`, or toolchain.

## Command line

`jackdaw` is exclusively the GUI. `jd` is the sole public command:

- `jd new <name> [--extension]`
- `jd import [path] [--plugin <Type>] [--apply]`
- `jd open [path]`
- `jd build [--project <path>]`
- `jd run [--project <path>]`
- `jd setup`
- `jd doctor`
- `jd extension <keygen|pack|install|verify|list|enable|disable|uninstall>`

Import previews by default and performs no writes without `--apply`.
Release-only `package-sdk` and `bundle` operations live under `cargo xtask`.

## Where the SDK lives

The SDK is the proxy dylib plus the compiled closure that
extension builds link against. The editor resolves it in this
order, and the first hit wins:

1. `JACKDAW_SDK_DIR`, if set. Usually an installed layout: an
   `sdk/manifest.txt` with the rustc wrapper, `Cargo.lock`, and
   `toolchain.txt` beside it. A bootstrap cache directory or a
   jackdaw checkout is accepted too, so pointing it at any of the
   three works.
2. A dev checkout's own `target/<triple>/`, when the SDK there is
   built. An in-tree SDK beats any cache, because a debug editor
   and a release cache are not link-compatible.
3. The same installed layout next to the running executable. This
   is what a downloaded bundle uses, with no env var.
4. The bootstrap cache at `~/.jackdaw/sdk/<version>-<toolchain>/`
   (or under `$XDG_DATA_HOME` when that is set to an absolute
   path). Written by first-run setup and keyed by jackdaw version
   and toolchain, so an upgrade lands in a fresh directory and the
   old one is reclaimed.

`jd doctor` reports which of these won, whether the prerequisites for
building it are in place, and whether the resolved SDK is actually
usable; `jd setup` builds it.

A missing library or rustc wrapper stops a build before it compiles
anything, rather than minutes in, and names what to do about it:

```
[fail] SDK: explicit JACKDAW_SDK_DIR at /opt/empty/sdk/.../libjackdaw_sdk.so is not usable
       no SDK library at /opt/empty/sdk/x86_64-unknown-linux-gnu/libjackdaw_sdk.so
       no rustc wrapper at /opt/empty/jackdaw-rustc-wrapper
       fix: unset JACKDAW_SDK_DIR to use the SDK this jackdaw found for itself
```

## Cargo features

These are features of the `jackdaw` crate itself, relevant if you
build the editor from source. Projects have no jackdaw-related
features.

- `default = ["multiplayer", "camera_rig", "embed-recipe"]`.
- `multiplayer`. Bundles the editor-only networking authoring
  extension. The editor writes replication metadata; no lightyear
  is compiled into it.
- `camera_rig`. The authorable camera-rig components.
- `dylib`. The SDK-backed extension flow: builds the proxy dylib
  that extension builds link against. On by default in
  precompiled releases because loading native extensions
  in-process requires sharing the SDK's type graph.
  Source builds opt in explicitly with `--features dylib`.
- `embed-recipe`. Bakes the SDK-builder recipe into the binary so
  a packaged, source-free jackdaw can build its own SDK on first
  launch. On by default for self-contained Cargo installs.

Building with `dylib` needs an explicit `--target <host-triple>`,
so the editor links the same SDK the build pipeline compiles
extension dylibs against.

## User config directory

Resolved via `dirs::config_dir()` joined with `jackdaw`. On
Linux that lands at `~/.config/jackdaw/`. The directory
holds:

- `recent.json`: launcher's recent-projects list. Filtered
  to existing folders at startup.
- `keybinds.json`: user-overridden keybinds. Defaults live
  in code; the file only contains overrides.
- `keymap_preset.json`: the selected keymap preset.
- `last_new_project_location`: the folder the New Project
  dialog opens in.
- `extensions.json`: desired enabled/disabled state.
- `trusted_publishers.json`: publisher keys accepted through the native-code
  trust prompt.

Signed `.jdext` payloads live in the platform data directory under
`jackdaw/extensions/<id>/<version>/`. `active.json` selects one version and
`garbage.json` queues retired mappings for deletion on the next launch.
Loose dylib search directories and their environment variables are unsupported.

## Project file

`.jackdaw/project.json` (see [BSN Format](../developer-guide/bsn-format.md))
holds project-scoped editor settings:

- `last_open_tabs`: scene paths, relative to the project root,
  restored in order on the next open. `last_active_tab` indexes
  into it and is clamped on load.
- `layout`: persisted dock layout, parsed as
  `jackdaw_panels::LayoutState`. Editing this by hand is
  not recommended; let the editor write it.
- `name`, `description`: free-form metadata, shown in the
  launcher.
- `default_scene`: reserved. The field is read and written, but
  nothing currently opens a scene from it; tab restore plus the
  `assets/scene.bsn` fallback decide what opens.

## Custom editor composition

Programmatic configuration goes through `jackdaw_editor` and its
`JackdawEditorPlugins` plugin group:

```rust
App::new()
    .add_plugins(EnhancedInputPlugin)
    .add_plugins(jackdaw_editor::JackdawEditorPlugins::default())
    .run();
```

Notes:

- `EnhancedInputPlugin` must be added before `JackdawEditorPlugins`.
- `DylibLoaderPlugin` is intentionally not in the group. The
  official GUI opts into marketplace loading separately.

The builder API for swapping out built-in extensions or
adding statically linked ones is documented in
[Extending the Editor](../developer-guide/extending-the-editor.md).

## Toolchain

The repo ships a `rust-toolchain.toml` pinning
`nightly-2026-03-05`, and CI uses the same channel. The SDK is
pinned to that exact rustc: extension builds and the SDK have to
share one compiler for the shared type graph to line up, so
setup installs it through rustup rather than using whatever is
selected.

This affects the editor and the extensions it builds in-process.
Your game's own `cargo build` and `cargo run` use your own
toolchain, untouched.
