# bevy_window_chrome

Create customizable native-like window chrome for Bevy. Add UI elements to the title bar of your Bevy window.

## Usage

Add the plugin, configure a borderless primary window, then fill the shell slots with your UI. Mark non-interactive title-bar nodes with [`Pickable::IGNORE`] so they don't steal picks from the drag behavior. Interactive widgets can keep the default pick behavior.

See basic.rs for more detail.

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
