use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use bevy::{
    app::SceneSpawnerSystems,
    ecs::{
        entity::EntityHashSet,
        system::{SystemParam, SystemState},
    },
    gltf::GltfAssetLabel,
    prelude::*,
    world_serialization::WorldAsset,
};

use crate::{
    EditorEntity,
    commands::{CommandHistory, DespawnEntity, EditorCommand, HierarchyLocation, MoveEntity},
    frame_work::FramePace,
    selection::{Selected, Selection},
};

/// Routes entity copy and paste through the OS clipboard, Bevy's [`Clipboard`].
/// Without it they use the [`EntityClipboard`] mirror alone.
#[derive(Resource, Default)]
pub struct SystemClipboard {
    /// The text this editor last put on the OS clipboard, so a paste can tell
    /// its own emission from a stranger's.
    last_emitted: String,
}

/// The last entity subtree copied in this editor, as BSN text. It mirrors every
/// copy, so a paste works when the OS clipboard is absent or holds no scene.
#[derive(Resource, Default)]
pub struct EntityClipboard {
    /// BSN text, empty until the first copy.
    pub text: String,
}

pub use jackdaw_scene_types::GltfSource;
use jackdaw_scene_types::LodGroup;
use jackdaw_scene_types::model_import::{ModelLevels, ModelLodIndex};

pub struct EntityOpsPlugin;

impl Plugin for EntityOpsPlugin {
    fn build(&self, app: &mut App) {
        // Note: GltfSource type registration is handled by SceneTypesPlugin
        app.init_resource::<SystemClipboard>()
            .init_resource::<EntityClipboard>()
            .init_resource::<PendingModelRoots>()
            .add_systems(Update, hand_out_model_roots)
            .add_systems(PostUpdate, ask_for_waiting_models)
            .add_systems(
                SpawnScene,
                (
                    start_model_cost.before(SceneSpawnerSystems::WorldInstanceSpawn),
                    measure_model_cost.after(SceneSpawnerSystems::WorldInstanceSpawn),
                ),
            )
            .add_observer(derive_world_asset_root)
            .add_observer(forget_model_root)
            .add_observer(queue_models_past_last_level)
            .add_observer(queue_models_of_ungrouped_levels)
            .add_observer(queue_models_left_by_settings)
            .register_type::<EmptyEntity>()
            .register_type::<SceneCamera>()
            .register_type::<SceneLight>()
            .register_type::<SceneFogVolume>()
            .register_type::<SceneReflectionProbe>()
            .register_type::<SceneAnimationPlayer>()
            .register_type::<SceneAudioSource>();
    }
}

/// The least a frame will spend bringing models on, whatever else it is doing.
const MODEL_FLOOR_BUDGET: Duration = Duration::from_millis(16);

/// How far an added point or spot light reaches, in metres. Shadows stay off
/// until asked for: each shadowed point light renders six shadow maps.
const NEW_LIGHT_RANGE: f32 = 10.0;

/// The fewest and the most models one frame hands over.
const MODEL_BATCH_FLOOR: usize = 1;
const MODEL_BATCH_CEILING: usize = 256;

/// What bringing models on may cost a frame while a scene is opening. The
/// load overlay is up and nothing is being edited, so the scene comes in at a
/// few frames a second rather than at the editing frame rate.
const MODEL_LOAD_BUDGET: Duration = Duration::from_millis(150);

/// The most models one frame hands over while a scene is opening.
const MODEL_LOAD_BATCH_CEILING: usize = 4096;

/// Models whose render root has been derived but not yet handed to the
/// world-asset spawner.
///
/// Inserting `WorldAssetRoot` registers an instance the spawner builds later
/// in the same frame, and a large scene holds thousands of them: handing a
/// whole document over at once spends the frame in the spawner and the window
/// never paints. They wait here and go out a batch a frame instead.
#[derive(Resource)]
pub struct PendingModelRoots {
    /// The order models come on in, first derived first.
    order: VecDeque<Entity>,
    /// The model each waiting entity is to be given. An entity whose source
    /// changed again before its turn keeps its place and takes the later
    /// model, so nothing comes on twice.
    /// `None` until the entity can be told apart from a LOD level, which needs
    /// its parent in place: a group keeps its levels' models to itself, so
    /// asking for the file waits until then.
    wanted: HashMap<Entity, Option<Handle<WorldAsset>>>,
    /// Entities waiting with no model asked for yet, in the order they came.
    unasked: Vec<Entity>,
    /// Entities whose model's import settings are still being read, asked
    /// about again on the next frame.
    waiting: Vec<Entity>,
    pace: FramePace,
    /// The pace while a scene is opening, kept apart so a load does not leave
    /// editing with a batch sized for a load.
    load_pace: FramePace,
    /// When the spawner started on this frame's batch, so what bringing it on
    /// cost can be read once the spawner has run.
    handed: Option<Instant>,
    /// What the last batch cost the world-asset spawner.
    /// The frame time as a whole says nothing useful here: a heavy scene
    /// renders slowly whether or not anything is still coming on.
    cost: Duration,
}

impl Default for PendingModelRoots {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            wanted: HashMap::new(),
            unasked: Vec::new(),
            waiting: Vec::new(),
            pace: FramePace::new(MODEL_BATCH_FLOOR, MODEL_BATCH_CEILING),
            load_pace: FramePace::new(MODEL_BATCH_FLOOR, MODEL_LOAD_BATCH_CEILING),
            handed: None,
            cost: Duration::ZERO,
        }
    }
}

impl PendingModelRoots {
    /// How many models are still waiting to come on. What is still wanted, not
    /// what is still in the order: an entity that has gone leaves its place in
    /// the order behind and is stepped over when its turn comes.
    pub fn len(&self) -> usize {
        self.wanted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.wanted.is_empty()
    }

    /// The models still waiting their turn.
    pub fn handles(&self) -> impl Iterator<Item = &Handle<WorldAsset>> {
        self.wanted.values().flatten()
    }

    fn push(&mut self, entity: Entity, scene: Option<Handle<WorldAsset>>) {
        if scene.is_none() {
            self.unasked.push(entity);
        }
        if self.wanted.insert(entity, scene).is_none() {
            self.order.push_back(entity);
        }
    }

    /// Stop waiting to bring a model on. The model it was to be given goes
    /// with it, so the glTF it names is held only for models still wanted.
    fn forget(&mut self, entity: Entity) {
        self.wanted.remove(&entity);
    }

    /// The models this frame takes on.
    ///
    /// While editing, bringing them on may cost the frame about as much as
    /// everything else in it already does, so a scene that is slow to draw
    /// still fills in at a useful rate and a frame never much more than
    /// doubles. While a scene is opening they get a fixed share of the frame.
    fn take(
        &mut self,
        frame: Duration,
        opening: bool,
    ) -> Vec<(Entity, Option<Handle<WorldAsset>>)> {
        let count = if opening {
            self.load_pace
                .take(self.cost, MODEL_LOAD_BUDGET, self.wanted.len())
        } else {
            let budget = frame.saturating_sub(self.cost).max(MODEL_FLOOR_BUDGET);
            self.pace.take(self.cost, budget, self.wanted.len())
        };
        let mut taking = Vec::with_capacity(count);
        while taking.len() < count {
            let Some(entity) = self.order.pop_front() else {
                break;
            };
            if let Some(scene) = self.wanted.remove(&entity) {
                taking.push((entity, scene));
            }
        }
        if !taking.is_empty() {
            self.handed = Some(Instant::now());
        }
        taking
    }
}

/// Stop waiting to bring on a model whose source has gone, rather than leaving
/// it queued until its turn comes.
///
/// A queued model holds the glTF it names, and the footer and the wait for an
/// idle editor both count what is queued; an entry nothing will use any more
/// would hold an asset nothing renders and a wait nothing can end.
fn forget_model_root(remove: On<Remove<GltfSource>>, mut pending: ResMut<PendingModelRoots>) {
    pending.forget(remove.entity);
}

/// Stop waiting to bring on the models queued for `entities`, for a caller
/// taking them down itself.
///
/// What is queued belongs to whoever authored it: a scene taken down takes its
/// own queue with it, and leaves alone the models queued beside it for the
/// thumbnail stage and anything else that outlives one scene.
pub(crate) fn forget_model_roots<'a>(
    world: &mut World,
    entities: impl IntoIterator<Item = &'a Entity>,
) {
    let Some(mut pending) = world.get_resource_mut::<PendingModelRoots>() else {
        return;
    };
    for entity in entities {
        pending.forget(*entity);
    }
}

/// Derive the render-side `WorldAssetRoot` from the authored `GltfSource`,
/// the way reference images and terrains derive their render state.
///
/// Undo, redo, tab swaps and file loads all re-insert `GltfSource` from the
/// document, so deriving it here is what brings the model back on each of
/// those paths without the handle ever being written to the document.
fn derive_world_asset_root(
    insert: On<Insert<GltfSource>>,
    sources: Query<&GltfSource>,
    existing: Query<&WorldAssetRoot>,
    parents: Query<&ChildOf>,
    groups: Query<(&LodGroup, &Children)>,
    own_groups: Query<(), Or<(With<LodGroup>, With<ModelLevels>)>>,
    asset_server: Res<AssetServer>,
    imports: Res<ModelLodIndex>,
    mut pending: ResMut<PendingModelRoots>,
) {
    let entity = insert.entity;
    let Ok(source) = sources.get(entity) else {
        return;
    };
    let lod_level = own_groups.contains(entity)
        || parents
            .get(entity)
            .and_then(|parent| groups.get(parent.parent()))
            .is_ok_and(|(group, children)| jackdaw_runtime::is_level_of(group, children, entity));
    if lod_level {
        pending.forget(entity);
        return;
    }
    let Ok(root) = existing.get(entity) else {
        pending.push(entity, None);
        return;
    };
    let scene = model_scene(&asset_server, Some(&imports), source);
    // Re-inserting an equal handle still trips `Changed`, and the world-asset
    // spawner despawns and respawns the whole instance on every change.
    // Applying the document re-inserts `GltfSource` wholesale, so without this
    // the model is rebuilt on every undo.
    if root.0 == scene {
        return;
    }
    pending.push(entity, Some(scene));
}

/// The glTF scene `source` names, as the handle the world-asset spawner
/// instantiates.
fn model_scene(
    asset_server: &AssetServer,
    imports: Option<&ModelLodIndex>,
    source: &GltfSource,
) -> Handle<WorldAsset> {
    // Scenes authored before paths were normalised still hold an absolute
    // path; `to_asset_path` reduces those and passes a relative one through.
    let asset_path = to_asset_path(&source.path);
    jackdaw_scene_types::render_assets::load_model(
        asset_server,
        imports,
        GltfAssetLabel::Scene(source.scene_index).from_asset(asset_path),
    )
}

/// Queue the models of children a LOD group no longer keeps as levels: those
/// past its last level after a change.
fn queue_models_past_last_level(
    insert: On<Insert<LodGroup>>,
    groups: Query<(&LodGroup, &Children)>,
    unplaced: Query<&GltfSource, Without<WorldAssetRoot>>,
    asset_server: Res<AssetServer>,
    imports: Res<ModelLodIndex>,
    mut pending: ResMut<PendingModelRoots>,
) {
    let Ok((group, children)) = groups.get(insert.entity) else {
        return;
    };
    for model in children.iter().skip(group.levels.len()) {
        if let Ok(source) = unplaced.get(model) {
            pending.push(
                model,
                Some(model_scene(&asset_server, Some(&imports), source)),
            );
        }
    }
}

/// Queue the models a LOD group that is going kept live as its levels: each
/// child's, or its own where it names the model itself.
fn queue_models_of_ungrouped_levels(
    remove: On<Remove<LodGroup>>,
    children: Query<&Children>,
    unplaced: Query<&GltfSource, Without<WorldAssetRoot>>,
    asset_server: Res<AssetServer>,
    imports: Res<ModelLodIndex>,
    mut pending: ResMut<PendingModelRoots>,
) {
    let group = remove.entity;
    let levels = children
        .get(group)
        .into_iter()
        .flat_map(RelationshipTarget::iter);
    for model in std::iter::once(group).chain(levels) {
        if let Ok(source) = unplaced.get(model) {
            pending.push(
                model,
                Some(model_scene(&asset_server, Some(&imports), source)),
            );
        }
    }
}

/// Queue the model of a placement that no longer draws the levels its import
/// settings gave it, once it is told apart from a LOD level.
fn queue_models_left_by_settings(
    remove: On<Remove<ModelLevels>>,
    unplaced: Query<(), (With<GltfSource>, Without<WorldAssetRoot>)>,
    mut pending: ResMut<PendingModelRoots>,
) {
    if unplaced.contains(remove.entity) {
        pending.push(remove.entity, None);
    }
}

/// How the model `entity` names comes on now: as an instance, or not at all
/// because a LOD group or its import settings draw it as live levels. `None`
/// while its import settings are still being read, so it keeps its place.
fn instance_wanted(world: &World, entity: Entity) -> Option<bool> {
    match jackdaw_runtime::model_placement(world, entity) {
        jackdaw_runtime::ModelPlacement::Instance => Some(true),
        jackdaw_runtime::ModelPlacement::Levels => Some(false),
        jackdaw_runtime::ModelPlacement::Waiting => None,
    }
}

/// Ask for the files of the models waiting their turn as soon as their
/// parents are in place, and drop the LOD levels among them, which their
/// groups keep live. Runs on the frame a scene spawns, so ordinary models are
/// asked for ahead of every file a LOD group asks for.
fn ask_for_waiting_models(world: &mut World) {
    if world.resource::<PendingModelRoots>().unasked.is_empty()
        && world.resource::<PendingModelRoots>().waiting.is_empty()
    {
        return;
    }
    world.resource_scope(|world, mut pending: Mut<PendingModelRoots>| {
        let waited = std::mem::take(&mut pending.waiting);
        pending.unasked.extend(waited);
        for entity in std::mem::take(&mut pending.unasked) {
            if !matches!(pending.wanted.get(&entity), Some(None)) {
                continue;
            }
            match instance_wanted(world, entity) {
                Some(true) => {}
                Some(false) => {
                    pending.forget(entity);
                    continue;
                }
                None => {
                    pending.waiting.push(entity);
                    continue;
                }
            }
            let Some(source) = world.get::<GltfSource>(entity) else {
                continue;
            };
            let scene = model_scene(
                world.resource::<AssetServer>(),
                world.get_resource(),
                source,
            );
            pending.wanted.insert(entity, Some(scene));
        }
    });
}

/// What the footer calls the models still coming on.
const MODEL_PHASE: &str = "models";

/// Hand the next batch of models to the world-asset spawner. Runs in `Update`,
/// so what it inserts is spawned in the same frame's `SpawnScene`.
fn hand_out_model_roots(
    time: Res<Time<Real>>,
    progress: Option<Res<crate::progress::EditorProgress>>,
    mut pending: ResMut<PendingModelRoots>,
    // Worlds without a footer -- the launcher, and the test harnesses -- still
    // bring models on.
    mut phase: Option<ResMut<crate::status_bar::EditorPhase>>,
    mut named: Local<bool>,
    mut commands: Commands,
) {
    if pending.is_empty() {
        if std::mem::take(&mut *named)
            && let Some(phase) = phase.as_mut()
        {
            phase.finish(MODEL_PHASE);
        }
        return;
    }
    if let Some(phase) = phase.as_mut() {
        phase.begin(MODEL_PHASE, format!("Placing {} models", pending.len()));
        *named = true;
    }
    let opening = progress
        .is_some_and(|progress| progress.is_running(crate::scenes::load_progress::SCENE_LOAD));
    let batch = pending.take(time.delta(), opening);
    commands.queue(move |world: &mut World| {
        for (entity, scene) in batch {
            match instance_wanted(world, entity) {
                Some(true) => {}
                Some(false) => continue,
                None => {
                    world
                        .resource_mut::<PendingModelRoots>()
                        .push(entity, scene);
                    continue;
                }
            }
            // The source may also have gone -- an undo of the placement that
            // set it, say. An instance under an entity that no longer names a
            // model is one nothing owns and nothing takes down again.
            let Some(source) = world.get::<GltfSource>(entity) else {
                continue;
            };
            let scene = scene.unwrap_or_else(|| {
                model_scene(
                    world.resource::<AssetServer>(),
                    world.get_resource(),
                    source,
                )
            });
            // Between deriving the root and this frame the entity may have
            // been despawned, or already given this very handle.
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                continue;
            };
            if entity
                .get::<WorldAssetRoot>()
                .is_some_and(|root| root.0 == scene)
            {
                continue;
            }
            entity.insert(WorldAssetRoot(scene));
        }
    });
}

/// Start the clock on this frame's batch as the world-asset spawner reaches it.
fn start_model_cost(mut pending: ResMut<PendingModelRoots>) {
    if pending.handed.is_some() {
        pending.bypass_change_detection().handed = Some(Instant::now());
    }
}

/// Read what the batch this frame handed over cost, once the scene spawner
/// that builds it has run.
fn measure_model_cost(pending: ResMut<PendingModelRoots>) {
    // Reaching through `ResMut` marks the resource changed, and most frames
    // hand nothing over, so the empty case answers before it does.
    if pending.handed.is_none() {
        return;
    }
    let pending = pending.into_inner();
    if let Some(handed) = pending.handed.take() {
        pending.cost = handed.elapsed();
    }
}

/// Marks an entity as an intentionally-empty scene entity (`Add > Empty`).
/// Used by the viewport-overlay system to decide whether to draw a
/// fallback wireframe-cube marker. Serialises through the type registry
/// so empties loaded from a `.jsn` scene keep the marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct EmptyEntity;

/// Marks a camera as scene-authored (added via `Add > Camera` or by an
/// extension), so viewport overlays draw a frustum gizmo for it.
/// Editor-internal cameras (main viewport camera, material preview
/// camera) deliberately don't carry this marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneCamera;

/// Marks a light as scene-authored, so viewport overlays draw
/// light-specific gizmos for it. Editor-internal lights (e.g. the
/// material-preview rig) deliberately don't carry this marker.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneLight;

/// Marks a fog-volume entity (`Add > Fog Volume`), so viewport
/// overlays draw a box gizmo at the volume's extent. The box is the
/// unit cube scaled by the entity's `Transform.scale`.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneFogVolume;

/// Marks a reflection-probe entity (`Add > Reflection Probe`), so
/// viewport overlays draw a box gizmo at the probe's influence region.
/// The box is the unit cube scaled by the entity's `Transform.scale`. A new
/// probe reflects nothing until `environment.bake_probe` bakes it.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneReflectionProbe;

/// Marks an animation-player entity (`Add > Animation Player`) so
/// viewport overlays draw a marker gizmo for it. The entity has no
/// spatial extent, so the marker is the only on-screen cue.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneAnimationPlayer;

/// Marks an audio-source entity (`Add > Audio Source`) so viewport
/// overlays draw a marker gizmo for it. The entity has no spatial
/// extent, so the marker is the only on-screen cue.
#[derive(Component, Default, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneAudioSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityTemplate {
    Empty,
    Cube,
    Sphere,
    PointLight,
    DirectionalLight,
    SpotLight,
    Camera3d,
    Camera2d,
    #[cfg(feature = "camera_rig")]
    CameraRig,
    Plane,
    Cylinder,
    Wedge,
    Cone,
    Pyramid,
    FogVolume,
    AnimationPlayer,
    AudioSource,
}

impl EntityTemplate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty Entity",
            Self::Cube => "Cube",
            Self::Sphere => "Sphere",
            Self::PointLight => "Point Light",
            Self::DirectionalLight => "Directional Light",
            Self::SpotLight => "Spot Light",
            Self::Camera3d => "3D Camera",
            Self::Camera2d => "2D Camera",
            #[cfg(feature = "camera_rig")]
            Self::CameraRig => "Camera Rig",
            Self::Plane => "Plane",
            Self::Cylinder => "Cylinder",
            Self::Wedge => "Wedge",
            Self::Cone => "Cone",
            Self::Pyramid => "Pyramid",
            Self::FogVolume => "Fog Volume",
            Self::AnimationPlayer => "Animation Player",
            Self::AudioSource => "Audio Source",
        }
    }
}

/// Where a light or 3D camera `template` spawns; identity for anything else.
pub(crate) fn template_transform(template: EntityTemplate) -> Transform {
    match template {
        EntityTemplate::PointLight => Transform::from_xyz(0.0, 3.0, 0.0),
        EntityTemplate::DirectionalLight => {
            Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.4, 0.0))
                .with_translation(Vec3::new(0.0, 10.0, 0.0))
        }
        EntityTemplate::SpotLight => {
            Transform::from_xyz(0.0, 3.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y)
        }
        EntityTemplate::Camera3d => {
            Transform::from_xyz(0.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y)
        }
        _ => Transform::default(),
    }
}

pub fn create_entity(
    commands: &mut Commands,
    template: EntityTemplate,
    selection: &mut Selection,
) -> Entity {
    let entity = match template {
        EntityTemplate::Empty => commands
            .spawn((
                Name::new("Empty"),
                EmptyEntity,
                Transform::default(),
                // Required so `InheritedVisibility` exists on the
                // entity. Without it, viewport-overlay systems that
                // gate on `InheritedVisibility` (e.g. the empty
                // wireframe gizmo) silently skip the entity.
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::Cube => {
            let id = commands
                .spawn((
                    Name::new("Cube"),
                    crate::brush::Brush::cuboid(0.5, 0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Sphere => {
            let id = commands
                .spawn((
                    Name::new("Sphere"),
                    crate::brush::Brush::sphere(0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::PointLight => commands
            .spawn((
                Name::new("Point Light"),
                SceneLight,
                PointLight {
                    range: NEW_LIGHT_RANGE,
                    ..default()
                },
                template_transform(template),
            ))
            .id(),
        EntityTemplate::DirectionalLight => commands
            .spawn((
                Name::new("Directional Light"),
                SceneLight,
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                template_transform(template),
            ))
            .id(),
        EntityTemplate::SpotLight => commands
            .spawn((
                Name::new("Spot Light"),
                SceneLight,
                SpotLight {
                    range: NEW_LIGHT_RANGE,
                    ..default()
                },
                template_transform(template),
            ))
            .id(),
        // Both camera templates save the camera active so Play draws through
        // it; `deactivate_document_cameras` keeps it out of the editor's window.
        EntityTemplate::Camera3d => commands
            .spawn((
                Name::new("Camera"),
                SceneCamera,
                Camera3d::default(),
                template_transform(template),
            ))
            .id(),
        EntityTemplate::Camera2d => {
            let camera = commands
                .spawn((
                    Name::new("Camera"),
                    SceneCamera,
                    Camera2d,
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(move |world: &mut World| {
                crate::pixel_camera::zoom_new_camera(world, camera);
            });
            camera
        }
        #[cfg(feature = "camera_rig")]
        EntityTemplate::CameraRig => commands
            .spawn((
                Name::new("Camera Rig"),
                jackdaw_camera_rig::CameraRig::default(),
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::Plane => {
            let id = commands
                .spawn((
                    Name::new("Plane"),
                    crate::brush::Brush::plane(0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Cylinder => {
            let id = commands
                .spawn((
                    Name::new("Cylinder"),
                    crate::brush::Brush::cylinder(0.5, 0.5, 16),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Wedge => {
            let id = commands
                .spawn((
                    Name::new("Wedge"),
                    crate::brush::Brush::wedge(0.5, 0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Cone => {
            let id = commands
                .spawn((
                    Name::new("Cone"),
                    crate::brush::Brush::cone(0.5, 0.5, 16),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::Pyramid => {
            let id = commands
                .spawn((
                    Name::new("Pyramid"),
                    crate::brush::Brush::pyramid(0.5, 0.5, 0.5),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            commands.queue(apply_last_material(id));
            id
        }
        EntityTemplate::FogVolume => commands
            .spawn((
                Name::new("Fog Volume"),
                bevy::light::FogVolume::default(),
                SceneFogVolume,
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::AnimationPlayer => commands
            .spawn((
                Name::new("Animation Player"),
                SceneAnimationPlayer,
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
        EntityTemplate::AudioSource => commands
            .spawn((
                Name::new("Audio Source"),
                SceneAudioSource,
                Transform::default(),
                Visibility::default(),
            ))
            .id(),
    };

    selection.select_single(commands, entity);
    entity
}

/// Returns a command that applies the last-used material to all faces of a brush entity.
fn apply_last_material(entity: Entity) -> impl FnOnce(&mut World) {
    move |world: &mut World| {
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        if let Some(mat) = last_mat
            && let Some(mut brush) = world.get_mut::<crate::brush::Brush>(entity)
        {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }
    }
}

/// Spawn a template into the live world and register it in the scene document.
/// In an open 2D world scene it joins that scene, by the rule `sprite.add`
/// places a sprite by.
pub fn spawn_template_in_document(world: &mut World, template: EntityTemplate) -> Entity {
    let parent = crate::sprite_ops::scene_2d_parent(world, None);
    spawn_template_under(world, template, parent)
}

/// `spawn_template_in_document` under a `parent` chosen beforehand, so a redo
/// puts the entity back where the first spawn did.
pub(crate) fn spawn_template_under(
    world: &mut World,
    template: EntityTemplate,
    parent: Option<Entity>,
) -> Entity {
    spawn_shaped_under(world, template, parent, |_| {})
}

/// `spawn_template_under`, with `shape` editing the new entity before it is
/// parented and written to the document.
pub(crate) fn spawn_shaped_under(
    world: &mut World,
    template: EntityTemplate,
    parent: Option<Entity>,
    shape: impl FnOnce(&mut EntityWorldMut),
) -> Entity {
    spawn_selected_under(
        world,
        parent,
        |commands, selection| create_entity(commands, template, selection),
        shape,
    )
}

/// A brush node named "Brush" holding `brush`, spawned and selected the way
/// [`spawn_shaped_under`] spawns a template, without a template's own shape.
pub(crate) fn spawn_brush_under(
    world: &mut World,
    brush: crate::brush::Brush,
    parent: Option<Entity>,
    shape: impl FnOnce(&mut EntityWorldMut),
) -> Entity {
    spawn_selected_under(
        world,
        parent,
        move |commands, selection| {
            let entity = commands
                .spawn((
                    Name::new("Brush"),
                    brush,
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(commands, entity);
            entity
        },
        shape,
    )
}

/// Spawn through `create`, select it, let `shape` edit it, then parent it
/// under `parent` and write it to the document.
pub(crate) fn spawn_selected_under(
    world: &mut World,
    parent: Option<Entity>,
    create: impl FnOnce(&mut Commands, &mut Selection) -> Entity,
    shape: impl FnOnce(&mut EntityWorldMut),
) -> Entity {
    let parent = parent.map(|parent| crate::scene_nodes::live_entity(world, parent));
    let mut system_state: SystemState<(Commands, ResMut<Selection>)> = SystemState::new(world);
    let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
        return Entity::PLACEHOLDER;
    };
    let entity = create(&mut commands, &mut selection);
    system_state.apply(world);
    shape(&mut world.entity_mut(entity));
    if let Some(parent) = parent {
        world.entity_mut(entity).insert(ChildOf(parent));
    }
    crate::scene_io::register_entity_in_ast(world, entity);
    if world.get::<crate::brush::Brush>(entity).is_some() {
        crate::brush::apply_brush_defaults(world, entity);
    }
    entity
}

/// Give the world a scene document to register entities in, if it has none.
/// `register_entity_in_ast` returns silently without one.
pub(crate) fn ensure_scene_document(world: &mut World) {
    if !world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
        world.insert_resource(jackdaw_bsn::SceneBsnAst::default());
    }
}

/// Seed an empty live 3D document with a directional light. A UI document has
/// nothing to light.
pub(crate) fn seed_new_scene_defaults(world: &mut World) {
    ensure_scene_document(world);
    spawn_template_in_document(world, EntityTemplate::DirectionalLight);
}

/// What [`seed_2d_scene_root`] names the root it makes. Space-free, so an
/// operator clause can address it as `name=Scene2d`.
pub const SCENE_2D_ROOT_NAME: &str = "Scene2d";

/// Seed the root a new 2D scene starts from: one marked, transformed node
/// sprites are parented to. The marker is reflected, so a reopened document is
/// recognised as 2D again.
pub fn seed_2d_scene_root(world: &mut World) -> Entity {
    ensure_scene_document(world);
    let root = world
        .spawn((
            Name::new(SCENE_2D_ROOT_NAME),
            jackdaw_scene_types::Scene2dRoot,
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, root);
    crate::selection::select_only(world, root);
    root
}

/// World-access version of `create_entity`. Used from menu actions and other deferred contexts.
/// Pushes a `SpawnEntity` command so the addition can be undone.
pub fn create_entity_in_world(world: &mut World, template: EntityTemplate) {
    let label = format!("Add {}", template.label());
    let parent = crate::sprite_ops::scene_2d_parent(world, None);
    let spawn_fn =
        move |world: &mut World| -> Entity { spawn_template_under(world, template, parent) };

    let mut cmd: Box<dyn EditorCommand> =
        Box::new(crate::commands::SpawnEntity::new(spawn_fn, label));
    cmd.execute(world);
    world.resource_mut::<CommandHistory>().push_executed(cmd);
}

pub fn spawn_gltf(
    commands: &mut Commands,
    path: &str,
    position: Vec3,
    selection: &mut Selection,
) -> Entity {
    let file_name = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "GLTF Model".to_string());
    let scene_index = 0;
    // Store the asset-relative path, not the browser's absolute one: the
    // runtime and other machines cannot strip this project's assets prefix,
    // and an unapproved absolute path is refused outright by the asset server.
    let asset_path = to_asset_path(path);
    let entity = commands
        .spawn((
            Name::new(file_name),
            GltfSource {
                path: asset_path,
                scene_index,
            },
            Transform::from_translation(position),
        ))
        .id();
    selection.select_single(commands, entity);
    entity
}

fn spawn_gltf_in_world(world: &mut World, path: &str, position: Vec3) {
    let mut system_state: SystemState<(Commands, ResMut<Selection>)> = SystemState::new(world);
    let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
        return;
    };
    let entity = spawn_gltf(&mut commands, path, position, &mut selection);
    system_state.apply(world);
    crate::scene_io::register_entity_in_ast(world, entity);
}

pub fn delete_selected(world: &mut World) {
    let entities: Vec<Entity> = world.resource::<Selection>().entities.clone();
    delete_entities(world, &entities);
}

/// Delete these entities as one history entry, dropping each from the
/// selection.
pub fn delete_entities(world: &mut World, entities: &[Entity]) {
    if entities.is_empty() {
        return;
    }

    let targets: EntityHashSet = entities.iter().copied().collect();
    let mut despawns = Vec::new();
    for &entity in entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        if world.get::<EditorEntity>(entity).is_some() {
            continue;
        }
        if has_ancestor_among(world, entity, &targets) {
            continue;
        }
        despawns.push(DespawnEntity::from_world(world, entity));
    }

    // Drop Selected and Selection before despawn so neither holds ids
    // of entities that are about to disappear.
    for &entity in entities {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }
    let mut selection = world.resource_mut::<Selection>();
    selection.entities.retain(|held| !entities.contains(held));

    if despawns.is_empty() {
        return;
    }
    let mut delete = DeleteEntities { despawns };
    delete.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(delete));
}

/// Whether one of `entity`'s ancestors is in `among`, and so takes `entity`
/// down and brings it back with its own subtree.
fn has_ancestor_among(world: &World, entity: Entity, among: &EntityHashSet) -> bool {
    let mut current = entity;
    while let Some(parent) = world.get::<ChildOf>(current).map(ChildOf::parent) {
        if among.contains(&parent) {
            return true;
        }
        current = parent;
    }
    false
}

/// A delete of several entities as one history entry. Undo puts them back
/// and selects them again.
struct DeleteEntities {
    despawns: Vec<DespawnEntity>,
}

impl EditorCommand for DeleteEntities {
    fn execute(&mut self, world: &mut World) {
        for despawn in &mut self.despawns {
            despawn.execute(world);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for despawn in self.despawns.iter_mut().rev() {
            despawn.undo(world);
        }
        let restored: Vec<Entity> = self
            .despawns
            .iter()
            .map(|despawn| despawn.entity)
            .filter(|&entity| world.get_entity(entity).is_ok())
            .collect();
        crate::selection::select_many(world, &restored);
    }

    fn description(&self) -> &str {
        "Delete entities"
    }
}

/// How many siblings the list `parent` names holds. `None` is the scene's own
/// root list.
fn sibling_count(world: &World, parent: Option<Entity>) -> usize {
    match parent {
        Some(parent) => world.get::<Children>(parent).map_or(0, Children::len),
        None => world
            .get_resource::<jackdaw_bsn::SceneBsnAst>()
            .map_or(0, |ast| ast.roots.len()),
    }
}

/// Move every selected entity one slot along its own sibling list, earlier for
/// `delta` of -1 and later for 1, as one history entry. Each list keeps a
/// frontier -- the nearest slot still free -- so a selection reaching the end
/// packs against it keeping its own order.
pub(crate) fn move_selected_siblings(world: &mut World, delta: isize) {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let mut located: Vec<(Entity, HierarchyLocation)> = selected
        .into_iter()
        .filter(|&entity| {
            world.get_entity(entity).is_ok() && world.get::<EditorEntity>(entity).is_none()
        })
        .map(|entity| {
            let location = HierarchyLocation::from_world(world, entity);
            (entity, location)
        })
        .collect();
    // Nearest the destination first, so two entities cannot swap past each other.
    located.sort_by_key(|(_, location)| location.index);
    if delta > 0 {
        located.reverse();
    }

    let mut moves: Vec<Box<dyn EditorCommand>> = Vec::new();
    let mut lists: Vec<Option<Entity>> = Vec::new();
    // The nearest free slot to the destination, per sibling list.
    let mut frontiers: Vec<(Option<Entity>, isize)> = Vec::new();
    for (entity, _) in located {
        let old = HierarchyLocation::from_world(world, entity);
        let here = old.index as isize;
        let end = if delta > 0 {
            sibling_count(world, old.parent) as isize - 1
        } else {
            0
        };
        let frontier = frontiers
            .iter()
            .find(|(parent, _)| *parent == old.parent)
            .map_or(end, |(_, at)| *at);
        let target = if delta > 0 {
            (here + delta).min(frontier)
        } else {
            (here + delta).max(frontier)
        };
        let blocked = if delta > 0 {
            target <= here
        } else {
            target >= here
        };
        let settled = if blocked { here } else { target };
        // Whatever the entity settled on, the next one cannot have it.
        let next = if delta > 0 { settled - 1 } else { settled + 1 };
        match frontiers
            .iter_mut()
            .find(|(parent, _)| *parent == old.parent)
        {
            Some((_, at)) => *at = next,
            None => frontiers.push((old.parent, next)),
        }
        if blocked {
            continue;
        }
        let index = settled as usize;
        let mut command = MoveEntity::new(
            world,
            entity,
            HierarchyLocation {
                parent: old.parent,
                index,
            },
        );
        command.execute(world);
        moves.push(Box::new(command));
        if !lists.contains(&old.parent) {
            lists.push(old.parent);
        }
    }

    let entry: Box<dyn EditorCommand> = match moves.len() {
        0 => return,
        1 => moves.pop().expect("one move"),
        _ => Box::new(crate::commands::CommandGroup {
            commands: moves,
            label: "Reorder entities".to_string(),
        }),
    };
    world.resource_mut::<CommandHistory>().push_executed(entry);
    for list in lists {
        crate::hierarchy::sync_outliner_row_order(world, list);
    }
}

/// Duplicate selected entities by grafting authored AST subtrees into the live
/// document and spawning from those patches.
pub fn duplicate_selected(world: &mut World) {
    let entities = world.resource::<Selection>().entities.clone();
    duplicate_entities(world, &entities, &DuplicateRun::default());
}

/// How a duplicate lays its copies out: copy `k` of `count` sits `k * offset`
/// from its source and is turned by `step_rotation` `k` times, in the space
/// its translation is written in. `parent` adopts every copy, keeping the
/// source's local values.
#[derive(Clone, Copy)]
pub struct DuplicateRun {
    pub count: usize,
    pub offset: Vec3,
    pub step_rotation: Quat,
    pub parent: Option<Entity>,
}

impl Default for DuplicateRun {
    fn default() -> Self {
        Self {
            count: 1,
            offset: Vec3::ZERO,
            step_rotation: Quat::IDENTITY,
            parent: None,
        }
    }
}

impl DuplicateRun {
    fn moves(&self) -> bool {
        self.offset != Vec3::ZERO || self.step_rotation != Quat::IDENTITY
    }
}

/// The type path a node's authored `Transform` patch carries.
const TRANSFORM_TYPE_PATH: &str = "bevy_transform::components::transform::Transform";

/// Whether the document authors a `Transform` on `entity`.
pub(crate) fn authors_transform(world: &World, entity: Entity) -> bool {
    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    ast.ast_for(entity).is_some_and(|node| {
        ast.find_patch_by_type_path(node, TRANSFORM_TYPE_PATH)
            .is_some()
    })
}

/// Duplicate `entities` as `run` lays them out, select the copies and return
/// them, every first copy before every second one. A call that copies
/// nothing leaves the selection as it was.
pub fn duplicate_entities(
    world: &mut World,
    entities: &[Entity],
    run: &DuplicateRun,
) -> Vec<Entity> {
    // Each source is copied out of the document once, before anything is
    // grafted, so a copy landing inside a source is not copied again.
    let sources: Vec<(jackdaw_bsn::SceneBsnAst, Option<Entity>)> = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        let adopter = run.parent.and_then(|parent| ast.ast_for(parent));
        entities
            .iter()
            .copied()
            .filter(|&entity| {
                world.get_entity(entity).is_ok() && world.get::<EditorEntity>(entity).is_none()
            })
            .filter_map(|entity| {
                let Some(src_node) = ast.ast_for(entity) else {
                    warn!("Duplicate: entity {entity:?} has no document node");
                    return None;
                };
                let parent_ast = match run.parent {
                    Some(_) => adopter,
                    None => ast.find_ast_parent_of(src_node),
                };
                let mut copy = jackdaw_bsn::SceneBsnAst::default();
                jackdaw_bsn::clone_subtree_into(&mut copy, ast, src_node, None);
                Some((copy, parent_ast))
            })
            .collect()
    };
    if sources.is_empty() {
        return Vec::new();
    }

    let mut taken = scene_entity_names(world);
    let mut new_entities = Vec::new();
    let mut turn = Quat::IDENTITY;
    let mut sources = sources;
    for step in 1..=run.count {
        turn = run.step_rotation * turn;
        for (copy, parent_ast) in &mut sources {
            mint_scene_node_ids(world, copy);
            claim_entity_names(world, copy, &mut taken);
            let Some(&root) = graft_and_spawn(world, copy, *parent_ast).first() else {
                continue;
            };
            // The copy spawned from the authored patch, so its transform is the
            // source's authored one.
            if run.moves()
                && let Some(&authored) = world.get::<Transform>(root)
            {
                let placed = Transform {
                    translation: authored.translation + run.offset * step as f32,
                    rotation: turn * authored.rotation,
                    scale: authored.scale,
                };
                world.entity_mut(root).insert(placed);
                crate::commands::sync_component_to_ast(world, root, TRANSFORM_TYPE_PATH, &placed);
            }
            crate::commands::SpawnedEntities::record(world, root);
            new_entities.push(root);
        }
    }

    if !new_entities.is_empty() {
        crate::selection::select_many(world, &new_entities);
    }
    new_entities
}

/// Mint fresh ids and unique root names on an authored subtree before it is
/// grafted/spawned.
fn prepare_authored_subtree_for_spawn(world: &mut World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    mint_scene_node_ids(world, ast);
    assign_unique_entity_names(world, ast);
}

/// `SceneNodeIds` on entity roots of `ast`.
fn entity_root_scene_node_ids(
    world: &World,
    ast: &jackdaw_bsn::SceneBsnAst,
) -> Vec<jackdaw_scene_types::SceneNodeId> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    jackdaw_bsn::entity_roots(ast, &reg)
        .into_iter()
        .filter_map(|root| ast.stable_id_of(root).map(jackdaw_scene_types::SceneNodeId))
        .collect()
}

/// Give every named node in `ast`, roots and descendants alike, a `#Name` no
/// live scene entity and no other node in the batch has claimed, since a name
/// is what an operator clause addresses an entity by.
fn assign_unique_entity_names(world: &mut World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    let mut taken = scene_entity_names(world);
    claim_entity_names(world, ast, &mut taken);
}

/// Rename every named node in `ast` to a name `taken` does not hold, and
/// claim it there.
fn claim_entity_names(
    world: &World,
    ast: &mut jackdaw_bsn::SceneBsnAst,
    taken: &mut std::collections::HashSet<String>,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_roots: std::collections::HashSet<Entity> = {
        let reg = registry.read();
        jackdaw_bsn::asset_roots(ast, &reg).into_iter().collect()
    };
    for node in walk_entity_nodes(ast) {
        if asset_roots.contains(&node) {
            // An asset entry's name is the reference its components carry.
            continue;
        }
        let Some(name) = ast.get_name(node).map(str::to_owned) else {
            continue;
        };
        if let Some(free) = claim_free_name(taken, &name) {
            crate::commands::set_name_patch(ast, node, Some(&free));
        }
    }
}

/// Every node in `ast`, parents before children, each visited once -- a
/// clipboard document's `Children` lists may form a cycle.
fn walk_entity_nodes(ast: &jackdaw_bsn::SceneBsnAst) -> Vec<Entity> {
    let mut queue: std::collections::VecDeque<Entity> = ast.roots.iter().copied().collect();
    let mut seen: std::collections::HashSet<Entity> = queue.iter().copied().collect();
    let mut nodes = Vec::new();
    while let Some(node) = queue.pop_front() {
        nodes.push(node);
        for child in ast.get_children_ast(node) {
            if seen.insert(child) {
                queue.push_back(child);
            }
        }
    }
    nodes
}

/// Every `Name` on a scene entity, editor chrome excluded.
#[doc(hidden)]
pub fn scene_entity_names(world: &mut World) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let mut query = world.query_filtered::<&Name, Without<EditorEntity>>();
    for existing in query.iter(world) {
        names.insert(existing.as_str().to_owned());
    }
    names
}

/// Reserve `name` in `taken`, and say what it had to become: `None` when it was
/// free, otherwise the next free `BaseN` suffix, also claimed. The suffix
/// carries no space, which an operator clause could not address.
#[doc(hidden)]
pub fn claim_free_name(
    taken: &mut std::collections::HashSet<String>,
    name: &str,
) -> Option<String> {
    if taken.insert(name.to_owned()) {
        return None;
    }

    // `Button2` and `Button 2` renumber from `Button`, not from themselves.
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = if trimmed.is_empty() {
        name.to_owned()
    } else {
        trimmed.trim_end().to_owned()
    };

    let mut max_num = 0u32;
    for existing in taken.iter() {
        if existing == &base {
            max_num = max_num.max(1);
        } else if let Some(rest) = existing.strip_prefix(base.as_str())
            && let Ok(n) = rest.trim_start().parse::<u32>()
        {
            max_num = max_num.max(n);
        }
    }
    let free = format!("{base}{}", max_num + 1);
    taken.insert(free.clone());
    Some(free)
}

/// Snap a vector to the nearest cardinal world axis (+/-X, +/-Y, +/-Z).
/// Returns a signed unit vector along the axis with the largest absolute component.
fn snap_to_nearest_axis(v: Vec3) -> Vec3 {
    let abs = v.abs();
    if abs.x >= abs.y && abs.x >= abs.z {
        Vec3::new(v.x.signum(), 0.0, 0.0)
    } else if abs.y >= abs.x && abs.y >= abs.z {
        Vec3::new(0.0, v.y.signum(), 0.0)
    } else {
        Vec3::new(0.0, 0.0, v.z.signum())
    }
}

/// Derive TrenchBroom-style rotation axes from the camera transform.
///
/// - **Yaw** (left/right arrows): always world Y. Vertical rotation is always intuitive.
/// - **Roll** (up/down arrows): camera forward projected to horizontal, snapped to nearest
///   world axis, then negated. This is the axis you're "looking along".
/// - **Pitch** (PageUp/PageDown): camera right snapped to nearest world axis. If it
///   collides with the roll axis, use the cross product with Y instead.
pub(crate) fn camera_snapped_rotation_axes(gt: &GlobalTransform) -> (Vec3, Vec3, Vec3) {
    let yaw_axis = Vec3::Y;

    // Forward projected onto the horizontal plane, snapped to nearest axis
    let fwd = gt.forward().as_vec3();
    let fwd_horiz = Vec3::new(fwd.x, 0.0, fwd.z);
    let roll_axis = if fwd_horiz.length_squared() > 1e-6 {
        -snap_to_nearest_axis(fwd_horiz)
    } else {
        // Looking straight down/up, use camera up projected horizontally instead.
        let up = gt.up().as_vec3();
        let up_horiz = Vec3::new(up.x, 0.0, up.z);
        if up_horiz.length_squared() > 1e-6 {
            snap_to_nearest_axis(up_horiz)
        } else {
            Vec3::NEG_Z
        }
    };

    // Right snapped to nearest axis, with deduplication against roll
    let right = gt.right().as_vec3();
    let mut pitch_axis = snap_to_nearest_axis(right);
    if pitch_axis.abs() == roll_axis.abs() {
        // Collision, derive perpendicular horizontal axis.
        pitch_axis = snap_to_nearest_axis(yaw_axis.cross(roll_axis));
    }

    (yaw_axis, roll_axis, pitch_axis)
}

pub(crate) enum TransformReset {
    Position,
    Rotation,
    Scale,
}

pub(crate) fn reset_transform_selected(world: &mut World, reset: TransformReset) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        let Some(&old_transform) = world.get::<Transform>(entity) else {
            continue;
        };

        let new_transform = match reset {
            TransformReset::Position => Transform {
                translation: Vec3::ZERO,
                ..old_transform
            },
            TransformReset::Rotation => Transform {
                rotation: Quat::IDENTITY,
                ..old_transform
            },
            TransformReset::Scale => Transform {
                scale: Vec3::ONE,
                ..old_transform
            },
        };

        if old_transform == new_transform {
            continue;
        }

        let mut cmd = crate::commands::SetTransform {
            entity,
            old_transform,
            new_transform,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let label = match reset {
            TransformReset::Position => "Reset position",
            TransformReset::Rotation => "Reset rotation",
            TransformReset::Scale => "Reset scale",
        };
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: label.to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

pub(crate) fn nudge_selected(world: &mut World, offset: Vec3) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        let Some(&old_transform) = world.get::<Transform>(entity) else {
            continue;
        };

        let new_transform = Transform {
            translation: old_transform.translation + offset,
            ..old_transform
        };

        let mut cmd = crate::commands::SetTransform {
            entity,
            old_transform,
            new_transform,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Nudge".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

pub(crate) fn rotate_selected(world: &mut World, rotation: Quat) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    for &entity in &entities {
        if world.get_entity(entity).is_err() {
            continue;
        }
        let Some(&old_transform) = world.get::<Transform>(entity) else {
            continue;
        };

        let new_transform = Transform {
            rotation: rotation * old_transform.rotation,
            ..old_transform
        };

        let mut cmd = crate::commands::SetTransform {
            entity,
            old_transform,
            new_transform,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Rotate 90\u{00b0}".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

/// The selection as BSN text -- the selected subtrees plus embedded asset
/// entries, in the shape a saved scene uses. `None` when nothing selected is in
/// the document.
fn selection_as_bsn(world: &mut World) -> Option<String> {
    let selected: Vec<Entity> = world.resource::<Selection>().entities.clone();
    if selected.is_empty() {
        return None;
    }
    let nodes: Vec<Entity> = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        selected.iter().filter_map(|&e| ast.ast_for(e)).collect()
    };
    if nodes.is_empty() {
        warn!("Copy: no selected entities have document nodes");
        return None;
    }
    let parent_path = world
        .resource::<crate::scene_io::SceneFilePath>()
        .path
        .as_ref()
        .and_then(|p| Path::new(p).parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let text = crate::scene_io::emit_bsn_entities_with_inline_assets(world, &parent_path, &nodes);
    if text.trim().is_empty() {
        warn!("Copy: selected entities emitted no BSN text");
        return None;
    }
    Some(text)
}

/// Put `text` on the OS clipboard, and on the editor's own [`EntityClipboard`]
/// either way, so a run with no OS clipboard still copies and pastes.
fn write_clipboard(world: &mut World, text: String) {
    if let Some(mut system) = world.get_resource_mut::<SystemClipboard>() {
        system.last_emitted.clone_from(&text);
        if let Some(mut clipboard) = world.get_resource_mut::<Clipboard>()
            && let Err(error) = clipboard.set_text(text.as_str())
        {
            warn!("Copy: system clipboard failed ({error}), keeping the editor's own copy");
        }
    }
    world.resource_mut::<EntityClipboard>().text = text;
}

/// The OS clipboard's text, or `None` when entity paste does not use the OS
/// clipboard or it holds no text.
fn os_clipboard_text(world: &mut World) -> Option<String> {
    world.get_resource::<SystemClipboard>()?;
    world
        .get_resource_mut::<Clipboard>()?
        .fetch_text()
        .poll_result()?
        .ok()
}

/// The largest clipboard payload a paste will read. Anything past it is refused
/// before it is parsed.
const MAX_CLIPBOARD_BYTES: usize = 2 * 1024 * 1024;

/// What a refused paste says.
const NOT_ENTITIES: &str = "the clipboard does not hold entities";

/// What a paste refused for its size says, with the cap in the sentence.
fn too_large_to_paste(bytes: usize) -> String {
    format!("the clipboard holds {bytes} bytes, past the {MAX_CLIPBOARD_BYTES} a paste reads")
}

/// Whether `text` is an entity document this editor can paste: within
/// `MAX_CLIPBOARD_BYTES`, parsing, and yielding an entity root carrying a
/// registered component. Prose parses as a bare `BsnPatch::Type`, so the parser
/// saying yes is not on its own an answer.
fn is_entity_document(world: &World, text: &str) -> bool {
    if text.len() > MAX_CLIPBOARD_BYTES {
        warn!(
            "Paste: the clipboard holds {} bytes, past the {MAX_CLIPBOARD_BYTES} a paste reads",
            text.len()
        );
        return false;
    }
    let Ok(ast) = jackdaw_bsn::parse_bsn_text(text) else {
        return false;
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    jackdaw_bsn::entity_roots(&ast, &registry)
        .into_iter()
        .any(|root| {
            let Some(patches) = ast.get_patches(root) else {
                return false;
            };
            patches.0.iter().any(|&pe| {
                ast.get_patch(pe)
                    .and_then(jackdaw_bsn::patch_type_path)
                    .is_some_and(|type_path| registry.get_with_type_path(type_path).is_some())
            })
        })
}

/// The scene text a paste should spawn from, or `None` when the clipboard holds
/// no entities. OS text this editor last emitted is answered from the
/// [`EntityClipboard`] mirror; other text is accepted only when it reads as an
/// entity document, and is otherwise refused rather than falling back.
fn clipboard_entities(world: &mut World) -> Option<String> {
    let own = world
        .get_resource::<EntityClipboard>()
        .map(|clipboard| clipboard.text.clone())
        .filter(|text| !text.trim().is_empty());

    let os_text = os_clipboard_text(world)
        .zip(world.get_resource::<SystemClipboard>())
        .map(|(text, system)| (text, system.last_emitted.clone()))
        .filter(|(text, _)| !text.trim().is_empty());

    choose_clipboard_text(world, own, os_text)
}

/// `clipboard_entities` with both clipboards already read, so the choice can be
/// exercised without an OS clipboard. `os` pairs its text with our last
/// emission.
fn choose_clipboard_text(
    world: &World,
    own: Option<String>,
    os: Option<(String, String)>,
) -> Option<String> {
    let Some((text, last_emitted)) = os else {
        // No readable OS clipboard: the mirror is the whole answer.
        return own;
    };
    if text == last_emitted {
        return own.or(Some(text));
    }
    is_entity_document(world, &text).then_some(text)
}

/// Copy selected entities to the clipboard as BSN text.
fn copy_components(world: &mut World) {
    let Some(text) = selection_as_bsn(world) else {
        return;
    };
    write_clipboard(world, text);
}

/// Undo entry for a paste: undo despawns each pasted root by its `SceneNodeId`,
/// redo re-spawns from the remapped clipboard text carrying those ids.
struct PasteEntitiesCommand {
    /// `SceneNodeIds` assigned to pasted entity roots at first paste.
    spawned_node_ids: Vec<jackdaw_scene_types::SceneNodeId>,
    /// Clipboard BSN with fresh ids and unique names already written in, so
    /// redo re-spawns the same authored patches.
    remapped_text: String,
    /// Where the paste landed, so a redo puts it back in the same place.
    target: PasteTarget,
    label: String,
}

impl crate::commands::EditorCommand for PasteEntitiesCommand {
    /// Only a redo reaches this: `push_executed` does not run what it is given.
    fn execute(&mut self, world: &mut World) {
        let spawned = spawn_bsn_clipboard(world, &self.remapped_text, self.target);
        select_entities(world, &spawned);
        info!("Redo: re-pasted {} entities", spawned.len());
    }

    fn undo(&mut self, world: &mut World) {
        let id_to_entity: std::collections::HashMap<_, _> = world
            .query::<(Entity, &jackdaw_scene_types::SceneNodeId)>()
            .iter(world)
            .map(|(entity, node_id)| (*node_id, entity))
            .collect();

        let mut to_despawn: Vec<Entity> = Vec::new();
        for node_id in &self.spawned_node_ids {
            if let Some(&entity) = id_to_entity.get(node_id) {
                to_despawn.push(entity);
            }
        }

        crate::commands::deselect_entities(world, &to_despawn);
        for e in to_despawn {
            crate::commands::despawn_scene_entity(world, e);
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Spawn a new brush by cloning `source`'s authored document node and
/// replacing its geometry. Parent, physics, modifiers, and extra
/// components follow the original. Children are not copied.
pub(crate) fn spawn_cloned_brush_with_geometry(
    world: &mut World,
    source: Entity,
    brush: crate::brush::Brush,
    transform: Transform,
) -> Option<Entity> {
    let (parent_ast, patches) = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        let node = ast.ast_for(source)?;
        (
            ast.find_ast_parent_of(node),
            ast.cloned_component_patches(node),
        )
    };
    let mut temp = jackdaw_bsn::SceneBsnAst::default();
    let root = temp.create_entity_node(patches);
    temp.add_to_roots(root);
    mint_scene_node_ids(world, &mut temp);

    let entity = *graft_and_spawn(world, &temp, parent_ast).first()?;
    world.entity_mut(entity).insert((brush.clone(), transform));
    crate::brush::sync_brush_to_ast(world, entity, &brush);
    crate::commands::sync_component_to_ast(world, entity, Transform::type_path(), &transform);
    Some(entity)
}

/// Graft entity roots from `source` into the live document and spawn
/// them under `parent_ast` (`None` = scene roots).
fn graft_and_spawn(
    world: &mut World,
    source: &jackdaw_bsn::SceneBsnAst,
    parent_ast: Option<Entity>,
) -> Vec<Entity> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let entity_roots = {
        let reg = registry.read();
        jackdaw_bsn::entity_roots(source, &reg)
    };

    let ecs_parent = parent_ast.and_then(|parent| {
        world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ecs_for_ast(parent)
    });

    let mut grafted_roots = Vec::new();
    {
        let mut live = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        for src_root in entity_roots {
            let new_root = jackdaw_bsn::clone_subtree_into(&mut live, source, src_root, parent_ast);
            grafted_roots.push(new_root);
        }
    }

    let mut spawned_roots = Vec::new();
    let mut all_spawned = Vec::new();
    for &ast_root in &grafted_roots {
        let before = all_spawned.len();
        jackdaw_bsn::spawn_ast_node(world, ast_root, ecs_parent, &mut all_spawned);
        if let Some(&ecs_root) = all_spawned.get(before) {
            spawned_roots.push(ecs_root);
        }
    }
    jackdaw_bsn::apply_dirty_ast_patches(world);
    spawned_roots
}

/// Parse clipboard BSN text and graft it into the live scene document.
fn spawn_bsn_clipboard(world: &mut World, text: &str, target: PasteTarget) -> Vec<Entity> {
    let parsed = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(e) => {
            warn!("Paste: failed to parse clipboard BSN: {e}");
            return Vec::new();
        }
    };
    graft_and_spawn_at(world, &parsed, target)
}

/// Where a paste lands, named by `SceneNodeId` so a redo can resolve it after an
/// undo respawned the parent.
#[derive(Clone, Copy, Default)]
struct PasteTarget {
    /// `None` is the scene's own root list.
    parent: Option<jackdaw_scene_types::SceneNodeId>,
    /// Sibling index the first pasted root takes; the rest follow it.
    index: usize,
}

impl PasteTarget {
    /// The live location, or the end of the scene root list when the named
    /// parent is gone.
    fn resolve(&self, world: &mut World) -> HierarchyLocation {
        let Some(wanted) = self.parent else {
            return HierarchyLocation {
                parent: None,
                index: self.index,
            };
        };
        let parent = crate::scene_nodes::unit_holder(world, wanted);
        HierarchyLocation {
            parent,
            index: if parent.is_some() {
                self.index
            } else {
                usize::MAX
            },
        }
    }
}

/// Sibling straight after the primary selection, which is where a paste goes.
/// With no usable selection, the end of the UI scene's root or of the scene's
/// own root list.
fn paste_target(world: &mut World) -> PasteTarget {
    let primary = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    if let Some(primary) = primary
        && world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ast_for(primary)
            .is_some()
    {
        let location = HierarchyLocation::from_world(world, primary);
        return PasteTarget {
            parent: location.parent.and_then(|parent| {
                world
                    .get::<jackdaw_scene_types::SceneNodeId>(parent)
                    .copied()
            }),
            index: location.index + 1,
        };
    }
    let root = crate::ui_palette::ui_scene_root(world);
    PasteTarget {
        parent: root.and_then(|root| world.get::<jackdaw_scene_types::SceneNodeId>(root).copied()),
        index: usize::MAX,
    }
}

/// `graft_and_spawn` landing the roots at `target` rather than at the end of the
/// scene's root list.
fn graft_and_spawn_at(
    world: &mut World,
    source: &jackdaw_bsn::SceneBsnAst,
    target: PasteTarget,
) -> Vec<Entity> {
    let location = target.resolve(world);
    let parent_ast = location
        .parent
        .and_then(|parent| world.resource::<jackdaw_bsn::SceneBsnAst>().ast_for(parent));
    let spawned = graft_and_spawn(world, source, parent_ast);
    if location.index != usize::MAX {
        for (offset, &root) in spawned.iter().enumerate() {
            // No transform has propagated for the new roots yet, so reading a
            // world position would author an identity `Transform`.
            crate::commands::place_entity(
                world,
                root,
                HierarchyLocation {
                    parent: location.parent,
                    index: location.index + offset,
                },
                crate::commands::WorldTransform::Unplaced,
            );
        }
        crate::hierarchy::sync_outliner_row_order(world, location.parent);
    }
    spawned
}

/// Remove the `PrefabEntityId` from every node in `ast` that no instance root in
/// `ast` sits above. A copy of one node from inside an instance has no file to
/// match that id against, and would read as an override of the original.
fn drop_orphaned_prefab_entity_ids(ast: &mut jackdaw_bsn::SceneBsnAst) {
    use crate::prefab::resolver_bsn::{ISA_TYPE, PREFAB_ENTITY_ID_TYPE};

    let mut stack: Vec<(Entity, bool)> = ast.roots.iter().map(|&root| (root, false)).collect();
    let mut seen: std::collections::HashSet<Entity> = ast.roots.iter().copied().collect();
    while let Some((node, below)) = stack.pop() {
        let instance_root = ast.find_patch_by_type_path(node, ISA_TYPE).is_some();
        if !below && !instance_root {
            ast.remove_component_patch(node, PREFAB_ENTITY_ID_TYPE);
        }
        let children_below = below || instance_root;
        for child in ast.get_children_ast(node) {
            if seen.insert(child) {
                stack.push((child, children_below));
            }
        }
    }
}

/// Ensure every entity node in `ast` carries a fresh `SceneNodeId` patch so
/// spawn/apply installs unique ids.
fn mint_scene_node_ids(world: &World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    drop_orphaned_prefab_entity_ids(ast);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_roots: std::collections::HashSet<Entity> = {
        let reg = registry.read();
        jackdaw_bsn::asset_roots(ast, &reg).into_iter().collect()
    };

    // A visited set, not merely a stack: a clipboard document's `Children` lists
    // may form a cycle.
    let mut stack: Vec<Entity> = ast.roots.clone();
    let mut seen: std::collections::HashSet<Entity> = stack.iter().copied().collect();
    let mut nodes = Vec::new();
    while let Some(node) = stack.pop() {
        nodes.push(node);
        for child in ast.get_children_ast(node) {
            if seen.insert(child) {
                stack.push(child);
            }
        }
    }
    for node in nodes {
        if asset_roots.contains(&node) {
            continue;
        }
        let existing = ast.get_patches(node).and_then(|patches| {
            patches.0.iter().copied().find(|&pe| {
                matches!(
                    ast.get_patch(pe),
                    Some(jackdaw_bsn::BsnPatch::TupleStruct(data))
                        if data.type_path.ends_with("SceneNodeId")
                )
            })
        });
        let fresh = jackdaw_scene_types::SceneNodeId::next();
        let patch = jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
            type_path: jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH.to_string(),
            values: vec![jackdaw_bsn::BsnValue::Int(fresh.0 as i128)],
        });
        if let Some(pe) = existing {
            ast.set_patch(pe, patch);
        } else {
            let pe = ast.world.spawn(patch).id();
            if let Some(patches) = ast.get_patches_mut(node) {
                patches.0.push(pe);
            }
        }
    }
}

/// Whether the open document is a UI scene, which decides what may be pasted.
fn open_scene_is_ui(world: &mut World) -> bool {
    crate::ui_palette::ui_scene_root(world).is_some()
}

/// What kind of scene `ast`'s entity roots belong in. A `Node` patch is what
/// makes a root a UI node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PayloadKind {
    /// Every root is a UI node.
    Ui,
    /// No root is.
    World,
    /// Some are and some are not, so no one scene holds them all.
    Mixed,
}

fn payload_kind(world: &World, ast: &jackdaw_bsn::SceneBsnAst) -> PayloadKind {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let node_type_path = crate::inspector::node_card::node_type_path();
    let roots = jackdaw_bsn::entity_roots(ast, &registry);
    let ui = roots
        .iter()
        .filter(|&&root| ast.find_patch_by_type_path(root, node_type_path).is_some())
        .count();
    // `World` first, so a payload with no entity roots does not read as UI.
    if ui == 0 {
        PayloadKind::World
    } else if ui == roots.len() {
        PayloadKind::Ui
    } else {
        PayloadKind::Mixed
    }
}

/// Paste entities from clipboard scene text at `target`, as one history entry,
/// returning the pasted roots, which become the selection.
fn paste_clipboard_entities(world: &mut World, text: &str, target: PasteTarget) -> Vec<Entity> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let mut parsed = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(e) => {
            warn!("Clipboard text is not valid BSN: {e}");
            crate::status_bar::notify_error(world, NOT_ENTITIES);
            return Vec::new();
        }
    };
    if parsed.roots.is_empty() {
        crate::status_bar::notify_error(world, NOT_ENTITIES);
        return Vec::new();
    }

    // A UI node in a world, or a mesh in a screen, neither draws nor saves right.
    let kind = payload_kind(world, &parsed);
    let scene_is_ui = open_scene_is_ui(world);
    let refusal = match (kind, scene_is_ui) {
        (PayloadKind::Mixed, _) => {
            Some("the clipboard holds both UI nodes and world entities, and no scene holds both")
        }
        (PayloadKind::Ui, false) => {
            Some("the clipboard holds UI nodes, and this is not a UI scene")
        }
        (PayloadKind::World, true) => {
            Some("the clipboard holds world entities, and this is a UI scene")
        }
        _ => None,
    };
    if let Some(message) = refusal {
        crate::status_bar::notify_error(world, message);
        return Vec::new();
    }

    // Assets travel with the subtree, so a pasted image node finds its image.
    let dropped = jackdaw_bsn::adopt_asset_roots(world, &mut parsed);
    if !dropped.is_empty() {
        crate::status_bar::notify_warn(world, format!("pasted without {}", dropped.join(", ")));
    }

    // Fresh ids and names before the graft, so the paste is a second thing in
    // the scene rather than a second reference to the first.
    prepare_authored_subtree_for_spawn(world, &mut parsed);
    let spawned_node_ids = entity_root_scene_node_ids(world, &parsed);
    let remapped_text = jackdaw_bsn::emit_scene(&parsed);

    let spawned = graft_and_spawn_at(world, &parsed, target);
    if spawned.is_empty() {
        return Vec::new();
    }

    select_entities(world, &spawned);
    info!("Pasted {} entities from BSN clipboard", spawned.len());

    let cmd = PasteEntitiesCommand {
        spawned_node_ids,
        remapped_text,
        target,
        label: "Paste entities".to_string(),
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(cmd));
    spawned
}

/// Make `entities` the whole selection.
fn select_entities(world: &mut World, entities: &[Entity]) {
    for &entity in &world.resource::<Selection>().entities.clone() {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }
    world.resource_mut::<Selection>().entities = entities.to_vec();
    for &entity in entities {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(Selected);
        }
    }
}

/// Paste clipboard entities at the end of the scene's root list.
fn paste_components(world: &mut World) {
    paste_clipboard(
        world,
        PasteTarget {
            parent: None,
            index: usize::MAX,
        },
    );
}

/// Paste clipboard entities as the sibling straight after the selection.
pub fn paste_entities_after_selection(world: &mut World) {
    let target = paste_target(world);
    paste_clipboard(world, target);
}

/// Put whatever the clipboard holds into the scene at `target`, entities before
/// images: a clipboard holds both at once, and a screenshot taken between a copy
/// and its paste must not swallow the subtree.
fn paste_clipboard(world: &mut World, target: PasteTarget) {
    if let Some(text) = clipboard_entities(world) {
        paste_clipboard_entities(world, &text, target);
        return;
    }
    if crate::asset_ingest::paste_clipboard_image(world) {
        return;
    }
    // The size cap refuses before the parse, so say which of the two it was.
    let oversized = os_clipboard_text(world)
        .map(|text| text.len())
        .filter(|&bytes| bytes > MAX_CLIPBOARD_BYTES);
    match oversized {
        Some(bytes) => crate::status_bar::notify_error(world, too_large_to_paste(bytes)),
        None => crate::status_bar::notify_error(world, NOT_ENTITIES),
    }
}

/// Copy the selection to the clipboard as BSN text.
pub fn copy_selected_entities(world: &mut World) {
    let Some(text) = selection_as_bsn(world) else {
        return;
    };
    write_clipboard(world, text);
}

/// Copy the selection and then delete it. The copy records nothing, so the
/// delete's entry is the whole cut.
fn cut_selected_entities(world: &mut World) {
    let Some(text) = selection_as_bsn(world) else {
        return;
    };
    write_clipboard(world, text);
    delete_selected(world);
}

/// Flip `Visibility` between hidden and inherited on every selected entity,
/// writing through the document. It pushes no history entry of its own: the
/// dispatcher's before/after snapshot pair is the entry.
fn hide_selected(world: &mut World) {
    let selection = world.resource::<Selection>();
    let entities: Vec<Entity> = selection.entities.clone();

    if entities.is_empty() {
        return;
    }

    for &entity in &entities {
        let current = world
            .get::<Visibility>(entity)
            .copied()
            .unwrap_or(Visibility::Inherited);

        let new_visibility = match current {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };

        let mut cmd = crate::commands::SetBsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: Some(jackdaw_bsn::BsnValue::Type(format!(
                "bevy_camera::visibility::Visibility::{current:?}"
            ))),
            new_value: jackdaw_bsn::BsnValue::Type(format!(
                "bevy_camera::visibility::Visibility::{new_visibility:?}"
            )),
            was_derived: false,
        };
        cmd.execute(world);
    }
}

// FIXME: this breaks down whenever an extension uses `Name`
#[derive(SystemParam, Deref, DerefMut)]
struct SceneEntities<'w, 's> {
    query: Query<
        'w,
        's,
        (Entity, &'static Visibility),
        (With<Name>, Without<EditorEntity>, Without<Node>),
    >,
}

fn unhide_all_entities(world: &mut World, scene_entities: &mut SystemState<SceneEntities>) {
    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    // Only unhide top-level scene entities (with Name), matching hide_unselected logic.
    let hidden: Vec<Entity> = {
        let Ok(entities) = scene_entities.get(world) else {
            return;
        };
        entities
            .iter()
            .filter(|(_, vis)| **vis == Visibility::Hidden)
            .map(|(e, _)| e)
            .collect()
    };

    for entity in hidden {
        let mut cmd = crate::commands::SetBsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: Some(jackdaw_bsn::BsnValue::Type(
                "bevy_camera::visibility::Visibility::Hidden".to_string(),
            )),
            new_value: jackdaw_bsn::BsnValue::Type(
                "bevy_camera::visibility::Visibility::Inherited".to_string(),
            ),
            was_derived: false,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Unhide all".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

fn hide_all_entities(world: &mut World, scene_entities: &mut SystemState<SceneEntities>) {
    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    // Hide all top-level scene entities (same filter as H, applied to everything).
    let to_hide: Vec<(Entity, Visibility)> = {
        let Ok(entities) = scene_entities.get(world) else {
            return;
        };
        entities
            .iter()
            .filter(|(_, vis)| **vis != Visibility::Hidden)
            .map(|(e, vis)| (e, *vis))
            .collect()
    };

    for (entity, current) in to_hide {
        let mut cmd = crate::commands::SetBsnField {
            entity,
            type_path: "bevy_camera::visibility::Visibility".to_string(),
            field_path: String::new(),
            old_value: Some(jackdaw_bsn::BsnValue::Type(format!(
                "bevy_camera::visibility::Visibility::{current:?}"
            ))),
            new_value: jackdaw_bsn::BsnValue::Type(
                "bevy_camera::visibility::Visibility::Hidden".to_string(),
            ),
            was_derived: false,
        };
        cmd.execute(world);
        cmds.push(Box::new(cmd));
    }

    if !cmds.is_empty() {
        let group = crate::commands::CommandGroup {
            commands: cmds,
            label: "Hide all".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(group));
    }
}

/// Convert a filesystem path to a Bevy asset path (relative to the assets directory).
///
/// Bevy's default asset source reads from `<base>/assets/` where `<base>` is
/// `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR`, or the executable's parent directory.
///
/// The rule itself lives in [`jackdaw_scene_types::to_asset_path`], which the
/// standalone runtime applies to the same authored paths; this only supplies
/// the editor's notion of where the assets directory is.
pub fn to_asset_path(path: &str) -> String {
    jackdaw_scene_types::to_asset_path(path, get_assets_base_dir().as_deref())
}

/// Get the absolute path of Bevy's assets directory.
///
/// The open project's asset folder comes from the resident `ProjectRoot`
/// mirror rather than from disk, because this runs once per model per frame;
/// with no project open it falls back to the recents file and
/// `FileAssetReader`.
pub fn get_assets_base_dir() -> Option<std::path::PathBuf> {
    if let Some(assets) = crate::project::open_project_assets_dir() {
        return Some(dunce::simplified(assets.as_path()).to_path_buf());
    }

    if let Some(project_dir) = crate::project::read_last_project() {
        let assets = crate::project::project_assets_dir(dunce::simplified(project_dir.as_path()));
        if assets.is_dir() {
            return Some(assets);
        }
    }

    let base = if let Ok(dir) = std::env::var("BEVY_ASSET_ROOT") {
        std::path::PathBuf::from(dir)
    } else if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        std::env::current_exe().ok()?.parent()?.to_path_buf()
    };
    Some(dunce::simplified(base.as_path()).join("assets"))
}

// ----------------------- Operators ----------------------------
//
// Entity-level operators (`entity.*`) and the `Add` menu
// (`entity.add.*`). Keybind and menu dispatch both arrive here.
// Operators are gated with `is_available = can_act_on_entities` so
// they refuse to fire while a brush sub-element drag or modal
// operator has the scene locked, matching the guards the legacy
// `handle_entity_keys` applied.

use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EntityDeleteOp>()
        .register_operator::<EntityDuplicateOp>()
        .register_operator::<EntityCopyOp>()
        .register_operator::<EntityCutOp>()
        .register_operator::<EntityPasteOp>()
        .register_operator::<EntityMoveUpOp>()
        .register_operator::<EntityMoveDownOp>()
        .register_operator::<EntityPlaceGltfOp>()
        .register_operator::<EntityCopyComponentsOp>()
        .register_operator::<EntityPasteComponentsOp>()
        .register_operator::<EntityToggleVisibilityOp>()
        .register_operator::<EntityHideUnselectedOp>()
        .register_operator::<EntityUnhideAllOp>()
        .register_operator::<EntityAddCubeOp>()
        .register_operator::<EntityAddSphereOp>()
        .register_operator::<EntityAddPointLightOp>()
        .register_operator::<EntityAddDirectionalLightOp>()
        .register_operator::<EntityAddSpotLightOp>()
        .register_operator::<EntityAddCameraOp>();
    #[cfg(feature = "camera_rig")]
    ctx.register_operator::<EntityAddCameraRigOp>();
    ctx.register_operator::<EntityAddEmptyOp>()
        .register_operator::<EntityAddImageOp>()
        .register_operator::<EntityAddTerrainOp>()
        .register_operator::<EntityAddPrefabOp>()
        .register_operator::<EntityAddPlaneOp>()
        .register_operator::<EntityAddCylinderOp>()
        .register_operator::<EntityAddWedgeOp>()
        .register_operator::<EntityAddConeOp>()
        .register_operator::<EntityAddPyramidOp>()
        .register_operator::<EntityAddAnimationPlayerOp>()
        .register_operator::<AnimationSetStateOp>()
        .register_operator::<EntityAddAudioSourceOp>()
        .register_operator::<EntityAddFogVolumeOp>()
        .register_operator::<EntityAddReflectionProbeOp>()
        // Registered on core rather than on the UI Widgets extension, so it
        // reports an unknown widget name rather than disappearing with it.
        .register_operator::<crate::ui_palette::WidgetAddOp>()
        .register_operator::<crate::sprite_ops::SpriteAddOp>()
        .register_operator::<crate::sprite_props::SpriteSetPixelsPerUnitOp>()
        .register_operator::<crate::sprite_props::SpriteSetAnchorOp>()
        .register_operator::<crate::sprite_props::SpriteZUpOp>()
        .register_operator::<crate::sprite_props::SpriteZDownOp>()
        .register_operator::<crate::sprite_props::SpriteYSortOp>()
        .register_operator::<crate::pixel_camera::CameraPixelCanvasOp>()
        .register_operator::<EntityAddCamera2dOp>()
        .register_operator::<EntityAddEmpty2dOp>()
        .register_operator::<crate::add_entity_picker::EntityAddPickerOp>();

    #[cfg(feature = "multiplayer")]
    ctx.register_operator::<EntityAddSpawnPointOp>()
        .register_operator::<EntityAddZoneTransitionOp>()
        .register_operator::<EntityAddNetworkRoomOp>();

    ctx.bind_operator::<CoreExtensionInputContext, EntityDeleteOp>([PresetInput::key("Delete")]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityDuplicateOp>([
        PresetInput::key("KeyD").ctrl_or_super()
    ]);
    // The timeline claims the same chords for keyframes; the two availability
    // checks are disjoint on it being focused, so one press answers once.
    ctx.bind_operator::<CoreExtensionInputContext, EntityCopyOp>([
        PresetInput::key("KeyC").ctrl_or_super()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityCutOp>([
        PresetInput::key("KeyX").ctrl_or_super()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityPasteOp>([
        PresetInput::key("KeyV").ctrl_or_super()
    ]);
    // Ctrl+Shift is the whole-component clipboard, which pastes at the scene
    // root rather than beside the selection.
    ctx.bind_operator::<CoreExtensionInputContext, EntityCopyComponentsOp>([PresetInput::key(
        "KeyC",
    )
    .ctrl_or_super()
    .shift()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityPasteComponentsOp>([PresetInput::key(
        "KeyV",
    )
    .ctrl_or_super()
    .shift()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityMoveUpOp>([
        PresetInput::key("ArrowUp").ctrl_or_super()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityMoveDownOp>([PresetInput::key(
        "ArrowDown",
    )
    .ctrl_or_super()]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityToggleVisibilityOp>([PresetInput::key(
        "KeyH",
    )]);
    // Ctrl on every platform: macOS keeps Cmd+H for hiding the application.
    ctx.bind_operator::<CoreExtensionInputContext, EntityUnhideAllOp>([
        PresetInput::key("KeyH").ctrl()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, EntityHideUnselectedOp>([PresetInput::key(
        "KeyH",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, crate::add_entity_picker::EntityAddPickerOp>([
        PresetInput::key("KeyA").ctrl_or_super(),
    ]);
}

/// Shared availability check for entity manipulation operators.
///
/// Refuses while a text input has focus, a modal is in flight, or the timeline
/// is focused, so the keyframe operators sharing those chords answer alone.
/// Typing is asked through `KeybindFocus`, since `InputFocus` reports the
/// primary window as focused when nothing has claimed it.
pub(crate) fn can_act_on_entities(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    edit_mode: Res<crate::brush::EditMode>,
    panel_focus: crate::panel_focus::PanelFocus,
) -> bool {
    if keybind_focus.keyboard_is_spoken_for() || active.is_modal_running() || modal.active.is_some()
    {
        return false;
    }
    if draw_state.active.is_some() {
        return false;
    }
    if panel_focus.is_focused(crate::animation_mode::TIMELINE_WINDOW_ID) {
        return false;
    }

    matches!(*edit_mode, crate::brush::EditMode::Object)
}

// -- Entity lifecycle --------------------------------------------

#[operator(
    id = "entity.place_gltf",
    label = "Place GLTF",
    description = "Place a GLTF asset into the active scene at a world position.",
    allows_undo = true,
    params(
        path(String, doc = "Path to the GLTF asset."),
        pos_x(f64, doc = "World-space X position."),
        pos_y(
            f64,
            doc = "World-space Y position. The terrain height under the point \
                   when omitted."
        ),
        pos_z(f64, doc = "World-space Z position."),
    )
)]
pub(crate) fn entity_place_gltf(
    params: In<OperatorParameters>,
    terrains: crate::terrain_mode::TerrainSurfaces,
    store: Option<Res<crate::terrain_mode::TerrainDataStore>>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_owned) else {
        warn!("entity.place_gltf: missing `path` param");
        return OperatorResult::Cancelled;
    };
    let Some(x) = params.as_float("pos_x") else {
        warn!("entity.place_gltf: missing `pos_x` param");
        return OperatorResult::Cancelled;
    };
    let Some(z) = params.as_float("pos_z") else {
        warn!("entity.place_gltf: missing `pos_z` param");
        return OperatorResult::Cancelled;
    };
    let ground = || {
        let store = store.as_deref()?;
        crate::terrain_mode::height_under(&terrains, store, Vec2::new(x as f32, z as f32))
    };
    // Placing a model on the ground is what a caller with no viewport asks
    // for, so an omitted height is the ground rather than a refusal.
    let y = params
        .as_float("pos_y")
        .map(|y| y as f32)
        .or_else(ground)
        .unwrap_or(0.0);
    let position = Vec3::new(x as f32, y, z as f32);
    commands.queue(move |world: &mut World| {
        spawn_gltf_in_world(world, &path, position);
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.delete",
    label = "Delete",
    allows_undo = false,
    is_available = can_act_on_entities,
    params(
        entity(Entity, doc = "Entity to delete. Defaults to the selection."),
        entities(
            String,
            doc = "Entities to delete, as a list of ids, names, paths or queries such as #roof. Defaults \
                   to the selection."
        ),
    )
)]
pub(crate) fn entity_delete(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    authored: Query<(), Without<crate::EditorEntity>>,
    mut commands: Commands,
) -> OperatorResult {
    let targets = match crate::boot_ops::target_entities(&params, &selection, &authored) {
        Ok(targets) => targets,
        Err(refusal) => {
            commands.queue(move |world: &mut World| {
                warn_caller(world, format!("entity.delete: {refusal}"));
            });
            return OperatorResult::Cancelled;
        }
    };
    commands.queue(move |world: &mut World| delete_entities(world, &targets));
    OperatorResult::Finished
}

#[operator(
    id = "entity.duplicate",
    label = "Duplicate",
    description = "Duplicate entities, or lay out a run of copies each offset and turned from the last.",
    is_available = can_act_on_entities,
    params(
        entity(Entity, doc = "Entity to duplicate. Defaults to the selection."),
        entities(
            String,
            doc = "Entities to duplicate, as a list of ids, names, paths or queries such as #roof. Defaults \
                   to the selection."
        ),
        offset(
            Vec3,
            doc = "Move [x, y, z] from each copy to the next, in its parent's space. Defaults to none."
        ),
        rot(
            Vec3,
            doc = "Turn [yaw, pitch, roll] in degrees from each copy to the next. Defaults to none."
        ),
        count(i64, doc = "Copies of each entity, 1 to 256. Defaults to 1."),
        parent(
            Entity,
            doc = "Node that adopts every copy, keeping the source's local placement. Defaults to \
                   each source's own parent."
        ),
    )
)]
pub(crate) fn entity_duplicate(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let request = read_duplicate(world, &params);
    let (entities, run) = match request {
        Ok(request) => request,
        Err(why) => {
            warn_caller(world, format!("entity.duplicate: {why}"));
            set_outcome(world, Outcome::Invalid);
            return OperatorResult::Cancelled;
        }
    };
    let copies = duplicate_entities(world, &entities, &run);
    set_outcome(
        world,
        if copies.is_empty() {
            Outcome::Unchanged
        } else {
            Outcome::Changed
        },
    );
    OperatorResult::Finished
}

/// `name` is the bare name a clause gives, which the resolver has already
/// turned into `entity`.
const DUPLICATE_KEYS: [&str; 7] = [
    "entity", "entities", "name", "offset", "rot", "count", "parent",
];

/// The sources and layout an `entity.duplicate` call asked for, checked.
fn read_duplicate(
    world: &mut World,
    params: &OperatorParameters,
) -> Result<(Vec<Entity>, DuplicateRun), String> {
    if let Some(key) = params
        .keys()
        .find(|key| !DUPLICATE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("this takes no `{key}`"));
    }
    let count = crate::primitive_ops::int_param(params, "count", 1..=256)?.unwrap_or(1);
    let offset = crate::primitive_ops::vec3_param(params, "offset")?.unwrap_or(Vec3::ZERO);
    let step_rotation = crate::primitive_ops::vec3_param(params, "rot")?
        .map_or(Quat::IDENTITY, crate::primitive_ops::euler_degrees);
    let parent = crate::primitive_ops::parent_param(world, params)?;
    if let Some(name) = params.as_str("name")
        && !matches!(
            params.get("entity"),
            Some(jackdaw_scene_types::PropertyValue::Entity(_))
        )
    {
        return Err(format!(
            "`name` {name:?} names no entity in this scene, or two"
        ));
    }
    let mut state: SystemState<(Res<Selection>, Query<(), Without<EditorEntity>>)> =
        SystemState::new(world);
    let (selection, authored) = state.get(world).map_err(|err| err.to_string())?;
    let entities = crate::boot_ops::target_entities(params, &selection, &authored)?;
    if entities.is_empty() {
        return Err("nothing is selected and `entities` names nothing".into());
    }
    let moves = offset != Vec3::ZERO || step_rotation != Quat::IDENTITY;
    if moves
        && let Some(&bare) = entities
            .iter()
            .find(|&&entity| !authors_transform(world, entity))
    {
        return Err(format!(
            "{bare} has no Transform, so `offset` and `rot` have nothing to move"
        ));
    }
    Ok((
        entities,
        DuplicateRun {
            count,
            offset,
            step_rotation,
            parent,
        },
    ))
}

#[operator(
    id = "entity.move_up",
    label = "Move Up",
    description = "Move the selection one slot earlier among its siblings.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_move_up(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| move_selected_siblings(world, -1));
    OperatorResult::Finished
}

#[operator(
    id = "entity.move_down",
    label = "Move Down",
    description = "Move the selection one slot later among its siblings.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_move_down(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| move_selected_siblings(world, 1));
    OperatorResult::Finished
}

#[operator(
    id = "entity.copy",
    label = "Copy",
    description = "Copy the selected subtrees to the clipboard as scene text.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_copy(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(copy_selected_entities);
    OperatorResult::Finished
}

#[operator(
    id = "entity.cut",
    label = "Cut",
    description = "Copy the selected subtrees to the clipboard and delete them.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_cut(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(cut_selected_entities);
    OperatorResult::Finished
}

#[operator(
    id = "entity.paste",
    label = "Paste",
    description = "Paste the clipboard's subtrees as siblings after the selection.",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_paste(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(paste_entities_after_selection);
    OperatorResult::Finished
}

#[operator(
    id = "entity.copy_components",
    label = "Copy Components",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_copy_components(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(copy_components);
    OperatorResult::Finished
}

#[operator(
    id = "entity.paste_components",
    label = "Paste Components",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_paste_components(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(paste_components);
    OperatorResult::Finished
}

#[operator(
    id = "entity.toggle_visibility",
    label = "Toggle Visibility",
    is_available = can_act_on_entities
)]
pub(crate) fn entity_toggle_visibility(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(hide_selected);
    OperatorResult::Finished
}

#[operator(
    id = "entity.hide_unselected",
    label = "Hide Unselected",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_hide_unselected(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Err(err) = world.run_system_cached(hide_all_entities) {
            warn!("hide_all_entities: {err:?}");
        }
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.unhide_all",
    label = "Unhide All",
    allows_undo = false,
    is_available = can_act_on_entities
)]
pub(crate) fn entity_unhide_all(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Err(err) = world.run_system_cached(unhide_all_entities) {
            warn!("unhide_all_entities: {err:?}");
        }
    });
    OperatorResult::Finished
}

// -- Add menu ----------------------------------------------------

#[operator(
    id = "entity.add.cube",
    label = "Cube",
    description = "Add a cube brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(Vec3, doc = "Edge lengths [x, y, z]. Defaults to 1 on each axis."),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Cube\"."),
    )
)]
pub(crate) fn entity_add_cube(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Cube)
}

#[operator(
    id = "entity.add.sphere",
    label = "Sphere",
    description = "Add a sphere brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Diameters [x, y, z] of the sphere its vertices lie on. Defaults to 1."
        ),
        kind(
            String,
            doc = "uv for rings and segments, ico for a subdivided 20-face sphere. Defaults to uv when segments or rings are given, else ico."
        ),
        segments(
            i64,
            doc = "Sides around the axis of a uv sphere, 3 to 256. Defaults to 8."
        ),
        rings(
            i64,
            doc = "Bands from pole to pole of a uv sphere, 2 to 128. Defaults to half the segments."
        ),
        subdivisions(
            i64,
            doc = "Times an ico sphere's triangles split in four, 0 to 4. Defaults to 0, 20 faces."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Sphere\"."),
    )
)]
pub(crate) fn entity_add_sphere(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Sphere)
}

#[operator(
    id = "entity.add.point_light",
    label = "Point Light",
    description = "Add a point light, optionally coloured, sized, placed, named and parented.",
    allows_undo = false,
    params(
        color(
            String,
            doc = "Colour: a hex code, a name, or r,g,b in 0..1. Defaults to white."
        ),
        intensity(f64, doc = "Brightness in lumens, 0 or more. Defaults to 1000000."),
        range(f64, doc = "How far it reaches in metres, above 0. Defaults to 10."),
        radius(
            f64,
            doc = "Size of the glowing source in metres, 0 or more. Defaults to 0."
        ),
        shadows(bool, doc = "Whether it casts shadows. Defaults to false."),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to [0, 3, 0]."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Point Light\"."),
    )
)]
pub(crate) fn entity_add_point_light(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::PointLight),
    )
}

#[operator(
    id = "entity.add.directional_light",
    label = "Directional Light",
    description = "Add a directional light, optionally coloured, aimed, placed, named and parented.",
    allows_undo = false,
    params(
        color(
            String,
            doc = "Colour: a hex code, a name, or r,g,b in 0..1. Defaults to white."
        ),
        intensity(f64, doc = "Illuminance in lux, 0 or more. Defaults to 10000."),
        shadows(bool, doc = "Whether it casts shadows. Defaults to true."),
        target(
            Vec3,
            doc = "Point [x, y, z] to aim at, in the same space as pos. Not with rot. Defaults to a downward tilt."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to [0, 10, 0]."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to a downward tilt."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(
            String,
            doc = "Name for the new node. Defaults to \"Directional Light\"."
        ),
    )
)]
pub(crate) fn entity_add_directional_light(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::DirectionalLight),
    )
}

#[operator(
    id = "entity.add.spot_light",
    label = "Spot Light",
    description = "Add a spot light, optionally coloured, sized, coned, aimed, placed, named and parented.",
    allows_undo = false,
    params(
        color(
            String,
            doc = "Colour: a hex code, a name, or r,g,b in 0..1. Defaults to white."
        ),
        intensity(f64, doc = "Brightness in lumens, 0 or more. Defaults to 1000000."),
        range(f64, doc = "How far it reaches in metres, above 0. Defaults to 10."),
        radius(
            f64,
            doc = "Size of the glowing source in metres, 0 or more. Defaults to 0."
        ),
        shadows(bool, doc = "Whether it casts shadows. Defaults to false."),
        cone(
            Vec2,
            doc = "[inner, outer] angles from the axis in degrees, 0 <= inner <= outer <= 90. Defaults to [0, 45]."
        ),
        target(
            Vec3,
            doc = "Point [x, y, z] to aim at, in the same space as pos. Not with rot. Defaults to the origin."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to [0, 3, 0]."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to aimed at the origin."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Spot Light\"."),
    )
)]
pub(crate) fn entity_add_spot_light(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::SpotLight),
    )
}

#[operator(
    id = "entity.add.camera",
    label = "3D Camera",
    description = "Add a 3D camera the game draws through, optionally placed, named and parented. A camera saved inactive by an earlier editor stays off: set its Camera is_active to true to use it.",
    allows_undo = false,
    params(
        pos(Vec3, doc = "Position [x, y, z]. Defaults to [0, 2, 5]."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to facing the origin."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Camera\"."),
    )
)]
pub(crate) fn entity_add_camera(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::Camera3d),
    )
}

#[cfg(feature = "camera_rig")]
#[operator(id = "entity.add.camera_rig", label = "Camera Rig")]
pub(crate) fn entity_add_camera_rig(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::CameraRig);
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.add.image",
    label = "Reference Image",
    description = "Add a reference image. With a path it is placed directly, else a file picker opens.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Image file to show (png, jpg, jpeg, ktx2, bmp, tga, webp). Left out, a file picker opens and nothing else may be given."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Image\"."),
    )
)]
pub fn entity_add_image(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    if params.is_empty() {
        crate::reference_image::open_reference_image_picker(world);
        return OperatorResult::Finished;
    }
    crate::node_ops::add_node(world, &params, crate::node_ops::NodeKind::Image)
}

/// Add a 2D camera to the open 2D world scene.
#[operator(
    id = "entity.add.camera_2d",
    label = "2D Camera",
    unavailable_reason = crate::sprite_ops::camera_needs_2d_scene,
    is_available = can_add_camera_2d
)]
pub(crate) fn entity_add_camera_2d(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Camera2d);
    });
    OperatorResult::Finished
}

fn can_add_camera_2d(gate: crate::sprite_ops::SpriteAddGate) -> bool {
    gate.allows()
}

fn can_add_empty_2d(gate: crate::sprite_ops::SpriteAddGate) -> bool {
    gate.allows()
}

#[operator(
    id = "entity.add.empty",
    label = "Empty",
    description = "Add an empty node, optionally placed, named and parented.",
    allows_undo = false,
    params(
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Empty\"."),
    )
)]
pub(crate) fn entity_add_empty(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::Empty),
    )
}

/// Add an empty entity to the open 2D world scene.
#[operator(
    id = "entity.add.empty_2d",
    label = "Empty 2D",
    description = "Add an empty entity to the open 2D world scene.",
    unavailable_reason = crate::sprite_ops::empty_needs_2d_scene,
    is_available = can_add_empty_2d
)]
pub(crate) fn entity_add_empty_2d(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::Empty);
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.add.plane",
    label = "Plane",
    description = "Add a plane brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Footprint [x, y, z]; y is ignored, the slab stays thin. Defaults to 1 by 1."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Plane\"."),
    )
)]
pub(crate) fn entity_add_plane(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Plane)
}

#[operator(
    id = "entity.add.cylinder",
    label = "Cylinder",
    description = "Add a cylinder brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Bounding box [x, y, z]: x and z are the diameters, y the height. Defaults to 1."
        ),
        segments(i64, doc = "Sides around the axis, 3 to 256. Defaults to 16."),
        radius_top(
            f64,
            doc = "Top radius along x, z in proportion; 0 is a cone. Defaults to the base radius."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Cylinder\"."),
    )
)]
pub(crate) fn entity_add_cylinder(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Cylinder)
}

#[operator(
    id = "entity.add.wedge",
    label = "Wedge",
    description = "Add a wedge brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Bounding box [x, y, z] of the ramp. Defaults to 1 on each axis."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Wedge\"."),
    )
)]
pub(crate) fn entity_add_wedge(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Wedge)
}

#[operator(
    id = "entity.add.cone",
    label = "Cone",
    description = "Add a cone brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Bounding box [x, y, z]: x and z are the base diameters, y the height. Defaults to 1."
        ),
        segments(i64, doc = "Sides around the base, 3 to 256. Defaults to 16."),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Cone\"."),
    )
)]
pub(crate) fn entity_add_cone(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Cone)
}

#[operator(
    id = "entity.add.pyramid",
    label = "Pyramid",
    description = "Add a pyramid brush, optionally sized, placed, materialled, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Bounding box [x, y, z]: x and z are the base, y the height. Defaults to 1."
        ),
        sides(
            i64,
            doc = "Sides of the base, 3 to 256, stretched to fill the size. Defaults to 4."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        material(
            String,
            doc = "Palette name or material file path for every face. Defaults to the last used."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Pyramid\"."),
    )
)]
pub(crate) fn entity_add_pyramid(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::primitive_ops::add_primitive(world, &params, EntityTemplate::Pyramid)
}

#[operator(id = "entity.add.animation_player", label = "Animation Player")]
pub(crate) fn entity_add_animation_player(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        create_entity_in_world(world, EntityTemplate::AnimationPlayer);
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.add.audio_source",
    label = "Audio Source",
    description = "Add an audio source, optionally placed, named and parented.",
    allows_undo = false,
    params(
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Audio Source\"."),
    )
)]
pub(crate) fn entity_add_audio_source(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::AudioSource),
    )
}

#[operator(
    id = "entity.add.fog_volume",
    label = "Fog Volume",
    description = "Add a fog volume, optionally sized, placed, named and parented.",
    allows_undo = false,
    params(
        size(Vec3, doc = "Box [x, y, z] the fog fills, its scale. Defaults to 1."),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Fog Volume\"."),
    )
)]
pub(crate) fn entity_add_fog_volume(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(
        world,
        &params,
        crate::node_ops::NodeKind::Template(EntityTemplate::FogVolume),
    )
}

#[operator(
    id = "entity.add.reflection_probe",
    label = "Reflection Probe",
    description = "Add a reflection probe, optionally sized, placed, named and parented.",
    allows_undo = false,
    params(
        size(
            Vec3,
            doc = "Box [x, y, z] the probe covers, its scale. Defaults to 10."
        ),
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(
            String,
            doc = "Name for the new node. Defaults to \"Reflection Probe\"."
        ),
    )
)]
pub(crate) fn entity_add_reflection_probe(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(world, &params, crate::node_ops::NodeKind::ReflectionProbe)
}

#[cfg(feature = "multiplayer")]
#[operator(
    id = "entity.add.spawn_point",
    label = "Spawn Point",
    description = "Add a spawn point, optionally placed, named and parented.",
    allows_undo = false,
    params(
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Spawn Point\"."),
    )
)]
pub(crate) fn entity_add_spawn_point(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    crate::node_ops::add_node(world, &params, crate::node_ops::NodeKind::SpawnPoint)
}

#[cfg(feature = "multiplayer")]
#[operator(id = "entity.add.zone_transition", label = "Zone Transition")]
pub(crate) fn entity_add_zone_transition(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Zone Transition", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = commands
                .spawn((
                    Name::new("Zone Transition"),
                    jackdaw_multiplayer::ZoneTransition {
                        half_extents: Vec3::splat(1.0),
                        ..default()
                    },
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[cfg(feature = "multiplayer")]
#[operator(id = "entity.add.network_room", label = "Network Room")]
pub(crate) fn entity_add_network_room(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::spawn_undoable(world, "Add Network Room", |world| {
            let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                SystemState::new(world);
            let Ok((mut commands, mut selection)) = system_state.get_mut(world) else {
                return Entity::PLACEHOLDER;
            };
            let entity = commands
                .spawn((
                    Name::new("Network Room"),
                    jackdaw_multiplayer::NetworkRoom::default(),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            selection.select_single(&mut commands, entity);
            system_state.apply(world);
            crate::scene_io::register_entity_in_ast(world, entity);
            entity
        });
    });
    OperatorResult::Finished
}

#[operator(
    id = "entity.add.terrain",
    label = "Terrain",
    description = "Add a terrain, optionally placed, named and parented.",
    allows_undo = false,
    params(
        pos(Vec3, doc = "Position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts it. Defaults to top level."),
        name(String, doc = "Name for the new node. Defaults to \"Terrain\"."),
    )
)]
pub(crate) fn entity_add_terrain(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let is_first_terrain = world
        .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>()
        .iter(world)
        .next()
        .is_none();
    let result = crate::node_ops::add_node(world, &params, crate::node_ops::NodeKind::Terrain);
    // Only the first terrain opens the panel: a later add must not steal
    // focus from the tab in use.
    if is_first_terrain && result == OperatorResult::Finished {
        crate::open_window_in_default_area_if_absent(world, crate::terrain_mode::TERRAIN_WINDOW_ID);
    }
    result
}

/// Pick a prefab file and drop an instance of it at the origin, through a polled
/// async picker so the editor keeps drawing while the dialog is up.
#[operator(id = "entity.add.prefab", label = "Prefab")]
pub fn entity_add_prefab(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(crate::prefab::operators::open_prefab_picker);
    OperatorResult::Finished
}

// -- Animation ---------------------------------------------------

/// Ask an entity's animation set for one of the states it declares, so an
/// authored set previews in the viewport the way it plays in the game.
#[operator(
    id = "animation.set_state",
    label = "Set Animation State",
    description = "Play one of the states an entity's animation set declares.",
    params(
        entity(
            Entity,
            doc = "Entity carrying the animation set. Defaults to the selection."
        ),
        state(String, doc = "Name of the state to play."),
    )
)]
pub(crate) fn animation_set_state(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> Option<OperatorResult> {
    let entity = params.as_entity("entity")?;
    let state = params.as_str("state").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        if let Ok(mut entity) = world.get_entity_mut(entity) {
            entity.insert(jackdaw_animation_runtime::AnimationState(state));
        }
    });
    Some(OperatorResult::Finished)
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    /// A world whose registry holds `Node`, so a document naming it reads as a
    /// scene.
    fn world_with_node_registered() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Node>();
        world.insert_resource(registry);
        world
    }

    /// The only shape a paste accepts from a stranger: one entity root carrying
    /// a registered component.
    fn a_real_subtree() -> String {
        format!(
            "#Pasted\n{}\n",
            crate::inspector::node_card::node_type_path()
        )
    }

    #[test]
    fn prose_is_not_an_entity_document() {
        let world = world_with_node_registered();
        // A bare identifier parses as a component patch.
        assert!(!is_entity_document(&world, "Remember to buy milk"));
        assert!(!is_entity_document(&world, "hello"));
    }

    #[test]
    fn json_is_not_an_entity_document() {
        let world = world_with_node_registered();
        assert!(!is_entity_document(
            &world,
            r#"{"name": "thing", "value": 3}"#
        ));
    }

    #[test]
    fn a_root_naming_no_registered_component_is_not_an_entity_document() {
        let world = world_with_node_registered();
        assert!(
            !is_entity_document(&world, "#Lonely\nsome::type::NobodyRegisters\n"),
            "a root whose every patch names an unknown type is text that parses"
        );
    }

    #[test]
    fn a_copied_subtree_is_an_entity_document() {
        let world = world_with_node_registered();
        assert!(is_entity_document(&world, &a_real_subtree()));
    }

    #[test]
    fn a_payload_past_the_size_cap_is_refused() {
        let world = world_with_node_registered();
        let mut oversized = a_real_subtree();
        oversized.push_str(&" ".repeat(MAX_CLIPBOARD_BYTES));
        assert!(
            !is_entity_document(&world, &oversized),
            "a payload past the cap is refused before it is parsed"
        );
    }

    #[test]
    fn os_text_this_editor_emitted_pastes_the_mirror() {
        let world = world_with_node_registered();
        let emitted = a_real_subtree();
        assert_eq!(
            choose_clipboard_text(
                &world,
                Some(emitted.clone()),
                Some((emitted.clone(), emitted.clone()))
            ),
            Some(emitted),
            "text equal to the last emission is our own copy"
        );
    }

    #[test]
    fn os_text_that_is_not_entities_refuses_rather_than_pasting_the_mirror() {
        let world = world_with_node_registered();
        let mirror = a_real_subtree();
        for foreign in ["Remember to buy milk", r#"{"a": 1}"#] {
            assert_eq!(
                choose_clipboard_text(
                    &world,
                    Some(mirror.clone()),
                    Some((foreign.to_string(), mirror.clone()))
                ),
                None,
                "{foreign} must refuse, not paste what was copied an hour ago"
            );
        }
    }

    #[test]
    fn a_subtree_from_another_window_is_accepted_over_the_mirror() {
        let world = world_with_node_registered();
        let foreign = format!(
            "#FromElsewhere\n{}\n",
            crate::inspector::node_card::node_type_path()
        );
        assert_eq!(
            choose_clipboard_text(
                &world,
                Some(a_real_subtree()),
                Some((foreign.clone(), a_real_subtree()))
            ),
            Some(foreign),
            "a real subtree written by another instance wins over the mirror"
        );
    }

    #[test]
    fn with_no_os_clipboard_the_mirror_answers() {
        let world = world_with_node_registered();
        let mirror = a_real_subtree();
        assert_eq!(
            choose_clipboard_text(&world, Some(mirror.clone()), None),
            Some(mirror)
        );
        assert_eq!(choose_clipboard_text(&world, None, None), None);
    }

    /// A cyclic document, which no parser produces but a corrupt payload can be.
    fn ast_with_a_cycle() -> jackdaw_bsn::SceneBsnAst {
        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let child = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Child".to_string())]);
        let root = ast.create_entity_node(vec![
            jackdaw_bsn::BsnPatch::Name("Root".to_string()),
            jackdaw_bsn::BsnPatch::Children(vec![child]),
        ]);
        ast.add_to_roots(root);
        ast.add_child_to_ast(child, root);
        ast
    }

    #[test]
    fn minting_ids_over_a_cyclic_document_ends() {
        let world = world_with_node_registered();
        let mut ast = ast_with_a_cycle();
        mint_scene_node_ids(&world, &mut ast);
        assert_eq!(
            walk_entity_nodes(&ast).len(),
            2,
            "each node is visited once however the children point"
        );
    }

    #[test]
    fn naming_a_cyclic_document_ends() {
        let mut world = world_with_node_registered();
        let mut ast = ast_with_a_cycle();
        assign_unique_entity_names(&mut world, &mut ast);
        let names: Vec<Option<&str>> = walk_entity_nodes(&ast)
            .into_iter()
            .map(|node| ast.get_name(node))
            .collect();
        assert_eq!(names, vec![Some("Root"), Some("Child")]);
    }

    #[test]
    fn a_pasted_subtree_uniquifies_its_descendants_too() {
        let mut world = world_with_node_registered();
        world.spawn(Name::new("Button"));
        world.spawn(Name::new("Caption"));

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let caption =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Caption".to_string())]);
        let root = ast.create_entity_node(vec![
            jackdaw_bsn::BsnPatch::Name("Button".to_string()),
            jackdaw_bsn::BsnPatch::Children(vec![caption]),
        ]);
        ast.add_to_roots(root);

        assign_unique_entity_names(&mut world, &mut ast);

        assert_eq!(ast.get_name(root), Some("Button2"));
        assert_eq!(
            ast.get_name(caption),
            Some("Caption2"),
            "a descendant colliding with a live name is renamed too"
        );
    }

    #[test]
    fn free_names_are_addressable_from_a_clause() {
        let mut taken = std::collections::HashSet::new();
        assert_eq!(claim_free_name(&mut taken, "Button"), None);
        assert_eq!(
            claim_free_name(&mut taken, "Button"),
            Some("Button2".to_string())
        );
        assert_eq!(
            claim_free_name(&mut taken, "Button"),
            Some("Button3".to_string())
        );
        for name in taken {
            assert!(!name.contains(' '), "{name} cannot be written in a clause");
        }
    }

    #[test]
    fn an_opening_scene_hands_over_larger_batches_than_editing() {
        let mut world = World::new();
        let mut pending = PendingModelRoots::default();
        for _ in 0..40_000 {
            pending.push(world.spawn_empty().id(), Some(Handle::default()));
        }
        let frame = Duration::from_millis(16);
        let editing = (0..16).map(|_| pending.take(frame, false).len()).max();
        let opening = (0..16).map(|_| pending.take(frame, true).len()).max();
        assert_eq!(editing, Some(MODEL_BATCH_CEILING));
        assert_eq!(opening, Some(MODEL_LOAD_BATCH_CEILING));

        let after_opening = (0..4).map(|_| pending.take(frame, false).len()).max();
        assert_eq!(
            after_opening,
            Some(MODEL_BATCH_CEILING),
            "editing goes back to its own pace once the scene is open"
        );
    }

    #[test]
    fn an_older_spaced_suffix_renumbers_from_its_base() {
        let mut taken: std::collections::HashSet<String> =
            ["Button".to_string(), "Button 2".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            claim_free_name(&mut taken, "Button 2"),
            Some("Button3".to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mint_scene_node_ids` replaces any existing `SceneNodeId` with a fresh
    /// sparse id and leaves other patches alone.
    #[test]
    fn mint_scene_node_ids_replaces_existing_ids() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let node = ast.create_entity_node(vec![
            jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
                type_path: jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH.to_string(),
                values: vec![jackdaw_bsn::BsnValue::Int(42)],
            }),
            jackdaw_bsn::BsnPatch::Name("Kept".to_string()),
        ]);
        ast.add_to_roots(node);

        mint_scene_node_ids(&world, &mut ast);

        let id = ast.stable_id_of(node).expect("id patch present");
        assert_ne!(id, 42, "the stale id must be replaced");
        assert!(
            id >= jackdaw_scene_types::SPARSE_MIN,
            "minted id must be in the sparse range"
        );
        assert_eq!(ast.get_name(node), Some("Kept"), "other patches survive");
    }

    #[test]
    fn assign_unique_entity_names_keeps_free_names_and_numbers_collisions() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.spawn(Name::new("Brush"));
        world.spawn(Name::new("Brush2"));
        world.spawn((Name::new("Camera"), EditorEntity));

        let mut ast = jackdaw_bsn::SceneBsnAst::default();
        let free = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Camera".to_string())]);
        let taken = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Brush".to_string())]);
        let also_taken =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("Brush".to_string())]);
        ast.add_to_roots(free);
        ast.add_to_roots(taken);
        ast.add_to_roots(also_taken);

        assign_unique_entity_names(&mut world, &mut ast);

        assert_eq!(
            ast.get_name(free),
            Some("Camera"),
            "editor chrome names do not force renames"
        );
        assert_eq!(
            ast.get_name(taken),
            Some("Brush3"),
            "colliding name takes the next free number"
        );
        assert_eq!(
            ast.get_name(also_taken),
            Some("Brush4"),
            "batch collisions advance past names assigned earlier in the same pass"
        );
    }

    /// The Terrain panel auto-focuses only on a document's first terrain.
    mod terrain_focus_guard {
        use jackdaw_panels::DockAreaStyle;
        use jackdaw_panels::tree::{DockLeaf, DockNode, DockTree};

        use super::*;

        /// Mirrors `build_default_tree`'s `right_sidebar` leaf, Components active.
        fn world_with_right_sidebar_seeded() -> World {
            let mut world = World::new();
            world.insert_resource(CommandHistory::default());
            world.insert_resource(Selection::default());
            let mut tree = DockTree::new();
            tree.set_root_leaf(
                DockLeaf::new("right_sidebar", DockAreaStyle::TabBar).with_windows(vec![
                    "jackdaw.inspector".to_string(),
                    "jackdaw.inspector.terrain".to_string(),
                    "jackdaw.inspector.materials".to_string(),
                ]),
            );
            world.insert_resource(tree);
            world
        }

        fn active_window(world: &World) -> Option<String> {
            let tree = world.resource::<DockTree>();
            let leaf = tree.get(tree.root?).and_then(DockNode::as_leaf)?;
            leaf.windows
                .iter()
                .find(|t| Some(t.id) == leaf.active)
                .map(|t| t.window_id.clone())
        }

        fn focus_window(world: &mut World, window_id: &str) {
            let mut tree = world.resource_mut::<DockTree>();
            let leaf_id = tree.root.expect("seeded tree has a root");
            let tab_id = tree
                .get(leaf_id)
                .and_then(DockNode::as_leaf)
                .and_then(|l| l.tabs().find(|(id, _)| *id == window_id))
                .map(|(_, tab)| tab)
                .expect("window is present as a seeded tab");
            tree.set_active(leaf_id, tab_id);
        }

        #[test]
        fn first_terrain_add_focuses_the_seeded_unfocused_tab() {
            let mut world = world_with_right_sidebar_seeded();

            let result = world
                .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
                .expect("system runs");
            assert_eq!(result, OperatorResult::Finished);

            assert_eq!(
                active_window(&world).as_deref(),
                Some("jackdaw.inspector.terrain"),
                "the document's first terrain should bring the panel to front"
            );
        }

        #[test]
        fn second_terrain_add_leaves_components_active() {
            let mut world = world_with_right_sidebar_seeded();

            let result = world
                .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
                .expect("first add runs");
            assert_eq!(result, OperatorResult::Finished);
            assert_eq!(
                active_window(&world).as_deref(),
                Some("jackdaw.inspector.terrain"),
                "sanity check: first add still focuses Terrain"
            );

            // The user switches back to Components to keep working there.
            focus_window(&mut world, "jackdaw.inspector");

            let result = world
                .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
                .expect("second add runs");
            assert_eq!(result, OperatorResult::Finished);

            assert_eq!(
                active_window(&world).as_deref(),
                Some("jackdaw.inspector"),
                "a second terrain must not steal focus from Components"
            );
        }
    }

    /// A terrain's extent lives in its sidecar, so a saved scene states no rectangle beside
    /// it. The two extent fields are read only by the load-time migration, where a stated
    /// rectangle means a sidecar that cannot place its own grid.
    #[test]
    fn a_saved_terrain_declares_no_extent() {
        use bevy::ecs::reflect::AppTypeRegistry;

        let mut world = World::new();
        world.insert_resource(CommandHistory::default());
        world.insert_resource(Selection::default());
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut writer = registry.write();
            writer.register::<Name>();
            writer.register::<Transform>();
            writer.register::<Visibility>();
            writer.register::<jackdaw_scene_types::Terrain>();
            writer.register::<jackdaw_scene_types::SceneNodeId>();
        }
        world.init_resource::<jackdaw_bsn::SceneBsnAst>();
        world.init_resource::<jackdaw_panels::tree::DockTree>();
        world.init_resource::<jackdaw_panels::registry::WindowRegistry>();

        let result = world
            .run_system_cached_with(entity_add_terrain, OperatorParameters::default())
            .expect("the operator runs");
        assert_eq!(result, OperatorResult::Finished);

        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(
            &mut world,
            std::path::Path::new("."),
        );

        assert!(
            !text.contains("resolution:"),
            "the saved scene must not declare a resolution:\n{text}"
        );
        assert!(
            !text.contains("size:"),
            "the saved scene must not declare a size:\n{text}"
        );
    }
}

#[cfg(test)]
mod asset_path_tests {
    use super::*;

    /// The directory named here does not exist, so a relative path coming back
    /// is proof nothing went to the filesystem. The mirror is process-global, so
    /// set, clear and the system driving it are one test rather than three.
    #[test]
    fn an_open_project_resolves_asset_paths_without_touching_the_disk() {
        crate::project::set_open_project_assets_dir(Some(std::path::PathBuf::from(
            "/jackdaw-no-such-project/assets",
        )));

        assert_eq!(
            to_asset_path("/jackdaw-no-such-project/assets/models/rock.glb"),
            "models/rock.glb"
        );

        // Cleared, the same path has no base to strip and comes back whole.
        crate::project::set_open_project_assets_dir(None);
        assert_eq!(
            to_asset_path("/jackdaw-no-such-project/assets/models/rock.glb"),
            "/jackdaw-no-such-project/assets/models/rock.glb"
        );

        // And what the mirror holds is what the resource says.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut world = World::new();
        world.insert_resource(crate::project::ProjectRoot {
            root: root.clone(),
            config: default(),
        });
        world
            .run_system_cached(crate::project::mirror_open_project)
            .expect("the mirror runs");
        assert_eq!(
            crate::project::open_project_assets_dir(),
            Some(root.join("assets"))
        );

        world.remove_resource::<crate::project::ProjectRoot>();
        world
            .run_system_cached(crate::project::mirror_open_project)
            .expect("the mirror runs");
        assert_eq!(crate::project::open_project_assets_dir(), None);
    }
}

#[cfg(test)]
mod clone_brush_tests {
    use super::*;
    use bevy::reflect::PartialReflect;
    use jackdaw_bsn::SceneBsnAst;
    use jackdaw_scene_types::Brush;

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct ExtraBrushMarker;

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct DefaultBrushMarker(u32);

    #[test]
    fn cloned_brush_keeps_extra_components() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SceneBsnAst>();
        app.register_type::<Brush>();
        app.register_type::<Transform>();
        app.register_type::<Visibility>();
        app.register_type::<Name>();
        app.register_type::<jackdaw_scene_types::SceneNodeId>();
        app.register_type::<DefaultBrushMarker>();
        app.world_mut()
            .spawn(jackdaw_api_internal::brush::BrushDefault(Box::new(
                DefaultBrushMarker(7),
            )));
        app.register_type::<ExtraBrushMarker>();

        let source = app
            .world_mut()
            .spawn((
                Name::new("Brush"),
                Brush::cuboid(0.5, 0.5, 0.5),
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        crate::scene_io::register_entity_in_ast(app.world_mut(), source);
        crate::brush::apply_brush_defaults(app.world_mut(), source);

        app.world_mut().entity_mut(source).insert(ExtraBrushMarker);
        crate::commands::sync_component_to_bsn_doc(
            app.world_mut(),
            source,
            ExtraBrushMarker.as_partial_reflect(),
        );

        let new_brush = Brush::cuboid(1.0, 1.0, 1.0);
        let spawned = spawn_cloned_brush_with_geometry(
            app.world_mut(),
            source,
            new_brush.clone(),
            Transform::from_xyz(2.0, 0.0, 0.0),
        )
        .expect("clone spawn");

        let spawned_ref = app.world().entity(spawned);
        assert!(
            spawned_ref.contains::<ExtraBrushMarker>(),
            "user-added components must copy onto the clone"
        );
        assert_eq!(
            spawned_ref.get::<DefaultBrushMarker>(),
            Some(&DefaultBrushMarker(7)),
            "a component every new brush starts with must copy onto the clone"
        );
        assert_eq!(
            spawned_ref.get::<Brush>().map(|b| b.faces.len()),
            Some(new_brush.faces.len()),
            "clone should carry the replacement geometry"
        );
        assert_eq!(
            spawned_ref.get::<Transform>().map(|t| t.translation.x),
            Some(2.0)
        );
    }
}
