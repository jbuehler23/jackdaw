# Migrating an existing project

If you already have a Bevy 0.18 game, you can wire jackdaw in
without starting from scratch. The fastest path is `jackdaw init`,
which scaffolds everything below for you. The manual steps are
documented too, so you know what it does and can adjust.

The end state matches what `cargo generate` produces from the
`game-static` template:

- `src/lib.rs` holds your `MyGamePlugin` and your components.
- `src/main.rs` is a thin standalone runner.
- `src/bin/editor.rs` is the editor + game binary (one line).
- Scene data lives in `assets/scene.jsn`.

## Quick path: `jackdaw init`

From your project root:

```bash
jackdaw init
```

This is idempotent and only adds what is missing:

- the `jackdaw`, `jackdaw_runtime` (with the `physics` feature),
  and `avian3d` dependencies;
- an `editor` cargo feature and an `editor` `[[bin]]`;
- `src/bin/editor.rs` built on `jackdaw::editor_main`, linking the
  first `pub struct ...Plugin` it finds in `src/lib.rs` (override
  with `jackdaw init --plugin my_game::MyGamePlugin`);
- a starter `jackdaw.toml`;
- the `cargo editor` and `cargo play` aliases.

The one thing it cannot do for you is create a library target. The
editor links your crate to discover its components, so your shared
game code must live in `src/lib.rs` (see step 3). If it does not,
`jackdaw init` stops and tells you.

Then `cargo editor` (or open the project from the launcher).

The rest of this page covers the same setup done by hand.

## 1. Bump bevy to 0.18

If your project is on an older bevy, bump it first and get
`cargo run` working again. Jackdaw doesn't have a story for older
versions.

## 2. Cargo.toml deltas

```toml
[features]
default = []
# `jackdaw_runtime/pie` matches the feature set the Play build uses,
# so the editor build and the game build share one cargo cache.
editor = ["dep:jackdaw", "jackdaw_runtime/pie"]
# The editor's Play button enables this on the game build; it gates the
# `maybe_windowless` wrap in main.rs so the game runs inside the editor's
# Game panel instead of its own OS window.
pie = ["jackdaw_runtime/pie"]

[dependencies]
bevy = { version = "0.18", features = ["file_watcher"] }
jackdaw = { version = "0.5", default-features = false, optional = true }
# `physics` builds avian colliders from brushes you author with an
# AvianCollider, so the level collides in the standalone game and in
# PIE, not only in the editor preview.
jackdaw_runtime = { version = "0.5", features = ["physics"] }
avian3d = "0.6"
ctrlc = "3"

[[bin]]
name = "editor"
required-features = ["editor"]
```

Notes:

- `bevy/file_watcher` powers hot-reload of `assets/scene.jsn` in
  the standalone runner.
- `jackdaw` is optional and gated behind `editor`. Without that
  feature your standalone game has no editor deps.
- `jackdaw_runtime` is the small runtime-only crate that loads
  scenes from `.jsn`. Always present.

If `0.5` isn't on crates.io yet, patch to a local checkout:

```toml
[patch.crates-io]
jackdaw = { path = "/path/to/jackdaw" }
jackdaw_runtime = { path = "/path/to/jackdaw/crates/jackdaw_runtime" }
```

## 3. Move gameplay into a plugin

The editor binary adds its own plugins on top of yours, so your
gameplay has to be reachable as a `Plugin` in a library target.

In `src/lib.rs`:

```rust
use bevy::prelude::*;
use jackdaw_runtime::prelude::*;

#[derive(Default)]
pub struct MyGamePlugin;

impl Plugin for MyGamePlugin {
    fn build(&self, app: &mut App) {
        // your systems, observers, resources
    }
}
```

Anything you write inline in `main()` after `App::new()`
moves into `build()`, with one exception, see step 5.

## 4. Standalone main.rs

```rust
use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_runtime::prelude::*;

fn main() -> AppExit {
    let _ = ctrlc::set_handler(|| std::process::exit(130));

    let default_plugins = DefaultPlugins;
    // The editor's Play build sets the `pie` feature; `maybe_windowless`
    // then drops the OS window and streams the game into the editor's Game
    // panel. A standalone `cargo run` (no `pie`) opens a normal window.
    #[cfg(feature = "pie")]
    let default_plugins = jackdaw_runtime::maybe_windowless(default_plugins);

    App::new()
        .add_plugins(default_plugins)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(JackdawPlugin)
        .add_plugins(your_crate::MyGamePlugin)
        .add_systems(Startup, spawn_initial_scene)
        .run()
}

fn spawn_initial_scene(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(JackdawSceneRoot(asset_server.load("scene.jsn")));
}
```

`JackdawPlugin` spawns the entities listed in `assets/scene.jsn`
and, with the `physics` feature, builds avian colliders from
authored `AvianCollider` components. `PhysicsPlugins` runs the
simulation; without it the colliders exist but nothing moves. The
`maybe_windowless` wrap is what makes the game run inside the editor's
Game panel during Play instead of opening its own window.

## 5. Editor binary

`src/bin/editor.rs` is a single call:

```rust
use bevy::prelude::*;

fn main() -> AppExit {
    jackdaw::editor_main(your_crate::MyGamePlugin)
}
```

`editor_main` wires `DefaultPlugins` (with your project's asset
root and the tiling image sampler), the ambient `PhysicsPlugins`
and `EnhancedInputPlugin`, the editor itself, and auto-opens the
project. Passing `MyGamePlugin` is what links your crate's
reflected components into the inspector.

Ambient plugins go in `main.rs` / `editor_main`, never in
`MyGamePlugin`: both binaries need them, so adding them in
`MyGamePlugin` too triggers a "plugin already added" panic.

## 6. Move authored data into the scene

If your existing game spawns entities in code (lights, cameras,
level geometry), pick the ones that should be authorable in the
editor and move them out. They live in `assets/scene.jsn` instead.

For each component you want to author in the editor, derive
`Reflect`:

```rust
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct PlayerSpawn;
```

The component shows up in the Add Component picker, with one
caveat below. See
[Custom Components](../developer-guide/custom-components.md) for
the full story.

## 7. Try it

```bash
cargo run        # standalone
cargo editor     # editor + game
```

## Common gotchas

**"plugin already added" panic on cargo editor.** Either
`MyGamePlugin` is adding `DefaultPlugins`, `PhysicsPlugins`,
`EnhancedInputPlugin`, or another plugin the editor already added.
Move it to `main.rs` / `editor.rs`.

**Component doesn't show in the picker.** First check
`#[derive(Reflect)]` and `#[reflect(Component)]` are both present.
If they are and it still doesn't show, the cause is almost always
that nothing the editor binary links *references* the type, so the
linker stripped its auto-registration. Bevy 0.18's
`reflect_auto_register` only sees types in crates that are both
linked and referenced; `editor_main(MyGamePlugin)` provides that
reference. If a component lives in a crate `MyGamePlugin` never
touches, register it explicitly with `app.register_type::<T>()`.

**Brushes have no collision in-game.** You need the `physics`
feature on `jackdaw_runtime` (step 2), `PhysicsPlugins` added in
your `main.rs` (step 4), and an `AvianCollider` component on the
brush, authored in the editor.

**Standalone game crashes on scene load.** Most likely your
`Cargo.toml` has `panic = "abort"` and a reflected component in
your scene file no longer matches its current type definition. Fix
the schema drift; don't try to swallow the panic.
