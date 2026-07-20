# Migrating an existing project

If you already have a Bevy 0.19 game, you can open it in jackdaw
without restructuring it. Import is additive: jackdaw writes a
`jackdaw.toml`, a gitignored `.jackdaw/` directory, and nothing
else. Your `Cargo.toml`, `Cargo.lock`, toolchain, and source code
stay untouched, and your own `cargo build` / `cargo run` keep
compiling plain crates.io Bevy exactly as before.

## Import it

Two equivalent entry points:

- The launcher's **Import** action: pick your project folder.
- `jackdaw init` from the project root.

Import does four things:

1. **Verifies the Bevy version.** Each jackdaw release supports one
   Bevy minor (currently 0.19). If your project depends on a
   different one, import stops with a clear error telling you to
   update the project or use a matching jackdaw release. Nothing is
   written in that case.
2. **Ensures a library target.** The editor loads your project as a
   library, so your game code must be reachable from `src/lib.rs`.
   If your project is bin-only, import offers to create a
   `src/lib.rs` stub containing an empty `GamePlugin`. It never
   moves or rewrites your existing code; moving your game setup
   into the stub is up to you (see below).
3. **Detects your plugin type.** Import looks for a `pub struct
   ...Plugin` in your library and records the answer in
   `jackdaw.toml`; if detection is ambiguous it asks. Override from
   the CLI with `jackdaw init --plugin MyGamePlugin`.
4. **Writes the additive files:** `jackdaw.toml`, the `.jackdaw/`
   directory, and a `.gitignore` entry for `.jackdaw/`.

Import is idempotent. Run it again and it only adds what is
missing.

### Projects set up by the old `jackdaw init`

Earlier releases wired projects up invasively: an `editor` cargo
feature, an `editor` `[[bin]]`, `cargo editor` / `cargo play`
aliases, and profile pins. Import detects that setup and removes
it automatically, leaving the project in the current additive
shape. You don't need to undo anything by hand.

## The shape jackdaw expects

A jackdaw project is a normal Bevy crate:

```toml
[package]
name = "my-game"
edition = "2024"

[dependencies]
bevy = "0.19"
# Optional: loads the scenes you author in the editor
# (assets/*.bsn) and builds physics colliders from them.
jackdaw_runtime = { version = "0.19", features = ["physics"] }
avian3d = "0.7"
```

Jackdaw's crate versions track the Bevy minor they target, so a
project on Bevy 0.19 uses the `0.19.x` line of `jackdaw_runtime`.

- `src/lib.rs` exposes your game's plugin. The editor looks for a
  type named `GamePlugin` by default; the top-level `plugin` key
  in `jackdaw.toml` overrides that.
- An optional `[[bin]]` (or `src/main.rs`) is your standalone
  binary; `cargo run` works with or without the editor installed.
- No cargo features, no `crate-type`, no `build.rs`, no macros, no
  registration code. There is nothing jackdaw-specific in the
  manifest.

The editor owns all editor-related builds. It generates a shim
crate into `.jackdaw/` and builds your project as a dynamic
library against its own SDK, in `.jackdaw/target/`, separate from
your `target/`.

## Move gameplay into the plugin

Everything you set up inline in `main()` after `App::new()` (your
systems, observers, resources) belongs in the plugin's `build()`:

```rust
use bevy::prelude::*;

#[derive(Default)]
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        // your systems, observers, resources
    }
}
```

Ambient plugins (`DefaultPlugins`, `PhysicsPlugins`) stay in
`main.rs`: the editor's game runner adds them itself during Play,
so adding them inside `GamePlugin` triggers a "plugin already
added" panic.

## Load authored scenes in the game

`jackdaw_runtime` is an ordinary dependency that loads the `.bsn`
scenes you author in the editor. Bevy has no built-in loader for
`.bsn`; `JackdawPlugin` registers one for that extension. In
`main.rs`:

```rust
use bevy::prelude::*;
use jackdaw_runtime::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(avian3d::prelude::PhysicsPlugins::default())
        .add_plugins(jackdaw_runtime::JackdawPlugin)
        .add_plugins(my_game::GamePlugin)
        .add_systems(Startup, spawn_initial_scene)
        .run()
}

fn spawn_initial_scene(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(JackdawSceneRoot(asset_server.load("scene.bsn")));
}
```

`JackdawPlugin` spawns the entities listed in the scene file and,
with the `physics` feature, builds avian colliders from authored
`AvianCollider` components. `PhysicsPlugins` runs the simulation;
without it the colliders exist but nothing moves. If you don't use
authored scenes, you don't need `jackdaw_runtime` at all.

## Make your components editable

For each component you want to author in the editor, derive
`Reflect`:

```rust
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct PlayerSpawn;
```

That is the whole story: Bevy's `reflect_auto_register` registers
the type when the editor loads your project's library, and it
shows up in the Add Component picker. No registration code. See
[Custom Components](../developer-guide/custom-components.md).

## Open and play

Open the imported folder from the launcher. The editor starts a
background build of your project library; when it finishes, your
reflected components and resources appear in the inspector and
pickers.

After that first build, rebuilds are on request rather than
automatic, so an editor session never competes with a `cargo
build` you started yourself. Use the **Rebuild Project** action in
the editor, or run `jackdaw-cli build` from the project root; a
running editor watches `.jackdaw/schema.json` and picks up the new
types either way. **Toggle Auto Build** turns on rebuild-on-source-
change if you prefer it.

The Play button launches a prebuilt `jackdaw-runner` process with
the already-built project library. Nothing compiles at play time,
and the game runs out of process, so a crash takes down the game,
not the editor.

## Common gotchas

**Import fails with a Bevy version error.** Your project's Bevy
minor doesn't match this jackdaw release. Update the project to
the supported minor (or install the matching jackdaw release) and
import again.

**Component doesn't show in the picker.** Check that
`#[derive(Reflect)]` and `#[reflect(Component)]` are both present,
and that the project has been rebuilt since you added the type
(Rebuild Project, or `jackdaw-cli build`).

**"plugin already added" panic during Play.** Your `GamePlugin`
adds `DefaultPlugins`, `PhysicsPlugins`, or another ambient plugin
the runner already provides. Move it to `main.rs`.

**Brushes have no collision in-game.** You need the `physics`
feature on `jackdaw_runtime`, `PhysicsPlugins` added in your
`main.rs`, and an `AvianCollider` component on the brush, authored
in the editor.

**Nothing happens on Play after importing a bin-only project.**
If import created the `src/lib.rs` stub, your game code still
lives in `main.rs` and the stub's `GamePlugin` is empty. Move your
setup into the plugin so the editor can run your systems.
