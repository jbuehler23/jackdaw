# Custom components

Anything you can `#[derive(Reflect)]` can show up in the
editor's Add Component picker. There's no separate
registration step and no jackdaw-specific macro.

## Minimum

```rust
use bevy::prelude::*;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct PlayerSpawn;
```

That's it. Compile, restart the editor, open the inspector
on an entity, click `+ Add Component`, type `PlayerSpawn`. It
shows up.

A few things make this work without ceremony:

- Bevy 0.18's `reflect_auto_register` finds and registers the
  type at app build, so you don't need
  `app.register_type::<PlayerSpawn>()`.
- The `reflect_documentation` cargo feature is on
  workspace-wide, so doc comments on the type become picker
  tooltips.
- Jackdaw can construct a default-valued instance from
  primitive field defaults, so you don't strictly need
  `Default`. Adding it is just nicer.

## Categories and tooltip overrides

```rust
use jackdaw_runtime::prelude::*;

/// Spawns the player at this entity's world transform.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorCategory::new("Actor"))]
pub struct PlayerSpawn;
```

The picker groups `PlayerSpawn` under "Actor". The doc comment
above the struct becomes the tooltip. If you want a tooltip
that's different from the doc comment (for example, the doc
comment is for rustdoc readers and the tooltip is for level
designers), use `@EditorDescription`:

```rust
#[reflect(
    Component,
    Default,
    @EditorCategory::new("Actor"),
    @EditorDescription::new("Where the player respawns."),
)]
pub struct PlayerSpawn;
```

## Hiding a component from the picker

Sometimes a component is part of your plugin's internal
plumbing and shouldn't be authorable from the inspector.
`@EditorHidden` on the type drops it from the picker but
keeps the type registered for serialization:

```rust
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorHidden)]
pub struct PlayerInternalState {
    pub spawn_count: u32,
}
```

`EditorHidden` does double duty: as a reflect attribute on a
type (hides from picker), and as a Bevy `Component` on an
entity (hides the entity from the outliner). Same name, two
roles.

## Reacting to scene-loaded components

Use a normal `On<Insert, T>` observer:

```rust
fn spawn_player(
    trigger: On<Insert, PlayerSpawn>,
    transforms: Query<&GlobalTransform>,
    mut commands: Commands,
) {
    let Ok(gt) = transforms.get(trigger.entity) else { return };
    commands.spawn((
        ChildOf(trigger.entity),
        // ... your player rig at gt's world position
    ));
}
```

`GlobalTransform` is correct here, even when the entity is
loading from `.jsn`. The scene loader propagates transforms
inline before firing observers, so you get the entity's true
world-space pose. You don't need `On<SceneInstanceReady>` or
the recursive-walk pattern from vanilla Bevy.

Register the observer in your plugin:

```rust
impl Plugin for MyGamePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(spawn_player);
    }
}
```

## Editor-only visuals

Sometimes you want a visual indicator at a spawn point that's
visible while authoring but absent from the shipped game.
`EditorOnly` is the marker:

```rust
fn spawn_player(
    trigger: On<Insert, PlayerSpawn>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        ChildOf(trigger.entity),
        EditorOnly,
        Transform::default(),
        Mesh3d(meshes.add(Cuboid::new(0.4, 0.4, 0.4))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.2, 0.2),
            unlit: true,
            ..default()
        })),
    ));
}
```

The red cube renders in the editor. When the user saves, the
cube is skipped from `assets/scene.jsn`. The shipped game
never sees it.

You can also do this entirely in the editor without code: make
a brush, set it as a child of an empty that holds your
component, then add `EditorOnly` to the brush from the
inspector. The empty + your component ships, the brush
doesn't.

`EditorOnly` skips the whole entity from save, so don't put it
on the same entity as your gameplay marker. The pattern is
always parent (gameplay component) plus child (editor visual
with `EditorOnly`).

## Common gotchas

**Component doesn't appear in the picker.** Almost always one
of:
- Missing `#[derive(Reflect)]`.
- Missing `#[reflect(Component)]`.
- Has `@EditorHidden` somewhere (intentional or pasted from a
  template).
- The crate that defines the component isn't loaded yet (if
  you're using the dylib path). Restart the editor.

**Doc comment doesn't show as tooltip.** The
`reflect_documentation` feature has to be on for the type's
own crate. The workspace `Cargo.toml` enables it by default.
If you have your own bevy override, make sure
`reflect_documentation` is in the feature list.

**`On<Insert, T>` runs but the entity has the wrong
GlobalTransform.** Shouldn't happen in current jackdaw. If it
does, file a bug. Older versions of jackdaw needed an
`On<SceneInstanceReady>` walk; that's gone now.

**Scene fails to load with a panic.** Probably your
`Cargo.toml` has `panic = "abort"` and a reflected component
in your scene file no longer matches its current type
definition (you renamed a field, changed a type, etc). The
deserialize step returns errors cleanly, but a genuinely
panicking insert kills the process. Fix the schema drift in
the scene file or the type. Jackdaw used to swallow these
panics with `catch_unwind`; it doesn't anymore, because that
was hiding real bugs.
