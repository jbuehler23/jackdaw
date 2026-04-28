use crate::default_style;
use crate::{
    EditorEntity,
    gizmos::handle_gizmo_hover,
    selection::Selection,
    viewport::{InteractionGuards, ViewportCursor},
};
use bevy::input_focus::InputFocus;
use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    picking::prelude::Pickable,
    prelude::*,
};
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_jsn::BrushGroup;

/// Marker for the box-select visual overlay node.
#[derive(Component)]
struct BoxSelectOverlay;

pub struct ViewportSelectPlugin;

impl Plugin for ViewportSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BoxSelectState>()
            .init_resource::<GroupEditState>()
            .init_resource::<LastClick>()
            .add_systems(
                Update,
                (
                    handle_viewport_click.after(handle_gizmo_hover),
                    box_select_invoke_trigger,
                    update_box_select_overlay,
                )
                    .in_set(crate::EditorInteractionSystems),
            );
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BoxSelectOp>()
        .register_operator::<ViewportExitGroupEditOp>();
    let ext = ctx.id();
    ctx.spawn((
        Action::<ViewportExitGroupEditOp>::new(),
        ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Escape, Press::default())],
    ));
}

fn group_edit_active(
    group_edit: Res<GroupEditState>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
) -> bool {
    !keybind_focus.is_typing() && group_edit.active_group.is_some()
}

/// Exit `BrushGroup` edit mode (entered via double-click on a group).
#[operator(
    id = "viewport.exit_group_edit",
    label = "Exit Group Edit",
    description = "Stop editing the current brush group.",
    is_available = group_edit_active,
)]
pub(crate) fn viewport_exit_group_edit(
    _: In<OperatorParameters>,
    mut group_edit: ResMut<GroupEditState>,
) -> OperatorResult {
    group_edit.active_group = None;
    OperatorResult::Finished
}

#[derive(Resource, Default)]
pub struct BoxSelectState {
    pub active: bool,
    pub start: Vec2,
    pub current: Vec2,
}

/// Tracks whether the user is editing inside a `BrushGroup` (entered via double-click).
#[derive(Resource, Default)]
pub struct GroupEditState {
    /// The `BrushGroup` entity we're currently editing inside of.
    pub active_group: Option<Entity>,
}

/// Tracks last click for double-click detection.
#[derive(Resource, Default)]
pub(crate) struct LastClick {
    entity: Option<Entity>,
    time: f64,
}

pub(crate) fn handle_viewport_click(
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    scene_entities: Query<(Entity, &GlobalTransform), (Without<EditorEntity>, With<Transform>)>,
    parents: Query<&ChildOf>,
    brush_groups: Query<(), With<BrushGroup>>,
    guards: InteractionGuards,
    mut selection: ResMut<Selection>,
    mut input_focus: ResMut<InputFocus>,
    mut commands: Commands,
    mut ray_cast: MeshRayCast,
    (mut group_edit, mut last_click, time): (ResMut<GroupEditState>, ResMut<LastClick>, Res<Time>),
    // One-frame memory of `draw_state.active`. `draw_brush.confirm` clears
    // the state inline before this system runs, so the same mouse-press
    // would otherwise fall through to `selection.clear()` and strip
    // `Selected` from the just-spawned brush.
    mut was_drawing: Local<bool>,
) {
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    let drawing_now = guards.draw_state.active.is_some();
    let just_finished_draw = *was_drawing && !drawing_now;
    *was_drawing = drawing_now;

    // Don't select during gizmo drag, modal ops, viewport drag, brush edit mode, draw mode,
    // terrain sculpt mode, or shift+click (which starts box select).
    // Physics mode IS allowed, the user needs to click-select entities to
    // drag them in the physics tool.
    if !mouse.just_pressed(MouseButton::Left)
        || shift
        || guards.gizmo_drag.active
        || guards.gizmo_hover.hovered_axis.is_some()
        || guards.modal.active.is_some()
        || guards.viewport_drag.active.is_some()
        || matches!(*guards.edit_mode, crate::brush::EditMode::BrushEdit(_))
        || drawing_now
        || just_finished_draw
        || matches!(
            *guards.terrain_edit_mode,
            crate::terrain::TerrainEditMode::Sculpt(_)
        )
    {
        return;
    }

    let Ok(window) = vp.windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // Check if cursor is within viewport
    let Ok((vp_computed, vp_tf)) = vp.viewport.single() else {
        return;
    };
    let scale = vp_computed.inverse_scale_factor();
    let vp_pos = vp_tf.translation * scale;
    let vp_size = vp_computed.size() * scale;
    let vp_top_left = vp_pos - vp_size / 2.0;
    let local_cursor = cursor_pos - vp_top_left;
    if local_cursor.x < 0.0
        || local_cursor.y < 0.0
        || local_cursor.x > vp_size.x
        || local_cursor.y > vp_size.y
    {
        return;
    }

    // Clear input focus so keyboard shortcuts (G/R/S) work after viewport click
    input_focus.0 = None;

    let Ok((camera, cam_tf)) = vp.camera.single() else {
        return;
    };

    // Remap from UI-logical space to camera render-target space
    let target_size = camera.logical_viewport_size().unwrap_or(vp_size);
    let local_cursor = local_cursor * target_size / vp_size;

    // Try mesh raycast first for accurate geometry-based selection
    let mut best_entity = None;

    if let Ok(ray) = camera.viewport_to_world(cam_tf, local_cursor) {
        let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
        let hits = ray_cast.cast_ray(ray, &settings);

        // Find the first hit that resolves to a scene entity (skip editor entities)
        for (hit_entity, _) in hits {
            if let Some(ancestor) = find_selectable_ancestor(
                *hit_entity,
                &scene_entities,
                &parents,
                &group_edit,
                &brush_groups,
            ) {
                best_entity = Some(ancestor);
                break;
            }
        }

        // If we'd select a different entity, but the current selection is also
        // under the cursor (overlapping geometry), keep the current selection.
        // This prevents re-selecting the original after Ctrl+D duplication.
        if let Some(candidate) = best_entity
            && let Some(current_primary) = selection.primary()
            && candidate != current_primary
        {
            for (hit_entity, _) in hits {
                if find_selectable_ancestor(
                    *hit_entity,
                    &scene_entities,
                    &parents,
                    &group_edit,
                    &brush_groups,
                ) == Some(current_primary)
                {
                    return;
                }
            }
        }
    }

    // Fall back to screen-space proximity for non-mesh entities (lights, empties)
    if best_entity.is_none() {
        let mut best_dist = 30.0_f32;
        for (entity, global_tf) in &scene_entities {
            let pos = global_tf.translation();
            if let Ok(screen_pos) = camera.world_to_viewport(cam_tf, pos) {
                let dist = (screen_pos - local_cursor).length();
                if dist < best_dist {
                    best_dist = dist;
                    best_entity = Some(entity);
                }
            }
        }
    }

    // Double-click detection: if same entity clicked within 400ms, enter group
    let now = time.elapsed_secs_f64();
    if let Some(entity) = best_entity {
        let is_double_click = last_click.entity == Some(entity) && (now - last_click.time) < 0.4;

        if is_double_click && brush_groups.contains(entity) {
            // Double-click on a BrushGroup: enter group edit mode
            group_edit.active_group = Some(entity);
            last_click.entity = None;
            last_click.time = 0.0;
            return;
        }

        last_click.entity = Some(entity);
        last_click.time = now;

        let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        let in_physics_mode = *guards.edit_mode == crate::brush::EditMode::Physics;

        if in_physics_mode {
            // In Physics mode: clicking an already-selected entity is a drag
            // start, NOT a re-select. Only modify selection for unselected
            // entities (add them). This preserves multi-selection.
            if !selection.is_selected(entity) {
                if ctrl {
                    selection.toggle(&mut commands, entity);
                } else {
                    selection.select_single(&mut commands, entity);
                }
            }
        } else if ctrl {
            selection.toggle(&mut commands, entity);
        } else {
            selection.select_single(&mut commands, entity);
        }
    } else {
        last_click.entity = None;
        last_click.time = 0.0;

        // Clicked on empty space  -- exit group edit and deselect all (unless Ctrl held)
        if group_edit.active_group.is_some() {
            group_edit.active_group = None;
        }
        let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        if !ctrl {
            selection.clear(&mut commands);
        }
    }
}

/// Shift+LMB in the viewport dispatches [`BoxSelectOp`], and a release
/// inside the operator commits it. Both the trigger here and the
/// release detection on line 327 sit outside the BEI keybind menu
/// because modifier+mouse gestures aren't expressible as BEI key
/// actions. Surfacing them in the customisation UI is part of the
/// keybind menu rebuild (Phase 11).
fn box_select_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    guards: InteractionGuards,
    box_state: Res<BoxSelectState>,
    mut commands: Commands,
) {
    if box_state.active
        || !mouse.just_pressed(MouseButton::Left)
        || !keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
        || guards.gizmo_drag.active
        || matches!(*guards.edit_mode, crate::brush::EditMode::BrushEdit(_))
        || guards.draw_state.active.is_some()
        || guards.modal.active.is_some()
    {
        return;
    }
    let Ok(window) = vp.windows.single() else {
        return;
    };
    if window.cursor_position().is_none() {
        return;
    }
    commands.queue(|world: &mut World| {
        if let Err(err) = world.operator(BoxSelectOp::ID).call() {
            error!("box-select dispatch failed: {err}");
        }
    });
}

#[operator(
    id = "selection.box_select",
    label = "Box Select",
    description = "Drag a rectangle to select entities inside it.",
    modal = true,
    cancel = cancel_box_select,
)]
pub fn box_select(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    vp: ViewportCursor,
    mut box_state: ResMut<BoxSelectState>,
    scene_entities: Query<(Entity, &GlobalTransform), (Without<EditorEntity>, With<Name>)>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
    active: ActiveModalQuery,
) -> OperatorResult {
    let Ok(window) = vp.windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };

    if !active.is_modal_running() {
        box_state.active = true;
        box_state.start = cursor_pos;
        box_state.current = cursor_pos;
        return OperatorResult::Running;
    }

    box_state.current = cursor_pos;
    if !mouse.just_released(MouseButton::Left) {
        return OperatorResult::Running;
    }
    box_state.active = false;

    let Ok((camera, cam_tf)) = vp.camera.single() else {
        return OperatorResult::Finished;
    };
    let Ok((vp_computed, vp_tf)) = vp.viewport.single() else {
        return OperatorResult::Finished;
    };
    let map = crate::viewport_util::ViewportRemap::new(camera, vp_computed, vp_tf);
    let start_local = box_state.start - map.top_left;
    let current_local = box_state.current - map.top_left;
    let min = start_local.min(current_local) * map.remap;
    let max = start_local.max(current_local) * map.remap;

    let selected: Vec<Entity> = scene_entities
        .iter()
        .filter_map(|(entity, tf)| {
            let screen = camera.world_to_viewport(cam_tf, tf.translation()).ok()?;
            (screen.x >= min.x && screen.x <= max.x && screen.y >= min.y && screen.y <= max.y)
                .then_some(entity)
        })
        .collect();

    if !selected.is_empty() {
        selection.select_multiple(&mut commands, &selected);
    }
    OperatorResult::Finished
}

fn cancel_box_select(mut box_state: ResMut<BoxSelectState>) {
    box_state.active = false;
}

fn update_box_select_overlay(
    box_state: Res<BoxSelectState>,
    overlay_query: Query<Entity, With<BoxSelectOverlay>>,
    mut commands: Commands,
) {
    if box_state.active {
        let min = box_state.start.min(box_state.current);
        let max = box_state.start.max(box_state.current);
        let size = max - min;

        let node = (
            BoxSelectOverlay,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(min.x),
                top: Val::Px(min.y),
                width: Val::Px(size.x),
                height: Val::Px(size.y),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(default_style::SELECTION_MARQUEE_BG),
            BorderColor::all(default_style::SELECTION_MARQUEE_BORDER),
            GlobalZIndex(50),
            Pickable::IGNORE,
        );

        if let Some(entity) = overlay_query.iter().next() {
            commands.entity(entity).insert(node);
        } else {
            commands.spawn(node);
        }
    } else {
        for entity in &overlay_query {
            commands.entity(entity).despawn();
        }
    }
}

/// Walk up the `ChildOf` hierarchy from a raycast hit entity to find the
/// top-level scene entity (one that appears in `scene_entities`).
/// Handles GLTF child meshes and brush face children.
///
/// When inside a group (`GroupEditState::active_group` is set), stops at children
/// of that group so individual fragments can be selected.
fn find_selectable_ancestor(
    mut entity: Entity,
    scene_entities: &Query<(Entity, &GlobalTransform), (Without<EditorEntity>, With<Transform>)>,
    parents: &Query<&ChildOf>,
    group_edit: &GroupEditState,
    brush_groups: &Query<(), With<BrushGroup>>,
) -> Option<Entity> {
    // Walk up until we find a scene entity (one that has Transform and is not EditorEntity)
    // Start with the hit entity itself  -- it may already be a scene entity
    loop {
        if scene_entities.contains(entity) {
            // Check if this entity has a parent that's also a scene entity;
            // if so, prefer the parent (handles GLTF sub-meshes).
            if let Ok(child_of) = parents.get(entity) {
                let parent = child_of.0;
                if scene_entities.contains(parent) {
                    // If we're inside a group and this parent IS that group,
                    // stop here  -- let the user select the child fragment.
                    if group_edit.active_group == Some(parent) && brush_groups.contains(parent) {
                        return Some(entity);
                    }
                    // Keep walking up  -- the parent is also selectable
                    entity = parent;
                    continue;
                }
            }
            return Some(entity);
        }
        if let Ok(child_of) = parents.get(entity) {
            entity = child_of.0;
        } else {
            return None;
        }
    }
}
