//! `jd-mcp`: the Model Context Protocol server for a running editor.
//!
//! A separate binary from `jd` on purpose. The MCP stack (rmcp, tokio,
//! reqwest) is a dependency of this crate alone, so the editor -- which
//! needs none of it -- does not carry it. `jd mcp` finds and executes
//! this binary; see `src/bin/jd.rs`.
//!
//! Nothing may reach stdout but the protocol: stdout is the channel.

use std::path::PathBuf;
use std::process::ExitCode;

#[expect(clippy::print_stderr, reason = "MCP owns stdout; errors go to stderr")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        eprintln!("usage: jd-mcp [--project <path>]");
        return ExitCode::SUCCESS;
    }

    let project = args
        .iter()
        .position(|arg| arg == "--project" || arg == "-p")
        .and_then(|at| args.get(at + 1))
        .map(PathBuf::from)
        .or_else(|| {
            args.iter()
                .find(|arg| !arg.starts_with('-'))
                .map(PathBuf::from)
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();

    match jackdaw_mcp::serve_stdio_blocking(project) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("jd-mcp: {err}");
            ExitCode::FAILURE
        }
    }
}
