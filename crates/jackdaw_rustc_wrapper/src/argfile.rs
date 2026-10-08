//! Passing rustc's arguments through an `@file` when the command line
//! would be too long to spawn.
//!
//! Windows refuses a command line past 32,767 UTF-16 units ("The
//! filename or extension is too long", os error 206), and the redirect
//! plan adds an `--extern` per dependency edge, so a crate with many
//! dependencies can cross it. rustc reads `@path` as a UTF-8 file
//! holding one argument per line.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Longest command line, in bytes, the wrapper hands to rustc directly.
/// Below the Windows limit with room for quoting and escaping.
pub(crate) const COMMAND_LINE_LIMIT: usize = 30_000;

/// An argument file on disk, removed when dropped.
#[derive(Debug)]
pub(crate) struct ArgFile(PathBuf);

impl Drop for ArgFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// An upper bound on the command line Windows would build for `program`
/// and `args`: each argument quoted, separated by a space, and every
/// quote or backslash escaped.
pub(crate) fn command_line_length(program: &OsStr, args: &[OsString]) -> usize {
    std::iter::once(program)
        .chain(args.iter().map(OsString::as_os_str))
        .map(|arg| {
            let bytes = arg.as_encoded_bytes();
            let escapes = bytes.iter().filter(|b| matches!(b, b'"' | b'\\')).count();
            bytes.len() + escapes + 3
        })
        .sum()
}

/// The argument file text for `args`, or `None` when an argument cannot be
/// written as one line of UTF-8.
pub(crate) fn contents(args: &[OsString]) -> Option<String> {
    let mut text = String::new();
    for arg in args {
        let arg = arg.to_str()?;
        if arg.contains(['\n', '\r']) {
            return None;
        }
        text.push_str(arg);
        text.push('\n');
    }
    Some(text)
}

/// The arguments to spawn `program` with. Under [`COMMAND_LINE_LIMIT`]
/// they are `args` unchanged; past it they are written to a file in `dir`
/// and replaced by a single `@file`, which lives as long as the returned
/// [`ArgFile`]. Arguments no file can carry are passed directly.
pub(crate) fn forwarded(
    program: &OsStr,
    args: &[OsString],
    dir: &Path,
) -> std::io::Result<(Vec<OsString>, Option<ArgFile>)> {
    if command_line_length(program, args) <= COMMAND_LINE_LIMIT {
        return Ok((args.to_vec(), None));
    }
    let Some(text) = contents(args) else {
        return Ok((args.to_vec(), None));
    };
    let path = dir.join(format!("jackdaw-rustc-{}.args", std::process::id()));
    std::fs::write(&path, text)?;
    let mut at = OsString::from("@");
    at.push(&path);
    Ok((vec![at], Some(ArgFile(path))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_wrapper_argfile_{tag}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Every argument is counted as quoted and space separated, with its
    /// quotes and backslashes escaped.
    #[test]
    fn the_length_counts_quoting_and_escapes() {
        assert_eq!(command_line_length(OsStr::new("rustc"), &[]), 8);
        assert_eq!(
            command_line_length(OsStr::new("rustc"), &args(&[r#"C:\a "b""#])),
            8 + 8 + 3 + 3
        );
    }

    #[test]
    fn a_short_command_line_is_passed_directly() {
        let dir = scratch("short");
        let short = args(&["--crate-name", "demo", "src/lib.rs"]);
        let (forwarded, file) = forwarded(OsStr::new("rustc"), &short, &dir).expect("forwarded");
        assert_eq!(forwarded, short);
        assert!(file.is_none());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// Past the limit the arguments move to a file, one per line, and the
    /// file goes away with its handle.
    #[test]
    fn a_long_command_line_moves_to_an_argument_file() {
        let dir = scratch("long");
        let mut long = args(&[
            "--crate-name",
            "demo",
            "--extern",
            "has space=C:\\x y\\a.rlib",
        ]);
        let edge = format!("bevy_ecs=/sdk/deps/{}.rlib", "x".repeat(200));
        while command_line_length(OsStr::new("rustc"), &long) <= COMMAND_LINE_LIMIT {
            long.push("--extern".into());
            long.push(edge.as_str().into());
        }
        let (forwarded, file) = forwarded(OsStr::new("rustc"), &long, &dir).expect("forwarded");
        let file = file.expect("an argument file");
        assert_eq!(forwarded.len(), 1);
        let at = forwarded[0].to_str().expect("utf-8");
        let path = Path::new(at.strip_prefix('@').expect("an @file argument"));
        assert_eq!(path, file.0);
        let written = std::fs::read_to_string(path).expect("argument file");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), long.len());
        assert_eq!(lines[3], "has space=C:\\x y\\a.rlib");
        assert!(written.ends_with('\n'));
        drop(file);
        assert!(!path.exists());
    }

    #[test]
    fn an_argument_spanning_lines_has_no_file_form() {
        assert_eq!(contents(&args(&["a", "b"])).as_deref(), Some("a\nb\n"));
        assert!(contents(&args(&["--cfg", "a\nb"])).is_none());
        assert!(contents(&args(&["--cfg", "a\r"])).is_none());
    }

    /// rustc reads the file the way it is written: a source path holding
    /// a space survives as one argument.
    #[test]
    fn rustc_reads_the_argument_file() {
        let dir = scratch("rustc");
        let source = dir.join("with space").join("probe.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let file = dir.join("probe.args");
        let text = contents(&[
            "--print".into(),
            "crate-name".into(),
            "--crate-name".into(),
            "argfile_probe".into(),
            source.into_os_string(),
        ])
        .expect("utf-8");
        std::fs::write(&file, text).unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let mut at = OsString::from("@");
        at.push(&file);
        let output = std::process::Command::new(rustc)
            .arg(at)
            .output()
            .expect("rustc runs");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "argfile_probe"
        );
    }
}
