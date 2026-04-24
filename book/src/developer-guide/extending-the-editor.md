# Extending the Editor

Jackdaw extensions are plain Rust crates that you write using
bevy-native APIs. Scaffolding, building, and installing are all
driven from inside the editor — no custom build scripts, no cargo
gymnastics, no hash-matching games.

## Prerequisite: install Bevy CLI

Templates are distributed via [Bevy CLI](https://github.com/TheBevyFlock/bevy_cli),
so install that once:

```sh
cargo install --locked --git https://github.com/TheBevyFlock/bevy_cli bevy_cli
```

Jackdaw shells out to `bevy new` when you create a new project, so
this needs to be on your `PATH`.

## Author workflow

### 1. Launch the editor

```sh
cargo run --features dylib
```

The launcher (project picker) opens. On first run, the Recent
Projects list is empty.

### 2. Create an extension

Click **+ New Extension** on the launcher. Fill in:

- **Name** — the crate name for your extension (e.g. `my_tool`).
- **Location** — parent directory the project will be created
  under. `Browse…` opens a folder picker.

Click **Create**. Jackdaw invokes
`bevy new -t https://github.com/jbuehler23/jackdaw_template_extension --yes <name>`
in the chosen location, then opens the newly-scaffolded project.

### 3. Edit and build

Edit `my_tool/src/lib.rs` in your preferred editor. Then, inside
jackdaw: **File → Extensions → Build from project folder…**, pick
`my_tool/`. Jackdaw runs `cargo rustc` with the right `--extern`
flags and live-loads the resulting dylib. Windows, operators, and
menu entries activate immediately.

Iterate: edit code, click **Build from project folder…** again,
see the changes.

### Creating a game

Click **+ New Game** on the launcher instead. Same scaffold flow
with the
[`jackdaw_template_game`](https://github.com/jbuehler23/jackdaw_template_game)
template. Until Play-in-Editor (PIE) lands, games load as
extensions so you can start prototyping against the editor.

## How it works

Jackdaw ships a tiny proxy crate (`jackdaw_sdk`) whose dylib
carries the one compiled copy of bevy + jackdaw types that both
the editor and every extension link against. When jackdaw builds
an extension it invokes `cargo rustc` with explicit
`--extern bevy=libjackdaw_sdk.so` and
`--extern jackdaw_api=libjackdaw_sdk.so` flags. Your extension's
code writes plain `use bevy::prelude::*;` and
`use jackdaw_api::prelude::*;` — those names resolve at compile
time through the SDK.

Scaffolded projects therefore have an *empty* `[dependencies]`
table:

```toml
[package]
name = "my_tool"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]  # intentionally empty
```

### BEI keybind caveat

Live-load activates windows, menu entries, operators, and panel
sections immediately. BEI input contexts are the exception:
`add_input_context::<C>()` needs `&mut App`, which only exists at
startup. Keybinds declared via BEI don't bind until the editor
restarts. Extensions that don't use BEI keybinds don't need a
restart.

## Escape hatches

### Install a prebuilt dylib

If you already have a compatible `.so` / `.dylib` / `.dll`
(a teammate's build, a CI artefact), use **File → Extensions →
Install prebuilt dylib…** and pick the file. The editor copies it
into the extension directory and live-loads.

### Statically link an extension

For in-house tools bundled into a custom editor binary, skip the
dylib path entirely:

```rust
// your_editor/src/main.rs
fn main() {
    App::new()
        .add_plugins(
            jackdaw::EditorPlugins::default()
                .set(ExtensionPlugin::new().with_extension::<MyExtension>())
        )
        .run();
}
```

Nothing crosses a dylib boundary; everything is normal static
linking. Use for tools you control and ship together with the
editor.

## Troubleshooting

- *bevy CLI not found* — install it (`cargo install --locked
  --git https://github.com/TheBevyFlock/bevy_cli bevy_cli`) and
  make sure `bevy` is on your `PATH`.
- *SDK dylib not found* — rebuild the editor with
  `cargo run --features dylib`. Without that feature, jackdaw
  doesn't produce `libjackdaw_sdk.so`.
- *cargo exited with non-zero status* during Build — your
  extension has a compile error. The status line shows cargo's
  stderr.
- *picked path has no Cargo.toml* — point at the project root,
  not the `src/` directory.

## In-tree examples

The repo has two rich examples statically linked into the
prebuilt binary (behind the `dev` feature):

- `examples/sample_extension/` — operators with keybinds,
  availability checks, a dock window, and menu entries. Good
  reference for what the API supports.
- `examples/viewable_camera_extension/` — shows off heavier scene
  manipulation through `ExtensionContext::world()`.

Both use the static-linking path, so they skip the build-button
step. They're there to exercise the API surface; day-to-day
authoring should use the `+ New Extension` / `+ New Game`
workflow described above.
