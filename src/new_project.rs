//! Launcher-side scaffolding entry point.
//!
//! New projects come from the templates embedded in the binary (see
//! [`crate::scaffold`]); this module adds the launcher's thin wrapper.

use std::path::{Path, PathBuf};

pub use jackdaw_scaffold::dev_checkout::jackdaw_dev_checkout;

/// Create `<location>/<name>` from an embedded template and return
/// the created project directory.
pub fn scaffold_project(
    name: &str,
    location: &Path,
    kind: crate::scaffold::TemplateKind,
) -> Result<PathBuf, crate::scaffold::ScaffoldError> {
    // The default location is a guess (`~/Projects`) that does not
    // exist on a fresh machine, so refusing here failed the very first
    // New Game a downloaded jackdaw ever sees, for a path the user
    // never chose. Creating it is what `cargo new` effectively does.
    if !location.is_dir() {
        std::fs::create_dir_all(location).map_err(|error| {
            crate::scaffold::ScaffoldError::Io(format!(
                "could not create {}: {error}",
                location.display()
            ))
        })?;
    }
    let dest = location.join(name);
    crate::scaffold::scaffold_new_project(&dest, name, kind)?;
    // Parity with `jd new`, which inits a repository the way `cargo new`
    // does. Two entry points producing differently-set-up projects is a
    // difference nobody can see until they try to commit.
    crate::scaffold::init_git_repository(&dest);
    Ok(dest)
}
