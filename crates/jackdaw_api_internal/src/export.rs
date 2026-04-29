//! Boilerplate generators for dylib extensions and games.
//!
//! An extension crate uses [`export_extension!`](crate::export_extension);
//! a game project uses [`export_game!`](crate::export_game). Both
//! emit the `#[unsafe(no_mangle)]` entry function the loader looks
//! up plus the [`ExtensionEntry`](crate::ffi::ExtensionEntry) or
//! [`GameEntry`](crate::ffi::GameEntry) construction.
//!
//! # Extension example
//!
//! ```ignore
//! use jackdaw_api::prelude::*;
//! use jackdaw_api::export_extension;
//!
//! pub struct MyExtension;
//!
//! impl JackdawExtension for MyExtension {
//!     fn name(&self) -> &str { "my_extension" }
//!     fn register(&self, _ctx: &mut ExtensionContext) {}
//! }
//!
//! export_extension!("my_extension", || Box::new(MyExtension));
//! ```
//!
//! # Game example
//!
//! ```ignore
//! use bevy::prelude::*;
//! use jackdaw_api::export_game;
//!
//! pub struct GamePlugin;
//!
//! impl Plugin for GamePlugin {
//!     fn build(&self, app: &mut App) {
//!         app.add_systems(Update, spin_cube);
//!     }
//! }
//!
//! fn spin_cube() { /* run-condition gated on PlayState::Playing */ }
//!
//! export_game!("my_game", GamePlugin);
//! ```

/// Emit the `extern "C"` entry point a dylib extension needs.
///
/// The first argument is the extension name as a string literal; the
/// second is a `fn() -> Box<dyn JackdawExtension>` constructor. Both
/// are baked into the [`ExtensionEntry`](crate::ffi::ExtensionEntry)
/// returned by the generated `jackdaw_extension_entry_v1` symbol.
///
/// Invoke at most once per crate. Producing two extensions from one
/// dylib would fail at link time anyway (duplicate symbol).
#[macro_export]
macro_rules! export_extension {
    ($extension:ident) => {
        const _: () = {
            // Using a ZST means that our common pattern of creating a sample extension to harvest the ID etc. is a no-op.
            // We can also lift this requirement in the futureif we want.
            const _: () = {
                assert!(
                    std::mem::size_of::<$extension>() == 0,
                    "Extension must be a zero-sized type."
                );
            };

            unsafe extern "C" fn __jackdaw_ctor() -> $crate::ffi::JackdawExtensionPtr {
                let obj: Box<dyn JackdawExtension> = Box::new($extension);
                // SAFETY: Casting a trait object to a fat pointer is trivially fine in by itself,
                // but note that we are ignoring the fact that the ABI border does not guarantee
                // that we can actually turn this back
                unsafe {
                    let raw: *mut dyn JackdawExtension = Box::into_raw(obj);
                    std::mem::transmute(raw)
                }
            }

            unsafe extern "C" fn __jackdaw_dtor(ptr: $crate::ffi::JackdawExtensionPtr) {
                // SAFETY: This is not safe lol
                // We are just hoping that the ABI did not change in-between invocations
                // by making sure the editor and the extension are built with the exact same
                // compiler version + call
                unsafe {
                    let ptr: *mut dyn JackdawExtension = std::mem::transmute(ptr);
                    drop(Box::from_raw(ptr));
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn jackdaw_extension_entry_v1() -> $crate::ffi::ExtensionEntry {
                $crate::ffi::ExtensionEntry {
                    api_version: $crate::ffi::API_VERSION,
                    bevy_version: $crate::ffi::BEVY_VERSION.as_ptr(),
                    profile: $crate::ffi::PROFILE.as_ptr(),
                    ctor: __jackdaw_ctor,
                    dtor: __jackdaw_dtor,
                }
            }
        };

        $crate::__jackdaw_emit_reflect_register_symbol!();
    };
}

/// Emit the `extern "C"` entry point a dylib game needs.
///
/// The macro takes the game's [`Plugin`] type. The user's plugin must
/// implement `Default` so the editor's loader can construct fresh
/// instances on each (re)load. Invoke at most once per crate;
/// producing two entries from one dylib would fail at link time
/// anyway (duplicate symbol).
///
/// [`Plugin`]: bevy::app::Plugin
///
/// # Example
///
/// User code is plain Bevy — no jackdaw-specific imports beyond the
/// macro invocation:
///
/// ```ignore
/// use bevy::prelude::*;
///
/// #[derive(Default)]
/// pub struct MyGame;
///
/// impl Plugin for MyGame {
///     fn build(&self, app: &mut App) {
///         app.add_systems(Startup, spawn_player);
///         app.add_systems(Update, move_player);
///     }
/// }
///
/// jackdaw_api::export_game_plugin!(MyGame);
/// ```
///
/// The editor's dylib loader looks up the emitted
/// `jackdaw_game_entry_v1` symbol at startup, calls the factory to
/// obtain a `Box<dyn Plugin>`, and installs it against the editor's
/// `GameSubApp`.
#[macro_export]
macro_rules! export_game_plugin {
    ($plugin_ty:ty) => {
        const _: () = {
            // The dylib's name as a `CStr` for the FFI envelope. We
            // use the package name from the build environment because
            // the user no longer passes a name argument; the editor
            // doesn't actually need a unique per-game name (the
            // SubApp label `GameSubApp` is shared), but the FFI
            // header still carries one for diagnostics.
            const __JACKDAW_GAME_NAME: &::core::ffi::CStr =
                match ::core::ffi::CStr::from_bytes_with_nul(
                    ::core::concat!(env!("CARGO_PKG_NAME"), "\0").as_bytes(),
                ) {
                    ::core::result::Result::Ok(s) => s,
                    ::core::result::Result::Err(_) => {
                        ::core::panic!("CARGO_PKG_NAME contains interior NUL byte")
                    }
                };

            unsafe extern "C" fn __jackdaw_game_factory() -> $crate::ffi::JackdawGamePluginPtr {
                // Construct a fresh plugin instance and return it as
                // a stable-ABI fat pointer to the trait object.
                let plugin: ::std::boxed::Box<dyn ::bevy::app::Plugin> =
                    ::std::boxed::Box::new(<$plugin_ty as ::core::default::Default>::default());
                let raw: *mut dyn ::bevy::app::Plugin = ::std::boxed::Box::into_raw(plugin);
                // Split the fat pointer into (data, vtable). This
                // relies on the standard fat-pointer layout shared by
                // both sides via `bevy/dynamic_linking` + `jackdaw_sdk`.
                let parts: (*mut (), *mut ()) = unsafe {
                    ::core::mem::transmute::<*mut dyn ::bevy::app::Plugin, (*mut (), *mut ())>(raw)
                };
                $crate::ffi::JackdawGamePluginPtr {
                    data: parts.0,
                    vtable: parts.1,
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn jackdaw_game_entry_v1() -> $crate::ffi::GameEntry {
                $crate::ffi::GameEntry {
                    api_version: $crate::ffi::API_VERSION,
                    bevy_version: $crate::ffi::BEVY_VERSION.as_ptr(),
                    profile: $crate::ffi::PROFILE.as_ptr(),
                    name: __JACKDAW_GAME_NAME.as_ptr(),
                    factory: __jackdaw_game_factory,
                }
            }
        };

        $crate::__jackdaw_emit_reflect_register_symbol!();
    };
}

/// Internal helper shared by [`export_game!`] and
/// [`export_extension!`]. Emits the
/// `#[unsafe(no_mangle)] extern "Rust" fn
/// jackdaw_register_reflect_types_v1(&mut TypeRegistry)` symbol
/// the loader calls after `dlopen`.
///
/// The body `include!`s `$OUT_DIR/__jackdaw_reflect_types.rs`,
/// generated by the user crate's `build.rs`. A crate without the
/// build script (older scaffolds, hand-maintained extensions) hits
/// the `#[cfg(not(jackdaw_build_has_generated_reflect))]` arm and
/// emits an empty body instead.
///
/// build.rs sets the cfg via
/// `println!("cargo:rustc-cfg=jackdaw_build_has_generated_reflect")`.
#[doc(hidden)]
#[macro_export]
macro_rules! __jackdaw_emit_reflect_register_symbol {
    () => {
        // Suppress `unexpected_cfgs` for crates that don't ship the
        // build.rs. rustc's lint otherwise fires because it doesn't
        // know the cfg name.
        #[expect(non_snake_case, unexpected_cfgs)]
        mod __jackdaw_generated_reflect {
            #[cfg(jackdaw_build_has_generated_reflect)]
            include!(concat!(env!("OUT_DIR"), "/__jackdaw_reflect_types.rs"));

            #[cfg(not(jackdaw_build_has_generated_reflect))]
            pub(crate) fn __jackdaw_register_reflect_types(
                _registry: &mut ::bevy::reflect::TypeRegistry,
            ) {
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "Rust" fn jackdaw_register_reflect_types_v1(
            registry: &mut ::bevy::reflect::TypeRegistry,
        ) {
            __jackdaw_generated_reflect::__jackdaw_register_reflect_types(registry);
        }
    };
}
