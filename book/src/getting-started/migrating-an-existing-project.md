# Migrating an existing project

If you already have a Bevy 0.18 game, you can wire jackdaw in
without starting from scratch. The diff is small, but there are
a few gotchas. This page walks through it.

The end state matches what `cargo generate` produces from the
`game-static` template:

- `src/lib.rs` holds your `MyGamePlugin`.
- `src/main.rs` is a thin standalone runner.
- `src/bin/editor.rs` is the editor + game binary.
- Scene data lives in `assets/scene.jsn`.

You can read the templates directly at
[`templates/game-static/`](https://github.com/jbuehler23/jackdaw/tree/main/templates/game-static)
to compare against your project as you go.

## 1. Bump bevy to 0.18

If your project is on an older bevy, bump it first and get
`cargo run` working again. Jackdaw doesn't have a story for
older versions.

## 2. Cargo.toml deltas

Add these lines:

```toml
[features]
default = []
editor = ["dep:jackdaw"]

[dependencies]
bevy = { version = "0.18", features = ["file_watcher"] }
jackdaw = { version = "0.4", default-features = false, optional = true }
jackdaw_runtime = "0.4"
ctrlc = "3"

[[bin]]
name = "editor"
required-features = ["editor"]
```

Notes:

- `bevy/file_watcher` is what powers hot-reload of
  `assets/scene.jsn` in the standalone runner.
- `jackdaw` is optional and gated behind `editor`. Without that
  feature your standalone game has no editor deps.
- `jackdaw_runtime` is the small runtime-only crate that loads
  scenes from `.jsn`. Always present.
- `ctrlc` claims SIGINT and SIGTERM before wgpu and gilrs swallow
  them. Without it, Ctrl+C in your terminal won't kill the game.

If `0.4` isn't on crates.io yet (it isn't at the time of writing),
patch to a local checkout:

```toml
[patch.crates-io]
jackdaw = { path = "/path/to/jackdaw" }
jackdaw_runtime = { path = "/path/to/jackdaw/crates/jackdaw_runtime" }
```

## 3. Move gameplay into a plugin

If everything currently lives in `main.rs`, that won't fly. The
editor binary needs to add its own plugins on top of yours, so
your gameplay has to be reachable as a `Plugin`.

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

Anything you used to write inline in `main()` after
`App::new()` moves into `build()`. With one important
exception, see step 5.

## 4. Standalone main.rs

Replace your `main.rs` with something close to this:

```rust
use bevy::prelude::*;
use jackdaw_runtime::prelude::*;

fn main() -> AppExit {
    let _ = ctrlc::set_handler(|| std::process::exit(130));

    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(JackdawPlugin)
        .add_plugins(your_crate::MyGamePlugin)
        .add_systems(Startup, spawn_initial_scene)
        .run()
}

fn spawn_initial_scene(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(JackdawSceneRoot(asset_server.load("scene.jsn")));
}
```

`JackdawPlugin` is what spawns the entities listed in
`assets/scene.jsn`. Without it your runtime has no scene.

## 5. Editor binary

Create `src/bin/editor.rs`:

```rust
use bevy::prelude::*;
use jackdaw::prelude::*;
use jackdaw::project_select::PendingAutoOpen;
use std::path::PathBuf;

fn main() -> AppExit {
    let _ = ctrlc::set_handler(|| std::process::exit(130));

    let project = std::env::var_os("JACKDAW_PROJECT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());

    let mut app = App::new();
    app.add_plugins(DefaultPlugins)
        .add_plugins((PhysicsPlugins::default(), EnhancedInputPlugin))
        .add_plugins(EditorPlugins::default())
        .add_plugins(your_crate::MyGamePlugin);

    if let Some(root) = project.filter(|p| p.is_dir()) {
        app.insert_resource(PendingAutoOpen { path: root, skip_build: true });
    }

    app.run()
}
```

The important bit is **adding `PhysicsPlugins` and
`EnhancedInputPlugin` here, not in `MyGamePlugin`**. Both the
editor and your game need them, so if `MyGamePlugin` adds them
too you'll get a "plugin already added" panic.

Same applies to any other ambient plugin (`AhoyPlugins`, your
own UI plugins, etc): they go next to `DefaultPlugins`, not in
`MyGamePlugin`.

## 6. Move authored data into the scene

If your existing game spawns entities in code (lights, cameras,
level geometry), pick the ones that should be authorable in the
editor and move them out. They'll live in `assets/scene.jsn`
instead.

For each component you want to author in the editor, derive
`Reflect`:

```rust
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct PlayerSpawn;
```

That's all you need. The component shows up in the editor's
Add Component picker. See [Custom Components](../developer-guide/custom-components.md)
for the full story.

## 7. Try it

```bash
cargo run        # standalone
cargo editor     # editor + game
```

The editor opens, picks up your project, and you can author
scene data. The standalone reads the same `scene.jsn`.

## Common gotchas

**"plugin already added" panic on cargo editor.** Either
`MyGamePlugin` is adding `DefaultPlugins`, `PhysicsPlugins`,
`EnhancedInputPlugin`, or some other plugin the editor already
added. Move it to `main.rs` / `editor.rs`.

**Component doesn't show in the picker.** Probably missing
`#[derive(Reflect)]` or `#[reflect(Component)]`. If both are
present and it still doesn't show, check if it has
`@EditorHidden` somewhere (it shouldn't, for your own types).

**Scene loads but observer queries return wrong values.** If
you have an `On<Insert, T>` observer that reads
`GlobalTransform`, it should work. The scene loader runs
transform propagation inline before user component inserts.
If you see weird positions, file a bug.

**Standalone game crashes on scene load.** Most likely your
`Cargo.toml` has `panic = "abort"` and a reflected component
in your scene file no longer matches its current type
definition. The deserialize step returns errors cleanly, but a
genuinely panicking insert will kill the process. Fix the
schema drift, don't try to swallow the panic.
