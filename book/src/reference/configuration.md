# Configuration

Configuration is split across three places: `jackdaw.toml` in the
project root (plugin override and run configurations), the user
config directory (global preferences and extension install dirs),
and `.jackdaw/project.json` (per-project editor settings). A
fourth location, the SDK, is resolved rather than configured; see
[Where the SDK lives](#where-the-sdk-lives).

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

## Command line

Two binaries carry terminal commands.

`jackdaw` is the editor. With no arguments it opens the launcher.
These subcommands run headless and exit before the GUI:

- `jackdaw new <name> [--extension]`: scaffold a project into
  `<name>/` from the embedded template. Game by default,
  extension with the flag.
- `jackdaw init [--plugin <Type>]`: import the project in the
  current directory (writes `jackdaw.toml`, `.jackdaw/`, and the
  gitignore entry). Idempotent.
- `jackdaw migrate [--apply]`: lift a bin-only project's `main.rs`
  into its `GamePlugin` library. Prints the plan without
  `--apply`; with it, writes the files and keeps the original as
  `src/main.rs.bak`.
- `jackdaw doctor`: preflight report on the host environment
  (rustc, cmake, and the linker on Windows).
- `jackdaw --version`: version plus the Bevy minor it targets.

`jackdaw-cli` is the bevy-free tool. It ships in the downloadable
bundles and installs from the `jackdaw_cli` package.

- `jackdaw-cli build [--project <path>]`: build the project
  dylib and write `.jackdaw/schema.json`. A running editor
  watches that file and reloads the types from it.
- `jackdaw-cli run [--project <path>]`: the same build, then
  `cargo run` in the project. A build failure aborts before
  running.
- `jackdaw-cli setup`: build the SDK into the cache. Needs a
  binary with the `embed-recipe` feature (packaged releases).
  `build` and `run` do this on their own when the cache is cold.
- `jackdaw-cli doctor`: report whether cargo, rustup, and the
  pinned toolchain are in place.

`package-sdk --out <dir>` and `bundle --out <dir>` stage release
artifacts and are used by CI, not day to day.

## Where the SDK lives

The SDK is the proxy dylib plus the compiled closure that project
and extension builds link against. The editor resolves it in this
order, and the first hit wins:

1. `JACKDAW_SDK_DIR`, if set. Points at an installed layout: an
   `sdk/manifest.txt` with the rustc wrapper, the runner,
   `Cargo.lock`, and `toolchain.txt` beside it.
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

`jackdaw-cli doctor` reports whether the prerequisites for
building it are in place; `jackdaw-cli setup` builds it.

## Cargo features

These are features of the `jackdaw` crate itself, relevant if you
build the editor from source. Projects have no jackdaw-related
features.

- `default = ["multiplayer", "camera_rig", "dylib"]`.
- `multiplayer`. Bundles the editor-only networking authoring
  extension. The editor writes replication metadata; no lightyear
  is compiled into it.
- `camera_rig`. The authorable camera-rig components.
- `dylib`. The SDK-backed project flow: builds the proxy dylib
  that project and extension builds link against. On by default,
  because loading project code in-process requires sharing the
  SDK's type graph. `--no-default-features` gives a fast UI-only
  editor that skips it.
- `runner`. Builds the prebuilt game runner used by Play. Implies
  `dylib`.
- `embed-recipe`. Bakes the SDK-builder recipe into the binary so
  a packaged, source-free jackdaw can build its own SDK on first
  launch. Off in dev builds, on in packaged releases.

Building with `dylib` needs an explicit `--target <host-triple>`,
so the editor links the same SDK the build pipeline compiles
project dylibs against.

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
- `extensions.json`: catalog of installed extensions
  (enabled/disabled, install state).
- `extensions/`: installed extension dylibs. The editor's
  Extensions dialog installs into here; a compatible prebuilt
  `.so` / `.dylib` / `.dll` dropped here loads on the next
  editor start.
- `games/`: same idea, for game dylibs.

You can edit any of these by hand, but only the `extensions/` and
`games/` dylib directories are watched for changes. The JSON files
are read at startup, so edit them with the editor closed.

`JACKDAW_EXTENSIONS_DIR` and `JACKDAW_GAMES_DIR` add one extra
directory each to the same search and watch.

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

The repo ships a `rust-toolchain.toml` pinning
`nightly-2026-03-05`, and CI uses the same channel. The SDK is
pinned to that exact rustc: project builds and the SDK have to
share one compiler for the shared type graph to line up, so
setup installs it through rustup rather than using whatever is
selected.

This affects the editor and the code it builds for the editor.
Your project's own `cargo build` and `cargo run` use your own
toolchain, untouched.
