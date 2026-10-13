//! Runs `jd` for everything that touches the network or the installed
//! editors, so the editor itself never links TLS. Each run is a job with
//! progress read from `jd --json`, listed in the launcher's Downloads.

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

use bevy::prelude::*;

/// The `jd` binary the launcher runs.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct JdProgram(pub PathBuf);

impl Default for JdProgram {
    /// `jd` beside the editor binary, where editor bundles, `cargo
    /// install` and a source build all put it.
    fn default() -> Self {
        let name = if cfg!(windows) { "jd.exe" } else { "jd" };
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(name)));
        Self(beside.unwrap_or_else(|| PathBuf::from(name)))
    }
}

/// What a job does, for its label and for what follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobKind {
    RefreshIndex,
    Install { query: String },
    Remove { version: String },
    SetDefault { version: String },
    Verify { version: String },
    Locate,
    Clean,
    NewProject { name: String },
}

impl JobKind {
    pub fn label(&self) -> String {
        match self {
            Self::RefreshIndex => "Check for editor updates".into(),
            Self::Install { query } => {
                format!("Install jackdaw {}", super::model::version_label(query))
            }
            Self::Remove { version } => {
                format!("Uninstall jackdaw {}", super::model::version_label(version))
            }
            Self::SetDefault { version } => format!(
                "Make jackdaw {} the default",
                super::model::version_label(version)
            ),
            Self::Verify { version } => {
                format!("Verify jackdaw {}", super::model::version_label(version))
            }
            Self::Locate => "Register an editor".into(),
            Self::Clean => "Remove unused nightlies".into(),
            Self::NewProject { name } => format!("Create {name}"),
        }
    }

    /// Whether the job shows in Downloads; the index refresh is quiet.
    pub fn listed(&self) -> bool {
        !matches!(self, Self::RefreshIndex)
    }
}

/// Where a job has got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Succeeded(String),
    Failed(String),
}

/// What to do once a job succeeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterJob {
    /// Open a project with the editor its pin selects.
    OpenProject(PathBuf),
    /// Open a project with this editor install.
    OpenWith { project: PathBuf, version: String },
    /// Give a project another editor made the starter scene, then open it
    /// with that editor.
    NewProject {
        project: PathBuf,
        version: String,
        starter: crate::scaffold::StarterScene,
    },
}

/// One run of `jd`.
#[derive(Debug)]
pub struct JdJob {
    pub id: u64,
    pub kind: JobKind,
    pub args: Vec<String>,
    pub status: JobStatus,
    /// Downloaded bytes and the total, while downloading.
    pub progress: Option<(u64, u64)>,
    /// The step after the download: `unpack`, `modules` or `register`.
    pub stage: Option<String>,
    pub then: Option<AfterJob>,
    events: Option<Mutex<Receiver<JdEvent>>>,
    child: Option<Mutex<Child>>,
}

impl JdJob {
    pub fn is_running(&self) -> bool {
        self.status == JobStatus::Running
    }

    /// Download progress as a fraction, when known.
    pub fn fraction(&self) -> Option<f32> {
        let (done, total) = self.progress?;
        (total > 0).then(|| done as f32 / total as f32)
    }

    /// One line on where the job is.
    pub fn summary(&self) -> String {
        match &self.status {
            JobStatus::Succeeded(text) | JobStatus::Failed(text) => text.clone(),
            JobStatus::Running => match (&self.stage, self.progress) {
                (Some(stage), _) => match stage.as_str() {
                    "unpack" => "Checking and unpacking".into(),
                    "modules" => "Fetching modules".into(),
                    "register" => "Registering".into(),
                    other => other.to_string(),
                },
                (None, Some((done, total))) => format!(
                    "{} of {}",
                    super::model::format_bytes(done),
                    super::model::format_bytes(total)
                ),
                (None, None) => "Starting".into(),
            },
        }
    }
}

/// One line `jd --json` printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JdEvent {
    Progress {
        done: u64,
        total: u64,
    },
    Stage(String),
    Message(String),
    Warning(String),
    Index(IndexReport),
    Done(String),
    Error(String),
    /// The process ended; `true` when it exited successfully.
    Exited {
        success: bool,
        stderr: String,
    },
}

/// What `jd editor refresh` said about the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexReport {
    pub fetched: bool,
    pub serial: u64,
    pub reason: Option<String>,
    pub jd_version: Option<String>,
}

/// Read one line of `jd --json` output. A line that is not a known event
/// is passed on as a message.
pub fn parse_event(line: &str) -> Option<JdEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Some(JdEvent::Message(line.to_string()));
    };
    let text = || value["text"].as_str().unwrap_or_default().to_string();
    Some(match value["event"].as_str().unwrap_or_default() {
        "progress" => JdEvent::Progress {
            done: value["done"].as_u64().unwrap_or(0),
            total: value["total"].as_u64().unwrap_or(0),
        },
        "stage" => JdEvent::Stage(value["stage"].as_str().unwrap_or_default().to_string()),
        "warning" => JdEvent::Warning(text()),
        "done" => JdEvent::Done(text()),
        "error" => JdEvent::Error(text()),
        "index" => JdEvent::Index(IndexReport {
            fetched: value["origin"] == "fetched",
            serial: value["serial"].as_u64().unwrap_or(0),
            reason: value["reason"].as_str().map(str::to_string),
            jd_version: value["jd"].as_str().map(str::to_string),
        }),
        _ => JdEvent::Message(text()),
    })
}

/// Every `jd` run this session, newest last.
#[derive(Resource, Default, Debug)]
pub struct JdJobs {
    pub jobs: Vec<JdJob>,
    next_id: u64,
}

impl JdJobs {
    pub fn running(&self) -> impl Iterator<Item = &JdJob> {
        self.jobs
            .iter()
            .filter(|job| job.is_running() && job.kind.listed())
    }

    pub fn listed(&self) -> impl Iterator<Item = &JdJob> {
        self.jobs.iter().filter(|job| job.kind.listed())
    }

    /// A running job of this kind, so the same install is not started
    /// twice.
    pub fn find_running(&self, kind: &JobKind) -> Option<&JdJob> {
        self.jobs
            .iter()
            .find(|job| job.is_running() && &job.kind == kind)
    }

    pub fn get(&self, id: u64) -> Option<&JdJob> {
        self.jobs.iter().find(|job| job.id == id)
    }
}

/// How the launcher last heard from the release index.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub enum IndexStatus {
    /// Not asked yet.
    #[default]
    Unknown,
    Checking,
    /// Fetched just now.
    Online {
        jd_version: Option<String>,
    },
    /// The fetch failed; the cached index, if any, is what is shown.
    Offline {
        reason: String,
        jd_version: Option<String>,
    },
}

/// Start `jd <args>` as a job and return its id. A failure to start shows
/// as a failed job.
pub fn start_job(
    world: &mut World,
    kind: JobKind,
    args: Vec<String>,
    then: Option<AfterJob>,
) -> u64 {
    let program = world
        .get_resource_or_insert_with(JdProgram::default)
        .0
        .clone();
    start_job_with(world, program, kind, args, then)
}

/// As [`start_job`], running the `jd` at `program`.
pub fn start_job_with(
    world: &mut World,
    program: PathBuf,
    kind: JobKind,
    args: Vec<String>,
    then: Option<AfterJob>,
) -> u64 {
    let mut jobs = world.get_resource_or_insert_with(JdJobs::default);
    if let Some(running) = jobs.find_running(&kind) {
        return running.id;
    }
    jobs.next_id += 1;
    let id = jobs.next_id;
    let mut job = JdJob {
        id,
        kind,
        args: args.clone(),
        status: JobStatus::Running,
        progress: None,
        stage: None,
        then,
        events: None,
        child: None,
    };
    match spawn(&program, &args) {
        Ok((child, events)) => {
            job.child = Some(Mutex::new(child));
            job.events = Some(Mutex::new(events));
        }
        Err(error) => {
            job.status = JobStatus::Failed(format!("Could not run {}: {error}", program.display()));
        }
    }
    info!("jd {}", args.join(" "));
    jobs.jobs.push(job);
    id
}

fn spawn(program: &PathBuf, args: &[String]) -> std::io::Result<(Child, Receiver<JdEvent>)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let stderr_text = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            }
            text
        });
        if let Some(stdout) = stdout {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Some(event) = parse_event(&line)
                    && sender.send(event).is_err()
                {
                    return;
                }
            }
        }
        let stderr = stderr_text.join().unwrap_or_default();
        let _ = sender.send(JdEvent::Exited {
            success: false,
            stderr,
        });
    });
    Ok((child, receiver))
}

/// Stop a running job.
pub fn cancel_job(jobs: &mut JdJobs, id: u64) {
    let Some(job) = jobs.jobs.iter_mut().find(|job| job.id == id) else {
        return;
    };
    if !job.is_running() {
        return;
    }
    if let Some(child) = &job.child
        && let Ok(mut child) = child.lock()
    {
        let _ = child.kill();
    }
    job.status = JobStatus::Failed("Cancelled".into());
}

/// A finished job, for what follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinishedJob {
    pub id: u64,
    pub kind: JobKind,
    pub succeeded: bool,
    pub then: Option<AfterJob>,
}

/// Apply what running jobs printed since the last frame. Returns the jobs
/// that finished, and whether any job changed.
pub fn drain_jobs(jobs: &mut JdJobs, index: &mut IndexStatus) -> (Vec<FinishedJob>, bool) {
    let mut finished = Vec::new();
    let mut changed = false;
    for job in jobs.jobs.iter_mut().filter(|job| job.is_running()) {
        let events: Vec<JdEvent> = match &job.events {
            Some(receiver) => match receiver.lock() {
                Ok(receiver) => receiver.try_iter().collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };
        changed |= !events.is_empty();
        let mut result: Option<String> = None;
        let mut error: Option<String> = None;
        for event in events {
            match event {
                JdEvent::Progress { done, total } => {
                    job.progress = Some((done, total));
                    job.stage = None;
                }
                JdEvent::Stage(stage) => job.stage = Some(stage),
                JdEvent::Message(text) => debug!("jd: {text}"),
                JdEvent::Warning(text) => warn!("jd: {text}"),
                JdEvent::Index(report) => {
                    *index = if report.fetched {
                        IndexStatus::Online {
                            jd_version: report.jd_version,
                        }
                    } else {
                        IndexStatus::Offline {
                            reason: report.reason.unwrap_or_default(),
                            jd_version: report.jd_version,
                        }
                    };
                }
                JdEvent::Done(text) => result = Some(text),
                JdEvent::Error(text) => error = Some(text),
                JdEvent::Exited { stderr, .. } => {
                    let success = job
                        .child
                        .as_ref()
                        .and_then(|child| child.lock().ok()?.wait().ok())
                        .is_some_and(|status| status.success());
                    job.status = if success {
                        JobStatus::Succeeded(result.take().unwrap_or_else(|| "Done".into()))
                    } else {
                        let reason = error
                            .take()
                            .or_else(|| last_line(&stderr))
                            .unwrap_or_else(|| "jd failed".into());
                        JobStatus::Failed(reason)
                    };
                    if job.kind == JobKind::RefreshIndex
                        && !success
                        && matches!(index, IndexStatus::Checking)
                    {
                        *index = IndexStatus::Offline {
                            reason: match &job.status {
                                JobStatus::Failed(reason) => reason.clone(),
                                _ => String::new(),
                            },
                            jd_version: None,
                        };
                    }
                    job.child = None;
                    job.events = None;
                    finished.push(FinishedJob {
                        id: job.id,
                        kind: job.kind.clone(),
                        succeeded: success,
                        then: job.then.clone(),
                    });
                    break;
                }
            }
        }
    }
    (finished, changed)
}

fn last_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_lines_become_events_and_anything_else_a_message() {
        assert_eq!(
            parse_event(r#"{"event":"progress","stage":"download","done":10,"total":40}"#),
            Some(JdEvent::Progress {
                done: 10,
                total: 40
            })
        );
        assert_eq!(
            parse_event(r#"{"event":"stage","stage":"unpack"}"#),
            Some(JdEvent::Stage("unpack".into()))
        );
        assert_eq!(
            parse_event(r#"{"event":"error","text":"0.30.0 is not installed"}"#),
            Some(JdEvent::Error("0.30.0 is not installed".into()))
        );
        assert_eq!(
            parse_event(
                r#"{"event":"index","origin":"cached","serial":4,"reason":"offline","jd":"0.20.0"}"#
            ),
            Some(JdEvent::Index(IndexReport {
                fetched: false,
                serial: 4,
                reason: Some("offline".into()),
                jd_version: Some("0.20.0".into()),
            }))
        );
        assert_eq!(
            parse_event("installing jackdaw"),
            Some(JdEvent::Message("installing jackdaw".into()))
        );
        assert_eq!(parse_event("  "), None);
    }

    #[test]
    fn a_job_that_cannot_start_is_listed_as_failed() {
        let mut world = World::new();
        world.insert_resource(JdProgram(PathBuf::from("/no/such/jd")));
        let id = start_job(
            &mut world,
            JobKind::Install {
                query: "0.20".into(),
            },
            vec!["editor".into(), "install".into(), "0.20".into()],
            None,
        );
        let jobs = world.resource::<JdJobs>();
        let job = jobs.get(id).expect("the job is listed");
        assert!(matches!(&job.status, JobStatus::Failed(reason) if reason.contains("/no/such/jd")));
        assert_eq!(job.kind.label(), "Install jackdaw 0.20");
    }
}
