//! Runtime discovery and loading of Jackdaw extension dylibs.
//!
//! # Overview
//!
//! Add [`DylibLoaderPlugin`] to the editor `App`. During `build` it
//! walks every configured search path, opens each dynamic library
//! with `libloading`, drains the reflect types the dylib queued
//! during dlopen, looks up the plain-Rust `jackdaw_extension_ctor`
//! symbol (see [`EXTENSION_CTOR_SYMBOL`]), and registers the
//! extension through
//! [`jackdaw_api_internal::lifecycle::register_dylib_extension`].
//!
//! Loaded libraries live in [`LoadedDylibs`] as long as the `App`
//! lives. Unloading a library while systems still reference code
//! inside it is UB, so libraries are only dropped when the `App` is
//! destroyed.
//!
//! # Search paths
//!
//! By default the loader searches the per-user config directory
//! (`~/.config/jackdaw/extensions/` and platform equivalents). The
//! `JACKDAW_EXTENSIONS_DIR` environment variable adds another path.
//! Callers can add their own via [`DylibLoaderPlugin::extra_paths`].
//!
//! # Compatibility
//!
//! Host and extension share one Rust ABI because both link the same
//! SDK dylib (`libjackdaw_sdk.so`). Callers verify that identity with
//! linkage verification (`jackdaw::project_build::linkage`) BEFORE
//! handing a path to this crate; the check needs rustc, which lives
//! on the editor side, so this crate performs no version checks of
//! its own. A panic in the extension constructor is caught via
//! `catch_unwind`, but a segfault in extension code takes the
//! process down.

pub mod quarantine;

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api_internal::JackdawExtension;

/// Sub-directory inside the platform config directory where the
/// loader looks for per-user extensions (editor tools, panels,
/// operators).
pub const DEFAULT_EXTENSIONS_SUBDIR: &str = "jackdaw/extensions";

/// Sub-directory for per-user game dylibs. Games run out of process
/// via the runner, so the loader never scans this directory; the
/// constant remains for the install watcher and legacy cleanup.
pub const DEFAULT_GAMES_SUBDIR: &str = "jackdaw/games";

/// Prefix used by the install flow's atomic-rename tempfile. The
/// extension watcher skips paths starting with this prefix so
/// our own in-flight renames don't trip "Dylib changed on disk"
/// warnings. Shared here rather than duplicated in
/// `extensions_dialog::install_picked_file` and `extension_watcher`
/// so the two can't drift.
pub const INSTALL_TEMPFILE_PREFIX: &str = ".jackdaw-install-";

/// Environment variable whose value, if set to a directory path,
/// is added to the loader's search paths at startup for extensions.
pub const ENV_EXTENSIONS_PATH: &str = "JACKDAW_EXTENSIONS_DIR";

/// Environment variable naming a per-user games directory. Only the
/// install watcher observes it; the loader does not scan it.
pub const ENV_GAMES_PATH: &str = "JACKDAW_GAMES_DIR";

/// Back-compat alias for `ENV_EXTENSIONS_PATH`. Older docs and
/// scripts reference this name; prefer the split env vars above.
#[deprecated(note = "use ENV_EXTENSIONS_PATH or ENV_GAMES_PATH")]
pub const ENV_SEARCH_PATH: &str = ENV_EXTENSIONS_PATH;

/// Symbol every extension dylib exports: a plain-Rust
/// `fn() -> Box<dyn JackdawExtension>`. Editor-generated shims emit
/// it with `#[unsafe(no_mangle)]`. Includes the trailing NUL so it
/// can be passed directly to `libloading::Library::get`.
pub const EXTENSION_CTOR_SYMBOL: &[u8] = b"jackdaw_extension_ctor\0";

/// Signature of [`EXTENSION_CTOR_SYMBOL`]. A plain Rust fn is sound
/// because host and dylib share one compilation of
/// `JackdawExtension` through the SDK dylib.
type ExtensionCtor = fn() -> Box<dyn JackdawExtension>;

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

        let paths = self.collect_search_paths();
        if paths.is_empty() {
            info!("Dylib loader: no search paths configured");
            return;
        }

        let mut loaded = 0u32;
        let mut failed = 0u32;
        for file in walk_dylibs(&paths) {
            match try_load(app, &file) {
                Ok(id) => {
                    info!("Loaded extension `{id}` from {}", file.display());
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

impl DylibLoaderPlugin {
    fn collect_search_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if self.include_user_dir
            && let Some(config) = dirs::config_dir()
        {
            paths.push(config.join(DEFAULT_EXTENSIONS_SUBDIR));
        }
        if self.include_env_dir
            && let Ok(env_path) = std::env::var(ENV_EXTENSIONS_PATH)
        {
            paths.push(PathBuf::from(env_path));
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
    /// The file dlopened cleanly but exports no
    /// `jackdaw_extension_ctor` symbol, so it is not a Jackdaw
    /// extension.
    NoExtensionEntry,
    EntryPanicked,
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
            Self::NoExtensionEntry => write!(
                f,
                "the dylib exposes no extension entry \
                 (`jackdaw_extension_ctor` symbol not found)"
            ),
            Self::EntryPanicked => write!(f, "extension constructor panicked"),
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

/// dlopen `path`, keep the handle alive in [`LoadedDylibs`], drain
/// the reflect registrations its constructors queued, resolve the
/// extension ctor symbol, and construct one instance.
///
/// The library handle is moved into `LoadedDylibs` on every path
/// past a successful dlopen, including the error paths: dlopen has
/// already run the dylib's constructors and the drained type
/// registrations point into its code, so unloading is never safe.
fn open_and_construct(
    world: &mut World,
    path: &Path,
) -> Result<(ExtensionCtor, Box<dyn JackdawExtension>), LoadError> {
    // SAFETY: libloading's standard contract. Extensions are trusted
    // native code; the caller verified linkage identity against the
    // running SDK before handing us the path.
    let lib = unsafe { libloading::Library::new(path)? };

    drain_reflect_registrations(world);

    // SAFETY: the symbol's signature is fixed by the shim contract,
    // and both sides compile it against the one `JackdawExtension`
    // definition inside the shared SDK dylib.
    let ctor: ExtensionCtor = match unsafe { lib.get::<ExtensionCtor>(EXTENSION_CTOR_SYMBOL) } {
        Ok(sym) => *sym,
        Err(_) => {
            world.resource_mut::<LoadedDylibs>().libs.push(lib);
            return Err(LoadError::NoExtensionEntry);
        }
    };

    world.resource_mut::<LoadedDylibs>().libs.push(lib);

    #[expect(
        clippy::disallowed_methods,
        reason = "the ctor runs third-party code; catch_unwind is \
                  best-effort isolation under panic=unwind"
    )]
    let ext =
        std::panic::catch_unwind(AssertUnwindSafe(ctor)).map_err(|_| LoadError::EntryPanicked)?;
    Ok((ctor, ext))
}

/// Drain type registrations queued by dlopen into the world's
/// `AppTypeRegistry`, then assign `ComponentId`s so pickers and the
/// inspector see the new components immediately. dlopen runs the
/// dylib's static constructors, which submit its
/// `#[derive(Reflect)]` types to the shared SDK's
/// `reflect_auto_register` queue.
fn drain_reflect_registrations(world: &mut World) {
    if let Some(registry) = world.get_resource::<bevy::ecs::reflect::AppTypeRegistry>() {
        registry.write().register_derived_types();
    } else {
        debug!("drain_reflect_registrations: AppTypeRegistry missing, skipping");
        return;
    }
    register_derived_component_ids(world);
}

/// Startup-scan load: construct once to harvest the extension id and
/// run its one-time BEI input-context registration (needs `&mut
/// App`), then store the ctor in the catalog so enable/disable
/// cycles rebuild the extension fresh each time.
fn try_load(app: &mut App, path: &Path) -> Result<String, LoadError> {
    let (ctor, ext) = open_and_construct(app.world_mut(), path)?;
    ext.register_input_context(app);
    let id = ext.id();
    drop(ext);

    jackdaw_api_internal::lifecycle::register_dylib_extension(app.world_mut(), ctor);
    Ok(id)
}

/// Load an extension dylib at runtime from a `&mut World` context.
///
/// Requires the host binary to have been built with `jackdaw`'s
/// `dylib` feature (which pulls in `jackdaw_api/dynamic_linking`)
/// so both sides share one compiled copy of the jackdaw types.
/// Without that, `ExtensionContext::register_window` and similar
/// calls panic because the host keyed resources under different
/// `TypeId`s than the dylib sees.
///
/// Same shape as the startup loader path but skips the BEI
/// input-context registration that requires `&mut App`. In practice:
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
/// Returns the extension id on success.
pub fn load_from_path(world: &mut World, path: &Path) -> Result<String, LoadError> {
    let (ctor, ext) = open_and_construct(world, path)?;
    let id = ext.id();

    // Already-registered extensions come through this path when the
    // user re-installs a rebuild. Don't double-register; registering
    // the same extension twice produces duplicate windows/operators
    // and a phantom second catalog entry.
    if world
        .resource::<jackdaw_api_internal::ExtensionCatalog>()
        .contains(&id)
    {
        info!(
            "Extension `{id}` already registered; keeping the new library handle \
             alive but skipping re-registration."
        );
        return Ok(id);
    }

    jackdaw_api_internal::lifecycle::register_dylib_extension(world, ctor);
    jackdaw_api_internal::lifecycle::load_static_extension(world, ext);

    Ok(id)
}

/// Ensure every `Component`-reflecting type in `AppTypeRegistry` has a
/// bevy `ComponentId` assigned. Without this sweep, a newly-loaded
/// dylib's components stay invisible to the Add Component picker
/// (`src/inspector/component_picker.rs:108`) until something spawns or
/// queries them.
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
