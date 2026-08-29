//! The contextual options bar: a slim strip above the viewport, owned by
//! whichever terrain tool is active.
//!
//! Tool *selection* lives in the left-edge palette (`palette.rs`); this bar
//! shows the active tool's own fields and is hidden in Select mode.

use bevy::input_focus::InputFocus;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    combobox::{self, ComboBoxChangeEvent},
    number_input::is_focused_for_editing,
    tokens,
    tooltip::Tooltip,
};

use super::navmesh_bake::TerrainNavmeshState;
use super::regions::{RegionVisibility, TerrainRegionView};
use super::texture_ops::TerrainPaintTargetOp;
use super::ui_fields::{
    FieldKind, TerrainDefaultFontRoot, spawn_add_tile, spawn_bar_swatch, spawn_checkbox,
    spawn_hint, spawn_scrub_chip, spawn_tile, spawn_tile_grid, spawn_tile_remove,
};
use super::{PaintDomain, TerrainBrushSettings, TerrainEditMode, TerrainPaintState};
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            update_options_bar_content,
            sync_brush_fields,
            sync_texture_opacity_field,
            sync_overlay_blend_swatches,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    )
    .add_observer(on_scrub_value_change)
    .add_observer(on_terrain_checkbox_value_change);
}

/// Marker for the terrain options bar container.
#[derive(Component)]
pub struct TerrainOptionsBar;

/// Height of one row of the bar: a field's 22px control plus the bar's own
/// vertical padding. The tool palette measures its constant offset from this,
/// so the bar's first row and the palette below it read as one strip whatever
/// the active tool put on the bar.
pub(super) const ROW_HEIGHT_PX: f32 = 22.0 + 2.0 * tokens::SPACING_SM;

/// The bar's padding, keeping the tool palette's column clear on the left. The
/// bar wraps to as many rows as the active tool needs, and those rows run down
/// beside the palette rather than over it.
fn bar_padding() -> UiRect {
    UiRect {
        left: Val::Px(super::palette::PALETTE_GUTTER_PX),
        right: Val::Px(tokens::SPACING_MD),
        top: Val::Px(tokens::SPACING_SM),
        bottom: Val::Px(tokens::SPACING_SM),
    }
}

/// Tags the Quantization section's on/off checkbox. `ValueChange<bool>` fires
/// for every checkbox in the editor, so the handler filters by the tag on the
/// event's source entity rather than by the event itself.
#[derive(Component)]
struct QuantizationToggleCheckbox;

/// Tags the Paint bar's "show painted values" checkbox, same reasoning as
/// [`QuantizationToggleCheckbox`].
#[derive(Component)]
struct ShowPaintedValuesCheckbox;

/// Tags the Regions bar's grid checkbox, same reasoning as
/// [`QuantizationToggleCheckbox`].
#[derive(Component)]
struct RegionGridCheckbox;

/// Tags the texture bar's "restore auto" checkbox, same reasoning as
/// [`QuantizationToggleCheckbox`].
#[derive(Component)]
struct RestoreAutoCheckbox;

/// Tags the Navmesh bar's overlay checkbox, same reasoning as
/// [`QuantizationToggleCheckbox`].
#[derive(Component)]
struct NavmeshOverlayCheckbox;

/// Builds the options bar as a `bsn!` Scene. Starts empty and hidden
/// (`Display::None`); content is rebuilt by `update_options_bar_content` from
/// the active [`TerrainEditMode`].
///
/// Spawned standalone via `spawn_scene` (see `viewport::build_viewport_panel`),
/// because a Scene cannot nest inside a Bundle `children!` tree; the spawn site
/// attaches the [`TerrainOptionsBar`] and `EditorEntity` markers.
pub fn terrain_options_bar() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            padding: bar_padding(),
            column_gap: px(tokens::SPACING_MD),
            row_gap: px(tokens::SPACING_SM),
            width: percent(100),
            max_height: px(160.0),
            overflow: Overflow::scroll_y(),
            flex_shrink: 0.0,
            display: Display::None,
        }
        BackgroundColor(tokens::TOOLBAR_BG)
        TerrainDefaultFontRoot
    }
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

/// Tags the Texture-mode bar's Opacity chip. Its own binding enum because it
/// writes [`TerrainPaintState`], not [`TerrainBrushSettings`] like
/// [`BrushField`]'s chips.
#[derive(Component, Clone, Copy)]
enum TextureField {
    Opacity,
}

/// Tags the Navmesh bar's agent chips. They describe the character a bake is
/// for, which is authored on the terrain, so they write the scene document
/// rather than a resource.
#[derive(Component, Clone, Copy)]
enum NavmeshField {
    AgentRadius,
    AgentHeight,
    MaxSlope,
}

/// What the bar last rendered, so it rebuilds only on a structural change
/// (switching tool, terrain or paint target, or the paint channel table
/// changing shape) and never while a scrub field is being dragged or typed
/// into.
#[derive(Default, PartialEq, Clone, Debug)]
struct BarState {
    terrain_entity: Option<Entity>,
    mode: TerrainEditMode,
    channel_signature: Option<ChannelSignature>,
    texture_signature: Option<TextureBarSignature>,
    region_signature: Option<RegionBarSignature>,
    navmesh_signature: Option<NavmeshBarSignature>,
}

/// What the Navmesh bar shows beyond its chips: the overlay toggle and the line
/// reporting the last bake, which changes as a bake runs and again when the
/// ground moves out from under it.
#[derive(PartialEq, Clone, Debug)]
struct NavmeshBarSignature {
    show_overlay: bool,
    summary: String,
    agent: jackdaw_scene_types::TerrainNavmesh,
}

/// What the Regions bar shows: the two view choices and which region is active,
/// which the bar names in words.
#[derive(PartialEq, Clone, Debug)]
struct RegionBarSignature {
    show_grid: bool,
    visibility: RegionVisibility,
    active: Option<jackdaw_terrain::RegionCoord>,
}

#[derive(PartialEq, Clone, Debug)]
struct ChannelSignature {
    channels: Vec<(String, usize)>,
    active_channel: usize,
    active_entry: usize,
}

#[derive(PartialEq, Clone, Debug)]
struct TextureBarSignature {
    domain: PaintDomain,
    active_texture_id: u8,
    /// The selected slot's material name, so the bar's readout rebuilds when
    /// the list changes under a fixed id, not only when the id changes.
    slot_material: Option<String>,
    /// Slot 0's name, shown as the left half of the overlay-blend pair.
    base_material: Option<String>,
    /// How many albedo thumbnails have decoded. The swatches are blank
    /// placeholders until they land, so the bar rebuilds when they do.
    thumbnails_ready: usize,
    /// Whether the brush is restoring rather than painting, so the bar rebuilds
    /// when an undo or a scripted call flips it.
    restore_auto: bool,
}

fn update_options_bar_content(
    mut commands: Commands,
    selection: Res<Selection>,
    edit_mode: Res<TerrainEditMode>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    terrain_data: Query<&jackdaw_scene_types::Terrain>,
    bar_query: Query<(Entity, Option<&Children>), With<TerrainOptionsBar>>,
    mut local_state: Local<BarState>,
    brush_settings: Res<TerrainBrushSettings>,
    paint_state: Res<TerrainPaintState>,
    store: Res<super::TerrainDataStore>,
    splat: Res<super::splat::TerrainSplatMaterials>,
    region_view: Res<TerrainRegionView>,
    navmesh: Res<TerrainNavmeshState>,
) {
    let terrain_entity = selection.primary().filter(|&e| terrains.contains(e));
    let selected_terrain = terrain_entity.and_then(|e| terrain_data.get(e).ok());

    let channel_signature = (*edit_mode == TerrainEditMode::Paint)
        .then_some(selected_terrain)
        .flatten()
        .map(|terrain| ChannelSignature {
            channels: terrain
                .channels
                .iter()
                .map(|channel| (channel.name.clone(), channel.palette.len()))
                .collect(),
            active_channel: paint_state.active_channel,
            active_entry: paint_state.active_entry,
        });

    let slot_name = |index: usize| {
        selected_terrain
            .and_then(|terrain| store.materials(&terrain.data_path).get(index))
            .map(|slot| slot.material.clone())
    };
    let texture_signature = (*edit_mode == TerrainEditMode::Paint).then(|| TextureBarSignature {
        domain: paint_state.domain,
        active_texture_id: paint_state.active_texture_id,
        slot_material: slot_name(paint_state.active_texture_id as usize),
        base_material: slot_name(super::BASE_TEXTURE_SLOT),
        thumbnails_ready: selected_terrain
            .map(|terrain| {
                splat
                    .albedo_thumbnails(&terrain.data_path)
                    .iter()
                    .filter(|thumbnail| thumbnail.is_some())
                    .count()
            })
            .unwrap_or(0),
        restore_auto: paint_state.restore_auto,
    });

    let region_signature = (*edit_mode == TerrainEditMode::Regions).then(|| RegionBarSignature {
        show_grid: region_view.show_grid,
        visibility: region_view.visibility,
        active: terrain_entity.and_then(|entity| region_view.active_of(entity)),
    });

    let navmesh_signature = (*edit_mode == TerrainEditMode::Navmesh).then(|| NavmeshBarSignature {
        show_overlay: navmesh.show_overlay,
        summary: super::navmesh_bake::summary(&navmesh),
        agent: selected_terrain
            .map(|terrain| terrain.navmesh.clone())
            .unwrap_or_default(),
    });

    let state = BarState {
        terrain_entity,
        mode: edit_mode.clone(),
        channel_signature,
        texture_signature,
        region_signature,
        navmesh_signature,
    };
    if *local_state == state {
        return;
    }
    if bar_query.is_empty() {
        return;
    }
    *local_state = state;

    for (bar, children) in &bar_query {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }

        let show = terrain_entity.is_some() && *edit_mode != TerrainEditMode::None;
        // Full re-insert rather than mutating just `display`: this is the same
        // Node the spawn-time bsn! carries, so an update cannot drift from the
        // initial layout.
        commands.entity(bar).insert(Node {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            padding: bar_padding(),
            column_gap: px(tokens::SPACING_MD),
            row_gap: px(tokens::SPACING_SM),
            width: percent(100),
            max_height: px(160.0),
            overflow: Overflow::scroll_y(),
            flex_shrink: 0.0,
            display: if show { Display::Flex } else { Display::None },
            ..default()
        });

        let Some(terrain_entity_id) = terrain_entity else {
            continue;
        };

        match &*edit_mode {
            TerrainEditMode::None => {}
            TerrainEditMode::Sculpt(_) => {
                spawn_brush_fields(&mut commands, bar, &brush_settings);
            }
            TerrainEditMode::Paint => {
                spawn_paint_target_picker(&mut commands, bar, paint_state.domain);
                let terrain = terrain_data.get(terrain_entity_id).ok().cloned();
                match paint_state.domain {
                    PaintDomain::Channels => {
                        spawn_brush_fields(&mut commands, bar, &brush_settings);
                        spawn_channel_bar(&mut commands, bar, terrain.as_ref(), &paint_state);
                    }
                    PaintDomain::Textures => {
                        spawn_texture_paint_bar(
                            &mut commands,
                            bar,
                            &brush_settings,
                            &paint_state,
                            &store,
                            &splat,
                            terrain.as_ref(),
                        );
                    }
                }
            }
            TerrainEditMode::Quantize => {
                let quantization = terrain_data
                    .get(terrain_entity_id)
                    .map(|t| t.quantization.clone())
                    .unwrap_or_default();
                spawn_quantize_fields(&mut commands, bar, &quantization);
            }
            TerrainEditMode::Regions => {
                spawn_region_fields(
                    &mut commands,
                    bar,
                    &region_view,
                    region_view.active_of(terrain_entity_id),
                );
            }
            TerrainEditMode::Navmesh => {
                let agent = terrain_data
                    .get(terrain_entity_id)
                    .map(|t| t.navmesh.clone())
                    .unwrap_or_default();
                spawn_navmesh_fields(&mut commands, bar, &navmesh, &agent);
            }
        }
    }
}

fn spawn_brush_fields(commands: &mut Commands, parent: Entity, brush: &TerrainBrushSettings) {
    spawn_scrub_chip(
        commands,
        parent,
        "Radius",
        "Area of effect for the brush",
        brush.radius,
        0.1..50.0,
        FieldKind::Continuous,
        BrushField::Radius,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Strength",
        "How quickly the brush modifies terrain",
        brush.strength,
        0.1..50.0,
        FieldKind::Continuous,
        BrushField::Strength,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Falloff",
        "Brush edge softness (1=linear, 2=smooth)",
        brush.falloff,
        0.1..8.0,
        FieldKind::Continuous,
        BrushField::Falloff,
    );
}

/// The Regions bar: whether the boundaries are drawn, how the regions that are
/// not active draw, and which region is active.
///
/// Both controls are view state and neither leaves a history entry.
fn spawn_region_fields(
    commands: &mut Commands,
    parent: Entity,
    view: &TerrainRegionView,
    active: Option<jackdaw_terrain::RegionCoord>,
) {
    spawn_checkbox(
        commands,
        parent,
        "Region Grid",
        view.show_grid,
        RegionGridCheckbox,
    );

    let selected = RegionVisibility::ALL
        .iter()
        .position(|mode| *mode == view.visibility)
        .unwrap_or(0);
    commands
        .spawn((
            combobox::combobox_with_selected(
                RegionVisibility::ALL
                    .iter()
                    .map(|mode| mode.label().to_string())
                    .collect::<Vec<_>>(),
                selected,
            ),
            ChildOf(parent),
        ))
        .observe(|event: On<ComboBoxChangeEvent>, mut commands: Commands| {
            let mode = RegionVisibility::ALL
                .get(event.selected)
                .copied()
                .unwrap_or_default();
            commands
                .operator(super::regions::TerrainRegionVisibilityOp::ID)
                .param("mode", mode.param())
                .settings(CallOperatorSettings {
                    creates_history_entry: false,
                    execution_context: ExecutionContext::Invoke,
                })
                .call();
        });

    let group = spawn_bar_group(commands, parent);
    commands.entity(group).insert((
        Tooltip::title("Active region").with_description(
            "Click a region in the viewport to work in it. Painting and sculpting are \
             not restricted to it -- it only decides what the other regions show.",
        ),
        Hovered::default(),
    ));
    spawn_hint(
        commands,
        group,
        &match active {
            Some(coord) => format!("Active {coord}"),
            None => "Click a region to make it active".to_string(),
        },
    );
}

/// The Paint tool's target picker: scatter masks vs textures.
///
/// The operator's parameter values are `channels` and `textures`, the scripted
/// contract; renaming them would break every keybind and recorded call.
fn spawn_paint_target_picker(commands: &mut Commands, parent: Entity, domain: PaintDomain) {
    let selected = match domain {
        PaintDomain::Channels => 0,
        PaintDomain::Textures => 1,
    };
    commands
        .spawn((
            combobox::combobox_with_selected(
                vec!["Scatter Masks".to_string(), "Textures".to_string()],
                selected,
            ),
            ChildOf(parent),
        ))
        .observe(|event: On<ComboBoxChangeEvent>, mut commands: Commands| {
            let target = if event.selected == 1 {
                "textures"
            } else {
                "channels"
            };
            commands
                .operator(TerrainPaintTargetOp::ID)
                .param("target", target)
                .settings(CallOperatorSettings {
                    creates_history_entry: true,
                    execution_context: ExecutionContext::Invoke,
                })
                .call();
        });
}

/// The Texture-mode bar: brush radius and falloff, opacity, which texture is
/// loaded, and the modifier key for the overlay stroke.
///
/// Texture *selection* lives in the Terrain panel's Textures tab grid
/// (`panel.rs::spawn_textures_section`); this shows which one is active.
fn spawn_texture_paint_bar(
    commands: &mut Commands,
    parent: Entity,
    brush: &TerrainBrushSettings,
    paint: &TerrainPaintState,
    store: &super::TerrainDataStore,
    splat: &super::splat::TerrainSplatMaterials,
    terrain: Option<&jackdaw_scene_types::Terrain>,
) {
    spawn_scrub_chip(
        commands,
        parent,
        "Radius",
        "Area of effect for the brush",
        brush.radius,
        0.1..50.0,
        FieldKind::Continuous,
        BrushField::Radius,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Falloff",
        "Brush edge softness (1=linear, 2=smooth)",
        brush.falloff,
        0.1..8.0,
        FieldKind::Continuous,
        BrushField::Falloff,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Opacity",
        "Blend range crossed per second at full brush strength",
        paint.texture_opacity,
        0.01..1.0,
        FieldKind::Continuous,
        TextureField::Opacity,
    );

    // Beside the brush's own fields rather than in the Terrain panel, since it
    // is tool state. A restoring stroke hands cells back to autoterrain and
    // lays no texture down, so the readouts below do not apply while it is on.
    spawn_checkbox(
        commands,
        parent,
        "Restore Auto",
        paint.restore_auto,
        RestoreAutoCheckbox,
    );

    let data_path = terrain.map(|t| t.data_path.as_str()).unwrap_or_default();
    let slots = store.materials(data_path);
    let thumbnails = splat.albedo_thumbnails(data_path);
    let name_of = |index: usize| slots.get(index).map(|slot| slot.material.clone());
    let thumbnail_of = |index: usize| thumbnails.get(index).cloned().flatten();

    let active = paint.active_texture_id as usize;
    let active_name = name_of(active);
    let base_name = name_of(super::BASE_TEXTURE_SLOT);

    // A swatch rather than the id in text, since the brush lays down an image.
    // The id stays in the tooltip, for reading against the control map.
    let readout = spawn_bar_group(commands, parent);
    commands.entity(readout).insert((
        Tooltip::title(active_name.clone().unwrap_or_else(|| "No texture".into()))
            .with_description(match active_name {
                Some(_) => {
                    format!("Painting lays this down as the cell's base texture (id {active}).")
                }
                None => "Pick one in the Terrain panel's Textures tab.".to_string(),
            }),
        Hovered::default(),
    ));
    spawn_bar_swatch(commands, readout, thumbnail_of(active));
    spawn_hint(
        commands,
        readout,
        &name_of(active).unwrap_or_else(|| format!("id {active} -- pick one in the Terrain panel")),
    );

    // Shown only while Ctrl is down, by `sync_overlay_blend_swatches`, which is
    // the modifier that selects the blend stroke.
    let blend = spawn_bar_group(commands, parent);
    commands.entity(blend).insert((
        OverlayBlendSwatches,
        Tooltip::title("Ctrl blends toward the active texture").with_description(format!(
            "Holding Ctrl raises the blend of the cells under the brush from {} toward \
             {}, rather than replacing their base texture.",
            base_name.unwrap_or_else(|| "the base coat".into()),
            name_of(active).unwrap_or_else(|| "the active texture".into()),
        )),
        Hovered::default(),
    ));
    spawn_bar_swatch(commands, blend, thumbnail_of(super::BASE_TEXTURE_SLOT));
    spawn_hint(commands, blend, "->");
    spawn_bar_swatch(commands, blend, thumbnail_of(active));
    commands.entity(blend).insert(bar_group_node(false));
}

/// Marks the "base -> active" swatch pair, so [`sync_overlay_blend_swatches`]
/// can show it while Ctrl is held without rebuilding the bar around a modifier
/// key.
#[derive(Component)]
struct OverlayBlendSwatches;

/// One inline group on the bar: swatches and their text on a single line, sized
/// to content so a group sits beside the scrub chips rather than taking a line
/// of its own.
fn bar_group_node(shown: bool) -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: px(tokens::SPACING_XS),
        flex_shrink: 0.0,
        display: if shown { Display::Flex } else { Display::None },
        ..default()
    }
}

fn spawn_bar_group(commands: &mut Commands, parent: Entity) -> Entity {
    commands.spawn((bar_group_node(true), ChildOf(parent))).id()
}

/// The overlay-blend pair follows the Ctrl key directly rather than
/// through [`BarState`]: a modifier is pressed and released constantly
/// mid-stroke, and rebuilding the bar each time would tear down the scrub chips
/// beside it.
fn sync_overlay_blend_swatches(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut groups: Query<&mut Node, With<OverlayBlendSwatches>>,
) {
    if groups.is_empty() {
        return;
    }
    let held = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let wanted = if held { Display::Flex } else { Display::None };
    for mut node in &mut groups {
        if node.display != wanted {
            node.display = wanted;
        }
    }
}

fn spawn_quantize_fields(
    commands: &mut Commands,
    parent: Entity,
    quantization: &jackdaw_scene_types::TerrainQuantization,
) {
    spawn_hint(
        commands,
        parent,
        "Snaps terrain heights, and optionally cells, to fixed steps for stepped, stylized terrain.",
    );
    spawn_checkbox(
        commands,
        parent,
        "Quantization",
        quantization.enabled,
        QuantizationToggleCheckbox,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Cell Size",
        "World units per cell edge. 0 leaves the terrain's size alone",
        quantization.cell_size,
        0.0..20.0,
        FieldKind::Continuous,
        QuantField::CellSize,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Height Step",
        "World units per elevation step. 0 leaves heights unsnapped",
        quantization.height_step,
        0.0..5.0,
        FieldKind::Continuous,
        QuantField::HeightStep,
    );
    commands.spawn((
        button::button(
            ButtonProps::new("Apply")
                .with_variant(ButtonVariant::Primary)
                .call_operator(crate::terrain::quantize_ops::TerrainQuantizeApplyOp::ID),
        ),
        ChildOf(parent),
    ));
}

/// The Navmesh bar: what the agent is, the Bake action, whether the result
/// draws, and what the last bake produced.
///
/// The agent chips are scene data on the terrain, so they commit like the
/// quantization chips do: on release, through the document, undoable. The
/// overlay toggle is view state and leaves no history entry. There is no voxel
/// size field; the voxel follows the terrain's own cell, and a second size to
/// keep in step with it could produce a navmesh finer than the heights behind
/// it.
fn spawn_navmesh_fields(
    commands: &mut Commands,
    parent: Entity,
    navmesh: &TerrainNavmeshState,
    agent: &jackdaw_scene_types::TerrainNavmesh,
) {
    // Radius reads as continuous and is not: the baker erodes in whole voxels,
    // using `ceil(radius / voxel)`. At the voxel a kilometre of terrain bakes
    // at, the whole chip range lands on about six distinct erosions, so
    // neighbouring values on the chip give the same navmesh.
    spawn_scrub_chip(
        commands,
        parent,
        "Agent Radius",
        "How wide the character is. Walkable ground is pulled back this far from every wall",
        agent.agent_radius,
        0.05..3.0,
        FieldKind::Continuous,
        NavmeshField::AgentRadius,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Agent Height",
        "How tall the character is. Ground with less headroom than this is not walkable",
        agent.agent_height,
        0.5..6.0,
        FieldKind::Continuous,
        NavmeshField::AgentHeight,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Max Slope",
        "Steepest ground the character can stand on, in degrees",
        agent.max_slope_degrees,
        0.0..85.0,
        FieldKind::Continuous,
        NavmeshField::MaxSlope,
    );
    commands.spawn((
        button::button(
            ButtonProps::new("Bake Navmesh")
                .with_variant(ButtonVariant::Primary)
                .call_operator(super::navmesh_bake::TerrainNavmeshBakeOp::ID),
        ),
        ChildOf(parent),
    ));
    spawn_checkbox(
        commands,
        parent,
        "Show Navmesh",
        navmesh.show_overlay,
        NavmeshOverlayCheckbox,
    );
    spawn_hint(commands, parent, &super::navmesh_bake::summary(navmesh));
}

/// The Paint bar's channel and value tile grids.
fn spawn_channel_bar(
    commands: &mut Commands,
    parent: Entity,
    terrain: Option<&jackdaw_scene_types::Terrain>,
    paint: &TerrainPaintState,
) {
    let empty: &[jackdaw_scene_types::TerrainChannel] = &[];
    let channels = terrain.map(|t| t.channels.as_slice()).unwrap_or(empty);

    let column = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    spawn_hint(
        commands,
        column,
        "Scatter masks are invisible gameplay data: they decide where scatter places \
         objects. They do not texture the terrain -- that is the Textures target.",
    );
    spawn_hint(commands, column, "Mask");
    let grid = spawn_tile_grid(commands, column);
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

    spawn_hint(commands, column, "Value");
    let values = spawn_tile_grid(commands, column);
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

    spawn_checkbox(
        commands,
        column,
        "Show Painted Mask",
        paint.show_channel,
        ShowPaintedValuesCheckbox,
    );
}

// --- Commits ---

/// Brush fields commit on every event, since they only write a resource.
/// Quantization and navmesh-agent fields commit only on `is_final`: a commit
/// there syncs the scene document and can flag a full chunk rebuild, which per
/// drag frame would remesh while dragging and leave a history entry each frame.
fn on_scrub_value_change(
    event: On<ValueChange<f32>>,
    brush_bindings: Query<&BrushField>,
    quant_bindings: Query<&QuantField>,
    texture_bindings: Query<&TextureField>,
    navmesh_bindings: Query<&NavmeshField>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
    mut paint_state: ResMut<TerrainPaintState>,
    selection: Res<Selection>,
    mut commands: Commands,
) {
    let source = event.event_target();
    if let Ok(&field) = brush_bindings.get(source) {
        match field {
            BrushField::Radius => brush_settings.radius = event.value,
            BrushField::Strength => brush_settings.strength = event.value,
            BrushField::Falloff => brush_settings.falloff = event.value,
        }
        return;
    }
    if let Ok(&field) = texture_bindings.get(source) {
        match field {
            TextureField::Opacity => paint_state.texture_opacity = event.value.clamp(0.01, 1.0),
        }
        return;
    }
    if let Ok(&field) = navmesh_bindings.get(source) {
        if !event.is_final {
            return;
        }
        let Some(entity) = selection.primary() else {
            return;
        };
        let value = event.value;
        commands.queue(move |world: &mut World| {
            crate::terrain::navmesh_bake::commit_navmesh(world, entity, |agent| match field {
                NavmeshField::AgentRadius => agent.agent_radius = value.clamp(0.05, 3.0),
                NavmeshField::AgentHeight => agent.agent_height = value.clamp(0.5, 6.0),
                NavmeshField::MaxSlope => agent.max_slope_degrees = value.clamp(0.0, 85.0),
            });
        });
        return;
    }
    if let Ok(&field) = quant_bindings.get(source) {
        if !event.is_final {
            return;
        }
        let Some(entity) = selection.primary() else {
            return;
        };
        let value = event.value;
        commands.queue(move |world: &mut World| {
            crate::terrain::quantize_ops::commit_quantization(world, entity, |q| match field {
                QuantField::CellSize => q.cell_size = value,
                QuantField::HeightStep => q.height_step = value,
            });
        });
    }
}

/// Commit handler for the options bar's native checkboxes.
///
/// `FeathersCheckbox` does not self-manage `Checked` (see
/// `ui_fields::spawn_checkbox`), so this reflects the new value onto the source
/// entity before dispatching.
fn on_terrain_checkbox_value_change(
    event: On<ValueChange<bool>>,
    quantization: Query<(), With<QuantizationToggleCheckbox>>,
    show_painted_values: Query<(), With<ShowPaintedValuesCheckbox>>,
    region_grid: Query<(), With<RegionGridCheckbox>>,
    restore_auto: Query<(), With<RestoreAutoCheckbox>>,
    navmesh_overlay: Query<(), With<NavmeshOverlayCheckbox>>,
    mut commands: Commands,
) {
    let target = event.event_target();
    let op_id = if quantization.contains(target) {
        crate::terrain::quantize_ops::TerrainQuantizeToggleOp::ID
    } else if show_painted_values.contains(target) {
        crate::terrain::channel_ops::TerrainChannelToggleViewOp::ID
    } else if region_grid.contains(target) {
        crate::terrain::regions::TerrainRegionToggleGridOp::ID
    } else if restore_auto.contains(target) {
        crate::terrain::texture_ops::TerrainPaintRestoreOp::ID
    } else if navmesh_overlay.contains(target) {
        crate::terrain::navmesh_bake::TerrainNavmeshToggleOverlayOp::ID
    } else {
        return;
    };

    if event.value {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }

    commands
        .operator(op_id)
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
}

/// Pushes a `TerrainBrushSettings` change into the already-spawned brush
/// fields, without rebuilding the bar.
///
/// Fires both for an external change (Shift+Scroll brush resize in the
/// viewport) and for the fields' own `on_scrub_value_change` writing back every
/// tick of a drag, which is what makes the chip's fill and digits track the
/// gesture rather than snapping on release. Skips a field the user is typing
/// into; see `is_focused_for_editing`.
fn sync_brush_fields(
    brush_settings: Res<TerrainBrushSettings>,
    fields: Query<(Entity, &BrushField)>,
    mut commands: Commands,
) {
    if !brush_settings.is_changed() {
        return;
    }
    let values: Vec<(Entity, f32)> = fields
        .iter()
        .map(|(entity, field)| {
            let value = match field {
                BrushField::Radius => brush_settings.radius,
                BrushField::Strength => brush_settings.strength,
                BrushField::Falloff => brush_settings.falloff,
            };
            (entity, value)
        })
        .collect();
    commands.queue(move |world: &mut World| {
        let focus = world.get_resource::<InputFocus>().and_then(InputFocus::get);
        for (entity, value) in values {
            if is_focused_for_editing(world, entity, focus) {
                continue;
            }
            world.entity_mut(entity).insert(
                jackdaw_feathers::number_input::ScrubNumberInputValue::F32(value),
            );
        }
    });
}

/// Same live-resync contract as [`sync_brush_fields`], for the Opacity
/// chip, which writes [`TerrainPaintState`] rather than
/// [`TerrainBrushSettings`].
fn sync_texture_opacity_field(
    paint_state: Res<TerrainPaintState>,
    fields: Query<(Entity, &TextureField)>,
    mut commands: Commands,
) {
    if !paint_state.is_changed() {
        return;
    }
    let values: Vec<(Entity, f32)> = fields
        .iter()
        .map(|(entity, field)| {
            let value = match field {
                TextureField::Opacity => paint_state.texture_opacity,
            };
            (entity, value)
        })
        .collect();
    if values.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        let focus = world.get_resource::<InputFocus>().and_then(InputFocus::get);
        for (entity, value) in values {
            if is_focused_for_editing(world, entity, focus) {
                continue;
            }
            world.entity_mut(entity).insert(
                jackdaw_feathers::number_input::ScrubNumberInputValue::F32(value),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_state_changes_on_mode_switch() {
        let a = BarState {
            terrain_entity: None,
            mode: TerrainEditMode::None,
            channel_signature: None,
            texture_signature: None,
            region_signature: None,
            navmesh_signature: None,
        };
        let b = BarState {
            terrain_entity: None,
            mode: TerrainEditMode::Quantize,
            channel_signature: None,
            texture_signature: None,
            region_signature: None,
            navmesh_signature: None,
        };
        assert_ne!(a, b);
    }

    fn texture_bar(domain: PaintDomain) -> BarState {
        BarState {
            terrain_entity: None,
            mode: TerrainEditMode::Paint,
            channel_signature: None,
            texture_signature: Some(TextureBarSignature {
                domain,
                active_texture_id: 0,
                slot_material: None,
                base_material: None,
                thumbnails_ready: 0,
                restore_auto: false,
            }),
            region_signature: None,
            navmesh_signature: None,
        }
    }

    fn region_bar(visibility: RegionVisibility) -> BarState {
        BarState {
            terrain_entity: None,
            mode: TerrainEditMode::Regions,
            channel_signature: None,
            texture_signature: None,
            region_signature: Some(RegionBarSignature {
                show_grid: true,
                visibility,
                active: None,
            }),
            navmesh_signature: None,
        }
    }

    /// The Regions bar names the active region and shows both view choices, so
    /// a change to any of them rebuilds it even though `mode` stays `Regions`.
    #[test]
    fn bar_state_changes_on_region_view_switch() {
        assert_ne!(
            region_bar(RegionVisibility::Full),
            region_bar(RegionVisibility::Hidden)
        );
        let mut moved = region_bar(RegionVisibility::Full);
        moved
            .region_signature
            .as_mut()
            .expect("a regions bar")
            .active = Some(jackdaw_terrain::RegionCoord::new(1, 2));
        assert_ne!(region_bar(RegionVisibility::Full), moved);
    }

    /// Switching the paint target (Channels vs Textures) invalidates the bar
    /// even though `mode` stays `Paint`: the domain lives inside
    /// `texture_signature`, not `mode`.
    #[test]
    fn bar_state_changes_on_paint_target_switch() {
        assert_ne!(
            texture_bar(PaintDomain::Channels),
            texture_bar(PaintDomain::Textures)
        );
    }

    /// The bar draws the active texture's albedo, so it rebuilds when that
    /// image finishes decoding; otherwise the swatch stays the blank
    /// placeholder it spawned as.
    #[test]
    fn bar_state_changes_when_a_thumbnail_finishes_loading() {
        let loading = texture_bar(PaintDomain::Textures);
        let mut loaded = loading.clone();
        loaded.texture_signature.as_mut().unwrap().thumbnails_ready = 1;
        assert_ne!(loading, loaded);
    }

    /// The overlay-blend pair names slot 0, which can change without the active
    /// id moving.
    #[test]
    fn bar_state_changes_when_the_base_slots_material_changes() {
        let before = texture_bar(PaintDomain::Textures);
        let mut after = before.clone();
        after.texture_signature.as_mut().unwrap().base_material = Some("grass".into());
        assert_ne!(before, after);
    }
}
