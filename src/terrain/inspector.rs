use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    combobox::{self, ComboBoxChangeEvent},
    text_edit::{
        self, TextEditCommitEvent, TextEditDragging, TextEditProps, TextEditVariant,
        TextEditWrapper, format_numeric_value, set_text_input_value,
    },
    tokens,
};

use super::{TerrainBrushSettings, TerrainEditMode, TerrainPaintState};
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
    edit_mode_is_paint: bool,
    /// Channel table shape and selection, so the tile grid redraws when a
    /// channel is added, removed, renamed or picked. Compared rather than
    /// change-detected because `TerrainPaintState` also carries the brush
    /// position, which moves nearly every frame.
    channel_signature: Option<ChannelSignature>,
    /// Whether quantization was on last time the panel was drawn. Only
    /// the flag is tracked: the two numeric fields are typed into, and
    /// rebuilding the section on every committed keystroke would take
    /// the focus away mid-edit.
    quantization_enabled: bool,
    /// Scatter palette, mask selection, toggles and last-run report.
    /// Same reasoning as `channel_signature`: the numeric fields are
    /// typed into and must not redraw under the caret.
    scatter_signature: Option<crate::terrain::scatter::ScatterSignature>,
}

/// What the channel UI depends on. Anything else changing on the terrain
/// or the paint state leaves the panel alone.
#[derive(PartialEq, Clone)]
struct ChannelSignature {
    channels: Vec<(String, usize)>,
    active_channel: usize,
    active_entry: usize,
    show_channel: bool,
}

// --- Field binding tags ---

#[derive(Component, Clone, Copy)]
enum BrushField {
    Radius,
    Strength,
    Falloff,
}

#[derive(Component, Clone, Copy)]
enum QuantField {
    CellSize,
    HeightStep,
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
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    container_query: Query<(Entity, Option<&Children>), With<TerrainInspectorContainer>>,
    mut local_state: Local<InspectorState>,
    brush_settings: Res<TerrainBrushSettings>,
    gen_state: Res<TerrainGenerateState>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
    paint_state: Res<TerrainPaintState>,
    terrain_data: Query<&jackdaw_scene_types::Terrain>,
    scatter_state: Res<crate::terrain::scatter::TerrainScatterState>,
    scatter_report: Res<crate::terrain::scatter::TerrainScatterReport>,
) {
    // Determine if we should show terrain inspector
    let terrain_entity = selection.primary().filter(|&e| terrains.contains(e));

    let is_sculpt = matches!(*edit_mode, TerrainEditMode::Sculpt(_));
    let is_paint = *edit_mode == TerrainEditMode::Paint;

    let signature = terrain_entity
        .and_then(|e| terrain_data.get(e).ok())
        .map(|terrain| ChannelSignature {
            channels: terrain
                .channels
                .iter()
                .map(|channel| (channel.name.clone(), channel.palette.len()))
                .collect(),
            active_channel: paint_state.active_channel,
            active_entry: paint_state.active_entry,
            show_channel: paint_state.show_channel,
        });

    let quantization = terrain_entity
        .and_then(|e| terrain_data.get(e).ok())
        .map(|terrain| terrain.quantization.clone())
        .unwrap_or_default();

    let scatter_signature =
        terrain_entity.map(|_| crate::terrain::scatter::signature(&scatter_state, &scatter_report));

    let changed = local_state.terrain_entity != terrain_entity
        || local_state.edit_mode_is_sculpt != is_sculpt
        || local_state.edit_mode_is_paint != is_paint
        || local_state.channel_signature != signature
        || local_state.quantization_enabled != quantization.enabled
        || local_state.scatter_signature != scatter_signature
        || (terrain_entity.is_some() && edit_mode.is_changed());

    if !changed {
        return;
    }

    local_state.terrain_entity = terrain_entity;
    local_state.edit_mode_is_sculpt = is_sculpt;
    local_state.edit_mode_is_paint = is_paint;
    local_state.channel_signature = signature;
    local_state.quantization_enabled = quantization.enabled;
    local_state.scatter_signature = scatter_signature;

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

    let Some(terrain_entity_id) = terrain_entity else {
        return;
    };

    // --- Paint channels (when paint mode active) ---
    // Above the brush, matching every reference tool: you pick what you
    // are painting first, then how wide the brush is.
    if is_paint {
        let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
            &mut commands,
            "Paint Channels",
            &icon_font.0,
            container,
        );
        let terrain = terrain_data.get(terrain_entity_id).ok().cloned();
        spawn_channel_ui(&mut commands, body, terrain.as_ref(), &paint_state);
    }

    // --- Brush settings section (sculpt and paint share one brush) ---
    if is_sculpt || is_paint {
        let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
            &mut commands,
            "Brush",
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

    // --- Scatter section (always shown when terrain selected) ---
    // Declarative rather than brush-driven, so unlike Paint Channels it
    // is not gated on an edit mode: there is no scatter tool to pick up.
    {
        let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
            &mut commands,
            "Scatter",
            &icon_font.0,
            container,
        );
        let terrain = terrain_data.get(terrain_entity_id).ok().cloned();
        crate::terrain::scatter::spawn_scatter_ui(
            &mut commands,
            body,
            terrain.as_ref(),
            &scatter_state,
            &scatter_report,
        );
    }

    // --- Quantization section (always shown when terrain selected) ---
    // It describes the terrain itself, not a tool, so it is not gated on
    // an edit mode the way the brush is.
    {
        let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
            &mut commands,
            "Quantization",
            &icon_font.0,
            container,
        );

        commands.spawn((
            button::button(
                ButtonProps::new(if quantization.enabled {
                    "Quantization: On"
                } else {
                    "Quantization: Off"
                })
                .with_variant(if quantization.enabled {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Default
                })
                .call_operator(crate::terrain::quantize_ops::TerrainQuantizeToggleOp::ID),
            ),
            ChildOf(body),
        ));

        spawn_quant_field(
            &mut commands,
            body,
            "Cell Size",
            "World units per cell edge. 0 leaves the terrain's size alone",
            quantization.cell_size as f64,
            QuantField::CellSize,
        );
        spawn_quant_field(
            &mut commands,
            body,
            "Height Step",
            "World units per elevation step. 0 leaves heights unsnapped",
            quantization.height_step as f64,
            QuantField::HeightStep,
        );

        // Sculpt, generate and erode snap as they go; this is for the
        // heights that were already there when the setting was turned on.
        commands.spawn((
            button::button(
                ButtonProps::new("Apply")
                    .with_variant(ButtonVariant::Primary)
                    .call_operator(crate::terrain::quantize_ops::TerrainQuantizeApplyOp::ID),
            ),
            ChildOf(body),
        ));
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
            font_size: tokens::TEXT_SIZE_SM,
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
    mut editable_query: Query<&mut bevy::text::EditableText>,
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

        // Find inner entity: outer -> wrapper child -> TextEditWrapper -> inner entity
        let mut found = false;
        for child in children.iter() {
            if let Ok(wrapper) = wrapper_query.get(child) {
                if dragging_query.get(child).is_ok() || input_focus.get() == Some(wrapper.0) {
                    found = true;
                    break;
                }
                if let Ok(mut editable) = editable_query.get_mut(wrapper.0) {
                    let current: String = editable.value().into_iter().collect();
                    if current != formatted {
                        set_text_input_value(&mut editable, formatted.clone());
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
                        if dragging_query.get(gc).is_ok() || input_focus.get() == Some(wrapper.0) {
                            found = true;
                            break;
                        }
                        if let Ok(mut editable) = editable_query.get_mut(wrapper.0) {
                            let current: String = editable.value().into_iter().collect();
                            if current != formatted {
                                set_text_input_value(&mut editable, formatted.clone());
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

// --- Channel UI ---

/// Edge of one channel / palette tile.
const TILE_PX: f32 = 52.0;
/// Height of the accent bar marking the selected tile.
const ACCENT_PX: f32 = 3.0;

/// The channel tile grid, the active channel's palette swatches, and the
/// painted-state toggle.
///
/// A grid of tiles rather than a text list: each tile is click-to-select
/// with an accent bar when active, and a `+` tile at the end adds one. The
/// tiles show a palette swatch rather than a texture thumbnail because a
/// channel is an integer layer, not a material.
fn spawn_channel_ui(
    commands: &mut Commands,
    parent: Entity,
    terrain: Option<&jackdaw_scene_types::Terrain>,
    paint: &TerrainPaintState,
) {
    let empty: &[jackdaw_scene_types::TerrainChannel] = &[];
    let channels = terrain.map(|t| t.channels.as_slice()).unwrap_or(empty);

    spawn_hint(
        commands,
        parent,
        "Channels are yours to name. Paint what a cell is, not what it looks like.",
    );

    let grid = spawn_tile_grid(commands, parent);
    for (index, channel) in channels.iter().enumerate() {
        let swatch = channel
            .palette
            .iter()
            .find(|entry| entry.value != 0)
            .or_else(|| channel.palette.first())
            .map(|entry| entry.color)
            .unwrap_or(Color::srgb(0.5, 0.5, 0.5));
        spawn_tile(
            commands,
            grid,
            swatch,
            &channel.name,
            index == paint.active_channel,
            crate::terrain::channel_ops::TerrainChannelSelectOp::ID,
            Some(index),
        );
        spawn_tile_remove(
            commands,
            grid,
            crate::terrain::channel_ops::TerrainChannelRemoveOp::ID,
            index,
        );
    }
    spawn_add_tile(
        commands,
        grid,
        crate::terrain::channel_ops::TerrainChannelAddOp::ID,
    );

    let Some(channel) = channels.get(paint.active_channel) else {
        return;
    };

    spawn_hint(commands, parent, "Value");
    let values = spawn_tile_grid(commands, parent);
    for (index, entry) in channel.palette.iter().enumerate() {
        spawn_tile(
            commands,
            values,
            entry.color,
            &entry.label,
            index == paint.active_entry,
            crate::terrain::channel_ops::TerrainChannelValueSelectOp::ID,
            Some(index),
        );
    }
    spawn_add_tile(
        commands,
        values,
        crate::terrain::channel_ops::TerrainChannelValueAddOp::ID,
    );

    // The painted-state view. Without it the user is painting data with
    // no feedback at all.
    commands.spawn((
        button::button(
            ButtonProps::new(if paint.show_channel {
                "Hide Painted Values"
            } else {
                "Show Painted Values"
            })
            .with_variant(if paint.show_channel {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Default
            })
            .call_operator(crate::terrain::channel_ops::TerrainChannelToggleViewOp::ID),
        ),
        ChildOf(parent),
    ));
}

pub(super) fn spawn_hint(commands: &mut Commands, parent: Entity, text: &str) {
    commands.spawn((
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(parent),
    ));
}

pub(super) fn spawn_tile_grid(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(tokens::SPACING_XS),
                row_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id()
}

/// One selectable tile: colour swatch, accent bar, name under it.
/// Clicking dispatches `op_id` with the tile's index.
pub(super) fn spawn_tile(
    commands: &mut Commands,
    parent: Entity,
    swatch: Color,
    label: &str,
    selected: bool,
    op_id: &'static str,
    index: Option<usize>,
) {
    let tile = commands
        .spawn((
            Node {
                width: px(TILE_PX),
                flex_direction: FlexDirection::Column,
                row_gap: px(2.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: px(TILE_PX * 0.62),
            ..Default::default()
        },
        BackgroundColor(swatch),
        ChildOf(tile),
    ));

    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: px(ACCENT_PX),
            ..Default::default()
        },
        BackgroundColor(if selected {
            tokens::ACCENT_BLUE
        } else {
            Color::NONE
        }),
        ChildOf(tile),
    ));

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(if selected {
            tokens::TEXT_BODY_COLOR.into()
        } else {
            tokens::TEXT_SECONDARY
        }),
        ChildOf(tile),
    ));

    commands
        .entity(tile)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let mut call = commands.operator(op_id);
            if let Some(index) = index {
                call = call.param("index", index as i64);
            }
            call.call();
        });
}

/// The `+` tile: the way to add a layer sits at the end of the grid, not
/// in a separate menu.
pub(super) fn spawn_add_tile(commands: &mut Commands, parent: Entity, op_id: &'static str) {
    let tile = commands
        .spawn((
            Node {
                width: px(TILE_PX),
                height: px(TILE_PX * 0.62),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1.0)),
                ..Default::default()
            },
            BorderColor::all(tokens::TEXT_SECONDARY.with_alpha(0.4)),
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new("+"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(tile),
    ));
    commands
        .entity(tile)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.operator(op_id).call();
        });
}

/// A small remove affordance beside the tile rather than on it, so the
/// swatch stays a clean colour sample.
///
/// `op_id` is dispatched with the tile's index, so the same affordance
/// serves the channel grid and the scatter palette.
pub(super) fn spawn_tile_remove(
    commands: &mut Commands,
    parent: Entity,
    op_id: &'static str,
    index: usize,
) {
    let button = commands
        .spawn((
            Node {
                width: px(14.0),
                height: px(TILE_PX * 0.62),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new("x"),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(button),
    ));
    commands
        .entity(button)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.operator(op_id).param("index", index as i64).call();
        });
}

// --- Spawn helpers ---

/// A label, a one-line explanation, and a numeric `text_edit` tagged with
/// `field`.
///
/// Generic over the tag so a panel section can bring its own binding enum
/// and read commits back in its own observer, rather than every terrain
/// field having to be understood by one handler.
pub(super) fn spawn_labeled_field<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f64,
    field: C,
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
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
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

fn spawn_quant_field(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f64,
    field: QuantField,
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
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
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
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
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
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    commands.spawn((
        Text::new(tooltip),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
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
    quant_bindings: Query<&QuantField>,
    gen_bindings: Query<&GenField>,
    erosion_bindings: Query<&ErosionField>,
    child_of_query: Query<&ChildOf>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
    mut gen_state: ResMut<TerrainGenerateState>,
    selection: Res<Selection>,
    mut commands: Commands,
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
        // Unlike the brush and generation fields, these live on the
        // component rather than a resource, so the edit has to reach the
        // scene document. `commit_quantization` is the one place that
        // knows how.
        if let Ok(&field) = quant_bindings.get(parent) {
            let Some(entity) = selection.primary() else {
                return;
            };
            let value = value as f32;
            commands.queue(move |world: &mut World| {
                crate::terrain::quantize_ops::commit_quantization(world, entity, |q| match field {
                    QuantField::CellSize => q.cell_size = value,
                    QuantField::HeightStep => q.height_step = value,
                });
            });
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
