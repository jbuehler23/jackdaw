//! Operators for a terrain's autoterrain settings: whether the cells no
//! hand has claimed take their texture from the geometry, which two slots
//! they take it from, and across what band of slope.
//!
//! The settings are persisted, so each operator writes through
//! [`TerrainDataStore`] and pushes its own history entry. None of them
//! touches the scene document: the settings live in the sidecar, like the
//! control words they stand in for.
//!
//! Nothing here writes a control word. Autoterrain changes what unclaimed
//! cells draw, not what they hold, so a terrain's recorded strokes come
//! back when it is turned off.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_terrain::sidecar::{
    AutoTerrainSettings, MAX_SLOPE_DEG, MIN_SLOPE_BAND_DEG, MIN_SLOPE_DEG,
};

use super::TerrainDataStore;
use super::ops::has_selected_terrain;
use super::texture_ops::TerrainMaterialPicker;
use crate::commands::{CommandHistory, EditorCommand};
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainAutoterrainEnableOp>()
        .register_operator::<TerrainAutoterrainBaseOp>()
        .register_operator::<TerrainAutoterrainSlopeOp>()
        .register_operator::<TerrainAutoterrainRangeOp>();
}

/// The selected terrain's sidecar path, or nothing to act on.
fn selected_data_path(
    selection: &Selection,
    terrains: &Query<&jackdaw_scene_types::Terrain>,
) -> Option<String> {
    let entity = selection.primary()?;
    Some(terrains.get(entity).ok()?.data_path.clone())
}

/// A numeric parameter, however it was spelled.
///
/// `start=30` parses as an integer and `start=30.0` as a float; both name
/// the same angle, so both are read here rather than one reading as absent.
fn number(params: &OperatorParameters, key: &str) -> Option<f64> {
    params
        .as_float(key)
        .or_else(|| params.as_int(key).map(|value| value as f64))
}

/// Write new settings into the store and record them, or record why they
/// were refused.
///
/// The refusal lands in [`TerrainMaterialPicker::error`], the one error row
/// the Textures tab shows.
fn commit(
    store: &mut TerrainDataStore,
    history: &mut CommandHistory,
    picker: &mut TerrainMaterialPicker,
    data_path: String,
    new: AutoTerrainSettings,
    label: &str,
) -> OperatorResult {
    let old = store.autoterrain(&data_path);
    let new = new.sanitized();
    if old == new {
        picker.error = None;
        return OperatorResult::Finished;
    }
    match store.set_autoterrain(data_path.clone(), new) {
        Ok(()) => {
            picker.error = None;
            history.push_executed(Box::new(SetAutoTerrain {
                data_path,
                old,
                new,
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

/// Turn autoterrain on or off for the selected terrain.
///
/// Every terrain starts off, where the shader skips its slope path and
/// every cell draws the word it holds.
#[operator(
    id = "terrain.autoterrain.enable",
    label = "Autoterrain",
    description = "Texture the selected terrain's unpainted cells from their slope.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(enabled(
        bool,
        doc = "On or off. Omit to flip whichever way it currently is."
    )),
)]
pub(crate) fn terrain_autoterrain_enable(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let mut settings = store.autoterrain(&data_path);
    settings.enabled = params.as_bool("enabled").unwrap_or(!settings.enabled);
    commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        settings,
        "Autoterrain",
    )
}

/// Choose which texture id flat ground draws.
#[operator(
    id = "terrain.autoterrain.base",
    label = "Autoterrain Base Texture",
    description = "Choose which of the selected terrain's textures flat ground draws.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(index(i64, doc = "Texture id for flat ground.")),
)]
pub(crate) fn terrain_autoterrain_base(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let mut settings = store.autoterrain(&data_path);
    settings.base_slot = slot_id(params.as_int("index")?);
    commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        settings,
        "Autoterrain Base Texture",
    )
}

/// Choose which texture id steep ground draws.
#[operator(
    id = "terrain.autoterrain.slope",
    label = "Autoterrain Slope Texture",
    description = "Choose which of the selected terrain's textures steep ground draws.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(index(i64, doc = "Texture id for steep ground.")),
)]
pub(crate) fn terrain_autoterrain_slope(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let mut settings = store.autoterrain(&data_path);
    settings.slope_slot = slot_id(params.as_int("index")?);
    commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        settings,
        "Autoterrain Slope Texture",
    )
}

/// A texture id the control map can address. Clamped rather than refused:
/// the tiles that dispatch this are built from the list, so a value outside
/// it comes from a scripted call rather than a click.
fn slot_id(index: i64) -> u8 {
    index.clamp(0, jackdaw_terrain::MAX_TEXTURE_ID as i64) as u8
}

/// Move one or both ends of the slope band.
///
/// Two sliders drive this, one end each, so each parameter is optional and
/// an absent one leaves that end where it is. A scripted call may pass
/// both.
#[operator(
    id = "terrain.autoterrain.range",
    label = "Autoterrain Slope Range",
    description = "Set the slope band the selected terrain fades its base texture out across.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(
        start(f64, doc = "Degrees at which the base texture starts giving way."),
        end(f64, doc = "Degrees at which the slope texture has fully taken over."),
    ),
)]
pub(crate) fn terrain_autoterrain_range(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut store: ResMut<TerrainDataStore>,
    mut picker: ResMut<TerrainMaterialPicker>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let data_path = selected_data_path(&selection, &terrains)?;
    let mut settings = store.autoterrain(&data_path);
    let (start, end) = (number(&params, "start"), number(&params, "end"));
    if start.is_none() && end.is_none() {
        return OperatorResult::Cancelled;
    }
    // Clamped rather than refused: these arrive from a slider drag, and
    // the store sanitizes what it stores. Dragging one end past the other
    // holds it against that end rather than swapping the two under the
    // pointer, and stops a band's width short of its partner, since the
    // shader cannot shade a closed band.
    if let Some(start) = start {
        settings.slope_start_deg = (start as f32)
            .clamp(MIN_SLOPE_DEG, MAX_SLOPE_DEG)
            .min(settings.slope_end_deg - MIN_SLOPE_BAND_DEG);
    }
    if let Some(end) = end {
        settings.slope_end_deg = (end as f32)
            .clamp(MIN_SLOPE_DEG, MAX_SLOPE_DEG)
            .max(settings.slope_start_deg + MIN_SLOPE_BAND_DEG);
    }
    commit(
        &mut store,
        &mut history,
        &mut picker,
        data_path,
        settings,
        "Autoterrain Slope Range",
    )
}

/// Undo command for every autoterrain operator. Writes straight into
/// [`TerrainDataStore`], like
/// [`super::texture_ops::SetTerrainMaterials`]: these settings live in the
/// sidecar, not in the scene document.
pub struct SetAutoTerrain {
    pub data_path: String,
    pub old: AutoTerrainSettings,
    pub new: AutoTerrainSettings,
    pub label: String,
}

impl SetAutoTerrain {
    fn apply(&self, world: &mut World, settings: AutoTerrainSettings) {
        // Both directions were accepted when the command was built, so a
        // failure here means the path went load-failed; the store is left
        // as it is.
        let _ = world
            .resource_mut::<TerrainDataStore>()
            .set_autoterrain(self.data_path.clone(), settings);
    }
}

impl EditorCommand for SetAutoTerrain {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, self.new);
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, self.old);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jackdaw_scene_types::PropertyValue;

    use super::*;

    const PATH: &str = "zone.jdterrain";

    fn params(pairs: &[(&str, PropertyValue)]) -> OperatorParameters {
        let mut map = BTreeMap::new();
        for (key, value) in pairs {
            map.insert(key.to_string(), value.clone());
        }
        OperatorParameters(map)
    }

    fn world_with_terrain() -> World {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<TerrainDataStore>();
        world.init_resource::<TerrainMaterialPicker>();
        world.init_resource::<CommandHistory>();
        let entity = world
            .spawn(jackdaw_scene_types::Terrain {
                data_path: PATH.to_string(),
                ..default()
            })
            .id();
        world.resource_mut::<Selection>().entities = vec![entity];
        world
    }

    fn settings(world: &World) -> AutoTerrainSettings {
        world.resource::<TerrainDataStore>().autoterrain(PATH)
    }

    fn enable(world: &mut World, pairs: &[(&str, PropertyValue)]) -> OperatorResult {
        world
            .run_system_cached_with(terrain_autoterrain_enable, params(pairs))
            .expect("system runs")
    }

    fn range(world: &mut World, pairs: &[(&str, PropertyValue)]) -> OperatorResult {
        world
            .run_system_cached_with(terrain_autoterrain_range, params(pairs))
            .expect("system runs")
    }

    #[test]
    fn a_terrain_starts_with_autoterrain_off() {
        let world = world_with_terrain();
        assert!(!settings(&world).enabled);
    }

    #[test]
    fn enable_takes_an_explicit_value_and_flips_without_one() {
        let mut world = world_with_terrain();

        assert_eq!(
            enable(&mut world, &[("enabled", PropertyValue::Bool(true))]),
            OperatorResult::Finished
        );
        assert!(settings(&world).enabled);

        let _ = enable(&mut world, &[]);
        assert!(!settings(&world).enabled, "no value flips what is there");

        let _ = enable(&mut world, &[]);
        assert!(settings(&world).enabled);
    }

    /// Asking for the state the terrain is in leaves nothing on the undo
    /// stack.
    #[test]
    fn a_no_op_change_leaves_no_history_entry() {
        let mut world = world_with_terrain();
        let _ = enable(&mut world, &[("enabled", PropertyValue::Bool(false))]);
        assert!(world.resource::<CommandHistory>().undo_stack.is_empty());
    }

    #[test]
    fn the_two_slots_are_chosen_independently_and_clamp_to_the_id_space() {
        let mut world = world_with_terrain();
        let _ = world
            .run_system_cached_with(
                terrain_autoterrain_base,
                params(&[("index", PropertyValue::Int(3))]),
            )
            .expect("system runs");
        let _ = world
            .run_system_cached_with(
                terrain_autoterrain_slope,
                params(&[("index", PropertyValue::Int(9000))]),
            )
            .expect("system runs");

        assert_eq!(settings(&world).base_slot, 3);
        assert_eq!(settings(&world).slope_slot, jackdaw_terrain::MAX_TEXTURE_ID);
    }

    /// One slider drives one end, so the other stays where the author left
    /// it.
    #[test]
    fn moving_one_end_of_the_range_leaves_the_other_alone() {
        let mut world = world_with_terrain();
        let before = settings(&world);

        assert_eq!(
            range(&mut world, &[("start", PropertyValue::Float(10.0))]),
            OperatorResult::Finished
        );

        assert_eq!(settings(&world).slope_start_deg, 10.0);
        assert_eq!(settings(&world).slope_end_deg, before.slope_end_deg);
    }

    /// Dragging one end past the other holds it a band's width short of
    /// that end rather than swapping the two, and never on it: a closed
    /// band divides by zero on flat ground and shades every unclaimed cell
    /// NaN.
    #[test]
    fn an_end_dragged_past_its_partner_stops_a_band_short_of_it() {
        let mut world = world_with_terrain();
        let end = settings(&world).slope_end_deg;

        let _ = range(&mut world, &[("start", PropertyValue::Float(80.0))]);
        assert_eq!(settings(&world).slope_start_deg, end - MIN_SLOPE_BAND_DEG);
        assert_eq!(settings(&world).slope_end_deg, end);

        let _ = range(&mut world, &[("end", PropertyValue::Float(0.0))]);
        assert_eq!(settings(&world).slope_start_deg, end - MIN_SLOPE_BAND_DEG);
        assert_eq!(settings(&world).slope_end_deg, end);
    }

    #[test]
    fn the_range_clamps_to_the_slopes_that_exist() {
        let mut world = world_with_terrain();
        let _ = range(
            &mut world,
            &[
                ("start", PropertyValue::Float(-40.0)),
                ("end", PropertyValue::Float(400.0)),
            ],
        );
        assert_eq!(settings(&world).slope_start_deg, MIN_SLOPE_DEG);
        assert_eq!(settings(&world).slope_end_deg, MAX_SLOPE_DEG);
    }

    /// `start=30` parses as an integer and means the same angle as
    /// `start=30.0`.
    #[test]
    fn an_angle_spelled_as_a_whole_number_is_still_an_angle() {
        let mut world = world_with_terrain();
        let _ = range(&mut world, &[("start", PropertyValue::Int(12))]);
        assert_eq!(settings(&world).slope_start_deg, 12.0);
    }

    #[test]
    fn a_range_call_naming_neither_end_does_nothing() {
        let mut world = world_with_terrain();
        let before = settings(&world);
        assert_eq!(range(&mut world, &[]), OperatorResult::Cancelled);
        assert_eq!(settings(&world), before);
    }

    #[test]
    fn every_setting_undoes_back_to_what_it_was() {
        let mut world = world_with_terrain();
        let before = settings(&world);
        let _ = enable(&mut world, &[("enabled", PropertyValue::Bool(true))]);
        let _ = range(&mut world, &[("start", PropertyValue::Float(5.0))]);

        while let Some(mut command) = world.resource_mut::<CommandHistory>().undo_stack.pop() {
            command.undo(&mut world);
        }

        assert_eq!(settings(&world), before);
    }

    /// A quarantined terrain refuses this write and says why rather than
    /// storing settings that could never be saved.
    #[test]
    fn a_quarantined_terrain_refuses_the_change_by_name() {
        let mut world = world_with_terrain();
        world
            .resource_mut::<TerrainDataStore>()
            .mark_load_failed(PATH, "unreadable bytes");

        assert_eq!(
            enable(&mut world, &[("enabled", PropertyValue::Bool(true))]),
            OperatorResult::Cancelled
        );
        assert!(!settings(&world).enabled);
        assert!(world.resource::<TerrainMaterialPicker>().error.is_some());
    }
}
