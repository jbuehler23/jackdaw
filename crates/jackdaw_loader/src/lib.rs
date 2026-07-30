//! Runtime loading of signed, installed Jackdaw extensions.
//!
//! # Overview
//!
//! Add [`DylibLoaderPlugin`] to the editor `App`. During `build` it
//! reads the versioned `.jdext` installation index, opens each verified
//! dynamic library with `libloading`, and looks up `jackdaw_extension_ctor`
//! symbol (see [`EXTENSION_CTOR_SYMBOL`]), and registers the
//! extension through
//! [`jackdaw_api_internal::lifecycle::register_dylib_extension`].
//!
//! Loaded libraries live in [`LoadedDylibs`] as long as the `App`
//! lives. Unloading a library while systems still reference code
//! inside it is UB, so libraries are only dropped when the `App` is
//! destroyed.
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

pub mod package;
pub mod quarantine;

use std::panic::AssertUnwindSafe;
use std::path::Path;

use bevy::prelude::*;
use jackdaw_api_internal::JackdawExtension;

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

#[derive(Resource, Default)]
struct PendingActivationGuards(Vec<quarantine::QuarantineGuard>);

impl LoadedDylibs {
    pub fn len(&self) -> usize {
        self.libs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.libs.is_empty()
    }
}

/// Load active extensions from the signed, versioned installation index.
///
/// Dynamic-library extensions require the host binary to be
/// built with `bevy/dynamic_linking` so the editor and every
/// loaded extension share one copy of Bevy at runtime. Without
/// that, trait-object calls across the dylib boundary are
/// unsound.
///
#[derive(Default)]
pub struct DylibLoaderPlugin;

impl Plugin for DylibLoaderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadedDylibs>()
            .init_resource::<PendingActivationGuards>()
            .add_systems(Last, disarm_survived_activations);
        let quarantine = dirs::state_dir()
            .map(|path| path.join("jackdaw/extension-quarantine"))
            .and_then(|path| quarantine::Quarantine::open(path).ok());

        // A sentinel left by a dead process identifies the active version that
        // crashed during its first frame. Restore the retired version before
        // garbage collection can delete it.
        let mut recovery_failed = false;
        if let Some(quarantine) = &quarantine
            && let Ok(installed) = package::list_installed()
        {
            for extension in installed {
                if !quarantine.is_quarantined(&extension.library_path) {
                    continue;
                }
                let id = &extension.manifest.id;
                match package::rollback(id) {
                    Ok(Some(previous)) => {
                        warn!(
                            "Extension `{id}` crashed during activation; restored {}",
                            previous.manifest.version
                        );
                        quarantine.clear(&extension.library_path);
                    }
                    Ok(None) => match package::uninstall(id) {
                        Ok(_) => {
                            warn!(
                                "Extension `{id}` crashed during first activation and was disabled"
                            );
                            jackdaw_api_internal::extensions_config::set_extension_enabled(
                                id, false,
                            );
                            quarantine.clear(&extension.library_path);
                        }
                        Err(error) => {
                            warn!("Could not disable crashed extension `{id}`: {error}");
                            recovery_failed = true;
                        }
                    },
                    Err(error) => {
                        warn!("Could not roll back crashed extension `{id}`: {error}");
                        recovery_failed = true;
                    }
                }
            }
        }
        if !recovery_failed && let Err(error) = package::garbage_collect() {
            warn!("Extension garbage collection failed: {error}");
        }

        let mut loaded = 0u32;
        let mut failed = 0u32;
        let installed = match package::list_installed() {
            Ok(installed) => installed,
            Err(error) => {
                warn!("Could not read installed extensions: {error}");
                return;
            }
        };
        for extension in installed {
            let file = extension.library_path;
            if quarantine
                .as_ref()
                .is_some_and(|quarantine| quarantine.is_quarantined(&file))
            {
                warn!(
                    "Skipped quarantined extension `{}` at {}",
                    extension.manifest.id,
                    file.display()
                );
                failed += 1;
                continue;
            }
            let guard = quarantine
                .as_ref()
                .and_then(|quarantine| quarantine.arm(&file).ok());
            match try_load(app, &file) {
                Ok(id) => {
                    info!("Loaded extension `{id}` from {}", file.display());
                    if let Some(guard) = guard {
                        app.world_mut()
                            .resource_mut::<PendingActivationGuards>()
                            .0
                            .push(guard);
                    }
                    loaded += 1;
                }
                Err(err) => {
                    warn!("Failed to load {}: {err}", file.display());
                    failed += 1;
                }
            }
        }

        match (loaded, failed) {
            (0, 0) => info!("Dylib loader: no installed extensions"),
            _ => info!("Dylib loader: {loaded} loaded, {failed} failed"),
        }
    }
}

fn disarm_survived_activations(mut pending: ResMut<PendingActivationGuards>) {
    for guard in pending.0.drain(..) {
        guard.disarm();
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

/// dlopen `path`, keep the handle alive in [`LoadedDylibs`], resolve the
/// extension ctor symbol, and construct one instance.
///
/// The library handle is moved into `LoadedDylibs` on every path
/// past a successful dlopen, including error paths. Superseded mappings
/// remain inert until process exit; unloading native Rust code is unsafe.
fn open_and_construct(
    world: &mut World,
    path: &Path,
) -> Result<(ExtensionCtor, Box<dyn JackdawExtension>), LoadError> {
    // SAFETY: libloading's standard contract. Extensions are trusted
    // native code; the caller verified linkage identity against the
    // running SDK before handing us the path.
    let lib = unsafe { libloading::Library::new(path)? };

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

/// Startup load: construct once to harvest the extension id, then store the
/// constructor in the catalog. Marketplace extensions use host-owned keymap
/// registrations and do not mutate startup-only Bevy type registries.
fn try_load(app: &mut App, path: &Path) -> Result<String, LoadError> {
    let (ctor, ext) = open_and_construct(app.world_mut(), path)?;
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
/// The constructor is inserted into [`jackdaw_api_internal::ExtensionCatalog`]
/// so the Extensions dialog's enable/disable toggle can reuse it, and
/// the `Library` handle is moved into [`LoadedDylibs`] so the entry
/// point stays valid for the rest of the app's life.
///
/// Returns the extension id on success.
pub fn load_from_path(world: &mut World, path: &Path) -> Result<String, LoadError> {
    let (ctor, ext) = open_and_construct(world, path)?;
    let id = ext.id();

    // Logical unload is immediate. Every host-owned registration is removed,
    // while LoadedDylibs deliberately retains the superseded mapping.
    if world
        .resource::<jackdaw_api_internal::ExtensionCatalog>()
        .contains(&id)
    {
        jackdaw_api_internal::lifecycle::disable_extension(world, &id);
    }

    jackdaw_api_internal::lifecycle::register_dylib_extension(world, ctor);
    #[expect(
        clippy::disallowed_methods,
        reason = "extension registration is third-party native code; unwind is \
                  converted into an activation failure so the caller can roll back"
    )]
    let activated = std::panic::catch_unwind(AssertUnwindSafe(|| {
        jackdaw_api_internal::lifecycle::load_static_extension(world, ext);
    }));
    if activated.is_err() {
        jackdaw_api_internal::lifecycle::disable_extension(world, &id);
        jackdaw_api_internal::lifecycle::unregister_dylib_extension(world, &id);
        return Err(LoadError::EntryPanicked);
    }

    Ok(id)
}

/// Load an installed bundle's library with a crash sentinel held through the
/// end of the current frame. A hard process crash leaves the sentinel for
/// startup rollback; ordinary errors clear it immediately.
pub fn load_installed_from_path(world: &mut World, path: &Path) -> Result<String, LoadError> {
    let quarantine = dirs::state_dir()
        .map(|state| state.join("jackdaw/extension-quarantine"))
        .and_then(|dir| quarantine::Quarantine::open(dir).ok());
    let guard = quarantine
        .as_ref()
        .and_then(|quarantine| quarantine.arm(path).ok());
    let result = load_from_path(world, path);
    match result {
        Ok(id) => {
            if let Some(guard) = guard {
                world
                    .get_resource_or_init::<PendingActivationGuards>()
                    .0
                    .push(guard);
            }
            Ok(id)
        }
        Err(error) => {
            if let Some(quarantine) = quarantine {
                quarantine.clear(path);
            }
            Err(error)
        }
    }
}
