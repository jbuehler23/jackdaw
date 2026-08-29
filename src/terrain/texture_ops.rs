//! Operators for a terrain's material list and for which texture the paint
//! brush is loaded with.
//!
//! The list lives in [`TerrainDataStore`] alongside heights and control
//! words, so it never touches the scene document and its undo command
//! writes straight into the store. Every mutation is a whole-list rewrite,
//! and an undo entry holds both lists.
//!
//! Order is the id space: slot `i` is what a control word naming texture id
//! `i` draws, so an id may not shift under the control map. Removing a
//! material leaves a tombstone in its place
//! ([`TerrainMaterialSlot::tombstone`]) and adding one fills the first
//! tombstone before it appends. Reordering is the one operation that
//! renumbers ids.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_terrain::sidecar::{AutoTerrainSettings, TerrainMaterialSlot};
use jackdaw_terrain::texture_set::{DEFAULT_UV_SCALE, MAX_DETILE, MAX_UV_SCALE, MIN_UV_SCALE};

use super::TerrainDataStore;
use super::ops::has_selected_terrain;
use super::paint::{PaintDomain, TerrainPaintState};
use super::store::TerrainMaterialError;
use crate::commands::{CommandHistory, EditorCommand};
use crate::material_assets::MaterialRegistry;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainMaterialPicker>();
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainMaterialAddOp>()
        .register_operator::<TerrainMaterialRemoveOp>()
        .register_operator::<TerrainMaterialMoveOp>()
        .register_operator::<TerrainMaterialUvScaleOp>()
        .register_operator::<TerrainMaterialDetileOp>()
        .register_operator::<TerrainMaterialPickerOp>()
        .register_operator::<TerrainPaintTargetOp>()
        .register_operator::<TerrainPaintRestoreOp>()
        .register_operator::<TerrainTextureSelectOp>();
}

/// Whether the Textures tab is showing the saved-material picker, and the
/// last refusal (a [`TerrainMaterialError`], rendered) to show beside the
/// list.
#[derive(Resource, Default)]
pub(crate) struct TerrainMaterialPicker {
    pub open: bool,
    pub error: Option<String>,
}

/// The selected terrain's sidecar path, or nothing to act on.
fn selected_data_path(
    selection: &Selection,
    terrains: &Query<&jackdaw_scene_types::Terrain>,
) -> Option<String> {
    let entity = selection.primary()?;
    let terrain = terrains.get(entity).ok()?;
    Some(terrain.data_path.clone())
}

/// Write a new list into the store and record it, or record why it was
/// refused. Every material operator ends here.
///
/// `autoterrain` is the settings move a list edit forced, old and new, or
/// `None` where it forced none. It rides in the same history entry as the
/// list so that one undo restores both; two entries would let an undo put
/// the material back while the slots stayed on the id they fled to.
fn commit(
    store: &mut TerrainDataStore,
    history: &mut CommandHistory,
    picker: &mut TerrainMaterialPicker,
    data_path: String,
    new_materials: Vec<TerrainMaterialSlot>,
    autoterrain: Option<(AutoTerrainSettings, AutoTerrainSettings)>,
    label: &str,
) -> OperatorResult {
    let old_materials = store.materials(&data_path).to_vec();
    if old_materials == new_materials {
        picker.error = None;
        return OperatorResult::Finished;
    }
    match store.set_materials(data_path.clone(), new_materials.clone()) {
        Ok(()) => {
            picker.error = None;
            if let Some((_, new)) = autoterrain {
                let _ = store.set_autoterrain(data_path.clone(), new);
            }
            history.push_executed(Box::new(SetTerrainMaterials {
                data_path,
                old_materials,
                new_materials,
                autoterrain,
                label: label.to_string(),
            }));
            OperatorResult::Finished
        }
        Err(err) => {
            picker.error = Some(err.to_string());
            OperatorResult::Cancelled
        }
    }
}

/// Give the selected terrain another texture id, drawn by a saved
/// material.
///
/// Fills the first vacated id if there is one, and appends otherwise, so a
/// material re-added after a removal lands back on the id its painted cells
/// still name.
///
/// Only a saved material may be added: a terrain stores names, and an
/// unsaved material has no name that survives the run.
#[operator(
    id = "terrain.material.add",
    label = "Add Terrain Material",
    description = "Give the selected terrain another texture id, drawn by a saved material.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(material(String, doc = "Name of a saved material.")),
)]
pub(crate) fn terrain_material_add(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    registry: Res<MaterialRegistry>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let name = params.as_str("material").unwrap_or("").trim().to_string();

    let refusal = match registry.get_by_name(&name) {
        None => Some(TerrainMaterialError::Unknown(name.clone())),
        Some(entry) if !entry.saved => Some(TerrainMaterialError::Unsaved(name.clone())),
        Some(_) => None,
    };
    if let Some(refusal) = refusal {
        picker.error = Some(refusal.to_string());
        return OperatorResult::Cancelled;
    }

    let mut materials = store.materials(&data_path).to_vec();
    match materials.iter().position(TerrainMaterialSlot::is_tombstone) {
        Some(vacant) => materials[vacant] = TerrainMaterialSlot::new(name),
        None => materials.push(TerrainMaterialSlot::new(name)),
    }
    let result = commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        materials,
        None,
        "Add Terrain Material",
    );
    if result == OperatorResult::Finished {
        picker.open = false;
    }
    result
}

/// Take the material off one of the selected terrain's texture ids.
///
/// The id stays as a tombstone; closing the gap would slide every higher id
/// down one and change what cells painted with them draw. The control map
/// is left untouched, so cells painted with this id draw the fallback until
/// something fills it again.
///
/// Removing an already-vacant id is a no-op rather than an error. The brush
/// and either autoterrain slot move off the vacated id: a slot left on it
/// would texture every unpainted cell with the fallback.
#[operator(
    id = "terrain.material.remove",
    label = "Remove Terrain Material",
    description = "Take the material off one of the selected terrain's texture ids.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(index(i64, doc = "Texture id to vacate.")),
)]
pub(crate) fn terrain_material_remove(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let index = params.as_int("index")? as usize;
    let mut materials = store.materials(&data_path).to_vec();
    if index >= materials.len() {
        picker.error = Some(TerrainMaterialError::NoSuchSlot(index).to_string());
        return OperatorResult::Cancelled;
    }
    if materials[index].is_tombstone() {
        picker.error = None;
        return OperatorResult::Finished;
    }
    materials[index] = TerrainMaterialSlot::tombstone();
    let refuge = first_material_id(&materials);

    let old_auto = store.autoterrain(&data_path);
    let mut new_auto = old_auto;
    if old_auto.base_slot as usize == index {
        new_auto.base_slot = refuge;
    }
    if old_auto.slope_slot as usize == index {
        new_auto.slope_slot = refuge;
    }
    let autoterrain = (new_auto != old_auto).then_some((old_auto, new_auto));

    let result = commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        materials,
        autoterrain,
        "Remove Terrain Material",
    );
    // Brush state is not terrain data, so it stays out of the history entry.
    if result == OperatorResult::Finished && paint.active_texture_id as usize == index {
        paint.active_texture_id = refuge;
    }
    result
}

/// The lowest texture id that still has a material behind it, or 0 when
/// none does. Where the brush and either autoterrain slot go when the id
/// they were on is vacated.
fn first_material_id(materials: &[TerrainMaterialSlot]) -> u8 {
    materials
        .iter()
        .position(|slot| !slot.is_tombstone())
        .unwrap_or(0) as u8
}

/// Move one slot to another position, renumbering the ids in between.
///
/// The one operator that changes what painted cells draw: id order is the
/// id space, so a reorder renumbers it. Every other list edit holds ids
/// still.
#[operator(
    id = "terrain.material.move",
    label = "Reorder Terrain Material",
    description = "Move one of the selected terrain's materials to another texture id.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(
        index(i64, doc = "Texture id to move."),
        to(i64, doc = "Texture id to move it to."),
    ),
)]
pub(crate) fn terrain_material_move(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let index = params.as_int("index")? as usize;
    let to = params.as_int("to")?;
    let mut materials = store.materials(&data_path).to_vec();
    if index >= materials.len() {
        picker.error = Some(TerrainMaterialError::NoSuchSlot(index).to_string());
        return OperatorResult::Cancelled;
    }
    // Clamped rather than refused, so an Up on the first slot is a no-op.
    let to = to.clamp(0, materials.len() as i64 - 1) as usize;
    let slot = materials.remove(index);
    materials.insert(to, slot);

    let result = commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        materials,
        None,
        "Reorder Terrain Material",
    );
    // The brush follows the slot it was loaded with to its new id.
    if result == OperatorResult::Finished && paint.active_texture_id as usize == index {
        paint.active_texture_id = to as u8;
    }
    result
}

/// Retile one slot: how many times its material repeats per world unit.
///
/// Held terrain-side rather than on the material, which is shared across
/// surfaces and tiles independently on each.
#[operator(
    id = "terrain.material.uv_scale",
    label = "Terrain Material Tiling",
    description = "Set how often one of the selected terrain's materials repeats.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(
        index(i64, doc = "Texture id to retile."),
        value(f64, doc = "Texture repeats per world unit."),
    ),
)]
pub(crate) fn terrain_material_uv_scale(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let index = params.as_int("index")? as usize;
    let value = params.as_float("value")? as f32;
    let mut materials = store.materials(&data_path).to_vec();
    let Some(slot) = materials.get_mut(index) else {
        picker.error = Some(TerrainMaterialError::NoSuchSlot(index).to_string());
        return OperatorResult::Cancelled;
    };
    if slot.is_tombstone() {
        picker.error = Some(TerrainMaterialError::EmptySlot(index).to_string());
        return OperatorResult::Cancelled;
    }
    // Clamped rather than refused: the value arrives from a slider drag,
    // and one the array builder would reject must not reach the store.
    // Non-finite resets to the default rather than the floor.
    slot.uv_scale = if value.is_finite() {
        value.clamp(MIN_UV_SCALE, MAX_UV_SCALE)
    } else {
        DEFAULT_UV_SCALE
    };
    commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        materials,
        None,
        "Terrain Material Tiling",
    )
}

/// Set how hard one slot's texture is broken out of its own tiling grid.
///
/// Held on the slot for the same reason as `terrain.material.uv_scale`:
/// repetition is a property of how the material was laid on this surface.
#[operator(
    id = "terrain.material.detile",
    label = "Terrain Material Detiling",
    description = "Set how hard one of the selected terrain's materials is detiled.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(
        index(i64, doc = "Texture id to detile."),
        value(f64, doc = "Detiling strength, 0 (off) to 1."),
    ),
)]
pub(crate) fn terrain_material_detile(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let index = params.as_int("index")? as usize;
    let value = params.as_float("value")? as f32;
    let mut materials = store.materials(&data_path).to_vec();
    let Some(slot) = materials.get_mut(index) else {
        picker.error = Some(TerrainMaterialError::NoSuchSlot(index).to_string());
        return OperatorResult::Cancelled;
    };
    if slot.is_tombstone() {
        picker.error = Some(TerrainMaterialError::EmptySlot(index).to_string());
        return OperatorResult::Cancelled;
    }
    // Clamped rather than refused: the value arrives from a slider drag,
    // and one the set validator would reject must not reach the store.
    slot.detile = if value.is_finite() {
        value.clamp(0.0, MAX_DETILE)
    } else {
        0.0
    };
    commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        materials,
        None,
        "Terrain Material Detiling",
    )
}

/// Show or hide the Textures tab's saved-material picker.
#[operator(
    id = "terrain.material.picker",
    label = "Pick Terrain Material",
    description = "Show or hide the saved materials the selected terrain can draw with.",
    allows_undo = false,
    params(show(
        String,
        default = "toggle",
        doc = "\"true\", \"false\", or \"toggle\"."
    ))
)]
pub(crate) fn terrain_material_picker(
    params: In<OperatorParameters>,
    mut picker: ResMut<TerrainMaterialPicker>,
    browser: Option<ResMut<crate::material_browser::MaterialBrowserState>>,
) -> OperatorResult {
    picker.open = match params.as_str("show").unwrap_or("toggle") {
        "true" => true,
        "false" => false,
        _ => !picker.open,
    };
    if !picker.open {
        picker.error = None;
        return OperatorResult::Finished;
    }
    // A material file written since the project opened is not in the
    // registry until a rescan picks it up.
    if let Some(mut browser) = browser {
        browser.needs_rescan = true;
    }
    OperatorResult::Finished
}

/// Undo command for every material-list operator. Writes straight into
/// [`TerrainDataStore`], like [`super::sculpt::SetTerrainHeights`] and
/// [`super::paint::SetTerrainChannel`], since the list lives outside the
/// AST.
pub struct SetTerrainMaterials {
    pub data_path: String,
    pub old_materials: Vec<TerrainMaterialSlot>,
    pub new_materials: Vec<TerrainMaterialSlot>,
    /// The autoterrain slots this list edit forced off a vacated id, old
    /// and new. Carried here so one undo restores the list and the slots
    /// together; see `commit`.
    pub autoterrain: Option<(AutoTerrainSettings, AutoTerrainSettings)>,
    pub label: String,
}

impl SetTerrainMaterials {
    fn apply(
        &self,
        world: &mut World,
        materials: Vec<TerrainMaterialSlot>,
        autoterrain: Option<AutoTerrainSettings>,
    ) {
        // Both directions were validated when the command was built, so a
        // failure here means the path went load-failed; the store is left
        // as it is.
        let mut store = world.resource_mut::<TerrainDataStore>();
        let _ = store.set_materials(self.data_path.clone(), materials);
        if let Some(settings) = autoterrain {
            let _ = store.set_autoterrain(self.data_path.clone(), settings);
        }
    }
}

impl EditorCommand for SetTerrainMaterials {
    fn execute(&mut self, world: &mut World) {
        self.apply(
            world,
            self.new_materials.clone(),
            self.autoterrain.map(|(_, new)| new),
        );
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(
            world,
            self.old_materials.clone(),
            self.autoterrain.map(|(old, _)| old),
        );
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Switch the Paint tool's options bar between scatter masks and textures.
#[operator(
    id = "terrain.paint.target",
    label = "Paint Target",
    description = "Switch the paint brush between scatter masks and textures.",
    allows_undo = false,
    params(target(
        String,
        default = "channels",
        doc = "Which domain the brush paints: \"channels\" (scatter masks) or \"textures\"."
    ))
)]
pub(crate) fn terrain_paint_target(
    params: In<OperatorParameters>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    let target = params.as_str("target").unwrap_or("channels");
    paint.domain = match target {
        "textures" => PaintDomain::Textures,
        "channels" => PaintDomain::Channels,
        other => {
            warn!(
                "terrain.paint.target: unrecognized target {other:?}, falling back to \"channels\""
            );
            PaintDomain::Channels
        }
    };
    OperatorResult::Finished
}

/// Switch the texture brush between painting and handing cells back to
/// autoterrain.
///
/// Brush state rather than terrain data, so it leaves no history entry of
/// its own; the stroke it produces records one.
#[operator(
    id = "terrain.paint.restore",
    label = "Restore Auto",
    description = "Brush cells back to autoterrain instead of painting a texture into them.",
    allows_undo = false,
    params(enabled(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn terrain_paint_restore(
    params: In<OperatorParameters>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    paint.restore_auto = params.as_bool("enabled").unwrap_or(!paint.restore_auto);
    OperatorResult::Finished
}

/// Choose which texture id the brush paints, from the Textures tab's
/// thumbnail grid.
#[operator(
    id = "terrain.texture.select",
    label = "Select Texture",
    description = "Choose which texture id the paint brush writes.",
    allows_undo = false,
    params(index(i64, doc = "Texture id to select."))
)]
pub(crate) fn terrain_texture_select(
    params: In<OperatorParameters>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    let index = params.as_int("index")?;
    paint.active_texture_id = index.clamp(0, jackdaw_terrain::MAX_TEXTURE_ID as i64) as u8;
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jackdaw_scene_types::PropertyValue;

    use super::*;

    /// Control words go in, the list is edited, and what each painted cell
    /// resolves to drawing comes out. Asserting on names alone would pass
    /// while every cell changed material.
    mod painted_meaning {
        use bevy::asset::AssetPlugin;
        use jackdaw_terrain::Control;

        use super::*;

        const PATH: &str = "zone.jdterrain";

        fn terrain() -> jackdaw_scene_types::Terrain {
            jackdaw_scene_types::Terrain {
                resolution: 4,
                data_path: PATH.to_string(),
                ..default()
            }
        }

        /// A world with asset plumbing, three saved materials that each
        /// bind an albedo, and one selected terrain over four cells a side
        /// of laid-down ground. Painting reaches only as far as the
        /// regions do.
        fn world() -> World {
            let mut app = App::new();
            app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
            app.init_asset::<Image>();
            app.init_asset::<StandardMaterial>();
            app.init_resource::<Selection>();
            app.init_resource::<TerrainDataStore>();
            app.init_resource::<TerrainMaterialPicker>();
            app.init_resource::<TerrainPaintState>();
            app.init_resource::<CommandHistory>();
            app.init_resource::<MaterialRegistry>();

            for name in ["grass", "rock", "sand"] {
                let handle = {
                    let server = app.world().resource::<AssetServer>().clone();
                    let albedo = server.load::<Image>(format!("t/{name}.png"));
                    app.world_mut()
                        .resource_mut::<Assets<StandardMaterial>>()
                        .add(StandardMaterial {
                            base_color_texture: Some(albedo),
                            ..default()
                        })
                };
                app.world_mut()
                    .resource_mut::<MaterialRegistry>()
                    .add_saved(name.to_string(), handle);
            }

            let mut regions = jackdaw_terrain::TerrainRegions::new(
                jackdaw_terrain::RegionSize::new(4).expect("a power of two"),
            );
            regions.ensure_grid(4).expect("inside the region cap");
            app.world_mut().resource_mut::<TerrainDataStore>().insert(
                PATH.to_string(),
                jackdaw_terrain::RegionTerrainData {
                    regions,
                    ..default()
                },
            );

            let entity = app.world_mut().spawn(terrain()).id();
            app.world_mut().resource_mut::<Selection>().entities = vec![entity];
            std::mem::take(app.world_mut())
        }

        fn add(world: &mut World, name: &str) {
            let result = world
                .run_system_cached_with(terrain_material_add, params(&[("material", text(name))]))
                .expect("system runs");
            assert_eq!(result, OperatorResult::Finished, "adding {name}");
        }

        fn remove(world: &mut World, index: i64) -> OperatorResult {
            world
                .run_system_cached_with(
                    terrain_material_remove,
                    params(&[("index", PropertyValue::Int(index))]),
                )
                .expect("system runs")
        }

        /// [`remove`] where the removal is setup rather than the subject.
        fn vacate(world: &mut World, index: i64) {
            assert_eq!(
                remove(world, index),
                OperatorResult::Finished,
                "vacating id {index}"
            );
        }

        /// Paint one cell per texture id: cell `i` gets base id `i`, with
        /// the manual bit set the way the brush leaves a cell it touched.
        fn paint_one_cell_per_id(world: &mut World, ids: &[u8]) {
            let terrain = terrain();
            let mut store = world.resource_mut::<TerrainDataStore>();
            let mut control = store.control_mut(&terrain).expect("keyed");
            for (cell, id) in ids.iter().enumerate() {
                control[cell] = Control::default().with_base_id(*id).with_manual(true);
            }
        }

        fn control_words(world: &World) -> Vec<Control> {
            world.resource::<TerrainDataStore>().control(PATH).to_vec()
        }

        /// What each texture id draws, in id order: the material's name,
        /// or `None` where the id resolves to the fallback layer. Read
        /// through the same resolution the renderer uses.
        fn drawn_by_id(world: &World) -> Vec<Option<String>> {
            let slots = world
                .resource::<TerrainDataStore>()
                .materials(PATH)
                .to_vec();
            let registry = world.resource::<MaterialRegistry>();
            let materials = world.resource::<Assets<StandardMaterial>>();
            let resolved = jackdaw_terrain::render::resolve_with(
                &slots,
                |name| {
                    registry
                        .get_by_name(name)
                        .and_then(|entry| materials.get(&entry.handle))
                },
                world.resource::<AssetServer>(),
            );
            resolved
                .set
                .entries
                .iter()
                .map(|entry| entry.albedo.as_ref().map(|_| entry.material.clone()))
                .collect()
        }

        /// What one painted cell draws, by reading its own control word.
        fn drawn_by_cell(world: &World, cell: usize) -> Option<String> {
            let id = control_words(world)[cell].base_id() as usize;
            drawn_by_id(world).get(id).cloned().flatten()
        }

        fn three_painted_slots() -> World {
            let mut world = world();
            add(&mut world, "grass");
            add(&mut world, "rock");
            add(&mut world, "sand");
            paint_one_cell_per_id(&mut world, &[0, 1, 2]);
            world
        }

        /// Taking a material off a middle id must not slide the ids above
        /// it down, which would change what cells painted with those ids
        /// draw.
        #[test]
        fn removing_a_middle_material_leaves_every_other_cell_drawing_what_it_drew() {
            let mut world = three_painted_slots();
            let before = control_words(&world);
            assert_eq!(drawn_by_cell(&world, 2).as_deref(), Some("sand"));

            assert_eq!(remove(&mut world, 1), OperatorResult::Finished);

            assert_eq!(
                control_words(&world),
                before,
                "a list edit must not rewrite a single control word",
            );
            assert_eq!(
                drawn_by_cell(&world, 0).as_deref(),
                Some("grass"),
                "the cell below the removal is untouched",
            );
            assert_eq!(
                drawn_by_cell(&world, 2).as_deref(),
                Some("sand"),
                "the cell above the removal must still draw sand, not slide onto \
                 whatever took id 2",
            );
        }

        /// The cells painted with the removed id are the only ones that
        /// change, and they change to the fallback.
        #[test]
        fn the_cells_painted_with_the_removed_id_fall_back_and_nothing_else_does() {
            let mut world = three_painted_slots();
            vacate(&mut world, 1);

            assert_eq!(drawn_by_cell(&world, 1), None, "id 1 draws the fallback");
            let drawn = drawn_by_id(&world);
            assert_eq!(
                drawn,
                vec![Some("grass".to_string()), None, Some("sand".to_string()),],
                "the id space keeps its shape",
            );
        }

        /// Adding a material back puts it on the id its cells still name.
        #[test]
        fn re_adding_a_material_lands_in_the_vacated_id_and_restores_its_cells() {
            let mut world = three_painted_slots();
            vacate(&mut world, 1);
            assert_eq!(drawn_by_cell(&world, 1), None);

            add(&mut world, "rock");

            assert_eq!(
                drawn_by_cell(&world, 1).as_deref(),
                Some("rock"),
                "the vacated id is filled before the list grows",
            );
            assert_eq!(
                drawn_by_id(&world).len(),
                3,
                "filling a vacated id must not append a fourth",
            );
            assert_eq!(drawn_by_cell(&world, 2).as_deref(), Some("sand"));
        }

        /// A material added while nothing is vacant appends, so the
        /// tombstone rule does not cap the list.
        #[test]
        fn adding_with_no_vacated_id_appends() {
            let mut world = world();
            add(&mut world, "grass");
            add(&mut world, "rock");
            assert_eq!(
                drawn_by_id(&world),
                vec![Some("grass".to_string()), Some("rock".to_string())],
            );
        }

        /// Undo restores what the terrain draws, not only the list it
        /// holds.
        #[test]
        fn undoing_a_removal_restores_the_visual_state() {
            let mut world = three_painted_slots();
            vacate(&mut world, 1);
            assert_eq!(drawn_by_cell(&world, 1), None);

            let mut command = world
                .resource_mut::<CommandHistory>()
                .undo_stack
                .pop()
                .expect("the removal pushed a history entry");
            command.undo(&mut world);

            assert_eq!(
                drawn_by_id(&world),
                vec![
                    Some("grass".to_string()),
                    Some("rock".to_string()),
                    Some("sand".to_string()),
                ],
            );
            assert_eq!(drawn_by_cell(&world, 1).as_deref(), Some("rock"));
        }

        /// Reordering is the one edit that changes what painted cells
        /// draw, since it renumbers the ids under them.
        #[test]
        fn reordering_renumbers_what_painted_cells_draw() {
            let mut world = three_painted_slots();
            let before = control_words(&world);

            let result = world
                .run_system_cached_with(
                    terrain_material_move,
                    params(&[
                        ("index", PropertyValue::Int(2)),
                        ("to", PropertyValue::Int(0)),
                    ]),
                )
                .expect("system runs");
            assert_eq!(result, OperatorResult::Finished);

            assert_eq!(
                control_words(&world),
                before,
                "even a reorder writes no control words; the ids move under them",
            );
            assert_eq!(
                drawn_by_id(&world),
                vec![
                    Some("sand".to_string()),
                    Some("grass".to_string()),
                    Some("rock".to_string()),
                ],
            );
            assert_eq!(
                drawn_by_cell(&world, 0).as_deref(),
                Some("sand"),
                "the cell painted with id 0 now draws whatever id 0 became",
            );
        }

        /// A vacated id at the end serves nothing, so the list shortens
        /// rather than carrying a hole against the id ceiling.
        #[test]
        fn a_vacated_id_at_the_end_of_the_list_is_dropped() {
            let mut world = three_painted_slots();
            vacate(&mut world, 2);
            assert_eq!(
                drawn_by_id(&world),
                vec![Some("grass".to_string()), Some("rock".to_string())],
            );

            // ... and one in the middle is not.
            vacate(&mut world, 0);
            assert_eq!(drawn_by_id(&world), vec![None, Some("rock".to_string())]);
        }

        /// Vacating an id twice asks for a state the terrain is already
        /// in, so it succeeds without recording anything.
        #[test]
        fn removing_an_already_vacated_id_is_a_no_op() {
            let mut world = three_painted_slots();
            vacate(&mut world, 1);
            world.resource_mut::<CommandHistory>().undo_stack.clear();

            assert_eq!(remove(&mut world, 1), OperatorResult::Finished);
            assert!(world.resource::<CommandHistory>().undo_stack.is_empty());
            assert_eq!(drawn_by_id(&world).len(), 3);
        }

        /// The brush cannot stay loaded with a vacated id; a pick on any
        /// other id is left alone.
        #[test]
        fn the_brush_moves_off_a_vacated_id_and_stays_put_otherwise() {
            let mut world = three_painted_slots();
            world.resource_mut::<TerrainPaintState>().active_texture_id = 2;
            vacate(&mut world, 1);
            assert_eq!(
                world.resource::<TerrainPaintState>().active_texture_id,
                2,
                "a pick on an untouched id must not move",
            );

            vacate(&mut world, 2);
            assert_eq!(
                world.resource::<TerrainPaintState>().active_texture_id,
                0,
                "a pick on the id just vacated moves to one that still draws",
            );
        }

        /// An autoterrain slot left on a vacated id would texture every
        /// unpainted cell with the fallback. The move rides in the
        /// removal's history entry, so one undo restores both.
        #[test]
        fn an_autoterrain_slot_moves_off_a_vacated_id_and_undoes_back_onto_it() {
            let mut world = three_painted_slots();
            let settings = |world: &World| world.resource::<TerrainDataStore>().autoterrain(PATH);

            let mut chosen = settings(&world);
            chosen.enabled = true;
            chosen.base_slot = 1;
            chosen.slope_slot = 2;
            world
                .resource_mut::<TerrainDataStore>()
                .set_autoterrain(PATH.to_string(), chosen)
                .expect("keyed");

            vacate(&mut world, 1);
            assert_eq!(
                settings(&world).base_slot,
                0,
                "the base slot must leave the id it can no longer draw",
            );
            assert_eq!(
                settings(&world).slope_slot,
                2,
                "the slot on an untouched id must not move",
            );

            let mut command = world
                .resource_mut::<CommandHistory>()
                .undo_stack
                .pop()
                .expect("the removal pushed a history entry");
            command.undo(&mut world);

            assert_eq!(
                settings(&world),
                chosen,
                "one undo restores the settings, not just the list",
            );
            assert_eq!(drawn_by_cell(&world, 1).as_deref(), Some("rock"));
        }

        /// A retile aimed at a vacated id is refused rather than writing a
        /// scale onto a tombstone.
        #[test]
        fn retiling_a_vacated_id_is_refused_by_name() {
            let mut world = three_painted_slots();
            vacate(&mut world, 1);

            let result = world
                .run_system_cached_with(
                    terrain_material_uv_scale,
                    params(&[
                        ("index", PropertyValue::Int(1)),
                        ("value", PropertyValue::Float(0.5)),
                    ]),
                )
                .expect("system runs");

            assert_eq!(result, OperatorResult::Cancelled);
            assert!(
                world
                    .resource::<TerrainMaterialPicker>()
                    .error
                    .as_ref()
                    .is_some_and(|e| e.contains("no material")),
                "{:?}",
                world.resource::<TerrainMaterialPicker>().error,
            );
        }
    }

    fn params(pairs: &[(&str, PropertyValue)]) -> OperatorParameters {
        let mut map = BTreeMap::new();
        for (key, value) in pairs {
            map.insert(key.to_string(), value.clone());
        }
        OperatorParameters(map)
    }

    fn text(value: &str) -> PropertyValue {
        PropertyValue::String(value.to_string().into())
    }

    /// A world with one selected terrain and a registry holding one saved
    /// and one unsaved material.
    fn world_with_terrain(data_path: &str) -> World {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<TerrainDataStore>();
        world.init_resource::<TerrainMaterialPicker>();
        world.init_resource::<TerrainPaintState>();
        world.init_resource::<CommandHistory>();

        let mut assets = Assets::<StandardMaterial>::default();
        let mut registry = MaterialRegistry::default();
        registry.add_saved("grass".into(), assets.add(StandardMaterial::default()));
        registry.add_saved("rock".into(), assets.add(StandardMaterial::default()));
        registry.add("detected".into(), assets.add(StandardMaterial::default()));
        world.insert_resource(registry);

        let entity = world
            .spawn(jackdaw_scene_types::Terrain {
                data_path: data_path.to_string(),
                ..default()
            })
            .id();
        world.resource_mut::<Selection>().entities = vec![entity];
        world
    }

    fn add(world: &mut World, name: &str) -> OperatorResult {
        world
            .run_system_cached_with(terrain_material_add, params(&[("material", text(name))]))
            .expect("system runs")
    }

    fn names(world: &World) -> Vec<String> {
        world
            .resource::<TerrainDataStore>()
            .materials("zone.jdterrain")
            .iter()
            .map(|slot| slot.material.clone())
            .collect()
    }

    #[test]
    fn adding_a_saved_material_appends_a_texture_id() {
        let mut world = world_with_terrain("zone.jdterrain");
        assert_eq!(add(&mut world, "grass"), OperatorResult::Finished);
        assert_eq!(add(&mut world, "rock"), OperatorResult::Finished);
        assert_eq!(names(&world), vec!["grass", "rock"]);
        assert_eq!(world.resource::<TerrainMaterialPicker>().error, None);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .materials("zone.jdterrain")[0]
                .uv_scale,
            jackdaw_terrain::texture_set::DEFAULT_UV_SCALE,
            "a new slot starts at the default tiling",
        );
    }

    /// A terrain references a material by name. An unsaved material has no
    /// name that survives the run, so adding one is refused with a hint
    /// rather than stored and lost on the next launch.
    #[test]
    fn adding_an_unsaved_material_is_refused_with_a_hint_to_save_it() {
        let mut world = world_with_terrain("zone.jdterrain");
        assert_eq!(add(&mut world, "detected"), OperatorResult::Cancelled);
        assert!(names(&world).is_empty());
        let error = world
            .resource::<TerrainMaterialPicker>()
            .error
            .clone()
            .expect("the refusal is recorded for the panel");
        assert!(error.contains("detected"), "{error}");
        assert!(error.contains("save"), "{error}");
    }

    #[test]
    fn adding_a_material_that_does_not_exist_is_refused_by_name() {
        let mut world = world_with_terrain("zone.jdterrain");
        assert_eq!(add(&mut world, "nonesuch"), OperatorResult::Cancelled);
        assert!(names(&world).is_empty());
        assert!(
            world
                .resource::<TerrainMaterialPicker>()
                .error
                .as_ref()
                .is_some_and(|e| e.contains("nonesuch"))
        );
    }

    #[test]
    fn adding_the_same_material_twice_is_refused() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        assert_eq!(add(&mut world, "grass"), OperatorResult::Cancelled);
        assert_eq!(names(&world), vec!["grass"]);
    }

    /// Removing vacates the id rather than closing the gap, so the
    /// material above keeps its id.
    #[test]
    fn removing_a_slot_vacates_its_id_without_moving_the_others() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let _ = add(&mut world, "rock");
        let result = world
            .run_system_cached_with(
                terrain_material_remove,
                params(&[("index", PropertyValue::Int(0))]),
            )
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(names(&world), vec!["", "rock"]);
    }

    #[test]
    fn removing_a_slot_that_is_not_there_is_refused_rather_than_silent() {
        let mut world = world_with_terrain("zone.jdterrain");
        let result = world
            .run_system_cached_with(
                terrain_material_remove,
                params(&[("index", PropertyValue::Int(3))]),
            )
            .expect("system runs");
        assert_eq!(result, OperatorResult::Cancelled);
        assert!(world.resource::<TerrainMaterialPicker>().error.is_some());
    }

    /// Order is the id space, so a reorder carries the brush's pick with
    /// it.
    #[test]
    fn moving_a_slot_renumbers_the_ids_and_the_brush_follows() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let _ = add(&mut world, "rock");
        world.resource_mut::<TerrainPaintState>().active_texture_id = 1;

        let result = world
            .run_system_cached_with(
                terrain_material_move,
                params(&[
                    ("index", PropertyValue::Int(1)),
                    ("to", PropertyValue::Int(0)),
                ]),
            )
            .expect("system runs");

        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(names(&world), vec!["rock", "grass"]);
        assert_eq!(world.resource::<TerrainPaintState>().active_texture_id, 0);
    }

    #[test]
    fn moving_past_either_end_clamps_rather_than_refusing() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let _ = add(&mut world, "rock");
        let result = world
            .run_system_cached_with(
                terrain_material_move,
                params(&[
                    ("index", PropertyValue::Int(0)),
                    ("to", PropertyValue::Int(-1)),
                ]),
            )
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(names(&world), vec!["grass", "rock"]);
        assert!(world.resource::<TerrainMaterialPicker>().error.is_none());
    }

    #[test]
    fn uv_scale_lands_on_the_slot_and_is_clamped_to_a_usable_range() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let set = |world: &mut World, value: f64| {
            world
                .run_system_cached_with(
                    terrain_material_uv_scale,
                    params(&[
                        ("index", PropertyValue::Int(0)),
                        ("value", PropertyValue::Float(value)),
                    ]),
                )
                .expect("system runs")
        };

        assert_eq!(set(&mut world, 0.4), OperatorResult::Finished);
        let scale = |world: &World| {
            world
                .resource::<TerrainDataStore>()
                .materials("zone.jdterrain")[0]
                .uv_scale
        };
        assert!((scale(&world) - 0.4).abs() < 1e-6);

        let _ = set(&mut world, 0.0);
        assert_eq!(scale(&world), MIN_UV_SCALE, "a slider must not store zero");
        let _ = set(&mut world, 1e9);
        assert_eq!(scale(&world), MAX_UV_SCALE);
        let _ = set(&mut world, f64::NAN);
        assert_eq!(scale(&world), DEFAULT_UV_SCALE, "NaN resets to the default");
    }

    #[test]
    fn detiling_lands_on_the_slot_and_is_clamped_to_its_range() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let set = |world: &mut World, value: f64| {
            world
                .run_system_cached_with(
                    terrain_material_detile,
                    params(&[
                        ("index", PropertyValue::Int(0)),
                        ("value", PropertyValue::Float(value)),
                    ]),
                )
                .expect("system runs")
        };
        let detile = |world: &World| {
            world
                .resource::<TerrainDataStore>()
                .materials("zone.jdterrain")[0]
                .detile
        };

        assert_eq!(detile(&world), 0.0, "a fresh slot is not detiled");
        assert_eq!(set(&mut world, 0.7), OperatorResult::Finished);
        assert!((detile(&world) - 0.7).abs() < 1e-6);

        let _ = set(&mut world, -1.0);
        assert_eq!(detile(&world), 0.0, "off is the floor");
        let _ = set(&mut world, 4.0);
        assert_eq!(detile(&world), MAX_DETILE);
    }

    /// Detiling a vacated id is refused the same way retiling one is:
    /// there is no material there to detile.
    #[test]
    fn detiling_a_vacated_id_is_refused_by_name() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let _ = add(&mut world, "rock");
        let _ = world
            .run_system_cached_with(
                terrain_material_remove,
                params(&[("index", PropertyValue::Int(0))]),
            )
            .expect("system runs");

        let result = world
            .run_system_cached_with(
                terrain_material_detile,
                params(&[
                    ("index", PropertyValue::Int(0)),
                    ("value", PropertyValue::Float(0.5)),
                ]),
            )
            .expect("system runs");
        assert_eq!(result, OperatorResult::Cancelled);
        assert!(
            world
                .resource::<TerrainMaterialPicker>()
                .error
                .as_deref()
                .is_some_and(|message| message.contains("texture id 0")),
            "{:?}",
            world.resource::<TerrainMaterialPicker>().error,
        );
    }

    /// Add, then undo, restores the previous list.
    #[test]
    fn a_list_change_undoes_back_to_the_previous_list() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        let _ = add(&mut world, "rock");
        assert_eq!(names(&world), vec!["grass", "rock"]);

        let mut command = world
            .resource_mut::<CommandHistory>()
            .undo_stack
            .pop()
            .expect("the add pushed a history entry");
        command.undo(&mut world);
        assert_eq!(names(&world), vec!["grass"]);
        command.execute(&mut world);
        assert_eq!(names(&world), vec!["grass", "rock"]);
    }

    #[test]
    fn a_no_op_retile_leaves_no_history_entry() {
        let mut world = world_with_terrain("zone.jdterrain");
        let _ = add(&mut world, "grass");
        world.resource_mut::<CommandHistory>().undo_stack.clear();

        let _ = world
            .run_system_cached_with(
                terrain_material_uv_scale,
                params(&[
                    ("index", PropertyValue::Int(0)),
                    (
                        "value",
                        PropertyValue::Float(jackdaw_terrain::texture_set::DEFAULT_UV_SCALE as f64),
                    ),
                ]),
            )
            .expect("system runs");
        assert!(world.resource::<CommandHistory>().undo_stack.is_empty());
    }

    #[test]
    fn a_successful_add_closes_the_picker_and_a_refusal_leaves_it_open() {
        let mut world = world_with_terrain("zone.jdterrain");
        world.resource_mut::<TerrainMaterialPicker>().open = true;
        let _ = add(&mut world, "detected");
        assert!(world.resource::<TerrainMaterialPicker>().open);
        let _ = add(&mut world, "grass");
        assert!(!world.resource::<TerrainMaterialPicker>().open);
    }

    #[test]
    fn the_picker_toggles_and_clears_its_error_when_it_closes() {
        let mut world = World::new();
        world.init_resource::<TerrainMaterialPicker>();
        world.resource_mut::<TerrainMaterialPicker>().error = Some("boom".to_string());

        let _ = world
            .run_system_cached_with(terrain_material_picker, params(&[("show", text("true"))]))
            .expect("system runs");
        assert!(world.resource::<TerrainMaterialPicker>().open);
        assert_eq!(
            world.resource::<TerrainMaterialPicker>().error.as_deref(),
            Some("boom"),
        );

        let _ = world
            .run_system_cached_with(terrain_material_picker, params(&[("show", text("toggle"))]))
            .expect("system runs");
        assert!(!world.resource::<TerrainMaterialPicker>().open);
        assert_eq!(world.resource::<TerrainMaterialPicker>().error, None);
    }

    fn paint_target_world() -> World {
        let mut world = World::new();
        world.init_resource::<TerrainPaintState>();
        world
    }

    #[test]
    fn recognized_values_switch_the_paint_domain() {
        let mut world = paint_target_world();
        let result = world
            .run_system_cached_with(
                terrain_paint_target,
                params(&[("target", text("textures"))]),
            )
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(
            world.resource::<TerrainPaintState>().domain,
            PaintDomain::Textures
        );

        let _ = world
            .run_system_cached_with(
                terrain_paint_target,
                params(&[("target", text("channels"))]),
            )
            .expect("system runs");
        assert_eq!(
            world.resource::<TerrainPaintState>().domain,
            PaintDomain::Channels
        );
    }

    #[test]
    fn an_unrecognized_paint_target_falls_back_to_channels() {
        let mut world = paint_target_world();
        let _ = world
            .run_system_cached_with(
                terrain_paint_target,
                params(&[("target", text("textures"))]),
            )
            .expect("runs");
        let _ = world
            .run_system_cached_with(
                terrain_paint_target,
                params(&[("target", text("nonsense"))]),
            )
            .expect("system runs");
        assert_eq!(
            world.resource::<TerrainPaintState>().domain,
            PaintDomain::Channels
        );
    }

    #[test]
    fn texture_select_clamps_to_the_id_ceiling() {
        let mut world = paint_target_world();
        let result = world
            .run_system_cached_with(
                terrain_texture_select,
                params(&[("index", PropertyValue::Int(250))]),
            )
            .expect("system runs");
        assert_eq!(result, OperatorResult::Finished);
        assert_eq!(
            world.resource::<TerrainPaintState>().active_texture_id,
            jackdaw_terrain::MAX_TEXTURE_ID,
        );
    }
}
