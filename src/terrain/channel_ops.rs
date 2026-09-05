//! Operators for editing a terrain's channel table.
//!
//! Channel descriptors (name, width, palette) are small reflected data
//! that lives inline in the scene document, so every mutation here syncs
//! the component back to the AST. The per-cell values the descriptors
//! describe are bulk data and never come near it.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement, TerrainPaletteEntry};

use super::ops::has_selected_terrain;
use super::{TerrainDataStore, TerrainDirtyChunks, TerrainPaintState};
use crate::commands::{CommandHistory, EditorCommand};
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainChannelAddOp>()
        .register_operator::<TerrainChannelRemoveOp>()
        .register_operator::<TerrainChannelSelectOp>()
        .register_operator::<TerrainChannelValueAddOp>()
        .register_operator::<TerrainChannelValueSelectOp>()
        .register_operator::<TerrainChannelToggleViewOp>();
}

/// Colours a fresh palette entry cycles through.
///
/// Distinct hues so consecutive entries are told apart on a tile grid. The
/// seed only keeps a new swatch from being invisible; the user picks the
/// colour they want.
const SEED_COLORS: [Srgba; 6] = [
    Srgba::new(0.42, 0.62, 0.32, 1.0),
    Srgba::new(0.76, 0.62, 0.35, 1.0),
    Srgba::new(0.35, 0.52, 0.72, 1.0),
    Srgba::new(0.70, 0.40, 0.42, 1.0),
    Srgba::new(0.55, 0.42, 0.68, 1.0),
    Srgba::new(0.38, 0.66, 0.64, 1.0),
];

fn seed_color(index: usize) -> Color {
    SEED_COLORS[index % SEED_COLORS.len()].into()
}

/// Mint a channel name unique against `existing`, first free `channel-N`.
///
/// Naming by `existing.len()` alone collides once a channel has been
/// removed from the middle: add, add, remove index 0, add would mint
/// `channel-1` twice. Duplicate channel names collapse to one exported
/// file (`export.rs`'s writer is last-wins), so uniqueness is enforced at
/// creation.
fn mint_channel_name(existing: &[TerrainChannel]) -> String {
    for n in 0.. {
        let candidate = format!("channel-{n}");
        if !existing.iter().any(|channel| channel.name == candidate) {
            return candidate;
        }
    }
    unreachable!("the candidate space is unbounded")
}

#[cfg(test)]
mod mint_channel_name_tests {
    use super::*;

    fn channel(name: &str) -> TerrainChannel {
        TerrainChannel {
            name: name.to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        }
    }

    /// add, add, remove index 0, add mints a name not already in use.
    #[test]
    fn does_not_repeat_after_a_remove_from_the_middle() {
        let mut channels = vec![channel("channel-0"), channel("channel-1")];
        channels.remove(0);
        let minted = mint_channel_name(&channels);
        assert_ne!(minted, "channel-1", "must not mint a name already in use");
    }

    #[test]
    fn starts_at_zero_for_an_empty_terrain() {
        assert_eq!(mint_channel_name(&[]), "channel-0");
    }
}

/// Push the terrain's channel table back into the scene document and
/// force a mesh rebuild, so a table edit is both saved and visible.
fn commit_channels(world: &mut World, entity: Entity) {
    let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(entity) else {
        return;
    };
    let terrain = terrain.clone();
    // Reconciling here is what zeroes a newly added channel at
    // `resolution^2` and carries a renamed one's values across.
    world.resource_mut::<TerrainDataStore>().entry_for(&terrain);
    crate::commands::sync_component_to_ast(
        world,
        entity,
        "jackdaw_scene_types::types::Terrain",
        &terrain,
    );
    if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(entity) {
        dirty.rebuild_all = true;
    }
}

/// Add a channel to the selected terrain and make it active.
#[operator(
    id = "terrain.channel.add",
    label = "Add Scatter Mask",
    description = "Add a scatter mask to this terrain.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_channel_add(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut terrains: Query<&mut jackdaw_scene_types::Terrain>,
    mut paint: ResMut<TerrainPaintState>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    let mut terrain = terrains.get_mut(entity)?;

    let index = terrain.channels.len();
    let name = mint_channel_name(&terrain.channels);
    terrain.channels.push(TerrainChannel {
        name,
        element: TerrainChannelElement::U8,
        // A new channel carries an "unset" entry plus one paintable value,
        // so the brush has something to write without the user building a
        // palette first.
        palette: vec![
            TerrainPaletteEntry {
                value: 0,
                label: "unset".to_string(),
                color: Color::srgb(0.5, 0.5, 0.5),
            },
            TerrainPaletteEntry {
                value: 1,
                label: "value-1".to_string(),
                color: seed_color(0),
            },
        ],
    });
    paint.active_channel = index;
    paint.active_entry = 1;
    paint.show_channel = true;

    commands.queue(move |world: &mut World| commit_channels(world, entity));
    OperatorResult::Finished
}

/// Remove a channel from the selected terrain.
///
/// `allows_undo = false` because it pushes its own
/// [`RemoveTerrainChannel`] entry: the generic diff sees only the
/// descriptor half of a remove, since bulk channel values live outside the
/// AST, so a generic snapshot would double-record the change and lose the
/// painted data on undo.
#[operator(
    id = "terrain.channel.remove",
    label = "Remove Scatter Mask",
    description = "Remove a scatter mask and everything painted into it.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(index(i64, doc = "Scatter mask index to remove."))
)]
pub(crate) fn terrain_channel_remove(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    store: Res<TerrainDataStore>,
    mut paint: ResMut<TerrainPaintState>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    let index = usize::try_from(params.as_int("index")?).ok()?;
    let terrain = terrains.get(entity)?;
    if index >= terrain.channels.len() {
        return OperatorResult::Cancelled;
    }
    let descriptor = terrain.channels[index].clone();
    // The values live per region, so undo holds the plane gathered across
    // them: dropping the channel drops every region's plane at once.
    let data = store.get(&terrain.data_path).map(|document| {
        document
            .regions
            .read_grid_channel(index, document.grid_resolution())
    });

    let remaining = terrain.channels.len() - 1;
    paint.active_channel = paint.active_channel.min(remaining.saturating_sub(1));
    paint.active_entry = 0;

    commands.queue(move |world: &mut World| {
        let command = RemoveTerrainChannel {
            entity,
            index,
            descriptor,
            data,
            label: "Remove Scatter Mask".to_string(),
        };
        world.resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.execute(Box::new(command), world);
        });
    });
    OperatorResult::Finished
}

/// Undo command for `terrain.channel.remove`.
///
/// A channel removal touches two stores: the descriptor leaves
/// `Terrain::channels` and reaches the AST, and its per-cell values leave
/// [`TerrainDataStore`], dropped by `entry_for`'s reconcile once the
/// descriptor is gone. Both halves are captured here and restored on undo.
pub struct RemoveTerrainChannel {
    pub entity: Entity,
    pub index: usize,
    pub descriptor: TerrainChannel,
    pub data: Option<Vec<u16>>,
    pub label: String,
}

impl RemoveTerrainChannel {
    fn apply_removed(&self, world: &mut World) {
        let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        if self.index >= terrain.channels.len() {
            return;
        }
        terrain.channels.remove(self.index);
        commit_channels(world, self.entity);
    }

    fn apply_restored(&self, world: &mut World) {
        let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        let at = self.index.min(terrain.channels.len());
        terrain.channels.insert(at, self.descriptor.clone());
        let terrain = terrain.clone();

        if let Some(mut data) = world.resource_mut::<TerrainDataStore>().entry_for(&terrain)
            && let Some(restored) = &self.data
            && let Some(index) = data
                .channels()
                .iter()
                .position(|channel| channel.name == self.descriptor.name)
        {
            data.restore_channel(index, restored);
        }
        crate::commands::sync_component_to_ast(
            world,
            self.entity,
            "jackdaw_scene_types::types::Terrain",
            &terrain,
        );
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
    }
}

impl EditorCommand for RemoveTerrainChannel {
    fn execute(&mut self, world: &mut World) {
        self.apply_removed(world);
    }

    fn undo(&mut self, world: &mut World) {
        self.apply_restored(world);
    }

    fn description(&self) -> &str {
        &self.label
    }

    fn heap_bytes(&self) -> usize {
        // The removed channel's values: a `resolution^2` per-cell layer,
        // half a megabyte on a 512 terrain, held for as long as the removal
        // stays undoable.
        self.data
            .as_ref()
            .map_or(0, |values| values.capacity() * size_of::<u16>())
    }
}

#[cfg(test)]
mod remove_tests {
    use bevy::ecs::reflect::AppTypeRegistry;

    use super::*;

    fn channel(name: &str) -> TerrainChannel {
        TerrainChannel {
            name: name.to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        }
    }

    /// Undoing `terrain.channel.remove` restores the channel's painted
    /// values rather than minting a zeroed replacement.
    #[test]
    fn undoing_a_channel_remove_restores_its_painted_values() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.init_resource::<TerrainDataStore>();

        let terrain = jackdaw_scene_types::Terrain {
            resolution: 2,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            channels: vec![channel("biome"), channel("walkable")],
            ..default()
        };
        let entity = world.spawn(terrain.clone()).id();

        {
            // Regions sized to the fixture, so the terrain is four cells
            // rather than a default region's quarter-million.
            let mut regions = jackdaw_terrain::TerrainRegions::new(
                jackdaw_terrain::RegionSize::new(2).expect("a power of two"),
            );
            regions.ensure_grid(2).expect("inside the region cap");
            let mut store = world.resource_mut::<TerrainDataStore>();
            store.insert(
                terrain.data_path.clone(),
                jackdaw_terrain::RegionTerrainData {
                    regions,
                    ..default()
                },
            );
            let mut data = store.entry_for(&terrain).expect("keyed");
            data.set_channel_values(1, &[7, 7, 7, 7]);
        }

        let descriptor = terrain.channels[1].clone();
        let painted = world
            .resource::<TerrainDataStore>()
            .get(&terrain.data_path)
            .map(|data| data.regions.read_grid_channel(1, data.grid_resolution()))
            .expect("painted data captured");

        let mut command = RemoveTerrainChannel {
            entity,
            index: 1,
            descriptor,
            data: Some(painted),
            label: "Remove Scatter Mask".to_string(),
        };

        command.execute(&mut world);
        assert_eq!(
            world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .expect("terrain")
                .channels
                .len(),
            1,
            "the descriptor is gone after execute",
        );

        command.undo(&mut world);
        let restored = world
            .get::<jackdaw_scene_types::Terrain>(entity)
            .expect("terrain");
        assert_eq!(restored.channels.len(), 2);
        assert_eq!(restored.channels[1].name, "walkable");

        let data = world
            .resource::<TerrainDataStore>()
            .get(&restored.data_path)
            .expect("store entry still present");
        assert_eq!(
            data.regions.read_grid_channel(1, data.grid_resolution()),
            vec![7, 7, 7, 7],
            "undo must restore the painted values, not mint zeros",
        );
    }

    /// The captured channel is a whole `resolution^2` layer, so it counts
    /// against the history budget rather than reading as free.
    #[test]
    fn a_captured_channel_counts_against_the_history_budget() {
        let entity = Entity::from_raw_u32(1).expect("id");
        let populated = RemoveTerrainChannel {
            entity,
            index: 0,
            descriptor: channel("biome"),
            data: Some(vec![0u16; 64 * 64]),
            label: "Remove Scatter Mask".to_string(),
        };
        assert_eq!(populated.heap_bytes(), 64 * 64 * size_of::<u16>());

        let empty = RemoveTerrainChannel {
            entity,
            index: 0,
            descriptor: channel("biome"),
            data: None,
            label: "Remove Scatter Mask".to_string(),
        };
        assert_eq!(empty.heap_bytes(), 0);
    }
}

/// Make a channel the one the brush writes and the viewport tints.
#[operator(
    id = "terrain.channel.select",
    label = "Select Scatter Mask",
    description = "Choose which scatter mask the brush paints into.",
    allows_undo = false,
    params(index(i64, doc = "Scatter mask index to select."))
)]
pub(crate) fn terrain_channel_select(
    params: In<OperatorParameters>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    paint.active_channel = usize::try_from(params.as_int("index")?).ok()?;
    paint.active_entry = 0;
    OperatorResult::Finished
}

/// Append a palette entry to the active channel.
#[operator(
    id = "terrain.channel.value.add",
    label = "Add Palette Value",
    description = "Add a named value to the active scatter mask's palette.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_channel_value_add(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut terrains: Query<&mut jackdaw_scene_types::Terrain>,
    mut paint: ResMut<TerrainPaintState>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    let active = paint.active_channel;
    let mut terrain = terrains.get_mut(entity)?;
    let ceiling = terrain.channels.get(active)?.element.max_value();
    let channel = terrain.channels.get_mut(active)?;

    // One past the highest value in use, clamped to what the channel's
    // width can hold, so values stay stable as entries are added.
    let next = channel
        .palette
        .iter()
        .map(|entry| entry.value)
        .max()
        .map(|highest| highest.saturating_add(1))
        .unwrap_or(0)
        .min(ceiling);
    if channel.palette.iter().any(|entry| entry.value == next) {
        // The width is exhausted; another entry would alias an existing
        // value.
        return OperatorResult::Cancelled;
    }
    let index = channel.palette.len();
    channel.palette.push(TerrainPaletteEntry {
        value: next,
        label: format!("value-{next}"),
        color: seed_color(index.saturating_sub(1)),
    });
    paint.active_entry = index;

    commands.queue(move |world: &mut World| commit_channels(world, entity));
    OperatorResult::Finished
}

/// Choose which palette value the brush writes.
#[operator(
    id = "terrain.channel.value.select",
    label = "Select Palette Value",
    description = "Choose which value the brush paints.",
    allows_undo = false,
    params(index(i64, doc = "Palette entry index to select."))
)]
pub(crate) fn terrain_channel_value_select(
    params: In<OperatorParameters>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    paint.active_entry = usize::try_from(params.as_int("index")?).ok()?;
    OperatorResult::Finished
}

/// Toggle the viewport tint that shows what is painted.
#[operator(
    id = "terrain.channel.toggle_view",
    label = "Show Painted Mask",
    description = "Tint the terrain by the active scatter mask so painted values are visible.",
    allows_undo = false
)]
pub(crate) fn terrain_channel_toggle_view(
    _: In<OperatorParameters>,
    mut paint: ResMut<TerrainPaintState>,
) -> OperatorResult {
    paint.show_channel = !paint.show_channel;
    OperatorResult::Finished
}
