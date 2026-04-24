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
/// The first argument is the game name as a string literal; the
/// second is an expression evaluating to a value implementing
/// [`GamePlugin`](crate::runtime::GamePlugin). The plugin's
/// `build` method runs once at startup **and** on every hot-reload
/// cycle. Register game systems, observers, resources, and reflect
/// types via the [`GameApp`](crate::runtime::GameApp) context.
/// `GameApp` tracks every registration so the default `teardown`
/// can reverse them automatically when the dylib is swapped.
///
/// Invoke at most once per crate. Producing two entries from one
/// dylib would fail at link time anyway (duplicate symbol).
///
/// # Example
///
/// ```ignore
/// use bevy::prelude::*;
/// use jackdaw_api::prelude::*;    // brings `GamePlugin`, `GameApp`, `PlayState`
/// use jackdaw_api::export_game;
///
/// #[derive(Default)]
/// pub struct MyGame;
///
/// impl GamePlugin for MyGame {
///     fn build(&self, ctx: &mut GameApp<'_>) {
///         ctx.add_systems(Update, spin_cube.run_if(in_state(PlayState::Playing)));
///     }
/// }
///
/// fn spin_cube() { /* … */ }
///
/// export_game!("my_game", MyGame);
/// ```
#[macro_export]
macro_rules! export_game {
    ($name:literal, $plugin:expr) => {
        const _: () = {
            const __JACKDAW_GAME_NAME_STR: &str = $name;
            const __JACKDAW_GAME_NAME: &::core::ffi::CStr =
                match ::core::ffi::CStr::from_bytes_with_nul(
                    ::core::concat!($name, "\0").as_bytes(),
                ) {
                    ::core::result::Result::Ok(s) => s,
                    ::core::result::Result::Err(_) => {
                        ::core::panic!("game name contains interior NUL byte")
                    }
                };

            unsafe extern "C" fn __jackdaw_game_build(world: *mut ::bevy::ecs::world::World) {
                // SAFETY: the loader calls this with a valid `&mut World`
                // pointer obtained from an exclusive system. The pointer
                // lives for the duration of that system's call.
                let world: &mut ::bevy::ecs::world::World = unsafe { &mut *world };
                let mut ctx = $crate::runtime::GameApp::new(world, __JACKDAW_GAME_NAME_STR);
                $crate::runtime::GamePlugin::build(&$plugin, &mut ctx);
            }

            unsafe extern "C" fn __jackdaw_game_teardown(world: *mut ::bevy::ecs::world::World) {
                // SAFETY: same contract as `build`.
                let world: &mut ::bevy::ecs::world::World = unsafe { &mut *world };
                let mut ctx = $crate::runtime::GameApp::new(world, __JACKDAW_GAME_NAME_STR);
                $crate::runtime::GamePlugin::teardown(&$plugin, &mut ctx);
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn jackdaw_game_entry_v1() -> $crate::ffi::GameEntry {
                $crate::ffi::GameEntry {
                    api_version: $crate::ffi::API_VERSION,
                    bevy_version: $crate::ffi::BEVY_VERSION.as_ptr(),
                    profile: $crate::ffi::PROFILE.as_ptr(),
                    name: __JACKDAW_GAME_NAME.as_ptr(),
                    build: __jackdaw_game_build,
                    teardown: __jackdaw_game_teardown,
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
