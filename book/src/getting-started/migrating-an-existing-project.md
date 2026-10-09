# Importing an existing project

Open a Bevy 0.19 project through the launcher's **Import Bevy Project**
action, or preview the integration from a terminal:

```bash
jd import /path/to/game
```

Import planning is side-effect free. The launcher shows an **Apply changes**
confirmation; the CLI requires:

```bash
jd import /path/to/game --apply
```

The plan verifies the Bevy minor, creates `jackdaw.toml`, creates the
gitignored `.jackdaw/` build directory, and ensures the project exposes a
library plugin. A common bin-only `App::new()` program is converted into
`GamePlugin` as part of the same preview, with the original proposed as
`src/main.rs.bak`. Unsupported source shapes receive a library stub and a
clear manual-move note.

Jackdaw never edits the lockfile, toolchain, or ordinary `target/`.

## Runtime wiring

The same preview wires in the runtime the editor talks to. Component
discovery and Play need all three pieces:

1. `jackdaw_runtime` in the game package's `[dependencies]`, with the
   `physics` and `pie` features. An existing plain declaration only gains
   `pie`. The manifest is edited in place, keeping its comments and layout.
2. `app.add_plugins(jackdaw_runtime::JackdawPlugin)` in the game plugin's
   `build`.
3. `DefaultPlugins` passed through `jackdaw_runtime::maybe_windowless` in
   `src/main.rs`, so Play can show the game inside the editor.

Each piece already present is left alone, so running the import again finds
nothing to do.

### When import leaves it to you

Import does not edit anything it cannot change safely. It lists the step as a
`note:` in the preview instead, and `jd doctor --project` reports it as
`[fail]` until it is done:

- `jackdaw_runtime` declared as an optional, renamed, or target-specific
  dependency. Import leaves the manifest and your sources alone. Make it a
  plain `[dependencies]` entry and run `jd import` again.
- No single plugin to edit: more than one `impl Plugin` for the game plugin,
  or a `build` it cannot edit.
- A `main.rs` without an `App::new()...run()` chain that adds `DefaultPlugins`
  itself.
- A project on another Bevy minor, imported with `--allow-bevy-mismatch`.

The manual steps:

```bash
cargo add jackdaw_runtime@0.19 --features physics,pie
```

```rust
// in your game plugin
fn build(&self, app: &mut App) {
    app.add_plugins(jackdaw_runtime::JackdawPlugin);
}
```

```rust
// in src/main.rs
let default_plugins = jackdaw_runtime::maybe_windowless(DefaultPlugins);
App::new().add_plugins(default_plugins)
```

If the game builds its `App` somewhere other than `src/main.rs`, pass
`DefaultPlugins` through `maybe_windowless` there; `jd doctor` then reports
embedded Play as `[warn] not checked`.

## Cargo workspaces

Point the import at the workspace root. Jackdaw resolves the member that
depends on Bevy, writes `jackdaw.toml` at the root, and records which member
it chose:

```toml
package = "my-game"
```

When several members depend on Bevy, import says so and asks which one:

```bash
jd import /path/to/workspace --package my-game --apply
```

## Version pins

Setup records the versions the project was integrated against:

```toml
[jackdaw]
version = "0.19.0"
bevy = "0.19"
```

Jackdaw compares these on open. A different Bevy minor is reported before any
build starts, because the editor and your game code must share one Bevy
version; pass `--allow-bevy-mismatch` (or **Set up anyway** in the launcher)
to integrate regardless and deal with it later.

## Upgrading a project

When Jackdaw updates within the same Bevy minor, the project still builds,
but it records the old version and still requests the old release line of the
`jackdaw_*` crates. The launcher offers to update it on open, or:

```bash
jd upgrade /path/to/game          # preview
jd upgrade /path/to/game --apply
```

That rewrites the `[jackdaw]` pins and moves any `jackdaw_*` dependency to the
matching version, leaving your run configurations, comments, features, and
every other dependency untouched. Path and git dependencies are left alone.

## Bringing asset references up to date

A project written before assets were files at paths spells a material or any
other asset by a bare name (`@grass`), keeps entries in `assets/catalog.bsn`,
carries asset files with no header naming their type, and holds terrain
sidecars at an older format version. All of that still loads. One operator
writes it out in the current spelling:

```
project.migrate_asset_references
```

It rewrites every name a scene or a prefab spells for an asset as the path of
the file that answers to it, writes each `catalog.bsn` entry out as a file of
its own and leaves the catalog empty, puts the header naming its type on every
asset file that has none, and re-encodes every terrain sidecar with its
material slots as paths. It reports what it rewrote and what it left alone: a
name two files carry, which stands for neither, and a name no file carries,
such as a material that was never saved.

The operator writes over the project's files and undo does not reach them, so
it refuses to run while anything open has unsaved edits. Running it a second
time reports that there is nothing to migrate.

## Checking a project

```bash
jd doctor --project /path/to/game
```

reports the build prerequisites, the resolved package, whether a library
target and plugin were found, the version pins, and whether the project's
type schema has been built yet.

## Expected game shape

Game systems and resources live in a plugin exported by `src/lib.rs`:

```rust
use bevy::prelude::*;

#[derive(Default)]
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        // game systems, observers, resources
    }
}
```

Keep ambient plugins such as `DefaultPlugins` and `PhysicsPlugins` in the
standalone `main.rs`. To expose authorable components, derive Bevy reflection:

```rust
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct PlayerSpawn;
```

Use **Rebuild Project** or `jd build`. Manual build is the default; **Toggle
Auto Build** opts in and persists that choice for this project. Play launches
the project's own cargo binary in a separate process.

Authored `.bsn` scenes are loaded in the game through `jackdaw_runtime`.
