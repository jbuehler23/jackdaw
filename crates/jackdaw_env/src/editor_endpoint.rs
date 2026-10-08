//! Where a running editor can be reached, published under the project it has
//! open.
//!
//! The editor is the only process that knows which port it bound and which
//! project it opened. It writes both to `<project>/.jackdaw/editor.json` when the
//! project opens and removes the file on exit; a reader that finds one left
//! behind by a crash tells by the pid.
//!
//! The type lives in the dependency-light environment crate because the writer
//! and its readers share nothing heavier.

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
    /// A pid alone is not identity: pids wrap, so this is checked against the
    /// live process before the endpoint is believed. Absent in a file written
    /// before this field existed, which falls back to the pid alone.
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
    /// The pid has to name a live process, and when the platform can say what
    /// that process runs, it has to be the executable that wrote the file.
    pub fn is_running(&self) -> bool {
        if !process::is_alive(self.pid) {
            return false;
        }
        let Some(expected) = self.process.as_deref() else {
            // Written before the name was recorded; the pid is all there is
            // to go on.
            return true;
        };
        process::runs(self.pid, expected).unwrap_or(true)
    }
}

/// Asking the OS about another process by pid.
mod process {
    /// Whether `pid` names a live process.
    #[cfg(target_os = "linux")]
    pub fn is_alive(pid: u32) -> bool {
        pid != 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Whether `pid` names a live process. A pid owned by another user still
    /// answers, with a permission error rather than "no such process".
    #[cfg(all(unix, not(target_os = "linux")))]
    pub fn is_alive(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // SAFETY: signal 0 performs only the existence and permission checks.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// Whether `pid` names a live process.
    #[cfg(windows)]
    pub fn is_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
        };
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        if pid == 0 {
            return false;
        }
        // SAFETY: the handle is checked before use and closed once read.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return GetLastError() == ERROR_ACCESS_DENIED;
            }
            let mut code = 0u32;
            let read = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            // An exit code that cannot be read is not proof the process is gone.
            read == 0 || code == STILL_ACTIVE as u32
        }
    }

    /// Whether `pid` names a live process. No way to ask here, and reporting
    /// "gone" for a live editor is the worse mistake.
    #[cfg(not(any(unix, windows)))]
    pub fn is_alive(_pid: u32) -> bool {
        true
    }

    /// Whether `pid` runs the executable named `expected`, or `None` when
    /// that cannot be read.
    ///
    /// `/proc/<pid>/comm` holds the name the kernel stored, cut short at 15
    /// bytes.
    #[cfg(target_os = "linux")]
    pub fn runs(pid: u32, expected: &str) -> Option<bool> {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        Some(comm.trim() == truncated_comm(expected))
    }

    /// Whether `pid` runs the executable named `expected`, or `None` when
    /// that cannot be read.
    #[cfg(target_os = "macos")]
    pub fn runs(pid: u32, expected: &str) -> Option<bool> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is as long as the size passed with it.
        let len =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        let len = usize::try_from(len).ok().filter(|&len| len > 0)?;
        let path = std::path::Path::new(std::str::from_utf8(&buffer[..len]).ok()?);
        Some(path.file_name()? == expected)
    }

    /// Whether `pid` runs the executable named `expected`, or `None` when
    /// that cannot be read.
    #[cfg(windows)]
    pub fn runs(pid: u32, expected: &str) -> Option<bool> {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };

        let mut buffer = vec![0u16; 32_768];
        let mut len = buffer.len() as u32;
        // SAFETY: the handle is checked before use and closed once read, and
        // `len` carries the buffer's length in and the written length out.
        let read = unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let read = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                buffer.as_mut_ptr(),
                &mut len,
            );
            CloseHandle(handle);
            read
        };
        if read == 0 {
            return None;
        }
        let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len as usize]));
        let name = path.file_name()?.to_str()?;
        Some(name.eq_ignore_ascii_case(expected))
    }

    /// Whether `pid` runs the executable named `expected`; never readable here.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub fn runs(_pid: u32, _expected: &str) -> Option<bool> {
        None
    }

    /// The executable name as `/proc/<pid>/comm` spells it.
    ///
    /// The kernel stores 15 bytes plus a terminator, so a longer name comes
    /// back cut short and a comparison against the full name never matches.
    #[cfg(target_os = "linux")]
    pub fn truncated_comm(process: &str) -> &str {
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
}

/// This process's executable name, for [`EditorEndpoint::process`].
pub fn current_process_name() -> Option<String> {
    // Resolved, so a launch through a symlink records the name the OS reports
    // for the running image.
    let exe = std::env::current_exe().ok()?;
    std::fs::canonicalize(&exe)
        .unwrap_or(exe)
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
/// Written beside the file and renamed over it, so a client reading while the
/// editor republishes never sees a half-written file.
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

    /// A pid that named a process which has since exited.
    fn a_reaped_pid() -> u32 {
        #[cfg(windows)]
        let mut command = std::process::Command::new("cmd");
        #[cfg(windows)]
        command.args(["/C", "exit"]);
        #[cfg(not(windows))]
        let mut command = std::process::Command::new("true");
        let mut child = command.spawn().expect("spawn a short-lived process");
        let pid = child.id();
        child.wait().expect("reap it");
        pid
    }

    fn endpoint_for(pid: u32, process: Option<String>, root: &Path) -> EditorEndpoint {
        EditorEndpoint {
            pid,
            process,
            port: 15703,
            project: root.to_path_buf(),
            scene: None,
            started_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn this_process_reads_as_running() {
        let here = Path::new(".");
        assert!(endpoint_for(std::process::id(), current_process_name(), here).is_running());
        assert!(endpoint_for(std::process::id(), None, here).is_running());
    }

    /// A file left behind by a crashed editor reads as no editor at all,
    /// so a client does not try to connect to a port nothing holds.
    #[test]
    fn an_endpoint_whose_process_is_gone_reads_as_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = endpoint_for(a_reaped_pid(), Some("jackdaw".to_string()), dir.path());
        write_endpoint(dir.path(), &endpoint).expect("write the endpoint");
        assert_eq!(read_endpoint(dir.path()), None);
        assert!(!endpoint_for(0, None, dir.path()).is_running());
    }

    /// Pids wrap. An endpoint naming this live pid but another program
    /// reads as absent, so a client does not send BRP at whatever now
    /// holds the number a crashed editor had.
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn an_endpoint_whose_pid_belongs_to_another_program_reads_as_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = endpoint_for(
            std::process::id(),
            Some("definitely-not-this-test".to_string()),
            dir.path(),
        );
        write_endpoint(dir.path(), &endpoint).expect("write the endpoint");
        assert_eq!(read_endpoint(dir.path()), None);
    }

    /// `comm` holds 15 bytes, so a longer executable name is compared
    /// against what the kernel actually stored rather than never
    /// matching.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_long_executable_name_is_compared_as_the_kernel_truncates_it() {
        use super::process::truncated_comm;
        assert_eq!(truncated_comm("jackdaw"), "jackdaw");
        assert_eq!(
            truncated_comm("jackdaw-editor-with-a-long-name"),
            "jackdaw-editor-"
        );
    }
}
