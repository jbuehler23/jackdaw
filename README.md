<p align="center">
  <img src="https://raw.githubusercontent.com/jbuehler23/jackdaw/main/assets/logo/jackdaw_icon.png" alt="Jackdaw" width="200" />
</p>

# Jackdaw

[![License](https://img.shields.io/badge/license-MIT%2FApache-blue.svg)](https://github.com/jbuehler23/jackdaw#license)
[![Crates.io](https://img.shields.io/crates/v/jackdaw.svg)](https://crates.io/crates/jackdaw)
[![Downloads](https://img.shields.io/crates/d/jackdaw.svg)](https://crates.io/crates/jackdaw)
[![Docs](https://docs.rs/jackdaw/badge.svg)](https://docs.rs/jackdaw/latest/jackdaw/)
[![Discord](https://img.shields.io/discord/1486394042563428388.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2)](https://discord.gg/sDUPhWtGSM)

A 3D editor built for and with [Bevy](https://bevyengine.org/).
Very early in dev, expect bugs and changes!

We have also recently refactored our UX/UI to be _very_ similar to the official Bevy Editor Figma design, to keep things consistent. [Link here](https://www.figma.com/design/fkYfFPSBgnGkhbQd3HOMsL/Bevy-Editor?node-id=90-2)

From bevy_editor_prototypes repo:

[Bevy Editor Vision](https://bevyengine.github.io/bevy_editor_prototypes/vision.html)

[Bevy Editor Architecture](https://bevyengine.github.io/bevy_editor_prototypes/architecture.html)

[Bevy Editor Roadmap](https://bevyengine.github.io/bevy_editor_prototypes/roadmap.html)

<img width="1899" height="1014" alt="image" src="https://github.com/user-attachments/assets/3a6611b3-0974-42dc-af78-a6087c222c4d" />

https://github.com/user-attachments/assets/c8f1dc66-ef32-44c6-837b-35b7eeb01e41

https://github.com/user-attachments/assets/779af6e1-bd34-49f4-a3b4-ccd474ea2f76

https://github.com/user-attachments/assets/1e2d7cfe-601c-4af7-8dd6-a9cddc4a3c6f

https://github.com/user-attachments/assets/56834720-599e-4461-b712-fff7b85fb128

## Features

- **Brush-based geometry** draw, edit, and CSG-combine concave brushes with vertex/edge/face editing modes
- **Material system** VERY wip - texture browser, material definitions with ORM auto-detection, per-face application
- **Terrain** heightmap sculpting and texture painting, very WIP :)
- **Scene serialization** save/load scenes in the `.bsn` format with full asset references. Older `.jsn` scenes are migrated on open.
- **Transform tools** translate, rotate, scale with grid snapping and axis constraints
- **Undo/redo** full command history - some bugs atm with this
- **Extensible** register custom components, add inspector panels, integrate with your game

## Usage

Install cmake, via package manager, or VisualStudio on Windows

Download a signed release bundle, or install all required executables from
source:

```sh
cargo install --git https://github.com/jbuehler23/jackdaw jackdaw --locked
```

Take the release bundle if you can. It ships the prebuilt SDK, so you can
create a project straight away; `cargo install` builds that SDK first,
which is about half an hour, once per Jackdaw version. Either way your
project's own first build is around nine minutes, because it compiles Bevy
like any other Bevy project. Rebuilds after that are 1 to 4 seconds.

This installs `jackdaw`, `jd`, and
`jackdaw-rustc-wrapper`. Open Jackdaw and use **New Game** or **Import Bevy
Project**; the import preview shows every proposed change before applying it.
The same flows are available from the terminal:

```sh
jd new my-game && jd open my-game   # create
jd import /path/to/game             # preview integration for an existing game
jd import /path/to/game --apply
jd doctor --project /path/to/game   # why isn't this working?
jd upgrade /path/to/game --apply    # after a Jackdaw update
```

Import never edits your Cargo manifest, lockfile, toolchain, or `target/`, so
`cargo run` keeps behaving exactly as it did.
<img width="943" height="1018" alt="image" src="https://github.com/user-attachments/assets/3bda18cc-9cad-4d2c-b976-ca2e6e454314" />

Jackdaw is a standalone editor. Game applications depend only on
`jackdaw_runtime`. To build a custom standalone editor, depend on
`jackdaw_editor`:

```rust
use bevy::prelude::*;
use jackdaw_editor::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins((
            DefaultPlugins.set(editor_window_plugin()),
            EnhancedInputPlugin,
            PhysicsPlugins::default(),
            JackdawEditorPlugins::default(),
        ))
        .run()
}
```

Runtime extensions use the focused `jackdaw_extension` crate and install as
signed `.jdext` bundles. In precompiled/shared-SDK builds, install, update,
disable, and uninstall take effect without restarting; superseded native
mappings are reclaimed when Jackdaw exits.

To load a scene you authored into your own game, depend on `jackdaw_runtime`
and spawn a `JackdawSceneRoot`:

```rust
use bevy::prelude::*;
use jackdaw_runtime::prelude::*;

fn spawn_scene(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(JackdawSceneRoot(asset_server.load("scene.bsn")));
}
```

See the [examples](examples/) for more advanced usage.

## Keyboard Shortcuts

### Navigation

| Key        | Action               |
| ---------- | -------------------- |
| RMB + Drag | Look around          |
| WASD       | Move                 |
| Q / E      | Move up / down       |
| Shift      | Double speed         |
| Scroll     | Dolly forward / back |
| F          | Focus selected       |

### Editing

| Key                   | Action                                  |
| --------------------- | --------------------------------------- |
| Esc / R / T           | Translate / Rotate / Scale mode         |
| B / C                 | Draw brush (add / cut)                  |
| 1-4                   | Brush edit: Vertex / Edge / Face / Clip |
| Ctrl+D                | Duplicate                               |
| Delete                | Delete selected                         |
| Ctrl+Z / Ctrl+Shift+Z | Undo / Redo                             |
| Ctrl+S                | Save scene                              |

For the full shortcuts reference, see the [book](https://jbuehler23.github.io/jackdaw/user-guide/keyboard-shortcuts.html).

## Documentation

- [Book](https://jbuehler23.github.io/jackdaw/) - user guide, developer guide, and reference
- [Examples](examples/) - runnable examples
- [API Docs](https://docs.rs/jackdaw) - rustdoc

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and PR guidelines.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
