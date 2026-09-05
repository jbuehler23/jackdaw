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

Jackdaw never edits the project's Cargo manifest, lockfile, toolchain, or
ordinary `target/`. `cargo run` therefore behaves exactly as it did before.
For the same reason, migrated code never references a crate the project does
not already depend on: add `jackdaw_runtime` yourself to load authored `.bsn`
scenes in the game.

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
