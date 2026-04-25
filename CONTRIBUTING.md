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

Anything the user does in the editor is an **operator**: a small function with a string id. Buttons, menu entries, keybinds, and (later) the command palette and remote API all call the same operator by that id. Reach for an operator first when you're adding new behaviour. The exceptions are things that don't fit a single click, like continuous scroll input or per-frame reactive state.

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

The macro gives you back a `SpawnCubeOp` type. You'll use it to register the op, bind keys to it, and reference its id from buttons.

### Registering it on an extension

Operators live on a `JackdawExtension`. Built-in editor ops are on `JackdawCoreExtension` (`src/core_extension.rs`); your own extension registers its ops the same way. Inside `register`:

```rust
fn register(&self, ctx: &mut ExtensionContext) {
    ctx.register_operator::<SpawnCubeOp>();
}
```

### Binding it to a key

Bind a key by spawning a BEI action on the extension's input context. Use `ctx.spawn(...)` so the binding goes away with the extension if it's ever unloaded:

```rust
ctx.spawn((
    Action::<SpawnCubeOp>::new(),
    ActionOf::<MyInputContext>::new(ctx.id()),
    bindings![KeyCode::KeyP.with_mod_keys(ModKeys::CONTROL)],
));
```

For chords like Ctrl+Shift+S, use `with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT)` on a single binding. Don't make a separate "ctrl-held" action.

### Dispatching from a button

For Feathers buttons, the easiest path is `ButtonProps::from_operator`, which fills in both the label and the click dispatch from the operator's metadata:

```rust
button::button(ButtonProps::from_operator::<SpawnCubeOp>())
```

If you need a different label than the operator's `LABEL` (icon-only buttons, abbreviated toolbar entries), spell it out:

```rust
button::button(ButtonProps::new("Cube").call_operator(SpawnCubeOp::ID))
```

For other UI nodes (raw `Node`, picker entries, observers):

```rust
.observe(|_: On<Pointer<Click>>, mut commands: Commands| {
    commands.operator(SpawnCubeOp::ID).call();
})
```

Always go through the `<Op>::ID` constant. Don't type out the operator id as a string in editor code; that's only OK inside someone else's extension where the type isn't in scope.

### Parameters

If your operator needs to know something when it runs (which entity to delete, which axis to lock to), pull it out of `OperatorParameters`. There are helpers for the common types:

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

Document each parameter in the function's `///` doc comment under a `# Parameters` heading. Keep them out of the macro `description`, since that text shows up in the UI for artists.

The `let Some(...) = ... else { return Cancelled }` pattern is the current shape; the long-term goal is `let entity = params.as_entity("entity")?;`, which needs `OperatorResult` to implement `Try` (and `FromResidual<Option<Infallible>>`). It's not wired up yet, so use the `let-else` form for now.

### Availability checks

If your operator only makes sense when the editor is in a certain state (something selected, a particular mode active), add an `is_available` system that returns `bool`. Buttons and menu entries pointing at the operator will grey out automatically when it returns `false`:

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

Checks like "is an entity selected?" or "are we in edit mode?" belong in `is_available`, not inside the operator function. That way the UI can grey the action out instead of letting the user click it and silently fail.

### Modal operators

If your operator stays active across multiple frames (a drag, a dialog the user is filling in, sculpt-while-held), mark it `modal = true` and give it a `cancel` system. While the modal is running, your function gets called every frame; return `Running` to stay active, `Finished` when the user commits, or `Cancelled` to bail out. Pressing Escape calls your cancel system for you:

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
    active: ActiveModalQuery,
    /* ... */
) -> OperatorResult {
    if !active.is_modal_running() {
        // first frame setup
        return OperatorResult::Running;
    }
    // per-frame work; return Finished when committed
    OperatorResult::Running
}

fn cancel_box_select(/* ... */) { /* restore initial state */ }
```

`ActiveModalQuery` is the convenience system param for "is a modal currently running, and which one?". Use it instead of hand-rolling an `Option<Single<...>>` query against `ActiveModalOperator`.

### Description guidelines

The `description` is what users actually see in the editor and (later) in the command palette. Write it for an artist using the editor, not for the next developer reading the code:

- One sentence.
- No backticks or code names. Describe what the user sees happen.
- Don't talk about undo, history, dialogs, or "the OS". Assume those Just Work.
- Don't list parameters. Those go in the `///` doc comment.

Compare:

```rust
// Bad. Implementation detail leaked to the UI:
description = "Open an OS folder picker and store the result in `AssetBrowserFolderTask` for the polling system to consume."

// Good. What the user sees:
description = "Choose a different folder as the assets directory."
```

### Undo/redo

`allows_undo` defaults to `true`, which means the editor takes a snapshot of the scene before your operator runs and saves the changes as an undo step. That's what you want for almost every operator, so leave the default alone. Only set `allows_undo = false` when an undo entry would be wrong, like cancelling a modal or opening a settings dialog.

If your operator records its own history entry through `CommandHistory::push_executed`, you can still leave `allows_undo` as `true`. The automatic snapshot will see no changes and skip making a duplicate entry.

## License

By contributing, you agree that your contributions will be licensed under the project's dual MIT/Apache-2.0 license.
