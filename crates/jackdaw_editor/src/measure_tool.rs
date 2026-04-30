use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
    ui::UiGlobalTransform,
};
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens;

use crate::{
    JackdawDrawSystems, default_style,
    viewport::{MainViewportCamera, SceneViewport},
    viewport_util::window_to_viewport_cursor,
};

// ── Plugin ──

pub struct MeasureToolPlugin;

impl Plugin for MeasureToolPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MeasureToolState>()
            .init_resource::<MeasureLabelEntities>()
            .init_gizmo_group::<MeasureToolGizmoGroup>()
            .add_systems(Startup, configure_measure_tool_gizmos)
            .add_systems(
                OnEnter(crate::AppState::Editor),
                spawn_measure_label.after(crate::viewport::setup_viewport),
            )
            .add_systems(
                PostUpdate,
                (draw_measure_line, update_measure_labels)
                    .in_set(JackdawDrawSystems)
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

#[derive(Default, Reflect, GizmoConfigGroup)]
struct MeasureToolGizmoGroup;

fn configure_measure_tool_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<MeasureToolGizmoGroup>();
    config.depth_bias = -1.0;
}

// ── State ──

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct MeasureToolState {
    pub active: bool,
    pub initialized: bool,
    has_start: bool,
    start_point: Vec3,
    end_point: Vec3,
}

#[derive(Resource, Default)]
struct MeasureLabelEntities {
    label: Option<Entity>,
}

#[derive(Component)]
struct MeasureLabel;

// ── Extension registration ──

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    use crate::core_extension::CoreExtensionInputContext;
    use bevy_enhanced_input::prelude::Press;

    ctx.entity_mut()
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<MeasureDistanceOp>::new(),
            bindings![(KeyCode::KeyM, Press::default())],
        ));

    ctx.entity_mut()
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<ConfirmMeasureDistanceOp>::new(),
            bindings![(MouseButton::Left, Press::default())],
        ));

    ctx.register_operator::<MeasureDistanceOp>()
        .register_operator::<ConfirmMeasureDistanceOp>()
        .register_menu_entry::<MeasureDistanceOp>(TopLevelMenu::Tools);
}

// ── Operator ──

#[operator(
    id = "tools.measure_distance",
    label = "Measure Distance",
    description = "Click two points in the viewport to measure the distance between them",
    modal = true,
    allows_undo = false,
    cancel = cancel_measure_distance
)]
pub(crate) fn measure_distance(
    _: In<OperatorParameters>,
    mut state: ResMut<MeasureToolState>,
    window: Option<Single<&Window>>,
    camera: Option<Single<(&Camera, &GlobalTransform), With<MainViewportCamera>>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    mut ray_cast: MeshRayCast,
) -> OperatorResult {
    // Outside `AppState::Editor` (e.g. headless tests, project-select
    // screen) the window or main viewport camera don't exist yet.
    // Cancel rather than panic on `Single`'s unwrap.
    let (Some(window), Some(camera)) = (window, camera) else {
        return OperatorResult::Cancelled;
    };
    let (camera, cam_tf) = *camera;

    // Try to get a world-space point under the cursor.
    let current_point = window.cursor_position().and_then(|cursor_pos| {
        let vp_cursor = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)?;
        let ray = camera.viewport_to_world(cam_tf, vp_cursor).ok()?;
        Some(
            raycast_closest_point(ray, &mut ray_cast)
                .or_else(|| ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y))
                .unwrap_or(cam_tf.translation() + *ray.direction * 10.0),
        )
    });

    if !state.initialized {
        // First invocation: enter modal mode. Nothing is drawn until the first
        // confirm click sets the start point.
        let fallback = cam_tf.translation() + cam_tf.forward().as_vec3() * 5.0;
        state.initialized = true;
        state.active = true;
        state.has_start = false;
        state.end_point = current_point.unwrap_or(fallback);
        return OperatorResult::Running;
    }

    if !state.active {
        // Confirm triggered finish; clean up and exit modal.
        state.initialized = false;
        state.has_start = false;
        return OperatorResult::Finished;
    }

    // Track cursor while waiting for the first click or while measuring.
    if let Some(point) = current_point {
        state.end_point = point;
    }

    OperatorResult::Running
}

fn cancel_measure_distance(mut state: ResMut<MeasureToolState>) {
    state.active = false;
    state.initialized = false;
    state.has_start = false;
}

fn measure_tool_active(state: Res<MeasureToolState>) -> bool {
    state.active
}

#[operator(
    id = "tools.measure_distance.confirm",
    label = "Confirm Measurement",
    description = "First click sets the start point, second click finishes",
    is_available = measure_tool_active,
    allows_undo = false,
)]
fn confirm_measure_distance(
    _: In<OperatorParameters>,
    mut state: ResMut<MeasureToolState>,
) -> OperatorResult {
    if !state.active || !state.initialized {
        return OperatorResult::Cancelled;
    }

    if !state.has_start {
        // First click: capture start point and begin showing the line.
        state.has_start = true;
        state.start_point = state.end_point;
        OperatorResult::Running
    } else {
        // Subsequent click: finish the current measurement and immediately
        // start a new one from the current cursor position.
        state.start_point = state.end_point;
        OperatorResult::Running
    }
}

// ── Raycasting helpers ──

fn raycast_closest_point(ray: Ray3d, ray_cast: &mut MeshRayCast) -> Option<Vec3> {
    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
    ray_cast
        .cast_ray(ray, &settings)
        .first()
        .map(|(_, hit_data)| hit_data.point)
}

fn ray_plane_intersection(ray: Ray3d, plane_point: Vec3, plane_normal: Vec3) -> Option<Vec3> {
    let denom = ray.direction.dot(plane_normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_point - ray.origin).dot(plane_normal) / denom;
    if t < 0.0 {
        return None;
    }
    Some(ray.origin + *ray.direction * t)
}

// ── Viewport drawing ──

fn draw_measure_line(mut gizmos: Gizmos<MeasureToolGizmoGroup>, state: Res<MeasureToolState>) {
    if !state.active || !state.has_start {
        return;
    }

    let color = default_style::MEASURE_TOOL_LINE;
    let start = state.start_point;
    let end = state.end_point;

    // Main measurement line
    gizmos.line(start, end, color);

    // Endpoint markers (small crosses)
    for point in [start, end] {
        gizmos.line(
            point - Vec3::X * default_style::MARKER_SIZE,
            point + Vec3::X * default_style::MARKER_SIZE,
            color,
        );
        gizmos.line(
            point - Vec3::Y * default_style::MARKER_SIZE,
            point + Vec3::Y * default_style::MARKER_SIZE,
            color,
        );
        gizmos.line(
            point - Vec3::Z * default_style::MARKER_SIZE,
            point + Vec3::Z * default_style::MARKER_SIZE,
            color,
        );
    }
}

fn spawn_measure_label(
    mut commands: Commands,
    viewport_entity: Single<Entity, With<SceneViewport>>,
    mut label_entities: ResMut<MeasureLabelEntities>,
) {
    let entity = commands
        .spawn((
            MeasureLabel,
            crate::EditorEntity,
            crate::NonSerializable,
            Text::new(""),
            TextFont {
                font_size: tokens::TEXT_SIZE,
                ..default()
            },
            TextColor(default_style::MEASURE_TOOL_LABEL),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ))
        .id();
    commands.entity(*viewport_entity).add_child(entity);
    label_entities.label = Some(entity);
}

fn update_measure_labels(
    state: Res<MeasureToolState>,
    label_entities: Res<MeasureLabelEntities>,
    camera: Single<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_node: Option<Single<&ComputedNode, With<SceneViewport>>>,
    mut label_query: Query<(&mut Text, &mut Node, &mut Visibility), With<MeasureLabel>>,
) {
    let Some(entity) = label_entities.label else {
        return;
    };

    let Ok((mut text_comp, mut node, mut vis)) = label_query.get_mut(entity) else {
        return;
    };

    if !state.active || !state.has_start {
        *vis = Visibility::Hidden;
        return;
    }

    let (camera, cam_tf) = *camera;
    let (vp_node_size, scale) = match &viewport_node {
        Some(node) => (node.size(), node.inverse_scale_factor()),
        None => (Vec2::ONE, 1.0),
    };
    let render_target_size = camera
        .logical_viewport_size()
        .unwrap_or(vp_node_size * scale);

    let start = state.start_point;
    let end = state.end_point;
    let mid = (start + end) / 2.0;
    let dist = start.distance(end);

    text_comp.0 = format!("{:.3} m", dist);

    if let Ok(vp_coords) = camera.world_to_viewport(cam_tf, mid) {
        let ui_pos = vp_coords * vp_node_size / render_target_size * scale;
        node.left = px(ui_pos.x - 4.0);
        node.top = px(ui_pos.y - 7.0);
        *vis = Visibility::Inherited;
    } else {
        *vis = Visibility::Hidden;
    }
}
