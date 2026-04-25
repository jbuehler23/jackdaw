# Contributing to Jackdaw

Thank you for your interest in contributing to Jackdaw! This document covers the basics of getting set up and submitting changes.

## Development Setup

### Prerequisites

- **Rust nightly toolchain** - Jackdaw uses edition 2024 features
  ```sh
  rustup toolchain install nightly
  rustup default nightly
  ```
- **System dependencies** - GPU drivers with Vulkan support (or Metal on macOS)
- **Linux extras** - `libudev-dev`, `libasound2-dev`, `libwayland-dev` (or equivalent for your distro)

### Clone and Build

```sh
git clone https://github.com/jbuehler23/jackdaw.git
cd jackdaw
cargo build
```

### Running

```sh
# Run the basic example
cargo run --example basic

# Working on extension loading? Build with the dylib feature so the
# dylib loader is exercised end-to-end (editor binary links against
# the shared libbevy_dylib + libjackdaw_dylib). First build is slow
# because Bevy and the workspace's shared types recompile as
# dylibs; subsequent incremental builds are fast.
cargo run --features dylib
```

## Checks

Before submitting a PR, make sure the following pass:

```sh
# Format
cargo fmt --all --check

# Lint
cargo clippy --workspace -- -D warnings

# Tests
cargo test --workspace

# Doc build
cargo doc --workspace --no-deps
```

## Pull Requests

1. Fork the repository and create a feature branch from `main`
2. Keep changes focused if possible, but chat to me on discord if you want more overarching changes!
3. Make sure all checks above pass
4. Open a PR against `main` with a clear description of what changed and why

## Writing an operator

User-facing editor actions are written as **operators**: Blender-style functions identified by a string id, registered with the editor's extension catalog, and dispatchable from UI buttons, menu entries, keybinds, the (planned) command palette, and the (planned) remote/scripting API. Every new editor behaviour should be an operator unless there's a specific reason it can't be (continuous-scroll input, per-frame reactive state, etc.).

### A minimal example

```rust
use jackdaw_api::prelude::*;
use bevy::prelude::*;

/// Spawn a cube at the origin.
#[operator(
    id = "sample.spawn_cube",
    label = "Add Cube",
    description = "Add a cube to the scene."
)]
fn spawn_cube(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.spawn((Name::new("Cube"), Transform::default()));
    OperatorResult::Finished
}
```

The `#[operator]` macro generates a zero-sized type (`SpawnCubeOp`) that implements the `Operator` trait, derives `InputAction` so it can be used as a BEI binding target, and registers `spawn_cube` as the execute system.

### Registering it on an extension

Operators are registered on a `JackdawExtension`. The core editor functionality lives on `JackdawCoreExtension` (`src/core_extension.rs`); custom extensions register their own. Inside `register`:

```rust
fn register(&self, ctx: &mut ExtensionContext) {
    ctx.register_operator::<SpawnCubeOp>();
}
```

### Binding it to a key

Use BEI bindings on the extension's input context. `ctx.spawn(...)` attaches the binding to the extension entity so it's torn down on unload:

```rust
ctx.spawn((
    Action::<SpawnCubeOp>::new(),
    ActionOf::<MyInputContext>::new(ctx.id()),
    bindings![KeyCode::KeyP.with_mod_keys(ModKeys::CONTROL)],
));
```

Modifier chords use `with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT)` — don't define separate "ctrl-held" actions.

### Dispatching from a button

For Feathers buttons:

```rust
button::button(ButtonProps::new("Add Cube").call_operator(SpawnCubeOp::ID))
```

For other UI nodes (raw `Node`, picker entries, observers):

```rust
.observe(|_: On<Pointer<Click>>, mut commands: Commands| {
    commands.operator(SpawnCubeOp::ID).call();
})
```

Always reference the `<Op>::ID` constant — never hand-type the operator id string outside of user-extension code.

### Parameters

If the operator needs runtime data (an entity to act on, an axis to constrain to, etc.) use `OperatorParameters`. Helpers exist for the common conversions:

```rust
fn delete_entity(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    commands.entity(entity).despawn();
    OperatorResult::Finished
}
```

Document each parameter in the function's `///` doc comment under a `# Parameters` section. Don't list parameters in the macro `description` — that text is shown to artists in the UI.

### Availability checks

If the op should be greyed out / not callable when the editor isn't in the right state, attach an `is_available` system that returns `bool`:

```rust
fn has_selection(selection: Res<Selection>) -> bool {
    selection.primary().is_some()
}

#[operator(
    id = "selection.delete",
    label = "Delete Selected",
    description = "Delete the selected entity.",
    is_available = has_selection
)]
fn delete_selected(...) -> OperatorResult { ... }
```

Selection / mode / "is something open" preconditions belong in `is_available`, not in the operator body.

### Modal operators

For multi-frame interactions (drag, dialog, sculpt-while-held) set `modal = true` and provide a `cancel` system. The invoke runs every frame until it returns `Finished` or `Cancelled`; the global Escape binding routes to `cancel` automatically:

```rust
#[operator(
    id = "tool.box_select",
    label = "Box Select",
    description = "Drag a rectangle to select entities.",
    modal = true,
    cancel = cancel_box_select,
)]
fn box_select(
    _: In<OperatorParameters>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
    /* ... */
) -> OperatorResult {
    if modal.is_none() {
        // first frame setup
        return OperatorResult::Running;
    }
    // per-frame work; return Finished when committed
    OperatorResult::Running
}

fn cancel_box_select(/* ... */) { /* restore initial state */ }
```

### Description guidelines

The `description` ends up in the editor UI and the (planned) command palette, so write for non-programming artists:

- One sentence.
- No backticks or code references — describe the user-visible effect.
- No mentions of undo / history / dialogs / "OS" — assume they Just Work.
- No parameters — those go in `///` docs.

Compare:

```rust
// Bad — implementation detail leaked to the UI:
description = "Open an OS folder picker and store the result in `AssetBrowserFolderTask` for the polling system to consume."

// Good — what the user sees:
description = "Choose a different folder as the assets directory."
```

### Undo/redo

`allows_undo` defaults to `true`; the dispatcher captures a scene snapshot before the op runs and diffs after. The default is correct for almost all operators — leave it alone. Set `allows_undo = false` only for ops that explicitly should not produce an undo entry (cancelling a modal, opening a settings dialog, etc.).

If the operator pushes its own history command via `CommandHistory::push_executed`, the framework still defaults to `true` and harmlessly no-ops on the resulting empty diff.

## License

By contributing, you agree that your contributions will be licensed under the project's dual MIT/Apache-2.0 license.
