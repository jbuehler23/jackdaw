use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    combobox::{self, ComboBoxChangeEvent},
    text_edit::{
        self, TextEditCommitEvent, TextEditDragging, TextEditProps, TextEditVariant,
        TextEditWrapper, TextInputBuffer, TextInputQueue, format_numeric_value,
        set_text_input_value,
    },
    tokens,
};

use super::{TerrainBrushSettings, TerrainEditMode};
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainGenerateState>()
        .add_systems(
            Update,
            (update_terrain_inspector, sync_brush_fields).run_if(in_state(crate::AppState::Editor)),
        )
        .add_observer(on_terrain_text_commit);
}

// --- State ---

/// Persistent generation settings, preserved across inspector rebuilds.
#[derive(Resource, Default)]
pub struct TerrainGenerateState {
    pub settings: jackdaw_terrain::GenerateSettings,
    pub erosion: jackdaw_terrain::ErosionParams,
}

/// Marker for the terrain inspector container.
#[derive(Component)]
pub struct TerrainInspectorContainer;

/// Spawns the terrain inspector container. Called from the component display system.
pub fn spawn_terrain_inspector_container(commands: &mut Commands, parent: Entity) {
    commands.spawn((
        TerrainInspectorContainer,
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            row_gap: px(tokens::SPACING_SM),
            ..Default::default()
        },
        ChildOf(parent),
    ));
}

/// Tracks what we last rendered to avoid unnecessary rebuilds.
#[derive(Default)]
struct InspectorState {
    terrain_entity: Option<Entity>,
    edit_mode_is_sculpt: bool,
}

// --- Field binding tags ---

#[derive(Component, Clone, Copy)]
enum BrushField {
    Radius,
    Strength,
    Falloff,
}

#[derive(Component, Clone, Copy)]
enum GenField {
    Seed,
    Frequency,
    Octaves,
    Lacunarity,
    Persistence,
    Amplitude,
    Offset,
}

#[derive(Component, Clone, Copy)]
enum ErosionField {
    Iterations,
    ErosionRadius,
    Inertia,
    Capacity,
    Deposition,
    Erosion,
    Evaporation,
}

fn update_terrain_inspector(
    mut commands: Commands,
    selection: Res<Selection>,
    edit_mode: Res<TerrainEditMode>,
    terrains: Query<(), With<jackdaw_jsn::Terrain>>,
    container_query: Query<(Entity, Option<&Children>), With<TerrainInspectorContainer>>,
    mut local_state: Local<InspectorState>,
    brush_settings: Res<TerrainBrushSettings>,
    gen_state: Res<TerrainGenerateState>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
) {
    // Determine if we should show terrain inspector
    let terrain_entity = selection.primary().filter(|&e| terrains.contains(e));

    let is_sculpt = matches!(*edit_mode, TerrainEditMode::Sculpt(_));

    let changed = local_state.terrain_entity != terrain_entity
        || local_state.edit_mode_is_sculpt != is_sculpt
        || (terrain_entity.is_some() && edit_mode.is_changed());

    if !changed {
        return;
    }

    local_state.terrain_entity = terrain_entity;
    local_state.edit_mode_is_sculpt = is_sculpt;

    // Ensure container exists
    let container = if let Ok((entity, children)) = container_query.single() {
        // Clear existing content
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        entity
    } else {
        // Will be created by the component display system -- skip this frame
        return;
    };

    let Some(_terrain_entity) = terrain_entity else {
        return;
    };

    // --- Brush settings section (when sculpt mode active) ---
    if is_sculpt {
        let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
            &mut commands,
            "Sculpt Brush",
            &icon_font.0,
            container,
        );

        spawn_labeled_field(
            &mut commands,
            body,
            "Radius",
            "Area of effect for the brush",
            brush_settings.radius as f64,
            BrushField::Radius,
        );
        spawn_labeled_field(
            &mut commands,
            body,
            "Strength",
            "How quickly the brush modifies terrain",
            brush_settings.strength as f64,
            BrushField::Strength,
        );
        spawn_labeled_field(
            &mut commands,
            body,
            "Falloff",
            "Brush edge softness (1=linear, 2=smooth)",
            brush_settings.falloff as f64,
            BrushField::Falloff,
        );
    }

    // --- Generation section (always shown when terrain selected) ---
    let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
        &mut commands,
        "Terrain Generation",
        &icon_font.0,
        container,
    );

    // Noise type combobox
    let noise_options: Vec<String> = jackdaw_terrain::NoiseType::ALL
        .iter()
        .map(|n| n.label().to_string())
        .collect();
    let noise_row = commands
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
        Text::new("Noise Type"),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: px(80.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(noise_row),
    ));
    commands
        .spawn((
            combobox::combobox_with_selected(noise_options, gen_state.settings.noise_type.index()),
            ChildOf(noise_row),
        ))
        .observe(
            |event: On<ComboBoxChangeEvent>, mut gen_state: ResMut<TerrainGenerateState>| {
                gen_state.settings.noise_type =
                    jackdaw_terrain::NoiseType::from_index(event.selected);
            },
        );

    spawn_gen_field(
        &mut commands,
        body,
        "Seed",
        "Same seed always produces the same terrain",
        gen_state.settings.seed as f64,
        GenField::Seed,
    );
    spawn_gen_field(
        &mut commands,
        body,
        "Frequency",
        "Lower = broader features, higher = finer detail",
        gen_state.settings.frequency,
        GenField::Frequency,
    );
    spawn_gen_field(
        &mut commands,
        body,
        "Octaves",
        "Layers of noise stacked together. More = finer detail",
        gen_state.settings.octaves as f64,
        GenField::Octaves,
    );
    spawn_gen_field(
        &mut commands,
        body,
        "Lacunarity",
        "How much each octave's frequency increases",
        gen_state.settings.lacunarity,
        GenField::Lacunarity,
    );
    spawn_gen_field(
        &mut commands,
        body,
        "Persistence",
        "How much each octave contributes. Lower = subtler",
        gen_state.settings.persistence,
        GenField::Persistence,
    );
    spawn_gen_field(
        &mut commands,
        body,
        "Amplitude",
        "Overall height scale of the generated terrain",
        gen_state.settings.amplitude as f64,
        GenField::Amplitude,
    );
    spawn_gen_field(
        &mut commands,
        body,
        "Offset",
        "Vertical offset added after generation",
        gen_state.settings.offset as f64,
        GenField::Offset,
    );

    // Generate button
    commands.spawn((
        button::button(
            ButtonProps::new("Generate")
                .with_variant(ButtonVariant::Primary)
                .call_operator(crate::terrain::ops::TerrainGenerateOp::ID),
        ),
        ChildOf(body),
    ));

    // --- Erosion section ---
    let (_section, ebody) = jackdaw_feathers::collapsible::collapsible_section(
        &mut commands,
        "Hydraulic Erosion",
        &icon_font.0,
        container,
    );

    spawn_erosion_field(
        &mut commands,
        ebody,
        "Iterations",
        "Number of water droplets simulated",
        gen_state.erosion.iterations as f64,
        ErosionField::Iterations,
    );
    spawn_erosion_field(
        &mut commands,
        ebody,
        "Erosion Radius",
        "Area of effect for each erosion step",
        gen_state.erosion.erosion_radius as f64,
        ErosionField::ErosionRadius,
    );
    spawn_erosion_field(
        &mut commands,
        ebody,
        "Inertia",
        "How much a droplet keeps its previous direction",
        gen_state.erosion.inertia as f64,
        ErosionField::Inertia,
    );
    spawn_erosion_field(
        &mut commands,
        ebody,
        "Capacity",
        "How much sediment water can carry",
        gen_state.erosion.capacity as f64,
        ErosionField::Capacity,
    );
    spawn_erosion_field(
        &mut commands,
        ebody,
        "Deposition",
        "Rate sediment is dropped when water slows",
        gen_state.erosion.deposition as f64,
        ErosionField::Deposition,
    );
    spawn_erosion_field(
        &mut commands,
        ebody,
        "Erosion Rate",
        "Rate terrain is dissolved by flowing water",
        gen_state.erosion.erosion as f64,
        ErosionField::Erosion,
    );
    spawn_erosion_field(
        &mut commands,
        ebody,
        "Evaporation",
        "How quickly water droplets shrink",
        gen_state.erosion.evaporation as f64,
        ErosionField::Evaporation,
    );

    // Erode button
    commands.spawn((
        button::button(
            ButtonProps::new("Erode")
                .with_variant(ButtonVariant::Primary)
                .call_operator(crate::terrain::ops::TerrainErodeOp::ID),
        ),
        ChildOf(ebody),
    ));
}

/// Sync brush resource values into existing `text_edit` widgets without rebuilding the UI.
fn sync_brush_fields(
    brush_settings: Res<TerrainBrushSettings>,
    input_focus: Res<InputFocus>,
    outer_query: Query<(Entity, &BrushField, &Children)>,
    wrapper_query: Query<&TextEditWrapper>,
    dragging_query: Query<(), With<TextEditDragging>>,
    children_query: Query<&Children>,
    mut queue_query: Query<(&TextInputBuffer, &mut TextInputQueue)>,
) {
    if !brush_settings.is_changed() {
        return;
    }
    for (_outer, field, children) in &outer_query {
        let new_val = match field {
            BrushField::Radius => brush_settings.radius as f64,
            BrushField::Strength => brush_settings.strength as f64,
            BrushField::Falloff => brush_settings.falloff as f64,
        };
        let formatted = format_numeric_value(new_val, TextEditVariant::NumericF32);

        // Find inner entity: outer → wrapper child → TextEditWrapper → inner entity
        let mut found = false;
        for child in children.iter() {
            if let Ok(wrapper) = wrapper_query.get(child) {
                if dragging_query.get(child).is_ok() || input_focus.0 == Some(wrapper.0) {
                    found = true;
                    break;
                }
                if let Ok((buffer, mut queue)) = queue_query.get_mut(wrapper.0) {
                    let current: String = buffer.get_text();
                    if current != formatted {
                        set_text_input_value(&mut queue, formatted.clone());
                    }
                }
                found = true;
                break;
            }
        }
        if found {
            continue;
        }
        // One more level: wrapper child may be nested
        for child in children.iter() {
            if let Ok(grandchildren) = children_query.get(child) {
                for gc in grandchildren.iter() {
                    if let Ok(wrapper) = wrapper_query.get(gc) {
                        if dragging_query.get(gc).is_ok() || input_focus.0 == Some(wrapper.0) {
                            found = true;
                            break;
                        }
                        if let Ok((buffer, mut queue)) = queue_query.get_mut(wrapper.0) {
                            let current: String = buffer.get_text();
                            if current != formatted {
                                set_text_input_value(&mut queue, formatted.clone());
                            }
                        }
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
        }
    }
}

// --- Spawn helpers ---

fn spawn_labeled_field(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f64,
    field: BrushField,
) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: 10.0,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        text_edit::text_edit(
            TextEditProps::default()
                .numeric_f32()
                .with_default_value(value.to_string()),
        ),
        field,
        ChildOf(row),
    ));
}

fn spawn_gen_field(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f64,
    field: GenField,
) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: 10.0,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        text_edit::text_edit(
            TextEditProps::default()
                .numeric_f32()
                .with_default_value(value.to_string()),
        ),
        field,
        ChildOf(row),
    ));
}

fn spawn_erosion_field(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f64,
    field: ErosionField,
) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: 10.0,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        text_edit::text_edit(
            TextEditProps::default()
                .numeric_f32()
                .with_default_value(value.to_string()),
        ),
        field,
        ChildOf(row),
    ));
}

/// Handle `TextEditCommitEvent` for terrain inspector fields (brush, gen, erosion).
fn on_terrain_text_commit(
    event: On<TextEditCommitEvent>,
    brush_bindings: Query<&BrushField>,
    gen_bindings: Query<&GenField>,
    erosion_bindings: Query<&ErosionField>,
    child_of_query: Query<&ChildOf>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
    mut gen_state: ResMut<TerrainGenerateState>,
) {
    let value: f64 = event.text.parse().unwrap_or(0.0);

    // Walk up from committed entity to find a field binding
    let mut current = event.entity;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        let parent = child_of.parent();

        if let Ok(&field) = brush_bindings.get(parent) {
            match field {
                BrushField::Radius => brush_settings.radius = value as f32,
                BrushField::Strength => brush_settings.strength = value as f32,
                BrushField::Falloff => brush_settings.falloff = value as f32,
            }
            return;
        }
        if let Ok(&field) = gen_bindings.get(parent) {
            match field {
                GenField::Seed => gen_state.settings.seed = value as u32,
                GenField::Frequency => gen_state.settings.frequency = value,
                GenField::Octaves => gen_state.settings.octaves = value as usize,
                GenField::Lacunarity => gen_state.settings.lacunarity = value,
                GenField::Persistence => gen_state.settings.persistence = value,
                GenField::Amplitude => gen_state.settings.amplitude = value as f32,
                GenField::Offset => gen_state.settings.offset = value as f32,
            }
            return;
        }
        if let Ok(&field) = erosion_bindings.get(parent) {
            match field {
                ErosionField::Iterations => gen_state.erosion.iterations = value as u32,
                ErosionField::ErosionRadius => gen_state.erosion.erosion_radius = value as u32,
                ErosionField::Inertia => gen_state.erosion.inertia = value as f32,
                ErosionField::Capacity => gen_state.erosion.capacity = value as f32,
                ErosionField::Deposition => gen_state.erosion.deposition = value as f32,
                ErosionField::Erosion => gen_state.erosion.erosion = value as f32,
                ErosionField::Evaporation => gen_state.erosion.evaporation = value as f32,
            }
            return;
        }
        current = parent;
    }
}
