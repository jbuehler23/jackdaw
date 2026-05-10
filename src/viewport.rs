use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    camera::{RenderTarget, visibility::RenderLayers},
    core_pipeline::oit::OrderIndependentTransparencySettings,
    gizmos::{GizmoAsset, retained::Gizmo},
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
    ui::{UiGlobalTransform, widget::ViewportNode},
};
use bevy_enhanced_input::prelude::{Press, *};
use bevy_infinite_grid::{InfiniteGridBundle, InfiniteGridPlugin};
use jackdaw_api::prelude::*;
use jackdaw_camera::{JackdawCameraPlugin, JackdawCameraSettings};

use bevy::ecs::system::SystemParam;

use crate::core_extension::CoreExtensionInputContext;
use crate::selection::{Selected, Selection};
use jackdaw_widgets::file_browser::FileBrowserItem;

/// Marker for a 3D viewport camera. With Phase 2 multi-viewport
/// support every viewport panel spawns its own camera carrying this
/// marker, so queries that need *all* viewport cameras (or a specific
/// one selected via [`ActiveViewport`]) iterate them rather than
/// using `Single<>`.
#[derive(Component)]
pub struct MainViewportCamera;

const DEFAULT_VIEWPORT_WIDTH: u32 = 1280;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 720;

/// Marker on a UI node that hosts a 3D viewport (the leaf inside
/// which `ViewportNode` projects a camera's render target). With
/// multi-viewport, each registered `jackdaw.viewport` panel spawns
/// one of these.
#[derive(Component)]
pub struct SceneViewport;

/// Sits on the dock-leaf content entity that hosts a viewport panel.
/// Holds the camera entity that the panel's `ViewportNode` is
/// projecting, so the despawn observer can clean up the camera (and
/// its render-target image) when the panel content is torn down.
#[derive(Component)]
pub(crate) struct ViewportPanelHost {
    pub camera: Entity,
    /// Per-viewport infinite-grid entity. Spawned alongside the camera
    /// on a private `RenderLayers` so each viewport renders its own
    /// grid, oriented to its current view axis. Cleaned up together
    /// with the camera on panel teardown.
    pub grid: Entity,
    /// Per-viewport axis-orientation indicator (the small XYZ gizmo
    /// in the bottom-left). Lives on the same private `RenderLayers`
    /// as the camera so adjacent viewports don't see each other's
    /// indicators leaking through their shared world space.
    pub axis_indicator: Entity,
}

/// Component on the retained-gizmo entity that paints a viewport's
/// axis indicator. `viewport_overlays::draw_coordinate_indicator`
/// reads this to reposition the indicator in front of the camera each
/// frame; the despawn observer removes the entity when its panel is
/// torn down.
#[derive(Component)]
pub struct AxisIndicator {
    pub camera: Entity,
}

/// Shared retained-gizmo asset for the per-viewport axis indicator.
/// One asset, many `Gizmo` entities (one per viewport), each with its
/// own `Transform` + `RenderLayers`.
#[derive(Resource)]
struct AxisIndicatorAsset(Handle<GizmoAsset>);

/// Link from a viewport camera back to its private infinite-grid
/// entity. `view.set_axis` reads this to rotate just the active
/// viewport's grid when the user snaps to top / front / side, so other
/// viewports keep their own orientation.
#[derive(Component)]
pub struct ViewportGrid(pub Entity);

/// Shared counter that hands out a unique [`RenderLayers`] index per
/// viewport. Layer 0 is the default world; layer 1 is reserved for
/// the material preview. Per-viewport grids start at layer 2 so
/// they only render to "their" camera.
#[derive(Resource)]
pub(crate) struct ViewportLayerCounter(usize);

impl Default for ViewportLayerCounter {
    fn default() -> Self {
        Self(1)
    }
}

impl ViewportLayerCounter {
    fn next(&mut self) -> usize {
        self.0 += 1;
        self.0
    }
}

/// Tracks which viewport panel currently has the mouse over it.
///
/// Hover-routed viewport input (camera fly mode, click handling,
/// gizmo hover, etc.) reads this to decide which viewport to act on.
/// Updated each frame by `update_active_viewport`.
///
/// During a right-click fly session the resource keeps pointing at
/// the camera that started the session, even if the cursor strays
/// outside the viewport's bounds, so fly input stays attached to the
/// right viewport until the user releases the mouse.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ActiveViewport {
    /// Camera entity of the currently-hovered viewport.
    pub camera: Option<Entity>,
    /// UI-node entity of the currently-hovered viewport's
    /// `SceneViewport`.
    pub ui_node: Option<Entity>,
}

/// Bundled queries for converting screen position to a viewport ray.
/// Used by selection, gizmos, modal transforms, and drawing systems.
///
/// Multi-viewport-aware: routes via [`ActiveViewport`] (the hovered
/// viewport). Modal operators that need a stable target across frames
/// should capture an entity at start and pass it to [`Self::camera_for`].
#[derive(SystemParam)]
pub(crate) struct ViewportCursor<'w, 's> {
    pub windows: Query<'w, 's, &'static Window>,
    cameras: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<MainViewportCamera>>,
    viewports: Query<
        'w,
        's,
        (
            &'static ComputedNode,
            &'static UiGlobalTransform,
            &'static ViewportNode,
        ),
        With<SceneViewport>,
    >,
    active: Res<'w, ActiveViewport>,
}

impl ViewportCursor<'_, '_> {
    /// The hovered viewport's camera + global transform.
    pub fn camera(&self) -> Option<(&Camera, &GlobalTransform)> {
        let camera_entity = self.active.camera?;
        self.cameras.get(camera_entity).ok()
    }

    /// The hovered viewport's UI-node geometry (for cursor remapping).
    pub fn viewport(&self) -> Option<(&ComputedNode, &UiGlobalTransform)> {
        let ui_entity = self.active.ui_node?;
        self.viewports.get(ui_entity).ok().map(|(c, t, _)| (c, t))
    }

    /// Camera entity of the hovered viewport (for modal capture).
    pub fn camera_entity(&self) -> Option<Entity> {
        self.active.camera
    }

    /// UI-node entity of the hovered viewport (for modal capture).
    pub fn viewport_entity(&self) -> Option<Entity> {
        self.active.ui_node
    }

    /// Look up a specific camera by entity. Used by modal operators
    /// that captured the active viewport at drag-start and want to
    /// keep referring to it across frames regardless of where the
    /// cursor wanders.
    pub fn camera_for(&self, entity: Entity) -> Option<(&Camera, &GlobalTransform)> {
        self.cameras.get(entity).ok()
    }

    /// Look up a specific viewport UI node by entity (companion to
    /// [`Self::camera_for`]).
    pub fn viewport_for(&self, entity: Entity) -> Option<(&ComputedNode, &UiGlobalTransform)> {
        self.viewports.get(entity).ok().map(|(c, t, _)| (c, t))
    }

    /// Convert a window-space cursor position into the camera-space
    /// coordinates of a specific viewport, returning `None` if the
    /// cursor falls outside the viewport's bounds. Used by modal
    /// operators that captured a viewport entity at drag-start.
    pub fn viewport_cursor_for(
        &self,
        camera: &Camera,
        viewport_entity: Entity,
        cursor: Vec2,
    ) -> Option<Vec2> {
        let (computed, vp_tf, _) = self.viewports.get(viewport_entity).ok()?;
        let map = crate::viewport_util::ViewportRemap::new(camera, computed, vp_tf);
        let local = cursor - map.top_left;
        if local.x >= 0.0 && local.y >= 0.0 && local.x <= map.vp_size.x && local.y <= map.vp_size.y
        {
            Some(local * map.remap)
        } else {
            None
        }
    }
}

/// Read-only guard resources checked by many interaction systems before acting.
/// If any guard is active, the system should bail early.
#[derive(SystemParam)]
pub(crate) struct InteractionGuards<'w> {
    pub gizmo_drag: Res<'w, crate::gizmos::GizmoDragState>,
    pub gizmo_hover: Res<'w, crate::gizmos::GizmoHoverState>,
    pub modal: Res<'w, crate::modal_transform::ModalTransformState>,
    pub viewport_drag: Res<'w, crate::modal_transform::ViewportDragState>,
    pub draw_state: Res<'w, crate::draw_brush::DrawBrushState>,
    pub edit_mode: Res<'w, crate::brush::EditMode>,
    pub terrain_edit_mode: Res<'w, crate::terrain::TerrainEditMode>,
}

/// Tracks whether a right-click fly session started inside the viewport.
/// While active, the camera keeps responding even when the cursor leaves the viewport.
#[derive(Resource, Default)]
pub struct CameraFlyActive(pub bool);

pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((JackdawCameraPlugin, InfiniteGridPlugin))
            .init_resource::<CameraFlyActive>()
            .init_resource::<ActiveViewport>()
            .init_resource::<ViewportLayerCounter>()
            .insert_resource(GlobalAmbientLight::NONE)
            .add_systems(Startup, init_axis_indicator_asset)
            .add_systems(
                OnEnter(crate::AppState::Editor),
                // Runs after init_layout so the dock-tree reconciler
                // has had a chance to instantiate `jackdaw.viewport`
                // panels (and the cameras + SceneViewport nodes that
                // come with them) before any global viewport setup.
                setup_viewport.after(crate::init_layout),
            )
            .add_observer(on_viewport_panel_despawn)
            .add_systems(
                Update,
                (
                    update_active_viewport,
                    camera_bookmark_keys,
                    crate::view_ops::axis_view_keys,
                )
                    .in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                Update,
                disable_camera_on_dialog
                    .run_if(in_state(crate::AppState::Editor))
                    .run_if(not(crate::no_dialog_open)),
            );
        embedded_asset!(
            app,
            "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
        );
        embedded_asset!(
            app,
            "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
        );
    }
}

/// One-time global setup for the editor's viewport infrastructure.
///
/// Per-viewport setup (camera, render-target image, `SceneViewport`
/// UI node, infinite grid) lives in [`build_viewport_panel`], which
/// runs each time the dock-tree reconciler instantiates a
/// `jackdaw.viewport` panel.
pub(crate) fn setup_viewport() {}

/// Build a single shared [`GizmoAsset`] containing three world-axis
/// lines (X red, Y green, Z blue) of unit length. Each viewport
/// spawns a [`Gizmo`] entity referencing this handle so the asset
/// content is allocated once and reused.
fn init_axis_indicator_asset(mut commands: Commands, mut assets: ResMut<Assets<GizmoAsset>>) {
    let mut asset = GizmoAsset::default();
    asset.line(Vec3::ZERO, Vec3::X, crate::default_style::AXIS_X);
    asset.line(Vec3::ZERO, Vec3::Y, crate::default_style::AXIS_Y);
    asset.line(Vec3::ZERO, Vec3::Z, crate::default_style::AXIS_Z);
    commands.insert_resource(AxisIndicatorAsset(assets.add(asset)));
}

/// Build closure for the `jackdaw.viewport` `DockWindowDescriptor`.
///
/// Spawns a fresh camera + render-target image + `ViewportNode` for
/// the panel, plus the toolbar/SceneViewport UI bundle as the panel's
/// content. Each registered viewport instance gets its own camera, so
/// quad-view / stacked-viewport / multi-window setups all just work
/// once the user drops more `jackdaw.viewport` panels into the tree.
///
/// The despawn observer on `parent` (via [`ViewportPanelHost`]) cleans
/// up the camera when the panel content is torn down by the reconciler
/// (panel closed, leaf rebuilt due to split, workspace switch, etc.).
pub(crate) fn build_viewport_panel(world: &mut World, parent: Entity) {
    // Allocate a render-target image dedicated to this viewport. The
    // size is a starting point; `ViewportNode` will resize the camera
    // viewport to match the SceneViewport UI node automatically.
    let image_handle = {
        let size = Extent3d {
            width: DEFAULT_VIEWPORT_WIDTH,
            height: DEFAULT_VIEWPORT_HEIGHT,
            depth_or_array_layers: 1,
        };
        let mut image = Image::new_fill(
            size,
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Bgra8UnormSrgb,
            default(),
        );
        image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT;
        image.sampler = ImageSampler::linear();
        world.resource_mut::<Assets<Image>>().add(image)
    };

    let assets = world.resource::<AssetServer>().clone();
    let env_diffuse = load_embedded_asset!(
        &assets,
        "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
    );
    let env_specular = load_embedded_asset!(
        &assets,
        "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
    );

    // Allocate a per-viewport render layer so we can attach an
    // infinite grid that *only* this camera renders. Layer 0 stays
    // in the camera's mask so scene content (default-layer entities)
    // still draws here.
    let viewport_layer = world.resource_mut::<ViewportLayerCounter>().next();
    let camera_layers = RenderLayers::from_layers(&[0, viewport_layer]);
    let grid_layers = RenderLayers::layer(viewport_layer);

    let grid = world
        .spawn((
            crate::EditorEntity,
            InfiniteGridBundle::default(),
            grid_layers.clone(),
        ))
        .id();

    let camera = world
        .spawn((
            MainViewportCamera,
            crate::EditorEntity,
            Camera3d::default(),
            EnvironmentMapLight {
                diffuse_map: env_diffuse,
                specular_map: env_specular,
                intensity: 500.0,
                ..default()
            },
            OrderIndependentTransparencySettings::default(),
            Camera {
                order: -1,
                ..default()
            },
            RenderTarget::Image(image_handle.into()),
            Transform::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
            Msaa::Off,
            JackdawCameraSettings::default(),
            ViewportConfig::default(),
            camera_layers,
            ViewportGrid(grid),
        ))
        .id();

    // Per-viewport axis indicator: a retained-gizmo entity on the
    // same private `RenderLayers` mask as the camera, so the lines
    // never bleed into a sibling viewport with an overlapping
    // world-space frustum. The shared `AxisIndicatorAsset` resource
    // holds the actual line content; only the entity's `Transform`
    // and `RenderLayers` differ across viewports.
    let asset_handle = world.resource::<AxisIndicatorAsset>().0.clone();
    let axis_indicator = world
        .spawn((
            crate::EditorEntity,
            AxisIndicator { camera },
            Gizmo {
                handle: asset_handle,
                depth_bias: -0.5,
                ..default()
            },
            Transform::default(),
            // `Visibility` isn't required by `Gizmo`, but the
            // overlay system that repositions the indicator keys
            // off `&mut Visibility` (so the user-facing
            // `show_coordinate_indicator` toggle can hide it).
            // Without it the query filter excludes this entity,
            // the system never updates its `GlobalTransform`, and
            // the lines render at world origin instead of in front
            // of the camera.
            Visibility::Inherited,
            grid_layers.clone(),
        ))
        .id();

    // Spawn the toolbar + SceneViewport bundle as a child of the
    // panel's content entity. The `viewport_with_toolbar` helper
    // produces a column with the editor toolbar(s) on top and a
    // `SceneViewport` UI node filling the rest; we attach
    // `ViewportNode` to that SceneViewport so its camera renders into
    // the UI node's bounds.
    world.spawn((ChildOf(parent), crate::layout::viewport_with_toolbar()));

    // Find the freshly-spawned SceneViewport that's a descendant of
    // `parent` and attach the camera link plus the drop observer.
    let scene_vp = find_descendant_with::<SceneViewport>(world, parent);
    if let Some(scene_vp) = scene_vp {
        world.entity_mut(scene_vp).insert(ViewportNode::new(camera));
        world.entity_mut(scene_vp).observe(handle_viewport_drop);
    } else {
        warn!("build_viewport_panel: SceneViewport descendant not found under parent");
    }

    // Tag the panel content entity so the despawn observer can find
    // and clean up the camera when the reconciler tears the panel down.
    world.entity_mut(parent).insert(ViewportPanelHost {
        camera,
        grid,
        axis_indicator,
    });
}

/// Walk the descendants of `root` looking for the first entity that
/// has component `T`. Used by [`build_viewport_panel`] to locate the
/// `SceneViewport` node spawned inside the panel content bundle.
fn find_descendant_with<T: Component>(world: &mut World, root: Entity) -> Option<Entity> {
    let mut stack = vec![root];
    let mut q_t = world.query_filtered::<Entity, With<T>>();
    let with_t: std::collections::HashSet<Entity> = q_t.iter(world).collect();
    while let Some(entity) = stack.pop() {
        if with_t.contains(&entity) && entity != root {
            return Some(entity);
        }
        if let Some(children) = world.entity(entity).get::<Children>() {
            stack.extend(children.iter());
        }
    }
    None
}

/// When a viewport panel's content entity is despawned (panel closed,
/// leaf rebuilt by reconciler, workspace switch), tear down the
/// camera that was spawned for it.
pub(crate) fn on_viewport_panel_despawn(
    trigger: On<Despawn, ViewportPanelHost>,
    hosts: Query<&ViewportPanelHost>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    if let Ok(host) = hosts.get(entity) {
        if let Ok(mut ec) = commands.get_entity(host.camera) {
            ec.despawn();
        }
        if let Ok(mut ec) = commands.get_entity(host.grid) {
            ec.despawn();
        }
        if let Ok(mut ec) = commands.get_entity(host.axis_indicator) {
            ec.despawn();
        }
    }
}

/// Handle files dropped from the asset browser onto the viewport.
fn handle_viewport_drop(
    event: On<Pointer<DragDrop>>,
    file_items: Query<&FileBrowserItem>,
    parents: Query<&ChildOf>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    active: Res<ActiveViewport>,
    snap_settings: Res<crate::snapping::SnapSettings>,
    mut commands: Commands,
) {
    // Walk up the hierarchy to find the FileBrowserItem component
    let item = find_ancestor_component(event.dropped, &file_items, &parents);
    let Some(item) = item else {
        return;
    };

    let path_lower = item.path.to_lowercase();
    let is_gltf = path_lower.ends_with(".gltf") || path_lower.ends_with(".glb");
    let is_template = path_lower.ends_with(".template.json");
    let is_jsn = path_lower.ends_with(".jsn");

    if !is_gltf && !is_template && !is_jsn {
        return;
    }

    // Drop targets the viewport currently under the cursor (multi-viewport).
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Some(camera_entity) = active.camera else {
        return;
    };
    let Some(viewport_entity) = active.ui_node else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.get(camera_entity) else {
        return;
    };

    let position =
        cursor_to_ground_plane_for(cursor_pos, camera, cam_tf, viewport_entity, &viewport_query)
            .unwrap_or(Vec3::ZERO);

    let ctrl = false; // No Ctrl check needed for drop placement
    let snapped_pos = snap_settings.snap_translate_vec3_if(position, ctrl);

    let path = item.path.clone();
    if is_jsn {
        commands.queue(move |world: &mut World| {
            crate::entity_templates::instantiate_jsn_prefab(world, &path, snapped_pos);
        });
    } else if is_template {
        commands.queue(move |world: &mut World| {
            crate::entity_templates::instantiate_template(world, &path, snapped_pos);
        });
    } else {
        commands.queue(move |world: &mut World| {
            crate::entity_ops::spawn_gltf_in_world(world, &path, snapped_pos);
        });
    }
}

/// Multi-viewport-aware variant of `cursor_to_ground_plane`: remaps
/// the cursor against a specific viewport UI-node entity instead of
/// querying for "the" viewport. Used by hover-routed systems that
/// already know which viewport the cursor is over.
pub(crate) fn cursor_to_ground_plane_for(
    cursor_pos: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<Vec3> {
    let viewport_cursor = crate::viewport_util::window_to_viewport_cursor_for(
        cursor_pos,
        camera,
        viewport_entity,
        viewport_query,
    )?;
    raycast_to_ground(camera, cam_tf, viewport_cursor)
}

fn raycast_to_ground(
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_cursor: Vec2,
) -> Option<Vec3> {
    let ray = camera.viewport_to_world(cam_tf, viewport_cursor).ok()?;

    // Intersect with Y=0 plane
    if ray.direction.y.abs() < 1e-6 {
        return None; // Ray parallel to ground
    }
    let t = -ray.origin.y / ray.direction.y;
    if t < 0.0 {
        return None; // Ground behind camera
    }
    Some(ray.origin + *ray.direction * t)
}

/// Walk up the entity hierarchy to find a component.
fn find_ancestor_component<'a, C: Component>(
    mut entity: Entity,
    query: &'a Query<&C>,
    parents: &Query<&ChildOf>,
) -> Option<&'a C> {
    loop {
        if let Ok(component) = query.get(entity) {
            return Some(component);
        }
        if let Ok(child_of) = parents.get(entity) {
            entity = child_of.0;
        } else {
            return None;
        }
    }
}

/// Enable/disable camera controls based on viewport hover, modal state, etc.
/// Force-disable camera controls when any dialog is open.
fn disable_camera_on_dialog(mut camera_query: Query<&mut JackdawCameraSettings>) {
    for mut settings in &mut camera_query {
        settings.enabled = false;
    }
}

/// Multi-viewport-aware replacement for the old single-viewport
/// `update_camera_enabled` system. Each frame:
///
/// 1. Scans every `SceneViewport` UI node and finds the one under
///    the cursor (if any).
/// 2. Writes the hovered viewport into [`ActiveViewport`] so other
///    systems can route input by camera entity instead of querying
///    `Single<>` (which would panic with multiple viewports).
/// 3. Sticks during a right-click fly session, so the user can drag
///    outside the viewport's bounds without losing camera control.
/// 4. Sets `JackdawCameraSettings::enabled` so only the active
///    camera responds to fly input; the others stay parked.
fn update_active_viewport(
    windows: Query<&Window>,
    viewports: Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &ViewportNode),
        With<SceneViewport>,
    >,
    mut active: ResMut<ActiveViewport>,
    mut camera_query: Query<(Entity, &mut JackdawCameraSettings)>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    input_focus: Res<bevy::input_focus::InputFocus>,
    blockers: Query<(), With<crate::BlocksCameraInput>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut fly_state: ResMut<CameraFlyActive>,
) {
    if mouse.just_released(MouseButton::Right) {
        fly_state.0 = false;
    }

    let cursor = windows.single().ok().and_then(Window::cursor_position);

    // Find the viewport under the cursor (if any). Multi-viewport
    // setups iterate every SceneViewport panel; first hit wins.
    let mut hovered: Option<(Entity, Entity)> = None; // (ui_node, camera)
    if let Some(cursor) = cursor {
        for (ui_entity, computed, vp_transform, vp_node) in &viewports {
            let scale = computed.inverse_scale_factor();
            let vp_pos = vp_transform.translation * scale;
            let vp_size = computed.size() * scale;
            let top_left = vp_pos - vp_size / 2.0;
            let bottom_right = vp_pos + vp_size / 2.0;
            if cursor.x >= top_left.x
                && cursor.x <= bottom_right.x
                && cursor.y >= top_left.y
                && cursor.y <= bottom_right.y
            {
                hovered = Some((ui_entity, vp_node.camera));
                break;
            }
        }
    }

    // During an active fly session, keep the existing active viewport
    // pinned even when the cursor strays outside its bounds. A normal
    // hover update only takes effect once the user releases RMB.
    if !fly_state.0 {
        active.ui_node = hovered.map(|(ui, _)| ui);
        active.camera = hovered.map(|(_, cam)| cam);
    }

    if mouse.just_pressed(MouseButton::Right) && hovered.is_some() {
        fly_state.0 = true;
    }

    let modal_active = modal.active.is_some();
    let text_focused = input_focus.0.is_some();
    let overlay_blocking = !blockers.is_empty();
    let inputs_clear = !modal_active && !text_focused && !overlay_blocking;

    let target_camera = active.camera;
    let fly_engaged = fly_state.0;

    // Enable fly only on the active camera; disable all others. The
    // fly session also keeps fly enabled when the cursor leaves.
    for (entity, mut settings) in &mut camera_query {
        let is_target = target_camera == Some(entity);
        let should_enable = inputs_clear && (is_target && (hovered.is_some() || fly_engaged));
        if settings.enabled != should_enable {
            settings.enabled = should_enable;
        }
    }
}

/// Per-viewport state owned by each camera entity. Multi-viewport
/// users get one of these per panel, so bookmarks (and future
/// per-viewport overlay toggles, projection mode flags, etc.) don't
/// bleed across panels.
///
/// Inserted by `build_viewport_panel` alongside the camera.
#[derive(Component, Clone, Default, Debug)]
pub struct ViewportConfig {
    /// Numpad-1..9 camera bookmarks. Each slot holds a `Transform`
    /// snapshot the user can return to with the matching numpad key.
    pub bookmarks: [Option<CameraBookmark>; 9],
}

#[derive(Clone, Copy, Debug)]
pub struct CameraBookmark {
    pub transform: Transform,
}

/// Watch for save/load camera bookmark keypresses and dispatch the
/// corresponding op with a `slot` param. BEI bindings can't carry
/// payloads, so the slot index lives in a sidecar trigger system.
fn camera_bookmark_keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    edit_mode: Res<crate::brush::EditMode>,
    selection: Res<Selection>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    mut commands: Commands,
) {
    if modal.active.is_some() {
        return;
    }
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let in_object_mode = *edit_mode == crate::brush::EditMode::Object;
    // Don't shadow edit-mode digit shortcuts when a brush is selected
    // and we're in Object mode (Digit1-4 there switches to Vertex /
    // Edge / Face / Clip).
    let conflicts_with_edit_mode_digits =
        in_object_mode && selection.primary().is_some_and(|e| brushes.contains(e));
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    for (slot, key) in digits.iter().enumerate() {
        if !keyboard.just_pressed(*key) {
            continue;
        }
        if ctrl {
            commands
                .operator(ViewportBookmarkSaveOp::ID)
                .param("slot", slot as i64)
                .call();
        } else if in_object_mode && !conflicts_with_edit_mode_digits {
            commands
                .operator(ViewportBookmarkLoadOp::ID)
                .param("slot", slot as i64)
                .call();
        }
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportFocusSelectedOp>()
        .register_operator::<ViewportBookmarkSaveOp>()
        .register_operator::<ViewportBookmarkLoadOp>();

    let ext = ctx.id();
    ctx.spawn((
        Action::<ViewportFocusSelectedOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::KeyF, Press::default())],
    ));
}

fn has_primary_selection(selection: Res<Selection>) -> bool {
    selection.primary().is_some()
}

/// Center the camera on the selected entity.
#[operator(
    id = "viewport.focus_selected",
    label = "Focus Selected",
    description = "Center the camera on the selected entity.",
    is_available = has_primary_selection
)]
pub(crate) fn viewport_focus_selected(
    _: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    selection: Res<Selection>,
    selected_transforms: Query<&GlobalTransform, With<Selected>>,
    mut camera_query: Query<&mut Transform, With<JackdawCameraSettings>>,
) -> OperatorResult {
    let Some(primary) = selection.primary() else {
        return OperatorResult::Cancelled;
    };
    let Ok(global_tf) = selected_transforms.get(primary) else {
        return OperatorResult::Cancelled;
    };
    let target = global_tf.translation();
    let scale = global_tf.compute_transform().scale;
    let dist = f32::max(scale.length() * 3.0, 5.0);
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut transform) = camera_query.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let forward = transform.forward().as_vec3();
    transform.translation = target - forward * dist;
    *transform = transform.looking_at(target, Vec3::Y);
    OperatorResult::Finished
}

fn slot_param(params: &OperatorParameters) -> Option<usize> {
    let v = params.as_int("slot")?;
    (0..9).contains(&v).then_some(v as usize)
}

/// Save the camera position to a numbered slot.
#[operator(
    id = "viewport.bookmark.save",
    label = "Save Camera Bookmark",
    description = "Save the camera position to a numbered slot.",
    params(slot(i64, doc = "Bookmark slot 0..=8."))
)]
pub(crate) fn viewport_bookmark_save(
    params: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    mut cameras: Query<(&Transform, &mut ViewportConfig), With<JackdawCameraSettings>>,
) -> OperatorResult {
    let Some(slot) = slot_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Ok((transform, mut config)) = cameras.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    config.bookmarks[slot] = Some(CameraBookmark {
        transform: *transform,
    });
    OperatorResult::Finished
}

/// Restore the camera to a previously-saved bookmark slot. Cancels if
/// the slot is empty.
#[operator(
    id = "viewport.bookmark.load",
    label = "Load Camera Bookmark",
    description = "Restore the camera to a previously-saved slot.",
    params(slot(i64, doc = "Bookmark slot 0..=8."))
)]
pub(crate) fn viewport_bookmark_load(
    params: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    mut cameras: Query<(&mut Transform, &ViewportConfig), With<JackdawCameraSettings>>,
) -> OperatorResult {
    let Some(slot) = slot_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Ok((mut transform, config)) = cameras.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(bookmark) = config.bookmarks[slot] else {
        return OperatorResult::Cancelled;
    };
    *transform = bookmark.transform;
    OperatorResult::Finished
}
