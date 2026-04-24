//! Runtime discovery and loading of Jackdaw extension dylibs.
//!
//! # Overview
//!
//! Add [`DylibLoaderPlugin`] to the editor `App`. During `build` it
//! walks every configured search path, opens each dynamic library
//! with `libloading`, looks up the `jackdaw_extension_entry_v1`
//! symbol (see [`jackdaw_api_internal::ffi::ENTRY_SYMBOL`]), verifies ABI
//! compatibility, and registers the extension through
//! [`jackdaw_api_internal::lifecycle::register_dylib_extension`].
//!
//! The plugin lives in [`LoadedDylibs`] as long as the `App` lives.
//! Unloading a library while systems still reference code inside it
//! is UB, so libraries are only dropped when the `App` is destroyed.
//!
//! # Search paths
//!
//! By default the loader searches the per-user config directory
//! (`~/.config/jackdaw/extensions/` and platform equivalents). The
//! `JACKDAW_EXTENSIONS_DIR` environment variable adds another path.
//! Callers can add their own via [`DylibLoaderPlugin::extra_paths`].
//!
//! # Safety
//!
//! Loading third-party native code is inherently unsafe. Host and
//! extension must agree on ABI; the `compat` module enforces the
//! subset we can check automatically (API version, Bevy version,
//! build profile). A panic in the entry function is caught via
//! `catch_unwind`, but a segfault in extension code takes the
//! process down.
//!
//! # Shared-type ABI requirement
//!
//! Both sides must share one compiled copy of the jackdaw types
//! that cross the boundary so `TypeId::of::<T>()` agrees. That's
//! what `jackdaw_api`'s `dynamic_linking` feature sets up: it links
//! `jackdaw_dylib`, a single `.so` bundling `jackdaw_api_internal`,
//! `jackdaw_panels`, and `jackdaw_commands`. The host binary must
//! be built with `jackdaw`'s `dylib` feature; otherwise this
//! loader reads the entry point and compat stamp fine but the
//! extension panics as soon as it touches `ExtensionContext::
//! register_window` because the host's `WindowRegistry` is keyed by
//! a different `TypeId`.

mod compat;

use std::ffi::CStr;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api_internal::JackdawExtension;
use jackdaw_api_internal::ffi::{
    ENTRY_SYMBOL, ExtensionEntry, GAME_ENTRY_SYMBOL, GameEntry, JackdawExtensionPtr,
};

pub use compat::CompatError;

/// Names of all games whose dylibs have been successfully loaded
/// this session. Populated at startup by the loader; consumed by
/// the editor's PIE plugin to show the "game loaded" indicator
/// and to know which game the Play button should run.
///
/// `entries` holds the `build`/`teardown` fn pointers per game name
/// so the hot-reload driver can invoke them from an exclusive
/// system without re-opening the library.
#[derive(Resource, Default, Debug, Clone)]
pub struct GameCatalog {
    pub games: Vec<String>,
    pub entries: std::collections::HashMap<String, LoadedGameEntry>,
}

/// Callable pair from a loaded game dylib. Function pointers remain
/// valid as long as the backing `libloading::Library` is held in
/// [`LoadedDylibs`].
#[derive(Clone, Copy, Debug)]
pub struct LoadedGameEntry {
    pub build: unsafe extern "C" fn(*mut bevy::ecs::world::World),
    pub teardown: unsafe extern "C" fn(*mut bevy::ecs::world::World),
}

/// Sub-directory inside the platform config directory where the
/// loader looks for per-user extensions (editor tools, panels,
/// operators).
pub const DEFAULT_EXTENSIONS_SUBDIR: &str = "jackdaw/extensions";

/// Sub-directory for per-user game dylibs. Kept separate from
/// extensions so the two don't fight over filenames and the user
/// can manage each category independently.
pub const DEFAULT_GAMES_SUBDIR: &str = "jackdaw/games";

/// Prefix used by the install flow's atomic-rename tempfile. The
/// extension/games watcher skips paths starting with this prefix so
/// our own in-flight renames don't trip "Dylib changed on disk"
/// warnings. Shared here rather than duplicated in
/// `extensions_dialog::install_picked_file` and `extension_watcher`
/// so the two can't drift.
pub const INSTALL_TEMPFILE_PREFIX: &str = ".jackdaw-install-";

/// Environment variable whose value, if set to a directory path,
/// is added to the loader's search paths at startup for extensions.
pub const ENV_EXTENSIONS_PATH: &str = "JACKDAW_EXTENSIONS_DIR";

/// Environment variable whose value, if set to a directory path,
/// is added to the loader's search paths at startup for games.
pub const ENV_GAMES_PATH: &str = "JACKDAW_GAMES_DIR";

/// Back-compat alias for `ENV_EXTENSIONS_PATH`. Older docs and
/// scripts reference this name; prefer the split env vars above.
#[deprecated(note = "use ENV_EXTENSIONS_PATH or ENV_GAMES_PATH")]
pub const ENV_SEARCH_PATH: &str = ENV_EXTENSIONS_PATH;

/// Keeps `libloading::Library` handles alive for the lifetime of the
/// `App`. The resource is inserted by [`DylibLoaderPlugin::build`]
/// and never drained; dropping a `Library` while systems still
/// reference its code is UB.
#[derive(Resource, Default)]
pub struct LoadedDylibs {
    libs: Vec<libloading::Library>,
}

impl LoadedDylibs {
    pub fn len(&self) -> usize {
        self.libs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.libs.is_empty()
    }
}

/// Enable discovery and loading of dynamic-library extensions.
///
/// With the defaults, the loader scans the per-user config
/// directory (`~/.config/jackdaw/extensions/` and platform
/// equivalents) plus `$JACKDAW_EXTENSIONS_DIR` if set. Call
/// [`Self::with_extension_search_path`] to add more locations
/// or [`Self::with_user_extension_dir`] /
/// [`Self::with_extension_env_var`] to opt out of the defaults.
///
/// Dynamic-library extensions require the host binary to be
/// built with `bevy/dynamic_linking` so the editor and every
/// loaded extension share one copy of Bevy at runtime. Without
/// that, trait-object calls across the dylib boundary are
/// unsound.
///
/// Configuration lives on the plugin itself because loading happens
/// during `build()`, so the loader can reach `&mut App` to register
/// each discovered dylib into the extension catalog.
pub struct DylibLoaderPlugin {
    /// Extra search paths added on top of the defaults.
    pub extra_paths: Vec<PathBuf>,
    /// If `true` (default), also search the per-user config dir.
    pub include_user_dir: bool,
    /// If `true` (default), also search
    /// `$JACKDAW_EXTENSIONS_DIR` when that env var is set.
    pub include_env_dir: bool,
}

impl Default for DylibLoaderPlugin {
    fn default() -> Self {
        Self {
            extra_paths: Vec::new(),
            include_user_dir: true,
            include_env_dir: true,
        }
    }
}

impl DylibLoaderPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an explicit search path for the dylib loader. Implicitly
    /// enables the loader if it wasn't already.
    pub fn with_extension_search_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.extra_paths.push(path.into());
        self
    }

    /// Opt in or out of honouring `$JACKDAW_EXTENSIONS_DIR`.
    /// Defaults to `true` when the loader is enabled.
    pub fn with_extension_env_var(mut self, enable: bool) -> Self {
        self.include_env_dir = enable;
        self
    }
    /// Opt in or out of searching the per-user config directory.
    /// Defaults to `true` when the loader is enabled.
    pub fn with_user_extension_dir(mut self, enable: bool) -> Self {
        self.include_user_dir = enable;
        self
    }
}

impl Plugin for DylibLoaderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadedDylibs>();
        app.init_resource::<GameCatalog>();

        let paths = self.collect_search_paths();
        if paths.is_empty() {
            info!("Dylib loader: no search paths configured");
            return;
        }

        let mut loaded = 0u32;
        let mut failed = 0u32;
        for file in walk_dylibs(&paths) {
            match try_load(app, &file) {
                Ok(LoadedKind::Extension(name)) => {
                    info!("Loaded extension `{name}` from {}", file.display());
                    loaded += 1;
                }
                Ok(LoadedKind::Game(name)) => {
                    info!("Loaded game `{name}` from {}", file.display());
                    loaded += 1;
                }
                Err(err) => {
                    warn!("Failed to load {}: {err}", file.display());
                    failed += 1;
                }
            }
        }

        match (loaded, failed) {
            (0, 0) => info!("Dylib loader: no dylibs found"),
            _ => info!("Dylib loader: {loaded} loaded, {failed} failed"),
        }
    }
}

/// Report from a successful `try_load` / `load_from_path`.
#[derive(Debug, Clone)]
pub enum LoadedKind {
    Extension(String),
    Game(String),
}

impl LoadedKind {
    pub fn name(&self) -> &str {
        match self {
            Self::Extension(n) | Self::Game(n) => n,
        }
    }
}

/// Peek at a dylib's entry symbol to classify it as an extension or
/// a game without wiring it into the editor. The caller uses this to
/// decide where to copy the file before installing. The library
/// handle returned by the internal `open_and_verify` is dropped at
/// the end of this call, so the peeked dylib is unloaded before it
/// gets reopened from its final destination.
pub fn peek_kind(path: &Path) -> Result<LoadedKind, LoadError> {
    match open_and_verify(path)? {
        OpenedDylib::Extension { ctor, .. } => Ok(LoadedKind::Extension(ctor().id())),
        OpenedDylib::Game { name, .. } => Ok(LoadedKind::Game(name)),
    }
}

impl DylibLoaderPlugin {
    fn collect_search_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if self.include_user_dir
            && let Some(config) = dirs::config_dir()
        {
            paths.push(config.join(DEFAULT_EXTENSIONS_SUBDIR));
            paths.push(config.join(DEFAULT_GAMES_SUBDIR));
        }
        if self.include_env_dir {
            if let Ok(env_path) = std::env::var(ENV_EXTENSIONS_PATH) {
                paths.push(PathBuf::from(env_path));
            }
            if let Ok(env_path) = std::env::var(ENV_GAMES_PATH) {
                paths.push(PathBuf::from(env_path));
            }
        }
        paths.extend(self.extra_paths.iter().cloned());
        paths
    }
}

/// Everything that can go wrong loading one extension dylib. Each
/// failure is reported per-file and does not stop the loader from
/// trying the rest.
#[derive(Debug)]
pub enum LoadError {
    Libloading(libloading::Error),
    EntryPanicked,
    Compat(CompatError),
    InvalidName,
    /// Non-dlopen failure, e.g., the install step's filesystem
    /// rename failed. Doesn't reach the library-loader itself but
    /// is surfaced through the same Result so call sites have a
    /// single error type to match on.
    InstallIo(String),
    Other(BevyError),
}

impl LoadError {
    /// `true` when the underlying `libloading` failure is the tell-
    /// tale signature of a stale cache: the dylib resolved
    /// successfully but its reference to a jackdaw SDK symbol
    /// couldn't be found, because the SDK was rebuilt after the
    /// dylib was last compiled.
    ///
    /// Callers (`project_select`, `hot_reload`) use this to trigger
    /// an auto-`cargo clean -p <crate>` + rebuild recovery path
    /// transparently, so the user never has to manually nuke their
    /// project target dir after an editor rebuild.
    ///
    /// Heuristic: looks for `undefined symbol` plus any jackdaw
    /// identifier in the formatted error string. Both pieces need
    /// to match to avoid classifying unrelated libloading failures
    /// (missing .so, malformed binary, etc.) as cache staleness.
    pub fn is_symbol_mismatch(&self) -> bool {
        let Self::Libloading(e) = self else {
            return false;
        };
        let msg = format!("{e}");
        msg.contains("undefined symbol")
            && (msg.contains("jackdaw")
                || msg.contains("teardown_tracked")
                || msg.contains("GameApp"))
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Libloading(e) => write!(f, "libloading: {e}"),
            Self::EntryPanicked => write!(f, "extension entry function panicked"),
            Self::Compat(e) => write!(f, "{e}"),
            Self::InvalidName => {
                write!(f, "extension name is not valid UTF-8 or contains NUL")
            }
            Self::InstallIo(msg) => write!(f, "install io: {msg}"),
            Self::Other(e) => write!(f, "other: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<libloading::Error> for LoadError {
    fn from(value: libloading::Error) -> Self {
        Self::Libloading(value)
    }
}

impl From<CompatError> for LoadError {
    fn from(value: CompatError) -> Self {
        Self::Compat(value)
    }
}

impl From<BevyError> for LoadError {
    fn from(value: BevyError) -> Self {
        Self::Other(value)
    }
}

fn walk_dylibs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in paths {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_dylib(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn is_dylib(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| matches!(ext, "so" | "dylib" | "dll"))
}

/// Result of a successfully-verified dylib open. The loader keeps
/// each variant's `Library` handle alive for the duration of the
/// `App`; dropping it while the entry code is still reachable is UB.
enum OpenedDylib {
    Extension {
        lib: libloading::Library,
        ctor: Box<dyn Fn() -> Box<dyn JackdawExtension> + Send + Sync>,
        #[expect(
            dead_code,
            reason = "Ideally we should clean up after ourselves, but the extension is a ZST anyways, so there's not really any data to leak anyways. Still, feel free to implement the destructor!"
        )]
        dtor: Box<dyn Fn(Box<dyn JackdawExtension>) + Send + Sync>,
    },
    Game {
        lib: libloading::Library,
        name: String,
        build: unsafe extern "C" fn(*mut bevy::ecs::world::World),
        teardown: unsafe extern "C" fn(*mut bevy::ecs::world::World),
    },
}

/// Try to open `path`, dispatching on which entry symbol it
/// exposes. Game symbol wins if both somehow exist (a cdylib
/// should only `export_game!` or `export_extension!`, not both).
fn open_and_verify(path: &Path) -> Result<OpenedDylib, LoadError> {
    // SAFETY: libloading's standard contract. Caller trusts `path`
    // is a well-formed dynamic library; if not, the call returns
    // `Err`. Extensions and games are trusted native code.
    let lib = unsafe { libloading::Library::new(path)? };

    // Try the game symbol first. If it's present, the dylib is a
    // game and we take that path. If absent (most dylibs), fall
    // through to the extension symbol.
    //
    // `lib.get` returns `Err` for missing symbols rather than
    // panicking, so this is safe to try speculatively.
    type GameEntryFn = unsafe extern "C" fn() -> GameEntry;
    if let Ok(game_sym) = unsafe { lib.get::<GameEntryFn>(GAME_ENTRY_SYMBOL) } {
        let entry = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: game_sym is guaranteed non-null by libloading's
            // successful lookup; calling convention matches the
            // declared prototype.
            unsafe { game_sym() }
        }))
        .map_err(|_| LoadError::EntryPanicked)?;

        compat::verify_game_compat(&entry)?;

        // SAFETY: `verify_game_compat` rejected null; the library
        // stays alive at least until `lib` is dropped by the caller.
        let name = unsafe { CStr::from_ptr(entry.name) }
            .to_str()
            .map_err(|_| LoadError::InvalidName)?
            .to_owned();

        return Ok(OpenedDylib::Game {
            lib,
            name,
            build: entry.build,
            teardown: entry.teardown,
        });
    }

    // SAFETY: the entry symbol has the signature declared by
    // `jackdaw_api_internal::ffi::ExtensionEntry`. Calling it is isolated
    // inside `catch_unwind` below.
    type EntryFn = unsafe extern "C" fn() -> ExtensionEntry;
    let entry_sym: libloading::Symbol<EntryFn> = unsafe { lib.get(ENTRY_SYMBOL)? };

    let entry = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: entry_sym is guaranteed non-null by libloading's
        // successful lookup; calling convention matches the
        // declared prototype.
        unsafe { entry_sym() }
    }))
    .map_err(|_| LoadError::EntryPanicked)?;

    compat::verify_compat(&entry)?;

    let construct_extension = move || -> Box<dyn JackdawExtension> {
        // SAFETY: the dylib stays loaded because its
        // Library handle is kept in LoadedDylibs;
        // `verify_compat` asserted the ABI contract at
        // load time mooore or less (see next comment),
        //  and the ctor pointer itself is just a plain function pointer.
        let raw = unsafe { (entry.ctor)() };
        // SAFETY: we control the export site, which exports the pointer of
        // a `dyn JackdawExtension` trait object, so we can "safely" transmute
        // the pointer. This is not really safe, since there's no guarantee
        // that the ABI is preserved across compiler invocations, but good enough for now
        unsafe {
            let ptr: *mut dyn JackdawExtension = std::mem::transmute(raw);
            Box::from_raw(ptr)
        }
    };

    let destruct_extension = move |ext: Box<dyn JackdawExtension>| {
        // SAFETY: we control the export site, which exports the pointer of
        // a `dyn JackdawExtension` trait object, so we can "safely" transmute
        // the pointer. This is not really safe, since there's no guarantee
        // that the ABI is preserved across compiler invocations, but good enough for now
        let raw = unsafe {
            std::mem::transmute::<*mut dyn JackdawExtension, JackdawExtensionPtr>(Box::into_raw(
                ext,
            ))
        };
        // SAFETY: the dylib stays loaded because its
        // Library handle is kept in LoadedDylibs;
        // `verify_compat` asserted the ABI contract at
        // load time mooore or less (see next comment),
        //  and the dtor pointer itself is just a plain function pointer.
        unsafe { (entry.dtor)(raw) }
    };

    Ok(OpenedDylib::Extension {
        lib,
        ctor: Box::new(construct_extension),
        dtor: Box::new(destruct_extension),
    })
}

fn try_load(app: &mut App, path: &Path) -> Result<LoadedKind, LoadError> {
    // Open the dylib first. `lib` is the libloading handle that
    // keeps the code mapped for the life of the process; we look
    // up any extra symbols (like the reflect-register fn) on it
    // while still holding it on our side of ownership.
    let lib_and_kind = open_and_verify_keep_lib(path)?;
    match lib_and_kind {
        (lib, OpenedKind::Extension { ctor }) => {
            // Run the per-dylib reflect registrar against our registry
            // BEFORE handing the library off. Uses the exported
            // `REFLECT_REGISTER_SYMBOL` (if present); absent on older
            // dylibs, treated as a no-op.
            call_reflect_register_symbol(app.world_mut(), &lib);

            // Construct once to harvest kind + run its one-time BEI
            // input-context registration; then store the ctor in the
            // catalog so subsequent enable/disable cycles rebuild the
            // extension fresh each time.
            let ext = ctor();
            ext.register_input_context(app);
            let id = ext.id();
            drop(ext);

            jackdaw_api_internal::lifecycle::register_dylib_extension(app.world_mut(), ctor);

            app.world_mut()
                .resource_mut::<LoadedDylibs>()
                .libs
                .push(lib);

            // Ensure every reflected Component has a ComponentId so
            // the Add Component picker finds it without waiting for
            // the game to start querying.
            register_derived_component_ids(app.world_mut());

            Ok(LoadedKind::Extension(id))
        }
        (
            lib,
            OpenedKind::Game {
                name,
                build,
                teardown,
            },
        ) => {
            // Register the game's `#[derive(Reflect)]` types into our
            // AppTypeRegistry via the exported symbol, then assign
            // ComponentIds so the inspector sees them before build()
            // runs any systems that query them.
            call_reflect_register_symbol(app.world_mut(), &lib);
            register_derived_component_ids(app.world_mut());

            // v2 builds take `*mut World`. Call from an exclusive
            // context: we have `&mut App` here, so deriving
            // `&mut World` via `world_mut()` is fine.
            let world_ptr: *mut bevy::ecs::world::World = std::ptr::from_mut(app.world_mut());
            let build_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: `build` is a function pointer from a
                // compat-verified dylib; `world_ptr` is a valid
                // mutable reference for the duration of this call;
                // the dylib stays alive via LoadedDylibs.
                unsafe { build(world_ptr) }
            }));
            if build_result.is_err() {
                // Build panicked. The library is still loaded; keep
                // it alive rather than risk UB unloading partially-
                // executed init code.
                app.world_mut()
                    .resource_mut::<LoadedDylibs>()
                    .libs
                    .push(lib);
                return Err(LoadError::EntryPanicked);
            }

            // Record build + teardown fn pointers in the GameCatalog
            // so hot-reload can find them later.
            {
                let mut catalog = app.world_mut().resource_mut::<GameCatalog>();
                catalog.games.push(name.clone());
                catalog
                    .entries
                    .insert(name.clone(), LoadedGameEntry { build, teardown });
            }
            app.world_mut()
                .resource_mut::<LoadedDylibs>()
                .libs
                .push(lib);

            Ok(LoadedKind::Game(name))
        }
    }
}

/// Load a dylib at runtime from a `&mut World` context.
///
/// Requires the host binary to have been built with `jackdaw`'s
/// `dylib` feature (which pulls in `jackdaw_api/dynamic_linking`)
/// so both sides share one compiled copy of the jackdaw types.
/// Without that, `ExtensionContext::register_window` and similar
/// calls panic because the host keyed resources under different
/// `TypeId`s than the dylib sees.
///
/// Mirrors the startup loader path but skips the BEI input-context
/// registration that requires `&mut App`. In practice that means:
///
/// * Windows, operators, menu entries, and panel-extension sections
///   activate immediately.
/// * BEI keybinds declared via `add_input_context::<C>()` do **not**
///   activate until the editor restarts and picks the dylib up
///   through the normal [`DylibLoaderPlugin`] startup path.
///
/// The constructor is inserted into [`jackdaw_api_internal::ExtensionCatalog`]
/// so the Extensions dialog's enable/disable toggle can reuse it, and
/// the `Library` handle is moved into [`LoadedDylibs`] so the entry
/// point stays valid for the rest of the app's life.
///
/// Returns the loaded kind (Extension or Game) on success.
pub fn load_from_path(world: &mut World, path: &Path) -> Result<LoadedKind, LoadError> {
    let lib_and_kind = open_and_verify_keep_lib(path)?;
    match lib_and_kind {
        (lib, OpenedKind::Extension { ctor }) => {
            let ext = ctor();
            let id = ext.id();
            // Already-registered extensions come through this path
            // when the user re-installs a rebuild. Don't double-
            // register; registering the same extension twice produces
            // duplicate windows/operators and a phantom second
            // catalog entry.
            if world
                .resource::<jackdaw_api_internal::ExtensionCatalog>()
                .contains(&id)
            {
                info!(
                    "Extension `{id}` already registered; keeping the new library handle \
                     alive but skipping re-registration."
                );
                // Re-run reflect register in case the rebuild added
                // new types to the existing extension.
                call_reflect_register_symbol(world, &lib);
                register_derived_component_ids(world);
                world.resource_mut::<LoadedDylibs>().libs.push(lib);
                return Ok(LoadedKind::Extension(id));
            }

            call_reflect_register_symbol(world, &lib);
            register_derived_component_ids(world);

            jackdaw_api_internal::lifecycle::register_dylib_extension(world, ctor);

            jackdaw_api_internal::lifecycle::load_static_extension(world, ext);

            world.resource_mut::<LoadedDylibs>().libs.push(lib);

            Ok(LoadedKind::Extension(id))
        }
        (
            lib,
            OpenedKind::Game {
                name,
                build,
                teardown,
            },
        ) => {
            let already_loaded = world
                .resource::<GameCatalog>()
                .games
                .iter()
                .any(|n| n == &name);
            if already_loaded {
                let prior = world.resource::<GameCatalog>().entries.get(&name).copied();
                if let Some(prior_entry) = prior {
                    info!("Hot reload: tearing down prior version of `{name}`");
                    let world_ptr: *mut bevy::ecs::world::World = std::ptr::from_mut(world);
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
                        (prior_entry.teardown)(world_ptr);
                    }));
                }
            }

            // Register reflect types + ComponentIds BEFORE build().
            // build() adds systems that may query game components;
            // the Add Component picker needs ComponentIds immediately.
            call_reflect_register_symbol(world, &lib);
            register_derived_component_ids(world);

            let world_ptr: *mut bevy::ecs::world::World = std::ptr::from_mut(world);
            let build_result =
                std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { build(world_ptr) }));
            if build_result.is_err() {
                warn!("load_from_path: build panicked for `{name}`");
                world.resource_mut::<LoadedDylibs>().libs.push(lib);
                return Err(LoadError::EntryPanicked);
            }

            {
                let mut catalog = world.resource_mut::<GameCatalog>();
                if !catalog.games.iter().any(|n| n == &name) {
                    catalog.games.push(name.clone());
                }
                catalog
                    .entries
                    .insert(name.clone(), LoadedGameEntry { build, teardown });
            }
            world.resource_mut::<LoadedDylibs>().libs.push(lib);

            Ok(LoadedKind::Game(name))
        }
    }
}

/// Opened-dylib payload without carrying the `Library` handle. Paired
/// with the handle at the call site so the caller can both move the
/// library into `LoadedDylibs` at the right moment and look up symbols
/// on it in the meantime.
enum OpenedKind {
    Extension {
        ctor: Box<dyn Fn() -> Box<dyn JackdawExtension> + Send + Sync>,
    },
    Game {
        name: String,
        build: unsafe extern "C" fn(*mut bevy::ecs::world::World),
        teardown: unsafe extern "C" fn(*mut bevy::ecs::world::World),
    },
}

/// dlopen + verify-compat + read entry, returning the library handle
/// separately so callers can look up extra symbols (like the reflect-
/// register FFI symbol) on the loaded library before moving the handle
/// into `LoadedDylibs`.
fn open_and_verify_keep_lib(path: &Path) -> Result<(libloading::Library, OpenedKind), LoadError> {
    match open_and_verify(path)? {
        OpenedDylib::Extension { lib, ctor, .. } => Ok((lib, OpenedKind::Extension { ctor })),
        OpenedDylib::Game {
            lib,
            name,
            build,
            teardown,
        } => Ok((
            lib,
            OpenedKind::Game {
                name,
                build,
                teardown,
            },
        )),
    }
}

/// Look up the per-dylib reflect-registrar symbol and, if present, run
/// it against the host's `AppTypeRegistry`. Missing symbol is a no-op
/// (backward-compatible with extensions that pre-date this FFI point).
///
/// The symbol is `jackdaw_register_reflect_types_v1` (see
/// [`jackdaw_api_internal::ffi::REFLECT_REGISTER_SYMBOL`]). Each cdylib's
/// `build.rs` generates its body with explicit
/// `registry.register::<T>()` calls for every `#[derive(Reflect)]`
/// type in the crate, and the `export_game!` / `export_extension!`
/// macros emit the `#[unsafe(no_mangle)] extern "Rust"` wrapper.
fn call_reflect_register_symbol(world: &mut World, lib: &libloading::Library) {
    use jackdaw_api_internal::ffi::{REFLECT_REGISTER_SYMBOL, ReflectRegisterFn};

    let Some(registry_res) = world.get_resource::<bevy::ecs::reflect::AppTypeRegistry>() else {
        debug!("reflect-register: AppTypeRegistry missing, skipping");
        return;
    };
    let registry_handle = registry_res.clone();

    let reg_sym: libloading::Symbol<ReflectRegisterFn> = match unsafe {
        lib.get(REFLECT_REGISTER_SYMBOL)
    } {
        Ok(s) => s,
        Err(e) => {
            debug!("reflect-register: symbol lookup failed: {e}; dylib has no types to register");
            return;
        }
    };

    let before = registry_handle.read().iter().count();
    let mut guard = registry_handle.write();
    let call_result = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { reg_sym(&mut guard) }));
    let after = guard.iter().count();
    drop(guard);
    if call_result.is_err() {
        warn!("reflect-register symbol panicked; types may be missing from the registry");
    } else {
        debug!(
            "reflect-register: called. Registry size {before} -> {after} \
             ({} new type entries)",
            after.saturating_sub(before)
        );
    }
}

/// Ensure every `Component`-reflecting type in `AppTypeRegistry` has a
/// bevy `ComponentId` assigned. Without this sweep, a newly-loaded
/// game's components stay invisible to the Add Component picker
/// (`src/inspector/component_picker.rs:108`) until something spawns or
/// queries them, which for game components won't happen until Play is
/// pressed.
///
/// [`ReflectComponent::register_component`] is idempotent, so this sweep
/// is safe to run on every dlopen.
fn register_derived_component_ids(world: &mut World) {
    let reflect_components: Vec<bevy::ecs::reflect::ReflectComponent> = {
        let registry = world
            .resource::<bevy::ecs::reflect::AppTypeRegistry>()
            .read();
        registry
            .iter()
            .filter_map(|r| r.data::<bevy::ecs::reflect::ReflectComponent>().cloned())
            .collect()
    };
    for rc in &reflect_components {
        rc.register_component(world);
    }
    debug!(
        "register_derived_component_ids: ensured {} ComponentIds registered",
        reflect_components.len()
    );
}
