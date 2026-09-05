use crate::EditorEntity;
use crate::brush::{Brush, BrushEditMode, BrushFaceData, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::selection::Selection;

use bevy::ecs::lifecycle::Insert;
use bevy::feathers::containers::{group, group_body, group_header, pane_body};
use bevy::feathers::controls::{FeathersTextInput, FeathersTextInputContainer};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input_focus::{FocusLost, FocusedInput};
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit};
use bevy::ui_widgets::ValueChange;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{button::ButtonOperatorCall, tokens};

use super::{BrushFaceField, BrushFaceFieldBinding, BrushFacePropsContainer};

/// Initial text staged on a brush face text input container. The
/// `Insert`-triggered `seed_brush_face_text` observer writes it into the
/// editable buffer once the child text entry spawns, then removes it.
#[derive(Component)]
struct PendingBrushFaceText(String);

/// Container framing a `FeathersTextInput` bound to a brush face UV field.
/// The inner text entry drives the commit observers; the container holds the
/// `BrushFaceFieldBinding` and staged value, and seeds the text buffer once the
/// staged value lands.
fn brush_face_text_scene() -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        on(seed_brush_face_text)
        Children [
            @FeathersTextInput
            on(brush_face_text_on_enter_key)
            on(brush_face_text_on_focus_lost)
        ]
    }
}

/// Spawn a `FeathersTextInput` bound to a brush face UV field. The binding and
/// the staged text ride on the container; the inner text entry emits
/// `ValueChange<String>` on Enter or blur, which `on_brush_face_text_commit`
/// writes back through `apply_brush_face_field`.
fn spawn_brush_face_text(
    commands: &mut Commands,
    parent: Entity,
    current_value: &str,
    field: BrushFaceField,
) {
    commands.spawn_scene(brush_face_text_scene()).insert((
        BrushFaceFieldBinding { field },
        PendingBrushFaceText(current_value.to_string()),
        ChildOf(parent),
    ));
}

/// Write the staged text into the editable buffer once it is inserted on the
/// container, then clear it so a later refresh does not re-seed it.
fn seed_brush_face_text(
    inserted: On<Insert, PendingBrushFaceText>,
    q_children: Query<&Children>,
    q_pending: Query<&PendingBrushFaceText>,
    mut q_text: Query<&mut EditableText>,
    mut commands: Commands,
) {
    let container = inserted.event_target();
    let Ok(pending) = q_pending.get(container) else {
        return;
    };
    let text_id = q_children
        .iter_descendants(container)
        .find(|e| q_text.contains(*e));
    if let Some(text_id) = text_id
        && let Ok(mut editable) = q_text.get_mut(text_id)
    {
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(pending.0.clone().into()));
    }
    commands.entity(container).remove::<PendingBrushFaceText>();
}

/// Emit a final `ValueChange<String>` when Enter is pressed in a brush face text input.
fn brush_face_text_on_enter_key(
    key_input: On<FocusedInput<KeyboardInput>>,
    q_text: Query<&EditableText>,
    mut commands: Commands,
) {
    if key_input.input.key_code != KeyCode::Enter {
        return;
    }
    let text_id = key_input.event_target();
    if let Ok(editable) = q_text.get(text_id) {
        commands.trigger(ValueChange {
            source: text_id,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

/// Emit a final `ValueChange<String>` when a brush face text input loses focus.
fn brush_face_text_on_focus_lost(
    focus_lost: On<FocusLost>,
    q_text: Query<&EditableText>,
    mut commands: Commands,
) {
    let text_id = focus_lost.event_target();
    if let Ok(editable) = q_text.get(text_id) {
        commands.trigger(ValueChange {
            source: text_id,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

/// Spawn a titled feathers group with `caption` as its header, returning the
/// group body entity that rows are parented to.
fn spawn_group(commands: &mut Commands, parent: Entity, caption: &str) -> Entity {
    let group_entity = commands.spawn_scene(group()).insert(ChildOf(parent)).id();
    let header = commands
        .spawn_scene(group_header())
        .insert(ChildOf(group_entity))
        .id();
    commands.spawn((Text::new(caption.to_string()), ThemedText, ChildOf(header)));
    commands
        .spawn_scene(group_body())
        .insert(ChildOf(group_entity))
        .id()
}

fn resolve_material_label(
    mat_handle: &Handle<StandardMaterial>,
    materials: &Assets<StandardMaterial>,
) -> String {
    if let Some(path) = mat_handle.path() {
        return path.to_string();
    }
    if let Some(mat) = materials.get(mat_handle)
        && let Some(ref tex) = mat.base_color_texture
        && let Some(path) = tex.path()
        && let Some(filename) = path.path().file_name()
    {
        return filename.to_string_lossy().to_string();
    }
    format!("Material {:?}", mat_handle.id())
}

pub(super) fn spawn_brush_display(
    commands: &mut Commands,
    parent: Entity,
    brush: &crate::brush::Brush,
    materials: &Assets<StandardMaterial>,
) {
    // Brushes always have populated topology in normal flow; the
    // plane-intersection fallback only fires for the degenerate
    // empty-brush case.
    let (face_count, vertex_count, edge_count) = if !brush.topology.polygons.is_empty() {
        (
            brush.topology.polygons.len(),
            brush.topology.vertices.len(),
            brush.topology.edges.len(),
        )
    } else {
        let (vertices, face_polygons) =
            crate::brush::compute_brush_geometry_from_planes(&brush.faces);
        let mut edges = std::collections::HashSet::new();
        for polygon in &face_polygons {
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let edge = if a < b { (a, b) } else { (b, a) };
                edges.insert(edge);
            }
        }
        (brush.faces.len(), vertices.len(), edges.len())
    };

    let info = format!("{face_count} faces, {vertex_count} vertices, {edge_count} edges");

    // Seat the card content in a feathers content pane.
    let body = commands
        .spawn_scene(pane_body())
        .insert(ChildOf(parent))
        .id();

    commands.spawn((
        Text::new(info),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(body),
    ));

    // Topology section
    let topo_body = spawn_group(commands, body, "Topology");

    // Fallback status row for the degenerate empty-brush case while
    // `topology_migration` still ships as a safety net.
    if brush.topology.polygons.is_empty() {
        commands.spawn((
            Text::new("Empty (legacy brush; will populate after migration)"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_DISABLED),
            ChildOf(topo_body),
        ));
    } else {
        let topo_rows: &[(&str, usize)] = &[
            ("Vertices", brush.topology.vertices.len()),
            ("Edges", brush.topology.edges.len()),
            ("Polygons", brush.topology.polygons.len()),
            ("Loops", brush.topology.loops.len()),
        ];
        for (label, count) in topo_rows {
            let row = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: px(tokens::SPACING_XS),
                        width: Val::Percent(100.0),
                        ..Default::default()
                    },
                    ChildOf(topo_body),
                ))
                .id();
            commands.spawn((
                Text::new(*label),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    min_width: px(60.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                ChildOf(row),
            ));
            commands.spawn((
                Text::new(count.to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
                ChildOf(row),
            ));
        }
    }

    // Material summary: shows unique materials used by this brush.
    spawn_material_summary(commands, body, brush, materials);

    // Face properties container -- populated dynamically by update_brush_face_properties
    commands.spawn((
        BrushFacePropsContainer,
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            row_gap: px(tokens::SPACING_XS),
            ..Default::default()
        },
        ChildOf(body),
    ));
}

fn spawn_material_summary(
    commands: &mut Commands,
    parent: Entity,
    brush: &Brush,
    materials: &Assets<StandardMaterial>,
) {
    // Collect unique materials with face counts
    let mut material_counts: Vec<(Handle<StandardMaterial>, usize)> = Vec::new();
    for face in &brush.faces {
        if let Some(entry) = material_counts
            .iter_mut()
            .find(|(h, _)| *h == face.material)
        {
            entry.1 += 1;
        } else {
            material_counts.push((face.material.clone(), 1));
        }
    }

    let total_faces = brush.faces.len();
    let any_has_material = material_counts.iter().any(|(h, _)| *h != Handle::default());

    // Materials section
    let mat_body = spawn_group(commands, parent, "Materials & Textures");

    for (mat_handle, count) in &material_counts {
        let is_default = *mat_handle == Handle::default();

        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                ChildOf(mat_body),
            ))
            .id();

        // Thumbnail
        if !is_default
            && let Some(mat) = materials.get(mat_handle)
            && let Some(ref tex) = mat.base_color_texture
        {
            commands.spawn((
                ImageNode::new(tex.clone()),
                Node {
                    width: Val::Px(32.0),
                    height: Val::Px(32.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                ChildOf(row),
            ));
        }

        // Material name
        let mat_label = if is_default {
            "No Material".to_string()
        } else {
            resolve_material_label(mat_handle, materials)
        };
        commands.spawn((
            Text::new(mat_label),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(if is_default {
                tokens::TEXT_SECONDARY
            } else {
                tokens::TEXT_PRIMARY
            }),
            Node {
                flex_grow: 1.0,
                ..Default::default()
            },
            ChildOf(row),
        ));

        // Face count
        let count_text = if *count == total_faces {
            "(all faces)".to_string()
        } else {
            format!("({count} faces)")
        };
        commands.spawn((
            Text::new(count_text),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(row),
        ));
    }

    // Clear All button, only if at least one face has a material. The
    // `ButtonOperatorCall` drives both dispatch (via `Activate` ->
    // `dispatch_activate_operator`) and the hover tooltip.
    if any_has_material {
        commands
            .spawn_scene(jackdaw_feathers::button::operator_button(
                BrushClearAllMaterialsOp::ID,
                "Clear All",
            ))
            .insert(ChildOf(mat_body));
    }
}

/// Tracks the last state we rendered so we only rebuild on change.
#[derive(Default)]
pub(super) struct BrushFacePropsState {
    entity: Option<Entity>,
    faces: Vec<usize>,
    /// Hash of face data to detect UV edits
    data_hash: u64,
}

fn hash_face_data(face: &BrushFaceData) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Hash the material handle id
    face.material.id().hash(&mut hasher);
    face.uv_offset.x.to_bits().hash(&mut hasher);
    face.uv_offset.y.to_bits().hash(&mut hasher);
    face.uv_scale.x.to_bits().hash(&mut hasher);
    face.uv_scale.y.to_bits().hash(&mut hasher);
    face.uv_rotation.to_bits().hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn update_brush_face_properties(
    mut commands: Commands,
    edit_mode: Res<EditMode>,
    brush_selection: Res<BrushSelection>,
    brushes: Query<&Brush>,
    container_query: Query<(Entity, Option<&Children>), With<BrushFacePropsContainer>>,
    mut local_state: Local<BrushFacePropsState>,
    materials: Res<Assets<StandardMaterial>>,
) {
    // `iter().next()` rather than `single()`: during an inspector rebuild the
    // old container is despawned while the new one spawns, so there is a window
    // with 0 or 2 containers where `single()` would error and skip the update,
    // leaving the face-property fields stale.
    let Some((container_entity, container_children)) = container_query.iter().next() else {
        return;
    };

    let active_sub = brush_selection.active_sub();
    let show = *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && brush_selection.active_brush.is_some()
        && active_sub.is_some_and(|s| !s.faces.is_empty());

    if !show {
        // Clear if we had content
        if local_state.entity.is_some() {
            if let Some(children) = container_children {
                for child in children.iter() {
                    commands.entity(child).despawn();
                }
            }
            *local_state = BrushFacePropsState::default();
        }
        return;
    }

    let brush_entity = brush_selection.active_brush.unwrap();
    let sub = brush_selection.sub(brush_entity).unwrap();
    let Ok(brush) = brushes.get(brush_entity) else {
        return;
    };

    // Compute hash of selected face data
    let mut combined_hash = 0u64;
    for &fi in &sub.faces {
        if fi < brush.faces.len() {
            combined_hash = combined_hash.wrapping_add(hash_face_data(&brush.faces[fi]));
        }
    }

    // Check if anything changed
    if local_state.entity == Some(brush_entity)
        && local_state.faces == sub.faces
        && local_state.data_hash == combined_hash
    {
        return;
    }

    // Rebuild UI
    if let Some(children) = container_children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    local_state.entity = Some(brush_entity);
    local_state.faces = sub.faces.clone();
    local_state.data_hash = combined_hash;

    // Seat the rebuilt face-property rows in a feathers group whose header
    // names the current face selection.
    let first_face_idx = sub.faces[0];
    let face = &brush.faces[first_face_idx];
    let multi = sub.faces.len() > 1;

    let header_text = if multi {
        format!("{} faces selected", sub.faces.len())
    } else {
        format!("Face {}", first_face_idx)
    };
    let body = spawn_group(&mut commands, container_entity, &header_text);

    // Material info
    let has_material = face.material != Handle::default();
    if has_material {
        let mat_label = resolve_material_label(&face.material, &materials);

        let mat_row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                ChildOf(body),
            ))
            .id();

        // Show base_color thumbnail if available
        if let Some(mat) = materials.get(&face.material)
            && let Some(ref tex) = mat.base_color_texture
        {
            commands.spawn((
                ImageNode::new(tex.clone()),
                Node {
                    width: Val::Px(32.0),
                    height: Val::Px(32.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                ChildOf(mat_row),
            ));
        }

        commands.spawn((
            Text::new(mat_label),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                flex_grow: 1.0,
                ..Default::default()
            },
            ChildOf(mat_row),
        ));

        // Clear material button. The `ButtonOperatorCall` drives both
        // dispatch (via `Activate`) and the hover tooltip.
        commands
            .spawn_scene(jackdaw_feathers::button::operator_button(
                BrushFaceClearMaterialOp::ID,
                "Clear",
            ))
            .insert(ChildOf(mat_row));

        // "Apply to All Faces" button.
        commands
            .spawn_scene(jackdaw_feathers::button::operator_button(
                BrushFaceApplyTextureToAllOp::ID,
                "Apply to All Faces",
            ))
            .insert(ChildOf(body));
    } else {
        commands.spawn((
            Text::new("No Material"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(body),
        ));
    }

    // UV Offset
    spawn_brush_face_field_row(
        &mut commands,
        body,
        "UV Offset",
        face.uv_offset.x as f64,
        face.uv_offset.y as f64,
        BrushFaceField::UvOffsetX,
        BrushFaceField::UvOffsetY,
    );

    // UV Scale
    spawn_brush_face_field_row(
        &mut commands,
        body,
        "UV Scale",
        face.uv_scale.x as f64,
        face.uv_scale.y as f64,
        BrushFaceField::UvScaleX,
        BrushFaceField::UvScaleY,
    );

    // UV Scale preset buttons. Each carries a `scale` param; the
    // `ButtonOperatorCall` drives dispatch (via `Activate`) and tooltip.
    let preset_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(body),
        ))
        .id();
    for preset in [0.25_f32, 0.5, 1.0, 2.0] {
        let label = if preset == 1.0 {
            "1x".to_string()
        } else {
            format!("{preset}x")
        };
        // Wrap in a growing cell so the presets spread evenly; the button
        // keeps its native feathers Node. Override `ButtonOperatorCall` with
        // the per-preset `scale` param.
        let cell = commands
            .spawn((
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
                ChildOf(preset_row),
            ))
            .id();
        commands
            .spawn_scene(jackdaw_feathers::button::operator_button(
                BrushFaceSetUvScalePresetOp::ID,
                label,
            ))
            .insert((
                ButtonOperatorCall::new(BrushFaceSetUvScalePresetOp::ID)
                    .with_param("scale", preset as f64),
                ChildOf(cell),
            ));
    }

    // UV Rotation
    let rot_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(body),
        ))
        .id();

    commands.spawn((
        Text::new("Rotation"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: px(60.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(rot_row),
    ));

    let rotation_degrees = face.uv_rotation.to_degrees() as f64;
    spawn_brush_face_text(
        &mut commands,
        rot_row,
        &rotation_degrees.to_string(),
        BrushFaceField::UvRotation,
    );
}

fn spawn_brush_face_field_row(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    x_value: f64,
    y_value: f64,
    x_field: BrushFaceField,
    y_field: BrushFaceField,
) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: px(60.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(row),
    ));

    spawn_brush_face_text(commands, row, &x_value.to_string(), x_field);
    spawn_brush_face_text(commands, row, &y_value.to_string(), y_field);
}

/// Handle `ValueChange<String>` for brush face field bindings. The event fires
/// on the inner text entry, so the binding is found by walking up to its
/// container.
pub(crate) fn on_brush_face_text_commit(
    event: On<ValueChange<String>>,
    bindings: Query<&BrushFaceFieldBinding>,
    child_of_query: Query<&ChildOf>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
) {
    if !event.is_final {
        return;
    }

    // Walk up from the committed entity to find a BrushFaceFieldBinding
    let mut current = event.source;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        if let Ok(binding) = bindings.get(child_of.parent()) {
            let value: f64 = event.value.parse().unwrap_or(0.0);
            apply_brush_face_field(
                binding.field,
                value,
                &brush_selection,
                &mut brushes,
                &mut history,
            );
            return;
        }
        current = child_of.parent();
    }
}

fn apply_brush_face_field(
    field: BrushFaceField,
    value: f64,
    brush_selection: &BrushSelection,
    brushes: &mut Query<&mut Brush>,
    history: &mut CommandHistory,
) {
    let Some(brush_entity) = brush_selection.active_brush else {
        return;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return;
    };
    let faces: Vec<usize> = brush_selection
        .sub(brush_entity)
        .map(|s| s.faces.clone())
        .unwrap_or_default();

    let old = brush.clone();
    for &face_idx in &faces {
        if face_idx >= brush.faces.len() {
            continue;
        }
        let face = &mut brush.faces[face_idx];
        match field {
            BrushFaceField::UvOffsetX => face.uv_offset.x = value as f32,
            BrushFaceField::UvOffsetY => face.uv_offset.y = value as f32,
            BrushFaceField::UvScaleX => face.uv_scale.x = value as f32,
            BrushFaceField::UvScaleY => face.uv_scale.y = value as f32,
            BrushFaceField::UvRotation => face.uv_rotation = (value as f32).to_radians(),
        }
    }

    let cmd = SetBrush {
        entity: brush_entity,
        old,
        new: brush.clone(),
        label: "Edit face UV".to_string(),
    };
    history.push_executed(Box::new(cmd));
}

/// True when the brush face inspector has at least one face selected
/// in the active brush; gates the per-face operators so the buttons
/// grey out when there's nothing to act on.
fn brush_face_with_selection(
    brush_selection: Res<BrushSelection>,
    edit_mode: Res<EditMode>,
) -> bool {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return false;
    }
    brush_selection.active_brush.is_some()
        && brush_selection
            .active_sub()
            .is_some_and(|s| !s.faces.is_empty())
}

#[operator(
    id = "brush.face.clear_material",
    label = "Clear Material",
    description = "Remove the material/texture from the selected faces.",
    is_available = brush_face_with_selection,
)]
pub(crate) fn brush_face_clear_material(
    _: In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
    mut commands: Commands,
) -> OperatorResult {
    let brush_entity = brush_selection.active_brush?;
    let faces: Vec<usize> = brush_selection
        .sub(brush_entity)
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    let mut brush = brushes.get_mut(brush_entity)?;

    let old = brush.clone();
    for &face_idx in &faces {
        if face_idx < brush.faces.len() {
            brush.faces[face_idx].material = Handle::default();
        }
    }

    let cmd = SetBrush {
        entity: brush_entity,
        old,
        new: brush.clone(),
        label: "Clear material".to_string(),
    };
    history.push_executed(Box::new(cmd));
    commands.entity(brush_entity).insert(super::InspectorDirty);
    OperatorResult::Finished
}

#[operator(
    id = "brush.face.apply_texture_to_all",
    label = "Apply Material to All Faces",
    description = "Copy the first selected face's material and UV transform onto every face of the brush.",
    is_available = brush_face_with_selection,
)]
pub(crate) fn brush_face_apply_texture_to_all(
    _: In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
    mut commands: Commands,
) -> OperatorResult {
    let brush_entity = brush_selection.active_brush?;
    let source_idx = brush_selection
        .sub(brush_entity)
        .and_then(|s| s.faces.first().copied())?;
    let mut brush = brushes.get_mut(brush_entity)?;
    if source_idx >= brush.faces.len() {
        return OperatorResult::Cancelled;
    }
    let source = brush.faces[source_idx].clone();

    let old = brush.clone();
    for face in &mut brush.faces {
        face.material = source.material.clone();
        face.uv_scale = source.uv_scale;
        face.uv_offset = source.uv_offset;
        face.uv_rotation = source.uv_rotation;
    }

    let cmd = SetBrush {
        entity: brush_entity,
        old,
        new: brush.clone(),
        label: "Apply material to all faces".to_string(),
    };
    history.push_executed(Box::new(cmd));
    commands.entity(brush_entity).insert(super::InspectorDirty);
    OperatorResult::Finished
}

#[operator(
    id = "brush.face.set_uv_scale_preset",
    label = "Set UV Scale",
    description = "Set the UV scale of the selected faces to the given uniform value.",
    is_available = brush_face_with_selection,
    params(scale(f64, doc = "Uniform UV scale (the same value for U and V).")),
)]
pub(crate) fn brush_face_set_uv_scale_preset(
    In(params): In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let scale_value = params.as_float("scale").unwrap_or(1.0) as f32;
    let brush_entity = brush_selection.active_brush?;
    let faces: Vec<usize> = brush_selection
        .sub(brush_entity)
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    let mut brush = brushes.get_mut(brush_entity)?;

    let old = brush.clone();
    let scale = Vec2::splat(scale_value);
    for &face_idx in &faces {
        if face_idx < brush.faces.len() {
            brush.faces[face_idx].uv_scale = scale;
        }
    }

    let cmd = SetBrush {
        entity: brush_entity,
        old,
        new: brush.clone(),
        label: "Set UV scale preset".to_string(),
    };
    history.push_executed(Box::new(cmd));
    OperatorResult::Finished
}

#[operator(
    id = "brush.clear_all_materials",
    label = "Clear All Materials",
    description = "Clear materials from every face of the selected brushes (expanding any selected non-brush parents into their child brushes)."
)]
pub(crate) fn brush_clear_all_materials(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
    children_query: Query<&Children>,
    mut commands: Commands,
) -> OperatorResult {
    let targets: Vec<Entity> = crate::brush::shown_edit_brushes(
        &selection.entities,
        |e| brushes.contains(e),
        |e| {
            children_query
                .get(e)
                .map(|c| c.iter().collect())
                .unwrap_or_default()
        },
    );

    let mut group_commands: Vec<Box<dyn jackdaw_commands::EditorCommand>> = Vec::new();
    for entity in targets {
        if let Ok(mut brush) = brushes.get_mut(entity) {
            let has_any_material = brush.faces.iter().any(|f| f.material != Handle::default());
            if !has_any_material {
                continue;
            }
            let old = brush.clone();
            for face in brush.faces.iter_mut() {
                face.material = Handle::default();
            }
            let cmd = SetBrush {
                entity,
                old,
                new: brush.clone(),
                label: "Clear all materials".to_string(),
            };
            group_commands.push(Box::new(cmd));
            commands.entity(entity).insert(super::InspectorDirty);
        }
    }
    if !group_commands.is_empty() {
        history.push_executed(Box::new(jackdaw_commands::CommandGroup {
            commands: group_commands,
            label: "Clear all materials".to_string(),
        }));
    }
    OperatorResult::Finished
}
