//! Where the jackdaw crates this build is made of can be fetched from.
//!
//! A scaffolded project depends on `jackdaw_runtime`, and the dependency
//! has to name somewhere cargo can actually reach. Asking the registry
//! for the anchored version is right only when that version is published:
//! an editor installed straight from git carries crates no registry has,
//! and a project it writes a `version = "0.19"` for cannot resolve at
//! all. So the editor records what it was built from, and the scaffold
//! states the matching dependency form.

use std::path::{Path, PathBuf};

/// Where a build's jackdaw crates come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSource {
    /// A published release: the anchored version line is on crates.io.
    Release,
    /// A git build, at a revision. Pinned to the revision rather than a
    /// branch, so a project keeps building against the editor that made
    /// it after the branch has moved on.
    Git {
        /// The repository the crates are published from.
        repository: String,
        /// The full revision this jackdaw was built at.
        rev: String,
    },
    /// A workspace on this machine, with nothing published to match it.
    Path(PathBuf),
}

/// The repository the jackdaw crates are published from, from this
/// crate's own manifest.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// What this build was made from, as recorded by `build.rs`.
pub fn build_source() -> BuildSource {
    BuildSource::parse(env!("JACKDAW_BUILD_SOURCE"), REPOSITORY)
}

impl BuildSource {
    /// Read back what `build.rs` recorded. An unreadable record reads as
    /// a release, which is the form every published build wants and the
    /// only one that says nothing about this machine.
    fn parse(raw: &str, repository: &str) -> Self {
        if let Some(rev) = raw.strip_prefix("git:") {
            return Self::Git {
                repository: repository.to_string(),
                rev: rev.to_string(),
            };
        }
        if let Some(path) = raw.strip_prefix("path:") {
            return Self::Path(PathBuf::from(path));
        }
        Self::Release
    }

    /// The requirement a scaffolded project states for `crate_name`, as
    /// the body of an inline table: the caller wraps it in the braces and
    /// adds whatever `features` the template asks for.
    ///
    /// Paths are single-quoted, so a Windows separator is not read as a
    /// TOML escape.
    pub fn dep_requirement(&self, crate_name: &str) -> String {
        match self {
            Self::Release => format!("version = \"{}\"", crate::BEVY_VERSION),
            Self::Git { repository, rev } => {
                format!("git = \"{repository}\", rev = \"{rev}\"")
            }
            Self::Path(root) => {
                format!("path = '{}'", crate_path(root, crate_name).display())
            }
        }
    }

    /// Why this project's jackdaw dependency may be the thing that failed
    /// to resolve, for a build that could not work out its dependencies.
    /// `None` for a release, where the registry has the answer and a
    /// resolution failure is about something else.
    pub fn resolution_note(&self) -> Option<String> {
        match self {
            Self::Release => None,
            Self::Git { rev, .. } => Some(format!(
                "this jackdaw was built from git at {rev}, which no registry \
                 has; a project it scaffolds depends on that revision and \
                 needs network access to the repository"
            )),
            Self::Path(root) => Some(format!(
                "this jackdaw was built from the checkout at {}, which no \
                 registry has; a project it scaffolds depends on that \
                 directory and only builds on a machine that has it",
                root.display()
            )),
        }
    }
}

/// Where a jackdaw crate's sources sit inside a workspace.
fn crate_path(root: &Path, crate_name: &str) -> PathBuf {
    root.join("crates").join(crate_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = "https://github.com/jbuehler23/jackdaw";
    const REV: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_release_build_asks_the_registry_for_the_anchored_line() {
        let source = BuildSource::parse("release", REPO);
        assert_eq!(source, BuildSource::Release);
        assert_eq!(
            source.dep_requirement("jackdaw_runtime"),
            format!("version = \"{}\"", crate::BEVY_VERSION)
        );
        assert!(source.resolution_note().is_none());
    }

    #[test]
    fn a_git_build_pins_the_revision_it_was_built_at() {
        let source = BuildSource::parse(&format!("git:{REV}"), REPO);
        assert_eq!(
            source,
            BuildSource::Git {
                repository: REPO.to_string(),
                rev: REV.to_string(),
            }
        );
        assert_eq!(
            source.dep_requirement("jackdaw_runtime"),
            format!("git = \"{REPO}\", rev = \"{REV}\"")
        );
        assert!(
            source
                .resolution_note()
                .is_some_and(|note| note.contains(REV)),
            "the note names the revision a project would be pinned to"
        );
    }

    #[test]
    fn a_checkout_build_points_at_the_crate_in_that_workspace() {
        let source = BuildSource::parse("path:/home/dev/jackdaw", REPO);
        assert_eq!(
            source,
            BuildSource::Path(PathBuf::from("/home/dev/jackdaw"))
        );
        assert_eq!(
            source.dep_requirement("jackdaw_extension"),
            format!(
                "path = '{}'",
                PathBuf::from("/home/dev/jackdaw/crates/jackdaw_extension").display()
            )
        );
        assert!(
            source
                .resolution_note()
                .is_some_and(|note| note.contains("/home/dev/jackdaw")),
            "the note names the checkout a project would be tied to"
        );
    }

    /// The requirement is written into an inline table the template
    /// closes and may append `features` to, so it must not carry its own
    /// braces or a trailing comma.
    #[test]
    fn a_requirement_is_a_table_body_and_not_a_table() {
        for raw in ["release", &format!("git:{REV}"), "path:/tmp/jackdaw"] {
            let requirement = BuildSource::parse(raw, REPO).dep_requirement("jackdaw_runtime");
            assert!(
                !requirement.starts_with('{') && !requirement.ends_with(','),
                "{raw} yielded {requirement}"
            );
        }
    }

    /// A record this build did not write -- an older stamp, or an empty
    /// one -- must not be read as a path on some other machine.
    #[test]
    fn an_unreadable_record_reads_as_a_release() {
        assert_eq!(BuildSource::parse("", REPO), BuildSource::Release);
        assert_eq!(
            BuildSource::parse("something else", REPO),
            BuildSource::Release
        );
    }
}
