//! A folder's files kept before a run and put back after it.

use std::path::{Path, PathBuf};

/// Every file under a folder as it was, apart from the folders left out.
pub struct Preserved {
    root: PathBuf,
    skip: Vec<PathBuf>,
    files: Vec<(PathBuf, Vec<u8>)>,
    dirs: Vec<PathBuf>,
}

impl Preserved {
    /// Keep every file under `root` except those below `skip`.
    pub fn take(root: &Path, skip: &[PathBuf]) -> Self {
        let (files, dirs) = walk(root, skip);
        let files = files
            .into_iter()
            .filter_map(|file| Some((file.clone(), std::fs::read(&file).ok()?)))
            .collect();
        Self {
            root: root.to_path_buf(),
            skip: skip.to_vec(),
            files,
            dirs,
        }
    }

    /// Remove what was added under the folder and write back what changed or
    /// went. Answers what could not be put back.
    pub fn restore(&self) -> Vec<String> {
        let mut failed = Vec::new();
        let (files, mut dirs) = walk(&self.root, &self.skip);
        for file in files {
            if !self.files.iter().any(|(kept, _)| *kept == file)
                && let Err(err) = std::fs::remove_file(&file)
            {
                failed.push(format!("remove {}: {err}", file.display()));
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
        dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in dirs {
            if !self.dirs.contains(&dir) {
                let _ = std::fs::remove_dir(&dir);
            }
        }
        failed
    }
}

/// The files and folders below `root`, leaving out `skip` and what is under it.
fn walk(root: &Path, skip: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if skip.iter().any(|skipped| path.starts_with(skipped)) {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    dirs.push(path.clone());
                    pending.push(path);
                }
                Ok(_) => files.push(path),
                Err(_) => {}
            }
        }
    }
    (files, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_undoes_additions_changes_and_removals_but_not_skipped_folders() {
        let root = std::env::temp_dir().join(format!("xtask-preserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("project.json", "before");
        write("editor.json", "stale endpoint");
        write("thumbnails/old.png", "old");
        write("target/game", "binary");
        let skip = [root.join("target"), root.join("perf")];
        let kept = Preserved::take(&root, &skip);

        write("project.json", "after");
        std::fs::remove_file(root.join("editor.json")).unwrap();
        write("thumbnails/new/added.png", "new");
        write("target/game", "rebuilt");
        write("perf/report.md", "results");
        assert!(kept.restore().is_empty());

        let read = |path: &str| std::fs::read_to_string(root.join(path)).ok();
        assert_eq!(read("project.json").as_deref(), Some("before"));
        assert_eq!(read("editor.json").as_deref(), Some("stale endpoint"));
        assert_eq!(read("thumbnails/old.png").as_deref(), Some("old"));
        assert!(!root.join("thumbnails/new").exists());
        assert_eq!(read("target/game").as_deref(), Some("rebuilt"));
        assert_eq!(read("perf/report.md").as_deref(), Some("results"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
