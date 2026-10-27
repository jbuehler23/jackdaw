//! Running toolchain processes so that their failures reach the user.
//!
//! The editor has no terminal, so a child's inherited stderr is lost there.
//! These keep it and hand it back with the failure.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::thread::JoinHandle;

/// Run `command` to completion with both pipes captured.
///
/// On a non-zero exit the error is the child's stderr, without cargo's
/// progress notes, or its exit status when it wrote nothing.
pub fn run_captured(command: &mut Command) -> Result<Output, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("could not run {program}: {error}"))?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = without_progress(&String::from_utf8_lossy(&output.stderr));
    Err(if stderr.trim().is_empty() {
        format!("{program} failed ({})", output.status)
    } else {
        stderr.trim().to_string()
    })
}

/// A child's stderr, read on its own thread while the caller reads stdout.
///
/// Draining both keeps either pipe from filling and stalling the child.
pub struct StderrCapture(Option<JoinHandle<Vec<u8>>>);

impl StderrCapture {
    /// Start collecting `child`'s stderr, which must have been piped.
    pub fn take(child: &mut Child) -> Self {
        Self::collect(child, false)
    }

    /// As [`StderrCapture::take`], also passing each line on to this
    /// process's stderr as it arrives, so a terminal still shows the child's
    /// progress.
    pub fn echoed(child: &mut Child) -> Self {
        Self::collect(child, true)
    }

    fn collect(child: &mut Child, echo: bool) -> Self {
        Self(child.stderr.take().map(|pipe| {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut buffer = Vec::new();
                if !echo {
                    let _ = reader.read_to_end(&mut buffer);
                    return buffer;
                }
                let mut line = Vec::new();
                while reader
                    .read_until(b'\n', &mut line)
                    .is_ok_and(|read| read > 0)
                {
                    let _ = std::io::stderr().write_all(&line);
                    buffer.append(&mut line);
                }
                buffer
            })
        }))
    }

    /// What the child wrote, without cargo's progress notes. Call after the
    /// child has exited.
    pub fn finish(self) -> String {
        let bytes = self
            .0
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        without_progress(&String::from_utf8_lossy(&bytes))
    }
}

/// Cargo's stderr without its per-crate progress notes, which say nothing
/// about why a build failed and would bury the lines that do.
fn without_progress(stderr: &str) -> String {
    const PROGRESS: [&str; 7] = [
        "Compiling ",
        "Checking ",
        "Downloading ",
        "Downloaded ",
        "Fresh ",
        "Updating ",
        "Locking ",
    ];
    stderr
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !PROGRESS.iter().any(|verb| line.starts_with(verb))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_leaves_out_cargos_progress_notes() {
        let stderr = "   Compiling bevy v0.19.1\n   Compiling my-game v0.1.0 (/p)\n\
                      error: could not compile `my-game` (lib) due to 1 previous error\n";

        assert_eq!(
            without_progress(stderr),
            "error: could not compile `my-game` (lib) due to 1 previous error"
        );
    }

    #[test]
    fn a_program_that_cannot_start_says_which_one() {
        let error = run_captured(&mut Command::new("jackdaw-no-such-program")).unwrap_err();
        assert!(error.contains("jackdaw-no-such-program"), "got {error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_returns_what_it_wrote_to_stderr() {
        let error =
            run_captured(Command::new("sh").args(["-c", "echo broken >&2; exit 3"])).unwrap_err();
        assert_eq!(error, "broken");
    }

    #[cfg(unix)]
    #[test]
    fn a_silent_failure_still_reports_its_exit_status() {
        let error = run_captured(Command::new("sh").args(["-c", "exit 3"])).unwrap_err();
        assert!(error.contains("sh failed"), "got {error}");
    }

    #[cfg(unix)]
    #[test]
    fn captured_stderr_is_returned_after_the_child_exits() {
        let mut child = Command::new("sh")
            .args(["-c", "echo '   Compiling x v1' >&2; echo 'error: nope' >&2"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = StderrCapture::take(&mut child);
        child.wait().unwrap();
        assert_eq!(stderr.finish(), "error: nope");
    }
}
