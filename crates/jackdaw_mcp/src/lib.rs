//! An MCP server that drives a running Jackdaw editor.
//!
//! `jd mcp` speaks the Model Context Protocol over stdio and the editor's
//! remote-control BRP over loopback HTTP. It holds no editor state of its own:
//! every tool is a thin translation of one BRP method, and every write is
//! undoable, so anything a client does lands on the same undo stack a person's
//! clicks do.
//!
//! The editor is found through `<project>/.jackdaw/editor.json` (see
//! [`jackdaw_env::editor_endpoint`]), so nothing has to be configured with a
//! port.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use jackdaw_env::editor_endpoint::read_endpoint;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Longest a single BRP call may take. Generous, because
/// `jackdaw/screenshot` and `jackdaw/wait` hold the connection open until the
/// editor has caught up; it exists so a wedged editor produces an error rather
/// than a session that never answers.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The resource URI for the operator catalogue.
const OPERATORS_URI: &str = "jackdaw://operators";
/// The resource URI for the open scene as BSN.
const SCENE_URI: &str = "jackdaw://scene";

/// Talks BRP to one editor, found under one project root.
#[derive(Clone)]
pub struct JackdawMcp {
    project: PathBuf,
    http: reqwest::Client,
    /// Serial for the `request` id the waiting methods are keyed by.
    requests: Arc<AtomicU64>,
    tool_router: ToolRouter<Self>,
}

impl JackdawMcp {
    /// A server for the project rooted at `project`.
    pub fn new(project: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
            http: reqwest::Client::new(),
            requests: Arc::new(AtomicU64::new(0)),
            tool_router: Self::tool_router(),
        }
    }

    /// An id no other call of this session uses. The editor's waiting methods
    /// are re-run every frame with the same parameters, so two identical `wait`
    /// calls would otherwise share one countdown.
    fn next_request_id(&self) -> String {
        format!(
            "{}-{}",
            std::process::id(),
            self.requests.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Call one BRP method on the editor holding this project open.
    async fn brp(&self, method: &str, params: Value) -> Result<Value, ErrorData> {
        self.brp_answer(method, params)
            .await?
            .map_err(|error| ErrorData::internal_error(format!("{method}: {error}"), None))
    }

    /// Call one BRP method, handing back the editor's own error object rather
    /// than a message made from it.
    async fn brp_answer(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Result<Value, Value>, ErrorData> {
        let Some(endpoint) = read_endpoint(&self.project) else {
            return Err(ErrorData::internal_error(
                format!(
                    "no Jackdaw editor has {} open. Start one with `jd open {}`.",
                    self.project.display(),
                    self.project.display()
                ),
                None,
            ));
        };
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let response = self
            .http
            .post(endpoint.url())
            .timeout(CALL_TIMEOUT)
            .json(&request)
            .send()
            .await
            .map_err(|err| ErrorData::internal_error(format!("{method}: {err}"), None))?;
        let body: Value = response
            .json()
            .await
            .map_err(|err| ErrorData::internal_error(format!("{method}: {err}"), None))?;
        if let Some(error) = body.get("error") {
            return Ok(Err(error.clone()));
        }
        Ok(Ok(body.get("result").cloned().unwrap_or(Value::Null)))
    }

    /// A BRP call whose JSON result is the tool's whole answer.
    async fn brp_text(&self, method: &str, params: Value) -> Result<CallToolResult, ErrorData> {
        let result = self.brp(method, params).await?;
        Ok(json_result(&result))
    }
}

/// A JSON value as a tool result: one compact text block, the form every
/// client reads.
fn json_result(value: &Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(value.to_string())])
}

/// Longest side of a screenshot thumbnail when the caller names none.
const DEFAULT_MAX_PX: u32 = 512;

/// What a batch file holds: its calls, its undo label if it names one, and
/// any of the batch options in [`BATCH_OPTIONS`]. The file is either the call
/// list itself or `{"calls": [...], "label": "...", "stage": "...", ...}`.
struct BatchFile {
    calls: Vec<Value>,
    label: Option<String>,
    options: serde_json::Map<String, Value>,
}

/// The `jackdaw/batch` options a batch file or `run_batch_file` may set.
const BATCH_OPTIONS: [&str; 5] = ["atomic", "dry_run", "stage", "after", "checks"];

fn parse_batch_file(text: &str) -> Result<BatchFile, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|err| format!("not a JSON batch: {err}"))?;
    match value {
        Value::Array(calls) => Ok(BatchFile {
            calls,
            label: None,
            options: serde_json::Map::new(),
        }),
        Value::Object(mut fields) => {
            let Some(Value::Array(calls)) = fields.remove("calls") else {
                return Err("expected a list of calls, or {\"calls\": [...]}".to_string());
            };
            let label = fields
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_string);
            fields.retain(|key, _| BATCH_OPTIONS.contains(&key.as_str()));
            Ok(BatchFile {
                calls,
                label,
                options: fields,
            })
        }
        _ => Err("expected a list of calls, or {\"calls\": [...]}".to_string()),
    }
}

/// The `jackdaw/batch` request for a batch file: the file's options, each
/// overridden by the one the tool call names.
fn batch_file_request(file: BatchFile, label: String, args: &RunBatchFileArgs) -> Value {
    let mut request = file.options;
    let given = compact(json!({
        "atomic": args.atomic,
        "dry_run": args.dry_run,
        "stage": args.stage,
        "after": args.after,
        "checks": args.checks,
    }));
    if let Value::Object(given) = given {
        request.extend(given);
    }
    request.insert("calls".to_string(), Value::Array(file.calls));
    request.insert("label".to_string(), Value::String(label));
    Value::Object(request)
}

/// Copy what a batch said about its stage, its checks and a rollback from
/// `source` (an answer or an error's data) into `summary`. Only the checks
/// that failed are listed. A batch that was taken back spawned nothing; what
/// its calls added before that is reported as `would_spawn`.
fn add_batch_outcome(summary: &mut Value, source: &Value) {
    for key in ["stage", "dry_run", "rolled_back"] {
        if let Some(value) = source.get(key) {
            summary[key] = value.clone();
        }
    }
    let taken_back = [&source["rolled_back"], &source["dry_run"]]
        .into_iter()
        .any(|flag| *flag == json!(true));
    if taken_back {
        summary["would_spawn"] = summary["spawned"].take();
        summary["spawned"] = json!(0);
    }
    if let Some(checks) = source.get("checks") {
        let failing: Vec<Value> = checks["results"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, result)| result["pass"] != json!(true))
            .map(|(index, result)| {
                let mut result = result.clone();
                result["index"] = json!(index);
                result
            })
            .collect();
        summary["checks"] = json!({ "passed": checks["passed"], "failed": checks["failed"] });
        if !failing.is_empty() {
            summary["checks"]["failing"] = json!(failing);
        }
    }
}

/// `path` resolved against the project root, refused when it leads outside.
fn file_in_project(project: &std::path::Path, path: &str) -> Result<PathBuf, String> {
    let root = project
        .canonicalize()
        .map_err(|err| format!("{}: {err}", project.display()))?;
    let file = root
        .join(path)
        .canonicalize()
        .map_err(|err| format!("{path}: {err}"))?;
    if file.starts_with(&root) {
        Ok(file)
    } else {
        Err(format!("{path} is outside the project"))
    }
}

/// What a batch did, without the per-call outcomes: how many calls ran, how
/// many entities they added, and every warning with the call it came from.
/// `failure` is the editor's error when a call failed; a call that stopped the
/// batch by not finishing is reported with its outcome under `stopped`.
fn batch_summary(total: usize, calls: &[Value], failure: Option<(&Value, &str)>) -> Value {
    let spawned: usize = calls
        .iter()
        .filter_map(|call| call.get("entities").and_then(Value::as_array))
        .map(Vec::len)
        .sum();
    let warnings: Vec<String> = calls
        .iter()
        .enumerate()
        .flat_map(|(index, call)| {
            call.get("warnings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(move |warning| format!("call {index}: {warning}"))
        })
        .collect();
    let mut summary = json!({ "ran": calls.len(), "of": total, "spawned": spawned });
    if !warnings.is_empty() {
        summary["warnings"] = json!(warnings);
    }
    if let Some((failed_at, message)) = failure {
        summary["failed_at"] = failed_at.clone();
        summary["error"] = json!(message);
    } else if let Some(last) = calls.last()
        && (last["result"] != json!("finished") || calls.len() < total)
    {
        // A call that ends `cancelled` or `running` stops the batch without
        // an error; its outcome says why.
        summary["failed_at"] = json!(calls.len() - 1);
        summary["stopped"] = last.clone();
    }
    summary
}

/// Drop `null` fields so an omitted optional argument is absent from the
/// BRP call rather than explicitly null.
fn compact(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, field| !field.is_null());
    }
    value
}

// --- Tool arguments ---

/// Arguments for `list_operators`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListOperatorsArgs {
    /// Only operators whose id starts with this, e.g. `terrain.`.
    #[serde(default)]
    pub prefix: Option<String>,
}

/// Arguments for `call_operator`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CallOperatorArgs {
    /// Operator id, e.g. `entity.add.cube`.
    pub id: String,
    /// Parameters by name. Values are typed from the operator's schema.
    #[serde(default)]
    pub params: Option<Value>,
    /// Whether the call pushes an undo entry. Defaults to true.
    #[serde(default)]
    pub history: Option<bool>,
    /// Report the whole outcome. By default only how the call ended, the
    /// entities it added and any warnings come back.
    #[serde(default)]
    pub verbose: Option<bool>,
}

/// Arguments for `describe_operator`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DescribeOperatorArgs {
    /// Operator id, e.g. `entity.add.cube`.
    pub id: String,
}

/// Arguments for `batch`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct BatchArgs {
    /// The calls to run, each `{"id": ..., "params": {...}}`. A string
    /// parameter `$N.path` takes a value from call N's outcome, e.g.
    /// `$0.entities.0` for the first entity call 0 added; `$$` spells a
    /// literal leading `$`.
    pub calls: Vec<Value>,
    /// Undo label for the whole batch.
    #[serde(default)]
    pub label: Option<String>,
    /// Report every call's whole outcome rather than the compact one.
    #[serde(default)]
    pub verbose: Option<bool>,
    /// Take the whole batch back if a call fails, does not finish, or a check
    /// fails.
    #[serde(default)]
    pub atomic: Option<bool>,
    /// Run the calls and checks, report them, then take everything back.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Build into the stage group of this name, replacing what an earlier
    /// run built there. A staged batch is atomic.
    #[serde(default)]
    pub stage: Option<String>,
    /// Stages that must already exist before this one builds.
    #[serde(default)]
    pub after: Option<Vec<String>>,
    /// `check` assertions evaluated after the calls; one that fails fails the
    /// batch. `$N.path` references work here too.
    #[serde(default)]
    pub checks: Option<Vec<Value>>,
}

/// Arguments for `run_batch_file`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RunBatchFileArgs {
    /// JSON file under the project root: a list of calls as `batch` takes
    /// them, or `{"calls": [...], "label": "..."}`.
    pub path: String,
    /// Undo label, over the file's own. The file name when neither names one.
    #[serde(default)]
    pub label: Option<String>,
    /// Take the whole batch back on any failure. Over the file's own.
    #[serde(default)]
    pub atomic: Option<bool>,
    /// Run and report, then take everything back. Over the file's own.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Build into the stage group of this name, replacing its last build.
    /// Over the file's own.
    #[serde(default)]
    pub stage: Option<String>,
    /// Stages that must already exist. Over the file's own.
    #[serde(default)]
    pub after: Option<Vec<String>>,
    /// `check` assertions run after the calls. Over the file's own.
    #[serde(default)]
    pub checks: Option<Vec<Value>>,
}

/// Arguments for `scene_tree`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SceneTreeArgs {
    /// Entity id or name to start from. The scene roots when omitted.
    #[serde(default)]
    pub root: Option<Value>,
    /// Generations of children to include: `0` is the node alone, `1`
    /// adds its children. The whole subtree when omitted.
    #[serde(default)]
    pub depth: Option<u32>,
}

/// Arguments for `scene_summary`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SceneSummaryArgs {
    /// Entity id or name to start from. The whole scene when omitted.
    #[serde(default)]
    pub root: Option<Value>,
    /// Generations of children to include below `root`. All when omitted.
    #[serde(default)]
    pub depth: Option<u32>,
    /// Columns after `id`, from name, parent, pos, rot, size, bounds,
    /// material, kind, faces, verts, modifiers. Defaults to name, parent,
    /// kind, pos, size.
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    /// A `revision` from an earlier summary of the same scope: answer with
    /// only the rows added, changed and removed since.
    #[serde(default)]
    pub since_revision: Option<u64>,
    /// Keep only nodes matching `{name_prefix?, kind?}`, where kind is one
    /// of brush, terrain, prefab, light, camera, model, mesh, ui, empty.
    #[serde(default)]
    pub filter: Option<Value>,
}

/// Arguments for `check`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CheckArgs {
    /// Assertions, each `{"<kind>": {...}}`: `rests_on {entity, on,
    /// tolerance?}`, `inside {entity, of, tolerance?}`, `no_overlap
    /// {entities, tolerance?}`, `bounds {entity, min?, max?, tolerance?}`,
    /// `count {filter, eq?, min?, max?}`, `watertight {entity}`. Entities
    /// are ids or names; tolerance defaults to 0.01.
    pub assertions: Vec<Value>,
}

/// Arguments for `get_entity`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct EntityArgs {
    /// Entity id, as reported by `scene_tree`.
    #[serde(default)]
    pub entity: Option<u64>,
    /// The entity's `Name`, when it has a unique one.
    #[serde(default)]
    pub name: Option<String>,
}

/// Arguments for `brush_topology`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrushTopologyArgs {
    /// Brush entity id.
    #[serde(default)]
    pub entity: Option<u64>,
    /// The brush's `Name`, when it has a unique one.
    #[serde(default)]
    pub name: Option<String>,
    /// Arrays to return, from positions, polygons, edges, normals, colors.
    /// Defaults to positions and polygons.
    #[serde(default)]
    pub fields: Option<Vec<String>>,
}

/// Arguments for `apply_bsn`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ApplyBsnArgs {
    /// BSN text to spawn into the open scene.
    pub source: String,
    /// Entity id or name of the node that adopts what is spawned.
    #[serde(default)]
    pub parent: Option<Value>,
}

/// Arguments for `open_scene`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OpenSceneArgs {
    /// Scene file, relative to the project's `assets/` directory.
    pub path: String,
}

/// Arguments for `select`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SelectArgs {
    /// Names of the entities to select, in order. The last is primary.
    pub names: Vec<String>,
    /// Frame the viewport on the selection once it is made.
    #[serde(default)]
    pub frame: Option<bool>,
}

/// Arguments for `screenshot`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ScreenshotArgs {
    /// `viewport` (the 3D view, the default), `viewport2d` or `window`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Where to write the PNG, relative to the project root. A
    /// timestamped file under the project when omitted.
    #[serde(default)]
    pub path: Option<String>,
    /// Where to aim the viewport camera before capturing.
    #[serde(default)]
    pub look_at: Option<LookAt>,
    /// Keep only part of the capture.
    #[serde(default)]
    pub region: Option<Region>,
    /// Longest side of the returned image in pixels, applied after `region`.
    /// Defaults to 512; 0 returns the capture at full size. The full capture
    /// stays on disk at `path` either way.
    #[serde(default)]
    pub max_px: Option<u32>,
    /// `png` (the default) or `jpeg` for the returned image. JPEG is smaller
    /// but blurs edges.
    #[serde(default)]
    pub format: Option<String>,
}

impl ScreenshotArgs {
    /// The `jackdaw/screenshot` parameters for these arguments.
    fn brp_params(&self, request: String) -> Value {
        let max_px = self.max_px.unwrap_or(DEFAULT_MAX_PX);
        let thumbnail = max_px > 0;
        compact(json!({
            "kind": self.kind,
            "path": self.path,
            "region": self.region,
            "max_px": thumbnail.then_some(max_px),
            "format": self.format.as_ref().filter(|_| thumbnail),
            "request": request,
        }))
    }
}

/// The part of a screenshot to keep, from its minimum corner. Clamped to the
/// image; refused when nothing is left.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct Region {
    /// Left edge in pixels, or minimum x in world units.
    pub x: f64,
    /// Top edge in pixels (y down), or minimum y in world units (y up).
    pub y: f64,
    /// Width, in the same units.
    pub w: f64,
    /// Height, in the same units.
    pub h: f64,
    /// `world` (the 2D scene's units, the default for a 2D view) or
    /// `pixels` (image pixels from the top-left, the only choice for the 3D
    /// view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
}

/// An eye and a target in world metres, as `view.look_at` takes them.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct LookAt {
    /// Where the camera sits, as `[x, y, z]`.
    pub eye: [f64; 3],
    /// What it looks at, as `[x, y, z]`. The origin when omitted.
    #[serde(default)]
    pub target: Option<[f64; 3]>,
}

/// Arguments for `wait`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct WaitArgs {
    /// Frames to let pass. Defaults to 1.
    #[serde(default)]
    pub frames: Option<u32>,
    /// Wait until no bake, build or modal tool is running instead.
    #[serde(default)]
    pub until_idle: Option<bool>,
    /// A state to hold for instead of frames: `idle`, `pie_running` or
    /// `pie_stopped`.
    #[serde(default)]
    pub until: Option<String>,
}

/// Arguments for `assets`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct AssetsArgs {
    /// Substring or `*` glob matched against paths under `assets/`,
    /// e.g. `*Fence*.gltf`.
    #[serde(default)]
    pub glob: Option<String>,
    /// Report each path as `{path, kind, clips, binary}` instead of a bare
    /// string, where `clips` names the animations a glTF file holds and
    /// `binary` says the document is held in the binary form.
    #[serde(default)]
    pub details: Option<bool>,
}

// --- Tools ---

#[tool_router(router = tool_router)]
impl JackdawMcp {
    /// What the editor has open and what is selected.
    #[tool(
        description = "What the Jackdaw editor has open: project, scene, dirty flag, selection."
    )]
    pub async fn status(&self) -> Result<CallToolResult, ErrorData> {
        self.brp_text("jackdaw/status", json!({})).await
    }

    /// Every operator the editor will answer to.
    #[tool(
        description = "List the editor's operators with their parameter schemas. Everything the \
                       editor can do is an operator, so this is the whole vocabulary. Filter with \
                       a prefix such as `terrain.` or `entity.`."
    )]
    pub async fn list_operators(
        &self,
        Parameters(args): Parameters<ListOperatorsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/operators",
            compact(json!({ "prefix": args.prefix })),
        )
        .await
    }

    /// One operator's schema.
    #[tool(
        description = "One operator's id, label, description, availability and parameter \
                       schema, by exact id. Cheaper than listing a whole prefix when you know \
                       the id."
    )]
    pub async fn describe_operator(
        &self,
        Parameters(args): Parameters<DescribeOperatorArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = self
            .brp("jackdaw/operators", json!({ "id": args.id }))
            .await?;
        let found = result["operators"]
            .as_array()
            .and_then(|ops| ops.iter().find(|op| op["id"] == json!(args.id)));
        match found {
            Some(operator) => Ok(json_result(operator)),
            None => Err(ErrorData::invalid_params(
                format!(
                    "no operator `{}`; `list_operators` with a prefix finds ids",
                    args.id
                ),
                None,
            )),
        }
    }

    /// Run one operator.
    #[tool(
        description = "Run one editor operator by id. Parameters are typed from the operator's \
                       own schema, so a string reaches a float parameter. The call pushes an undo \
                       entry unless `history` is false."
    )]
    pub async fn call_operator(
        &self,
        Parameters(args): Parameters<CallOperatorArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/call_operator",
            compact(json!({
                "id": args.id,
                "params": args.params,
                "history": args.history,
                "verbose": args.verbose,
            })),
        )
        .await
    }

    /// Run several operators as one undo entry.
    #[tool(
        description = "Run several operator calls as a single undo entry. Stops at the first \
                       call that does not finish and reports which one. A string parameter \
                       `$N.path` takes a value from an earlier call's outcome, e.g. \
                       `\"parent\": \"$0.entities.0\"`; a malformed or forward reference \
                       refuses the batch before anything runs. Write `$$` for text that \
                       starts with a literal `$`, e.g. `\"$$100 Prop\"`. `atomic` takes \
                       everything back on any failure; `dry_run` runs, reports and takes back; \
                       `stage` rebuilds the named stage group in place (re-run safe, atomic); \
                       `checks` are `check` assertions that fail the batch. These refuse calls \
                       that write to disk."
    )]
    pub async fn batch(
        &self,
        Parameters(args): Parameters<BatchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/batch",
            compact(json!({
                "calls": args.calls,
                "label": args.label,
                "verbose": args.verbose,
                "atomic": args.atomic,
                "dry_run": args.dry_run,
                "stage": args.stage,
                "after": args.after,
                "checks": args.checks,
            })),
        )
        .await
    }

    /// Run a batch held in a file and report only a summary.
    #[tool(
        description = "Run a JSON file of operator calls as one batch (one undo entry) and \
                       return a summary: calls run, entities added, warnings, and where it \
                       stopped. The calls never pass through the conversation, so write long \
                       scripts to a file and run them with this. Same call and `$N.path` \
                       format as `batch`; the path is relative to the project root. The file \
                       or the arguments may set `atomic`, `dry_run`, `stage`, `after` and \
                       `checks` as `batch` takes them; arguments win."
    )]
    pub async fn run_batch_file(
        &self,
        Parameters(args): Parameters<RunBatchFileArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let file = file_in_project(&self.project, &args.path)
            .map_err(|err| ErrorData::invalid_params(err, None))?;
        let text = tokio::task::spawn_blocking({
            let file = file.clone();
            move || std::fs::read_to_string(file)
        })
        .await
        .map_err(|err| ErrorData::internal_error(format!("run_batch_file: {err}"), None))?
        .map_err(|err| ErrorData::invalid_params(format!("{}: {err}", args.path), None))?;
        let batch = parse_batch_file(&text)
            .map_err(|err| ErrorData::invalid_params(format!("{}: {err}", args.path), None))?;
        let label = args
            .label
            .clone()
            .or(batch.label.clone())
            .unwrap_or_else(|| {
                file.file_name().map_or_else(
                    || args.path.clone(),
                    |name| name.to_string_lossy().into_owned(),
                )
            });
        let total = batch.calls.len();
        let request = batch_file_request(batch, label, &args);
        let answer = self.brp_answer("jackdaw/batch", request).await?;
        Ok(match answer {
            Ok(result) => {
                let done = result["calls"].as_array().cloned().unwrap_or_default();
                let mut summary = batch_summary(total, &done, None);
                add_batch_outcome(&mut summary, &result);
                if summary.get("failed_at").is_some() {
                    CallToolResult::error(vec![ContentBlock::text(summary.to_string())])
                } else {
                    json_result(&summary)
                }
            }
            Err(error) => {
                let data = &error["data"];
                let done = data["calls"].as_array().cloned().unwrap_or_default();
                let message = error["message"].as_str().unwrap_or_default();
                let mut summary = batch_summary(total, &done, Some((&data["failed_at"], message)));
                add_batch_outcome(&mut summary, data);
                CallToolResult::error(vec![ContentBlock::text(summary.to_string())])
            }
        })
    }

    /// The scene as the outliner shows it.
    #[tool(
        description = "The open scene's entity tree: ids, names, component type paths. Large; \
                       use `scene_summary` and `check` to verify edits."
    )]
    pub async fn scene_tree(
        &self,
        Parameters(args): Parameters<SceneTreeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/scene_tree",
            compact(json!({ "root": args.root, "depth": args.depth })),
        )
        .await
    }

    /// The scene as compact columnar rows, or what changed since a revision.
    #[tool(
        description = "The open scene as compact rows: `columns` names each value and every \
                       row starts with the entity id. Positions are world space, sizes and \
                       bounds are world-space boxes, rotations are [yaw, pitch, roll] degrees, \
                       all rounded to 3 decimals. Pass the answer's `revision` back as \
                       `since_revision` (same root, depth, fields and filter) to get only the \
                       added, changed and removed rows."
    )]
    pub async fn scene_summary(
        &self,
        Parameters(args): Parameters<SceneSummaryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/scene_summary",
            compact(json!({
                "root": args.root,
                "depth": args.depth,
                "fields": args.fields,
                "since_revision": args.since_revision,
                "filter": args.filter,
            })),
        )
        .await
    }

    /// Geometric assertions about the scene, each with what it measured.
    #[tool(
        description = "Verify the scene without a screenshot: each assertion passes or fails \
                       and reports what it measured. `rests_on` gives the gap from the \
                       entity's bottom to the top of `on` (or the ground of a terrain) and \
                       whether they overlap seen from above; `inside` how far it sticks out; \
                       `no_overlap` the deepest overlap and the pairs; `bounds` the measured \
                       box; `count` the matching nodes; `watertight` a brush's open edges."
    )]
    pub async fn check(
        &self,
        Parameters(args): Parameters<CheckArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text("jackdaw/check", json!({ "assertions": args.assertions }))
            .await
    }

    /// A brush's element arrays, for picking indices.
    #[tool(
        description = "A brush's faces, edges and vertices as compact brush-local arrays: \
                       `positions` per vertex, `polygons` as vertex rings (face index = \
                       position), `edges` as vertex pairs (edge index = position), `normals` \
                       per face, `colors` when painted. `revision` changes when the elements \
                       are renumbered. Prefer selecting by rule with the \
                       `brush.select_elements` operator's `where` over reading indices."
    )]
    pub async fn brush_topology(
        &self,
        Parameters(args): Parameters<BrushTopologyArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/brush_topology",
            compact(json!({ "entity": args.entity, "name": args.name, "fields": args.fields })),
        )
        .await
    }

    /// One node as BSN text.
    #[tool(description = "One entity and its descendants as BSN text, by entity id or by name.")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<EntityArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/entity",
            compact(json!({ "entity": args.entity, "name": args.name })),
        )
        .await
    }

    /// Spawn BSN text into the open scene.
    #[tool(
        description = "Spawn BSN text into the open scene, optionally under a named parent. Use \
                       this for structure the operators do not model; everything it spawns joins \
                       the scene document and saves with it."
    )]
    pub async fn apply_bsn(
        &self,
        Parameters(args): Parameters<ApplyBsnArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/apply_bsn",
            compact(json!({ "source": args.source, "parent": args.parent })),
        )
        .await
    }

    /// The whole open document as BSN text.
    #[tool(description = "The whole open scene as BSN text, as saving it would write it.")]
    pub async fn scene_bsn(&self) -> Result<CallToolResult, ErrorData> {
        self.brp_text("jackdaw/scene_bsn", json!({})).await
    }

    /// Open a scene by its asset-relative path.
    #[tool(description = "Open a scene file, relative to the project's assets directory.")]
    pub async fn open_scene(
        &self,
        Parameters(args): Parameters<OpenSceneArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/call_operator",
            json!({ "id": "scene.open", "params": { "path": args.path } }),
        )
        .await
    }

    /// Save the open scene.
    #[tool(
        description = "Save the open scene to its file. The only tool here that writes to disk."
    )]
    pub async fn save_scene(&self) -> Result<CallToolResult, ErrorData> {
        self.brp_text("jackdaw/call_operator", json!({ "id": "scene.save" }))
            .await
    }

    /// Select entities by name.
    #[tool(
        description = "Select entities by name, so operators that act on the selection have a \
                       target. The last name becomes the primary selection. With `frame`, the \
                       viewport camera moves onto the selection in the same call."
    )]
    pub async fn select(
        &self,
        Parameters(args): Parameters<SelectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut calls: Vec<Value> = args
            .names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                json!({
                    "id": if index == 0 { "selection.select" } else { "selection.extend" },
                    "params": { "name": name },
                })
            })
            .collect();
        if args.frame.unwrap_or(false) {
            calls.push(json!({ "id": "view.frame_selected" }));
        }
        self.brp_text(
            "jackdaw/batch",
            json!({ "calls": calls, "label": "Select" }),
        )
        .await
    }

    /// Capture the editor and hand the PNG back.
    #[tool(
        description = "Capture the viewport (or the whole window) to a PNG and return a \
                       thumbnail (512 px on the longer side unless `max_px` says otherwise; 0 \
                       for full size) along with the paths. Waits for the capture to reach \
                       disk, so the file is there when this answers. `look_at` aims the camera \
                       first, since a camera left where it was frames a scene edge-on. \
                       `region` keeps only part of the image: world units by default for a 2D \
                       view, pixels from the top-left otherwise."
    )]
    pub async fn screenshot(
        &self,
        Parameters(args): Parameters<ScreenshotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(look_at) = &args.look_at {
            let target = look_at.target.unwrap_or([0.0; 3]);
            self.brp(
                "jackdaw/call_operator",
                json!({
                    "id": "view.look_at",
                    "params": {
                        "eye_x": look_at.eye[0],
                        "eye_y": look_at.eye[1],
                        "eye_z": look_at.eye[2],
                        "target_x": target[0],
                        "target_y": target[1],
                        "target_z": target[2],
                    },
                }),
            )
            .await?;
        }
        let result = self
            .brp(
                "jackdaw/screenshot",
                args.brp_params(self.next_request_id()),
            )
            .await?;
        let (path, mime) = match result["thumb"]["path"].as_str() {
            Some(thumb) if thumb.ends_with(".jpg") => (thumb, "image/jpeg"),
            Some(thumb) => (thumb, "image/png"),
            None => (result["path"].as_str().unwrap_or_default(), "image/png"),
        };
        let path = path.to_string();
        let mut blocks = vec![ContentBlock::text(result.to_string())];
        // Off the runtime, like the asset walk: a screenshot of a large
        // viewport is megabytes, and a blocking read here stalls every
        // other task the server is serving.
        let read = tokio::task::spawn_blocking({
            let path = path.clone();
            move || std::fs::read(path)
        })
        .await
        .map_err(|err| ErrorData::internal_error(format!("screenshot: {err}"), None))?;
        match read {
            Ok(bytes) => blocks.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                mime,
            )),
            Err(err) => blocks.push(ContentBlock::text(format!(
                "the editor reported {path} but it could not be read: {err}"
            ))),
        }
        Ok(CallToolResult::success(blocks))
    }

    /// End a modal operator holding the editor.
    #[tool(
        description = "End the modal operator holding the editor, if one is. A modal operator \
                       waits for a pointer that a caller does not have; while it is active every \
                       other modal call is refused. `status` reports it under `modal`."
    )]
    pub async fn cancel(&self) -> Result<CallToolResult, ErrorData> {
        self.brp_text("jackdaw/cancel", json!({})).await
    }

    /// Let the editor catch up.
    #[tool(
        description = "Let frames pass, or hold until the editor reaches a state: `idle` (no \
                       bake, build, modal tool or synthetic input still running), `pie_running` \
                       or `pie_stopped`. Use after opening a scene, starting a bake, sending \
                       input or pressing play, before reading or capturing."
    )]
    pub async fn wait(
        &self,
        Parameters(args): Parameters<WaitArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let until = args
            .until
            .clone()
            .or_else(|| args.until_idle.unwrap_or(false).then(|| "idle".to_string()));
        let params = match until {
            Some(until) => json!({ "until": until }),
            None => json!({
                "frames": args.frames.unwrap_or(1),
                "request": self.next_request_id(),
            }),
        };
        self.brp_text("jackdaw/wait", params).await
    }

    /// What is in the project's assets directory.
    #[tool(
        description = "List asset paths under the project's assets directory, so you can see \
                       which kit pieces, models and scenes exist before placing any. Pass \
                       details=true to get each path's kind and, for a glTF file, its clip names."
    )]
    pub async fn assets(
        &self,
        Parameters(args): Parameters<AssetsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/assets",
            compact(json!({ "glob": args.glob, "details": args.details })),
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for JackdawMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        );
        info.protocol_version = ProtocolVersion::default();
        info.instructions = Some(
            "Drives a running Jackdaw editor. Everything the editor does is an operator: \
                 use `list_operators` with a prefix or `describe_operator` to learn one; never \
                 load the `operators` resource whole. Run one with `call_operator`. Group a run \
                 of calls with `batch` so one undo takes them all back, naming earlier results \
                 with `$N.path`; for long scripts write the calls to a file and use \
                 `run_batch_file`, which answers with a summary. To read and verify the scene \
                 prefer `scene_summary` (compact rows; pass its `revision` back as \
                 `since_revision` for only what changed) and `check` (rests_on, inside, \
                 no_overlap, bounds, count, watertight, each with the measured value) over \
                 `scene_tree` and screenshots. `get_entity` reads one node in full, \
                 `screenshot` is for how things look, and `save_scene` writes the open scene back to its file. Operators reach the disk \
                 too, through the same saves, exports and bakes the menus run."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new(OPERATORS_URI, "operators")
                    .with_title("Editor operators")
                    .with_description("Every operator the editor answers to, with its parameters.")
                    .with_mime_type("application/json"),
                Resource::new(SCENE_URI, "scene")
                    .with_title("Open scene")
                    .with_description("The open scene as BSN text.")
                    .with_mime_type("text/plain"),
            ],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        match request.uri.as_str() {
            OPERATORS_URI => {
                let result = self.brp("jackdaw/operators", json!({})).await?;
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    result.to_string(),
                    OPERATORS_URI,
                )])
                .into())
            }
            SCENE_URI => {
                let result = self.brp("jackdaw/scene_bsn", json!({})).await?;
                let bsn = result
                    .get("bsn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(ReadResourceResult::new(vec![ResourceContents::text(bsn, SCENE_URI)]).into())
            }
            other => Err(ErrorData::resource_not_found(
                format!("no such resource: {other}"),
                None,
            )),
        }
    }
}

/// Serve MCP over stdio until the client disconnects.
///
/// Nothing here may write to stdout: stdout *is* the protocol channel.
pub async fn serve_stdio(project: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use rmcp::ServiceExt;

    let service = JackdawMcp::new(project)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

/// [`serve_stdio`] with its own runtime, for a caller that has none: `jd` is a
/// plain synchronous CLI.
pub fn serve_stdio_blocking(project: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve_stdio(project))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An omitted optional argument must not reach the editor as an
    /// explicit null, which a handler that only checks presence would
    /// read as "a value was given".
    #[test]
    fn an_omitted_argument_is_dropped_rather_than_sent_as_null() {
        let params = compact(json!({ "kind": "viewport", "path": Value::Null }));
        assert_eq!(params, json!({ "kind": "viewport" }));
    }

    #[test]
    fn a_screenshot_region_is_forwarded_and_an_omitted_one_is_not() {
        let args: ScreenshotArgs = serde_json::from_value(json!({
            "kind": "viewport2d",
            "region": { "x": -8.0, "y": 4.0, "w": 16.0, "h": 16.0, "space": "world" },
        }))
        .expect("region arguments parse");
        assert_eq!(
            args.brp_params("r1".to_string()),
            json!({
                "kind": "viewport2d",
                "region": { "x": -8.0, "y": 4.0, "w": 16.0, "h": 16.0, "space": "world" },
                "max_px": 512,
                "request": "r1",
            }),
        );

        let args: ScreenshotArgs = serde_json::from_value(json!({
            "region": { "x": 0.0, "y": 0.0, "w": 64.0, "h": 32.0 },
        }))
        .expect("a region without a space parses");
        let params = args.brp_params("r2".to_string());
        assert_eq!(
            params["region"],
            json!({ "x": 0.0, "y": 0.0, "w": 64.0, "h": 32.0 })
        );

        let args: ScreenshotArgs = serde_json::from_value(json!({})).expect("no arguments");
        assert_eq!(
            args.brp_params("r3".to_string()),
            json!({ "max_px": 512, "request": "r3" })
        );
    }

    #[test]
    fn a_screenshot_asks_for_a_thumbnail_unless_max_px_is_zero() {
        let args: ScreenshotArgs =
            serde_json::from_value(json!({ "max_px": 256, "format": "jpeg" })).expect("parses");
        let params = args.brp_params("r".to_string());
        assert_eq!(params["max_px"], json!(256));
        assert_eq!(params["format"], json!("jpeg"));

        let args: ScreenshotArgs =
            serde_json::from_value(json!({ "max_px": 0, "format": "jpeg" })).expect("parses");
        assert_eq!(
            args.brp_params("r".to_string()),
            json!({ "request": "r" }),
            "full size asks for no thumbnail"
        );
    }

    #[test]
    fn a_result_is_one_compact_text_block() {
        let result = json_result(&json!({ "a": [1, 2], "b": "c" }));
        assert!(result.structured_content.is_none());
        assert_eq!(result.content.len(), 1);
        let text = result.content[0]
            .as_text()
            .map(|text| text.text.clone())
            .expect("a text block");
        assert_eq!(text, r#"{"a":[1,2],"b":"c"}"#);
    }

    #[test]
    fn a_batch_file_is_a_call_list_or_an_object_with_one() {
        let file = parse_batch_file(r#"[{"id":"entity.add.cube"}]"#).expect("a bare list");
        assert_eq!(file.calls.len(), 1);
        assert_eq!(file.label, None);

        let file = parse_batch_file(r#"{"label":"House","calls":[{"id":"a"},{"id":"b"}]}"#)
            .expect("an object");
        assert_eq!(file.calls.len(), 2);
        assert_eq!(file.label.as_deref(), Some("House"));

        assert!(parse_batch_file(r#"{"steps":[]}"#).is_err());
        assert!(parse_batch_file("not json").is_err());
    }

    #[test]
    fn a_batch_file_passes_its_options_and_the_tool_call_overrides_them() {
        let file = parse_batch_file(
            r#"{"calls":[{"id":"a"}],"stage":"Inn","after":["Walls"],"atomic":false,
                "checks":[{"count":{"eq":1}}],"colour":"red"}"#,
        )
        .expect("an object");
        let args = RunBatchFileArgs {
            path: "inn.json".to_string(),
            label: None,
            atomic: Some(true),
            dry_run: Some(true),
            stage: None,
            after: None,
            checks: None,
        };
        let request = batch_file_request(file, "inn.json".to_string(), &args);
        assert_eq!(
            request,
            json!({
                "calls": [{ "id": "a" }],
                "label": "inn.json",
                "stage": "Inn",
                "after": ["Walls"],
                "atomic": true,
                "dry_run": true,
                "checks": [{ "count": { "eq": 1 } }],
            })
        );
    }

    #[test]
    fn a_batch_summary_carries_the_stage_rollback_and_failing_checks() {
        let mut summary = json!({ "ran": 1, "spawned": 3 });
        add_batch_outcome(
            &mut summary,
            &json!({
                "rolled_back": true,
                "stage": { "name": "Inn", "replaced": 2 },
                "checks": {
                    "passed": 1,
                    "failed": 1,
                    "results": [
                        { "check": "count", "pass": true },
                        { "check": "bounds", "pass": false, "reason": "no bounds" },
                    ],
                },
            }),
        );
        assert_eq!(summary["rolled_back"], json!(true));
        assert_eq!(summary["spawned"], json!(0));
        assert_eq!(summary["would_spawn"], json!(3));
        assert_eq!(summary["stage"]["name"], json!("Inn"));
        assert_eq!(summary["checks"]["failed"], json!(1));
        assert_eq!(
            summary["checks"]["failing"],
            json!([{ "check": "bounds", "pass": false, "reason": "no bounds", "index": 1 }])
        );
    }

    #[test]
    fn a_batch_file_must_be_inside_the_project() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("scripts")).expect("dirs");
        std::fs::write(project.join("scripts/house.json"), "[]").expect("a file");
        std::fs::write(dir.path().join("outside.json"), "[]").expect("a file");

        assert!(file_in_project(&project, "scripts/house.json").is_ok());
        assert!(file_in_project(&project, "../outside.json").is_err());
        assert!(file_in_project(&project, "missing.json").is_err());
    }

    #[test]
    fn a_batch_summary_counts_without_repeating_the_outcomes() {
        let calls = vec![
            json!({ "result": "finished", "entities": [1, 2] }),
            json!({ "result": "finished", "warnings": ["no such material"] }),
            json!({ "result": "finished", "entities": [3] }),
        ];
        assert_eq!(
            batch_summary(3, &calls, None),
            json!({
                "ran": 3,
                "of": 3,
                "spawned": 3,
                "warnings": ["call 1: no such material"],
            })
        );

        let failed = batch_summary(10, &calls[..1], Some((&json!(1), "call 1 failed")));
        assert_eq!(failed["ran"], json!(1));
        assert_eq!(failed["of"], json!(10));
        assert_eq!(failed["failed_at"], json!(1));
        assert_eq!(failed["error"], json!("call 1 failed"));
        assert!(failed.get("warnings").is_none());
    }

    #[test]
    fn a_batch_that_stopped_on_a_call_that_did_not_finish_says_so() {
        let calls = vec![
            json!({ "result": "finished", "entities": [1] }),
            json!({
                "result": "cancelled",
                "outcome": "invalid",
                "entities": [],
                "warnings": ["segments must be at least 3"],
            }),
        ];
        let summary = batch_summary(5, &calls, None);
        assert_eq!(summary["ran"], json!(2));
        assert_eq!(summary["failed_at"], json!(1));
        assert_eq!(summary["stopped"]["result"], json!("cancelled"));
        assert_eq!(summary["stopped"]["outcome"], json!("invalid"));
        assert_eq!(
            summary["warnings"],
            json!(["call 1: segments must be at least 3"])
        );
    }

    #[test]
    fn the_instructions_steer_away_from_the_whole_catalogue() {
        let info = JackdawMcp::new(".").get_info();
        let text = info.instructions.expect("instructions");
        assert!(text.contains("describe_operator"), "{text}");
        assert!(text.contains("never load the `operators` resource whole"));
        assert!(text.contains("run_batch_file"));
        assert!(!text.contains("start with `list_operators`"));
    }

    #[test]
    fn the_instructions_prefer_the_summary_and_checks_for_verifying() {
        let text = JackdawMcp::new(".")
            .get_info()
            .instructions
            .expect("instructions");
        let summary = text.find("`scene_summary`").expect("names scene_summary");
        let check = text.find("`check`").expect("names check");
        let tree = text.find("`scene_tree`").expect("still names scene_tree");
        assert!(summary < tree && check < tree, "{text}");
        assert!(text.contains("since_revision"), "{text}");
    }
}
