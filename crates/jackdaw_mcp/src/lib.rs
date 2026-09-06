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
            return Err(ErrorData::internal_error(
                format!("{method}: {error}"),
                None,
            ));
        }
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    }

    /// A BRP call whose JSON result is the tool's whole answer.
    async fn brp_text(&self, method: &str, params: Value) -> Result<CallToolResult, ErrorData> {
        let result = self.brp(method, params).await?;
        Ok(json_result(&result))
    }
}

/// A JSON value as a tool result: pretty text for the model to read, and
/// the same value as structured content for a client that parses it.
fn json_result(value: &Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(value.clone());
    result
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
}

/// Arguments for `batch`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct BatchArgs {
    /// The calls to run, each `{"id": ..., "params": {...}}`.
    pub calls: Vec<Value>,
    /// Undo label for the whole batch.
    #[serde(default)]
    pub label: Option<String>,
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
    /// Report each path as `{path, kind, clips}` instead of a bare string,
    /// where `clips` names the animations a glTF file holds.
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
            })),
        )
        .await
    }

    /// Run several operators as one undo entry.
    #[tool(
        description = "Run several operator calls as a single undo entry. Stops at the first \
                       call that does not finish and reports which one."
    )]
    pub async fn batch(
        &self,
        Parameters(args): Parameters<BatchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.brp_text(
            "jackdaw/batch",
            compact(json!({ "calls": args.calls, "label": args.label })),
        )
        .await
    }

    /// The scene as the outliner shows it.
    #[tool(description = "The open scene's entity tree: ids, names, component type paths.")]
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
        description = "Capture the viewport (or the whole window) to a PNG and return the image \
                       along with its path. Waits for the capture to reach disk, so the file is \
                       there when this answers. `look_at` aims the camera first, since a camera \
                       left where it was frames a scene edge-on."
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
                compact(json!({
                    "kind": args.kind,
                    "path": args.path,
                    "request": self.next_request_id(),
                })),
            )
            .await?;
        let path = result
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut blocks = vec![ContentBlock::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )];
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
                "image/png",
            )),
            Err(err) => blocks.push(ContentBlock::text(format!(
                "the editor reported {path} but it could not be read: {err}"
            ))),
        }
        let mut call = CallToolResult::success(blocks);
        call.structured_content = Some(result);
        Ok(call)
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
                       bake, build or modal tool running), `pie_running` or `pie_stopped`. Use \
                       after opening a scene, starting a bake or pressing play, before reading \
                       or capturing."
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
                 start with `list_operators` to see the vocabulary, then `call_operator`. Group \
                 a run of calls with `batch` so one undo takes them all back. `scene_tree` and \
                 `get_entity` read the scene, `screenshot` looks at it, and `save_scene` writes \
                 the open scene back to its file. Operators reach the disk too, through the same \
                 saves, exports and bakes the menus run."
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
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
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
}
