//! The editor's log file, for builds that run without a console.
//!
//! A release build on Windows runs as a GUI application, so nothing
//! reads its stderr. It writes the log, panics included, to
//! [`log_file`] instead, keeping the previous run's log beside it.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bevy::log::tracing::Subscriber;
use bevy::log::tracing_subscriber::fmt::{self, format::DefaultFields, format::Format};
use bevy::log::tracing_subscriber::registry::LookupSpan;

/// Where the editor writes its log when it has no console.
pub fn log_file() -> Option<PathBuf> {
    jackdaw_env::paths::local_data_dir().map(|dir| dir.join("logs").join("editor.log"))
}

/// A log layer writing plain text to `path`. The log already there is
/// renamed to `editor.previous.log` first, so a relaunch after a crash
/// keeps the crash's log.
pub fn file_layer<S>(
    path: &Path,
) -> std::io::Result<fmt::Layer<S, DefaultFields, Format, Mutex<File>>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let _ = std::fs::rename(path, path.with_extension("previous.log"));
    }
    let file = File::create(path)?;
    Ok(fmt::layer().with_ansi(false).with_writer(Mutex::new(file)))
}

/// Reports panics through the log as well as the hook already installed.
pub fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        bevy::log::error!("{info}\n{backtrace}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::log::tracing_subscriber::{Registry, layer::SubscriberExt};

    #[test]
    fn the_log_goes_to_the_file_and_the_last_run_is_kept() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("logs").join("editor.log");
        for run in ["first run", "second run"] {
            let layer = file_layer(&path).expect("log file");
            let subscriber = Registry::default().with(layer);
            bevy::log::tracing::subscriber::with_default(subscriber, || {
                bevy::log::info!("{run}");
            });
        }
        let current = std::fs::read_to_string(&path).expect("current log");
        assert!(current.contains("second run"), "{current}");
        assert!(!current.contains("first run"), "{current}");
        assert!(!current.contains('\u{1b}'), "no colour codes: {current}");
        let previous = std::fs::read_to_string(dir.path().join("logs").join("editor.previous.log"))
            .expect("previous log");
        assert!(previous.contains("first run"), "{previous}");
    }
}
