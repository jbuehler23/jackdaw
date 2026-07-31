#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! The prebuilt game runner boots a project dylib over the existing PIE
//! IPC with zero play-time compilation.
//!
//! Builds the runner shim for `reflect_game` (the fixture stand-in for the
//! crate the editor generates into `.jackdaw/`) into a Rust dylib
//! through the SDK pipeline, opens a PIE rendezvous the way the editor
//! does, launches the prebuilt `jackdaw-runner` binary against the
//! dylib, and asserts three things: the child connects over the IPC
//! link, the user's gameplay code actually runs (stderr markers from
//! `GamePlugin`), and rendered frames arrive on the `Frames` lane
//! after a `StartFrameStream` control message.
//!
//! ```text
//! cargo test --features "dylib runner" --target <host-triple> \
//!     --test runner_boots_project_dylib -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use jackdaw::project_build::{BuildEvent, build_project_dylib, shim_spec_for_project};
use jackdaw_pie_protocol::event::to_bytes;
use jackdaw_pie_protocol::{ControlEvent, PieChannel, decode_frame};

mod util;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn runner_boots_a_project_dylib_over_pie_ipc() {
    let sdk = jackdaw::sdk_paths::SdkPaths::for_workspace(&workspace_root());
    let triple = sdk.triple.clone();
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build with --features dylib --target {triple}"
    );
    assert!(sdk.wrapper_exists(), "wrapper missing");

    // Build the runner binary (prebuilt in production; here it shares
    // the SDK's cached build graph).
    let status = Command::new("cargo")
        .args(["build", "-p", "jackdaw_runner", "--target", &triple])
        .current_dir(workspace_root())
        .status()
        .expect("build jackdaw-runner");
    assert!(status.success(), "runner build failed");
    let runner = sdk.runner.clone();

    // Build the project dylib the way the editor does, through the real
    // shim. A hand-written stand-in used to live in
    // `tests/fixtures/shim_game`; it drifted from the generated one (the
    // entry was renamed and changed shape) and the runner then failed
    // with `undefined symbol: jackdaw_run_game`. Generating it removes
    // the copy that can drift.
    let project = util::stage_fixture("reflect_game");
    util::ensure_sdk_metadata(&sdk);
    let spec = shim_spec_for_project(&project, None).expect("the fixture is a lib crate");
    let jackdaw_dir = project.join(".jackdaw");
    let mut ignore_progress = |_: BuildEvent| {};
    let build = build_project_dylib(
        &spec,
        &jackdaw_dir,
        &sdk,
        Some(&workspace_root()),
        &mut ignore_progress,
    )
    .expect("build the project dylib through the pipeline");
    let shim_dir = build.dylib.parent().unwrap_or(&project).to_path_buf();
    let shim_dylib = build.dylib.clone();
    assert!(shim_dylib.exists(), "project dylib missing");

    // Open the PIE rendezvous the way the editor does, then launch the
    // runner as the editor would launch a game process.
    let (handle, server_name) = jackdaw_pie_protocol::serve().expect("open the ipc rendezvous");
    let mut child = Command::new(&runner)
        .arg(&shim_dylib)
        .current_dir(&shim_dir)
        .env("JACKDAW_PIE", &server_name)
        .env("JACKDAW_PIE_WINDOWLESS", "1")
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the runner");

    // Stream the child's stderr into a shared buffer; the GamePlugin
    // markers prove the user's code ran inside the runner process.
    let child_stderr = child.stderr.take().expect("piped stderr");
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let stderr_buf = std::sync::Arc::clone(&stderr_buf);
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(child_stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let mut buf = stderr_buf.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
            }
        });
    }

    // accept() blocks until the child connects; run it with a timeout.
    let (accept_tx, accept_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = accept_tx.send(handle.accept());
    });
    // The child's stderr says why it never got as far as connecting
    // (a missing adapter, a panic inside the loaded dylib). Without it
    // the failure is a bare timeout with nothing to act on.
    let accepted = accept_rx.recv_timeout(Duration::from_secs(60));
    let mut transport = match accepted {
        Ok(accepted) => accepted.expect("ipc accept failed"),
        Err(_) => {
            let captured = stderr_buf.lock().unwrap().clone();
            let captured = if captured.trim().is_empty() {
                "(the runner produced no output at all)".to_string()
            } else {
                captured
            };
            let _ = child.kill();
            panic!("the runner never connected to the PIE link. Its output was:\n{captured}");
        }
    };

    // Ask for frames and wait for the first one.
    use jackdaw_pie_protocol::PieTransport;
    let start = to_bytes(&ControlEvent::StartFrameStream {
        width: 320,
        height: 180,
    })
    .unwrap();
    transport.send(PieChannel::Reliable, &start);

    // Wait for all three signals before tearing the child down: a
    // decoded frame on the Frames lane, and both GamePlugin markers.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut frame_seen = false;
    let (mut started_seen, mut ticked_seen) = (false, false);
    while Instant::now() < deadline {
        for (channel, bytes) in transport.drain_received() {
            if channel == PieChannel::Frames && decode_frame(&bytes).is_some() {
                frame_seen = true;
            }
        }
        {
            let buf = stderr_buf.lock().unwrap();
            started_seen = buf.contains("JACKDAW_TEST_GAME_STARTED");
            ticked_seen = buf.contains("JACKDAW_TEST_GAME_TICKED count=1");
        }
        if frame_seen && started_seen && ticked_seen {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
    let stderr = stderr_buf.lock().unwrap().clone();

    assert!(
        started_seen,
        "GamePlugin startup never ran; runner stderr:\n{stderr}"
    );
    assert!(
        ticked_seen,
        "GamePlugin update systems never ticked; runner stderr:\n{stderr}"
    );
    assert!(
        frame_seen,
        "no frame arrived on the Frames lane; runner stderr:\n{stderr}"
    );

    println!("runner booted the project dylib, gameplay ran, frames streamed");
}
