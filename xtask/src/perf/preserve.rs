//! A folder's files kept before a run and put back after it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Every file under a folder as it was, apart from the paths left out. Put
/// back when restored or dropped.
pub struct Preserved {
    root: PathBuf,
    skip: Vec<PathBuf>,
    files: Vec<(PathBuf, Vec<u8>)>,
    existed: HashSet<PathBuf>,
    restored: bool,
}

impl Preserved {
    /// Keep every file under `root` except those at or below `skip`.
    pub fn take(root: &Path, skip: &[PathBuf]) -> Self {
        let walked = walk(root, skip);
        let files = walked
            .files
            .iter()
            .filter_map(|file| Some((file.clone(), std::fs::read(file).ok()?)))
            .collect();
        let mut skip = skip.to_vec();
        skip.extend(walked.unreadable.iter().cloned());
        Self {
            root: root.to_path_buf(),
            skip,
            files,
            existed: walked.all().collect(),
            restored: false,
        }
    }

    /// Remove what the run added under the folder and write back what changed
    /// or went. A path that existed before is never removed, read or not.
    /// Answers what could not be put back.
    pub fn restore(&mut self) -> Vec<String> {
        if self.restored {
            return Vec::new();
        }
        self.restored = true;
        let mut failed = Vec::new();
        let walked = walk(&self.root, &self.skip);
        for added in walked
            .files
            .iter()
            .chain(&walked.others)
            .filter(|path| !self.existed.contains(*path))
        {
            if let Err(err) = std::fs::remove_file(added) {
                failed.push(format!("remove {}: {err}", added.display()));
            }
        }
        for (file, bytes) in &self.files {
            if std::fs::read(file).ok().as_ref() == Some(bytes) {
                continue;
            }
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(file, bytes));
            if let Err(err) = written {
                failed.push(format!("restore {}: {err}", file.display()));
            }
        }
        let mut dirs: Vec<&PathBuf> = walked
            .dirs
            .iter()
            .filter(|dir| !self.existed.contains(*dir))
            .collect();
        dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in dirs {
            let _ = std::fs::remove_dir(dir);
        }
        failed
    }
}

impl Drop for Preserved {
    fn drop(&mut self) {
        for failed in self.restore() {
            eprintln!("perf: {failed}");
        }
    }
}

/// What a walk found: regular files, other entries such as links and sockets,
/// folders, and folders it could not list.
#[derive(Default)]
struct Walked {
    files: Vec<PathBuf>,
    others: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
    unreadable: Vec<PathBuf>,
}

impl Walked {
    fn all(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.files
            .iter()
            .chain(&self.others)
            .chain(&self.dirs)
            .chain(&self.unreadable)
            .cloned()
    }
}

/// Everything below `root`, leaving out `skip` and what is under it. Links
/// are listed, not followed.
fn walk(root: &Path, skip: &[PathBuf]) -> Walked {
    let mut walked = Walked::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            if dir != root {
                walked.unreadable.push(dir);
            }
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if skip.iter().any(|skipped| path.starts_with(skipped)) {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    walked.dirs.push(path.clone());
                    pending.push(path);
                }
                Ok(kind) if kind.is_file() => walked.files.push(path),
                _ => walked.others.push(path),
            }
        }
    }
    walked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("xtask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(root: &Path, path: &str) -> Option<String> {
        std::fs::read_to_string(root.join(path)).ok()
    }

    #[test]
    fn restore_undoes_additions_changes_and_removals_but_not_skipped_paths() {
        let root = scratch("preserve");
        write(&root, "project.json", "before");
        write(&root, "editor.json", "stale endpoint");
        write(&root, "thumbnails/old.png", "old");
        write(&root, "schema.json", "old types");
        write(&root, "target/game", "binary");
        let skip = [
            root.join("target"),
            root.join("schema.json"),
            root.join("perf"),
        ];
        let mut kept = Preserved::take(&root, &skip);

        write(&root, "project.json", "after");
        std::fs::remove_file(root.join("editor.json")).unwrap();
        write(&root, "thumbnails/new/added.png", "new");
        write(&root, "schema.json", "new types");
        write(&root, "target/game", "rebuilt");
        write(&root, "perf/report.md", "results");
        assert!(kept.restore().is_empty());

        assert_eq!(read(&root, "project.json").as_deref(), Some("before"));
        assert_eq!(
            read(&root, "editor.json").as_deref(),
            Some("stale endpoint")
        );
        assert_eq!(read(&root, "thumbnails/old.png").as_deref(), Some("old"));
        assert!(!root.join("thumbnails/new").exists());
        assert_eq!(read(&root, "schema.json").as_deref(), Some("new types"));
        assert_eq!(read(&root, "target/game").as_deref(), Some("rebuilt"));
        assert_eq!(read(&root, "perf/report.md").as_deref(), Some("results"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn paths_the_snapshot_could_not_read_are_left_alone() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("preserve-unread");
        write(&root, "elsewhere/kept.txt", "kept");
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("link")).unwrap();
        write(&root, "locked.bin", "secret");
        let locked = root.join("locked.bin");
        std::fs::set_permissions(&locked, PermissionsExt::from_mode(0o000)).unwrap();

        drop(Preserved::take(&root, &[]));

        assert!(
            root.join("link").symlink_metadata().is_ok(),
            "the link stays"
        );
        assert_eq!(read(&root, "link/kept.txt").as_deref(), Some("kept"));
        assert!(
            locked.symlink_metadata().is_ok(),
            "the unreadable file stays"
        );
        std::fs::set_permissions(&locked, PermissionsExt::from_mode(0o644)).unwrap();
        assert_eq!(read(&root, "locked.bin").as_deref(), Some("secret"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
