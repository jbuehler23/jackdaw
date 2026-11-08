//! A BRP server in the editor, so `jd mcp`, a script or a test can drive
//! authoring the way a person does.
//!
//! `jackdaw/operators` says what can be called and `jackdaw/call_operator`
//! calls it; the rest reports what a caller needs to decide what to call next.
//! The server binds loopback only and publishes its port in
//! `<project>/.jackdaw/editor.json`.

use std::path::{Path, PathBuf};

use bevy::asset::UntypedAssetId;
use bevy::diagnostic::FrameCount;
use bevy::ecs::entity::EntityHashSet;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, RemotePlugin, error_codes, http::RemoteHttpPlugin};
use bevy::tasks::{IoTaskPool, Task, futures_lite::future};
use jackdaw_api_internal::lifecycle::{ActiveModalOperator, OperatorEntity};
use jackdaw_api_internal::operator::{OperatorResult, OperatorWorldExt};
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_editor_core::operator_calls::{
    belongs_to_the_scene, depth_param, entity_from_params, filter_param, flag, has_ancestor_in,
    internal_error, invalid_params, is_at_or_under, is_scene_descendant, root_param, scene_roots,
    vec_of,
};
use jackdaw_env::editor_endpoint::{
    EditorEndpoint, current_process_name, remove_endpoint, write_endpoint,
};
use jackdaw_scene_types::PropertyValue;
use path_slash::PathExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use jackdaw_editor_core::game_query::{GameEntityRef, GameQueries, GameQuery, ask_game};
use jackdaw_editor_core::project::ProjectRoot;
use jackdaw_editor_core::scenes::Scenes;
use jackdaw_editor_core::screenshot::CaptureError;
use jackdaw_editor_core::selection::Selection;

pub use super::paced::paced_batch_handler;
pub use jackdaw_editor_core::operator_calls::property_from_json;

/// The open scene as compact rows; see [`jackdaw_editor_core::operator_calls::summary`].
pub fn scene_summary_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    jackdaw_editor_core::operator_calls::summary::scene_summary(params, world)
}

/// A brush's authored topology as compact arrays.
pub fn brush_topology_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    jackdaw_editor_core::operator_calls::topology::brush_topology(In(params), world)
}

/// Evaluate geometric assertions against the open scene.
pub fn check_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    jackdaw_editor_core::operator_calls::summary::check(params, world)
}

/// The editor's BRP port. One past the game's 15702, so an editor and the
/// game it is running never contend for the same socket.
pub const DEFAULT_PORT: u16 = 15703;

/// Overrides `DEFAULT_PORT`, for a second editor on one machine.
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

/// Whether remote control is on for the project rooted at `root`. On by default;
/// a project turns it off with `{"remote": {"enabled": false}}` in
/// `.jackdaw/settings.json`.
pub fn remote_enabled_for(root: &Path) -> bool {
    jackdaw_editor_core::project_settings::load_section::<RemoteSettings>(
        root,
        jackdaw_editor_core::project_settings::Section::Key("remote"),
    )
    .enabled
}

/// Whether the project this process is about to open wants a server. Asked at
/// build time, since a listening socket is decided when the plugin is.
fn remote_enabled_at_startup() -> bool {
    let root = jackdaw_editor_core::project::requested_project()
        .or_else(jackdaw_editor_core::project::read_last_project);
    root.as_deref().is_none_or(remote_enabled_for)
}

/// Whether the project currently open wants remote control. The socket is bound
/// for the life of the process, so this is what gates the methods.
#[derive(Resource, Debug)]
struct RemoteEnabled(bool);

/// Re-read `remote.enabled` whenever the open project changes.
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
pub(super) fn check_enabled(world: &World) -> Result<(), BrpError> {
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
    /// Port to bind. Defaults to `configured_port`.
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
        // `RemoteHttpPlugin` binds inside a task and reports failure only to the
        // log, so a second editor would otherwise publish a port the first one
        // holds. Probing here makes that failure actionable.
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
                    .with_watching_method_main("jackdaw/paced_batch", paced_batch_handler)
                    .with_method_main("jackdaw/cancel", cancel_handler)
                    .with_method_main("jackdaw/scene_tree", scene_tree_handler)
                    .with_method_main("jackdaw/scene_summary", scene_summary_handler)
                    .with_method_main("jackdaw/check", check_handler)
                    .with_method_main("jackdaw/brush_topology", brush_topology_handler)
                    .with_method_main("jackdaw/entity", entity_handler)
                    .with_method_main("jackdaw/apply_bsn", apply_bsn_handler)
                    .with_method_main("jackdaw/scene_bsn", scene_bsn_handler)
                    .with_watching_method_main("jackdaw/assets", assets_handler)
                    .with_watching_method_main("jackdaw/screenshot", screenshot_handler)
                    .with_watching_method_main("jackdaw/views", views_handler)
                    .with_watching_method_main("jackdaw/wait", wait_handler)
                    .with_watching_method_main("jackdaw/game_entities", game_entities_handler)
                    .with_watching_method_main("jackdaw/game_entity", game_entity_handler)
                    .with_watching_method_main("jackdaw/game_query", game_query_handler),
            )
            // No CORS headers: allowing an origin would hand any page served on
            // loopback write access to the open scene.
            .add_plugins(
                RemoteHttpPlugin::default()
                    .with_address(std::net::Ipv4Addr::LOCALHOST)
                    .with_port(self.port),
            )
            .add_systems(Update, (track_remote_enabled, publish_endpoint).chain())
            .add_systems(Update, super::paced::drive_paced_batch)
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

/// Keep `<project>/.jackdaw/editor.json` in step with the open project and its
/// active scene.
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
                started_at: jackdaw_editor_core::timestamps::utc_rfc3339_now(),
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
/// `publish_endpoint` asks every frame and only rarely acts.
fn active_scene_path(world: &World) -> Option<&Path> {
    let scenes = world.get_resource::<Scenes>()?;
    scenes.tabs.get(scenes.active)?.path.as_deref()
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

/// Resolve a caller-supplied path inside the open project, refusing the call
/// when no project is open.
fn project_path(world: &World, raw: &str) -> Result<PathBuf, BrpError> {
    let Some(project) = world.get_resource::<ProjectRoot>() else {
        return Err(invalid_params(
            "no project is open, so there is nowhere to write",
        ));
    };
    jackdaw_editor_core::project::path_within(&project.root, Path::new(raw)).map_err(invalid_params)
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

    let progress = world
        .get_resource::<jackdaw_editor_core::progress::EditorProgress>()
        .and_then(jackdaw_editor_core::progress::EditorProgress::current)
        .map(|task| {
            json!({
                "title": task.title,
                "stage": task.stage,
                "done": task.done,
                "total": task.total,
            })
        });

    Ok(json!({
        "pid": std::process::id(),
        "port": configured_port(),
        "progress": progress,
        "project": project,
        "scene": scene,
        "dirty": dirty,
        "selection": selection,
        // The project palette: the active one, every palette's names and the
        // material files they stand for, and why the file could not be read.
        "palette": jackdaw_editor_core::material_palette::palette_status(world),
        // Non-null means a modal operator is holding the editor and every
        // other modal call will be refused until `jackdaw/cancel`.
        "modal": modal,
        // Non-null means a dialog is up and nothing else will happen
        // until `dialog.answer` presses one of its buttons.
        "dialog": dialog,
        // Non-null means a list of choices is up, naming which list it is
        // and holding the editor until an entry is taken or `modal.cancel`
        // drops it.
        "picker": open_picker(world),
        // `building`, `running`, `paused`, `failed` or `stopped`: what
        // play-in-editor is doing, which `jackdaw/wait` can be held on.
        "pie": jackdaw_editor_core::pie::play_status(world),
    }))
}

/// The list of choices on screen: which list it is, and the entries it offers
/// where they are text a caller can name.
fn open_picker(world: &mut World) -> Option<Value> {
    let mut pickers = world.query_filtered::<Entity, With<jackdaw_feathers::picker::Picker>>();
    let picker = pickers.iter(world).next()?;
    let list = if world
        .get::<jackdaw_editor_core::new_asset::NewAssetList>(picker)
        .is_some()
    {
        Some("new asset")
    } else if world
        .get::<jackdaw_editor_core::inspector::asset_row::AssetFieldPicker>(picker)
        .is_some()
    {
        Some("asset field")
    } else {
        None
    };
    let items = world
        .get::<jackdaw_feathers::picker::PickerItems<String>>(picker)
        .map(|items| items.items().to_vec());
    Some(json!({ "list": list, "items": items }))
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

/// Every operator a caller may call, with its parameter schema. `prefix`
/// narrows the list, and `id` answers for that one operator.
pub fn operators_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let prefix = params
        .as_ref()
        .and_then(|params| params.get("prefix"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let exact = params
        .as_ref()
        .and_then(|params| params.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let listed = world
        .run_system_cached(offered_operators)
        .map_err(|err| internal_error(err.to_string()))?;

    let operators: Vec<Value> = listed
        .into_iter()
        .filter(|op| op.id().starts_with(&prefix))
        .filter(|op| exact.as_deref().is_none_or(|id| op.id() == id))
        .map(|op| {
            let available = world.operator(op.id()).is_available().unwrap_or(false);
            let params: Vec<Value> = op
                .parameters()
                .iter()
                .map(|spec| {
                    let mut param = json!({ "name": spec.name, "type": spec.ty, "doc": spec.doc });
                    if let Some(default) = &spec.default {
                        param["default"] = property_to_json(default);
                    }
                    param
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
    super::paced::refuse_while_playing(world, "jackdaw/call_operator")?;
    jackdaw_editor_core::operator_calls::call_operator(In(params), world)
}

/// Run a batch of calls as one undo entry; see
/// [`jackdaw_editor_core::operator_calls::run_batch`].
pub fn batch_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    super::paced::refuse_while_playing(world, "jackdaw/batch")?;
    jackdaw_editor_core::operator_calls::run_batch(In(params), world)
}

/// End the modal operator holding the editor, if one is. A modal call over the
/// remote has no pointer to finish it, and blocks every later modal call.
pub fn cancel_handler(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    super::paced::refuse_while_playing(world, "jackdaw/cancel")?;
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
/// The scene as the outliner shows it. `root` names the node to report and
/// `depth` counts generations below it; absent, they mean the whole scene.
/// `filter` is an entity reference: the nodes it matches (within `root`, when
/// given) are reported as the roots, the topmost one where one sits under
/// another.
pub fn scene_tree_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let depth = depth_param(&params)?;

    let asked_for = root_param(world, &params)?;
    // A named root may be a row inside a UI screen, which is not a scene root.
    let roots: Vec<Entity> = match asked_for {
        None => scene_roots(world),
        Some(root) => vec![root]
            .into_iter()
            .filter(|entity| belongs_to_the_scene(world, *entity))
            .collect(),
    };
    let roots = match filter_param(world, &params)? {
        None => roots,
        Some(matched) => {
            let kept: Vec<Entity> = matched
                .into_iter()
                .filter(|&entity| {
                    belongs_to_the_scene(world, entity)
                        && asked_for.is_none_or(|root| is_at_or_under(world, entity, root))
                })
                .collect();
            let kept_set: EntityHashSet = kept.iter().copied().collect();
            kept.into_iter()
                .filter(|&entity| !has_ancestor_in(world, entity, &kept_set))
                .collect()
        }
    };
    let tree: Vec<Value> = roots
        .into_iter()
        .map(|entity| node_json(world, entity, depth))
        .collect();
    Ok(json!({ "tree": tree }))
}
fn node_json(world: &mut World, entity: Entity, depth: u32) -> Value {
    let name = world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
        .or_else(|| jackdaw_editor_core::hierarchy::prefab_stem_label(world, entity));
    let missing_source = jackdaw_editor_core::hierarchy::missing_prefab_source(world, entity)
        .map(|source| source.display().to_string());
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
        .filter(|child| is_scene_descendant(world, *child))
        .collect();
    let children: Vec<Value> = children
        .into_iter()
        .map(|child| node_json(world, child, depth.saturating_sub(1)))
        .collect();

    json!({
        "entity": entity.to_bits(),
        "name": name,
        "missing_source": missing_source,
        "components": components,
        "children": children,
    })
}

/// One node as BSN text, with the mask palettes a terrain paints and scatters
/// by, which a caller cannot name without being told them.
pub fn entity_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected an \"entity\" or a \"name\""))?;
    let entity = entity_from_params(world, &params)?;
    let names = jackdaw_editor_core::scene_io::save::asset_reference_seed(world);
    let bsn = jackdaw_remote::bsn_methods::entity_bsn_naming(world, entity, &names)
        .map_err(invalid_params)?;
    let mut answer = json!({ "entity": entity.to_bits(), "bsn": bsn });
    if let Some(masks) = terrain_masks(world, entity) {
        answer["masks"] = masks;
    }
    Ok(answer)
}

/// A terrain's mask channels, each with the palette `terrain.channel.*` and
/// `terrain.scatter` take names from.
fn terrain_masks(world: &World, entity: Entity) -> Option<Value> {
    let terrain = world.get::<jackdaw_scene_types::Terrain>(entity)?;
    let masks: Vec<Value> = terrain
        .channels
        .iter()
        .enumerate()
        .map(|(index, channel)| {
            let palette: Vec<Value> = channel
                .palette
                .iter()
                .map(|entry| json!({ "value": entry.value, "label": entry.label }))
                .collect();
            json!({ "index": index, "name": channel.name, "palette": palette })
        })
        .collect();
    Some(Value::Array(masks))
}

/// Spawn BSN text into the open scene, optionally under a chosen node.
pub fn apply_bsn_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    super::paced::refuse_while_playing(world, "jackdaw/apply_bsn")?;
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
        self.parent = self
            .parent
            .map(|parent| jackdaw_editor_core::scene_nodes::live_entity(world, parent));
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

        // Only the roots of the applied source move under `parent`; the rest
        // already carry the `ChildOf` the source spelled, and reparenting them
        // too would flatten the subtree onto the parent.
        for entity in &self.spawned {
            if let Some(parent) = self.parent
                && world.get::<ChildOf>(*entity).is_none()
                && world.get_entity(parent).is_ok()
            {
                world.entity_mut(*entity).insert(ChildOf(parent));
            }
        }

        // Registering puts the spawned nodes in the document, so they save and
        // appear in the outliner. Parents come first in `spawned`, so a child
        // finds its parent's node already there.
        for entity in &self.spawned {
            jackdaw_editor_core::scene_io::register_entity_in_ast(world, *entity);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for entity in self.spawned.drain(..) {
            let entity = jackdaw_editor_core::scene_nodes::live_entity(world, entity);
            if world.get_entity(entity).is_ok() {
                jackdaw_editor_core::commands::despawn_scene_entity(world, entity);
            }
        }
    }

    fn description(&self) -> &str {
        "Apply BSN"
    }
}

/// Deepest directory tree the asset listing walks.
const MAX_ASSET_DEPTH: usize = 12;

/// The project's asset files, as paths relative to its assets directory.
///
/// A document held in both forms is one file to a caller, so the pair is listed
/// once under its text path; a detailed listing carries `binary`, which says
/// the document is held in the binary form.
///
/// Registered as a watching method: the walk is blocking `std::fs` over a tree
/// of unknown size, so it runs on the IO pool and the handler is polled each
/// frame until the answer is there.
pub fn assets_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let key = request_key("jackdaw/assets", &params);
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
    let found = collapse_document_twins(found);
    if !params
        .get("details")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let paths: Vec<&String> = found.iter().map(|(path, _)| path).collect();
        return Ok(Some(json!({ "assets": paths })));
    }
    if let Some(mut demand) =
        world.get_resource_mut::<jackdaw_editor_core::animation_mode::LibraryDemand>()
    {
        demand.requested = true;
    }
    world.get_resource_or_init::<jackdaw_editor_core::asset_files::AssetKindCache>();
    let detailed: Vec<Value> = world.resource_scope(
        |world, mut cache: Mut<jackdaw_editor_core::asset_files::AssetKindCache>| {
            let asset_kinds = world.get_resource::<jackdaw_api::prelude::AssetKinds>();
            let assets_dir = world
                .get_resource::<jackdaw_editor_core::project::ProjectRoot>()
                .map(jackdaw_editor_core::project::ProjectRoot::assets_dir);
            let library =
                world.get_resource::<jackdaw_editor_core::animation_mode::AnimationLibrary>();
            found
                .into_iter()
                .map(|(path, binary)| {
                    let clips: Vec<&str> = library
                        .and_then(|library| library.file(&path))
                        .map(|file| file.clips.iter().map(|clip| clip.name.as_str()).collect())
                        .unwrap_or_default();
                    let kind = asset_kinds
                        .zip(assets_dir.as_ref())
                        .and_then(|(kinds, assets)| {
                            cache
                                .check(&assets.join(&path), kinds)
                                .type_path()
                                .and_then(|type_path| kinds.by_type_path(type_path))
                                .map(|known| known.kind.clone())
                        })
                        .unwrap_or_else(|| asset_kind(&path).to_string());
                    json!({ "path": path, "kind": kind, "clips": clips, "binary": binary })
                })
                .collect()
        },
    );
    Ok(Some(json!({ "assets": detailed })))
}

/// The listing with each document named once, as the path and whether a binary
/// twin of it is on disk. A pair drops its binary file and answers to the text
/// path; a document held only as binary keeps the path it sits at.
fn collapse_document_twins(found: Vec<String>) -> Vec<(String, bool)> {
    let binary_path = |path: &str| jackdaw_bsn::is_binary_path(Path::new(path));
    let text_twin = |path: &str| {
        jackdaw_bsn::text_twin(Path::new(path))
            .to_slash_lossy()
            .into_owned()
    };
    let text: std::collections::HashSet<&str> = found
        .iter()
        .filter(|path| !binary_path(path))
        .map(String::as_str)
        .collect();
    let twinned: std::collections::HashSet<String> = found
        .iter()
        .filter(|path| binary_path(path))
        .map(|path| text_twin(path))
        .collect();
    found
        .iter()
        .filter(|path| !binary_path(path) || !text.contains(text_twin(path).as_str()))
        .map(|path| {
            let held = binary_path(path) || twinned.contains(path);
            (path.clone(), held)
        })
        .collect()
}

/// What kind of thing an asset path names, from its extension.
///
/// Coarse on purpose: a caller uses it to tell a model from a document, and
/// asks the editor about anything finer. A file belonging to a registered
/// definition type reports that type's kind instead.
fn asset_kind(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "glb" | "gltf" | "obj" | "fbx" => "model",
        "png" | "jpg" | "jpeg" | "ktx2" | "basis" | "exr" | "hdr" | "dds" => "image",
        "ogg" | "wav" | "flac" | "mp3" => "audio",
        "bsn" | "bsb" | "jsn" | "scn" | "ron" => "scene",
        "ttf" | "otf" => "font",
        "wgsl" | "wesl" | "glsl" | "spv" => "shader",
        _ => "file",
    }
}

/// Walk `dir` and collect every file whose path under `root` matches `pattern`.
/// Symlinks are listed but never followed, since a link back up the tree would
/// recurse and one pointing outside the assets directory would leak paths.
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
        let relative = relative.to_slash_lossy().into_owned();
        if matches_pattern(&relative, pattern) {
            found.push(relative);
        }
    }
}

/// Whether `text` matches a `*`-separated pattern. An empty pattern matches
/// everything, a pattern with no `*` is a plain substring, and a `*` anchors
/// what sits beside it.
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
                .map(jackdaw_editor_core::project::ProjectRoot::assets_dir)
        })
        .unwrap_or_else(|| PathBuf::from("."));
    let bsn =
        jackdaw_editor_core::scene_io::save::emit_bsn_scene_with_inline_assets(world, &parent);
    Ok(json!({ "bsn": bsn }))
}

// --- The two methods that wait ---

/// Frames a pending request may go unpolled before it is forgotten. Nothing
/// tells the handler when a client drops, so a stale entry would otherwise be
/// inherited by the next request with the same shape.
const STALE_FRAMES: u32 = 120;

/// Queue a capture, then answer once the PNG is on disk.
///
/// Registered as a watching method with no `+watch` in its name, so the HTTP
/// layer holds the connection while the handler is polled each frame: the
/// capture operator returns long before the GPU readback lands.
///
/// `look_at: {eye, target}` aims the viewport camera before the capture and
/// puts it back once the capture lands, unless `keep_camera` is true.
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
    let key = request_key("jackdaw/screenshot", &params);

    let frame = current_frame(world);
    expire_stale(world, frame);

    // A default path carries a timestamp, so it is made once, when the capture
    // is queued, and read back from the request on every later poll.
    let found = match world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .get_mut(&key)
    {
        // The HTTP layer polls once more after the answer goes out; that
        // poll's error reaches no one, and a reused id hears why.
        Some(PendingRequest {
            state: PendingState::Answered,
            ..
        }) => Some(None),
        Some(pending) => match &pending.state {
            PendingState::Capture { path, queued, .. } => {
                let found = (path.clone(), *queued);
                pending.last_seen = frame;
                Some(Some(found))
            }
            _ => None,
        },
        None => None,
    };
    let path = match found {
        Some(None) => {
            return Err(invalid_params(format!(
                "screenshot request {key} was already answered; give each screenshot its own id"
            )));
        }
        // Aimed on the last poll: the camera's new place has been propagated
        // since, so the capture sees the aimed view.
        Some(Some((path, false))) => {
            if let Err(err) = queue_shot(world, &params, &kind, &path) {
                if let Some(pending) = world
                    .get_resource_or_init::<PendingWaits>()
                    .requests
                    .remove(&key)
                    && let PendingState::Capture { aim: Some(aim), .. } = pending.state
                {
                    aim.put_back(world);
                }
                return Err(err);
            }
            if let Some(PendingRequest {
                state: PendingState::Capture { queued, .. },
                ..
            }) = world
                .get_resource_or_init::<PendingWaits>()
                .requests
                .get_mut(&key)
            {
                *queued = true;
            }
            return Ok(None);
        }
        Some(Some((path, true))) => path,
        None => {
            let path = match params.get("path").and_then(Value::as_str) {
                Some(path) => project_path(world, path)?,
                None => default_capture_path(world, &kind),
            };
            capture_region(&params)?;
            thumbnail_request(&params)?;
            let aim = match look_at_request(&params)? {
                Some((eye, target)) => {
                    if !key.starts_with("id:") {
                        return Err(invalid_params(
                            "a screenshot with `look_at` needs a `request` id, so the poll after \
                             its answer does not aim and capture again",
                        ));
                    }
                    if aimed_capture_in_flight(world) {
                        return Err(invalid_params(
                            "another screenshot with `look_at` is still capturing; wait for it",
                        ));
                    }
                    let keep_camera = flag(&params, "keep_camera")?;
                    let before = (!keep_camera)
                        .then(|| jackdaw_editor_core::view_ops::CameraPose::of_active(world))
                        .flatten();
                    aim_camera(world, eye, target)?;
                    Some(Box::new(Aim {
                        before,
                        aimed: jackdaw_editor_core::view_ops::CameraPose::of_active(world),
                    }))
                }
                None => {
                    queue_shot(world, &params, &kind, &path)?;
                    None
                }
            };
            let queued = aim.is_none();
            world
                .get_resource_or_init::<PendingWaits>()
                .requests
                .insert(
                    key,
                    PendingRequest {
                        last_seen: frame,
                        state: PendingState::Capture { path, queued, aim },
                    },
                );
            return Ok(None);
        }
    };

    let mut log = world.get_resource_or_init::<jackdaw_editor_core::screenshot::CaptureLog>();
    let Some((width, height)) = log.size_of(&path) else {
        return Ok(None);
    };
    let thumbnail = log.take_thumbnail(&path);
    log.forget(&path);
    let finished = {
        let mut waits = world.get_resource_or_init::<PendingWaits>();
        // Only a request with its own id can be told from a new one spelled
        // the same, so only that kind is remembered as answered. The frame it
        // was answered on is kept, so the memory expires however often it is
        // polled.
        if key.starts_with("id:") {
            waits.requests.get_mut(&key).map(|pending| {
                pending.last_seen = frame;
                std::mem::replace(&mut pending.state, PendingState::Answered)
            })
        } else {
            waits.requests.remove(&key).map(|pending| pending.state)
        }
    };
    if let Some(PendingState::Capture { aim: Some(aim), .. }) = finished {
        aim.put_back(world);
    }
    let mut answer = json!({
        "path": path,
        "width": width,
        "height": height,
    });
    match thumbnail {
        Some(Ok(thumb)) => {
            answer["thumb"] = json!({
                "path": thumb.path,
                "width": thumb.width,
                "height": thumb.height,
            });
        }
        // The capture itself landed; a caller can still read it at full size.
        Some(Err(err)) => answer["warnings"] = json!([err]),
        None => {}
    }
    Ok(Some(answer))
}

/// A camera a screenshot aimed: where it stood before, unless the caller
/// keeps it aimed, and where the aim put it.
struct Aim {
    before: Option<jackdaw_editor_core::view_ops::CameraPose>,
    aimed: Option<jackdaw_editor_core::view_ops::CameraPose>,
}

impl Aim {
    /// Put the camera back, unless someone has moved it off the aimed pose
    /// since.
    fn put_back(self, world: &mut World) {
        let untouched = self.aimed.is_some_and(|aimed| aimed.is_current(world));
        if let Some(before) = self.before
            && untouched
        {
            before.restore(world);
        }
    }
}

/// Whether a screenshot that aimed the camera has not answered yet.
fn aimed_capture_in_flight(world: &mut World) -> bool {
    world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .values()
        .any(|pending| matches!(pending.state, PendingState::Capture { aim: Some(_), .. }))
}

/// Queue the capture a screenshot request names, at `path`.
fn queue_shot(world: &mut World, params: &Value, kind: &str, path: &Path) -> BrpResult<()> {
    let region = capture_region(params)?;
    let thumbnail = thumbnail_request(params)?;
    // A capture a previous holder of this path left behind is not this
    // request's answer.
    {
        let mut log = world.get_resource_or_init::<jackdaw_editor_core::screenshot::CaptureLog>();
        log.forget(path);
        if let Some((max_px, format)) = thumbnail {
            log.want_thumbnail(path.to_path_buf(), max_px, format);
        }
    }
    let queue = path.to_path_buf();
    let outcome = match kind {
        "window" => {
            jackdaw_editor_core::screenshot::queue_window_capture(world, queue, false, region)
        }
        "viewport2d" => {
            jackdaw_editor_core::screenshot::queue_2d_capture(world, queue, false, region)
        }
        _ => jackdaw_editor_core::screenshot::queue_capture(world, queue, false, region),
    };
    let Err(err) = outcome else {
        return Ok(());
    };
    world
        .get_resource_or_init::<jackdaw_editor_core::screenshot::CaptureLog>()
        .forget(path);
    Err(match err {
        CaptureError::NoViewport | CaptureError::NotAnImageTarget => {
            internal_error(err.to_string())
        }
        CaptureError::WorldRegionWithoutView
        | CaptureError::WorldRegionOnUiScene
        | CaptureError::EmptyRegion => invalid_params(err.to_string()),
    })
}

/// The eye and target a screenshot's `look_at: {eye, target}` names; the
/// target defaults to the origin.
fn look_at_request(params: &Value) -> BrpResult<Option<(Vec3, Vec3)>> {
    let Some(look_at) = params.get("look_at").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let point = |key: &str, default: Option<Vec3>| -> BrpResult<Vec3> {
        match look_at.get(key).filter(|value| !value.is_null()) {
            None => default.ok_or_else(|| invalid_params("`look_at` needs an `eye` [x, y, z]")),
            Some(value) => vec_of(value, 3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .ok_or_else(|| {
                    invalid_params(format!("`look_at.{key}` = {value} is not [x, y, z]"))
                }),
        }
    };
    Ok(Some((
        point("eye", None)?,
        point("target", Some(Vec3::ZERO))?,
    )))
}

/// Aim the viewport camera with `view.look_at`.
fn aim_camera(world: &mut World, eye: Vec3, target: Vec3) -> BrpResult<()> {
    let result = world
        .operator("view.look_at")
        .param("eye_x", f64::from(eye.x))
        .param("eye_y", f64::from(eye.y))
        .param("eye_z", f64::from(eye.z))
        .param("target_x", f64::from(target.x))
        .param("target_y", f64::from(target.y))
        .param("target_z", f64::from(target.z))
        .call()
        .map_err(|err| internal_error(err.to_string()))?;
    if matches!(result, OperatorResult::Finished) {
        Ok(())
    } else {
        Err(invalid_params(
            "`look_at` could not aim the camera: no 3D viewport, or the eye is the target",
        ))
    }
}

/// The thumbnail a screenshot request asks for: `max_px` on the longer side,
/// in `format` (`png`, the default, or `jpeg`). None without `max_px`.
fn thumbnail_request(
    params: &Value,
) -> BrpResult<Option<(u32, jackdaw_editor_core::screenshot::ThumbnailFormat)>> {
    let Some(max_px) = params.get("max_px").filter(|max| !max.is_null()) else {
        return Ok(None);
    };
    let max_px = max_px
        .as_u64()
        .and_then(|max| u32::try_from(max).ok())
        .filter(|max| *max > 0)
        .ok_or_else(|| invalid_params(format!("`max_px` = {max_px} is not a positive size")))?;
    let format = match params.get("format").and_then(Value::as_str) {
        None | Some("png") => jackdaw_editor_core::screenshot::ThumbnailFormat::Png,
        Some("jpeg" | "jpg") => jackdaw_editor_core::screenshot::ThumbnailFormat::Jpeg,
        Some(other) => {
            return Err(invalid_params(format!(
                "`format` = {other}: expected `png` or `jpeg`"
            )));
        }
    };
    Ok(Some((max_px, format)))
}

/// The `region` a screenshot request names: `{x, y, w, h, space}` with the
/// space optional.
fn capture_region(
    params: &Value,
) -> BrpResult<Option<jackdaw_editor_core::screenshot::CaptureRegion>> {
    match params.get("region").filter(|region| !region.is_null()) {
        Some(region) => jackdaw_editor_core::screenshot::CaptureRegion::from_json(region)
            .map(Some)
            .map_err(invalid_params),
        None => Ok(None),
    }
}

/// Where an unnamed remote capture goes.
fn default_capture_path(world: &World, kind: &str) -> PathBuf {
    let stamp = jackdaw_editor_core::timestamps::utc_rfc3339_now().replace(':', "-");
    let dir = world.get_resource::<ProjectRoot>().map_or_else(
        || PathBuf::from("screenshots"),
        |p| p.root.join("screenshots"),
    );
    dir.join(format!("{kind}-{stamp}.png"))
}

/// Render offscreen views of `targets` and answer with one sheet once every
/// tile is back. Each view has its own camera framed on the targets' bounds;
/// the viewport camera does not move.
///
/// Watching like `jackdaw/screenshot`: the first poll validates and queues,
/// later polls answer once the readbacks have landed.
pub fn views_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let key = request_key("jackdaw/views", &params);
    let frame = current_frame(world);
    expire_stale(world, frame);

    let found = match world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .get_mut(&key)
    {
        None => None,
        Some(pending) => match &pending.state {
            PendingState::Views(job) => {
                pending.last_seen = frame;
                Some((job.request, job.tiles.len()))
            }
            _ => {
                return Err(invalid_params(format!(
                    "views request {key} was already answered; give each call its own id"
                )));
            }
        },
    };
    let Some((request, count)) = found else {
        if !key.starts_with("id:") {
            return Err(invalid_params(
                "views needs a `request` id, so the poll after its answer does not render again",
            ));
        }
        let job = plan_views(world, &params)?;
        world
            .get_resource_or_init::<PendingWaits>()
            .requests
            .insert(
                key,
                PendingRequest {
                    last_seen: frame,
                    state: PendingState::Views(Box::new(job)),
                },
            );
        return Ok(None);
    };
    let Some(images) = jackdaw_editor_core::views::take_landed(world, request, count) else {
        return Ok(None);
    };
    let finished = {
        let mut waits = world.get_resource_or_init::<PendingWaits>();
        if key.starts_with("id:") {
            waits.requests.get_mut(&key).map(|pending| {
                pending.last_seen = frame;
                std::mem::replace(&mut pending.state, PendingState::Answered)
            })
        } else {
            waits.requests.remove(&key).map(|pending| pending.state)
        }
    };
    let Some(PendingState::Views(job)) = finished else {
        return Ok(None);
    };
    let ViewsJob {
        request,
        tiles,
        layout,
        format,
        path,
        isolation,
        bounds,
    } = *job;
    let late = jackdaw_editor_core::views::was_late(world, request);
    jackdaw_editor_core::views::finish(world, request, isolation);
    let mut answer = write_views(&tiles, layout, format, &path, &images, bounds)?;
    if late {
        answer["warnings"] = json!([
            "the renderer was still compiling when the views were read back; tiles may be \
             blank, so call again"
        ]);
    }
    Ok(Some(answer))
}

/// Largest edge of a tile written to its own file, in pixels.
const MAX_TILE_FILE_EDGE: u32 = 1024;
/// Most pixels the tiles of one call may add up to when written one per file.
const MAX_TILE_FILE_PIXELS: u64 = 4 * 1024 * 1024;

/// A views request between its first poll and its answer.
struct ViewsJob {
    request: u64,
    tiles: Vec<ViewTileLabel>,
    /// The sheet the tiles go on; `None` writes each tile to its own file.
    layout: Option<jackdaw_editor_core::views::SheetLayout>,
    format: jackdaw_editor_core::screenshot::ThumbnailFormat,
    path: PathBuf,
    isolation: Option<jackdaw_editor_core::views::Isolation>,
    bounds: (Vec3, Vec3),
}

struct ViewTileLabel {
    label: String,
    target: Option<Entity>,
}

/// Check a views request and queue its cameras. Nothing is queued when any
/// parameter is wrong.
fn plan_views(world: &mut World, params: &Value) -> BrpResult<ViewsJob> {
    use jackdaw_editor_core::views::{MAX_EDGE, MAX_TILES, MIN_TILE, SheetLayout, TilePlan};

    let targets = view_targets(world, params)?;
    let per_target = flag(params, "per_target")?;
    let angles = view_angles(params, per_target)?;
    let max_px = match params.get("max_px").filter(|max| !max.is_null()) {
        None => 512,
        Some(max) => max
            .as_u64()
            .and_then(|max| u32::try_from(max).ok())
            .filter(|max| (MIN_TILE..=MAX_EDGE).contains(max))
            .ok_or_else(|| {
                invalid_params(format!(
                    "`max_px` = {max} is not a size from {MIN_TILE} to {MAX_EDGE}"
                ))
            })?,
    };
    let format = match params.get("format").and_then(Value::as_str) {
        None | Some("jpeg" | "jpg") => jackdaw_editor_core::screenshot::ThumbnailFormat::Jpeg,
        Some("png") => jackdaw_editor_core::screenshot::ThumbnailFormat::Png,
        Some(other) => {
            return Err(invalid_params(format!(
                "`format` = {other}: expected `png` or `jpeg`"
            )));
        }
    };
    let sheet = match params.get("sheet") {
        None | Some(Value::Null) => true,
        _ => flag(params, "sheet")?,
    };
    let isolate = flag(params, "isolate")?;

    let measured: Vec<(Entity, Option<(Vec3, Vec3)>)> = targets
        .iter()
        .map(|target| {
            (
                *target,
                jackdaw_editor_core::operator_calls::summary::world_bounds(world, *target),
            )
        })
        .collect();
    let bounded: Vec<(Entity, (Vec3, Vec3))> = measured
        .iter()
        .map(|(target, bounds)| (*target, bounds.unwrap_or_else(|| unit_box(world, *target))))
        .collect();
    // Lights, cameras and markers have no bounds; they are framed only when
    // nothing named has any.
    let union = |(min, max): (Vec3, Vec3), (lo, hi): (Vec3, Vec3)| (min.min(lo), max.max(hi));
    let bounds = measured
        .iter()
        .filter_map(|(_, bounds)| *bounds)
        .reduce(union)
        .or_else(|| bounded.iter().map(|(_, bounds)| *bounds).reduce(union))
        .ok_or_else(|| invalid_params("nothing to view"))?;
    let framed: Vec<(Option<Entity>, (Vec3, Vec3))> = if per_target {
        bounded
            .iter()
            .map(|(target, bounds)| (Some(*target), *bounds))
            .collect()
    } else {
        vec![(None, bounds)]
    };
    let count = framed.len() * angles.len();
    if count > MAX_TILES {
        return Err(invalid_params(format!(
            "{count} views asked for; at most {MAX_TILES} fit one call"
        )));
    }
    let layout = if sheet {
        let columns = (per_target && angles.len() > 1).then_some(angles.len());
        Some(SheetLayout::new(count, columns, max_px).ok_or_else(|| {
            invalid_params(format!(
                "`max_px` = {max_px} leaves {count} tiles under {MIN_TILE} px each"
            ))
        })?)
    } else {
        if max_px > MAX_TILE_FILE_EDGE {
            return Err(invalid_params(format!(
                "`max_px` = {max_px}: a tile written on its own is at most \
                 {MAX_TILE_FILE_EDGE} px; ask for a sheet or a smaller size"
            )));
        }
        let pixels = count as u64 * u64::from(max_px) * u64::from(max_px);
        if pixels > MAX_TILE_FILE_PIXELS {
            return Err(invalid_params(format!(
                "{count} tiles of {max_px} px are {pixels} pixels, over the \
                 {MAX_TILE_FILE_PIXELS} one call may write; ask for fewer or smaller tiles"
            )));
        }
        None
    };
    if world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .values()
        .any(|pending| matches!(pending.state, PendingState::Views(_)))
    {
        return Err(invalid_params(
            "another views request is still rendering; wait for it",
        ));
    }
    let path = match params.get("path").and_then(Value::as_str) {
        Some(path) => project_path(world, path)?,
        None => default_capture_path(world, "views"),
    }
    .with_extension(format.extension());

    let mut plans = Vec::with_capacity(count);
    let mut tiles = Vec::with_capacity(count);
    for (target, bounds) in &framed {
        for angle in &angles {
            let (transform, projection) = jackdaw_editor_core::views::frame(*angle, *bounds);
            let label = match target {
                Some(target) => format!("{} {}", view_name(world, *target), angle.label()),
                None => angle.label().to_string(),
            };
            plans.push(TilePlan {
                label: label.clone(),
                target: *target,
                transform,
                projection,
            });
            tiles.push(ViewTileLabel {
                label,
                target: *target,
            });
        }
    }
    let edge = layout.map_or(max_px, |layout| layout.tile);
    let isolated = isolate.then_some(targets.as_slice());
    let (request, isolation) = jackdaw_editor_core::views::start(world, &plans, edge, isolated);
    Ok(ViewsJob {
        request,
        tiles,
        layout,
        format,
        path,
        isolation,
        bounds,
    })
}

/// The entities a views request names, or the selection, or every scene root.
fn view_targets(world: &mut World, params: &Value) -> BrpResult<Vec<Entity>> {
    let named = match params.get("targets") {
        None | Some(Value::Null) => None,
        Some(Value::Array(targets)) if !targets.is_empty() => Some(targets.clone()),
        Some(other) => {
            return Err(invalid_params(format!(
                "`targets` is a list of entity ids or names, not {other}"
            )));
        }
    };
    let targets: Vec<Entity> = match named {
        Some(named) => {
            let mut targets = Vec::with_capacity(named.len());
            let mut seen = EntityHashSet::default();
            for target in &named {
                let entity = match target {
                    Value::Number(_) => entity_from_params(world, &json!({ "entity": target }))?,
                    Value::String(_) => entity_from_params(world, &json!({ "name": target }))?,
                    other => {
                        return Err(invalid_params(format!(
                            "a target is an entity id or a name, not {other}"
                        )));
                    }
                };
                if !belongs_to_the_scene(world, entity) {
                    return Err(invalid_params(format!("{target} is not in the open scene")));
                }
                if seen.insert(entity) {
                    targets.push(entity);
                }
            }
            targets
        }
        None => {
            let selected: Vec<Entity> = world
                .get_resource::<Selection>()
                .map(|selection| selection.entities.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|entity| world.get_entity(*entity).is_ok())
                .collect();
            if selected.is_empty() {
                let mut roots = scene_roots(world);
                roots.sort();
                roots
            } else {
                selected
            }
        }
    };
    if targets.is_empty() {
        return Err(invalid_params(
            "the scene is empty, so there is nothing to view",
        ));
    }
    Ok(targets)
}

/// The angles a views request names: names from
/// [`jackdaw_editor_core::views::ViewAngle::NAMES`] or `{eye, target}`.
fn view_angles(
    params: &Value,
    per_target: bool,
) -> BrpResult<Vec<jackdaw_editor_core::views::ViewAngle>> {
    use jackdaw_editor_core::views::ViewAngle;
    let given = match params.get("angles") {
        None | Some(Value::Null) => {
            return Ok(if per_target {
                vec![ViewAngle::Iso]
            } else {
                ViewAngle::DEFAULT.to_vec()
            });
        }
        Some(Value::Array(angles)) if !angles.is_empty() => angles,
        Some(other) => {
            return Err(invalid_params(format!(
                "`angles` is a list of names or {{eye, target}}, not {other}"
            )));
        }
    };
    given
        .iter()
        .map(|angle| match angle {
            Value::String(name) => ViewAngle::named(name).ok_or_else(|| {
                invalid_params(format!(
                    "angle `{name}`: expected one of {} or {{eye, target}}",
                    ViewAngle::NAMES.join(", ")
                ))
            }),
            Value::Object(_) => {
                let point = |key: &str| -> BrpResult<Option<Vec3>> {
                    match angle.get(key).filter(|value| !value.is_null()) {
                        None => Ok(None),
                        Some(value) => vec_of(value, 3)
                            .map(|v| Some(Vec3::new(v[0], v[1], v[2])))
                            .ok_or_else(|| {
                                invalid_params(format!("angle `{key}` = {value} is not [x, y, z]"))
                            }),
                    }
                };
                let eye = point("eye")?
                    .ok_or_else(|| invalid_params("an `{eye, target}` angle needs an `eye`"))?;
                let target = point("target")?;
                if target.is_some_and(|target| target == eye) {
                    return Err(invalid_params("an angle's eye is its target"));
                }
                Ok(ViewAngle::Eye { eye, target })
            }
            other => Err(invalid_params(format!(
                "an angle is a name or {{eye, target}}, not {other}"
            ))),
        })
        .collect()
}

/// A unit box at `target`'s position, for a target with no bounds of its own.
fn unit_box(world: &World, target: Entity) -> (Vec3, Vec3) {
    let at = world
        .get::<GlobalTransform>(target)
        .map(GlobalTransform::translation)
        .or_else(|| world.get::<Transform>(target).map(|t| t.translation))
        .unwrap_or_default();
    (at - Vec3::splat(0.5), at + Vec3::splat(0.5))
}

fn view_name(world: &World, entity: Entity) -> String {
    world.get::<Name>(entity).map_or_else(
        || entity.to_bits().to_string(),
        |name| name.as_str().to_string(),
    )
}

/// Write a views request's tiles, as one sheet or a file each, and the answer
/// naming them.
fn write_views(
    tiles: &[ViewTileLabel],
    layout: Option<jackdaw_editor_core::views::SheetLayout>,
    format: jackdaw_editor_core::screenshot::ThumbnailFormat,
    path: &Path,
    images: &[::image::RgbImage],
    (min, max): (Vec3, Vec3),
) -> BrpResult<Value> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    let bounds = json!([
        jackdaw_editor_core::operator_calls::summary::vector(min),
        jackdaw_editor_core::operator_calls::summary::vector(max)
    ]);
    let tile_entry = |tile: &ViewTileLabel, mut entry: Value| {
        entry["label"] = json!(tile.label);
        if let Some(target) = tile.target {
            entry["entity"] = json!(target.to_bits());
        }
        entry
    };
    let cannot = |path: &Path, err: ::image::ImageError| {
        internal_error(format!("cannot write {}: {err}", path.display()))
    };
    match layout {
        Some(layout) => {
            let sheet = jackdaw_editor_core::views::compose(layout, images);
            jackdaw_editor_core::screenshot::save_rgb(&sheet, path, format)
                .map_err(|err| cannot(path, err))?;
            let laid: Vec<Value> = tiles
                .iter()
                .enumerate()
                .map(|(index, tile)| {
                    let (x, y) = layout.origin(index);
                    tile_entry(tile, json!({ "at": [x, y, layout.tile, layout.tile] }))
                })
                .collect();
            Ok(json!({
                "path": path,
                "width": sheet.width(),
                "height": sheet.height(),
                "tiles": laid,
                "bounds": bounds,
            }))
        }
        None => {
            let mut written = Vec::with_capacity(tiles.len());
            for (index, (tile, image)) in tiles.iter().zip(images).enumerate() {
                let stem = path.file_stem().map_or_else(
                    || "views".to_string(),
                    |stem| stem.to_string_lossy().into_owned(),
                );
                let file = path.with_file_name(format!("{stem}-{index}.{}", format.extension()));
                jackdaw_editor_core::screenshot::save_rgb(image, &file, format)
                    .map_err(|err| cannot(&file, err))?;
                written.push(tile_entry(
                    tile,
                    json!({ "path": file, "width": image.width(), "height": image.height() }),
                ));
            }
            Ok(json!({ "tiles": written, "bounds": bounds }))
        }
    }
}

/// Let frames pass, or wait for the editor to reach a state. `until` takes
/// `idle`, `pie_running` (a paused game counts) or `pie_stopped`; without it
/// the call waits `frames`.
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
        // Idle means no build, model load, background task or synthetic gesture
        // is still running. A modal operator is reported rather than waited on,
        // since nothing is going to finish it.
        if editor_is_busy(world) {
            return Ok(None);
        }
        return Ok(Some(json!({ "idle": true, "modal": modal })));
    }

    let frames = params.get("frames").and_then(Value::as_u64).unwrap_or(1) as u32;
    let key = request_key("jackdaw/wait", &params);
    let mut waits = world.get_resource_or_init::<PendingWaits>();
    // The frame the wait is over, rather than a countdown: a poll is not a
    // frame, and two clients sharing a key would each decrement one countdown.
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
/// otherwise. Ten minutes at 60fps, since a game build takes minutes.
const PIE_WAIT_FRAMES: u32 = 36_000;

/// Hold until play-in-editor reaches `wanted`, the build behind it fails, or the
/// wait runs out of frames. A `pie_stopped` wait resolves only once a game has
/// been seen building or up, since `pie.play` returns while the editor still
/// reads as stopped.
fn pie_wait(
    world: &mut World,
    params: &Value,
    wanted: &'static str,
    frame: u32,
) -> BrpResult<Option<Value>> {
    let key = request_key("jackdaw/wait", params);
    let status = jackdaw_editor_core::pie::play_status(world);
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
        "running" => matches!(status, "running" | "paused"),
        _ => wait.seen_active && status == "stopped",
    };
    let gone = wait.wanted == "running" && wait.seen_active && status == "stopped";
    let out_of_frames = frame >= wait.until;

    // A build that did not compile, or a game that went away before it
    // connected, ends every wait on it; holding for the frame cap would report
    // the failure as a timeout.
    if status == "failed" {
        waits.requests.remove(&key);
        return Err(internal_error("the game build failed"));
    }
    if gone {
        waits.requests.remove(&key);
        return Err(internal_error("the game stopped before it connected"));
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

/// The running game's entities that no scene node spawned, with the authored
/// entities they sit under.
pub fn game_entities_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    ask_the_game(world, "jackdaw/game_entities", params, |_| {
        Ok(GameQuery::Runtime)
    })
}

/// One entity of the running game with all its components, by `entity` (its
/// bits in the game) or every entity of the scene node `node`.
pub fn game_entity_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    ask_the_game(world, "jackdaw/game_entity", params, |params| {
        let id = |name: &str| params.get(name).and_then(Value::as_u64);
        match (id("entity"), id("node")) {
            (Some(entity), _) => Ok(GameQuery::Entity(GameEntityRef::Entity(entity))),
            (None, Some(node)) => Ok(GameQuery::Entity(GameEntityRef::Node(node))),
            (None, None) => Err(invalid_params("pass `entity` or `node`")),
        }
    })
}

/// The running game's entities holding the component `with` (a type path), and
/// whose name contains `name`, each with the value of `with`.
pub fn game_query_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    ask_the_game(world, "jackdaw/game_query", params, |params| {
        let text = |name: &str| params.get(name).and_then(Value::as_str).map(str::to_string);
        Ok(GameQuery::Find {
            with: text("with"),
            name: text("name"),
        })
    })
}

/// Ask the running game the question `query` makes of the params on the first
/// poll, and answer with its entities once the game replies. The poll the HTTP
/// layer makes just after the answer goes out finds it answered and asks nothing.
fn ask_the_game(
    world: &mut World,
    method: &str,
    params: Option<Value>,
    query: impl FnOnce(&Value) -> BrpResult<GameQuery>,
) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let frame = current_frame(world);
    expire_stale(world, frame);
    let key = request_key(method, &params);
    let held = match world
        .get_resource_or_init::<PendingWaits>()
        .requests
        .get_mut(&key)
    {
        Some(PendingRequest {
            state: PendingState::Answered,
            last_seen,
        }) if frame <= last_seen.saturating_add(1) => return Ok(None),
        None
        | Some(PendingRequest {
            state: PendingState::Answered,
            ..
        }) => None,
        Some(pending) => {
            pending.last_seen = frame;
            match pending.state {
                PendingState::Game { request } => Some(request),
                _ => {
                    return Err(internal_error(format!(
                        "request {key} is already waiting on something else"
                    )));
                }
            }
        }
    };
    let held = held.filter(|&request| world.get_resource_or_init::<GameQueries>().holds(request));
    let request = match held {
        Some(request) => request,
        None => {
            let request = ask_game(world, query(&params)?).map_err(internal_error)?;
            world.resource_mut::<PendingWaits>().requests.insert(
                key.clone(),
                PendingRequest {
                    last_seen: frame,
                    state: PendingState::Game { request },
                },
            );
            request
        }
    };
    let Some(answer) = world.get_resource_or_init::<GameQueries>().take(request) else {
        return Ok(None);
    };
    world.resource_mut::<PendingWaits>().requests.insert(
        key,
        PendingRequest {
            last_seen: frame,
            state: PendingState::Answered,
        },
    );
    let found = answer.map_err(internal_error)?;
    Ok(Some(json!({
        "entities": found.entities,
        "truncated": found.truncated,
    })))
}

/// Work the editor started on its own and has not finished.
fn editor_is_busy(world: &mut World) -> bool {
    let building = world
        .get_resource::<jackdaw_editor_core::build_status::BuildStatus>()
        .is_some_and(|status| {
            matches!(
                status.state,
                jackdaw_editor_core::build_status::BuildState::Building { .. }
            )
        });
    // A model still queued for the scene spawner is not loading and has no
    // handle to ask about, so the queue is asked before the load states.
    let placing = world
        .get_resource::<jackdaw_editor_core::entity_ops::PendingModelRoots>()
        .is_some_and(|pending| !pending.is_empty());
    let refining = world
        .get_resource::<jackdaw_runtime::LiveLevelProgress>()
        .is_some_and(|progress| !progress.is_refined())
        || world
            .get_resource::<jackdaw_scene_types::model_parts::ModelParts>()
            .is_some_and(|parts| parts.loading_count() > 0);
    // A synthetic gesture is delivered over the frames after its call returns.
    let gesturing = world
        .get_resource::<jackdaw_editor_core::test_input::SyntheticInput>()
        .is_some_and(|input| !input.is_idle());
    let pacing = world
        .get_resource::<super::paced::PacedBatches>()
        .is_some_and(super::paced::PacedBatches::is_playing);
    building
        || placing
        || refining
        || gesturing
        || pacing
        || world
            .query::<&jackdaw_api_internal::BackgroundTask>()
            .iter(world)
            .next()
            .is_some()
        || scene_is_loading(world)
}

/// Whether any model the open scene names is still coming off disk. Opening a
/// scene puts its entities in the world in one frame and its glTF instances in
/// over the following hundreds.
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
/// The recursive state, not the direct one: a glTF root reports `Loaded` while
/// its meshes and images are still coming in. A failed or unknown load is not
/// waited on, since no amount of waiting produces it.
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

/// Forget requests nothing has polled for `STALE_FRAMES`.
fn expire_stale(world: &mut World, frame: u32) {
    let mut dropped: Vec<(PathBuf, Option<Box<Aim>>)> = Vec::new();
    let mut abandoned: Vec<Box<ViewsJob>> = Vec::new();
    {
        let mut waits = world.get_resource_or_init::<PendingWaits>();
        waits.requests.retain(|_, pending| {
            if frame.saturating_sub(pending.last_seen) < STALE_FRAMES {
                return true;
            }
            match std::mem::replace(&mut pending.state, PendingState::Answered) {
                PendingState::Capture { path, aim, .. } => dropped.push((path, aim)),
                PendingState::Views(job) => abandoned.push(job),
                _ => {}
            }
            false
        });
    }
    for job in abandoned {
        jackdaw_editor_core::views::finish(world, job.request, job.isolation);
    }
    // A capture whose client left still lands on disk; its log entry goes with
    // the request so the next capture to that path is not answered with it,
    // and a camera it aimed goes back if nobody has moved it since.
    for (path, aim) in dropped {
        if let Some(aim) = aim {
            aim.put_back(world);
        }
        if let Some(mut log) =
            world.get_resource_mut::<jackdaw_editor_core::screenshot::CaptureLog>()
        {
            log.forget(&path);
        }
    }
}

/// The key a watching request to `method` is tracked under. A client that passes
/// `request` gets an identity of its own; one that does not shares state with any
/// concurrent call to the same method spelled identically.
fn request_key(method: &str, params: &Value) -> String {
    match params.get("request").and_then(Value::as_str) {
        Some(id) => format!("id:{method}:{id}"),
        None => format!("params:{method}:{params}"),
    }
}

/// What the two waiting methods have promised to answer.
#[derive(Resource, Default)]
struct PendingWaits {
    requests: HashMap<String, PendingRequest>,
}

struct PendingRequest {
    /// The frame this request was last polled on, for `expire_stale`.
    last_seen: u32,
    state: PendingState,
}

enum PendingState {
    Capture {
        path: PathBuf,
        /// Whether the capture is queued yet; an aimed one waits a frame so
        /// the camera's new place reaches the render.
        queued: bool,
        /// The camera the capture aimed.
        aim: Option<Box<Aim>>,
    },
    /// Offscreen views waiting for their tiles.
    Views(Box<ViewsJob>),
    /// A capture already answered, kept until the HTTP layer stops polling.
    Answered,
    /// The frame at which the wait is over.
    Frames { until: u32 },
    /// The asset walk running on the IO pool.
    Assets { task: Task<Vec<String>> },
    /// A play-in-editor state being held for.
    Pie(PieWait),
    /// A question the running game has yet to answer.
    Game { request: u64 },
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
    /// A scene is not settled until its models are in.
    #[test]
    fn a_loading_model_is_work_the_editor_is_still_doing() {
        use bevy::asset::RecursiveDependencyLoadState as State;
        assert!(any_still_loading([Some(State::Loading)].into_iter()));
        assert!(any_still_loading([Some(State::NotLoaded)].into_iter()));
        assert!(any_still_loading(
            [Some(State::Loaded), Some(State::Loading)].into_iter()
        ));
    }

    /// A load that failed is not waited on.
    #[test]
    fn a_failed_or_unknown_load_is_not_waited_on() {
        use bevy::asset::RecursiveDependencyLoadState as State;
        assert!(!any_still_loading([Some(State::Loaded)].into_iter()));
        assert!(!any_still_loading([None].into_iter()));
        assert!(!any_still_loading(std::iter::empty()));
        assert!(!any_still_loading([Some(State::Loaded), None].into_iter()));
    }

    /// A `*` anchors what sits beside it.
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

    /// A pattern with no `*` is a plain substring, anchored nowhere.
    #[test]
    fn a_pattern_with_no_star_matches_anywhere_in_the_path() {
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "Fence"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", "kit/"));
        assert!(matches_pattern("kit/Prop_Fence_01.gltf", ".gltf"));
        assert!(!matches_pattern("kit/Prop_Fence_01.gltf", "Wagon"));
    }

    /// A document held in both forms is one entry, under its text path.
    #[test]
    fn a_listing_names_a_document_once_and_says_a_binary_twin_is_there() {
        let found = vec![
            "models/town.glb".to_string(),
            "zones/hedgerow.bsb".to_string(),
            "zones/hedgerow.bsn".to_string(),
            "zones/shipped.bsb".to_string(),
        ];

        let listed = collapse_document_twins(found);

        assert_eq!(
            listed,
            vec![
                ("models/town.glb".to_string(), false),
                ("zones/hedgerow.bsn".to_string(), true),
                ("zones/shipped.bsb".to_string(), true),
            ]
        );
    }

    /// The default port is not the game's, so an editor and the game it
    /// launches never fight over the socket.
    #[test]
    fn the_editor_port_is_not_the_game_port() {
        assert_ne!(DEFAULT_PORT, jackdaw_remote::DEFAULT_PORT);
    }
}
