//! Stable ABI used by the dylib extension and game loaders.
//!
//! Two kinds of loadable dylib are supported, distinguished by
//! which entry symbol they expose:
//!
//! * **Extensions** expose `jackdaw_extension_entry_v1` and return
//!   an [`ExtensionEntry`]. Loaded by the dylib loader at startup
//!   and via the runtime Install UI.
//! * **Games** expose `jackdaw_game_entry_v1` and return a
//!   [`GameEntry`]. Loaded at startup alongside extensions; their
//!   `build` callback is invoked against the editor's `App` so
//!   game plugins integrate natively.
//!
//! Both envelopes share the same version fields
//! ([`API_VERSION`] / [`BEVY_VERSION`] / [`PROFILE`]); the loader
//! verifies them identically.
//!
//! Authors don't write these structs by hand. The
//! [`export_extension!`](crate::export_extension) and
//! [`export_game!`](crate::export_game) macros emit the entry
//! functions. The loader lives in `crates/jackdaw_loader`.
//!
//! # ABI stability
//!
//! All three embedded version fields must match host values
//! exactly:
//!
//! * [`API_VERSION`]: bumped on any breaking change to
//!   `JackdawExtension`, the FFI struct layout, or entry semantics.
//! * [`BEVY_VERSION`]: Bevy minor-version string. Bevy's types
//!   (`App`, `World`, `Commands`) appear in the extension's vtable,
//!   so any Bevy version change risks vtable drift.
//! * [`PROFILE`]: debug vs release. The two are ABI-incompatible in
//!   practice (different feature combinations, different layout
//!   optimisations).

use core::ffi::{CStr, c_char};

/// Current ABI version. Bump on any breaking change to
/// [`ExtensionEntry`], [`GameEntry`], [`crate::JackdawExtension`],
/// or the loader's expectations about the entry function.
///
/// Extensions and games can diverge (each struct carries its own
/// version field in its header), but we use a shared monotonic
/// number so the loader's compat check is a single comparison.
///
/// - v2 introduced the `GameEntry::teardown` pointer and swapped
///   `build` from `*mut App` to `*mut World`, needed for in-process
///   hot reload of games against a single `World`.
/// - v3 (current) replaces `GameEntry::{build, teardown}` with a
///   single [`GameEntry::factory`] returning a [`JackdawGamePluginPtr`].
///   The dylib loader installs the plugin against a `GameSubApp` at
///   editor startup; teardown becomes `App::remove_sub_app` /
///   `insert_sub_app` swap, no longer needs the dylib's involvement.
pub const API_VERSION: u32 = 3;

/// Bevy minor-version string the host was built against. The loader
/// compares this against the dylib's embedded value and refuses to
/// load on mismatch.
pub const BEVY_VERSION: &CStr = c"0.18";

/// Compile-time build profile. Debug and release builds are
/// ABI-incompatible in practice, so the loader refuses to mix them.
pub const PROFILE: &CStr = if cfg!(debug_assertions) {
    c"debug"
} else {
    c"release"
};

/// Symbol name the loader looks up in extension dylibs. Includes
/// the trailing NUL so it can be passed directly to
/// `libloading::Library::get`.
pub const ENTRY_SYMBOL: &[u8] = b"jackdaw_extension_entry_v1\0";

/// Symbol name the loader looks up in game dylibs. Includes the
/// trailing NUL for the same reason as [`ENTRY_SYMBOL`].
///
/// When a dylib is opened, the loader tries this symbol first. If
/// it's absent the dylib is treated as an extension and
/// [`ENTRY_SYMBOL`] is looked up next.
pub const GAME_ENTRY_SYMBOL: &[u8] = b"jackdaw_game_entry_v1\0";

/// Symbol name for the cdylib's layout fingerprint, emitted by the
/// [`export_game_plugin!`](crate::export_game_plugin) macro. The
/// loader reads this `u64` constant at dlopen and compares it to
/// `jackdaw_runtime::LAYOUT_FINGERPRINT`; any mismatch refuses the
/// swap. See [`crate::fingerprint`] for the rationale.
pub const FINGERPRINT_SYMBOL: &[u8] = b"__JACKDAW_LAYOUT_FINGERPRINT\0";

/// Optional symbol each extension or game dylib may expose to
/// register its own `#[derive(Reflect)]` types into the host's
/// `AppTypeRegistry` after `dlopen`. The loader calls it with a
/// mutable reference to the host's `TypeRegistry` right after
/// `dlopen` succeeds and before invoking `build`.
///
/// A dylib without this symbol contributes no types; the loader
/// skips the step.
///
/// Jackdaw's scaffolded projects ship a `build.rs` that scans
/// `src/` for `#[derive(Reflect)]` and writes explicit
/// `registry.register::<T>()` lines to
/// `$OUT_DIR/__jackdaw_reflect_types.rs`. The
/// [`export_game!`](crate::export_game) and
/// [`export_extension!`](crate::export_extension) macros `include!`
/// that file into a `#[unsafe(no_mangle)] extern "Rust" fn`
/// wrapper named `jackdaw_register_reflect_types_v1`.
///
/// This is the pattern `dexterous_developer` uses. We don't reach
/// across the boundary into bevy's `inventory`-based
/// `AutomaticReflectRegistrations`: that breaks under
/// `bevy/dynamic_linking` because the shared-dylib proxy doesn't
/// preserve inventory's private invariants for external consumers.
pub const REFLECT_REGISTER_SYMBOL: &[u8] = b"jackdaw_register_reflect_types_v1\0";

/// Signature for [`REFLECT_REGISTER_SYMBOL`]. `extern "Rust"` is
/// sound when both sides share `TypeRegistry`'s type layout through
/// a single shared compilation graph (the per-project editor binary
/// model that subsequent phases rebuild).
pub type ReflectRegisterFn = unsafe extern "Rust" fn(&mut bevy::reflect::TypeRegistry);

/// Shape returned by every dylib extension's entry function.
///
/// Declared `#[repr(C)]` so the layout is stable across compilation
/// units. Trait-object fields (`ctor`'s return type) require the
/// editor and extension to have been built against the same Bevy
/// version, hence the embedded version fields.
///
/// # Safety
///
/// Every pointer field must reference NUL-terminated static data
/// that outlives the host process. The returned `Box` from `ctor`
/// must be allocated with the same allocator the host uses; this is
/// guaranteed when both sides link against Bevy with
/// `dynamic_linking` enabled, which shares the Rust allocator.
#[repr(C)]
pub struct ExtensionEntry {
    pub api_version: u32,
    pub bevy_version: *const c_char,
    pub profile: *const c_char,
    pub ctor: unsafe extern "C" fn() -> JackdawExtensionPtr,
    pub dtor: unsafe extern "C" fn(JackdawExtensionPtr),
}

#[repr(C)]
pub struct JackdawExtensionPtr {
    data: *mut (),
    vtable: *mut (),
}

/// Shape returned by every game dylib's entry function.
///
/// v3 replaces v2's `build`/`teardown` `*mut World` callbacks with a
/// single [`Self::factory`] that returns the user's [`Plugin`] as a
/// stable-ABI [`JackdawGamePluginPtr`]. The loader installs the
/// returned plugin against a `GameSubApp` at editor startup via
/// [`App::insert_sub_app`](bevy::app::App::insert_sub_app). Hot-reload
/// becomes `remove_sub_app` + `insert_sub_app` (the dylib's
/// involvement ends after the factory call).
///
/// [`Plugin`]: bevy::app::Plugin
///
/// # Safety
///
/// Same contract as [`ExtensionEntry`]: NUL-terminated static
/// strings, same allocator on both sides (guaranteed when both
/// sides compile through one shared graph). The factory is
/// callable any number of times; each call returns a freshly-boxed
/// plugin instance the caller owns. The loader wraps the call in
/// `catch_unwind` so a panic in game code doesn't abort the editor.
#[repr(C)]
pub struct GameEntry {
    pub api_version: u32,
    pub bevy_version: *const c_char,
    pub profile: *const c_char,
    pub name: *const c_char,
    /// Construct a fresh `Box<dyn Plugin>` for the user's game.
    /// The loader takes ownership of the returned pointer and is
    /// responsible for `Box::from_raw` reconstruction before
    /// `app.add_plugins(plugin)`.
    pub factory: unsafe extern "C" fn() -> JackdawGamePluginPtr,
}

/// Stable-ABI representation of a `Box<dyn bevy::app::Plugin>`.
///
/// Bevy's `Box<dyn Plugin>` is not `extern "C"`-safe across rustc
/// compilations. It's a fat pointer (data + vtable) whose vtable
/// layout depends on the rustc that built the `dyn Plugin`. We rely
/// on a shared compile graph (Bevy's `dynamic_linking` plus the
/// per-project editor binary architecture) so both editor and dylib
/// see one Bevy compilation and the fat pointer reconstructs
/// correctly on the loader side.
#[repr(C)]
pub struct JackdawGamePluginPtr {
    pub data: *mut (),
    pub vtable: *mut (),
}
