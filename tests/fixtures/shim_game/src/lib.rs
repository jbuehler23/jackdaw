//! Runner shim for `reflect_game`: the stand-in for the crate the
//! editor generates into `.jackdaw/` at build time. The shim is the
//! crate that actually becomes the project dylib; the user's library is
//! an rlib inside it. It exposes the one symbol the game runner looks
//! up. Because host and dylib provably share one ABI (the SDK pipeline
//! plus the linkage-identity check), the entry is a plain Rust
//! function over `&mut App`: no C envelope, no version tags.

#[unsafe(no_mangle)]
pub fn jackdaw_runner_entry(app: &mut bevy::app::App) {
    app.add_plugins(reflect_game::GamePlugin);
}
