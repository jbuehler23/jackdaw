//! Watch extension dylib directories for `.so` / `.dylib` / `.dll`
//! changes and surface them to the user.
//!
//! For now this is an observability feature, not a hot-reload
//! mechanism: when a dylib in a search path is rewritten (e.g. the
//! user rebuilt their extension or game), we log a warning telling
//! them to restart jackdaw to pick up the new code. True in-process
//! reload is tracked as a separate task because it requires draining
//! systems the old dylib registered and reconciling bevy resources
//! across the transition.
//!
//! The watcher honours the same search paths as
//! [`jackdaw_loader::DylibLoaderPlugin`]: per-user config dir plus
//! `JACKDAW_EXTENSIONS_DIR`.
//!
//! Logging happens on notify's background thread directly; bevy's
//! `warn!` is just a `tracing::warn!` and its subscriber is thread-
//! safe. This sidesteps bevy's Update-schedule throttling (the main
//! loop can coalesce frames when the window is unfocused, which
//! otherwise delays or skips our notifications).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw_loader::{
    DEFAULT_EXTENSIONS_SUBDIR, DEFAULT_GAMES_SUBDIR, ENV_EXTENSIONS_PATH, ENV_GAMES_PATH,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Installs the extension-dylib watcher.
pub struct ExtensionWatcherPlugin;

impl Plugin for ExtensionWatcherPlugin {
    fn build(&self, app: &mut App) {
        let paths = collect_search_paths();
        if paths.is_empty() {
            return;
        }

        let debounce = Arc::new(Mutex::new(Debounce::new(Duration::from_millis(500))));
        let debounce_for_cb = Arc::clone(&debounce);

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else { return };
            if !is_dylib_event(&event) {
                return;
            }
            for path in event.paths {
                if !is_dylib(&path) {
                    continue;
                }
                let should_emit = debounce_for_cb
                    .lock()
                    .map(|mut d| d.should_emit(&path))
                    .unwrap_or(false);
                if should_emit {
                    warn!(
                        "Dylib changed on disk: {}. Restart jackdaw to pick up the new code.",
                        path.display()
                    );
                }
            }
        });

        let Ok(mut watcher) = watcher else {
            warn!("ExtensionWatcher: could not create watcher; hot-reload notifications disabled");
            return;
        };

        let mut watched_any = false;
        for path in &paths {
            if !path.is_dir() {
                continue;
            }
            match watcher.watch(path, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    info!("Watching for dylib changes in {}", path.display());
                    watched_any = true;
                }
                Err(e) => warn!("Failed to watch {}: {e}", path.display()),
            }
        }

        if !watched_any {
            return;
        }

        // Keep the watcher + debounce alive for the App's lifetime.
        // The resource is never read; dropping it would stop the
        // watcher thread.
        app.insert_resource(WatcherHandle {
            _watcher: watcher,
            _debounce: debounce,
        });
    }
}

/// Owns the `notify::RecommendedWatcher` and the shared debounce
/// state. Dropping this resource stops the watcher. We never read
/// from it; the callback handles logging directly.
#[derive(Resource)]
struct WatcherHandle {
    _watcher: RecommendedWatcher,
    _debounce: Arc<Mutex<Debounce>>,
}

fn collect_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config) = dirs::config_dir() {
        paths.push(config.join(DEFAULT_EXTENSIONS_SUBDIR));
        paths.push(config.join(DEFAULT_GAMES_SUBDIR));
    }
    if let Ok(env_path) = std::env::var(ENV_EXTENSIONS_PATH) {
        paths.push(PathBuf::from(env_path));
    }
    if let Ok(env_path) = std::env::var(ENV_GAMES_PATH) {
        paths.push(PathBuf::from(env_path));
    }
    paths
}

fn is_dylib(path: &Path) -> bool {
    // Skip our own atomic-rename tempfiles; the install flow writes
    // to `<subdir>/.jackdaw-install-<pid>-<name>.so` and then renames
    // into place, and we don't want those intermediate writes to
    // fire the user-facing "Dylib changed on disk" warning.
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with(jackdaw_loader::INSTALL_TEMPFILE_PREFIX))
    {
        return false;
    }
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| matches!(ext, "so" | "dylib" | "dll"))
}

fn is_dylib_event(event: &Event) -> bool {
    // Cargo atomically renames the new dylib into place on some
    // platforms and overwrites in place on others; linkers and file
    // managers also have their own event signatures. Rather than
    // enumerate every variant, accept any Create or Modify and let
    // the Debounce collapse the burst. Explicitly skip pure
    // attribute changes (chmod / chown) and Access events; neither
    // implies new code is available.
    matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
        && !matches!(
            event.kind,
            EventKind::Modify(notify::event::ModifyKind::Metadata(_))
        )
}

/// Collapses a burst of events on the same path into a single
/// notification. Cargo writes + renames fire multiple events per
/// artifact; without debouncing we'd log the same dylib two or
/// three times per rebuild.
struct Debounce {
    window: Duration,
    last: Option<(PathBuf, Instant)>,
}

impl Debounce {
    fn new(window: Duration) -> Self {
        Self { window, last: None }
    }

    fn should_emit(&mut self, path: &Path) -> bool {
        let now = Instant::now();
        if let Some((last_path, last_at)) = &self.last
            && last_path == path
            && now.duration_since(*last_at) < self.window
        {
            return false;
        }
        self.last = Some((path.to_path_buf(), now));
        true
    }
}
