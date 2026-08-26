//! The dockable "Terrain" panel: Textures, Scatter and Generation sections.
//!
//! Separate from the Components inspector so PCG and authoring parameters live
//! with the feature they configure rather than the object they act on.

use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{SliderValue, ValueChange};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonOperatorCall, ButtonProps, ButtonVariant},
    combobox::{self, ComboBoxChangeEvent},
    icons::{EditorFontItalic, Icon, IconFont},
    number_input::ScrubNumberInputValue,
    panel_card::PanelCardCollapseState,
    tab_strip::{self, TabStripItem, TabStripOrientation},
    tokens,
};

use super::autoterrain_ops::{
    TerrainAutoterrainBaseOp, TerrainAutoterrainEnableOp, TerrainAutoterrainRangeOp,
    TerrainAutoterrainSlopeOp,
};
use super::ops::{TerrainErodeOp, TerrainGenerateOp};
use super::shape_ops::{MAX_CELL_SIZE, MIN_CELL_SIZE, clamp_cell_size, commit_shape};
use super::splat::TerrainSplatMaterials;
use super::texture_ops::{
    TerrainMaterialAddOp, TerrainMaterialDetileOp, TerrainMaterialMoveOp, TerrainMaterialPicker,
    TerrainMaterialPickerOp, TerrainMaterialRemoveOp, TerrainMaterialUvScaleOp,
    TerrainTextureSelectOp,
};
use super::ui_fields::{
    FieldKind, TerrainDefaultFontRoot, spawn_checkbox, spawn_error_hint, spawn_hint,
    spawn_scrub_chip, spawn_slider_row, spawn_texture_tile, spawn_tile_grid,
};
use super::{TerrainDataStore, TerrainPaintState};
use crate::material_assets::{MaterialRegistry, MaterialTile, material_thumbnail};
use crate::material_preview::MaterialPreviewState;
use crate::material_ui::{
    ActionHeaderProps, HeaderAction, MaterialSection, fill_surface_rows, fill_texture_rows,
    spawn_action_header, spawn_preview, spawn_section,
};
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainGenerateState>()
        .init_resource::<TerrainPanelTab>()
        .add_systems(
            Update,
            (
                update_terrain_panel_content,
                sync_shape_fields,
                sync_gen_fields,
                sync_material_uv_fields,
                sync_material_detile_fields,
                sync_autoterrain_fields,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        )
        .add_observer(on_shape_scrub_change)
        .add_observer(on_gen_value_change)
        .add_observer(on_material_uv_change)
        .add_observer(on_material_detile_change)
        .add_observer(on_autoterrain_slider_change)
        .add_observer(on_autoterrain_checkbox_change);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainPanelTabOp>();
}

/// Switches the Terrain panel's active section.
///
/// The tab strip (`jackdaw_feathers::tab_strip::spawn_tab_strip`) dispatches
/// this operator on click, so a scripted run (`JACKDAW_RUN_OP`) and a click
/// take the same path.
#[operator(
    id = "terrain.panel.tab",
    label = "Terrain Panel Tab",
    description = "Switch the Terrain panel between its Textures, Scatter and Generation \
                   sections.",
    params(tab(
        String,
        default = "scatter",
        doc = "Which section to show: \"textures\", \"scatter\" or \"generation\"."
    ),),
    allows_undo = false
)]
pub(crate) fn terrain_panel_tab(
    params: In<OperatorParameters>,
    mut tab: ResMut<TerrainPanelTab>,
) -> OperatorResult {
    let tab_str = params.as_str("tab").unwrap_or("scatter");
    *tab = match tab_str {
        "textures" => TerrainPanelTab::Textures,
        "generation" => TerrainPanelTab::Generation,
        "scatter" => TerrainPanelTab::Scatter,
        other => {
            warn!("terrain.panel.tab: unrecognized tab \"{other}\", falling back to \"scatter\"");
            TerrainPanelTab::Scatter
        }
    };
    OperatorResult::Finished
}

/// Persistent generation settings, preserved across panel rebuilds.
#[derive(Resource, Default)]
pub struct TerrainGenerateState {
    pub settings: jackdaw_terrain::GenerateSettings,
    pub erosion: jackdaw_terrain::ErosionParams,
}

/// Which section the panel currently shows.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
enum TerrainPanelTab {
    Textures,
    #[default]
    Scatter,
    Generation,
}

/// Marker for the panel's tab-strip container.
#[derive(Component)]
struct TerrainPanelTabStrip;

/// Marker for the panel's tab-content container.
#[derive(Component)]
struct TerrainPanelBody;

/// Builds the Terrain panel: a tab strip over a scrollable body. Registered as
/// a `jackdaw.inspector.terrain` dock window. Both containers start empty;
/// `update_terrain_panel_content` populates them.
pub fn terrain_panel_content() -> impl Bundle {
    (
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        TerrainDefaultFontRoot,
        children![
            (
                TerrainPanelTabStrip,
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: px(tokens::SPACING_XS),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    flex_shrink: 0.0,
                    ..default()
                },
            ),
            (
                TerrainPanelBody,
                Node {
                    width: percent(100),
                    flex_grow: 1.0,
                    min_height: px(0),
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::scroll_y(),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    row_gap: px(tokens::SPACING_SM),
                    ..default()
                },
                ScrollPosition::default(),
            ),
        ],
    )
}

// --- Field binding tags ---

/// The one field a terrain's shape has: how far apart its cells sit.
///
/// A tag rather than a bare marker so the scrub handler and the sync system
/// agree on which chip they address.
#[derive(Component, Clone, Copy)]
enum ShapeField {
    CellSize,
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

#[derive(Default, PartialEq, Clone)]
struct PanelState {
    terrain_entity: Option<Entity>,
    tab: TerrainPanelTab,
    scatter_signature: Option<super::scatter::ScatterSignature>,
    textures_signature: Option<TexturesSignature>,
    /// The selected terrain's vertex grid. In the signature because it changes
    /// only through the Shape section's combobox, a discrete pick. The extent
    /// beside it is absent, arriving from a scrub drag that a rebuild would
    /// despawn under the pointer; `sync_shape_fields` keeps that one current.
    resolution: Option<u32>,
}

/// What the Textures tab rebuilds on: the material list, the quarantine and
/// typed-error states, which slots have decoded thumbnails, which one is
/// selected, and what the picker is offering.
///
/// Not the per-slot tiling values: those arrive from a slider drag, and a
/// rebuild mid-drag would despawn the slider under the pointer.
/// `sync_material_uv_fields` keeps them current instead.
#[derive(PartialEq, Clone)]
struct TexturesSignature {
    materials: Vec<String>,
    quarantine_reason: Option<String>,
    list_error: Option<String>,
    splat_error: Option<String>,
    missing: Vec<String>,
    thumbnails_ready: usize,
    active_texture_id: u8,
    picker_open: bool,
    offered: Vec<String>,
    /// Autoterrain's on/off and its two slots, the parts the section draws
    /// structurally. Its two angles are absent for the same reason the
    /// per-slot tiling values are: they arrive from a slider drag.
    autoterrain: (bool, u8, u8),
}

/// The Textures tab's resources, bundled into one `SystemParam` so
/// `update_terrain_panel_content` stays under bevy's system-argument limit.
#[derive(bevy::ecs::system::SystemParam)]
struct TexturesTabRefs<'w> {
    store: Res<'w, TerrainDataStore>,
    splat: Res<'w, TerrainSplatMaterials>,
    paint: Res<'w, TerrainPaintState>,
    picker: Res<'w, TerrainMaterialPicker>,
    registry: Res<'w, MaterialRegistry>,
    materials: Res<'w, Assets<StandardMaterial>>,
    italic_font: Res<'w, EditorFontItalic>,
    icon_font: Res<'w, IconFont>,
    preview: Res<'w, MaterialPreviewState>,
    collapse: Res<'w, PanelCardCollapseState>,
}

fn textures_signature(data_path: &str, refs: &TexturesTabRefs) -> TexturesSignature {
    TexturesSignature {
        materials: refs
            .store
            .materials(data_path)
            .iter()
            .map(|slot| slot.material.clone())
            .collect(),
        quarantine_reason: refs.store.load_failed_reason(data_path).map(str::to_string),
        list_error: refs.picker.error.clone(),
        splat_error: refs.splat.error(data_path).map(str::to_string),
        missing: refs.splat.missing(data_path).to_vec(),
        thumbnails_ready: refs
            .splat
            .albedo_thumbnails(data_path)
            .iter()
            .filter(|thumb| thumb.is_some())
            .count(),
        active_texture_id: refs.paint.active_texture_id,
        picker_open: refs.picker.open,
        autoterrain: {
            let settings = refs.store.autoterrain(data_path);
            (settings.enabled, settings.base_slot, settings.slope_slot)
        },
        offered: if refs.picker.open {
            refs.registry
                .entries
                .iter()
                .filter(|entry| entry.handle != Handle::default())
                .map(|entry| format!("{}:{}", entry.saved, entry.name))
                .collect()
        } else {
            Vec::new()
        },
    }
}

fn update_terrain_panel_content(
    mut commands: Commands,
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    terrain_data: Query<&jackdaw_scene_types::Terrain>,
    strip_query: Query<(Entity, Option<&Children>), With<TerrainPanelTabStrip>>,
    body_query: Query<(Entity, Option<&Children>), With<TerrainPanelBody>>,
    mut local_state: Local<PanelState>,
    tab: Res<TerrainPanelTab>,
    gen_state: Res<TerrainGenerateState>,
    scatter_state: Res<super::scatter::TerrainScatterState>,
    scatter_report: Res<super::scatter::TerrainScatterReport>,
    textures: TexturesTabRefs,
    store: Res<TerrainDataStore>,
) {
    let terrain_entity = selection.primary().filter(|&e| terrains.contains(e));

    let scatter_signature = (*tab == TerrainPanelTab::Scatter && terrain_entity.is_some())
        .then(|| super::scatter::signature(&scatter_state, &scatter_report));

    let textures_signature = (*tab == TerrainPanelTab::Textures)
        .then(|| terrain_entity.and_then(|e| terrain_data.get(e).ok()))
        .flatten()
        .map(|terrain| textures_signature(&terrain.data_path, &textures));

    let state = PanelState {
        terrain_entity,
        tab: *tab,
        scatter_signature,
        textures_signature,
        resolution: terrain_entity
            .and_then(|e| terrain_data.get(e).ok())
            .map(|terrain| store.grid_shape(terrain).resolution),
    };
    if *local_state == state || body_query.is_empty() {
        return;
    }
    *local_state = state;

    for (strip, children) in &strip_query {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        tab_strip::spawn_tab_strip(
            &mut commands,
            strip,
            TerrainPanelTabOp::ID,
            "tab",
            TabStripOrientation::Horizontal,
            [
                TabStripItem::new("Textures", *tab == TerrainPanelTab::Textures, "textures"),
                TabStripItem::new("Scatter", *tab == TerrainPanelTab::Scatter, "scatter"),
                TabStripItem::new(
                    "Generation",
                    *tab == TerrainPanelTab::Generation,
                    "generation",
                ),
            ],
        );
    }

    for (body, children) in &body_query {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }

        let Some(terrain_entity_id) = terrain_entity else {
            commands.spawn((
                Text::new(
                    "Select a terrain to edit its textures, scatter and generation settings.",
                ),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                ChildOf(body),
            ));
            continue;
        };

        if let Ok(terrain) = terrain_data.get(terrain_entity_id) {
            spawn_shape_section(&mut commands, body, terrain);
        }

        match *tab {
            TerrainPanelTab::Textures => {
                let terrain = terrain_data.get(terrain_entity_id).ok().cloned();
                spawn_textures_section(&mut commands, body, terrain.as_ref(), &textures);
            }
            TerrainPanelTab::Scatter => {
                let terrain = terrain_data.get(terrain_entity_id).ok().cloned();
                super::scatter::spawn_scatter_ui(
                    &mut commands,
                    body,
                    terrain.as_ref(),
                    &scatter_state,
                    &scatter_report,
                );
            }
            TerrainPanelTab::Generation => {
                spawn_generation_section(&mut commands, body, &gen_state);
            }
        }
    }
}

/// The ground's own extent and vertex grid, above the tab content so it is
/// reachable from whichever section the user is working in.
///
/// Nothing here touches stored region data; `shape_ops` covers what a shrink
/// does and does not do.
fn spawn_shape_section(
    commands: &mut Commands,
    parent: Entity,
    terrain: &jackdaw_scene_types::Terrain,
) {
    let section = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(tokens::SPACING_SM),
                row_gap: px(tokens::SPACING_XS),
                width: percent(100),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    // Cell size is the whole of a terrain's shape. How much ground it covers is
    // the regions it has been sculpted into, which nothing here sets.
    spawn_scrub_chip(
        commands,
        section,
        "Cell Size",
        "Metres per cell edge. Rescales the terrain laterally; no height moves and \
         no stored ground is resampled.",
        terrain.cell_size,
        MIN_CELL_SIZE..MAX_CELL_SIZE,
        FieldKind::Continuous,
        ShapeField::CellSize,
    );
}

/// The Textures tab: the terrain's material list as a thumbnail grid, the
/// picker that adds another, and, for the selected slot, the same material
/// editor every other surface shows.
///
/// Clicking a thumbnail dispatches `terrain.texture.select`, which the Paint
/// tool's Textures mode reads for its active texture id, so the grid's order is
/// the control map's id order.
fn spawn_textures_section(
    commands: &mut Commands,
    parent: Entity,
    terrain: Option<&jackdaw_scene_types::Terrain>,
    refs: &TexturesTabRefs,
) {
    let Some(terrain) = terrain else {
        return;
    };
    let data_path = &terrain.data_path;
    let icon_font = refs.icon_font.0.clone();

    // A load-failed sidecar is read-only: nothing below the notice is editable
    // while the reason it failed is unaddressed.
    if let Some(reason) = refs.store.load_failed_reason(data_path) {
        spawn_error_hint(commands, parent, &format!("read-only: {reason}"));
        return;
    }

    let slots = refs.store.materials(data_path);
    let thumbnails = refs.splat.albedo_thumbnails(data_path);
    let missing = refs.splat.missing(data_path);

    let materials = spawn_section(
        commands,
        parent,
        MATERIALS_SECTION,
        &icon_font,
        &refs.collapse,
    );
    if slots.is_empty() {
        spawn_hint(
            commands,
            materials.body,
            "None yet. Add a saved material to paint this terrain with.",
        );
    } else {
        let grid = spawn_tile_grid(commands, materials.body);
        // Tiles are labelled with the material name alone; a tile is too narrow
        // to carry a "missing" marker as well, and the error rows below name
        // every missing material. A vacated id shows as an empty tile that
        // cannot be picked: it holds a place in the id order rather than
        // offering the brush anything to paint with.
        for (index, slot) in slots.iter().enumerate() {
            let vacant = slot.is_tombstone();
            spawn_texture_tile(
                commands,
                grid,
                thumbnails.get(index).cloned().flatten(),
                if vacant { "(empty)" } else { &slot.material },
                !vacant && index == refs.paint.active_texture_id as usize,
                // Slot 0 is what an unpainted cell draws
                // (`Control::default().base_id()`), the coat the terrain
                // starts under rather than one of the textures a brush lays
                // over it.
                (index == super::BASE_TEXTURE_SLOT).then_some("Base"),
                (!vacant).then_some(TerrainTextureSelectOp::ID),
                Some(index),
            );
        }
    }

    // Editing and painting stay open while a material is missing: the control
    // map is intact and the slot keeps its id, so the fix is to restore or
    // replace the material rather than to repaint.
    for name in missing {
        spawn_error_hint(
            commands,
            materials.body,
            &format!(
                "'{name}' has no material file; that slot draws the fallback until \
                 it is restored"
            ),
        );
    }
    if let Some(error) = &refs.picker.error {
        spawn_error_hint(commands, materials.body, error);
    }
    if let Some(error) = refs.splat.error(data_path) {
        spawn_error_hint(commands, materials.body, error);
    }

    commands.spawn((
        button::button(
            ButtonProps::new(if refs.picker.open {
                "Close"
            } else {
                "Add Material"
            })
            .call_operator(TerrainMaterialPickerOp::ID),
        ),
        ChildOf(materials.body),
    ));
    if refs.picker.open {
        spawn_material_picker(commands, materials.body, refs);
    }

    spawn_autoterrain_section(commands, parent, data_path, refs);

    let selected = refs.paint.active_texture_id as usize;
    if let Some(slot) = slots.get(selected).filter(|slot| !slot.is_tombstone()) {
        spawn_slot_editor(commands, parent, selected, slot, slots.len(), refs);
    }
}

/// The Autoterrain section: whether the cells no hand has claimed take their
/// texture from the geometry, which two of this terrain's textures they take it
/// from, and across what band of slope.
///
/// The two pickers are the same thumbnail tiles the material list is made of. A
/// vacated id cannot be picked here either, since it draws the fallback.
fn spawn_autoterrain_section(
    commands: &mut Commands,
    parent: Entity,
    data_path: &str,
    refs: &TexturesTabRefs,
) {
    let icon_font = refs.icon_font.0.clone();
    let settings = refs.store.autoterrain(data_path);
    let slots = refs.store.materials(data_path);
    let thumbnails = refs.splat.albedo_thumbnails(data_path);

    let section = spawn_section(
        commands,
        parent,
        AUTOTERRAIN_SECTION,
        &icon_font,
        &refs.collapse,
    );
    spawn_checkbox(
        commands,
        section.body,
        "Enabled",
        settings.enabled,
        AutoterrainEnableCheckbox,
    );
    spawn_hint(
        commands,
        section.body,
        "Cells you have not painted take their texture from the slope under them. \
         Painting a cell claims it; the paint bar's Restore Auto brush hands it back.",
    );

    if slots.is_empty() {
        spawn_hint(
            commands,
            section.body,
            "Add a material above to choose what flat and steep ground draw.",
        );
    }
    for (label, chosen, op_id) in [
        (
            "Flat ground",
            settings.base_slot,
            TerrainAutoterrainBaseOp::ID,
        ),
        (
            "Steep ground",
            settings.slope_slot,
            TerrainAutoterrainSlopeOp::ID,
        ),
    ] {
        spawn_hint(commands, section.body, label);
        let grid = spawn_tile_grid(commands, section.body);
        for (index, slot) in slots.iter().enumerate() {
            let vacant = slot.is_tombstone();
            spawn_texture_tile(
                commands,
                grid,
                thumbnails.get(index).cloned().flatten(),
                if vacant { "(empty)" } else { &slot.material },
                !vacant && index == chosen as usize,
                None,
                (!vacant).then_some(op_id),
                Some(index),
            );
        }
    }

    spawn_slider_row(
        commands,
        section.body,
        "Slope start",
        "Slope at which the flat-ground texture starts giving way, in degrees",
        settings.slope_start_deg,
        SLOPE_RANGE,
        FieldKind::Continuous,
        AutoterrainField::SlopeStart,
    );
    spawn_slider_row(
        commands,
        section.body,
        "Slope end",
        "Slope at which the steep-ground texture has fully taken over, in degrees",
        settings.slope_end_deg,
        SLOPE_RANGE,
        FieldKind::Continuous,
        AutoterrainField::SlopeEnd,
    );
}

/// Every material the registry knows, in the same tile grammar the
/// Materials panel browses them in.
///
/// Unsaved materials are offered too, marked as such. A terrain stores names,
/// so adding one is refused; the refusal beside a tile marked unsaved is what
/// says which materials need saving before this terrain can draw with them.
fn spawn_material_picker(commands: &mut Commands, parent: Entity, refs: &TexturesTabRefs) {
    let offerable: Vec<_> = refs
        .registry
        .entries
        .iter()
        .filter(|entry| entry.handle != Handle::default())
        .collect();
    if offerable.is_empty() {
        spawn_hint(
            commands,
            parent,
            "No materials yet. Create one in the Materials panel and save it.",
        );
        return;
    }

    spawn_hint(commands, parent, "Materials to add");
    let grid = spawn_tile_grid(commands, parent);
    for entry in offerable {
        let name = entry.name.clone();
        let tile = crate::material_assets::spawn_material_tile(
            commands,
            grid,
            MaterialTile {
                name: name.clone(),
                thumbnail: material_thumbnail(&refs.materials, &entry.handle),
                saved: entry.saved,
                selected: false,
                italic_font: refs.italic_font.0.clone(),
            },
        );
        commands
            .entity(tile)
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                commands
                    .operator(TerrainMaterialAddOp::ID)
                    .param("material", name.clone())
                    .settings(CallOperatorSettings {
                        creates_history_entry: true,
                        execution_context: ExecutionContext::Invoke,
                    })
                    .call();
            });
    }
}

/// The selected slot: what it is drawn with, where it sits in the id order, how
/// often it tiles, and the material itself.
///
/// The material sections are the shared ones, so a texture bound or a scalar
/// changed here is the same edit the Materials window would make, and the splat
/// array resolves through the same live asset.
fn spawn_slot_editor(
    commands: &mut Commands,
    parent: Entity,
    index: usize,
    slot: &jackdaw_terrain::sidecar::TerrainMaterialSlot,
    count: usize,
    refs: &TexturesTabRefs,
) {
    let icon_font = refs.icon_font.0.clone();
    let entry = refs.registry.get_by_name(&slot.material);
    let saved = entry.is_some_and(|entry| entry.saved);

    spawn_action_header(
        commands,
        parent,
        ActionHeaderProps {
            name: format!("Slot {index}: {}", slot.material),
            saved,
            italic_font: &refs.italic_font.0,
            icon_font: &icon_font,
            actions: slot_actions(index, count),
        },
    );

    let Some(handle) = entry.map(|entry| entry.handle.clone()) else {
        spawn_error_hint(
            commands,
            parent,
            "This slot's material is not loaded; its id and paint are kept.",
        );
        return;
    };

    // The preview follows the slot the brush is loaded with.
    let image = refs.preview.preview_image.clone();
    let previewed = handle.clone();
    commands.queue(move |world: &mut World| {
        world.resource_mut::<MaterialPreviewState>().active_material = Some(previewed);
    });

    let preview = spawn_section(
        commands,
        parent,
        PREVIEW_SECTION,
        &icon_font,
        &refs.collapse,
    );
    spawn_preview(commands, preview.body, image);

    let slot_section = spawn_section(commands, parent, SLOT_SECTION, &icon_font, &refs.collapse);
    spawn_slider_row(
        commands,
        slot_section.body,
        "Tiling",
        "Texture repeats per world unit on this terrain. Lives on the slot, so one \
         material can tile differently on every surface that uses it",
        slot.uv_scale,
        UV_SCALE_RANGE,
        FieldKind::Continuous,
        MaterialUvField(index),
    );
    spawn_slider_row(
        commands,
        slot_section.body,
        "Detiling",
        "How hard to break up this material's repetition here: every tile of it is \
         turned and shifted a little. 0 leaves the tiling exactly as it is",
        slot.detile,
        DETILE_RANGE,
        FieldKind::Continuous,
        MaterialDetileField(index),
    );

    let Some(material) = refs.materials.get(&handle) else {
        return;
    };
    let surface = spawn_section(
        commands,
        parent,
        SURFACE_SECTION,
        &icon_font,
        &refs.collapse,
    );
    fill_surface_rows(commands, surface.body, material, &handle);

    let textures = spawn_section(
        commands,
        parent,
        TEXTURES_SECTION,
        &icon_font,
        &refs.collapse,
    );
    fill_texture_rows(commands, textures.body, material, &handle, &icon_font);
}

/// The list, the preview and the slot's own values open by default; the
/// material's internals stay collapsed behind them.
const MATERIALS_SECTION: MaterialSection = MaterialSection::new(
    "Materials",
    Icon::Layers,
    "terrain.textures.materials",
    false,
);
const PREVIEW_SECTION: MaterialSection =
    MaterialSection::new("Preview", Icon::Eye, "terrain.textures.preview", false);
const SLOT_SECTION: MaterialSection =
    MaterialSection::new("Slot", Icon::Grid3x3, "terrain.textures.slot", false);
const SURFACE_SECTION: MaterialSection =
    MaterialSection::new("Surface", Icon::Palette, "terrain.textures.surface", true);
const TEXTURES_SECTION: MaterialSection =
    MaterialSection::new("Textures", Icon::Image, "terrain.textures.textures", true);
/// Open by default, unlike the material internals below it: it holds the
/// terrain's own texturing rule, and hiding it would leave a terrain looking
/// auto-textured for no visible reason.
const AUTOTERRAIN_SECTION: MaterialSection = MaterialSection::new(
    "Autoterrain",
    Icon::Mountain,
    "terrain.textures.autoterrain",
    false,
);

/// The whole slope range: 0 is level ground and 90 is a wall.
const SLOPE_RANGE: std::ops::Range<f32> = 0.0..90.0;

/// Which end of the autoterrain slope band a slider row drives.
#[derive(Component, Clone, Copy)]
enum AutoterrainField {
    SlopeStart,
    SlopeEnd,
}

/// Tags the Autoterrain section's on/off checkbox so its commit handler can
/// tell it from every other checkbox in the editor.
#[derive(Component)]
struct AutoterrainEnableCheckbox;

fn on_autoterrain_slider_change(
    event: On<ValueChange<f32>>,
    fields: Query<&AutoterrainField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(event.event_target()) else {
        return;
    };
    let param = match field {
        AutoterrainField::SlopeStart => "start",
        AutoterrainField::SlopeEnd => "end",
    };
    // One end per call: the operator leaves the unnamed end alone.
    commands
        .operator(TerrainAutoterrainRangeOp::ID)
        .param(param, event.value as f64)
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
}

/// `FeathersCheckbox` does not self-manage `Checked` (see
/// `ui_fields::spawn_checkbox`), so this reflects the new value onto the source
/// entity before dispatching.
fn on_autoterrain_checkbox_change(
    event: On<ValueChange<bool>>,
    boxes: Query<(), With<AutoterrainEnableCheckbox>>,
    mut commands: Commands,
) {
    let target = event.event_target();
    if !boxes.contains(target) {
        return;
    }
    if event.value {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    commands
        .operator(TerrainAutoterrainEnableOp::ID)
        .param("enabled", event.value)
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
}

/// [`sync_material_uv_fields`] for the two slope rows: the section does not
/// rebuild on an angle change, so an undo, a scripted call and every
/// intermediate tick of a drag reach the sliders through here.
fn sync_autoterrain_fields(
    store: Res<TerrainDataStore>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    fields: Query<(Entity, &AutoterrainField)>,
    mut commands: Commands,
) {
    if !store.is_changed() || fields.is_empty() {
        return;
    }
    let Some(terrain) = selection.primary().and_then(|e| terrains.get(e).ok()) else {
        return;
    };
    let settings = store.autoterrain(&terrain.data_path);
    for (entity, field) in &fields {
        let value = match field {
            AutoterrainField::SlopeStart => settings.slope_start_deg,
            AutoterrainField::SlopeEnd => settings.slope_end_deg,
        };
        commands.entity(entity).insert(SliderValue(value));
    }
}

/// Where the slot sits in the id order, and whether it stays.
///
/// Removal empties the id rather than closing the order up, which the Remove
/// tooltip states: the grid shows a gap appearing.
fn slot_actions(index: usize, count: usize) -> Vec<HeaderAction> {
    vec![
        HeaderAction::new(
            Icon::ArrowUp,
            "Move Up",
            "Move this material earlier in the texture id order.",
            ButtonOperatorCall::new(TerrainMaterialMoveOp::ID)
                .with_param("index", index as i64)
                .with_param("to", index.saturating_sub(1) as i64),
        ),
        HeaderAction::new(
            Icon::ArrowDown,
            "Move Down",
            "Move this material later in the texture id order.",
            ButtonOperatorCall::new(TerrainMaterialMoveOp::ID)
                .with_param("index", index as i64)
                .with_param("to", (index + 1).min(count.saturating_sub(1)) as i64),
        ),
        HeaderAction::new(
            Icon::Trash2,
            "Remove",
            "Empties this texture id without moving the others. Cells painted with it \
             draw the fallback until a material is added back, which reuses this id \
             and restores them.",
            ButtonOperatorCall::new(TerrainMaterialRemoveOp::ID).with_param("index", index as i64),
        ),
    ]
}

/// Practical tiling range for the slider. Narrower than the array builder's
/// hard bounds, which the operator clamps to: a linear slider over the full
/// range would spend its whole travel below one repeat.
const UV_SCALE_RANGE: std::ops::Range<f32> = 0.01..2.0;

/// The whole detiling range: 0 is off and 1 is a full turn per tile.
const DETILE_RANGE: std::ops::Range<f32> = 0.0..1.0;

/// Which slot's tiling a slider row drives.
#[derive(Component, Clone, Copy)]
struct MaterialUvField(usize);

/// Which slot's detiling a slider row drives.
#[derive(Component, Clone, Copy)]
struct MaterialDetileField(usize);

fn on_material_uv_change(
    event: On<ValueChange<f32>>,
    fields: Query<&MaterialUvField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(event.event_target()) else {
        return;
    };
    commands
        .operator(TerrainMaterialUvScaleOp::ID)
        .param("index", field.0 as i64)
        .param("value", event.value as f64)
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
}

fn on_material_detile_change(
    event: On<ValueChange<f32>>,
    fields: Query<&MaterialDetileField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(event.event_target()) else {
        return;
    };
    commands
        .operator(TerrainMaterialDetileOp::ID)
        .param("index", field.0 as i64)
        .param("value", event.value as f64)
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
}

/// Pushes the stored tiling back onto its slider whenever the store changes: an
/// undo, a scripted call, or an intermediate tick of a drag, so the fill tracks
/// the gesture rather than snapping into place on release.
fn sync_material_uv_fields(
    store: Res<TerrainDataStore>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    fields: Query<(Entity, &MaterialUvField)>,
    mut commands: Commands,
) {
    if !store.is_changed() || fields.is_empty() {
        return;
    }
    let Some(terrain) = selection.primary().and_then(|e| terrains.get(e).ok()) else {
        return;
    };
    let slots = store.materials(&terrain.data_path);
    for (entity, field) in &fields {
        if let Some(slot) = slots.get(field.0) {
            commands.entity(entity).insert(SliderValue(slot.uv_scale));
        }
    }
}

/// [`sync_material_uv_fields`] for the detiling row beside it.
fn sync_material_detile_fields(
    store: Res<TerrainDataStore>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    fields: Query<(Entity, &MaterialDetileField)>,
    mut commands: Commands,
) {
    if !store.is_changed() || fields.is_empty() {
        return;
    }
    let Some(terrain) = selection.primary().and_then(|e| terrains.get(e).ok()) else {
        return;
    };
    let slots = store.materials(&terrain.data_path);
    for (entity, field) in &fields {
        if let Some(slot) = slots.get(field.0) {
            commands.entity(entity).insert(SliderValue(slot.detile));
        }
    }
}

fn spawn_generation_section(
    commands: &mut Commands,
    parent: Entity,
    gen_state: &TerrainGenerateState,
) {
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
                width: percent(100),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new("Noise Type"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: px(80.0),
            flex_shrink: 0.0,
            ..default()
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

    spawn_slider_row(
        commands,
        parent,
        "Seed",
        "Same seed always produces the same terrain",
        gen_state.settings.seed as f32,
        0.0..100_000.0,
        FieldKind::Count,
        GenField::Seed,
    );
    spawn_slider_row(
        commands,
        parent,
        "Frequency",
        "Lower = broader features, higher = finer detail",
        gen_state.settings.frequency as f32,
        0.001..2.0,
        FieldKind::Continuous,
        GenField::Frequency,
    );
    spawn_slider_row(
        commands,
        parent,
        "Octaves",
        "Layers of noise stacked together. More = finer detail",
        gen_state.settings.octaves as f32,
        1.0..8.0,
        FieldKind::Count,
        GenField::Octaves,
    );
    spawn_slider_row(
        commands,
        parent,
        "Lacunarity",
        "How much each octave's frequency increases",
        gen_state.settings.lacunarity as f32,
        1.0..4.0,
        FieldKind::Continuous,
        GenField::Lacunarity,
    );
    spawn_slider_row(
        commands,
        parent,
        "Persistence",
        "How much each octave contributes. Lower = subtler",
        gen_state.settings.persistence as f32,
        0.0..1.0,
        FieldKind::Continuous,
        GenField::Persistence,
    );
    spawn_slider_row(
        commands,
        parent,
        "Amplitude",
        "Overall height scale of the generated terrain",
        gen_state.settings.amplitude,
        0.0..200.0,
        FieldKind::Continuous,
        GenField::Amplitude,
    );
    spawn_slider_row(
        commands,
        parent,
        "Offset",
        "Vertical offset added after generation",
        gen_state.settings.offset,
        -100.0..100.0,
        FieldKind::Continuous,
        GenField::Offset,
    );

    commands.spawn((
        button::button(
            ButtonProps::new("Generate")
                .with_variant(ButtonVariant::Primary)
                .call_operator(TerrainGenerateOp::ID),
        ),
        ChildOf(parent),
    ));

    commands.spawn((
        Text::new("Hydraulic Erosion"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_BODY_COLOR.into()),
        ChildOf(parent),
    ));

    spawn_slider_row(
        commands,
        parent,
        "Iterations",
        "Number of water droplets simulated",
        gen_state.erosion.iterations as f32,
        0.0..5000.0,
        FieldKind::Count,
        ErosionField::Iterations,
    );
    spawn_slider_row(
        commands,
        parent,
        "Erosion Radius",
        "Area of effect for each erosion step",
        gen_state.erosion.erosion_radius as f32,
        1.0..10.0,
        FieldKind::Count,
        ErosionField::ErosionRadius,
    );
    spawn_slider_row(
        commands,
        parent,
        "Inertia",
        "How much a droplet keeps its previous direction",
        gen_state.erosion.inertia,
        0.0..1.0,
        FieldKind::Continuous,
        ErosionField::Inertia,
    );
    spawn_slider_row(
        commands,
        parent,
        "Capacity",
        "How much sediment water can carry",
        gen_state.erosion.capacity,
        0.0..50.0,
        FieldKind::Continuous,
        ErosionField::Capacity,
    );
    spawn_slider_row(
        commands,
        parent,
        "Deposition",
        "Rate sediment is dropped when water slows",
        gen_state.erosion.deposition,
        0.0..1.0,
        FieldKind::Continuous,
        ErosionField::Deposition,
    );
    spawn_slider_row(
        commands,
        parent,
        "Erosion Rate",
        "Rate terrain is dissolved by flowing water",
        gen_state.erosion.erosion,
        0.0..1.0,
        FieldKind::Continuous,
        ErosionField::Erosion,
    );
    spawn_slider_row(
        commands,
        parent,
        "Evaporation",
        "How quickly water droplets shrink",
        gen_state.erosion.evaporation,
        0.0..1.0,
        FieldKind::Continuous,
        ErosionField::Evaporation,
    );

    commands.spawn((
        button::button(
            ButtonProps::new("Erode")
                .with_variant(ButtonVariant::Primary)
                .call_operator(TerrainErodeOp::ID),
        ),
        ChildOf(parent),
    ));
}

fn on_gen_value_change(
    event: On<ValueChange<f32>>,
    gen_bindings: Query<&GenField>,
    erosion_bindings: Query<&ErosionField>,
    mut gen_state: ResMut<TerrainGenerateState>,
) {
    let source = event.event_target();
    let value = event.value;
    if let Ok(&field) = gen_bindings.get(source) {
        match field {
            GenField::Seed => gen_state.settings.seed = value.max(0.0) as u32,
            GenField::Frequency => gen_state.settings.frequency = value as f64,
            GenField::Octaves => gen_state.settings.octaves = value.max(1.0) as usize,
            GenField::Lacunarity => gen_state.settings.lacunarity = value as f64,
            GenField::Persistence => gen_state.settings.persistence = value as f64,
            GenField::Amplitude => gen_state.settings.amplitude = value,
            GenField::Offset => gen_state.settings.offset = value,
        }
        return;
    }
    if let Ok(&field) = erosion_bindings.get(source) {
        match field {
            // Soft-clamped at the field's range for scrubbing, hard-capped
            // again at run time in `hydraulic_erosion`.
            ErosionField::Iterations => {
                gen_state.erosion.iterations =
                    (value.max(0.0) as u32).min(jackdaw_terrain::erosion::MAX_ITERATIONS);
            }
            ErosionField::ErosionRadius => gen_state.erosion.erosion_radius = value.max(0.0) as u32,
            ErosionField::Inertia => gen_state.erosion.inertia = value,
            ErosionField::Capacity => gen_state.erosion.capacity = value,
            ErosionField::Deposition => gen_state.erosion.deposition = value,
            ErosionField::Erosion => gen_state.erosion.erosion = value,
            ErosionField::Evaporation => gen_state.erosion.evaporation = value,
        }
    }
}

/// Re-inserts `SliderValue` on every generation and erosion field whenever
/// `gen_state` changes, including every intermediate tick of a drag
/// (`on_gen_value_change` marks the resource changed on each one), so the fill
/// and digits track the gesture rather than snapping into place on release.
///
/// No focus guard: unlike `ScrubNumberInput`, `FeathersSlider` has no
/// editable-text descendant a resync could clobber mid-type.
fn sync_gen_fields(
    gen_state: Res<TerrainGenerateState>,
    gen_fields: Query<(Entity, &GenField)>,
    erosion_fields: Query<(Entity, &ErosionField)>,
    mut commands: Commands,
) {
    if !gen_state.is_changed() {
        return;
    }
    for (entity, field) in &gen_fields {
        let value = match field {
            GenField::Seed => gen_state.settings.seed as f32,
            GenField::Frequency => gen_state.settings.frequency as f32,
            GenField::Octaves => gen_state.settings.octaves as f32,
            GenField::Lacunarity => gen_state.settings.lacunarity as f32,
            GenField::Persistence => gen_state.settings.persistence as f32,
            GenField::Amplitude => gen_state.settings.amplitude,
            GenField::Offset => gen_state.settings.offset,
        };
        commands.entity(entity).insert(SliderValue(value));
    }
    for (entity, field) in &erosion_fields {
        let value = match field {
            ErosionField::Iterations => gen_state.erosion.iterations as f32,
            ErosionField::ErosionRadius => gen_state.erosion.erosion_radius as f32,
            ErosionField::Inertia => gen_state.erosion.inertia,
            ErosionField::Capacity => gen_state.erosion.capacity,
            ErosionField::Deposition => gen_state.erosion.deposition,
            ErosionField::Erosion => gen_state.erosion.erosion,
            ErosionField::Evaporation => gen_state.erosion.evaporation,
        };
        commands.entity(entity).insert(SliderValue(value));
    }
}

/// Write path for the Shape section's extent chips.
///
/// Two cadences on one gesture. The widget does not self-update (see
/// `ScrubNumberInput`), so the chip is re-inserted on every tick or the digits
/// freeze at their pre-drag value while the pointer moves. The document write
/// is gated on `is_final`: a commit syncs the scene document, flags a full
/// rebuild and leaves a history entry, none of which belongs on a drag frame.
fn on_shape_scrub_change(
    event: On<ValueChange<f32>>,
    shape_bindings: Query<&ShapeField>,
    selection: Res<Selection>,
    mut commands: Commands,
) {
    let source = event.event_target();
    let Ok(&field) = shape_bindings.get(source) else {
        return;
    };
    let value = clamp_cell_size(event.value);
    // Every tick, final or not: this is what the user reads while dragging.
    commands
        .entity(source)
        .insert(ScrubNumberInputValue::F32(value));
    if !event.is_final {
        return;
    }
    let Some(entity) = selection.primary() else {
        return;
    };
    let ShapeField::CellSize = field;
    commands.queue(move |world: &mut World| {
        commit_shape(world, entity, value);
    });
}

/// Keeps the Shape section's chip showing what the component holds, since it is
/// left out of the panel's rebuild signature (see [`PanelState`]). Undo, redo
/// and any script-driven `terrain.shape.cell_size` arrive this way; a drag's own
/// ticks go through [`on_shape_scrub_change`].
fn sync_shape_fields(
    terrains: Query<&jackdaw_scene_types::Terrain, Changed<jackdaw_scene_types::Terrain>>,
    selection: Res<Selection>,
    shape_fields: Query<(Entity, &ShapeField)>,
    mut commands: Commands,
) {
    let Some(terrain) = selection.primary().and_then(|e| terrains.get(e).ok()) else {
        return;
    };
    for (entity, field) in &shape_fields {
        let ShapeField::CellSize = field;
        let value = terrain.cell_size;
        commands
            .entity(entity)
            .insert(ScrubNumberInputValue::F32(value));
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::AssetPlugin;

    use super::*;

    /// An app wiring only the two systems under test: the `ValueChange`
    /// observer that writes `TerrainGenerateState`, and the resync system that
    /// writes `SliderValue` back onto the field.
    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<TerrainGenerateState>();
        app.add_systems(Update, sync_gen_fields);
        app.add_observer(on_gen_value_change);
        app
    }

    /// A single `is_final: false` tick, what one pointer-move mid-drag
    /// produces, updates both `TerrainGenerateState` and the field's
    /// `SliderValue` in the same pass.
    #[test]
    fn intermediate_drag_tick_updates_state_and_resyncs_the_widget_live() {
        let mut app = test_app();
        let entity = app
            .world_mut()
            .spawn((GenField::Persistence, SliderValue(0.45)))
            .id();

        app.world_mut().trigger(ValueChange::<f32> {
            source: entity,
            value: 0.2,
            is_final: false,
        });
        app.update();

        assert!(
            (app.world()
                .resource::<TerrainGenerateState>()
                .settings
                .persistence
                - 0.2)
                .abs()
                < 1e-5,
            "state must update on every drag tick, not just the final one",
        );
        let synced = app.world().get::<SliderValue>(entity).unwrap();
        assert!(
            (synced.0 - 0.2).abs() < 1e-5,
            "the field's own SliderValue must be resynced the same pass, so the fill and \
             digits move live instead of only on release",
        );
    }

    /// A second drag tick, still not final, resyncs again: the loop tracks
    /// every tick rather than catching up once.
    #[test]
    fn a_later_drag_tick_resyncs_again() {
        let mut app = test_app();
        let entity = app
            .world_mut()
            .spawn((GenField::Persistence, SliderValue(0.45)))
            .id();

        app.world_mut().trigger(ValueChange::<f32> {
            source: entity,
            value: 0.2,
            is_final: false,
        });
        app.update();
        app.world_mut().trigger(ValueChange::<f32> {
            source: entity,
            value: 0.8,
            is_final: false,
        });
        app.update();

        let synced = app.world().get::<SliderValue>(entity).unwrap();
        assert!((synced.0 - 0.8).abs() < 1e-5);
    }

    /// Erosion fields go through the same observer and resync pair as
    /// `GenField`.
    #[test]
    fn erosion_field_also_resyncs_live() {
        let mut app = test_app();
        let entity = app
            .world_mut()
            .spawn((ErosionField::Inertia, SliderValue(0.0)))
            .id();

        app.world_mut().trigger(ValueChange::<f32> {
            source: entity,
            value: 0.6,
            is_final: false,
        });
        app.update();

        assert!(
            (app.world()
                .resource::<TerrainGenerateState>()
                .erosion
                .inertia
                - 0.6)
                .abs()
                < 1e-5
        );
        let synced = app.world().get::<SliderValue>(entity).unwrap();
        assert!((synced.0 - 0.6).abs() < 1e-5);
    }

    fn params_with_tab(tab: &str) -> OperatorParameters {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "tab".to_string(),
            jackdaw_scene_types::PropertyValue::String(tab.to_string().into()),
        );
        OperatorParameters(map)
    }

    #[test]
    fn recognized_values_switch_the_active_tab() {
        let mut world = World::new();
        world.init_resource::<TerrainPanelTab>();

        let result = world
            .run_system_cached_with(terrain_panel_tab, params_with_tab("generation"))
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(
            *world.resource::<TerrainPanelTab>(),
            TerrainPanelTab::Generation
        );

        let result = world
            .run_system_cached_with(terrain_panel_tab, params_with_tab("scatter"))
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(
            *world.resource::<TerrainPanelTab>(),
            TerrainPanelTab::Scatter
        );
    }

    #[test]
    fn an_unrecognized_value_falls_back_to_scatter() {
        let mut world = World::new();
        world.init_resource::<TerrainPanelTab>();
        // Starting from Generation makes a silent no-op visible.
        let _ = world
            .run_system_cached_with(terrain_panel_tab, params_with_tab("generation"))
            .expect("system runs");

        let result = world
            .run_system_cached_with(terrain_panel_tab, params_with_tab("nonsense"))
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);

        assert_eq!(
            *world.resource::<TerrainPanelTab>(),
            TerrainPanelTab::Scatter
        );
    }

    /// The Textures tab renders for a world with no rendering in it, so its
    /// section is exercised directly rather than through the panel's rebuild
    /// pass.
    fn textures_tab_world() -> World {
        // Real asset plumbing, because the slot controls spawn a native slider
        // through `bsn!`, which resolves through the asset server.
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.init_asset::<Font>();
        app.init_asset::<bevy::scene::ScenePatch>();
        app.init_resource::<TerrainDataStore>();
        app.init_resource::<crate::terrain::splat::TerrainSplatMaterials>();
        app.init_resource::<TerrainPaintState>();
        app.init_resource::<TerrainMaterialPicker>();
        app.init_resource::<MaterialRegistry>();
        app.init_resource::<Selection>();
        app.init_resource::<MaterialPreviewState>();
        app.init_resource::<PanelCardCollapseState>();
        app.insert_resource(EditorFontItalic(Handle::default()));
        app.insert_resource(IconFont(Handle::default()));
        std::mem::take(app.world_mut())
    }

    fn rendered_text(world: &mut World, data_path: &str) -> Vec<String> {
        use bevy::ecs::system::RunSystemOnce;

        let terrain = jackdaw_scene_types::Terrain {
            data_path: data_path.to_string(),
            ..default()
        };
        let parent = world.spawn(Node::default()).id();
        world
            .run_system_once(move |mut commands: Commands, refs: TexturesTabRefs| {
                spawn_textures_section(&mut commands, parent, Some(&terrain), &refs);
            })
            .expect("system runs");
        let mut texts = world.query::<&Text>();
        texts.iter(world).map(|t| t.0.clone()).collect()
    }

    /// A terrain referencing a material with no file keeps the slot on screen,
    /// names the material in an error row, and stays editable: the paint under
    /// that id is intact.
    #[test]
    fn a_missing_material_is_named_and_keeps_its_tile() {
        let mut world = textures_tab_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "a.jdterrain",
                vec![
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass"),
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("deleted"),
                ],
            )
            .expect("accepted");
        world
            .resource_mut::<crate::terrain::splat::TerrainSplatMaterials>()
            .insert_test_entry("a.jdterrain", vec!["deleted".to_string()], None);

        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(
            text.iter().any(|t| t == "grass"),
            "the resolved slot still shows: {text:?}"
        );
        assert!(
            text.iter().any(|t| t == "deleted"),
            "the missing slot keeps its tile and its id: {text:?}"
        );
        assert!(
            text.iter()
                .any(|t| t.contains("'deleted' has no material file")),
            "the panel must name the missing material: {text:?}"
        );
    }

    /// A vacated texture id shows as a gap the operator can fill: the ids above
    /// it depend on it staying put, and a shorter grid would misrepresent the
    /// id order the paint is addressed by.
    #[test]
    fn a_vacated_id_shows_as_an_empty_tile_with_no_controls_behind_it() {
        let mut world = textures_tab_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "a.jdterrain",
                vec![
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::tombstone(),
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("sand"),
                ],
            )
            .expect("accepted");

        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(
            text.iter().any(|t| t == "(empty)"),
            "the vacated id keeps a tile: {text:?}"
        );
        assert!(
            text.iter().any(|t| t == "sand"),
            "and the material above it keeps its own: {text:?}"
        );
        // The brush starts on id 0, the vacated one, so this also pins that its
        // controls are not offered.
        assert!(
            !text.iter().any(|t| t.starts_with("Slot 0")),
            "a vacated id has no material to retile, reorder or remove: {text:?}"
        );
        assert!(!text.iter().any(|t| t == "Tiling"), "{text:?}");
    }

    /// Slot 0 is the coat an unpainted cell draws and Ctrl blends toward the
    /// active texture from it, so the grid badges which tile it is.
    #[test]
    fn the_base_slot_is_badged_and_no_other_slot_is() {
        let mut world = textures_tab_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "a.jdterrain",
                vec![
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass"),
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("sand"),
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("rock"),
                ],
            )
            .expect("accepted");

        let text = rendered_text(&mut world, "a.jdterrain");
        assert_eq!(
            text.iter().filter(|t| *t == "Base").count(),
            1,
            "exactly the one base slot carries the badge: {text:?}",
        );
        assert!(
            text.iter().any(|t| t == "grass"),
            "and the badge does not displace the slot's own name: {text:?}",
        );
    }

    /// The Autoterrain section is part of the Textures tab whether or not the
    /// terrain has it enabled, since it is where it is enabled.
    #[test]
    fn the_autoterrain_section_offers_both_slot_pickers_and_both_angles() {
        let mut world = textures_tab_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "a.jdterrain",
                vec![
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass"),
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("rock"),
                ],
            )
            .expect("accepted");

        let text = rendered_text(&mut world, "a.jdterrain");

        // The name is on the section header alone; the box under it says what
        // the box does.
        assert_eq!(
            text.iter().filter(|t| *t == "Autoterrain").count(),
            1,
            "{text:?}"
        );
        assert!(text.iter().any(|t| t == "Enabled"), "{text:?}");
        assert!(text.iter().any(|t| t == "Flat ground"), "{text:?}");
        assert!(text.iter().any(|t| t == "Steep ground"), "{text:?}");
        assert!(text.iter().any(|t| t == "Slope start"), "{text:?}");
        assert!(text.iter().any(|t| t == "Slope end"), "{text:?}");
        // Two pickers over the same two materials: each name is on a tile in
        // both grids, plus once in the material list above.
        assert_eq!(text.iter().filter(|t| *t == "grass").count(), 3, "{text:?}");
    }

    /// A terrain with nothing to draw with says so where the pickers would be,
    /// rather than showing two empty grids.
    #[test]
    fn the_autoterrain_pickers_say_what_is_missing_before_any_material_is_added() {
        let mut world = textures_tab_world();
        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(
            text.iter().any(|t| t.contains("Add a material above")),
            "{text:?}",
        );
    }

    /// A refusal from a material operator reaches the panel as a readable row
    /// rather than only the log.
    #[test]
    fn a_refused_add_renders_its_reason() {
        let mut world = textures_tab_world();
        world.resource_mut::<TerrainMaterialPicker>().error = Some(
            crate::terrain::store::TerrainMaterialError::Unsaved("detected".to_string())
                .to_string(),
        );

        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(
            text.iter()
                .any(|t| t.contains("detected") && t.contains("save")),
            "the refusal must be visible next to the list: {text:?}"
        );
    }

    /// A build failure the array builder reported renders, and the tab does not
    /// also claim the terrain has no materials.
    #[test]
    fn an_array_build_failure_renders_beside_the_list() {
        let mut world = textures_tab_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "a.jdterrain",
                vec![jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass")],
            )
            .expect("accepted");
        world
            .resource_mut::<crate::terrain::splat::TerrainSplatMaterials>()
            .insert_test_entry(
                "a.jdterrain",
                Vec::new(),
                Some("this terrain has 20 materials; the limit is 16".to_string()),
            );

        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(
            text.iter().any(|t| t.contains("the limit is 16")),
            "the error must render: {text:?}"
        );
        assert!(
            !text.iter().any(|t| t.starts_with("None yet")),
            "a terrain with a slot must not read as empty: {text:?}"
        );
    }

    /// A quarantined sidecar is read-only: the notice replaces the list
    /// rather than sitting above editable controls.
    #[test]
    fn a_quarantined_terrain_shows_only_its_reason() {
        let mut world = textures_tab_world();
        world
            .resource_mut::<TerrainDataStore>()
            .mark_load_failed("a.jdterrain", "unreadable bytes");

        let text = rendered_text(&mut world, "a.jdterrain");
        assert_eq!(text.len(), 1, "{text:?}");
        assert!(text[0].contains("read-only"), "{text:?}");
    }

    #[test]
    fn the_picker_says_so_when_there_is_nothing_to_offer() {
        let mut world = textures_tab_world();
        world.resource_mut::<TerrainMaterialPicker>().open = true;
        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(
            text.iter().any(|t| t.contains("No materials yet")),
            "{text:?}"
        );
    }

    /// An unsaved material has no durable name for a terrain to reference, so
    /// adding one is refused, but it still appears in the picker marked as
    /// unsaved.
    #[test]
    fn the_picker_shows_unsaved_materials_and_marks_them() {
        use bevy::picking::hover::Hovered;
        use jackdaw_feathers::tooltip::Tooltip;

        let mut world = textures_tab_world();
        world.resource_mut::<TerrainMaterialPicker>().open = true;
        let (saved, unsaved) = {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            (
                materials.add(StandardMaterial::default()),
                materials.add(StandardMaterial::default()),
            )
        };
        {
            let mut registry = world.resource_mut::<MaterialRegistry>();
            registry.add_saved("grass".into(), saved);
            registry.add("detected".into(), unsaved);
        }

        let text = rendered_text(&mut world, "a.jdterrain");
        assert!(text.iter().any(|t| t == "grass"), "{text:?}");
        assert!(text.iter().any(|t| t == "detected"), "{text:?}");

        let mut tooltips = world.query_filtered::<&Tooltip, With<Hovered>>();
        let marked: Vec<String> = tooltips
            .iter(&world)
            .map(|tooltip| tooltip.title.to_string())
            .collect();
        assert!(
            marked.iter().any(|t| t == "detected (unsaved)"),
            "the unsaved material must say so: {marked:?}"
        );
        assert!(
            !marked.iter().any(|t| t.contains("grass (unsaved)")),
            "a saved material must not: {marked:?}"
        );
    }

    /// A scrub chip bound to a selected terrain, with what a commit needs:
    /// history to record into and a document to write through. Returns the chip
    /// and the terrain.
    fn shape_chip_over_a_terrain() -> (App, Entity, Entity) {
        use bevy::ecs::reflect::AppTypeRegistry;

        let mut app = App::new();
        let world = app.world_mut();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut writer = registry.write();
            writer.register::<Name>();
            writer.register::<jackdaw_scene_types::Terrain>();
            writer.register::<jackdaw_scene_types::SceneNodeId>();
        }
        world.init_resource::<jackdaw_bsn::SceneBsnAst>();
        world.init_resource::<crate::commands::CommandHistory>();
        let mut store = TerrainDataStore::default();
        store.insert(
            "zone.terrain-0.jdterrain".to_string(),
            jackdaw_terrain::RegionTerrainData::default(),
        );
        world.insert_resource(store);

        let terrain = world
            .spawn((
                Name::new("Terrain"),
                jackdaw_scene_types::Terrain {
                    cell_size: 1.0,
                    data_path: "zone.terrain-0.jdterrain".to_string(),
                    ..default()
                },
            ))
            .id();
        crate::scene_io::register_entity_in_ast(world, terrain);
        world.insert_resource(Selection {
            entities: vec![terrain],
        });
        let chip = world
            .spawn((ShapeField::CellSize, ScrubNumberInputValue::F32(1.0)))
            .id();
        app.add_observer(on_shape_scrub_change);
        (app, chip, terrain)
    }

    fn cell_size_of(app: &App, terrain: Entity) -> f32 {
        app.world()
            .get::<jackdaw_scene_types::Terrain>(terrain)
            .expect("the terrain is still there")
            .cell_size
    }

    fn scene_text(app: &mut App) -> String {
        crate::scene_io::emit_bsn_scene_with_inline_assets(
            app.world_mut(),
            std::path::Path::new("."),
        )
    }

    /// The chip tracks the pointer: the widget does not self-update, so a
    /// handler that returned early on a non-final tick would leave the digits
    /// frozen at their pre-drag value for the whole gesture.
    ///
    /// Only the digits move. A mid-drag tick that committed would put an undo
    /// entry on the stack for every frame of the drag and leave the scene text
    /// carrying a value the user was passing through.
    #[test]
    fn an_intermediate_drag_tick_moves_the_chip_without_committing() {
        let (mut app, chip, terrain) = shape_chip_over_a_terrain();
        let before = scene_text(&mut app);

        app.world_mut().trigger(ValueChange::<f32> {
            source: chip,
            value: 3.0,
            is_final: false,
        });
        app.update();

        let synced = app.world().get::<ScrubNumberInputValue>(chip).unwrap();
        assert!(
            matches!(synced, ScrubNumberInputValue::F32(v) if (v - 3.0).abs() < 1e-3),
            "the chip must show the value being dragged, got {synced:?}"
        );
        assert_eq!(
            cell_size_of(&app, terrain),
            1.0,
            "a mid-drag tick must not move the terrain",
        );
        assert!(
            app.world()
                .resource::<crate::commands::CommandHistory>()
                .undo_stack
                .is_empty(),
            "a mid-drag tick must leave no undo entry",
        );
        assert_eq!(scene_text(&mut app), before, "the document must not move");
    }

    /// Release commits: the terrain and the document take the value, and one
    /// undo entry covers the gesture.
    #[test]
    fn the_final_drag_tick_commits_the_cell_size() {
        let (mut app, chip, terrain) = shape_chip_over_a_terrain();
        let before = scene_text(&mut app);

        app.world_mut().trigger(ValueChange::<f32> {
            source: chip,
            value: 3.0,
            is_final: true,
        });
        app.update();

        assert_eq!(cell_size_of(&app, terrain), 3.0);
        assert_eq!(
            app.world()
                .resource::<crate::commands::CommandHistory>()
                .undo_stack
                .len(),
            1,
            "one gesture, one undo entry",
        );
        assert_ne!(
            scene_text(&mut app),
            before,
            "the committed cell size has to reach the scene text",
        );
    }

    /// A later tick resyncs again rather than catching up once.
    #[test]
    fn a_later_shape_drag_tick_moves_the_chip_again() {
        let mut app = App::new();
        app.insert_resource(Selection::default());
        app.add_observer(on_shape_scrub_change);
        let entity = app
            .world_mut()
            .spawn((ShapeField::CellSize, ScrubNumberInputValue::F32(1.0)))
            .id();

        for value in [3.0, 2.5] {
            app.world_mut().trigger(ValueChange::<f32> {
                source: entity,
                value,
                is_final: false,
            });
            app.update();
        }

        let synced = app.world().get::<ScrubNumberInputValue>(entity).unwrap();
        assert!(
            matches!(synced, ScrubNumberInputValue::F32(v) if (v - 2.5).abs() < 1e-3),
            "every tick moves the chip, got {synced:?}"
        );
    }

    /// The clamp applies to what the chip shows, so the digits never claim a
    /// cell size the commit would refuse.
    #[test]
    fn a_drag_past_the_limit_shows_the_clamped_cell_size() {
        let mut app = App::new();
        app.insert_resource(Selection::default());
        app.add_observer(on_shape_scrub_change);
        let entity = app
            .world_mut()
            .spawn((ShapeField::CellSize, ScrubNumberInputValue::F32(1.0)))
            .id();

        app.world_mut().trigger(ValueChange::<f32> {
            source: entity,
            value: -5.0,
            is_final: false,
        });
        app.update();

        let synced = app.world().get::<ScrubNumberInputValue>(entity).unwrap();
        assert!(
            matches!(synced, ScrubNumberInputValue::F32(v) if (v - MIN_CELL_SIZE).abs() < 1e-3),
            "a drag below the floor shows the floor, got {synced:?}"
        );
    }
}
