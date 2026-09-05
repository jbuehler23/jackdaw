//! The editor's own remote-control surface.
//!
//! `jackdaw_remote` puts a BRP server in the *game* so the editor can
//! inspect a running build. This is the mirror image: a BRP server in the
//! *editor*, so something outside it -- `jd mcp`, a script, a test -- can
//! drive authoring the way a person does.
//!
//! Everything the editor does is an operator, so the surface is small on
//! purpose. `jackdaw/operators` says what can be called and
//! `jackdaw/call_operator` calls it; the rest is what a caller needs in
//! order to decide what to call next (the scene tree, one node's BSN, the
//! whole document, a screenshot) and to know when the editor has caught
//! up. Every edit to the open document is undoable: the operators through
//! their own history, `jackdaw/apply_bsn` through the command it pushes.
//! Undo does not reach the disk, and an operator that writes one -- a
//! save, an export, a navmesh bake, a project build -- is as reachable
//! here as it is from the menus.
//!
//! The server binds loopback only, on a port distinct from the game's
//! 15702, and publishes where it is listening in
//! `<project>/.jackdaw/editor.json` so a client does not have to be told.

use std::path::{Path, PathBuf};

use bevy::asset::UntypedAssetId;
use bevy::diagnostic::FrameCount;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, RemotePlugin, error_codes, http::RemoteHttpPlugin};
use bevy::tasks::{IoTaskPool, Task, futures_lite::future};
use jackdaw_api_internal::lifecycle::{ActiveModalOperator, OperatorEntity};
use jackdaw_api_internal::operator::{
    CallOperatorError, CallOperatorSettings, ExecutionContext, OperatorParameters, OperatorReports,
    OperatorResult, OperatorWarnings, OperatorWorldExt, ParamSpec, with_history_span,
};
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_env::editor_endpoint::{
    EditorEndpoint, current_process_name, remove_endpoint, write_endpoint,
};
use jackdaw_scene_types::PropertyValue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::boot_ops::{BootOp, resolve_entity_params};
use crate::project::ProjectRoot;
use crate::scenes::Scenes;
use crate::selection::Selection;

/// The editor's BRP port. One past the game's 15702, so an editor and the
/// game it is running never contend for the same socket.
pub const DEFAULT_PORT: u16 = 15703;

/// Overrides [`DEFAULT_PORT`], for a second editor on one machine.
pub const ENV_PORT: &str = "JACKDAW_REMOTE_PORT";

/// Project settings under the `remote` key of `.jackdaw/settings.json`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RemoteSettings {
    /// Whether the editor listens for remote control at all.
    pub enabled: bool,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// The port this process should listen on.
pub fn configured_port() -> u16 {
    std::env::var(ENV_PORT)
        .ok()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Whether remote control is on for the project rooted at `root`.
///
/// The default is on. A project turns it off with `{"remote": {"enabled":
/// false}}` in `.jackdaw/settings.json`, and then this editor answers no
/// method and publishes no endpoint while that project is open.
pub fn remote_enabled_for(root: &Path) -> bool {
    crate::project_settings::load_section::<RemoteSettings>(
        root,
        crate::project_settings::Section::Key("remote"),
    )
    .enabled
}

/// Whether the project this process is about to open wants a server.
///
/// Asked once more at build time than it strictly needs to be, because a
/// listening socket is decided when the plugin is: the project the process
/// will open is the one `jd open` named, else the last one used. Opening
/// a *different* project in-session is honoured by [`RemoteEnabled`],
/// which re-reads the setting from whatever is actually open.
fn remote_enabled_at_startup() -> bool {
    let root = crate::project::requested_project().or_else(crate::project::read_last_project);
    root.as_deref().is_none_or(remote_enabled_for)
}

/// Whether the project currently open wants remote control.
///
/// The socket is bound for the life of the process, so this is what
/// actually gates the methods: opening a project that says no turns the
/// surface off without tearing a listener down mid-frame.
#[derive(Resource, Debug)]
struct RemoteEnabled(bool);

/// Re-read `remote.enabled` whenever the open project changes.
///
/// Public so a test can put the editor in the state the plugin's schedule
/// would, without binding a socket to get there.
pub fn track_remote_enabled(world: &mut World) {
    let open = world
        .get_resource::<ProjectRoot>()
        .map(|project| project.root.clone());
    let wanted = open.as_deref().is_none_or(remote_enabled_for);
    if world.get_resource::<RemoteEnabled>().map(|e| e.0) != Some(wanted) {
        world.insert_resource(RemoteEnabled(wanted));
    }
}

/// The refusal every method gives while the open project has turned the
/// surface off.
fn check_enabled(world: &World) -> Result<(), BrpError> {
    if world.get_resource::<RemoteEnabled>().is_none_or(|e| e.0) {
        return Ok(());
    }
    Err(BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: "remote control is off for the open project (remote.enabled = false)".to_string(),
        data: None,
    })
}

/// Serves the editor's remote-control methods over BRP on loopback.
pub struct JackdawEditorRemotePlugin {
    /// Port to bind. Defaults to [`configured_port`].
    pub port: u16,
}

impl Default for JackdawEditorRemotePlugin {
    fn default() -> Self {
        Self {
            port: configured_port(),
        }
    }
}

impl Plugin for JackdawEditorRemotePlugin {
    fn build(&self, app: &mut App) {
        if !remote_enabled_at_startup() {
            info!("editor remote control is off for this project (remote.enabled = false)");
            return;
        }
        // `RemoteHttpPlugin` binds inside a task and reports a failure only
        // to the log, so without this a second editor would publish an
        // endpoint naming a port the first one holds, and clients would
        // drive the wrong editor. Probing here costs one socket and makes
        // the failure something the process can act on.
        if let Err(err) = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, self.port)) {
            warn!(
                "editor remote control is off: port {} is not available ({err}). Set {ENV_PORT} \
                 to give this editor its own port.",
                self.port
            );
            return;
        }

        app.init_resource::<PendingWaits>()
            .add_plugins(
                RemotePlugin::default()
                    .with_method_main("jackdaw/status", status_handler)
                    .with_method_main("jackdaw/operators", operators_handler)
                    .with_method_main("jackdaw/call_operator", call_operator_handler)
                    .with_method_main("jackdaw/batch", batch_handler)
                    .with_method_main("jackdaw/cancel", cancel_handler)
                    .with_method_main("jackdaw/scene_tree", scene_tree_handler)
                    .with_method_main("jackdaw/entity", entity_handler)
                    .with_method_main("jackdaw/apply_bsn", apply_bsn_handler)
                    .with_method_main("jackdaw/scene_bsn", scene_bsn_handler)
                    .with_watching_method_main("jackdaw/assets", assets_handler)
                    .with_watching_method_main("jackdaw/screenshot", screenshot_handler)
                    .with_watching_method_main("jackdaw/wait", wait_handler),
            )
            // No CORS headers: the surface has no browser client, and
            // allowing an origin would hand any page served on loopback
            // write access to the open scene.
            .add_plugins(
                RemoteHttpPlugin::default()
                    .with_address(std::net::Ipv4Addr::LOCALHOST)
                    .with_port(self.port),
            )
            .add_systems(Update, (track_remote_enabled, publish_endpoint).chain())
            .add_systems(Last, retract_endpoint);
    }
}

// --- The endpoint file ---

/// The project whose `editor.json` this process wrote, so it can take it
/// back on exit or when the editor moves to another project.
#[derive(Resource)]
pub struct PublishedEndpoint {
    project: PathBuf,
    scene: Option<PathBuf>,
}

/// Keep `<project>/.jackdaw/editor.json` in step with the open project
/// and its active scene.
///
/// Compares before it clones: this runs every frame, and the answer is
/// the same on all but the handful where a project or a tab changed.
///
/// Public alongside [`retract_endpoint`] so a test can drive the pair
/// without binding a socket to reach them.
pub fn publish_endpoint(world: &mut World) {
    let serving = world.get_resource::<RemoteEnabled>().is_none_or(|e| e.0);
    let open = world
        .get_resource::<ProjectRoot>()
        .filter(|_| serving)
        .map(|project| project.root.as_path());
    let scene = active_scene_path(world);
    let published = world.get_resource::<PublishedEndpoint>();

    let unchanged = match (open, published) {
        (Some(project), Some(published)) => {
            published.project == project && published.scene.as_deref() == scene
        }
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return;
    }

    let open = open.map(Path::to_path_buf);
    let scene = scene.map(Path::to_path_buf);
    let was = world
        .get_resource::<PublishedEndpoint>()
        .map(|published| published.project.clone());
    match open {
        Some(project) => {
            if let Some(was) = was.filter(|was| *was != project) {
                remove_endpoint(&was);
            }
            let endpoint = EditorEndpoint {
                pid: std::process::id(),
                process: current_process_name(),
                port: configured_port(),
                project: project.clone(),
                scene: scene.clone(),
                started_at: crate::timestamps::utc_rfc3339_now(),
            };
            if let Err(err) = write_endpoint(&project, &endpoint) {
                warn!("could not publish the editor endpoint: {err}");
            }
            world.insert_resource(PublishedEndpoint { project, scene });
        }
        None => {
            if let Some(was) = was {
                remove_endpoint(&was);
            }
            world.remove_resource::<PublishedEndpoint>();
        }
    }
}

/// Take the endpoint file back as the editor exits, so the next client
/// does not try to reach a process that has gone.
pub fn retract_endpoint(
    mut exits: MessageReader<AppExit>,
    published: Option<Res<PublishedEndpoint>>,
    mut commands: Commands,
) {
    if exits.read().next().is_none() {
        return;
    }
    let Some(published) = published else { return };
    remove_endpoint(&published.project);
    commands.remove_resource::<PublishedEndpoint>();
}

/// The file the active tab holds, when it has one. Borrowed, because
/// [`publish_endpoint`] asks every frame and only rarely acts.
fn active_scene_path(world: &World) -> Option<&Path> {
    let scenes = world.get_resource::<Scenes>()?;
    scenes.tabs.get(scenes.active)?.path.as_deref()
}

// --- Shared helpers ---

fn invalid_params(message: impl Into<String>) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

fn internal_error(message: impl Into<String>) -> BrpError {
    BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: message.into(),
        data: None,
    }
}

/// What is registered under `id`: its declared parameter schemas, and
/// whether anything answers to it at all.
///
/// `scene.new` and `scene.open` are each declared twice, and only one of
/// the two is reachable by id, so every registration is read rather than
/// betting on which.
///
/// Run as a cached system: a batch asks this once per call, and building
/// a fresh `QueryState` each time is an archetype scan per element.
fn lookup_operator(
    In(id): In<String>,
    ops: Query<&OperatorEntity>,
) -> (bool, Vec<&'static ParamSpec>) {
    let mut found = false;
    let mut specs = Vec::new();
    for op in &ops {
        if op.id() != id {
            continue;
        }
        found = true;
        specs.extend(op.parameters().iter());
    }
    (found, specs)
}

/// Every operator the surface will offer, deduplicated by id.
fn offered_operators(ops: Query<&OperatorEntity>) -> Vec<OperatorEntity> {
    let mut listed: Vec<OperatorEntity> = ops
        .iter()
        .filter(|op| op.remote_hidden().is_none())
        .cloned()
        .collect();
    listed.sort_by_key(OperatorEntity::id);
    listed.dedup_by_key(|op| op.id());
    listed
}

/// The id of the modal operator holding the editor, if one is.
fn active_modal(ops: Query<&OperatorEntity, With<ActiveModalOperator>>) -> Option<&'static str> {
    ops.iter().next().map(OperatorEntity::id)
}

/// Resolve a caller-supplied path inside the open project.
///
/// The confinement is [`crate::project::path_within`]; what this adds is
/// the project. With none open there is nothing to be inside of, and the
/// call is refused rather than aimed at the working directory.
fn project_path(world: &World, raw: &str) -> Result<PathBuf, BrpError> {
    let Some(project) = world.get_resource::<ProjectRoot>() else {
        return Err(invalid_params(
            "no project is open, so there is nowhere to write",
        ));
    };
    crate::project::path_within(&project.root, Path::new(raw)).map_err(invalid_params)
}

/// Type one JSON value as the parameter `spec` declares it.
///
/// A caller writing JSON has three scalar types and the editor has eight,
/// so the declared type is what decides: `radius: "5"` is a float because
/// the operator says `radius` is a float, and `name: 7` is the string
/// `"7"` because `name` is a string. Only an undeclared parameter falls
/// back to guessing from the spelling, which is what a text clause does
/// (see [`crate::boot_ops::parse_value`]).
///
/// An `Entity` is a name or a raw entity id here; filling it in is
/// [`resolve_entity_params`]'s job, and it takes both.
pub fn property_from_json(spec: Option<&ParamSpec>, value: &Value) -> Option<PropertyValue> {
    let Some(spec) = spec else {
        return untyped_property(value);
    };
    match spec.ty {
        "Bool" => match value {
            Value::Bool(flag) => Some(PropertyValue::Bool(*flag)),
            Value::String(text) => text.parse().ok().map(PropertyValue::Bool),
            Value::Number(number) => number.as_i64().map(|n| PropertyValue::Bool(n != 0)),
            _ => None,
        },
        "Int" => match value {
            Value::Number(number) => number.as_i64().map(PropertyValue::Int),
            Value::String(text) => text.trim().parse().ok().map(PropertyValue::Int),
            Value::Bool(flag) => Some(PropertyValue::Int(i64::from(*flag))),
            _ => None,
        },
        "Float" => match value {
            Value::Number(number) => number.as_f64().map(PropertyValue::Float),
            Value::String(text) => text.trim().parse().ok().map(PropertyValue::Float),
            _ => None,
        },
        "String" => Some(PropertyValue::String(
            match value {
                Value::String(text) => text.clone(),
                Value::Null => return None,
                other => other.to_string(),
            }
            .into(),
        )),
        "Vec2" => vec_of(value, 2).map(|v| PropertyValue::Vec2(Vec2::new(v[0], v[1]))),
        "Vec3" => vec_of(value, 3).map(|v| PropertyValue::Vec3(Vec3::new(v[0], v[1], v[2]))),
        "Color" => vec_of(value, 4)
            .map(|v| PropertyValue::Color(Color::srgba(v[0], v[1], v[2], v[3])))
            .or_else(|| {
                vec_of(value, 3).map(|v| PropertyValue::Color(Color::srgb(v[0], v[1], v[2])))
            }),
        // Everything else, `Entity` included, is read by its spelling.
        // An entity is resolved after typing, from a name or from the
        // selection (`boot_ops::resolve_entity_params`).
        _ => untyped_property(value),
    }
}

/// A value with no declared type, read the way a text clause reads one.
fn untyped_property(value: &Value) -> Option<PropertyValue> {
    match value {
        Value::Bool(flag) => Some(PropertyValue::Bool(*flag)),
        Value::Number(number) => number
            .as_i64()
            .map(PropertyValue::Int)
            .or_else(|| number.as_f64().map(PropertyValue::Float)),
        Value::String(text) => Some(crate::boot_ops::parse_value(text)),
        _ => None,
    }
}

/// `len` floats out of a JSON array, or out of a comma-separated string.
fn vec_of(value: &Value, len: usize) -> Option<Vec<f32>> {
    let parts: Vec<f32> = match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_f64().map(|f| f as f32))
            .collect(),
        Value::String(text) => text
            .split(',')
            .filter_map(|part| part.trim().parse::<f32>().ok())
            .collect(),
        _ => return None,
    };
    (parts.len() == len).then_some(parts)
}

/// A `PropertyValue` as JSON, for reporting a parameter's default back.
fn property_to_json(value: &PropertyValue) -> Value {
    match value {
        PropertyValue::Bool(flag) => json!(flag),
        PropertyValue::Int(int) => json!(int),
        PropertyValue::Float(float) => json!(float),
        PropertyValue::String(text) => json!(text),
        PropertyValue::Vec2(v) => json!([v.x, v.y]),
        PropertyValue::Vec3(v) => json!([v.x, v.y, v.z]),
        PropertyValue::Color(color) => {
            let c = color.to_srgba();
            json!([c.red, c.green, c.blue, c.alpha])
        }
        PropertyValue::Entity(entity) => json!(entity.to_bits()),
    }
}

/// The entity a `{"entity": <id>}` or `{"name": "<Name>"}` field means.
fn entity_from_params(world: &mut World, params: &Value) -> Result<Entity, BrpError> {
    if let Some(bits) = params.get("entity").and_then(Value::as_u64) {
        return Entity::try_from_bits(bits)
            .filter(|entity| world.get_entity(*entity).is_ok())
            .ok_or_else(|| invalid_params(format!("no entity with id {bits}")));
    }
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(invalid_params(
            "expected an \"entity\" id or a \"name\"".to_string(),
        ));
    };
    let mut query = world.query_filtered::<(Entity, &Name), Without<crate::EditorEntity>>();
    let named: Vec<(Entity, &Name)> = query.iter(world).collect();
    crate::boot_ops::unique_named_entity(named.into_iter(), name)
        .ok_or_else(|| invalid_params(format!("`{name}` names no entity in this scene, or two")))
}

// --- Methods ---

/// What the editor has open, and what is selected.
pub fn status_handler(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let modal = world.run_system_cached(active_modal).unwrap_or(None);
    let dialog = pending_dialog(world);
    let project = world
        .get_resource::<ProjectRoot>()
        .map(|project| project.root.clone());
    let (scene, dirty) = world
        .get_resource::<Scenes>()
        .and_then(|scenes| scenes.tabs.get(scenes.active))
        .map_or((None, false), |tab| (tab.path.clone(), tab.dirty));
    let selection: Vec<Entity> = world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();
    let selection: Vec<Value> = selection
        .into_iter()
        .map(|entity| {
            json!({
                "entity": entity.to_bits(),
                "name": world.get::<Name>(entity).map(|name| name.as_str().to_string()),
            })
        })
        .collect();

    Ok(json!({
        "pid": std::process::id(),
        "port": configured_port(),
        "project": project,
        "scene": scene,
        "dirty": dirty,
        "selection": selection,
        // Non-null means a modal operator is holding the editor and every
        // other modal call will be refused until `jackdaw/cancel`.
        "modal": modal,
        // Non-null means a dialog is up and nothing else will happen
        // until `dialog.answer` presses one of its buttons.
        "dialog": dialog,
        // `building`, `running` or `stopped`: what play-in-editor is
        // doing, which `jackdaw/wait` can be held on.
        "pie": crate::pie::play_status(world),
    }))
}

/// The dialog waiting for an answer, and what it will take.
fn pending_dialog(world: &mut World) -> Option<Value> {
    let mut dialogs = world.query_filtered::<
        &jackdaw_feathers::dialog::DialogChoices,
        With<jackdaw_feathers::dialog::EditorDialog>,
    >();
    let choices = dialogs.iter(world).next()?;
    Some(json!({
        "title": choices.title,
        "description": choices.description,
        "choices": choices.labels(),
    }))
}

/// Every operator a caller may call, with its parameter schema.
pub fn operators_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let prefix = params
        .as_ref()
        .and_then(|params| params.get("prefix"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let listed = world
        .run_system_cached(offered_operators)
        .map_err(|err| internal_error(err.to_string()))?;

    let operators: Vec<Value> = listed
        .into_iter()
        .filter(|op| op.id().starts_with(&prefix))
        .map(|op| {
            let available = world.operator(op.id()).is_available().unwrap_or(false);
            let params: Vec<Value> = op
                .parameters()
                .iter()
                .map(|spec| {
                    json!({
                        "name": spec.name,
                        "type": spec.ty,
                        "default": spec.default.as_ref().map(property_to_json),
                        "doc": spec.doc,
                    })
                })
                .collect();
            json!({
                "id": op.id(),
                "label": op.label(),
                "description": op.description(),
                "allows_undo": op.allows_undo(),
                "is_modal": op.is_modal(),
                "available": available,
                "params": params,
            })
        })
        .collect();

    Ok(json!({ "operators": operators }))
}

/// One operator call, typed from JSON and dispatched on the main world.
pub fn call_operator_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected {\"id\": \"<operator id>\"}"))?;
    let history = params
        .get("history")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let call = call_from_json(world, &params)?;
    let outcome = dispatch(world, call, history)?;
    Ok(json!(outcome))
}

/// A batch of calls as one undo entry.
///
/// A caller building a scene issues a run of operators that a person
/// would think of as one action. Left alone each would land on the undo
/// stack separately, so taking the action back would be a run of Ctrl-Z
/// of a length only the caller knew. Inside one history span it is one
/// entry, the way a nested operator call already is.
///
/// A call that stops the batch leaves the earlier ones done: they are in
/// the world and in that one undo entry, and the error says so, because
/// a caller that read "call 3 failed" and assumed nothing happened would
/// retry the whole batch and build everything twice.
pub fn batch_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected {\"calls\": [...]}"))?;
    let Some(calls) = params.get("calls").and_then(Value::as_array) else {
        return Err(invalid_params("expected {\"calls\": [...]}"));
    };
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("Batch")
        .to_string();

    let mut prepared = Vec::with_capacity(calls.len());
    for (index, call) in calls.iter().enumerate() {
        prepared.push(
            call_from_json(world, call)
                .map_err(|err| invalid_params(format!("call {index}: {}", err.message)))?,
        );
    }

    let results = with_history_span(world, label, move |world| {
        let mut results = Vec::with_capacity(prepared.len());
        for (index, call) in prepared.into_iter().enumerate() {
            match dispatch(world, call, true) {
                Ok(outcome) => {
                    let stop = outcome.result != "finished";
                    let entered_modal = outcome.result == "running";
                    results.push((index, Ok(outcome)));
                    if entered_modal {
                        // A modal operator waits for a pointer that is
                        // never coming, and while it holds the editor
                        // every later modal call is refused. Nobody else
                        // is going to end it.
                        if let Err(err) = world.cancel_active_modal() {
                            warn!("jackdaw/batch: could not cancel the modal call: {err}");
                        }
                    }
                    if stop {
                        break;
                    }
                }
                Err(err) => {
                    results.push((index, Err(err)));
                    break;
                }
            }
        }
        results
    });

    let mut done = Vec::new();
    for (index, result) in results {
        match result {
            Ok(outcome) => done.push(json!(outcome)),
            Err(err) => {
                return Err(BrpError {
                    code: err.code,
                    message: format!(
                        "call {index} ({}) failed: {}. The {} call(s) before it are done and are \
                         committed as one undo entry; undo once to take them back rather than \
                         retrying the whole batch.",
                        done.len(),
                        err.message,
                        done.len()
                    ),
                    data: Some(json!({ "failed_at": index, "calls": done })),
                });
            }
        }
    }
    Ok(json!({ "calls": done }))
}

/// End the modal operator holding the editor, if one is.
///
/// A modal call over the remote enters a gesture nothing is going to
/// finish: there is no pointer to release. Until it ends, every other
/// modal call is refused with `ModalAlreadyActive`. This is the way out,
/// and what `jackdaw/status` reports so a caller knows to use it.
pub fn cancel_handler(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let active = world
        .run_system_cached(active_modal)
        .map_err(|err| internal_error(err.to_string()))?;
    let Some(id) = active else {
        return Ok(json!({ "cancelled": Value::Null }));
    };
    world
        .cancel_active_modal()
        .map_err(|err| internal_error(err.to_string()))?;
    Ok(json!({ "cancelled": id }))
}

/// One prepared call: an id, its typed parameters, and how each `Entity`
/// parameter was filled in.
struct PreparedCall {
    op: BootOp,
    unresolved: Vec<String>,
}

/// What a call did.
#[derive(Serialize)]
struct CallOutcome {
    id: String,
    /// `finished`, `cancelled` or `running`.
    result: String,
    /// Anything the resolver could not fill in, plus whatever the
    /// operator itself reported through
    /// [`OperatorWarnings`]: a parameter value it did not
    /// recognise, a gesture it could not aim. Empty on a clean call.
    ///
    /// A refusal an operator only logs reaches a person reading the
    /// terminal and nobody else: a remote caller would see `finished`
    /// over a scene that had not changed.
    warnings: Vec<String>,
    /// Scene entities the call added, as ids the other methods take.
    ///
    /// A caller that placed a node has to name it to move, rename or
    /// parent it, and the only other way to find it is to guess from the
    /// tree which of the nodes there is new.
    entities: Vec<u64>,
    /// What the operator did, where the amount is the answer and the
    /// scene does not say it -- how many groups an operator replaced.
    /// Kept apart from `warnings` so a receipt is not read as a refusal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reports: Vec<String>,
}

/// Type a `{id, params}` object into a call the dispatcher can take.
fn call_from_json(world: &mut World, call: &Value) -> Result<PreparedCall, BrpError> {
    let id = call
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_params("expected an \"id\""))?
        .to_string();
    let (exists, specs) = world
        .run_system_cached_with(lookup_operator, id.clone())
        .map_err(|err| internal_error(err.to_string()))?;
    if !exists {
        return Err(invalid_params(format!("unknown operator: {id}")));
    }

    let mut params = Vec::new();
    if let Some(given) = call.get("params").and_then(Value::as_object) {
        for (key, value) in given {
            let spec = specs.iter().copied().find(|spec| spec.name == key);
            let Some(typed) = property_from_json(spec, value) else {
                return Err(invalid_params(format!(
                    "{id}: `{key}` = {value} is not a {}",
                    spec.map_or("value", |spec| spec.ty)
                )));
            };
            params.push((key.clone(), typed));
        }
    }

    let mut op = BootOp { id, params };
    let unresolved = resolve_entity_params(world, &mut op)
        .into_iter()
        .filter(crate::boot_ops::EntityParam::is_refusal)
        .filter_map(|outcome| outcome.line(&op.id))
        .collect();
    Ok(PreparedCall { op, unresolved })
}

/// Dispatch a prepared call and describe what happened.
fn dispatch(world: &mut World, call: PreparedCall, history: bool) -> Result<CallOutcome, BrpError> {
    let PreparedCall { op, unresolved } = call;
    let id = op.id.clone();
    // Anything the operator itself wants the caller told. Cleared first,
    // so a warning from an earlier call is not reported against this one.
    world.get_resource_or_init::<OperatorWarnings>().0.clear();
    world.get_resource_or_init::<OperatorReports>().0.clear();
    crate::commands::SpawnedEntities::watch(world);
    let params = OperatorParameters(op.params.into_iter().collect());
    let run = |world: &mut World| {
        world
            .operator(id.clone())
            .settings(CallOperatorSettings {
                creates_history_entry: history,
                execution_context: ExecutionContext::Execute,
            })
            .params(params)
            .call()
    };
    // One call is one undo entry. Some operators push their own commands
    // and also let the framework snapshot around them, which a user
    // undoing a menu click never notices and a caller counting entries
    // does. The span makes the two shapes agree.
    let result = if history {
        with_history_span(world, id.clone(), run)
    } else {
        run(world)
    }
    .map_err(|err| match err {
        CallOperatorError::UnknownId(_) => invalid_params(format!("unknown operator: {id}")),
        other => internal_error(other.to_string()),
    })?;

    let mut warnings = unresolved;
    warnings.append(&mut world.get_resource_or_init::<OperatorWarnings>().0);

    Ok(CallOutcome {
        id,
        result: match result {
            OperatorResult::Finished => "finished",
            OperatorResult::Cancelled => "cancelled",
            OperatorResult::Running => "running",
        }
        .to_string(),
        warnings,
        entities: spawned_by_the_call(world)
            .into_iter()
            .map(Entity::to_bits)
            .collect(),
        reports: std::mem::take(&mut world.get_resource_or_init::<OperatorReports>().0),
    })
}

/// The scene entities a call just added.
///
/// What the call recorded, which is what an operator knows and nothing
/// outside it can work out: an operator that writes a node into the
/// document and rebuilds the scene from it -- instancing a prefab is the
/// one that does -- mints new ids for every entity in the document, so
/// counting or diffing the roots afterwards would call every one of them
/// new. Each such path records the id its own rebuild left standing.
///
/// An id that did not survive the call is dropped rather than handed to a
/// caller that would only fail to address it.
fn spawned_by_the_call(world: &mut World) -> Vec<Entity> {
    crate::commands::SpawnedEntities::take(world)
        .into_iter()
        .filter(|entity| world.get_entity(*entity).is_ok())
        .collect()
}

/// The scene as the outliner shows it.
///
/// `root` is the node to report, as an entity id or as a name, and the
/// scene's own roots when it is absent. `depth` counts generations below
/// each reported node: `0` is the node alone, `1` adds its children, and
/// an absent `depth` reports the whole subtree.
pub fn scene_tree_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let depth = params
        .get("depth")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(u32::MAX)) as u32;

    let roots = match named_root(&params)? {
        None => {
            let mut query = world.query_filtered::<Entity, Without<ChildOf>>();
            query.iter(world).collect::<Vec<_>>()
        }
        Some(root) => vec![entity_from_params(world, &root)?],
    };

    let roots: Vec<Entity> = roots
        .into_iter()
        .filter(|entity| is_scene_node(world, *entity))
        .collect();
    let tree: Vec<Value> = roots
        .into_iter()
        .map(|entity| node_json(world, entity, depth))
        .collect();
    Ok(json!({ "tree": tree }))
}

/// The node a `root` field asks for, as [`entity_from_params`] takes it.
///
/// `root` holds an entity id or a name, the way `parent` does on
/// `jackdaw/apply_bsn`; `entity` and `name` are read as well, so the
/// spelling the other methods take reaches this one too.
fn named_root(params: &Value) -> Result<Option<Value>, BrpError> {
    match params.get("root") {
        Some(Value::Number(bits)) => Ok(Some(json!({ "entity": bits }))),
        Some(Value::String(name)) => Ok(Some(json!({ "name": name }))),
        Some(Value::Null) | None => Ok(params
            .get("entity")
            .or_else(|| params.get("name"))
            .filter(|given| !given.is_null())
            .map(|_| params.clone())),
        Some(other) => Err(invalid_params(format!(
            "`root` is an entity id or a name, not {other}"
        ))),
    }
}

/// Whether the outliner would draw a row for `entity`.
///
/// The same questions `crate::hierarchy::queue_root_row_spawn` asks:
/// editor furniture never shows, hidden nodes never show, and a row
/// exists only for something the scene is made of -- a `Transform` node
/// or a UI scene root. Without that last one the tree a caller reads
/// would be full of the editor's own entities, which are parentless,
/// unnamed and nothing to author.
fn is_scene_node(world: &World, entity: Entity) -> bool {
    if world.get::<crate::EditorEntity>(entity).is_some()
        || world
            .get::<jackdaw_scene_types::EditorHidden>(entity)
            .is_some()
    {
        return false;
    }
    world.get::<Transform>(entity).is_some()
        || world
            .get::<jackdaw_scene_types::UiSceneRoot>(entity)
            .is_some()
}

fn node_json(world: &mut World, entity: Entity, depth: u32) -> Value {
    let name = world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string());
    let component_ids: Vec<bevy::ecs::component::ComponentId> = world
        .get_entity(entity)
        .map(|entity_ref| entity_ref.archetype().components().to_vec())
        .unwrap_or_default();
    let mut components: Vec<String> = component_ids
        .into_iter()
        .filter_map(|id| world.components().get_info(id))
        .map(|info| info.name().to_string())
        .collect();
    components.sort();
    let children: Vec<Entity> = if depth == 0 {
        Vec::new()
    } else {
        world
            .get::<Children>(entity)
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    };
    let children: Vec<Entity> = children
        .into_iter()
        .filter(|child| is_scene_node(world, *child))
        .collect();
    let children: Vec<Value> = children
        .into_iter()
        .map(|child| node_json(world, child, depth.saturating_sub(1)))
        .collect();

    json!({
        "entity": entity.to_bits(),
        "name": name,
        "components": components,
        "children": children,
    })
}

/// One node as BSN text.
pub fn entity_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected an \"entity\" or a \"name\""))?;
    let entity = entity_from_params(world, &params)?;
    let bsn = jackdaw_remote::bsn_methods::entity_bsn(world, entity).map_err(invalid_params)?;
    Ok(json!({ "entity": entity.to_bits(), "bsn": bsn }))
}

/// Spawn BSN text into the open scene, optionally under a chosen node.
pub fn apply_bsn_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected {\"source\": \"<bsn text>\"}"))?;
    let parent = match params.get("parent") {
        None | Some(Value::Null) => None,
        Some(Value::Number(bits)) => Some(entity_from_params(world, &json!({ "entity": bits }))?),
        Some(Value::String(name)) => Some(entity_from_params(world, &json!({ "name": name }))?),
        Some(other) => {
            return Err(invalid_params(format!(
                "`parent` is an entity id or a name, not {other}"
            )));
        }
    };
    if params.get("source").and_then(Value::as_str).is_none() {
        return Err(invalid_params("expected {\"source\": \"<bsn text>\"}"));
    }

    let mut command = ApplyBsn {
        source: params.clone(),
        parent,
        spawned: Vec::new(),
        error: None,
    };
    command.execute(world);
    if let Some(err) = command.error.take() {
        return Err(err);
    }
    let entities: Vec<u64> = command.spawned.iter().map(|e| e.to_bits()).collect();
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));

    Ok(json!({ "entities": entities }))
}

/// One `jackdaw/apply_bsn` call, as an undo entry.
///
/// Applying BSN is a write like any other, and the surface promises every
/// write is undoable. Without a command the nodes would be in the scene
/// with nothing on the stack to take them back, and the user's next
/// Ctrl-Z would revert whatever they did before the call.
struct ApplyBsn {
    /// The whole call, replayed on redo.
    source: Value,
    parent: Option<Entity>,
    spawned: Vec<Entity>,
    /// A spawn failure, carried out to the caller rather than logged.
    error: Option<BrpError>,
}

impl EditorCommand for ApplyBsn {
    fn execute(&mut self, world: &mut World) {
        let spawned = match jackdaw_remote::bsn_methods::jackdaw_apply_bsn_handler(
            bevy::ecs::system::In(Some(self.source.clone())),
            world,
        ) {
            Ok(spawned) => spawned,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.spawned = spawned
            .get("entities")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_u64)
                    .filter_map(Entity::try_from_bits)
                    .collect()
            })
            .unwrap_or_default();

        // Registering puts the spawned nodes in the document, so they
        // save and appear in the outliner like anything else the editor
        // made.
        for entity in &self.spawned {
            if let Some(parent) = self.parent
                && world.get_entity(parent).is_ok()
            {
                world.entity_mut(*entity).insert(ChildOf(parent));
            }
            crate::scene_io::register_entity_in_ast(world, *entity);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for entity in self.spawned.drain(..) {
            if world.get_entity(entity).is_ok() {
                crate::commands::despawn_scene_entity(world, entity);
            }
        }
    }

    fn description(&self) -> &str {
        "Apply BSN"
    }
}

/// Deepest directory tree [`assets_handler`] walks.
///
/// The walk does not follow symlinks, so a loop cannot form through one;
/// this bounds an honestly deep tree instead, and keeps one listing from
/// costing a client its whole context.
const MAX_ASSET_DEPTH: usize = 12;

/// The project's asset files, as paths relative to its assets directory.
///
/// A caller places what the project already has, and nothing else in the
/// surface says what that is: the scene tree reports what is placed, not
/// what is on disk.
/// Registered as a *watching* method for the same reason the screenshot
/// is: the walk is blocking `std::fs` over a tree of unknown size, and
/// doing it inside the handler would hold the editor's frame for as long
/// as the disk took. It goes to the IO pool instead, and the handler is
/// polled each frame until the answer is there.
pub fn assets_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let key = request_key(&params);
    let frame = current_frame(world);
    expire_stale(world, frame);

    let known = world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .contains_key(&key);
    if !known {
        let Some(project) = world.get_resource::<ProjectRoot>() else {
            return Err(invalid_params("no project is open, so there are no assets"));
        };
        let root = project.assets_dir();
        let pattern = params
            .get("glob")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let task = IoTaskPool::get().spawn(async move {
            let mut found = Vec::new();
            collect_assets(&root, &root, &pattern, 0, &mut found);
            found.sort();
            found
        });
        world
            .get_resource_or_init::<PendingWaits>()
            .requests
            .insert(
                key,
                PendingRequest {
                    last_seen: frame,
                    state: PendingState::Assets { task },
                },
            );
        return Ok(None);
    }

    let mut waits = world.get_resource_or_init::<PendingWaits>();
    let Some(pending) = waits.requests.get_mut(&key) else {
        return Ok(None);
    };
    pending.last_seen = frame;
    let PendingState::Assets { task } = &mut pending.state else {
        return Err(internal_error(format!(
            "request {key} is already waiting on something else"
        )));
    };
    let Some(found) = future::block_on(future::poll_once(task)) else {
        return Ok(None);
    };
    waits.requests.remove(&key);
    Ok(Some(json!({ "assets": found })))
}

/// Walk `dir` and collect every file whose path under `root` matches
/// `pattern`.
///
/// Symlinks are listed but never followed: `is_dir` follows them, so a
/// link back up the tree recurses until the stack runs out and one
/// pointing outside the assets directory would report paths the project
/// does not own. `depth` bounds an honestly deep tree.
fn collect_assets(root: &Path, dir: &Path, pattern: &str, depth: usize, found: &mut Vec<String>) {
    if depth > MAX_ASSET_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            collect_assets(root, &path, pattern, depth + 1, found);
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().to_string();
        if matches_pattern(&relative, pattern) {
            found.push(relative);
        }
    }
}

/// Whether `text` matches a `*`-separated pattern. An empty pattern
/// matches everything.
///
/// A pattern with no `*` is a plain substring, so `Fence` finds
/// `kit/Prop_Fence_01.gltf`: a caller who has not been told the naming
/// convention writes the word, not the shape of the filename. A `*`
/// anchors what sits beside it, so `kit/*` is a prefix and `*.gltf` a
/// suffix.
fn matches_pattern(text: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    if !pattern.contains('*') {
        return text.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = text;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(at) = rest.find(part) else {
            return false;
        };
        if index == 0 && at != 0 {
            return false;
        }
        rest = &rest[at + part.len()..];
    }
    match parts.last() {
        Some(last) if !last.is_empty() => rest.is_empty(),
        _ => true,
    }
}

/// The whole open document as BSN text.
pub fn scene_bsn_handler(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let parent = active_scene_path(world)
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .or_else(|| {
            world
                .get_resource::<ProjectRoot>()
                .map(crate::project::ProjectRoot::assets_dir)
        })
        .unwrap_or_else(|| PathBuf::from("."));
    let bsn = crate::scene_io::save::emit_bsn_scene_with_inline_assets(world, &parent);
    Ok(json!({ "bsn": bsn }))
}

// --- The two methods that wait ---

/// Frames a pending request may go unpolled before it is forgotten.
///
/// A watching request is re-run every frame for as long as its client is
/// there. When the client drops, nothing tells the handler, and the entry
/// would otherwise sit in the map holding a half-spent frame count or a
/// finished capture, which the *next* request with the same shape would
/// then inherit. Two seconds at 60fps is far longer than a gap between
/// polls of a live request.
const STALE_FRAMES: u32 = 120;

/// Queue a capture, then answer once the PNG is on disk.
///
/// Registered as a *watching* method with no `+watch` in its name, so the
/// HTTP layer treats it as an ordinary request and holds the connection
/// while the handler is polled each frame. That is what makes a
/// screenshot one call: the operator that queues the capture returns
/// long before the GPU readback lands, and a caller that got the path
/// back immediately would read a file that is not there yet.
pub fn screenshot_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let kind = params
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("viewport")
        .to_string();
    let path = match params.get("path").and_then(Value::as_str) {
        Some(path) => project_path(world, path)?,
        None => default_capture_path(world, &kind),
    };
    let key = request_key(&params);

    let frame = current_frame(world);
    expire_stale(world, frame);

    let known = world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .contains_key(&key);
    if !known {
        // A capture the previous holder of this path left behind is not
        // this request's answer, so it is dropped before the queue rather
        // than returned instantly as someone else's image.
        if let Some(mut log) = world.get_resource_mut::<crate::screenshot::CaptureLog>() {
            log.forget(&path);
        }
        let queue = path.clone();
        let outcome = match kind.as_str() {
            "window" => {
                crate::screenshot::queue_window_capture(world, queue, false);
                Ok(())
            }
            "viewport2d" => crate::screenshot::queue_2d_capture(world, queue, false),
            _ => crate::screenshot::queue_capture(world, queue, false),
        };
        if let Err(err) = outcome {
            return Err(internal_error(err.to_string()));
        }
        world
            .get_resource_or_init::<PendingWaits>()
            .requests
            .insert(
                key.clone(),
                PendingRequest {
                    last_seen: frame,
                    state: PendingState::Capture { path: path.clone() },
                },
            );
        return Ok(None);
    }

    if let Some(pending) = world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .get_mut(&key)
    {
        pending.last_seen = frame;
    }

    let landed = world
        .get_resource::<crate::screenshot::CaptureLog>()
        .and_then(|log| log.size_of(&path));
    let Some((width, height)) = landed else {
        return Ok(None);
    };
    if let Some(mut log) = world.get_resource_mut::<crate::screenshot::CaptureLog>() {
        log.forget(&path);
    }
    world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .remove(&key);
    Ok(Some(json!({
        "path": path,
        "width": width,
        "height": height,
    })))
}

/// Where an unnamed remote capture goes.
fn default_capture_path(world: &World, kind: &str) -> PathBuf {
    let stamp = crate::timestamps::utc_rfc3339_now().replace(':', "-");
    let dir = world.get_resource::<ProjectRoot>().map_or_else(
        || PathBuf::from("screenshots"),
        |p| p.root.join("screenshots"),
    );
    dir.join(format!("{kind}-{stamp}.png"))
}

/// Let frames pass, or wait for the editor to reach a state.
///
/// `until` takes `idle`, `pie_running` or `pie_stopped`; without it the
/// call waits the number of `frames` it was given.
pub fn wait_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let frame = current_frame(world);
    expire_stale(world, frame);

    // A launch is a cargo build and then a process: minutes, not frames,
    // and the caller that started it has nothing else to poll.
    if let Some(wanted) = match params.get("until").and_then(Value::as_str) {
        Some("pie_running") => Some("running"),
        Some("pie_stopped") => Some("stopped"),
        _ => None,
    } {
        return pie_wait(world, &params, wanted, frame);
    }

    if params.get("until").and_then(Value::as_str) == Some("idle") {
        let modal = world.run_system_cached(active_modal).unwrap_or(None);
        // Idle means the editor is not still doing something it started
        // on its own: a build, a navmesh bake, or the models an opened
        // scene is still pulling off disk.
        //
        // A modal operator is not counted. Nothing is going to finish it
        // -- there is no pointer -- so waiting on it would block the call
        // for its whole timeout. It is reported instead, and
        // `jackdaw/cancel` is the way out.
        if editor_is_busy(world) {
            return Ok(None);
        }
        return Ok(Some(json!({ "idle": true, "modal": modal })));
    }

    let frames = params.get("frames").and_then(Value::as_u64).unwrap_or(1) as u32;
    let key = request_key(&params);
    let mut waits = world.get_resource_or_init::<PendingWaits>();
    // The frame the wait is over, rather than a countdown: a poll is not a
    // frame, and two clients that derived the same key would otherwise each
    // decrement one countdown and both be answered in half the frames they
    // asked for.
    let pending = waits.requests.entry(key.clone()).or_insert(PendingRequest {
        last_seen: frame,
        state: PendingState::Frames {
            until: frame.saturating_add(frames),
        },
    });
    pending.last_seen = frame;
    let PendingState::Frames { until } = &pending.state else {
        return Err(internal_error(format!(
            "request {key} is already waiting on something else"
        )));
    };
    if frame < *until {
        return Ok(None);
    }
    waits.requests.remove(&key);
    Ok(Some(json!({ "frames": frames })))
}

/// Frames a `pie_*` wait holds before it gives up, unless `frames` says
/// otherwise. A game build is minutes rather than frames, so this is
/// long: ten minutes at 60fps.
const PIE_WAIT_FRAMES: u32 = 36_000;

/// Hold until play-in-editor reaches `wanted`, the build behind it fails,
/// or the wait runs out of frames.
///
/// The state at the start of the wait is what makes this answer the
/// question the caller asked. `pie.play` returns while the editor still
/// reads as stopped -- the build has not registered yet -- so a
/// `pie_stopped` that answered on the current state would answer
/// instantly with the state the caller was trying to leave. It resolves
/// only once a game has been seen building or up.
fn pie_wait(
    world: &mut World,
    params: &Value,
    wanted: &'static str,
    frame: u32,
) -> BrpResult<Option<Value>> {
    let key = request_key(params);
    let status = crate::pie::play_status(world);
    let cap = params
        .get("frames")
        .and_then(Value::as_u64)
        .map_or(PIE_WAIT_FRAMES, |frames| frames as u32);

    let mut waits = world.get_resource_or_init::<PendingWaits>();
    let pending = waits.requests.entry(key.clone()).or_insert(PendingRequest {
        last_seen: frame,
        state: PendingState::Pie(PieWait {
            wanted,
            seen_active: false,
            until: frame.saturating_add(cap),
        }),
    });
    pending.last_seen = frame;
    let PendingState::Pie(wait) = &mut pending.state else {
        return Err(internal_error(format!(
            "request {key} is already waiting on something else"
        )));
    };
    wait.seen_active |= status != "stopped";
    let reached = match wait.wanted {
        "running" => status == "running",
        _ => wait.seen_active && status == "stopped",
    };
    let out_of_frames = frame >= wait.until;

    // A build that did not compile ends every wait on it: the game the
    // caller asked for is not coming, and holding for the frame cap would
    // report that as a timeout rather than as the failure it is.
    if status == "failed" {
        waits.requests.remove(&key);
        return Err(internal_error("the game build failed"));
    }
    if reached {
        waits.requests.remove(&key);
        return Ok(Some(json!({ "pie": status })));
    }
    if out_of_frames {
        waits.requests.remove(&key);
        return Err(internal_error(format!(
            "the game is still {status} after {cap} frames"
        )));
    }
    Ok(None)
}

/// Work the editor started on its own and has not finished.
fn editor_is_busy(world: &mut World) -> bool {
    let building = world
        .get_resource::<crate::build_status::BuildStatus>()
        .is_some_and(|status| {
            matches!(
                status.state,
                crate::build_status::BuildState::Building { .. }
            )
        });
    building || crate::terrain::navmesh_bake::bake_in_flight(world) || scene_is_loading(world)
}

/// Whether any model the open scene names is still coming off disk.
///
/// Opening a scene puts its entities in the world in one frame and its
/// glTF instances in over the following hundreds: `WorldAssetRoot` is
/// derived from `GltfSource` on insert and the handle loads in the
/// background. Without this a caller that opened a scene and waited for
/// idle is told `true` immediately, over a world its models have not
/// reached yet.
fn scene_is_loading(world: &mut World) -> bool {
    let handles: Vec<UntypedAssetId> = {
        let mut roots = world.query::<&bevy::world_serialization::WorldAssetRoot>();
        roots
            .iter(world)
            .map(|root| root.0.id().untyped())
            .collect()
    };
    if handles.is_empty() {
        return false;
    }
    let Some(asset_server) = world.get_resource::<AssetServer>() else {
        return false;
    };
    any_still_loading(
        handles
            .into_iter()
            .map(|id| asset_server.get_recursive_dependency_load_state(id)),
    )
}

/// Whether any of these load states is one the editor is still waiting on.
///
/// The recursive state, not the direct one: a glTF root reports `Loaded`
/// as soon as its own document is parsed, while its meshes and images are
/// still coming in, which is exactly the frame a caller must not screenshot
/// on.
///
/// A load that failed is not waited on: the file is missing or unreadable
/// and no amount of waiting produces it, so an editor that counted it
/// would never be idle again. A handle the server has never heard of is
/// not waited on either, for the same reason.
fn any_still_loading(
    states: impl Iterator<Item = Option<bevy::asset::RecursiveDependencyLoadState>>,
) -> bool {
    use bevy::asset::RecursiveDependencyLoadState;
    states.into_iter().any(|state| {
        matches!(
            state,
            Some(RecursiveDependencyLoadState::Loading | RecursiveDependencyLoadState::NotLoaded)
        )
    })
}

fn current_frame(world: &World) -> u32 {
    world
        .get_resource::<FrameCount>()
        .map_or(0, |count| count.0)
}

/// Forget requests nothing has polled for [`STALE_FRAMES`].
fn expire_stale(world: &mut World, frame: u32) {
    let mut dropped: Vec<PathBuf> = Vec::new();
    {
        let mut waits = world.get_resource_or_init::<PendingWaits>();
        waits.requests.retain(|_, pending| {
            if frame.saturating_sub(pending.last_seen) < STALE_FRAMES {
                return true;
            }
            if let PendingState::Capture { path } = &pending.state {
                dropped.push(path.clone());
            }
            false
        });
    }
    // A capture whose client left still lands on disk; its entry in the
    // log goes with the request so the next capture to that path does not
    // answer with this one's image.
    if !dropped.is_empty()
        && let Some(mut log) = world.get_resource_mut::<crate::screenshot::CaptureLog>()
    {
        for path in dropped {
            log.forget(&path);
        }
    }
}

/// The key a watching request is tracked under.
///
/// A watching handler is re-run with the same parameters every frame, so
/// per-request state cannot live in the call. A client that passes
/// `request` gets an identity of its own; one that does not shares state
/// with any concurrent call spelled identically, which is why `jd mcp`
/// always passes one.
fn request_key(params: &Value) -> String {
    match params.get("request").and_then(Value::as_str) {
        Some(id) => format!("id:{id}"),
        None => format!("params:{params}"),
    }
}

/// What the two waiting methods have promised to answer.
#[derive(Resource, Default)]
struct PendingWaits {
    requests: HashMap<String, PendingRequest>,
}

struct PendingRequest {
    /// The frame this request was last polled on, for [`expire_stale`].
    last_seen: u32,
    state: PendingState,
}

enum PendingState {
    Capture {
        path: PathBuf,
    },
    /// The frame at which the wait is over.
    Frames {
        until: u32,
    },
    /// The asset walk running on the IO pool.
    Assets {
        task: Task<Vec<String>>,
    },
    /// A play-in-editor state being held for.
    Pie(PieWait),
}

/// A wait held on a play-in-editor state.
struct PieWait {
    /// The status the caller asked for.
    wanted: &'static str,
    /// Whether a game has been seen building or up since the wait began.
    seen_active: bool,
    /// The frame the wait gives up on.
    until: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &'static str, ty: &'static str) -> ParamSpec {
        ParamSpec {
            name,
            ty,
            default: None,
            doc: "",
        }
    }

    /// The declared type decides, not the JSON's: an operator that wants
    /// a float takes `"5"`, and one that wants a string takes `7` as the
    /// text `"7"`. Without that, half the editor's operators would be
    /// callable only by a client that already knew every signature.
    #[test]
    fn a_declared_type_beats_the_json_spelling() {
        let float = spec("radius", "Float");
        assert_eq!(
            property_from_json(Some(&float), &json!("5")),
            Some(PropertyValue::Float(5.0))
        );
        assert_eq!(
            property_from_json(Some(&float), &json!(5)),
            Some(PropertyValue::Float(5.0))
        );
        let text = spec("name", "String");
        assert_eq!(
            property_from_json(Some(&text), &json!(7)),
            Some(PropertyValue::String("7".into()))
        );
        let int = spec("seed", "Int");
        assert_eq!(
            property_from_json(Some(&int), &json!(7)),
            Some(PropertyValue::Int(7))
        );
        assert_eq!(property_from_json(Some(&int), &json!("nope")), None);
    }

    /// An undeclared parameter is read the way a `JACKDAW_RUN_OP` clause
    /// reads one, so the two text surfaces agree.
    #[test]
    fn an_undeclared_parameter_is_typed_by_its_spelling() {
        assert_eq!(
            property_from_json(None, &json!("7")),
            Some(PropertyValue::Int(7))
        );
        assert_eq!(
            property_from_json(None, &json!("true")),
            Some(PropertyValue::Bool(true))
        );
        assert_eq!(
            property_from_json(None, &json!("kit/fence.gltf")),
            Some(PropertyValue::String("kit/fence.gltf".into()))
        );
    }

    /// Vectors arrive as arrays from a JSON client and as comma lists
    /// from a shell, and both have to reach the same operator.
    #[test]
    fn a_vector_reads_as_an_array_or_as_a_comma_list() {
        let position = spec("position", "Vec3");
        assert_eq!(
            property_from_json(Some(&position), &json!([1.0, 2.0, 3.0])),
            Some(PropertyValue::Vec3(Vec3::new(1.0, 2.0, 3.0)))
        );
        assert_eq!(
            property_from_json(Some(&position), &json!("1,2,3")),
            Some(PropertyValue::Vec3(Vec3::new(1.0, 2.0, 3.0)))
        );
        assert_eq!(property_from_json(Some(&position), &json!("1,2")), None);
    }

    /// A scene is not settled until its models are in. Opening one puts
    /// its entities in the world in a frame and its glTF instances in
    /// over the next few hundred, so a caller that takes `until_idle` at
    /// its word looks at a world with nothing in it.
    #[test]
    fn a_loading_model_is_work_the_editor_is_still_doing() {
        use bevy::asset::RecursiveDependencyLoadState as State;
        assert!(any_still_loading([Some(State::Loading)].into_iter()));
        assert!(any_still_loading([Some(State::NotLoaded)].into_iter()));
        assert!(any_still_loading(
            [Some(State::Loaded), Some(State::Loading)].into_iter()
        ));
    }

    /// A load that failed is not waited on. The file is missing or
    /// unreadable and no amount of waiting produces it, so counting it
    /// would leave the editor never idle and every `until_idle` call
    /// blocking for its whole timeout.
    #[test]
    fn a_failed_or_unknown_load_is_not_waited_on() {
        use bevy::asset::RecursiveDependencyLoadState as State;
        assert!(!any_still_loading([Some(State::Loaded)].into_iter()));
        assert!(!any_still_loading([None].into_iter()));
        assert!(!any_still_loading(std::iter::empty()));
        assert!(!any_still_loading([Some(State::Loaded), None].into_iter()));
    }

    /// The asset listing is what tells a caller which kit pieces exist,
    /// so the pattern has to behave the way a caller writing `*Fence*`
    /// expects rather than as a substring search anchored nowhere.
    #[test]
    fn a_glob_matches_in_order_and_anchors_where_it_has_no_star() {
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "*Fence*"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "kit/*"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "*.gltf"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", ""));
        assert!(!matches_pattern("models/Prop_Fence_01.gltf", "kit/*"));
        assert!(!matches_pattern("kit/Prop_Fence_01.gltf", "*.bsn"));
        assert!(!matches_pattern("kit/Prop_Fence_01.gltf", "*Wagon*"));
    }

    /// A caller who has not been told the naming convention writes the
    /// word, not the shape of the filename. Anchoring a starless pattern
    /// answers "no fences here" for a kit full of them, and the caller
    /// models one from scratch.
    #[test]
    fn a_pattern_with_no_star_matches_anywhere_in_the_path() {
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "Fence"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "kit/"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", ".gltf"));
        assert!(!matches_pattern("kit/Prop_Fence_01.gltf", "Wagon"));
    }

    /// The default port is not the game's, so an editor and the game it
    /// launches never fight over the socket.
    #[test]
    fn the_editor_port_is_not_the_game_port() {
        assert_ne!(DEFAULT_PORT, jackdaw_remote::DEFAULT_PORT);
    }
}
