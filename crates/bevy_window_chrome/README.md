# bevy_window_chrome

Create customizable native-like window chrome for Bevy. Add UI elements to the title bar of your bevy window.

## Usage

Add the plugin, configure a borderless primary window, then fill the shell slots with your UI. Mark non-interactive title-bar nodes with [`Pickable::IGNORE`](https://docs.rs/bevy/latest/bevy/picking/struct.Pickable.html) so they don't steal picks from the drag behavior. Interactive widgets can keep the default pick behavior.

```rust
use bevy::picking::Pickable;

app.add_plugins(WindowChromePlugin::new(WindowChromeTheme::default()));

// When adding DefaultPlugins, set WindowPlugin primary_window
app.add_plugins(DefaultPlugins.set(
    WindowPlugin {
        primary_window: Some(primary_window_attributes()),
        ..default()
    }
));

// In a startup system:
fn setup(
    mut commands: Commands,
    theme: Res<WindowChromeTheme>,
    caption_font: Option<Res<CaptionFont>>,
) {
    let slots = spawn_window_shell(&mut commands, &theme, caption_font, MyScreen);

    // Fill the title bar and body slots with your own content
    commands.entity(slots.title_bar).with_children(|title_bar| {
        title_bar.spawn((
            Text::new("My App"),
            Pickable::IGNORE,
            Node::default(),
        ));
    });
    commands.entity(slots.body).with_children(|body| {
        body.spawn((
            Node::default(),
        ));
    });
}
```

## Example

```bash
cargo run -p bevy_window_chrome --example basic
```

## Platforms

- **Windows** — client driven, Caption icons use Segoe when available, otherwise an embedded Lucide subset.
- **Linux / FreeBSD** —  client driven, Caption icons use an embedded Lucide subset.
- **macOS** — native traffic lights with a transparent integrated title bar slot, native window resize.

## License

Same as Jackdaw _(MIT or Apache-2.0)_. Linux and fallback Windows caption icons are derived from [Lucide Icons](https://lucide.dev) (ISC / MIT).
