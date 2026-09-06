//! `jd mcp` against a real editor, over the wire it will actually use: an editor
//! process listening on a socket, a `jd mcp` child speaking MCP over its stdin
//! and stdout, and the handshake plus one tool call travelling between them.
//! `tests/guards/editor_remote.rs` pins the handlers' behaviour in process.
//!
//! It needs a window server, and skips without one unless
//! `JACKDAW_MCP_E2E_REQUIRED` is set.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long the editor gets to open the project and publish its endpoint.
const EDITOR_DEADLINE: Duration = Duration::from_secs(180);
/// How long one MCP request gets to come back.
const REQUEST_DEADLINE: Duration = Duration::from_secs(60);
/// The port this test's editor binds, clear of a developer's own editor
/// on the default 15703.
const TEST_PORT: &str = "15793";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Whether the MCP server binary sits beside `jd`, which is where `jd mcp` looks
/// for it. `jd-mcp` belongs to another package, so no `CARGO_BIN_EXE_` names it;
/// what both share is the workspace target directory.
fn mcp_binary_built() -> bool {
    Path::new(env!("CARGO_BIN_EXE_jd"))
        .parent()
        .is_some_and(|dir| dir.join("jd-mcp").is_file())
}

/// Whether anything can put a window on screen.
fn has_a_display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Skip, unless this machine promised to run it.
macro_rules! skip_unless_required {
    ($why:expr) => {{
        assert!(
            std::env::var_os("JACKDAW_MCP_E2E_REQUIRED").is_none(),
            "the mcp smoke test was required but {}",
            $why
        );
        println!("SKIP mcp_smoke: {}", $why);
        return;
    }};
}

/// A throwaway project for the editor to open.
fn scratch_project(root: &Path) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-mcp-smoke-")
        .tempdir_in(root.join("target"))
        .expect("a temp project");
    std::fs::create_dir_all(dir.path().join("assets")).expect("an assets dir");
    std::fs::create_dir_all(dir.path().join(".jackdaw")).expect("a state dir");
    std::fs::write(
        dir.path().join(".jackdaw/project.json"),
        json!({ "name": "mcp-smoke" }).to_string(),
    )
    .expect("a project config");
    dir
}

/// The editor, opened on `project`, or `None` if it never published its
/// endpoint.
fn start_editor(project: &Path) -> Option<Child> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jackdaw"))
        .env("JACKDAW_OPEN_PROJECT", project)
        .env("JACKDAW_SKIP_SETUP_CHECK", "1")
        .env("JACKDAW_REMOTE_PORT", TEST_PORT)
        .env("RUST_MIN_STACK", "33554432")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + EDITOR_DEADLINE;
    while Instant::now() < deadline {
        if jackdaw_env::editor_endpoint::read_endpoint(project).is_some() {
            return Some(child);
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// One MCP request written to the child's stdin, and the matching
/// response read back off its stdout.
fn request(
    stdin: &mut impl Write,
    stdout: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    writeln!(stdin, "{message}").expect("write the request");
    stdin.flush().expect("flush the request");

    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        let read = stdout.read_line(&mut line).expect("read a response line");
        assert!(
            read > 0,
            "jd mcp closed its stdout before answering {method}"
        );
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // Notifications and unrelated responses share the stream.
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
    panic!("{method} did not answer within {REQUEST_DEADLINE:?}");
}

/// A notification, which gets no response.
fn notify(stdin: &mut impl Write, method: &str) {
    let message = json!({ "jsonrpc": "2.0", "method": method, "params": {} });
    writeln!(stdin, "{message}").expect("write the notification");
    stdin.flush().expect("flush the notification");
}

#[test]
fn jd_mcp_initializes_and_reaches_a_running_editor() {
    if !has_a_display() {
        skip_unless_required!("there is no DISPLAY or WAYLAND_DISPLAY");
    }
    let root = workspace_root();
    let project = scratch_project(&root);

    let Some(mut editor) = start_editor(project.path()) else {
        skip_unless_required!("the editor did not start or never published .jackdaw/editor.json");
    };

    // Through `jd mcp`, not `jd-mcp` directly, so the exec wiring is
    // covered too. It is a separate binary, so a run that did not build
    // it has nothing to test rather than something to fail.
    if !mcp_binary_built() {
        let _ = editor.kill();
        let _ = editor.wait();
        skip_unless_required!("jd-mcp is not built; run `cargo build --bin jd-mcp`");
    }
    let mut mcp = Command::new(env!("CARGO_BIN_EXE_jd"))
        .arg("mcp")
        .arg("--project")
        .arg(project.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn jd mcp");
    let mut stdin = mcp.stdin.take().expect("the child's stdin");
    let mut stdout = BufReader::new(mcp.stdout.take().expect("the child's stdout"));

    let initialize = request(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "jackdaw-mcp-smoke", "version": "0" },
        }),
    );
    assert!(
        initialize["result"]["capabilities"]["tools"].is_object(),
        "the server advertises no tools: {initialize}"
    );
    notify(&mut stdin, "notifications/initialized");

    let tools = request(&mut stdin, &mut stdout, 2, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("a tool array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for wanted in [
        "status",
        "list_operators",
        "call_operator",
        "batch",
        "scene_tree",
        "get_entity",
        "apply_bsn",
        "scene_bsn",
        "open_scene",
        "save_scene",
        "select",
        "screenshot",
        "wait",
        "cancel",
        "assets",
    ] {
        assert!(names.contains(&wanted), "no `{wanted}` tool: {names:?}");
    }

    // The arguments a client reads off the schema rather than being told:
    // aiming and framing are what make one call enough to look at a scene.
    for (tool, argument) in [
        ("select", "frame"),
        ("screenshot", "look_at"),
        ("wait", "until"),
    ] {
        let schema = tools["result"]["tools"]
            .as_array()
            .expect("a tool array")
            .iter()
            .find(|listed| listed["name"] == json!(tool))
            .unwrap_or_else(|| panic!("no `{tool}` tool"));
        assert!(
            schema["inputSchema"]["properties"][argument].is_object(),
            "`{tool}` offers no `{argument}`: {schema}"
        );
    }

    let status = request(
        &mut stdin,
        &mut stdout,
        3,
        "tools/call",
        json!({ "name": "status", "arguments": {} }),
    );
    let reported = &status["result"]["structuredContent"];
    assert_eq!(
        reported["port"].as_u64(),
        TEST_PORT.parse::<u64>().ok(),
        "status came from the wrong editor: {status}"
    );
    assert_eq!(
        reported["project"].as_str().map(PathBuf::from),
        Some(project.path().to_path_buf()),
        "status names another project: {status}"
    );

    drop(stdin);
    let _ = mcp.wait();
    let _ = editor.kill();
    let _ = editor.wait();

    // A killed editor leaves no endpoint a client would act on. The
    // retraction on a clean exit is pinned in `tests/guards/editor_remote.rs`,
    // which can drive the exit rather than the kill this test has to use.
    assert!(
        !project.path().join(".jackdaw/editor.json").exists()
            || jackdaw_env::editor_endpoint::read_endpoint(project.path()).is_none(),
        "a killed editor left a live-looking endpoint behind"
    );
}
