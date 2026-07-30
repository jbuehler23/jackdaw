//! Shared build-progress plumbing for the editor's UI.
//!
//! Project code (games and extensions alike) is compiled by
//! [`crate::project_build`]: a generated shim crate, a per-edge extern
//! redirect plan, and an isolated target directory. What lives here is
//! the presentation-side state that pipeline streams into: the rolling
//! per-crate progress and log tail the status bar and the Build panel
//! render.

use std::collections::VecDeque;
use std::path::Path;

/// Capacity of the rolling log-tail buffer surfaced in progress UI.
const LOG_TAIL_CAPACITY: usize = 20;

/// Live progress from a running cargo build. Writers: the build
/// pipeline's reporter. Reader: the UI poller that renders the progress
/// bar + log tail each frame. Wrap in `Arc<Mutex<_>>` when handing to a
/// long-running task.
///
/// `artifacts_total` is `Some` once the expected number of compile
/// units is known; until then the UI renders an indeterminate bar (or
/// just the counter).
#[derive(Debug, Default, Clone)]
pub struct BuildProgress {
    pub current_crate: Option<String>,
    pub artifacts_done: u32,
    pub artifacts_total: Option<u32>,
    pub recent_log_lines: VecDeque<String>,
    /// The complete build output, unbounded, for the Build panel's
    /// IDE-style scrollback. Compact surfaces render
    /// `recent_log_lines` instead.
    pub full_log: String,
    /// Set to `true` once cargo exits (success or failure). The UI uses
    /// this to flip the bar to 100%.
    pub finished: bool,
}

impl BuildProgress {
    pub fn push_log(&mut self, line: String) {
        if !self.full_log.is_empty() {
            self.full_log.push('\n');
        }
        self.full_log.push_str(&line);
        if self.recent_log_lines.len() >= LOG_TAIL_CAPACITY {
            self.recent_log_lines.pop_front();
        }
        self.recent_log_lines.push_back(line);
    }

    /// 0.0 when unknown, 1.0 when done.
    pub fn fraction(&self) -> Option<f32> {
        if self.finished {
            return Some(1.0);
        }
        let total = self.artifacts_total? as f32;
        if total <= 0.0 {
            return None;
        }
        Some((self.artifacts_done as f32 / total).clamp(0.0, 1.0))
    }
}

/// Derive the platform dylib filename for a project's package name.
/// Falls back to `libunnamed.<ext>` if the manifest declares no name
/// (which cargo would have rejected anyway, but it keeps this helper
/// infallible).
pub(crate) fn artifact_file_name(project_dir: &Path) -> String {
    let package_name = package_name_from_manifest(project_dir);

    if cfg!(target_os = "windows") {
        format!("{package_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{package_name}.dylib")
    } else {
        format!("lib{package_name}.so")
    }
}

/// The `[package] name` from a project's manifest, underscored so it
/// matches the artifact cargo emits.
fn package_name_from_manifest(project_dir: &Path) -> String {
    let Ok(contents) = std::fs::read_to_string(project_dir.join("Cargo.toml")) else {
        return "unnamed".to_string();
    };
    contents
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed
                .strip_prefix("name")?
                .trim_start()
                .strip_prefix('=')?;
            let value = rest.trim().trim_matches(['"', '\'']);
            (!value.is_empty()).then(|| value.replace('-', "_"))
        })
        .unwrap_or_else(|| "unnamed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_fraction_tracks_the_unit_count() {
        let mut progress = BuildProgress {
            artifacts_total: Some(4),
            artifacts_done: 1,
            ..Default::default()
        };
        assert_eq!(progress.fraction(), Some(0.25));
        progress.finished = true;
        assert_eq!(progress.fraction(), Some(1.0));
    }

    #[test]
    fn the_log_tail_is_bounded_but_the_full_log_is_not() {
        let mut progress = BuildProgress::default();
        for index in 0..LOG_TAIL_CAPACITY + 5 {
            progress.push_log(format!("line {index}"));
        }
        assert_eq!(progress.recent_log_lines.len(), LOG_TAIL_CAPACITY);
        assert!(progress.full_log.contains("line 0"));
        assert!(
            progress
                .full_log
                .contains(&format!("line {}", LOG_TAIL_CAPACITY + 4))
        );
    }

    #[test]
    fn the_artifact_name_comes_from_the_package_name() {
        let dir = std::env::temp_dir().join(format!("jackdaw_artifact_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert!(artifact_file_name(&dir).contains("my_game"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
