use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    camera::RenderTarget,
    core_pipeline::oit::OrderIndependentTransparencySettings,
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

/// Marker for the main 3D viewport camera (layer 0).
#[derive(Component)]
pub struct MainViewportCamera;

const DEFAULT_VIEWPORT_WIDTH: u32 = 1280;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 720;

/// Marker on the center-panel UI node that hosts the 3D viewport.
#[derive(Component)]
pub struct SceneViewport;

/// Bundled queries for converting screen position to a viewport ray.
/// Used by selection, gizmos, modal transforms, and drawing systems.
#[derive(SystemParam)]
pub(crate) struct ViewportCursor<'w, 's> {
    pub camera:
        Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<MainViewportCamera>>,
    pub windows: Query<'w, 's, &'static Window>,
    pub viewport:
        Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<SceneViewport>>,
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
            .init_resource::<CameraBookmarks>()
            .init_resource::<CameraFlyActive>()
            .insert_resource(GlobalAmbientLight::NONE)
            .add_systems(
                OnEnter(crate::AppState::Editor),
                setup_viewport.after(crate::spawn_layout),
            )
            .add_systems(
                Update,
                (update_camera_enabled, camera_bookmark_keys)
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

pub(crate) fn setup_viewport(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    viewport_query: Single<Entity, With<SceneViewport>>,
    assets: Res<AssetServer>,
) {
    // Create render-target image
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
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image.sampler = ImageSampler::linear();
    let image_handle = images.add(image);

    // Spawn 3D camera (marked EditorEntity so it's hidden from hierarchy and undeletable)
    let camera = commands
        .spawn((
            MainViewportCamera,
            crate::EditorEntity,
            Camera3d::default(),
            EnvironmentMapLight {
                diffuse_map: load_embedded_asset!(
                    &*assets,
                    "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
                ),
                specular_map: load_embedded_asset!(
                    &*assets,
                    "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
                ),
                intensity: 500.0,
                ..default()
            },
            // Needed for translucent materials to work correctly
            OrderIndependentTransparencySettings::default(),
            Camera {
                order: -1,
                ..default()
            },
            RenderTarget::Image(image_handle.into()),
            Transform::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
            Msaa::Off,
            JackdawCameraSettings::default(),
        ))
        .id();

    // Spawn infinite grid (marked EditorEntity so it's hidden from hierarchy and undeletable)
    commands.spawn((crate::EditorEntity, InfiniteGridBundle::default()));

    // Attach ViewportNode to the SceneViewport UI entity
    commands
        .entity(*viewport_query)
        .insert(ViewportNode::new(camera))
        .observe(handle_viewport_drop);
}

/// Handle files dropped from the asset browser onto the viewport.
fn handle_viewport_drop(
    event: On<Pointer<DragDrop>>,
    file_items: Query<&FileBrowserItem>,
    parents: Query<&ChildOf>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
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

    // Get cursor position and raycast to ground plane
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };

    let position =
        cursor_to_ground_plane(cursor_pos, camera, cam_tf, &viewport_query).unwrap_or(Vec3::ZERO);

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

/// Raycast from screen cursor to the Y=0 ground plane.
pub(crate) fn cursor_to_ground_plane(
    cursor_pos: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<Vec3> {
    // Convert window cursor to viewport-local coordinates, remapped to camera space
    let viewport_cursor = if let Ok((computed, vp_transform)) = viewport_query.single() {
        let scale = computed.inverse_scale_factor();
        let vp_pos = vp_transform.translation * scale;
        let vp_size = computed.size() * scale;
        let vp_top_left = vp_pos - vp_size / 2.0;
        let local = cursor_pos - vp_top_left;
        // Remap from UI-logical space to camera render-target space
        let target_size = camera.logical_viewport_size().unwrap_or(vp_size);
        local * target_size / vp_size
    } else {
        cursor_pos
    };

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

fn update_camera_enabled(
    windows: Query<&Window>,
    viewport_node: Single<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    mut camera_query: Query<&mut JackdawCameraSettings>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    input_focus: Res<bevy::input_focus::InputFocus>,
    blockers: Query<(), With<crate::BlocksCameraInput>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut fly_state: ResMut<CameraFlyActive>,
) {
    // Track right-click fly state
    if mouse.just_released(MouseButton::Right) {
        fly_state.0 = false;
    }

    let Ok(window) = windows.single() else {
        return;
    };

    let (computed, vp_transform) = *viewport_node;
    let scale = computed.inverse_scale_factor();
    let vp_pos = vp_transform.translation * scale;
    let vp_size = computed.size() * scale;
    let vp_top_left = vp_pos - vp_size / 2.0;
    let vp_bottom_right = vp_pos + vp_size / 2.0;

    let hovered = window.cursor_position().is_some_and(|cursor_pos| {
        cursor_pos.x >= vp_top_left.x
            && cursor_pos.x <= vp_bottom_right.x
            && cursor_pos.y >= vp_top_left.y
            && cursor_pos.y <= vp_bottom_right.y
    });

    // Start fly when right-click begins while hovering the viewport
    if mouse.just_pressed(MouseButton::Right) && hovered {
        fly_state.0 = true;
    }

    let modal_active = modal.active.is_some();
    let text_focused = input_focus.0.is_some();
    let overlay_blocking = !blockers.is_empty();
    let should_enable =
        (hovered || fly_state.0) && !modal_active && !text_focused && !overlay_blocking;

    for mut settings in &mut camera_query {
        settings.enabled = should_enable;
    }
}

#[derive(Resource, Default)]
pub struct CameraBookmarks {
    pub slots: [Option<CameraBookmark>; 9],
}

#[derive(Clone, Copy)]
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
    for mut transform in &mut camera_query {
        let forward = transform.forward().as_vec3();
        transform.translation = target - forward * dist;
        *transform = transform.looking_at(target, Vec3::Y);
    }
    OperatorResult::Finished
}

fn slot_param(params: &OperatorParameters) -> Option<usize> {
    let v = params.as_int("slot")?;
    (0..9).contains(&v).then_some(v as usize)
}

/// Save the camera position to a numbered slot.
///
/// # Parameters
/// - `slot` (`i64`, `0..=8`): which bookmark slot to write.
#[operator(
    id = "viewport.bookmark.save",
    label = "Save Camera Bookmark",
    description = "Save the camera position to a numbered slot."
)]
pub(crate) fn viewport_bookmark_save(
    params: In<OperatorParameters>,
    camera_query: Query<&Transform, With<JackdawCameraSettings>>,
    mut bookmarks: ResMut<CameraBookmarks>,
) -> OperatorResult {
    let Some(slot) = slot_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Ok(transform) = camera_query.single() else {
        return OperatorResult::Cancelled;
    };
    bookmarks.slots[slot] = Some(CameraBookmark {
        transform: *transform,
    });
    OperatorResult::Finished
}

/// Restore the camera to a previously-saved bookmark slot. Cancels if
/// the slot is empty.
///
/// # Parameters
/// - `slot` (`i64`, `0..=8`): which bookmark slot to read.
#[operator(
    id = "viewport.bookmark.load",
    label = "Load Camera Bookmark",
    description = "Restore the camera to a previously-saved slot."
)]
pub(crate) fn viewport_bookmark_load(
    params: In<OperatorParameters>,
    bookmarks: Res<CameraBookmarks>,
    mut camera_query: Query<&mut Transform, With<JackdawCameraSettings>>,
) -> OperatorResult {
    let Some(slot) = slot_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(bookmark) = bookmarks.slots[slot] else {
        return OperatorResult::Cancelled;
    };
    for mut transform in &mut camera_query {
        *transform = bookmark.transform;
    }
    OperatorResult::Finished
}
