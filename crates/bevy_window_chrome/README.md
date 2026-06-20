# bevy_window_chrome

Create customizable native-like window chrome for Bevy. Add UI elements to the title bar of your Bevy window.

## Usage

Configure the primary window, add the plugin, then fill the shell slots with your UI. Mark non-interactive title-bar nodes with [`Pickable::IGNORE`] so they don't steal picks from the drag behavior. Interactive widgets can keep the default pick behavior.

```rust
use bevy::prelude::*;
use bevy_window_chrome::{WindowChromePlugin, WindowChromeTheme, chrome_window_plugin, spawn_window_shell};

App::new()
    .add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(primary_window_attributes()),
        ..default()
    }))
    .add_plugins(WindowChromePlugin::new(WindowChromeTheme::default()))
    .add_systems(Startup, setup)
    .run()
}

fn setup(
    mut commands: Commands,
    theme: Res<WindowChromeTheme>,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    caption_font: Res<CaptionFont>,
) {
    let slots = spawn_window_shell(
        &mut commands,
        &theme,
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        caption_font,
        DemoRoot,
    );
    // use slots.title_bar and slots.body...
}
```

See `examples/basic.rs` for more detail.

## Example

```bash
cargo run -p bevy_window_chrome --example basic
```

## Platforms

- **Windows** — client driven, Caption icons use Segoe when available, otherwise an embedded Lucide subset.

[![Windows window chrome](https://raw.githubusercontent.com/jbuehler23/jackdaw/main/crates/bevy_window_chrome/assets/windows_screenshot.png)](https://raw.githubusercontent.com/jbuehler23/jackdaw/main/crates/bevy_window_chrome/assets/windows_screenshot.png)

- **Linux / FreeBSD** — client driven, Caption icons use an embedded Lucide subset.

[![Mac window chrome](https://raw.githubusercontent.com/jbuehler23/jackdaw/main/crates/bevy_window_chrome/assets/mac_screenshot.png)](https://raw.githubusercontent.com/jbuehler23/jackdaw/main/crates/bevy_window_chrome/assets/mac_screenshot.png)

- **macOS** — native traffic lights with a transparent integrated title bar slot, native window resize.

## TODO

- **Flexible Rework** — Make the system more flexible and customizable (able to place and style caption icons freely). The title bar/body split would be created using this system as a default.
- **Windows Caption Bug** — Dragging mouse to the very top of the window does not allow for caption button clicks.

## License

Same as Jackdaw _(MIT or Apache-2.0)_. Linux and fallback Windows caption icons are derived from [Lucide Icons](https://lucide.dev) (ISC / MIT).
