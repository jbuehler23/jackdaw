//! Cancellable subprocess runner with live line-by-line streaming.
//!
//! [`run`] spawns a [`Command`], drains stdout/stderr on background
//! threads (pushing each line into a `mpsc::Sender<LogChunk>` and
//! accumulating into a `String` for the final record), and polls the
//! child until it exits or the caller flips the cancel flag. On
//! cancel the child is killed and reaped before returning.
//!
//! Pipe drainage runs on dedicated threads so the child cannot
//! deadlock on its own writes when a pipe buffer fills.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use bevy::log::info;

#[derive(Debug, Clone)]
pub enum LogChunk {
    Stdout(String),
    Stderr(String),
}

impl LogChunk {
    pub fn line(&self) -> &str {
        match self {
            Self::Stdout(s) | Self::Stderr(s) => s.as_str(),
        }
    }
}

/// Caller-owned I/O channels for [`run`]. Build one per invocation;
/// the cancel flag is sticky so don't reuse it across runs.
#[derive(Clone)]
pub struct CommandIo {
    pub cancel: Arc<AtomicBool>,
    pub log_tx: Sender<LogChunk>,
}

#[derive(Debug)]
pub struct CommandRecord {
    pub program: String,
    pub args: Vec<String>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum CommandError {
    /// `execve` itself failed.
    Spawn {
        program: String,
        args: Vec<String>,
        source: std::io::Error,
    },
    /// Child started and exited non-zero.
    Failed {
        record: CommandRecord,
        status: std::process::ExitStatus,
    },
    /// Caller flipped the cancel flag; child was killed and reaped.
    Cancelled { record: CommandRecord },
}

pub fn format_invocation(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn {
                program,
                args,
                source,
            } => write!(
                f,
                "failed to spawn `{}`: {source}",
                format_invocation(program, args)
            ),
            Self::Failed { record, status } => {
                write!(
                    f,
                    "`{}` exited with {status}",
                    format_invocation(&record.program, &record.args)
                )?;
                if !record.stderr.is_empty() {
                    write!(f, "\nstderr:\n{}", record.stderr)?;
                }
                if !record.stdout.is_empty() {
                    write!(f, "\nstdout:\n{}", record.stdout)?;
                }
                Ok(())
            }
            Self::Cancelled { record } => write!(
                f,
                "`{}` was cancelled",
                format_invocation(&record.program, &record.args)
            ),
        }
    }
}

impl std::error::Error for CommandError {}

pub fn run(cmd: &mut Command, io: &CommandIo) -> Result<CommandRecord, CommandError> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(source) => {
            return Err(CommandError::Spawn {
                program,
                args,
                source,
            });
        }
    };

    let stdout_handle = drain(
        child.stdout.take().expect("piped stdout"),
        io.log_tx.clone(),
        false,
    );
    let stderr_handle = drain(
        child.stderr.take().expect("piped stderr"),
        io.log_tx.clone(),
        true,
    );

    let mut cancelled = false;
    let status = loop {
        // Don't busy wait (try_wait is non-blocking).
        std::thread::sleep(Duration::from_millis(50));
        if !cancelled && io.cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            info!("Cancelled!");
            cancelled = true;
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(source) => {
                return Err(CommandError::Spawn {
                    program,
                    args,
                    source,
                });
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    let record = CommandRecord {
        program,
        args,
        stdout,
        stderr,
    };

    if cancelled {
        return Err(CommandError::Cancelled { record });
    }
    if !status.success() {
        return Err(CommandError::Failed { record, status });
    }
    Ok(record)
}

fn drain<R: Read + Send + 'static>(
    reader: R,
    tx: Sender<LogChunk>,
    is_stderr: bool,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = String::new();
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let chunk = if is_stderr {
                LogChunk::Stderr(line.clone())
            } else {
                LogChunk::Stdout(line.clone())
            };
            // Receiver dropped just means UI stopped listening; keep
            // accumulating the buffer so the final record is complete.
            let _ = tx.send(chunk);
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    })
}
