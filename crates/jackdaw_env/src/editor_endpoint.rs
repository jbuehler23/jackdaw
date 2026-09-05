//! Where a running editor can be reached, published under the project it
//! has open.
//!
//! A client driving the editor has to find it first, and the editor is
//! the only process that knows which port it bound and which project it
//! opened. It writes both to `<project>/.jackdaw/editor.json` when the
//! project opens and removes the file on exit; a reader that finds one
//! left behind by a crash tells by the pid.
//!
//! The type lives here, in the dependency-light environment crate,
//! because the writer (the editor) and the readers (`jd mcp`, tooling)
//! share nothing heavier.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name under `.jackdaw/`.
pub const EDITOR_ENDPOINT_FILE: &str = "editor.json";

/// The editor process holding `project` open, and the loopback port its
/// remote-control server is listening on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EditorEndpoint {
    /// Process id of the editor, so a reader can tell a live endpoint
    /// from one a crash left behind.
    pub pid: u32,
    /// The editor executable's name, as the kernel reports it.
    ///
    /// A pid alone is not identity: pids wrap, and by the time a client
    /// reads a file a crashed editor left behind, something else may hold
    /// that number. Checked against the live process before the endpoint
    /// is believed. Absent in a file written before this field existed,
    /// which then falls back to the pid alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// Loopback port of the editor's BRP server.
    pub port: u16,
    /// The open project's root directory.
    pub project: PathBuf,
    /// The scene in the active tab, when it has been saved to a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<PathBuf>,
    /// RFC 3339 stamp of when the editor published this file.
    pub started_at: String,
}

impl EditorEndpoint {
    /// `http://127.0.0.1:<port>/`, the BRP endpoint to POST to.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Whether the editor that wrote this file is still the process
    /// holding that pid.
    ///
    /// Answered from `/proc` on Linux: the pid has to exist *and* its
    /// `comm` has to be the executable that wrote the file, because pids
    /// wrap and the number a crashed editor left behind is handed out
    /// again. Elsewhere there is no dependency-free way to ask, and
    /// reporting "gone" for a live editor is the worse mistake, so the
    /// endpoint is taken at its word.
    pub fn is_running(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", self.pid)) else {
                return false;
            };
            let Some(process) = self.process.as_deref() else {
                // Written before the name was recorded; the pid is all
                // there is to go on.
                return true;
            };
            comm.trim() == truncated_comm(process)
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
}

/// The executable name as `/proc/<pid>/comm` spells it.
///
/// The kernel stores 15 bytes plus a terminator, so a longer name comes
/// back cut short and a comparison against the full name never matches.
#[cfg(target_os = "linux")]
fn truncated_comm(process: &str) -> &str {
    const COMM_LEN: usize = 15;
    if process.len() <= COMM_LEN {
        return process;
    }
    // Bytes, as the kernel counts them, backed off to the nearest
    // character boundary so the slice is still a `str`.
    let mut at = COMM_LEN;
    while at > 0 && !process.is_char_boundary(at) {
        at -= 1;
    }
    &process[..at]
}

/// This process's executable name, for [`EditorEndpoint::process`].
pub fn current_process_name() -> Option<String> {
    std::env::current_exe()
        .ok()?
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

/// The endpoint file of the project rooted at `root`.
pub fn endpoint_path(root: &Path) -> PathBuf {
    root.join(".jackdaw").join(EDITOR_ENDPOINT_FILE)
}

/// Read the endpoint published under `root`.
///
/// `None` when no editor has it open, when the file is unreadable, or
/// when the process that wrote it is gone -- a stale file is the same
/// answer as no file for anyone about to connect.
pub fn read_endpoint(root: &Path) -> Option<EditorEndpoint> {
    let data = std::fs::read_to_string(endpoint_path(root)).ok()?;
    let endpoint: EditorEndpoint = serde_json::from_str(&data).ok()?;
    endpoint.is_running().then_some(endpoint)
}

/// Publish `endpoint` under `root`, creating `.jackdaw/` if it is missing.
///
/// Written beside the file and renamed over it, so a client reading while
/// the editor republishes sees the old endpoint or the new one and never a
/// half-written file it would refuse to parse.
pub fn write_endpoint(root: &Path, endpoint: &EditorEndpoint) -> std::io::Result<()> {
    let path = endpoint_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(endpoint)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let staged = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&staged, data)?;
    match std::fs::rename(&staged, &path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&staged);
            Err(err)
        }
    }
}

/// Remove the endpoint published under `root`. A missing file is success.
pub fn remove_endpoint(root: &Path) {
    let _ = std::fs::remove_file(endpoint_path(root));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_round_trips_through_the_project_state_dir() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = EditorEndpoint {
            pid: std::process::id(),
            process: current_process_name(),
            port: 15703,
            project: dir.path().to_path_buf(),
            scene: Some(PathBuf::from("assets/scene.bsn")),
            started_at: "2024-01-01T00:00:00Z".to_string(),
        };
        write_endpoint(dir.path(), &endpoint).expect("write the endpoint");
        assert_eq!(read_endpoint(dir.path()), Some(endpoint));
        remove_endpoint(dir.path());
        assert_eq!(read_endpoint(dir.path()), None);
    }

    /// A file left behind by a crashed editor reads as no editor at all,
    /// so a client does not try to connect to a port nothing holds.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_endpoint_whose_process_is_gone_reads_as_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        // Pid 0 is not a process on Linux, so /proc/0 never exists.
        let endpoint = EditorEndpoint {
            pid: 0,
            process: Some("jackdaw".to_string()),
            port: 15703,
            project: dir.path().to_path_buf(),
            scene: None,
            started_at: "2024-01-01T00:00:00Z".to_string(),
        };
        write_endpoint(dir.path(), &endpoint).expect("write the endpoint");
        assert_eq!(read_endpoint(dir.path()), None);
    }

    /// Pids wrap. An endpoint naming this live pid but another program
    /// reads as absent, so a client does not send BRP at whatever now
    /// holds the number a crashed editor had.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_endpoint_whose_pid_belongs_to_another_program_reads_as_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = EditorEndpoint {
            pid: std::process::id(),
            process: Some("definitely-not-this-test".to_string()),
            port: 15703,
            project: dir.path().to_path_buf(),
            scene: None,
            started_at: "2024-01-01T00:00:00Z".to_string(),
        };
        write_endpoint(dir.path(), &endpoint).expect("write the endpoint");
        assert_eq!(read_endpoint(dir.path()), None);
    }

    /// `comm` holds 15 bytes, so a longer executable name is compared
    /// against what the kernel actually stored rather than never
    /// matching.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_long_executable_name_is_compared_as_the_kernel_truncates_it() {
        assert_eq!(truncated_comm("jackdaw"), "jackdaw");
        assert_eq!(
            truncated_comm("jackdaw-editor-with-a-long-name"),
            "jackdaw-editor-"
        );
    }
}
