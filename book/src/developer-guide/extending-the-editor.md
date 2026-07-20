# Extending the editor

Jackdaw extensions are plain Rust crates that you write using
bevy-native APIs. An extension is a normal Bevy library that also
depends on `jackdaw_api` and implements the `JackdawExtension`
trait. No export macro, no custom build scripts, no cargo
gymnastics.

## Author workflow

### Create an extension

Click **New Project** on the launcher and pick **Extension** (or
run `jackdaw new my_tool --extension`). Fill in:

- **Name**: the crate name for your extension (e.g. `my_tool`).
- **Location**: parent directory the project will be created
  under. The `Browse` button opens a folder picker.

The template is embedded in the editor, so scaffolding is offline
and instant. The result is an ordinary crate:

```toml
[package]
name = "my_tool"
edition = "2024"

[dependencies]
bevy = "0.19"
jackdaw_api = "0.19"
```

Jackdaw's crate versions track the Bevy minor they target, so an
extension for a Bevy 0.19 editor uses the `0.19.x` line of
`jackdaw_api`.

`src/lib.rs` implements the trait:

```rust
use bevy::prelude::*;
use jackdaw_api::prelude::*;

#[derive(Default)]
pub struct MyTool;

impl JackdawExtension for MyTool {
    fn id(&self) -> String {
        "my_tool".to_string()
    }

    fn label(&self) -> String {
        "My Tool".to_string()
    }

    fn description(&self) -> String {
        "What this extension adds to the editor.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        // operators, menu entries, panels, windows, keybinds
    }
}
```

### Open it in jackdaw

Open the extension project from the launcher like any other
project. The editor builds it in the background (into the
project's gitignored `.jackdaw/` directory, against the editor's
SDK) and loads it into the running editor. Windows, operators,
and menu entries activate as soon as the build finishes.

Iterate: edit `src/lib.rs` in your preferred editor, rebuild from
jackdaw, see the changes. Your own `cargo check` and `cargo build`
in the project folder work standalone against plain crates.io
Bevy; the editor's build is separate and never touches your
manifest.

## How it works

Jackdaw ships an SDK: a proxy dylib carrying the one compiled
copy of bevy + jackdaw types that the editor and everything it
loads share. When the editor builds a project or extension, it
generates a shim crate into `.jackdaw/` and compiles your library
against that SDK, so `use bevy::prelude::*;` and
`use jackdaw_api::prelude::*;` in your code resolve to the
editor's own types. One shared ABI, no hash-matching games, and
nothing jackdaw-specific in your `Cargo.toml`.

### BEI keybind caveat

Loading an extension activates windows, menu entries, operators,
and panel sections immediately. BEI input contexts are the
exception: `add_input_context::<C>()` needs `&mut App`, which only
exists at startup. Keybinds declared via BEI don't bind until the
editor restarts. Extensions that don't use BEI keybinds don't need
a restart.

### Crash quarantine

If the editor crashed while an extension was loading, the next
start refuses to load that extension and surfaces an "Extension X
crashed" notice. You can re-enable it from the Extensions dialog.

## Escape hatches

### Install a prebuilt dylib

If you already have a compatible `.so` / `.dylib` / `.dll`
(a teammate's build, a CI artefact), install it through the
editor's Extensions dialog. The editor copies it into the
extension directory and loads it.

### Statically link an extension

For in-house tools bundled into a custom editor binary, skip the
dylib path entirely:

```rust
// your_editor/src/main.rs
fn main() {
    App::new()
        .add_plugins(
            jackdaw::EditorPlugins::default()
                .with_extension("my_tool", || Box::new(MyTool))
                .build(),
        )
        .run();
}
```

Nothing crosses a dylib boundary; everything is normal static
linking. Use for tools you control and ship together with the
editor.

## Troubleshooting

- *Build failed* after opening the project: your extension has a
  compile error. The status line shows the compiler output.
- *Extension doesn't load*: check the Extensions dialog for its
  state; a crash on a previous start quarantines it until you
  re-enable.
- *Bevy version mismatch on import*: each editor release supports
  one Bevy minor (currently 0.19); the error tells you which
  version the project declares.

## In-tree examples

The workspace has two example extensions you can read or
build against:

- `examples/extension/dynamic_extension/`: operators with
  keybinds, availability checks, a dock window, and menu
  entries. Good reference for what the API supports.
- `examples/extension/viewable_camera_extension/`: heavier
  scene manipulation through `ExtensionContext::world()`.

They build like any other workspace crate. They're there to
exercise the API surface; day-to-day authoring should use the
scaffold-and-open workflow described above.
