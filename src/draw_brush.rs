use crate::core_extension::CoreExtensionInputContext;
use crate::default_style;
use crate::prelude::*;
use crate::{
    EditorEntity,
    brush::{BrushFaceEntity, BrushMaterialPalette},
    commands::{
        CommandGroup, CommandHistory, DespawnEntity, EditorCommand, collect_entity_ids,
        deselect_entities,
    },
    selection::{Selected, Selection},
    snapping::SnapSettings,
    viewport::{MainViewportCamera, SceneViewport},
    viewport_util::window_to_viewport_cursor,
};
use bevy::{
    input_focus::InputFocus,
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
    ui::UiGlobalTransform,
};
use bevy_enhanced_input::prelude::Press;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::{
    brush_planes_to_world, brushes_intersect, clean_degenerate_faces, compute_brush_geometry,
    compute_face_tangent_axes, compute_face_uvs, intersect_brushes, subtract_brush,
    triangulate_face,
};
use jackdaw_jsn::{Brush, BrushFaceData, BrushGroup, BrushPlane};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.entity_mut()
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<ConfirmDrawBrushOp>::new(),
            bindings![(MouseButton::Left, Press::default()),],
        ))
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<ActivateDrawBrushModalOp>::new(),
            bindings![
                (MouseButton::Back, Press::default()),
                (KeyCode::KeyB, Press::default()),
            ],
        ))
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<BrushJoinOp>::new(),
            bindings![KeyCode::KeyJ],
        ))
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<BrushCsgSubtractOp>::new(),
            bindings![KeyCode::KeyK.with_mod_keys(ModKeys::CONTROL)],
        ))
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<BrushCsgIntersectOp>::new(),
            bindings![KeyCode::KeyK.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT)],
        ))
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<BrushExtendFaceToBrushOp>::new(),
            bindings![KeyCode::KeyE.with_mod_keys(ModKeys::CONTROL)],
        ));
    ctx.register_operator::<ActivateDrawBrushModalOp>()
        .register_operator::<AddBrushOp>()
        .register_operator::<ConfirmDrawBrushOp>()
        .register_operator::<BrushJoinOp>()
        .register_operator::<BrushCsgSubtractOp>()
        .register_operator::<BrushCsgIntersectOp>()
        .register_operator::<BrushExtendFaceToBrushOp>()
        .register_menu_entry(MenuEntryDescriptor {
            menu: "Add".to_string(),
            label: ActivateDrawBrushModalOp::LABEL.to_string(),
            operator_id: ActivateDrawBrushModalOp::ID,
        });

    ctx.init_resource::<DrawBrushState>()
        .init_resource::<StableIdCounter>();
}

#[operator(id = "viewport.draw_brush_modal", label = "Draw Brush", cancel = cancel_draw_brush_modal, modal = true)]
pub fn activate_draw_brush_modal(
    _: In<OperatorParameters>,
    mut input_focus: ResMut<InputFocus>,
    mut draw_state: ResMut<DrawBrushState>,
    mut edit_mode: ResMut<crate::brush::EditMode>,
    mut brush_selection: ResMut<crate::brush::BrushSelection>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    if modal.is_none() {
        let mode = DrawMode::Add;
        input_focus.0 = None;

        // Exit brush edit mode if active
        if *edit_mode != crate::brush::EditMode::Object {
            *edit_mode = crate::brush::EditMode::Object;
            brush_selection.entity = None;
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
        }

        draw_state.active = Some(ActiveDraw {
            corner1: Vec3::ZERO,
            corner2: Vec3::ZERO,
            depth: 0.0,
            phase: DrawPhase::PlacingFirstCorner,
            mode,
            plane: DrawPlane {
                origin: Vec3::ZERO,
                normal: Vec3::Y,
                axis_u: Vec3::X,
                axis_v: Vec3::Z,
            },
            extrude_start_cursor: Vec2::ZERO,
            plane_locked: false,
            cursor_on_plane: None,
            append_target: None,
            drag_footprint: false,
            press_screen_pos: None,
            polygon_vertices: Vec::new(),
            polygon_cursor: None,
            diagonal_snap: false,
            cached_face_hit: None,
        });
    }
    if draw_state.active.is_none() {
        return OperatorResult::Finished;
    }
    OperatorResult::Running
}

fn cancel_draw_brush_modal(mut draw_state: ResMut<DrawBrushState>) {
    draw_state.active = None;
}

#[operator(
    id = "draw_brush.confirm",
    label = "Draw Brush (Confirm)",
    description = "Confirms the current draw brush operation",
    is_available = is_in_draw_brush_modal,
    allows_undo = false
)]
fn confirm_draw_brush(
    _: In<OperatorParameters>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(ref mut active) = draw_state.active else {
        return OperatorResult::Cancelled;
    };

    // Verify cursor is in viewport
    let Ok(window) = windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let Ok((camera, _)) = camera_query.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return OperatorResult::Cancelled;
    };

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            if let Some(pos) = active.cursor_on_plane {
                active.corner1 = pos;
                active.corner2 = pos;
                active.phase = DrawPhase::DrawingFootprint;
                active.drag_footprint = true;
                active.press_screen_pos = Some(cursor_pos);
            }
        }
        DrawPhase::DrawingFootprint => {
            if active.drag_footprint {
                return OperatorResult::Cancelled;
            }
            let delta = active.corner2 - active.corner1;
            if delta.dot(active.plane.axis_u).abs() < MIN_FOOTPRINT_SIZE
                || delta.dot(active.plane.axis_v).abs() < MIN_FOOTPRINT_SIZE
            {
                return OperatorResult::Cancelled;
            }
            active.phase = DrawPhase::ExtrudingDepth;
            active.extrude_start_cursor = viewport_cursor;
            active.depth = 0.0;
        }
        DrawPhase::DrawingRotatedWidth => {
            if active.polygon_vertices.len() == 4 {
                let edge1 = (active.polygon_vertices[1] - active.polygon_vertices[0]).length();
                let edge2 = (active.polygon_vertices[3] - active.polygon_vertices[0]).length();
                if edge1 >= MIN_FOOTPRINT_SIZE && edge2 >= MIN_FOOTPRINT_SIZE {
                    active.phase = DrawPhase::ExtrudingDepth;
                    active.extrude_start_cursor = viewport_cursor;
                    active.depth = 0.0;
                }
            }
        }
        DrawPhase::DrawingPolygon => {
            if let Some(cursor) = active.polygon_cursor {
                // Accept all vertices, but skip near-duplicates
                let too_close = active
                    .polygon_vertices
                    .iter()
                    .any(|&v| (v - cursor).length() < 0.05);
                if !too_close {
                    active.polygon_vertices.push(cursor);
                }
            }
        }
        DrawPhase::ExtrudingDepth => {
            if active.depth.abs() < MIN_EXTRUDE_DEPTH {
                return OperatorResult::Cancelled; // No depth, keep extruding
            }
            let active = active.clone();
            draw_state.active = None;
            if !active.polygon_vertices.is_empty() {
                if active.append_target.is_some() {
                    append_to_brush(&active, &mut commands);
                } else {
                    spawn_polygon_brush(&active, &mut commands);
                }
            } else if active.append_target.is_some() {
                append_to_brush(&active, &mut commands);
            } else {
                spawn_drawn_brush(&active, &mut commands);
            }
        }
    }
    OperatorResult::Finished
}

fn is_in_draw_brush_modal(active: ActiveModalQuery) -> bool {
    active.is_operator(ActivateDrawBrushModalOp::ID)
}

#[operator(id = "mesh.add_brush")]
pub fn add_brush(_params: In<OperatorParameters>) -> OperatorResult {
    // TODO: make this add / finalize the geometry that was previewed by the draw model
    // The reason for this operator to exist is to be called by user extensions.
    OperatorResult::Finished
}

const EXTRUDE_DEPTH_SENSITIVITY: f32 = 0.003;
const MIN_FOOTPRINT_SIZE: f32 = 0.01;
const MIN_EXTRUDE_DEPTH: f32 = 0.01;
const MIN_FRAGMENT_SIZE: f32 = 0.005;

/// Stable identifier that survives the snapshot round-trip (undo
/// respawns fresh entity ids; selection is restored by matching
/// on this).
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
#[reflect(Component)]
pub struct BrushStableId(u64);

#[derive(Resource, Default)]
struct StableIdCounter(u64);

impl StableIdCounter {
    fn next(&mut self) -> BrushStableId {
        self.0 += 1;
        BrushStableId(self.0)
    }
}

/// Find the current Entity for a given stable ID, if it exists.
fn entity_by_stable_id(world: &mut World, id: BrushStableId) -> Option<Entity> {
    world
        .query::<(Entity, &BrushStableId)>()
        .iter(world)
        .find(|(_, sid)| **sid == id)
        .map(|(e, _)| e)
}

/// Lazily give every `Brush` a `BrushStableId` so the undo selection-restore
/// path (`apply_ast_to_world`) can match selections across scene reloads.
/// Brushes loaded from JSN carry the serialized id if they had one; fresh
/// draws that didn't insert one explicitly get one here.
fn assign_missing_brush_stable_ids(
    mut commands: Commands,
    mut counter: ResMut<StableIdCounter>,
    brushes: Query<Entity, (With<crate::brush::Brush>, Without<BrushStableId>)>,
) {
    for entity in &brushes {
        let sid = counter.next();
        if let Ok(mut ec) = commands.get_entity(entity) {
            ec.insert(sid);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DrawPhase {
    PlacingFirstCorner,
    DrawingFootprint,
    DrawingRotatedWidth,
    DrawingPolygon,
    ExtrudingDepth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum DrawMode {
    #[default]
    Add,
    Cut,
}

#[derive(Clone, Debug)]
pub(crate) struct DrawPlane {
    pub origin: Vec3,
    pub normal: Vec3,
    pub axis_u: Vec3,
    pub axis_v: Vec3,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveDraw {
    pub corner1: Vec3,
    pub corner2: Vec3,
    pub depth: f32,
    pub phase: DrawPhase,
    pub mode: DrawMode,
    pub plane: DrawPlane,
    pub extrude_start_cursor: Vec2,
    pub plane_locked: bool,
    /// World-space cursor position on the drawing plane (for crosshair preview).
    pub cursor_on_plane: Option<Vec3>,
    /// When set, the drawn shape will be CSG-unioned with this brush instead of spawning a new entity.
    pub append_target: Option<Entity>,
    /// True during press-drag-release rectangle drawing.
    pub drag_footprint: bool,
    /// Screen position at initial press (for drag vs click detection).
    pub press_screen_pos: Option<Vec2>,
    /// Placed polygon vertices in world space (polygon draw mode).
    pub polygon_vertices: Vec<Vec3>,
    /// Current cursor position on plane during polygon mode (for preview edge).
    pub polygon_cursor: Option<Vec3>,
    /// When true, constrain cursor to nearest 45° angle from last vertex.
    pub diagonal_snap: bool,
    /// Last successful face raycast hit point, for plane stickiness when raycast misses near edges.
    pub cached_face_hit: Option<Vec3>,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct DrawBrushState {
    pub(crate) active: Option<ActiveDraw>,
}

/// Minimal data needed to respawn a brush entity.
#[derive(Clone)]
pub(crate) struct BrushData {
    stable_id: BrushStableId,
    brush: Brush,
    transform: Transform,
    name: String,
    parent_stable_id: Option<BrushStableId>,
}

/// Either a single brush or a group containing child brushes.
#[derive(Clone)]
enum BrushOrGroup {
    Single(BrushData),
    Group {
        stable_id: BrushStableId,
        transform: Transform,
        name: String,
        parent_stable_id: Option<BrushStableId>,
        children: Vec<BrushData>,
    },
}

/// Read brush data from an existing entity. Lazily assigns a `BrushStableId` if missing.
pub(crate) fn brush_data_from_entity(world: &mut World, entity: Entity) -> BrushData {
    // Ensure the entity has a stable ID
    let stable_id = if let Some(sid) = world.get::<BrushStableId>(entity) {
        *sid
    } else {
        let sid = world.resource_mut::<StableIdCounter>().next();
        world.entity_mut(entity).insert(sid);
        sid
    };

    // Ensure parent has a stable ID too
    let parent_stable_id = if let Some(child_of) = world.get::<ChildOf>(entity) {
        let parent = child_of.0;
        if let Some(psid) = world.get::<BrushStableId>(parent) {
            Some(*psid)
        } else {
            let psid = world.resource_mut::<StableIdCounter>().next();
            world.entity_mut(parent).insert(psid);
            Some(psid)
        }
    } else {
        None
    };

    BrushData {
        stable_id,
        brush: world.get::<Brush>(entity).unwrap().clone(),
        transform: *world.get::<Transform>(entity).unwrap(),
        name: world
            .get::<Name>(entity)
            .map(std::string::ToString::to_string)
            .unwrap_or_default(),
        parent_stable_id,
    }
}

/// Spawn a brush entity from stored data. Returns new entity ID.
fn spawn_brush_from_data(world: &mut World, data: &BrushData) -> Entity {
    let parent_entity = data
        .parent_stable_id
        .and_then(|psid| entity_by_stable_id(world, psid));

    let mut ec = world.spawn((
        Name::new(data.name.clone()),
        data.brush.clone(),
        data.transform,
        data.stable_id,
        Visibility::default(),
    ));
    if let Some(parent) = parent_entity {
        ec.insert(ChildOf(parent));
    }
    let entity = ec.id();
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// Spawn a brush or group from stored data. Returns top-level entity ID.
fn spawn_brush_or_group(world: &mut World, data: &BrushOrGroup) -> Entity {
    match data {
        BrushOrGroup::Single(brush_data) => spawn_brush_from_data(world, brush_data),
        BrushOrGroup::Group {
            stable_id,
            transform,
            name,
            parent_stable_id,
            children,
        } => {
            let parent_entity = parent_stable_id.and_then(|psid| entity_by_stable_id(world, psid));

            let mut ec = world.spawn((
                Name::new(name.clone()),
                BrushGroup,
                *transform,
                *stable_id,
                Visibility::default(),
            ));
            if let Some(p) = parent_entity {
                ec.insert(ChildOf(p));
            }
            let group_id = ec.id();
            crate::scene_io::register_entity_in_ast(world, group_id);
            for child in children {
                // Children reference the group by the group's stable_id which
                // we just spawned, so spawn_brush_from_data will find it.
                let mut child_data = child.clone();
                child_data.parent_stable_id = Some(*stable_id);
                spawn_brush_from_data(world, &child_data);
            }
            group_id
        }
    }
}

/// Per-command undo entry for brush spawns from the legacy non-
/// operator paths (face extrude, brush clip/split). The draw-brush
/// modal operator doesn't push this — its `SnapshotDiff` covers the
/// whole transaction.
pub(crate) struct CreateBrushCommand {
    pub data: BrushData,
}

impl EditorCommand for CreateBrushCommand {
    fn execute(&mut self, world: &mut World) {
        spawn_brush_from_data(world, &self.data);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = entity_by_stable_id(world, self.data.stable_id) {
            deselect_entities(world, &[entity]);
            if let Ok(entity_mut) = world.get_entity_mut(entity) {
                entity_mut.despawn();
            }
        }
    }

    fn description(&self) -> &str {
        "Draw brush"
    }
}

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct DrawBrushGizmoGroup;

pub struct DrawBrushPlugin;

#[derive(Component)]
struct DrawPreviewMesh;

#[derive(Component)]
pub(crate) struct CutResultPreviewMesh;

/// Per-face data attached to each [`CutResultPreviewMesh`] so the face-grid
/// gizmo systems can render edges and grid lines on cut-preview fragments.
#[derive(Component)]
pub(crate) struct CutPreviewFace {
    pub world_vertices: Vec<Vec3>,
    pub world_normal: Vec3,
    pub is_default_material: bool,
    pub is_cap: bool,
}

/// Marker for brush face entities hidden during cut preview.
#[derive(Component)]
pub(crate) struct CutPreviewHidden;

impl Plugin for DrawBrushPlugin {
    fn build(&self, app: &mut App) {
        // TODO: Move *all* of this into the `extension` method and turn systems into ops on the way.
        app.register_type::<BrushStableId>()
            .init_resource::<StableIdCounter>()
            .add_systems(Update, assign_missing_brush_stable_ids)
            .init_gizmo_group::<DrawBrushGizmoGroup>()
            .add_systems(Startup, configure_draw_brush_gizmos)
            .add_systems(
                Update,
                (
                    draw_brush_activate,
                    draw_brush_update,
                    draw_brush_release,
                    draw_brush_confirm,
                    draw_brush_cancel,
                )
                    .chain()
                    .in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                Update,
                (
                    draw_brush_preview.after(draw_brush_cancel),
                    manage_draw_preview_mesh.after(crate::brush::mesh::regenerate_brush_meshes),
                )
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

fn configure_draw_brush_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DrawBrushGizmoGroup>();
    config.depth_bias = -1.0;
}

fn draw_brush_activate(
    keyboard: Res<ButtonInput<KeyCode>>,
    keybinds: Res<crate::keybinds::KeybindRegistry>,
    input_focus: Res<InputFocus>,
    mut draw_state: ResMut<DrawBrushState>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    mut edit_mode: ResMut<crate::brush::EditMode>,
    mut brush_selection: ResMut<crate::brush::BrushSelection>,
    selection: Res<Selection>,
    brush_query: Query<(), With<Brush>>,
) {
    use crate::keybinds::EditorAction;

    // Handle Tab toggle while in draw mode (works in all phases)
    if let Some(ref mut active) = draw_state.active {
        if keybinds.just_pressed(EditorAction::ToggleDrawMode, &keyboard) {
            active.mode = match active.mode {
                DrawMode::Add => DrawMode::Cut,
                DrawMode::Cut => DrawMode::Add,
            };
        }
        return;
    }

    // B or Mouse4 = draw in Add mode, Alt+B = append to brush, C = draw in Cut mode
    let append = keybinds.just_pressed(EditorAction::AppendToBrush, &keyboard);
    let mode = if append {
        DrawMode::Add
    } else if keybinds.just_pressed(EditorAction::DrawCut, &keyboard) {
        DrawMode::Cut
    } else {
        return;
    };
    // Standard guards
    if input_focus.0.is_some() || modal.active.is_some() {
        return;
    }

    // Only append to selected brush when AppendToBrush matched; otherwise always create new
    let append_target = if mode == DrawMode::Add && append {
        selection.primary().filter(|&e| brush_query.contains(e))
    } else {
        None
    };

    // Exit brush edit mode if active
    if *edit_mode != crate::brush::EditMode::Object {
        *edit_mode = crate::brush::EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }

    draw_state.active = Some(ActiveDraw {
        corner1: Vec3::ZERO,
        corner2: Vec3::ZERO,
        depth: 0.0,
        phase: DrawPhase::PlacingFirstCorner,
        mode,
        plane: DrawPlane {
            origin: Vec3::ZERO,
            normal: Vec3::Y,
            axis_u: Vec3::X,
            axis_v: Vec3::Z,
        },
        extrude_start_cursor: Vec2::ZERO,
        plane_locked: false,
        cursor_on_plane: None,
        append_target,
        drag_footprint: false,
        press_screen_pos: None,
        polygon_vertices: Vec::new(),
        polygon_cursor: None,
        diagonal_snap: false,
        cached_face_hit: None,
    });
}

fn draw_brush_update(
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    snap_settings: Res<SnapSettings>,
    mut ray_cast: MeshRayCast,
    brush_faces: Query<(&BrushFaceEntity, &GlobalTransform)>,
    brushes: Query<(&Brush, &GlobalTransform)>,
) {
    let Some(ref mut active) = draw_state.active else {
        return;
    };

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return;
    };

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    active.diagonal_snap = shift;

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            // Ctrl toggles plane lock
            active.plane_locked = ctrl;

            if !active.plane_locked {
                // Raycast against brush face meshes
                let settings =
                    MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
                let hits = ray_cast.cast_ray(ray, &settings);

                let mut best_hit: Option<(Vec3, Vec3)> = None;
                let mut best_distance = f32::MAX;
                let mut best_facing = f32::MIN;

                for (hit_entity, hit_data) in hits {
                    if let Ok((face_ent, _face_tf)) = brush_faces.get(*hit_entity)
                        && let Ok((brush, brush_tf)) = brushes.get(face_ent.brush_entity)
                    {
                        let face = &brush.faces[face_ent.face_index];
                        let (_, brush_rot, _) = brush_tf.to_scale_rotation_translation();
                        let world_normal = (brush_rot * face.plane.normal).normalize();
                        let camera_facing = (-*ray.direction).dot(world_normal);
                        if camera_facing <= 0.0 {
                            continue;
                        }

                        let dist = hit_data.distance;
                        if dist < best_distance - 0.01 {
                            // Clearly closer, take it.
                            best_hit = Some((hit_data.point, world_normal));
                            best_distance = dist;
                            best_facing = camera_facing;
                        } else if dist < best_distance + 0.01 && camera_facing > best_facing {
                            // Within tolerance, prefer more camera-facing.
                            best_hit = Some((hit_data.point, world_normal));
                            best_facing = camera_facing;
                            best_distance = best_distance.min(dist);
                        }
                    }
                }

                if let Some((hit_point, world_normal)) = best_hit {
                    // Face identified, update plane and cache hit point.
                    active.cached_face_hit = Some(hit_point);
                    let (u, v) = compute_face_tangent_axes(world_normal);
                    let plane = DrawPlane {
                        origin: hit_point,
                        normal: world_normal,
                        axis_u: u,
                        axis_v: v,
                    };
                    let snapped_origin =
                        snap_to_plane_grid(hit_point, &plane, &snap_settings, false);
                    active.plane = DrawPlane {
                        origin: snapped_origin,
                        normal: world_normal,
                        axis_u: u,
                        axis_v: v,
                    };
                } else if active.cached_face_hit.is_some() {
                    // Raycast missed but we recently identified a face.
                    // Project cursor onto cached plane; if still near the face, keep it.
                    if let Some(projected) =
                        ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
                    {
                        let last_hit = active.cached_face_hit.unwrap();
                        let dist = projected.distance(last_hit);
                        if dist > 2.0 {
                            // Cursor has moved well beyond the face, fall back to ground.
                            active.cached_face_hit = None;
                            if let Some(ground_hit) =
                                ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y)
                            {
                                let snapped_origin = snap_settings.snap_translate_vec3(ground_hit);
                                active.plane = DrawPlane {
                                    origin: snapped_origin,
                                    normal: Vec3::Y,
                                    axis_u: Vec3::X,
                                    axis_v: Vec3::Z,
                                };
                            }
                        }
                        // else: keep using cached face plane (cursor still near face)
                    }
                } else {
                    // Never been on a face, fall back to Y=0 ground plane.
                    if let Some(ground_hit) = ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y) {
                        let snapped_origin = snap_settings.snap_translate_vec3(ground_hit);
                        active.plane = DrawPlane {
                            origin: snapped_origin,
                            normal: Vec3::Y,
                            axis_u: Vec3::X,
                            axis_v: Vec3::Z,
                        };
                    }
                }
            }

            // Project cursor onto current plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                active.cursor_on_plane = Some(snapped);
            }
        }
        DrawPhase::DrawingFootprint => {
            // Project cursor onto the locked drawing plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let mut snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                if shift {
                    snapped = snap_to_diagonal(snapped, active.corner1, &active.plane);
                    snapped = snap_to_plane_grid(snapped, &active.plane, &snap_settings, false);
                }
                active.polygon_vertices.clear();
                active.corner2 = snapped;
            }
        }
        DrawPhase::DrawingRotatedWidth => {
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                let line_vec = active.corner2 - active.corner1;
                let line_dir = line_vec.normalize();
                let axis_perp = active.plane.normal.cross(line_dir).normalize();
                let raw_width = (snapped - active.corner1).dot(axis_perp);
                // Snap width to grid
                let width = if snap_settings.translate_active(ctrl)
                    && snap_settings.translate_increment > 0.0
                {
                    (raw_width / snap_settings.translate_increment).round()
                        * snap_settings.translate_increment
                } else {
                    raw_width
                };
                active.polygon_vertices = vec![
                    active.corner1,
                    active.corner2,
                    active.corner2 + axis_perp * width,
                    active.corner1 + axis_perp * width,
                ];
            }
        }
        DrawPhase::DrawingPolygon => {
            // Project cursor onto drawing plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let mut snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                if shift && let Some(&last) = active.polygon_vertices.last() {
                    snapped = snap_to_diagonal(snapped, last, &active.plane);
                    snapped = snap_to_plane_grid(snapped, &active.plane, &snap_settings, false);
                }
                active.polygon_cursor = Some(snapped);
            }
        }
        DrawPhase::ExtrudingDepth => {
            // Use polygon centroid if in polygon mode, otherwise rectangle midpoint
            let center = if !active.polygon_vertices.is_empty() {
                active.polygon_vertices.iter().sum::<Vec3>() / active.polygon_vertices.len() as f32
            } else {
                (active.corner1 + active.corner2) / 2.0
            };
            let cam_dist = (cam_tf.translation() - center).length();

            // Project the plane normal to screen space to determine drag direction
            if let (Ok(origin_screen), Ok(normal_screen)) = (
                camera.world_to_viewport(cam_tf, center),
                camera.world_to_viewport(cam_tf, center + active.plane.normal),
            ) {
                let screen_dir = (normal_screen - origin_screen).normalize_or_zero();
                let mouse_delta = viewport_cursor - active.extrude_start_cursor;
                let projected = mouse_delta.dot(screen_dir);
                let raw_depth = projected * cam_dist * EXTRUDE_DEPTH_SENSITIVITY;

                // Snap depth
                let depth = if snap_settings.translate_active(ctrl)
                    && snap_settings.translate_increment > 0.0
                {
                    (raw_depth / snap_settings.translate_increment).round()
                        * snap_settings.translate_increment
                } else {
                    raw_depth
                };
                active.depth = depth;
            }
        }
    }
}

fn draw_brush_release(
    mouse: Res<ButtonInput<MouseButton>>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) {
    if !mouse.just_released(MouseButton::Left) {
        return;
    }

    let Some(ref mut active) = draw_state.active else {
        return;
    };

    if active.phase != DrawPhase::DrawingFootprint || !active.drag_footprint {
        return;
    }

    let Some(press_pos) = active.press_screen_pos else {
        return;
    };

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, _)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    let screen_dist = (cursor_pos - press_pos).length();
    if screen_dist > 5.0 {
        if active.diagonal_snap {
            // Shift+drag: check line length, transition to rotated width phase
            let line_len = (active.corner2 - active.corner1).length();
            if line_len >= MIN_FOOTPRINT_SIZE {
                active.phase = DrawPhase::DrawingRotatedWidth;
            }
        } else {
            // Normal drag: check footprint size, transition to ExtrudingDepth
            let delta = active.corner2 - active.corner1;
            if delta.dot(active.plane.axis_u).abs() >= MIN_FOOTPRINT_SIZE
                && delta.dot(active.plane.axis_v).abs() >= MIN_FOOTPRINT_SIZE
            {
                active.phase = DrawPhase::ExtrudingDepth;
                active.extrude_start_cursor = viewport_cursor;
                active.depth = 0.0;
            }
        }
    } else {
        // Click (no drag): enter polygon mode
        active.phase = DrawPhase::DrawingPolygon;
        active.polygon_vertices = vec![active.corner1];
        active.drag_footprint = false;
    }
    active.press_screen_pos = None;
}

fn draw_brush_confirm(
    mouse: Res<ButtonInput<MouseButton>>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Some(ref mut active) = draw_state.active else {
        return;
    };
    if active.mode == DrawMode::Add {
        // Already migrated to operator
        return;
    }

    // Verify cursor is in viewport
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, _)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            if let Some(pos) = active.cursor_on_plane {
                active.corner1 = pos;
                active.corner2 = pos;
                active.phase = DrawPhase::DrawingFootprint;
                active.drag_footprint = true;
                active.press_screen_pos = Some(cursor_pos);
            }
        }
        DrawPhase::DrawingFootprint => {
            if active.drag_footprint {
                return;
            }
            let delta = active.corner2 - active.corner1;
            if delta.dot(active.plane.axis_u).abs() < MIN_FOOTPRINT_SIZE
                || delta.dot(active.plane.axis_v).abs() < MIN_FOOTPRINT_SIZE
            {
                return;
            }
            active.phase = DrawPhase::ExtrudingDepth;
            active.extrude_start_cursor = viewport_cursor;
            active.depth = 0.0;
        }
        DrawPhase::DrawingRotatedWidth => {
            if active.polygon_vertices.len() == 4 {
                let edge1 = (active.polygon_vertices[1] - active.polygon_vertices[0]).length();
                let edge2 = (active.polygon_vertices[3] - active.polygon_vertices[0]).length();
                if edge1 >= MIN_FOOTPRINT_SIZE && edge2 >= MIN_FOOTPRINT_SIZE {
                    active.phase = DrawPhase::ExtrudingDepth;
                    active.extrude_start_cursor = viewport_cursor;
                    active.depth = 0.0;
                }
            }
        }
        DrawPhase::DrawingPolygon => {
            if let Some(cursor) = active.polygon_cursor {
                // Accept all vertices, but skip near-duplicates
                let too_close = active
                    .polygon_vertices
                    .iter()
                    .any(|&v| (v - cursor).length() < 0.05);
                if !too_close {
                    active.polygon_vertices.push(cursor);
                }
            }
        }
        DrawPhase::ExtrudingDepth => {
            if active.depth.abs() < MIN_EXTRUDE_DEPTH {
                return; // No depth, keep extruding
            }
            let active_owned = active.clone();
            match active_owned.mode {
                DrawMode::Add => {
                    unreachable!()
                }
                DrawMode::Cut => {
                    subtract_drawn_brush(&active_owned, &mut commands);
                    commands.queue(|world: &mut World| {
                        world.resource_mut::<DrawBrushState>().active = None;
                    });
                }
            }
        }
    }
}

fn draw_brush_cancel(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<crate::keybinds::KeybindRegistry>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) {
    use crate::keybinds::EditorAction;

    let Some(ref mut active) = draw_state.active else {
        return;
    };

    // Polygon mode: Enter closes the polygon (via convex hull) and
    // transitions to ExtrudingDepth, Backspace removes the last
    // placed vertex. Lives outside the operator framework for now
    // and runs in both Add and Cut modes — the Add-mode operator
    // migration stopped short of migrating these keybinds.
    if active.phase == DrawPhase::DrawingPolygon {
        if keybinds.just_pressed(EditorAction::ClosePolygon, &keyboard) {
            let hull = convex_hull_on_plane(&active.polygon_vertices, &active.plane);
            if hull.len() >= 3 {
                active.polygon_vertices = hull;
                let viewport_cursor = (|| {
                    let window = windows.single().ok()?;
                    let cursor_pos = window.cursor_position()?;
                    let (camera, _) = camera_query.single().ok()?;
                    window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
                })();
                active.phase = DrawPhase::ExtrudingDepth;
                active.extrude_start_cursor = viewport_cursor.unwrap_or(Vec2::ZERO);
                active.depth = 0.0;
                return;
            }
        }
        if keybinds.just_pressed(EditorAction::RemoveLastVertex, &keyboard) {
            active.polygon_vertices.pop();
            if active.polygon_vertices.is_empty() {
                active.phase = DrawPhase::PlacingFirstCorner;
            }
            return;
        }
    }

    // Add-mode cancel is handled by the `CancelModalOp` operator
    // (bound to Esc via the core extension) so it goes through the
    // proper modal-finalize path. Right-click-to-cancel is still
    // only wired up for Cut mode.
    if active.mode == DrawMode::Add {
        return;
    }

    if keybinds.just_pressed(EditorAction::CancelDraw, &keyboard)
        || mouse.just_pressed(MouseButton::Right)
    {
        draw_state.active = None;
    }
}

fn draw_brush_preview(
    draw_state: Res<DrawBrushState>,
    snap_settings: Res<SnapSettings>,
    mut gizmos: Gizmos<DrawBrushGizmoGroup>,
    brushes: Query<(&Brush, &GlobalTransform)>,
) {
    let Some(ref active) = draw_state.active else {
        return;
    };

    let color = match active.mode {
        DrawMode::Add => default_style::DRAW_MODE,
        DrawMode::Cut => default_style::CUT_MODE,
    };

    // Highlight the append target brush so the user knows they're in hull mode
    if let Some(target) = active.append_target
        && let Ok((brush, brush_tf)) = brushes.get(target)
    {
        let (verts, polys) = compute_brush_geometry(&brush.faces);
        for polygon in &polys {
            for i in 0..polygon.len() {
                let a = brush_tf.transform_point(verts[polygon[i]]);
                let b = brush_tf.transform_point(verts[polygon[(i + 1) % polygon.len()]]);
                gizmos.line(a, b, default_style::DRAW_MODE);
            }
        }
    }

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            // Crosshair at cursor on surface
            if let Some(pos) = active.cursor_on_plane {
                let size = 0.3;
                gizmos.line(
                    pos - active.plane.axis_u * size,
                    pos + active.plane.axis_u * size,
                    color,
                );
                gizmos.line(
                    pos - active.plane.axis_v * size,
                    pos + active.plane.axis_v * size,
                    color,
                );

                // Draw plane grid overlay
                draw_plane_grid(&mut gizmos, &active.plane, pos, &snap_settings);
            }
        }
        DrawPhase::DrawingFootprint => {
            if active.diagonal_snap {
                // Phase 1: show the line being drawn
                gizmos.line(active.corner1, active.corner2, color);
                let mid = (active.corner1 + active.corner2) / 2.0;
                draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
            } else {
                // Normal axis-aligned rectangle
                let corners = footprint_corners(active);
                for i in 0..4 {
                    gizmos.line(corners[i], corners[(i + 1) % 4], color);
                }
                let mid = (active.corner1 + active.corner2) / 2.0;
                draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
            }
        }
        DrawPhase::DrawingRotatedWidth => {
            if active.polygon_vertices.len() == 4 {
                for i in 0..4 {
                    gizmos.line(
                        active.polygon_vertices[i],
                        active.polygon_vertices[(i + 1) % 4],
                        color,
                    );
                }
                let mid = active.polygon_vertices.iter().sum::<Vec3>() / 4.0;
                draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
            } else {
                // Before first mouse move, show just the locked line
                gizmos.line(active.corner1, active.corner2, color);
            }
        }
        DrawPhase::DrawingPolygon => {
            let verts = &active.polygon_vertices;
            let cursor = active.polygon_cursor;

            // Draw all placed vertices as small spheres
            for &v in verts.iter() {
                gizmos.sphere(Isometry3d::from_translation(v), 0.04, color);
            }

            // Compute and draw the convex hull outline
            let hull = convex_hull_on_plane(verts, &active.plane);
            if hull.len() >= 2 {
                for i in 0..hull.len() {
                    gizmos.line(hull[i], hull[(i + 1) % hull.len()], color);
                }
            }

            // Draw preview edge from last placed vertex to cursor
            if let (Some(&last), Some(cursor_pos)) = (verts.last(), cursor) {
                gizmos.line(last, cursor_pos, color);

                // Crosshair at cursor
                let size = 0.15;
                gizmos.line(
                    cursor_pos - active.plane.axis_u * size,
                    cursor_pos + active.plane.axis_u * size,
                    color,
                );
                gizmos.line(
                    cursor_pos - active.plane.axis_v * size,
                    cursor_pos + active.plane.axis_v * size,
                    color,
                );

                // Draw plane grid centered on cursor
                draw_plane_grid(&mut gizmos, &active.plane, cursor_pos, &snap_settings);
            }
        }
        DrawPhase::ExtrudingDepth => {
            let offset = active.plane.normal * active.depth;

            if !active.polygon_vertices.is_empty() {
                // Polygon prism wireframe
                let verts = &active.polygon_vertices;
                let n = verts.len();
                // Base polygon
                for i in 0..n {
                    gizmos.line(verts[i], verts[(i + 1) % n], color);
                }
                // Top polygon
                for i in 0..n {
                    gizmos.line(verts[i] + offset, verts[(i + 1) % n] + offset, color);
                }
                // Connecting edges
                for &v in verts {
                    gizmos.line(v, v + offset, color);
                }
            } else {
                // Cuboid wireframe
                let base = footprint_corners(active);
                let top: [Vec3; 4] = [
                    base[0] + offset,
                    base[1] + offset,
                    base[2] + offset,
                    base[3] + offset,
                ];
                for i in 0..4 {
                    gizmos.line(base[i], base[(i + 1) % 4], color);
                }
                for i in 0..4 {
                    gizmos.line(top[i], top[(i + 1) % 4], color);
                }
                for i in 0..4 {
                    gizmos.line(base[i], top[i], color);
                }
            }

            let grid_center = if !active.polygon_vertices.is_empty() {
                active.polygon_vertices.iter().sum::<Vec3>() / active.polygon_vertices.len() as f32
            } else {
                (active.corner1 + active.corner2) / 2.0
            };
            draw_plane_grid(&mut gizmos, &active.plane, grid_center, &snap_settings);
        }
    }
}

fn spawn_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let plane = &active.plane;

    // Decompose corners into plane-local u/v coordinates
    let c1_u = (active.corner1 - plane.origin).dot(plane.axis_u);
    let c1_v = (active.corner1 - plane.origin).dot(plane.axis_v);
    let c2_u = (active.corner2 - plane.origin).dot(plane.axis_u);
    let c2_v = (active.corner2 - plane.origin).dot(plane.axis_v);

    let min_u = c1_u.min(c2_u);
    let max_u = c1_u.max(c2_u);
    let min_v = c1_v.min(c2_v);
    let max_v = c1_v.max(c2_v);

    let half_u = (max_u - min_u) / 2.0;
    let half_v = (max_v - min_v) / 2.0;
    let half_depth = active.depth.abs() / 2.0;

    // Center on the plane
    let center_on_plane =
        plane.origin + plane.axis_u * (min_u + max_u) / 2.0 + plane.axis_v * (min_v + max_v) / 2.0;
    let center = center_on_plane + plane.normal * active.depth / 2.0;

    // For ground-plane (normal=Y): axis_u=X, axis_v=Z, normal=Y
    // Brush::cuboid uses half_x, half_y, half_z in local space
    // We need to map: local X -> axis_u, local Y -> normal, local Z -> axis_v
    let brush = Brush::cuboid(half_u, half_depth, half_v);

    // Build rotation that maps local (X,Y,Z) -> (axis_u, normal, axis_v)
    let rotation = if plane.normal == Vec3::Y {
        Quat::IDENTITY
    } else if plane.normal == Vec3::NEG_Y {
        Quat::from_rotation_x(std::f32::consts::PI)
    } else {
        let target_mat = Mat3::from_cols(plane.axis_u, plane.normal, -plane.axis_v);
        Quat::from_mat3(&target_mat)
    };

    commands.queue(move |world: &mut World| {
        // Apply last-used material to all faces
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        let mut brush = brush;
        if let Some(ref mat) = last_mat {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }

        let stable_id = world.resource_mut::<StableIdCounter>().next();
        let entity = world
            .spawn((
                Name::new("Brush"),
                brush,
                Transform {
                    translation: center,
                    rotation,
                    scale: Vec3::ONE,
                },
                Visibility::default(),
                stable_id,
            ))
            .id();

        crate::scene_io::register_entity_in_ast(world, entity);

        // Select the new brush
        {
            // Deselect current selection
            let selection = world.resource::<Selection>();
            let old_selected: Vec<Entity> = selection.entities.clone();
            for &e in &old_selected {
                if let Ok(mut ec) = world.get_entity_mut(e) {
                    ec.remove::<Selected>();
                }
            }
            let mut selection = world.resource_mut::<Selection>();
            selection.entities = vec![entity];
            world.entity_mut(entity).insert(Selected);
        }
    });
}

fn append_to_brush(active: &ActiveDraw, commands: &mut Commands) {
    let Some(target_entity) = active.append_target else {
        return;
    };

    // Build the drawn shape's world-space vertices (prism from polygon or cuboid from footprint)
    let offset = active.plane.normal * active.depth;
    let drawn_verts: Vec<Vec3> = if !active.polygon_vertices.is_empty() {
        let mut verts = Vec::with_capacity(active.polygon_vertices.len() * 2);
        for &v in &active.polygon_vertices {
            verts.push(v);
            verts.push(v + offset);
        }
        verts
    } else {
        let base = footprint_corners(active);
        let mut verts = Vec::with_capacity(8);
        for corner in &base {
            verts.push(*corner);
            verts.push(*corner + offset);
        }
        verts
    };

    commands.queue(move |world: &mut World| {
        use avian3d::parry::math::Point as ParryPoint;
        use avian3d::parry::transformation::convex_hull;

        let Some(brush) = world.get::<Brush>(target_entity) else {
            return;
        };
        let old_brush = brush.clone();

        let Some(global_tf) = world.get::<GlobalTransform>(target_entity) else {
            return;
        };
        let (_, rotation, translation) = global_tf.to_scale_rotation_translation();
        let inv_rotation = rotation.inverse();

        // Get existing brush vertices in local space, then convert drawn verts to local space
        let existing_verts = compute_brush_geometry(&old_brush.faces).0;
        let existing_count = existing_verts.len();

        let mut all_local_verts: Vec<Vec3> = existing_verts;
        for v in &drawn_verts {
            all_local_verts.push(inv_rotation * (*v - translation));
        }

        if all_local_verts.len() < 4 {
            return;
        }

        // Compute convex hull
        let points: Vec<ParryPoint<f32>> = all_local_verts
            .iter()
            .map(|v| ParryPoint::new(v.x, v.y, v.z))
            .collect();
        let (hull_verts, hull_tris) = convex_hull(&points);
        if hull_verts.len() < 4 || hull_tris.is_empty() {
            return;
        }

        let hull_positions: Vec<Vec3> = hull_verts
            .iter()
            .map(|p| Vec3::new(p.x, p.y, p.z))
            .collect();
        let hull_faces = crate::brush::merge_hull_triangles(&hull_positions, &hull_tris);
        if hull_faces.len() < 4 {
            return;
        }

        // Build new face data, matching old faces where possible for texture preservation
        let old_face_polygons = compute_brush_geometry(&old_brush.faces).1;
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();

        let mut new_faces = Vec::with_capacity(hull_faces.len());

        // Map hull vertex indices back to all_local_verts indices
        let hull_to_input: Vec<usize> = hull_positions
            .iter()
            .map(|hp| {
                all_local_verts
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (**a - *hp)
                            .length_squared()
                            .partial_cmp(&(**b - *hp).length_squared())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .collect();

        for hull_face in &hull_faces {
            // Find best matching old face by normal similarity
            let mut best_old = None;
            let mut best_score = -1.0_f32;

            // Check if this face has vertices from the original brush
            let input_verts: Vec<usize> = hull_face
                .vertex_indices
                .iter()
                .map(|&hi| hull_to_input[hi])
                .collect();
            let has_original = input_verts.iter().any(|&i| i < existing_count);

            if has_original {
                for (old_idx, old_polygon) in old_face_polygons.iter().enumerate() {
                    let old_set: std::collections::HashSet<usize> =
                        old_polygon.iter().copied().collect();
                    let overlap = input_verts
                        .iter()
                        .filter(|&&i| i < existing_count && old_set.contains(&i))
                        .count() as f32;
                    let normal_sim = hull_face.normal.dot(old_brush.faces[old_idx].plane.normal);
                    let score = overlap + normal_sim * 0.1;
                    if score > best_score {
                        best_score = score;
                        best_old = Some(old_idx);
                    }
                }
            }

            let face_data = if let Some(old_idx) = best_old {
                let old_face = &old_brush.faces[old_idx];
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    material: old_face.material.clone(),
                    uv_offset: old_face.uv_offset,
                    uv_scale: old_face.uv_scale,
                    uv_rotation: old_face.uv_rotation,
                    uv_u_axis: old_face.uv_u_axis,
                    uv_v_axis: old_face.uv_v_axis,
                    ..default()
                }
            } else {
                // New face from the appended shape, use last-used material.
                let (u, v) = compute_face_tangent_axes(hull_face.normal);
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    material: last_mat.clone().unwrap_or_default(),
                    uv_scale: Vec2::ONE,
                    uv_u_axis: u,
                    uv_v_axis: v,
                    ..default()
                }
            };
            new_faces.push(face_data);
        }

        let new_brush = Brush { faces: new_faces };

        // Apply (ECS + AST). Undo is handled by the enclosing
        // `viewport.draw_brush_modal` operator's snapshot diff; no
        // per-command push needed here.
        crate::brush::sync_brush_to_ast(world, target_entity, &new_brush);
        if let Some(mut brush) = world.get_mut::<Brush>(target_entity) {
            *brush = new_brush.clone();
        }
    });
}

/// Intersect a ray with a plane defined by a point and normal.
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

/// Draw a grid of small crosses on the drawing plane, centered near `center`.
/// Grid points are world-aligned (fixed at world-space multiples of `inc`),
/// so only the visible window moves with the cursor. Individual crosses stay put.
fn draw_plane_grid(
    gizmos: &mut Gizmos<DrawBrushGizmoGroup>,
    plane: &DrawPlane,
    center: Vec3,
    snap_settings: &SnapSettings,
) {
    let inc = snap_settings.grid_size();
    let cross_size = inc * 0.1;
    let range = 10_i32;
    let fade_radius = range as f32 * inc;

    // World-aligned: project center directly onto axes (not relative to plane.origin)
    let u_center = (center.dot(plane.axis_u) / inc).round() as i32;
    let v_center = (center.dot(plane.axis_v) / inc).round() as i32;

    // Distance of the plane from the world origin along its normal
    let plane_d = plane.origin.dot(plane.normal);

    for du in -range..=range {
        for dv in -range..=range {
            let u = (u_center + du) as f32 * inc;
            let v = (v_center + dv) as f32 * inc;
            let pt = plane.axis_u * u + plane.axis_v * v + plane.normal * plane_d;

            // Distance-based alpha fade from cursor
            let dist = (pt - center).length();
            let alpha = (1.0 - dist / fade_radius).clamp(0.0, 0.3);
            if alpha <= 0.0 {
                continue;
            }
            let grid_color = default_style::DRAW_PLANE_GRID.with_alpha(alpha);

            gizmos.line(
                pt - plane.axis_u * cross_size,
                pt + plane.axis_u * cross_size,
                grid_color,
            );
            gizmos.line(
                pt - plane.axis_v * cross_size,
                pt + plane.axis_v * cross_size,
                grid_color,
            );
        }
    }
}

/// Snap a world-space hit point to a world-aligned grid on the drawing plane.
fn snap_to_plane_grid(
    hit: Vec3,
    plane: &DrawPlane,
    snap_settings: &SnapSettings,
    ctrl: bool,
) -> Vec3 {
    if !snap_settings.translate_active(ctrl) || snap_settings.translate_increment <= 0.0 {
        return hit;
    }
    let inc = snap_settings.translate_increment;
    // World-aligned: snap using world-space projections onto axes
    let u = hit.dot(plane.axis_u);
    let v = hit.dot(plane.axis_v);
    let snapped_u = (u / inc).round() * inc;
    let snapped_v = (v / inc).round() * inc;
    let plane_d = plane.origin.dot(plane.normal);
    plane.axis_u * snapped_u + plane.axis_v * snapped_v + plane.normal * plane_d
}

/// Constrain a hit point to the nearest 45° angle from an origin on the drawing plane.
fn snap_to_diagonal(hit: Vec3, origin: Vec3, plane: &DrawPlane) -> Vec3 {
    let delta_u = hit.dot(plane.axis_u) - origin.dot(plane.axis_u);
    let delta_v = hit.dot(plane.axis_v) - origin.dot(plane.axis_v);
    let angle = delta_v.atan2(delta_u);
    let snapped_angle = (angle / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    let distance = (delta_u * delta_u + delta_v * delta_v).sqrt();
    let snapped_u = origin.dot(plane.axis_u) + distance * snapped_angle.cos();
    let snapped_v = origin.dot(plane.axis_v) + distance * snapped_angle.sin();
    let plane_d = plane.origin.dot(plane.normal);
    plane.axis_u * snapped_u + plane.axis_v * snapped_v + plane.normal * plane_d
}

/// Compute the 2D convex hull of coplanar points projected onto the drawing plane.
/// Returns the subset of input points forming the hull, in CCW winding order.
fn convex_hull_on_plane(points: &[Vec3], plane: &DrawPlane) -> Vec<Vec3> {
    if points.len() < 3 {
        return points.to_vec();
    }

    // Project to 2D
    let pts2d: Vec<Vec2> = points
        .iter()
        .map(|p| Vec2::new(p.dot(plane.axis_u), p.dot(plane.axis_v)))
        .collect();

    // Andrew's monotone chain algorithm
    let mut indexed: Vec<usize> = (0..pts2d.len()).collect();
    indexed.sort_by(|&a, &b| {
        pts2d[a]
            .x
            .partial_cmp(&pts2d[b].x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                pts2d[a]
                    .y
                    .partial_cmp(&pts2d[b].y)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

    let mut hull: Vec<usize> = Vec::new();
    // Lower hull
    for &i in &indexed {
        while hull.len() >= 2
            && cross(
                pts2d[hull[hull.len() - 2]],
                pts2d[hull[hull.len() - 1]],
                pts2d[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    // Upper hull
    let lower_len = hull.len() + 1;
    for &i in indexed.iter().rev() {
        while hull.len() >= lower_len
            && cross(
                pts2d[hull[hull.len() - 2]],
                pts2d[hull[hull.len() - 1]],
                pts2d[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    hull.pop(); // remove duplicate of first point

    hull.iter().map(|&i| points[i]).collect()
}

/// Spawn a brush from polygon vertices + extrude depth.
fn spawn_polygon_brush(active: &ActiveDraw, commands: &mut Commands) {
    if active.polygon_vertices.len() < 3 || active.depth.abs() < MIN_EXTRUDE_DEPTH {
        return;
    }

    let polygon = active.polygon_vertices.clone();
    let normal = active.plane.normal;
    let depth = active.depth;

    commands.queue(move |world: &mut World| {
        // Compute centroid + center
        let centroid: Vec3 = polygon.iter().sum::<Vec3>() / polygon.len() as f32;
        let center = centroid + normal * depth / 2.0;

        // Build rotation: local Y = plane normal
        let rotation = if normal == Vec3::Y {
            Quat::IDENTITY
        } else if normal == Vec3::NEG_Y {
            Quat::from_rotation_x(std::f32::consts::PI)
        } else {
            let (u, _v) = compute_face_tangent_axes(normal);
            let target_mat = Mat3::from_cols(u, normal, -normal.cross(u).normalize());
            Quat::from_mat3(&target_mat)
        };
        let inv_rotation = rotation.inverse();

        // Convert polygon vertices to local space
        let local_verts: Vec<Vec3> = polygon
            .iter()
            .map(|&v| inv_rotation * (v - center))
            .collect();

        let Some(mut brush) = Brush::prism(&local_verts, Vec3::Y, depth) else {
            return;
        };

        // Apply last-used material
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        if let Some(ref mat) = last_mat {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }

        let stable_id = world.resource_mut::<StableIdCounter>().next();
        let entity = world
            .spawn((
                Name::new("Brush"),
                brush,
                Transform {
                    translation: center,
                    rotation,
                    scale: Vec3::ONE,
                },
                Visibility::default(),
                stable_id,
            ))
            .id();

        crate::scene_io::register_entity_in_ast(world, entity);

        // Select the new brush
        {
            let selection = world.resource::<Selection>();
            let old_selected: Vec<Entity> = selection.entities.clone();
            for &e in &old_selected {
                if let Ok(mut ec) = world.get_entity_mut(e) {
                    ec.remove::<Selected>();
                }
            }
            let mut selection = world.resource_mut::<Selection>();
            selection.entities = vec![entity];
            world.entity_mut(entity).insert(Selected);
        }
    });
}

fn manage_draw_preview_mesh(
    draw_state: Res<DrawBrushState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    preview_query: Query<Entity, With<DrawPreviewMesh>>,
    result_preview_query: Query<Entity, With<CutResultPreviewMesh>>,
    brushes: Query<(Entity, &Brush, &GlobalTransform, Has<Selected>)>,
    hidden_query: Query<Entity, (With<CutPreviewHidden>, With<Brush>)>,
    mut visibility_query: Query<&mut Visibility>,
    palette: Res<BrushMaterialPalette>,
    mut cached_add_material: Local<Option<Handle<StandardMaterial>>>,
    mut cached_cut_material: Local<Option<Handle<StandardMaterial>>>,

    mut cached_preview_key: Local<Option<(Vec3, Vec3, f32, Vec<Vec3>)>>,
) {
    // Show preview for both Add and Cut during extrude phase
    let should_show = draw_state
        .active
        .as_ref()
        .is_some_and(|a| a.phase == DrawPhase::ExtrudingDepth);

    // Despawn existing preview meshes if we shouldn't show
    if !should_show {
        for entity in preview_query.iter() {
            commands.entity(entity).despawn();
        }
        for entity in result_preview_query.iter() {
            commands.entity(entity).despawn();
        }
        // Restore hidden brush faces
        for entity in hidden_query.iter() {
            if let Ok(mut vis) = visibility_query.get_mut(entity) {
                *vis = Visibility::Inherited;
            }
            commands.entity(entity).remove::<CutPreviewHidden>();
        }
        *cached_preview_key = None;
        return;
    }

    let active = draw_state.active.as_ref().unwrap();

    // Cache check: skip rebuild if cutter hasn't changed and preview entities exist
    let current_key = (
        active.corner1,
        active.corner2,
        active.depth,
        active.polygon_vertices.clone(),
    );
    if let Some(ref prev_key) = *cached_preview_key {
        let same = prev_key.0.abs_diff_eq(current_key.0, 1e-6)
            && prev_key.1.abs_diff_eq(current_key.1, 1e-6)
            && (prev_key.2 - current_key.2).abs() < 1e-6
            && prev_key.3.len() == current_key.3.len()
            && prev_key
                .3
                .iter()
                .zip(current_key.3.iter())
                .all(|(a, b)| a.abs_diff_eq(*b, 1e-6));
        if same
            && (active.mode == DrawMode::Cut || !preview_query.is_empty())
            && (active.mode != DrawMode::Cut || !result_preview_query.is_empty())
        {
            return;
        }
    }
    *cached_preview_key = Some(current_key);

    // Build volume planes based on draw type
    let cutter_planes = if !active.polygon_vertices.is_empty() {
        build_cutter_planes_polygon(active)
    } else {
        build_cutter_planes(active)
    };

    // Compute mesh geometry from planes
    let (verts, face_polys) = compute_brush_geometry(&cutter_planes);
    if verts.len() < 4 {
        for entity in preview_query.iter() {
            commands.entity(entity).despawn();
        }
        for entity in result_preview_query.iter() {
            commands.entity(entity).despawn();
        }
        // Restore hidden brush faces since geometry is invalid
        for entity in hidden_query.iter() {
            if let Ok(mut vis) = visibility_query.get_mut(entity) {
                *vis = Visibility::Inherited;
            }
            commands.entity(entity).remove::<CutPreviewHidden>();
        }
        return;
    }

    // Build triangle mesh from face polygons
    let positions: Vec<[f32; 3]> = verts.iter().map(Vec3::to_array).collect();
    let mut all_indices: Vec<u32> = Vec::new();
    for polygon in &face_polys {
        if polygon.len() < 3 {
            continue;
        }
        let tris = triangulate_face(polygon);
        for tri in &tris {
            all_indices.extend_from_slice(&[tri[0], tri[1], tri[2]]);
        }
    }

    // Compute per-vertex normals by averaging face normals
    let mut normals = vec![[0.0_f32; 3]; positions.len()];
    for (face_idx, polygon) in face_polys.iter().enumerate() {
        if face_idx < cutter_planes.len() {
            let n = cutter_planes[face_idx].plane.normal.to_array();
            for &vi in polygon {
                normals[vi][0] += n[0];
                normals[vi][1] += n[1];
                normals[vi][2] += n[2];
            }
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(all_indices));

    // Mode-dependent material color
    let material = match active.mode {
        DrawMode::Add => cached_add_material.get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: default_style::DRAW_PREVIEW_MESH,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                perceptual_roughness: 1.0,
                ..default()
            })
        }),
        DrawMode::Cut => cached_cut_material.get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: default_style::CUT_PREVIEW_MESH,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                perceptual_roughness: 1.0,
                ..default()
            })
        }),
    };

    // Despawn old preview meshes (do NOT restore hidden faces, they stay hidden while cut is active).
    for entity in preview_query.iter() {
        commands.entity(entity).despawn();
    }
    for entity in result_preview_query.iter() {
        commands.entity(entity).despawn();
    }

    // Spawn solid volume preview for Add mode only.
    if active.mode == DrawMode::Add {
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material.clone()),
            Visibility::Inherited,
            Transform::default(),
            DrawPreviewMesh,
            NotShadowCaster,
            NotShadowReceiver,
            EditorEntity,
        ));
    }

    // In Cut mode, spawn solid result preview meshes for affected brushes
    if active.mode == DrawMode::Cut {
        for (brush_entity, brush, brush_tf, is_selected) in brushes.iter() {
            let (_, rotation, translation) = brush_tf.to_scale_rotation_translation();
            let world_target = brush_planes_to_world(&brush.faces, rotation, translation);

            let intersects = brushes_intersect(&world_target, &cutter_planes);
            if !intersects {
                if hidden_query.get(brush_entity).is_ok() {
                    if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                        *vis = Visibility::Inherited;
                    }
                    commands.entity(brush_entity).remove::<CutPreviewHidden>();
                }
                continue;
            }

            let kept_fragments = subtract_brush(&world_target, &cutter_planes);

            // Only hide the original brush if subtraction produced valid fragments;
            // otherwise the cutter is degenerate (e.g. inverted depth) and we keep
            // the original visible.
            if kept_fragments.is_empty() {
                if hidden_query.get(brush_entity).is_ok() {
                    if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                        *vis = Visibility::Inherited;
                    }
                    commands.entity(brush_entity).remove::<CutPreviewHidden>();
                }
                continue;
            }

            if hidden_query.get(brush_entity).is_err() {
                if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                    *vis = Visibility::Hidden;
                }
                commands.entity(brush_entity).insert(CutPreviewHidden);
            }

            for raw_fragment in &kept_fragments {
                let fragment_faces = clean_degenerate_faces(raw_fragment);
                if fragment_faces.len() < 4 {
                    continue;
                }
                let (frag_verts, frag_polys) = compute_brush_geometry(&fragment_faces);
                if frag_verts.len() < 4 {
                    continue;
                }

                for (face_idx, face_data) in fragment_faces.iter().enumerate() {
                    let indices = &frag_polys[face_idx];
                    if indices.len() < 3 {
                        continue;
                    }

                    let positions: Vec<[f32; 3]> = indices
                        .iter()
                        .map(|&vi| frag_verts[vi].to_array())
                        .collect();
                    let normals: Vec<[f32; 3]> =
                        vec![face_data.plane.normal.to_array(); indices.len()];
                    let (u_axis, v_axis) =
                        if face_data.uv_u_axis != Vec3::ZERO && face_data.uv_v_axis != Vec3::ZERO {
                            (face_data.uv_u_axis, face_data.uv_v_axis)
                        } else {
                            compute_face_tangent_axes(face_data.plane.normal)
                        };
                    let uvs = compute_face_uvs(
                        &frag_verts,
                        indices,
                        u_axis,
                        v_axis,
                        face_data.uv_offset,
                        face_data.uv_scale,
                        face_data.uv_rotation,
                    );

                    let local_tris = triangulate_face(&(0..indices.len()).collect::<Vec<_>>());
                    let flat_indices: Vec<u32> =
                        local_tris.iter().flat_map(|t| t.iter().copied()).collect();

                    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
                    mesh.insert_indices(Indices::U32(flat_indices));

                    let material = if face_data.material != Handle::default() {
                        face_data.material.clone()
                    } else if is_selected {
                        palette.default_selected_material.clone()
                    } else {
                        palette.default_material.clone()
                    };

                    let face_world_verts: Vec<Vec3> =
                        indices.iter().map(|&vi| frag_verts[vi]).collect();

                    commands.spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material),
                        Visibility::Inherited,
                        Transform::default(),
                        CutResultPreviewMesh,
                        CutPreviewFace {
                            world_vertices: face_world_verts,
                            world_normal: face_data.plane.normal,
                            is_default_material: face_data.material == Handle::default(),
                            is_cap: face_data.is_cap,
                        },
                        NotShadowCaster,
                        NotShadowReceiver,
                        EditorEntity,
                    ));
                }
            }
        }
    }
}

/// Compute the 4 world-space corners of the footprint rectangle.
fn footprint_corners(active: &ActiveDraw) -> [Vec3; 4] {
    let plane = &active.plane;
    let c1_u = (active.corner1 - plane.origin).dot(plane.axis_u);
    let c1_v = (active.corner1 - plane.origin).dot(plane.axis_v);
    let c2_u = (active.corner2 - plane.origin).dot(plane.axis_u);
    let c2_v = (active.corner2 - plane.origin).dot(plane.axis_v);

    let min_u = c1_u.min(c2_u);
    let max_u = c1_u.max(c2_u);
    let min_v = c1_v.min(c2_v);
    let max_v = c1_v.max(c2_v);

    [
        plane.origin + plane.axis_u * min_u + plane.axis_v * min_v,
        plane.origin + plane.axis_u * max_u + plane.axis_v * min_v,
        plane.origin + plane.axis_u * max_u + plane.axis_v * max_v,
        plane.origin + plane.axis_u * min_u + plane.axis_v * max_v,
    ]
}

/// Build 6 world-space cutter planes from the `ActiveDraw` cuboid.
fn build_cutter_planes(active: &ActiveDraw) -> Vec<BrushFaceData> {
    let plane = &active.plane;

    let c1_u = (active.corner1 - plane.origin).dot(plane.axis_u);
    let c1_v = (active.corner1 - plane.origin).dot(plane.axis_v);
    let c2_u = (active.corner2 - plane.origin).dot(plane.axis_u);
    let c2_v = (active.corner2 - plane.origin).dot(plane.axis_v);

    let min_u = c1_u.min(c2_u);
    let max_u = c1_u.max(c2_u);
    let min_v = c1_v.min(c2_v);
    let max_v = c1_v.max(c2_v);

    let half_u = (max_u - min_u) / 2.0;
    let half_v = (max_v - min_v) / 2.0;
    let half_depth = active.depth.abs() / 2.0;

    let center_on_plane =
        plane.origin + plane.axis_u * (min_u + max_u) / 2.0 + plane.axis_v * (min_v + max_v) / 2.0;
    let center = center_on_plane + plane.normal * active.depth / 2.0;

    let normals_dists = [
        (plane.axis_u, plane.axis_u.dot(center) + half_u),
        (-plane.axis_u, (-plane.axis_u).dot(center) + half_u),
        (plane.axis_v, plane.axis_v.dot(center) + half_v),
        (-plane.axis_v, (-plane.axis_v).dot(center) + half_v),
        (plane.normal, plane.normal.dot(center) + half_depth),
        (-plane.normal, (-plane.normal).dot(center) + half_depth),
    ];
    normals_dists
        .iter()
        .map(|&(normal, distance)| {
            let (u, v) = compute_face_tangent_axes(normal);
            BrushFaceData {
                plane: BrushPlane { normal, distance },
                uv_scale: Vec2::ONE,
                uv_u_axis: u,
                uv_v_axis: v,
                ..default()
            }
        })
        .collect()
}

/// Build N+2 world-space cutter planes from a polygon prism `ActiveDraw`.
fn build_cutter_planes_polygon(active: &ActiveDraw) -> Vec<BrushFaceData> {
    let verts = &active.polygon_vertices;
    let normal = active.plane.normal;
    let depth = active.depth;
    let half_depth = depth.abs() / 2.0;
    let centroid: Vec3 = verts.iter().sum::<Vec3>() / verts.len() as f32;
    let center = centroid + normal * depth / 2.0;

    let mut faces = Vec::new();

    // Top cap (+normal)
    let (top_u, top_v) = compute_face_tangent_axes(normal);
    faces.push(BrushFaceData {
        plane: BrushPlane {
            normal,
            distance: normal.dot(center) + half_depth,
        },
        uv_scale: Vec2::ONE,
        uv_u_axis: top_u,
        uv_v_axis: top_v,
        ..default()
    });

    // Bottom cap (-normal)
    let (bot_u, bot_v) = compute_face_tangent_axes(-normal);
    faces.push(BrushFaceData {
        plane: BrushPlane {
            normal: -normal,
            distance: (-normal).dot(center) + half_depth,
        },
        uv_scale: Vec2::ONE,
        uv_u_axis: bot_u,
        uv_v_axis: bot_v,
        ..default()
    });

    // Side planes: one per polygon edge
    let n = verts.len();
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        let edge = b - a;
        let mut side_normal = edge.cross(normal).normalize_or_zero();
        if side_normal.length_squared() < 0.5 {
            continue;
        }
        // Ensure outward-facing
        if side_normal.dot(a - centroid) < 0.0 {
            side_normal = -side_normal;
        }
        let distance = side_normal.dot(a);
        let (su, sv) = compute_face_tangent_axes(side_normal);
        faces.push(BrushFaceData {
            plane: BrushPlane {
                normal: side_normal,
                distance,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: su,
            uv_v_axis: sv,
            ..default()
        });
    }

    faces
}

/// If `entity` is a child of a `BrushGroup`, return (`parent_entity`, `parent_translation`).
pub(crate) fn brush_parent_group(world: &World, entity: Entity) -> Option<(Entity, Vec3)> {
    let parent = world.get::<ChildOf>(entity)?.0;
    world.get::<BrushGroup>(parent)?;
    let translation = world.get::<GlobalTransform>(parent)?.translation();
    Some((parent, translation))
}

/// Perform CSG subtraction: subtract the drawn cuboid from all intersecting brushes.
fn subtract_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let cutter_planes = if active.polygon_vertices.is_empty() {
        build_cutter_planes(active)
    } else {
        build_cutter_planes_polygon(active)
    };

    commands.queue(move |world: &mut World| {
        // Phase 1: Collect all brush entities and their data
        let mut query = world.query::<(Entity, &Brush, &GlobalTransform)>();
        let targets: Vec<(Entity, Brush, GlobalTransform)> = query
            .iter(world)
            .map(|(e, b, gt)| (e, b.clone(), *gt))
            .collect();

        // Phase 2: Compute subtractions (pure computation)
        struct SubtractionResult {
            original_entity: Entity,
            fragments: Vec<(Brush, Transform)>,
        }

        let mut results: Vec<SubtractionResult> = Vec::new();

        for (entity, brush, global_transform) in &targets {
            // Transform target planes to world space
            let (_, rotation, translation) = global_transform.to_scale_rotation_translation();
            let world_target = brush_planes_to_world(&brush.faces, rotation, translation);

            // Check intersection
            if !brushes_intersect(&world_target, &cutter_planes) {
                continue;
            }

            // Perform subtraction
            let raw_fragments = subtract_brush(&world_target, &cutter_planes);

            let mut fragment_data: Vec<(Brush, Transform)> = Vec::new();
            for fragment_faces in &raw_fragments {
                // Compute vertices to find centroid (world space)
                let (world_verts, _) = compute_brush_geometry(fragment_faces);
                if world_verts.len() < 4 {
                    continue;
                }
                let bbox_min = world_verts.iter().fold(Vec3::MAX, |a, &b| a.min(b));
                let bbox_max = world_verts.iter().fold(Vec3::MIN, |a, &b| a.max(b));
                let bbox_size = bbox_max - bbox_min;
                if bbox_size.x < MIN_FRAGMENT_SIZE
                    || bbox_size.y < MIN_FRAGMENT_SIZE
                    || bbox_size.z < MIN_FRAGMENT_SIZE
                {
                    continue;
                }
                let centroid: Vec3 = world_verts.iter().sum::<Vec3>() / world_verts.len() as f32;

                // Convert to local space around centroid
                let local_faces: Vec<BrushFaceData> = fragment_faces
                    .iter()
                    .map(|f| BrushFaceData {
                        plane: BrushPlane {
                            normal: f.plane.normal,
                            distance: f.plane.distance - f.plane.normal.dot(centroid),
                        },
                        ..f.clone()
                    })
                    .collect();

                // Clean degenerate faces
                let clean = clean_degenerate_faces(&local_faces);
                if clean.len() < 4 {
                    continue;
                }

                fragment_data.push((
                    Brush { faces: clean },
                    Transform::from_translation(centroid),
                ));
            }

            results.push(SubtractionResult {
                original_entity: *entity,
                fragments: fragment_data,
            });
        }

        if results.is_empty() {
            return;
        }

        // Phase 3: Capture brush data for originals (assigns stable IDs)
        let mut originals: Vec<BrushData> = Vec::new();
        for result in &results {
            originals.push(brush_data_from_entity(world, result.original_entity));
        }

        // Capture parent group info before despawning originals
        // Now using stable IDs: (original_entity -> (parent_stable_id, parent_translation))
        let mut parent_groups: std::collections::HashMap<Entity, (BrushStableId, Vec3)> =
            std::collections::HashMap::new();
        for result in &results {
            if let Some((parent_entity, parent_translation)) =
                brush_parent_group(world, result.original_entity)
            {
                // Ensure the parent group has a stable ID
                let parent_sid = if let Some(sid) = world.get::<BrushStableId>(parent_entity) {
                    *sid
                } else {
                    let sid = world.resource_mut::<StableIdCounter>().next();
                    world.entity_mut(parent_entity).insert(sid);
                    sid
                };
                parent_groups.insert(result.original_entity, (parent_sid, parent_translation));
            }
        }

        // Clean up selection: remove originals that are about to be despawned
        {
            let despawning: Vec<Entity> = results.iter().map(|r| r.original_entity).collect();
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !despawning.contains(e));
        }
        for result in &results {
            if let Ok(mut e) = world.get_entity_mut(result.original_entity) {
                e.remove::<Selected>();
            }
        }

        // Despawn originals
        for result in &results {
            if let Ok(e) = world.get_entity_mut(result.original_entity) {
                e.despawn();
            }
        }

        // Spawn fragments and build BrushOrGroup data
        let mut fragments: Vec<BrushOrGroup> = Vec::new();
        let mut counter = world.resource_mut::<StableIdCounter>();
        // Pre-allocate stable IDs for all new fragments
        let fragment_stable_ids: Vec<Vec<BrushStableId>> = results
            .iter()
            .map(|r| r.fragments.iter().map(|_| counter.next()).collect())
            .collect();
        let group_stable_ids: Vec<Option<BrushStableId>> = results
            .iter()
            .map(|r| {
                if r.fragments.len() > 1 && !parent_groups.contains_key(&r.original_entity) {
                    Some(counter.next())
                } else {
                    None
                }
            })
            .collect();

        for (result_idx, result) in results.iter().enumerate() {
            if let Some(&(parent_sid, parent_translation)) =
                parent_groups.get(&result.original_entity)
            {
                // Fragments stay in existing parent group
                for (frag_idx, (brush, transform)) in result.fragments.iter().enumerate() {
                    let brush_data = BrushData {
                        stable_id: fragment_stable_ids[result_idx][frag_idx],
                        brush: brush.clone(),
                        transform: Transform::from_translation(
                            transform.translation - parent_translation,
                        ),
                        name: "Brush".to_string(),
                        parent_stable_id: Some(parent_sid),
                    };
                    spawn_brush_from_data(world, &brush_data);
                    fragments.push(BrushOrGroup::Single(brush_data));
                }
            } else if result.fragments.len() == 1 {
                // Single fragment: spawn standalone
                let (brush, transform) = &result.fragments[0];
                let brush_data = BrushData {
                    stable_id: fragment_stable_ids[result_idx][0],
                    brush: brush.clone(),
                    transform: *transform,
                    name: "Brush".to_string(),
                    parent_stable_id: None,
                };
                spawn_brush_from_data(world, &brush_data);
                fragments.push(BrushOrGroup::Single(brush_data));
            } else if result.fragments.len() > 1 {
                // Multiple fragments: group under a BrushGroup parent
                let group_center = result
                    .fragments
                    .iter()
                    .map(|(_, tf)| tf.translation)
                    .sum::<Vec3>()
                    / result.fragments.len() as f32;

                let group_sid = group_stable_ids[result_idx].unwrap();
                let children: Vec<BrushData> = result
                    .fragments
                    .iter()
                    .enumerate()
                    .map(|(frag_idx, (brush, transform))| BrushData {
                        stable_id: fragment_stable_ids[result_idx][frag_idx],
                        brush: brush.clone(),
                        transform: Transform::from_translation(
                            transform.translation - group_center,
                        ),
                        name: "Brush".to_string(),
                        parent_stable_id: None, // filled in by spawn_brush_or_group
                    })
                    .collect();

                let group_data = BrushOrGroup::Group {
                    stable_id: group_sid,
                    transform: Transform::from_translation(group_center),
                    name: "Brush Group".to_string(),
                    parent_stable_id: None,
                    children,
                };
                spawn_brush_or_group(world, &group_data);
                fragments.push(group_data);
            }
        }

        // Push undo command
        let cmd = SubtractBrushCommand {
            originals,
            fragments,
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(cmd));
    });
}

struct SubtractBrushCommand {
    originals: Vec<BrushData>,
    fragments: Vec<BrushOrGroup>,
}

impl SubtractBrushCommand {
    /// Resolve the stable ID of a `BrushOrGroup` to its current entity.
    fn fragment_stable_id(data: &BrushOrGroup) -> BrushStableId {
        match data {
            BrushOrGroup::Single(d) => d.stable_id,
            BrushOrGroup::Group { stable_id, .. } => *stable_id,
        }
    }
}

impl EditorCommand for SubtractBrushCommand {
    fn execute(&mut self, world: &mut World) {
        // Despawn originals by stable ID lookup
        let orig_entities: Vec<Entity> = self
            .originals
            .iter()
            .filter_map(|d| entity_by_stable_id(world, d.stable_id))
            .collect();
        deselect_entities(world, &orig_entities);
        for entity in &orig_entities {
            if let Ok(e) = world.get_entity_mut(*entity) {
                e.despawn();
            }
        }
        // Spawn fragments (stable IDs are reassigned from stored data)
        for data in &self.fragments {
            spawn_brush_or_group(world, data);
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Despawn fragments by stable ID lookup
        let mut all_entities = Vec::new();
        for data in &self.fragments {
            let sid = Self::fragment_stable_id(data);
            if let Some(entity) = entity_by_stable_id(world, sid) {
                collect_entity_ids(world, entity, &mut all_entities);
            }
        }
        deselect_entities(world, &all_entities);
        for data in &self.fragments {
            let sid = Self::fragment_stable_id(data);
            if let Some(entity) = entity_by_stable_id(world, sid)
                && let Ok(e) = world.get_entity_mut(entity)
            {
                e.despawn();
            }
        }
        // Respawn originals (stable IDs are reassigned from stored data)
        for data in &self.originals {
            spawn_brush_from_data(world, data);
        }
    }

    fn description(&self) -> &str {
        "Subtract brush"
    }
}

/// Core logic for Join (convex merge). Callable from both keyboard shortcut and menu.
pub(crate) fn join_selected_brushes_impl(world: &mut World) {
    let candidates: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let mut brush_query = world.query::<&Brush>();
    let selected_brushes: Vec<Entity> = candidates
        .into_iter()
        .filter(|&e| brush_query.get(world, e).is_ok())
        .collect();
    if selected_brushes.len() < 2 {
        return;
    }

    let primary_entity = selected_brushes[0];
    let others: Vec<Entity> = selected_brushes[1..].to_vec();

    {
        use avian3d::parry::math::Point as ParryPoint;
        use avian3d::parry::transformation::convex_hull;

        // Read primary brush data
        let Some(primary_brush) = world.get::<Brush>(primary_entity) else {
            return;
        };
        let old_primary_brush = primary_brush.clone();

        let Some(primary_gtf) = world.get::<GlobalTransform>(primary_entity) else {
            return;
        };
        let (_, rotation, translation) = primary_gtf.to_scale_rotation_translation();
        let inv_rotation = rotation.inverse();

        // Gather all vertices in primary's local space
        let existing_verts = compute_brush_geometry(&old_primary_brush.faces).0;
        let existing_count = existing_verts.len();
        let mut all_local_verts: Vec<Vec3> = existing_verts;

        // Gather vertices from other brushes, converted to primary's local space
        for &other in &others {
            let Some(other_brush) = world.get::<Brush>(other) else {
                continue;
            };
            let Some(other_gtf) = world.get::<GlobalTransform>(other) else {
                continue;
            };
            let (other_verts, _) = compute_brush_geometry(&other_brush.faces);
            for v in &other_verts {
                let world_pos = other_gtf.transform_point(*v);
                all_local_verts.push(inv_rotation * (world_pos - translation));
            }
        }

        if all_local_verts.len() < 4 {
            return;
        }

        // Compute convex hull
        let points: Vec<ParryPoint<f32>> = all_local_verts
            .iter()
            .map(|v| ParryPoint::new(v.x, v.y, v.z))
            .collect();
        let (hull_verts, hull_tris) = convex_hull(&points);
        if hull_verts.len() < 4 || hull_tris.is_empty() {
            return;
        }

        let hull_positions: Vec<Vec3> = hull_verts
            .iter()
            .map(|p| Vec3::new(p.x, p.y, p.z))
            .collect();
        let hull_faces = crate::brush::merge_hull_triangles(&hull_positions, &hull_tris);
        if hull_faces.len() < 4 {
            return;
        }

        // Build new face data, matching old primary faces where possible
        let old_face_polygons = compute_brush_geometry(&old_primary_brush.faces).1;
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();

        let hull_to_input: Vec<usize> = hull_positions
            .iter()
            .map(|hp| {
                all_local_verts
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (**a - *hp)
                            .length_squared()
                            .partial_cmp(&(**b - *hp).length_squared())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .collect();

        let mut new_faces = Vec::with_capacity(hull_faces.len());
        for hull_face in &hull_faces {
            let input_verts: Vec<usize> = hull_face
                .vertex_indices
                .iter()
                .map(|&hi| hull_to_input[hi])
                .collect();
            let has_original = input_verts.iter().any(|&i| i < existing_count);

            let mut best_old = None;
            let mut best_score = -1.0_f32;

            if has_original {
                for (old_idx, old_polygon) in old_face_polygons.iter().enumerate() {
                    let old_set: std::collections::HashSet<usize> =
                        old_polygon.iter().copied().collect();
                    let overlap = input_verts
                        .iter()
                        .filter(|&&i| i < existing_count && old_set.contains(&i))
                        .count() as f32;
                    let normal_sim = hull_face
                        .normal
                        .dot(old_primary_brush.faces[old_idx].plane.normal);
                    let score = overlap + normal_sim * 0.1;
                    if score > best_score {
                        best_score = score;
                        best_old = Some(old_idx);
                    }
                }
            }

            let face_data = if let Some(old_idx) = best_old {
                let old_face = &old_primary_brush.faces[old_idx];
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    material: old_face.material.clone(),
                    uv_offset: old_face.uv_offset,
                    uv_scale: old_face.uv_scale,
                    uv_rotation: old_face.uv_rotation,
                    uv_u_axis: old_face.uv_u_axis,
                    uv_v_axis: old_face.uv_v_axis,
                    ..default()
                }
            } else {
                let (u, v) = compute_face_tangent_axes(hull_face.normal);
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    material: last_mat.clone().unwrap_or_default(),
                    uv_scale: Vec2::ONE,
                    uv_u_axis: u,
                    uv_v_axis: v,
                    ..default()
                }
            };
            new_faces.push(face_data);
        }

        let new_brush = Brush { faces: new_faces };

        // Snapshot others before despawning (for undo)
        let mut undo_commands: Vec<Box<dyn EditorCommand>> = Vec::new();

        // SetBrush for primary
        undo_commands.push(Box::new(crate::brush::SetBrush {
            entity: primary_entity,
            old: old_primary_brush,
            new: new_brush.clone(),
            label: "Join brushes".to_string(),
        }));

        // Snapshot and despawn each other brush
        for &other in &others {
            undo_commands.push(Box::new(DespawnEntity::from_world(world, other)));
        }

        // Apply: update primary brush (ECS + AST)
        crate::brush::sync_brush_to_ast(world, primary_entity, &new_brush);
        if let Some(mut brush) = world.get_mut::<Brush>(primary_entity) {
            *brush = new_brush;
        }

        // Deselect entities before despawning so that `On<Remove, Selected>`
        // observers can clean up tree-row UI while the entities still exist.
        for &other in &others {
            if let Ok(mut ec) = world.get_entity_mut(other) {
                ec.remove::<Selected>();
            }
        }
        {
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !others.contains(e));
        }

        // Despawn others
        for &other in &others {
            if let Ok(entity_mut) = world.get_entity_mut(other) {
                entity_mut.despawn();
            }
        }

        // Push grouped undo command
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(CommandGroup {
            commands: undo_commands,
            label: "Join brushes".to_string(),
        }));
    }
}

/// Core logic for CSG Subtract. Selected brushes are cutters, non-selected are targets.
pub(crate) fn csg_subtract_selected_impl(world: &mut World) {
    let selection = world.resource::<Selection>();
    let selected_set: Vec<Entity> = selection.entities.clone();

    let mut brush_query = world.query::<(Entity, &Brush, &GlobalTransform)>();
    let all_brushes: Vec<(Entity, Brush, GlobalTransform)> = brush_query
        .iter(world)
        .map(|(e, b, gt)| (e, b.clone(), *gt))
        .collect();

    // Cutters = selected brushes, targets = non-selected brushes
    let cutters: Vec<&(Entity, Brush, GlobalTransform)> = all_brushes
        .iter()
        .filter(|(e, _, _)| selected_set.contains(e))
        .collect();
    let targets: Vec<&(Entity, Brush, GlobalTransform)> = all_brushes
        .iter()
        .filter(|(e, _, _)| !selected_set.contains(e))
        .collect();

    if cutters.is_empty() || targets.is_empty() {
        return;
    }

    // Transform cutter faces to world space
    let cutter_world_faces: Vec<Vec<BrushFaceData>> = cutters
        .iter()
        .map(|(_, brush, gt)| {
            let (_, rotation, translation) = gt.to_scale_rotation_translation();
            brush_planes_to_world(&brush.faces, rotation, translation)
        })
        .collect();

    // For each target, check intersection with each cutter and subtract
    struct SubtractionResult {
        original_entity: Entity,
        fragments: Vec<(Brush, Transform)>,
    }

    let mut results: Vec<SubtractionResult> = Vec::new();

    for (entity, brush, global_transform) in &targets {
        let entity = *entity;
        let (_, rotation, translation) = global_transform.to_scale_rotation_translation();
        let world_target = brush_planes_to_world(&brush.faces, rotation, translation);

        // Iteratively subtract each cutter from the target fragments
        let mut current_fragments: Vec<Vec<BrushFaceData>> = vec![world_target];

        for cutter_faces in &cutter_world_faces {
            let mut next_fragments = Vec::new();
            for fragment in &current_fragments {
                if brushes_intersect(fragment, cutter_faces) {
                    let pieces = subtract_brush(fragment, cutter_faces);
                    next_fragments.extend(pieces);
                } else {
                    next_fragments.push(fragment.clone());
                }
            }
            current_fragments = next_fragments;
        }

        // Check if anything was actually cut (same number of fragments with same face count = no cut)
        if current_fragments.len() == 1 && current_fragments[0].len() == brush.faces.len() {
            let orig_world = brush_planes_to_world(&brush.faces, rotation, translation);
            if current_fragments[0].len() == orig_world.len() {
                let all_same = current_fragments[0]
                    .iter()
                    .zip(orig_world.iter())
                    .all(|(a, b)| {
                        (a.plane.normal - b.plane.normal).length() < 1e-3
                            && (a.plane.distance - b.plane.distance).abs() < 1e-3
                    });
                if all_same {
                    continue;
                }
            }
        }

        // Convert world-space fragments to local-space brushes
        let mut fragment_data: Vec<(Brush, Transform)> = Vec::new();
        for fragment_faces in &current_fragments {
            let (world_verts, _) = compute_brush_geometry(fragment_faces);
            if world_verts.len() < 4 {
                continue;
            }
            let centroid: Vec3 = world_verts.iter().sum::<Vec3>() / world_verts.len() as f32;

            let local_faces: Vec<BrushFaceData> = fragment_faces
                .iter()
                .map(|f| BrushFaceData {
                    plane: BrushPlane {
                        normal: f.plane.normal,
                        distance: f.plane.distance - f.plane.normal.dot(centroid),
                    },
                    ..f.clone()
                })
                .collect();

            let clean = clean_degenerate_faces(&local_faces);
            if clean.len() < 4 {
                continue;
            }

            fragment_data.push((
                Brush { faces: clean },
                Transform::from_translation(centroid),
            ));
        }

        results.push(SubtractionResult {
            original_entity: entity,
            fragments: fragment_data,
        });
    }

    if results.is_empty() {
        return;
    }

    // Capture brush data for originals (assigns stable IDs)
    let mut originals: Vec<BrushData> = Vec::new();
    for result in &results {
        originals.push(brush_data_from_entity(world, result.original_entity));
    }

    // Capture parent group info before despawning originals
    let mut parent_groups: std::collections::HashMap<Entity, (BrushStableId, Vec3)> =
        std::collections::HashMap::new();
    for result in &results {
        if let Some((parent_entity, parent_translation)) =
            brush_parent_group(world, result.original_entity)
        {
            let parent_sid = if let Some(sid) = world.get::<BrushStableId>(parent_entity) {
                *sid
            } else {
                let sid = world.resource_mut::<StableIdCounter>().next();
                world.entity_mut(parent_entity).insert(sid);
                sid
            };
            parent_groups.insert(result.original_entity, (parent_sid, parent_translation));
        }
    }

    // Clean up selection: remove targets about to be despawned
    {
        let despawning: Vec<Entity> = results.iter().map(|r| r.original_entity).collect();
        let mut selection = world.resource_mut::<Selection>();
        selection.entities.retain(|e| !despawning.contains(e));
    }
    for result in &results {
        if let Ok(mut e) = world.get_entity_mut(result.original_entity) {
            e.remove::<Selected>();
        }
    }

    // Despawn originals
    for result in &results {
        if let Ok(e) = world.get_entity_mut(result.original_entity) {
            e.despawn();
        }
    }

    // Spawn fragments and build BrushOrGroup data
    let mut fragments: Vec<BrushOrGroup> = Vec::new();
    let mut counter = world.resource_mut::<StableIdCounter>();
    let fragment_stable_ids: Vec<Vec<BrushStableId>> = results
        .iter()
        .map(|r| r.fragments.iter().map(|_| counter.next()).collect())
        .collect();
    let group_stable_ids: Vec<Option<BrushStableId>> = results
        .iter()
        .map(|r| {
            if r.fragments.len() > 1 && !parent_groups.contains_key(&r.original_entity) {
                Some(counter.next())
            } else {
                None
            }
        })
        .collect();

    for (result_idx, result) in results.iter().enumerate() {
        if let Some(&(parent_sid, parent_translation)) = parent_groups.get(&result.original_entity)
        {
            for (frag_idx, (brush, transform)) in result.fragments.iter().enumerate() {
                let brush_data = BrushData {
                    stable_id: fragment_stable_ids[result_idx][frag_idx],
                    brush: brush.clone(),
                    transform: Transform::from_translation(
                        transform.translation - parent_translation,
                    ),
                    name: "Brush".to_string(),
                    parent_stable_id: Some(parent_sid),
                };
                spawn_brush_from_data(world, &brush_data);
                fragments.push(BrushOrGroup::Single(brush_data));
            }
        } else if result.fragments.len() == 1 {
            let (brush, transform) = &result.fragments[0];
            let brush_data = BrushData {
                stable_id: fragment_stable_ids[result_idx][0],
                brush: brush.clone(),
                transform: *transform,
                name: "Brush".to_string(),
                parent_stable_id: None,
            };
            spawn_brush_from_data(world, &brush_data);
            fragments.push(BrushOrGroup::Single(brush_data));
        } else if result.fragments.len() > 1 {
            let group_center = result
                .fragments
                .iter()
                .map(|(_, tf)| tf.translation)
                .sum::<Vec3>()
                / result.fragments.len() as f32;

            let group_sid = group_stable_ids[result_idx].unwrap();
            let children: Vec<BrushData> = result
                .fragments
                .iter()
                .enumerate()
                .map(|(frag_idx, (brush, transform))| BrushData {
                    stable_id: fragment_stable_ids[result_idx][frag_idx],
                    brush: brush.clone(),
                    transform: Transform::from_translation(transform.translation - group_center),
                    name: "Brush".to_string(),
                    parent_stable_id: None,
                })
                .collect();

            let group_data = BrushOrGroup::Group {
                stable_id: group_sid,
                transform: Transform::from_translation(group_center),
                name: "Brush Group".to_string(),
                parent_stable_id: None,
                children,
            };
            spawn_brush_or_group(world, &group_data);
            fragments.push(group_data);
        }
    }

    // Push undo command
    let cmd = SubtractBrushCommand {
        originals,
        fragments,
    };
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
}

/// Core logic for CSG Intersect. Replaces all selected brushes with their intersection.
pub(crate) fn csg_intersect_selected_impl(world: &mut World) {
    let selection = world.resource::<Selection>();
    let selected_set: Vec<Entity> = selection.entities.clone();

    let mut brush_query = world.query::<(Entity, &Brush, &GlobalTransform)>();
    let selected_brushes: Vec<(Entity, Brush, GlobalTransform)> = brush_query
        .iter(world)
        .filter(|(e, _, _)| selected_set.contains(e))
        .map(|(e, b, gt)| (e, b.clone(), *gt))
        .collect();

    if selected_brushes.len() < 2 {
        return;
    }

    // Transform all faces to world space
    let world_face_sets: Vec<Vec<BrushFaceData>> = selected_brushes
        .iter()
        .map(|(_, brush, gt)| {
            let (_, rotation, translation) = gt.to_scale_rotation_translation();
            brush_planes_to_world(&brush.faces, rotation, translation)
        })
        .collect();

    let face_refs: Vec<&[BrushFaceData]> = world_face_sets
        .iter()
        .map(std::vec::Vec::as_slice)
        .collect();
    let Some(intersection_faces) = intersect_brushes(&face_refs) else {
        return;
    };
    if intersection_faces.len() < 4 {
        return;
    }

    // Compute centroid for the result
    let (world_verts, _) = compute_brush_geometry(&intersection_faces);
    if world_verts.len() < 4 {
        return;
    }
    let centroid: Vec3 = world_verts.iter().sum::<Vec3>() / world_verts.len() as f32;

    // Convert to local space around centroid
    let local_faces: Vec<BrushFaceData> = intersection_faces
        .iter()
        .map(|f| BrushFaceData {
            plane: BrushPlane {
                normal: f.plane.normal,
                distance: f.plane.distance - f.plane.normal.dot(centroid),
            },
            ..f.clone()
        })
        .collect();
    let clean = clean_degenerate_faces(&local_faces);
    if clean.len() < 4 {
        return;
    }

    // Capture brush data for originals (assigns stable IDs)
    let mut originals: Vec<BrushData> = Vec::new();
    for (entity, _, _) in &selected_brushes {
        originals.push(brush_data_from_entity(world, *entity));
    }

    // Clean up selection
    {
        let despawning: Vec<Entity> = selected_brushes.iter().map(|(e, _, _)| *e).collect();
        let mut selection = world.resource_mut::<Selection>();
        selection.entities.retain(|e| !despawning.contains(e));
    }
    for (entity, _, _) in &selected_brushes {
        if let Ok(mut e) = world.get_entity_mut(*entity) {
            e.remove::<Selected>();
        }
    }

    // Despawn originals
    for (entity, _, _) in &selected_brushes {
        if let Ok(e) = world.get_entity_mut(*entity) {
            e.despawn();
        }
    }

    // Spawn the intersection brush
    let new_brush = Brush { faces: clean };
    let frag_sid = world.resource_mut::<StableIdCounter>().next();
    let brush_data = BrushData {
        stable_id: frag_sid,
        brush: new_brush,
        transform: Transform::from_translation(centroid),
        name: "Brush".to_string(),
        parent_stable_id: None,
    };
    let entity = spawn_brush_from_data(world, &brush_data);

    // Select the new brush
    {
        let mut selection = world.resource_mut::<Selection>();
        selection.entities.push(entity);
    }
    world.entity_mut(entity).insert(Selected);

    // Push undo command (reuses SubtractBrushCommand, same undo/redo pattern).
    let cmd = SubtractBrushCommand {
        originals,
        fragments: vec![BrushOrGroup::Single(brush_data)],
    };
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
}

#[operator(
    id = "brush.join",
    label = "Join (Convex Merge)",
    description = "Merge all selected brushes into a single convex-hull brush. \
                   Requires at least two `Brush` entities in \
                   `Selection::entities`; availability (`can_run_binary_brush_op`) \
                   is false otherwise. The first selected brush keeps its entity \
                   id and transform; others are despawned.",
    is_available = can_run_binary_brush_op,
)]
pub(crate) fn brush_join(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(join_selected_brushes_impl);
    OperatorResult::Finished
}

#[operator(
    id = "brush.csg_subtract",
    label = "CSG Subtract",
    description = "Subtract the non-first selected brushes from the first. \
                   Requires at least two `Brush` entities in `Selection::entities` \
                   (first is the target, rest are cutters); availability \
                   (`can_run_binary_brush_op`) is false otherwise. The target may \
                   be split into multiple fragment brushes.",
    is_available = can_run_binary_brush_op,
)]
pub(crate) fn brush_csg_subtract(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(csg_subtract_selected_impl);
    OperatorResult::Finished
}

#[operator(
    id = "brush.csg_intersect",
    label = "CSG Intersect",
    description = "Replace the selected brushes with the solid shared by all of \
                   them. Requires at least two `Brush` entities in \
                   `Selection::entities`; availability \
                   (`can_run_binary_brush_op`) is false otherwise. When the \
                   intersection is empty the impl exits silently without \
                   mutating the world.",
    is_available = can_run_binary_brush_op,
)]
pub(crate) fn brush_csg_intersect(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(csg_intersect_selected_impl);
    OperatorResult::Finished
}

/// Shared environment gate: brush-level operators never run mid-draw,
/// mid-modal, or while a text input is focused. Each specific op
/// composes this with its own selection-state precondition check.
fn env_allows_brush_op(
    input_focus: &InputFocus,
    modal: &crate::modal_transform::ModalTransformState,
    draw_state: &DrawBrushState,
) -> bool {
    input_focus.0.is_none() && modal.active.is_none() && draw_state.active.is_none()
}

/// `brush.join` / `brush.csg_subtract` / `brush.csg_intersect` all
/// require at least two `Brush` entities in the current selection.
fn can_run_binary_brush_op(
    input_focus: Res<InputFocus>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<Brush>>,
) -> bool {
    if !env_allows_brush_op(&input_focus, &modal, &draw_state) {
        return false;
    }
    selection
        .entities
        .iter()
        .filter(|&&e| brushes.contains(e))
        .count()
        >= 2
}

/// `brush.extend_face_to_brush` needs either (a) Face edit mode with a
/// face picked and another brush selected, or (b) Object mode with ≥ 2
/// brushes selected and a remembered/hovered face on the primary.
fn can_run_extend_face(
    input_focus: Res<InputFocus>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    brush_selection: Res<crate::brush::BrushSelection>,
    edit_mode: Res<crate::brush::EditMode>,
    brushes: Query<(), With<Brush>>,
) -> bool {
    if !env_allows_brush_op(&input_focus, &modal, &draw_state) {
        return false;
    }
    let brush_count = selection
        .entities
        .iter()
        .filter(|&&e| brushes.contains(e))
        .count();
    match *edit_mode {
        crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face) => {
            brush_selection.entity.is_some()
                && !brush_selection.faces.is_empty()
                && brush_count >= 1
        }
        crate::brush::EditMode::Object => {
            brush_count >= 2
                && brush_selection.last_face_entity.is_some_and(|e| {
                    brush_selection.last_face_index.is_some() && brushes.contains(e)
                })
        }
        _ => false,
    }
}

#[operator(
    id = "brush.extend_face_to_brush",
    label = "Extend to Brush",
    description = "Extend a face of the primary brush to conform to the shape of \
                   the other selected brushes. Two entry paths:\n\
                   • `EditMode::BrushEdit(Face)` with a face selected on \
                     `BrushSelection` and ≥ 1 other brush in `Selection::entities`.\n\
                   • `EditMode::Object` with ≥ 2 brushes in `Selection::entities` \
                     and a remembered face on the primary.\n\
                   Availability (`can_run_extend_face`) is false when neither \
                   entry path applies. The Object-mode path additionally \
                   tries to resolve a hovered face via raycast once invoked; \
                   if that also fails it returns `Cancelled`.",
    is_available = can_run_extend_face,
)]
pub(crate) fn brush_extend_face_to_brush(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<crate::brush::EditMode>,
    selection: Res<Selection>,
    mut brush_selection: ResMut<crate::brush::BrushSelection>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    mut ray_cast: MeshRayCast,
    brush_faces: Query<&BrushFaceEntity>,
    brush_query: Query<(), With<Brush>>,
    mut commands: Commands,
) -> OperatorResult {
    // Resolve (primary, face_index, targets) depending on edit mode
    let (primary, face_index, targets) =
        if *edit_mode == crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face) {
            // Face mode path: primary is the brush being edited, face is the selected face
            let Some(primary) = brush_selection.entity.filter(|&e| brush_query.contains(e)) else {
                return OperatorResult::Cancelled;
            };
            let Some(&face_index) = brush_selection.faces.last() else {
                return OperatorResult::Cancelled;
            };
            let targets: Vec<Entity> = selection
                .entities
                .iter()
                .copied()
                .filter(|&e| e != primary && brush_query.contains(e))
                .collect();
            if targets.is_empty() {
                return OperatorResult::Cancelled;
            }
            (primary, face_index, targets)
        } else if *edit_mode == crate::brush::EditMode::Object {
            // Object mode: need 2+ brushes selected
            let selected_brushes: Vec<Entity> = selection
                .entities
                .iter()
                .copied()
                .filter(|&e| brush_query.contains(e))
                .collect();
            if selected_brushes.len() < 2 {
                return OperatorResult::Cancelled;
            }

            let Some(primary) = selection.primary().filter(|e| brush_query.contains(*e)) else {
                return OperatorResult::Cancelled;
            };
            let targets: Vec<Entity> = selected_brushes
                .into_iter()
                .filter(|&e| e != primary)
                .collect();

            // Try hover raycast first to find the face
            let face_index = find_hovered_face_on_brush(
                primary,
                &windows,
                &camera_query,
                &viewport_query,
                &mut ray_cast,
                &brush_faces,
            )
            .or_else(|| {
                // Fall back to remembered face
                if brush_selection.last_face_entity == Some(primary) {
                    brush_selection.last_face_index
                } else {
                    None
                }
            });

            let Some(face_index) = face_index else {
                return OperatorResult::Cancelled;
            };
            (primary, face_index, targets)
        } else {
            return OperatorResult::Cancelled;
        };

    // If we were in face mode, exit it (geometry is about to change, indices become invalid)
    if *edit_mode == crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face) {
        *edit_mode = crate::brush::EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }

    let targets_clone = targets.clone();
    commands.queue(move |world: &mut World| {
        extend_face_to_brush_impl(world, primary, &targets_clone, face_index);
    });
    OperatorResult::Finished
}

/// Raycast from cursor to find a hovered `BrushFaceEntity` belonging to the given brush.
/// Returns the face index if found.
fn find_hovered_face_on_brush(
    brush_entity: Entity,
    windows: &Query<&Window>,
    camera_query: &Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    ray_cast: &mut MeshRayCast,
    brush_faces: &Query<&BrushFaceEntity>,
) -> Option<usize> {
    let window = windows.single().ok()?;
    let cursor_pos = window.cursor_position()?;
    let (camera, cam_tf) = camera_query.single().ok()?;
    let viewport_cursor = window_to_viewport_cursor(cursor_pos, camera, viewport_query)?;
    let ray = camera.viewport_to_world(cam_tf, viewport_cursor).ok()?;

    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
    let hits = ray_cast.cast_ray(ray, &settings);

    for (hit_entity, _) in hits {
        if let Ok(face_ent) = brush_faces.get(*hit_entity)
            && face_ent.brush_entity == brush_entity
        {
            return Some(face_ent.face_index);
        }
    }
    None
}

/// Core logic for Extend Face to Brush.
///
/// Removes the specified face from the primary brush, adds all target brush faces,
/// then computes the intersection. The result is the primary brush reshaped to
/// conform to the target brushes in the direction of the removed face.
pub(crate) fn extend_face_to_brush_impl(
    world: &mut World,
    primary: Entity,
    targets: &[Entity],
    face_index: usize,
) {
    // Read primary brush
    let Some(primary_brush) = world.get::<Brush>(primary) else {
        return;
    };
    let old_brush = primary_brush.clone();
    if face_index >= old_brush.faces.len() {
        return;
    }

    let Some(primary_gtf) = world.get::<GlobalTransform>(primary) else {
        return;
    };
    let (_, rotation, translation) = primary_gtf.to_scale_rotation_translation();
    let inv_rotation = rotation.inverse();

    // Transform primary faces to world space, removing the target face
    let all_world_faces = brush_planes_to_world(&old_brush.faces, rotation, translation);
    let removed_normal = all_world_faces[face_index].plane.normal;
    let mut world_faces: Vec<BrushFaceData> = all_world_faces
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i != face_index)
        .map(|(_, f)| f)
        .collect();

    // Collect candidate target faces in world space
    let mut candidate_faces = Vec::new();
    for &target in targets {
        let Some(target_brush) = world.get::<Brush>(target) else {
            continue;
        };
        let Some(target_gtf) = world.get::<GlobalTransform>(target) else {
            continue;
        };
        let (_, t_rot, t_trans) = target_gtf.to_scale_rotation_translation();
        let target_world_faces = brush_planes_to_world(&target_brush.faces, t_rot, t_trans);
        // Flip target faces: negate normal and distance so the half-space constraint
        // means "on the outside of the target brush" rather than "inside it". This way
        // the wall extends UP TO the target surface instead of being clipped to the
        // target interior.
        candidate_faces.extend(target_world_faces.into_iter().map(|f| BrushFaceData {
            plane: BrushPlane {
                normal: -f.plane.normal,
                distance: -f.plane.distance,
            },
            ..f
        }));
    }

    // Filter target faces: prefer angled faces (not anti-parallel or perpendicular to the
    // removed face). Anti-parallel faces (dot ≈ -1) would just re-cap at the same level,
    // and perpendicular/same-direction faces (dot ≥ 0) don't constrain the extension.
    let angled: Vec<BrushFaceData> = candidate_faces
        .iter()
        .filter(|f| {
            let dot = f.plane.normal.dot(removed_normal);
            dot < -0.01 && dot > -0.99
        })
        .cloned()
        .collect();

    // If we found angled faces, use those. Otherwise fall back to all faces with a
    // negative dot (the simple flat-ceiling case where anti-parallel IS the constraint).
    if !angled.is_empty() {
        world_faces.extend(angled);
    } else {
        let opposing: Vec<BrushFaceData> = candidate_faces
            .into_iter()
            .filter(|f| f.plane.normal.dot(removed_normal) < -0.01)
            .collect();
        world_faces.extend(opposing);
    }

    // Compute geometry from combined face set
    let (verts, _) = compute_brush_geometry(&world_faces);
    if verts.len() < 4 {
        return;
    }

    // No-op check: compare with original geometry
    let (old_verts, _) = compute_brush_geometry(&brush_planes_to_world(
        &old_brush.faces,
        rotation,
        translation,
    ));
    if verts.len() == old_verts.len() {
        let mut changed = false;
        for (a, b) in verts.iter().zip(old_verts.iter()) {
            if a.distance(*b) > 1e-4 {
                changed = true;
                break;
            }
        }
        if !changed {
            return;
        }
    }

    // Convert ALL world faces back to local space (keeping constraint planes),
    // then clean degenerate faces once in local space.
    let local_faces: Vec<BrushFaceData> = world_faces
        .iter()
        .map(|f| BrushFaceData {
            plane: BrushPlane {
                normal: inv_rotation * f.plane.normal,
                distance: f.plane.distance - f.plane.normal.dot(translation),
            },
            ..f.clone()
        })
        .collect();
    let local_clean = clean_degenerate_faces(&local_faces);
    if local_clean.len() < 4 {
        return;
    }
    // Apply via undo-able SetBrush command (ECS + AST)
    let new_brush = Brush { faces: local_clean };
    crate::brush::sync_brush_to_ast(world, primary, &new_brush);
    if let Some(mut brush) = world.get_mut::<Brush>(primary) {
        *brush = new_brush.clone();
    }

    let cmd = crate::brush::SetBrush {
        entity: primary,
        old: old_brush,
        new: new_brush,
        label: "Extend face to brush".to_string(),
    };
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
}
