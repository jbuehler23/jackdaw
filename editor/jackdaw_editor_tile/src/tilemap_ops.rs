//! Making a tilemap from a tileset and editing its layer list. Every edit is
//! one history entry over both the scene's `Tilemap` and the store's cells.

use std::sync::{Arc, Mutex, PoisonError};

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalQuery;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_feathers::picker::{
    PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text, picker_item,
};
use jackdaw_scene_types::{LayerTerrainMatch, Tilemap, TilemapLayer, TilemapLayerKind};
use jackdaw_tilemap::{LayerData, TilemapData};
use jackdaw_tileset::{TerrainMatch, Tileset};

use crate::tile_tools::TileEditMode;
use crate::tilemap::TilemapDataStore;
use crate::tilemap_stroke::layer_param;
use crate::tileset_import::TILESET_KIND;
use jackdaw_editor_core::asset_index::AssetIndex;
use jackdaw_editor_core::commands::{
    CommandHistory, EditorCommand, SpawnEntity, sync_component_to_ast,
};
use jackdaw_editor_core::scene_nodes::live_entity;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilemapCreateOp>()
        .register_operator::<TilemapLayerAddOp>()
        .register_operator::<TilemapLayerRemoveOp>()
        .register_operator::<TilemapLayerSetOp>()
        .register_operator::<TilemapLayerActivateOp>();
}

const NO_TILESET: &str = "Import a tileset first: right-click an image in the Project window and \
                          choose Import as Tileset.";

/// The layer match modes as `tilemap.layer.set` names them, in combo order.
pub(crate) const MATCHES: [(&str, LayerTerrainMatch); 4] = [
    ("auto", LayerTerrainMatch::Auto),
    ("corners", LayerTerrainMatch::Corners),
    ("sides", LayerTerrainMatch::Sides),
    ("corners_and_sides", LayerTerrainMatch::CornersAndSides),
];

/// The project's tileset files, sorted, as the paths layers name them by.
pub(crate) fn project_tilesets(world: &World) -> Vec<String> {
    let mut paths = (world.get_resource::<AssetIndex>())
        .map(|index| index.paths_of_kind(TILESET_KIND))
        .unwrap_or_default();
    paths.sort();
    paths
}

/// Unlike the other Add rows this ignores keyboard focus, so the tileset list
/// it opens can call it while its search field holds the focus.
fn can_create(
    gate: jackdaw_editor_core::sprite_ops::SpriteAddGate,
    active: ActiveModalQuery,
    index: Option<Res<AssetIndex>>,
) -> bool {
    gate.has_2d_scene()
        && !active.is_modal_running()
        && index.is_some_and(|index| index.of_kind(TILESET_KIND).next().is_some())
}

fn create_unavailable(world: &World) -> Option<String> {
    jackdaw_editor_core::sprite_ops::needs_2d_scene(world, "a tilemap")
        .or_else(|| (project_tilesets(world).is_empty()).then(|| NO_TILESET.to_string()))
}

pub(crate) fn no_modal(active: ActiveModalQuery) -> bool {
    !active.is_modal_running()
}

/// A param, trimmed, or `None` when left out or blank.
fn text(params: &OperatorParameters, name: &str) -> Option<String> {
    Some(params.as_str(name)?.trim().to_string()).filter(|text| !text.is_empty())
}

/// Ends a call of operator `id`: a message goes back to the caller, empty for
/// none, and an error is warned as the reason the call was refused.
pub(crate) fn finish(
    world: &mut World,
    id: &str,
    result: Result<String, String>,
) -> OperatorResult {
    match result {
        Ok(message) => {
            if !message.is_empty() {
                report_to_caller(world, message);
            }
            OperatorResult::Finished
        }
        Err(err) => {
            warn_caller(world, format!("{id}: {err}"));
            OperatorResult::Cancelled
        }
    }
}

/// Add a tilemap of a tileset to the open 2D world scene.
#[operator(
    id = "tilemap.create",
    label = "Tilemap",
    description = "Add a tilemap of a tileset to the open 2D world scene.",
    allows_undo = false,
    unavailable_reason = create_unavailable,
    is_available = can_create,
    params(
        tileset(String, doc = "Tileset file; left out, a list of the project's tilesets opens."),
        kind(String, doc = "terrain or tile; terrain when the tileset has terrain sets."),
        name(String, doc = "Name of the map; Tilemap when left out."),
    )
)]
pub(crate) fn tilemap_create(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    if text(&params, "tileset").is_none() {
        open_tileset_picker(world);
        return OperatorResult::Finished;
    }
    let result = create(world, &params);
    finish(world, TilemapCreateOp::ID, result.map(|()| String::new()))
}

fn create(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    close_tileset_pickers(world);
    let path = asset_path(params).unwrap_or_default();
    let (tile_size, modes) = tileset_facts(world, &path)?;
    let mut layer = new_layer(&modes, path, text(params, "kind"), "Layer 1".into())?;
    let parent = jackdaw_editor_core::sprite_ops::scene_2d_parent(world, None)
        .ok_or("no 2D world scene is open")?;
    let name = text(params, "name").unwrap_or_else(|| "Tilemap".to_string());
    let taken = &mut jackdaw_editor_core::entity_ops::scene_entity_names(world);
    let name = jackdaw_editor_core::entity_ops::claim_free_name(taken, &name).unwrap_or(name);
    let mut tilemap = Tilemap {
        cell_size: tile_size,
        data_path: crate::tilemap::mint_map_path(world),
        ..default()
    };
    layer.id = tilemap.mint_layer_id();
    tilemap.layers.push(layer);
    let tilemap = Arc::new(Mutex::new(tilemap));
    let shared = tilemap.clone();
    let label = format!("Add Tilemap {name}");
    let spawn = SpawnEntity::new(
        move |world: &mut World| {
            let parent = live_entity(world, parent);
            let tilemap = shared.lock().unwrap_or_else(PoisonError::into_inner);
            let bundle = (Name::new(name.clone()), tilemap.clone(), ChildOf(parent));
            let entity = world.spawn(bundle).id();
            jackdaw_editor_core::scene_io::register_entity_in_ast(world, entity);
            jackdaw_editor_core::selection::select_only(world, entity);
            entity
        },
        label,
    );
    let command = CreateTilemap {
        spawn,
        tilemap,
        map: None,
        held: None,
    };
    push(world, command);
    Ok(())
}

fn push(world: &mut World, mut command: impl EditorCommand + 'static) {
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
}

fn asset_path(params: &OperatorParameters) -> Option<String> {
    text(params, "tileset").map(|path| jackdaw_editor_core::entity_ops::to_asset_path(&path))
}

/// A tileset's tile size and the match mode of each of its terrain sets.
fn tileset_facts(world: &mut World, path: &str) -> Result<(UVec2, Vec<TerrainMatch>), String> {
    let id = crate::tileset_dock::tileset_id(world, path)?;
    let tileset = (world.resource::<Assets<Tileset>>().get(id))
        .ok_or_else(|| format!("{path} is not loaded yet"))?;
    let modes = (tileset.terrain_sets.iter()).map(|set| set.mode).collect();
    Ok((tileset.tile_size, modes))
}

/// A layer of the tileset at `path` whose terrain sets have `modes`: of
/// `kind`, or terrain when the tileset has terrain sets and tile otherwise.
/// A tileset of mixed modes gets its first set's mode. Its id is left 0 for
/// the caller to give.
fn new_layer(
    modes: &[TerrainMatch],
    path: String,
    kind: Option<String>,
    name: String,
) -> Result<TilemapLayer, String> {
    let kind = match kind.as_deref() {
        None if modes.is_empty() => TilemapLayerKind::Tile,
        None | Some("terrain") => TilemapLayerKind::Terrain,
        Some("tile") => TilemapLayerKind::Tile,
        Some(other) => return Err(format!("kind is terrain or tile, not `{other}`")),
    };
    let mut terrain_match = LayerTerrainMatch::Auto;
    if kind == TilemapLayerKind::Terrain {
        let first = *(modes.first())
            .ok_or_else(|| format!("{path} has no terrain sets; make a tile layer"))?;
        if modes.iter().any(|mode| *mode != first) {
            terrain_match = MATCHES[1 + first as usize].1;
        }
    }
    Ok(TilemapLayer {
        id: 0,
        name,
        tileset: path,
        kind,
        terrain_match,
    })
}

/// Spawning a map and its cells. The map is held by its store id, so undo and
/// redo reach its cells after a save moves them.
struct CreateTilemap {
    spawn: SpawnEntity,
    /// The map as the next redo spawns it.
    tilemap: Arc<Mutex<Tilemap>>,
    map: Option<u64>,
    held: Option<(TilemapData, u64)>,
}

impl EditorCommand for CreateTilemap {
    fn execute(&mut self, world: &mut World) {
        let tilemap = self.tilemap.lock().unwrap_or_else(PoisonError::into_inner);
        let data_path = tilemap.data_path.clone();
        drop(tilemap);
        self.map = crate::tilemap::claim_map(world, &data_path, self.held.take());
        self.spawn.execute(world);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawn.spawned.map(|e| live_entity(world, e))
            && let Some(now) = world.get::<Tilemap>(entity)
        {
            *self.tilemap.lock().unwrap_or_else(PoisonError::into_inner) = now.clone();
        }
        self.spawn.undo(world);
        self.held = self
            .map
            .and_then(|id| crate::tilemap::release_map(world, id));
    }

    fn description(&self) -> &str {
        self.spawn.description()
    }
}

/// The tilesets list the Add menu's Tilemap row opens.
#[derive(Component)]
pub struct TilesetPicker;

fn close_tileset_pickers(world: &mut World) {
    let open: Vec<Entity> = (world.query_filtered::<Entity, With<TilesetPicker>>())
        .iter(world)
        .collect();
    for picker in open {
        world.entity_mut(picker).despawn();
    }
}

fn open_tileset_picker(world: &mut World) {
    close_tileset_pickers(world);
    let tilesets = project_tilesets(world);
    if tilesets.is_empty() {
        warn_caller(world, format!("{}: {NO_TILESET}", TilemapCreateOp::ID));
        return;
    }
    world.spawn((
        PickerProps::new(spawn_tileset_item, pick_tileset)
            .items(tilesets)
            .title("Pick Tileset")
            .placeholder(Some("Search tilesets..")),
        TilesetPicker,
        jackdaw_editor_core::EditorEntity,
        jackdaw_editor_core::BlocksCameraInput,
    ));
}

fn spawn_tileset_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    mut commands: Commands,
) -> Result {
    commands.spawn((
        picker_item(matched.index),
        ChildOf(entities.list),
        children![match_text(matched.segments)],
    ));
    Ok(())
}

fn pick_tileset(
    input: In<SelectInput>,
    items: Query<&PickerItems<String>>,
    mut commands: Commands,
) -> Result {
    let picker = input.entities.picker;
    let chosen = items.get(picker)?.at(input.index)?.clone();
    commands.entity(picker).try_despawn();
    commands
        .operator(TilemapCreateOp::ID)
        .param("tileset", chosen)
        .call();
    Ok(())
}

/// One layer as an edit finds or leaves it: the scene layer, and the cells it
/// moves into the store or took out of it.
struct LayerSlot {
    layer: TilemapLayer,
    data: Option<LayerData>,
}

/// Puts `after` in place of `before` at `index` of one map; both, when
/// given, have the same layer id. Adding or removing a layer moves its cells;
/// a change moves them only when `cells`.
struct EditTilemapLayers {
    entity: Entity,
    map: u64,
    index: usize,
    before: Option<LayerSlot>,
    after: Option<LayerSlot>,
    cells: bool,
    /// The map's next layer id once the edit is made; undo keeps it, so an
    /// id is never given twice.
    next_layer_id: u32,
    label: String,
}

impl EditTilemapLayers {
    fn apply(&mut self, world: &mut World, undo: bool) {
        self.entity = live_entity(world, self.entity);
        let (entity, index, map) = (self.entity, self.index, self.map);
        let cells = self.cells || self.before.is_none() || self.after.is_none();
        let (from, to) = if undo {
            (&mut self.after, &mut self.before)
        } else {
            (&mut self.before, &mut self.after)
        };
        let Some(id) = (to.as_ref().or(from.as_ref())).map(|slot| slot.layer.id) else {
            return;
        };
        let Some(mut tilemap) = world.get_mut::<Tilemap>(entity) else {
            return;
        };
        if from.is_some()
            && let Some(at) = tilemap.layer_index(id)
        {
            tilemap.layers.remove(at);
        }
        if let Some(to) = to.as_ref() {
            let at = index.min(tilemap.layers.len());
            tilemap.layers.insert(at, to.layer.clone());
        }
        tilemap.next_layer_id = tilemap.next_layer_id.max(self.next_layer_id);
        let tilemap = tilemap.clone();
        sync_component_to_ast(world, entity, Tilemap::type_path(), &tilemap);
        let mut store = world.resource_mut::<TilemapDataStore>();
        if let Some(key) = store.key_of_map(map).map(str::to_owned)
            && cells
        {
            let insert = to.as_mut().and_then(|to| to.data.take());
            let removed = store.replace_layer(&key, id, insert);
            if let Some(from) = from.as_mut() {
                from.data = removed;
            }
        }
        keep_active_layer(world, entity, index, from.is_some(), to.is_some());
        rebuild_card(world, entity);
    }
}

impl EditorCommand for EditTilemapLayers {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, false);
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, true);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

fn rebuild_card(world: &mut World, entity: Entity) {
    if let Some(mut pending) =
        world.get_resource_mut::<jackdaw_editor_core::inspector::PendingInspectorRebuild>()
    {
        pending.0 = Some(entity);
    }
}

/// Keeps tile edit mode on the layer it paints when a layer below it comes or
/// goes, and leaves the mode when that layer goes.
fn keep_active_layer(world: &mut World, map: Entity, index: usize, removed: bool, added: bool) {
    let mode = *world.resource::<TileEditMode>();
    if mode.map != Some(map) || removed == added || index > mode.layer {
        return;
    }
    if removed && index == mode.layer {
        crate::tile_tools::leave_edit_mode(world);
        return;
    }
    let layer = if removed {
        mode.layer - 1
    } else {
        mode.layer + 1
    };
    world.resource_mut::<TileEditMode>().layer = layer;
}

/// The map `params` name, or the selected one.
pub(crate) fn target_entity(world: &World, params: &OperatorParameters) -> Option<Entity> {
    (params.as_entity("entity")).or_else(|| {
        world
            .resource::<jackdaw_editor_core::selection::Selection>()
            .primary()
    })
}

/// The map `params` name, or the selected one, with its store id.
fn target(
    world: &mut World,
    params: &OperatorParameters,
) -> Result<(Entity, Tilemap, u64), String> {
    let entity = target_entity(world, params).ok_or("no tilemap given or selected")?;
    let tilemap = (world.get::<Tilemap>(entity).cloned()).ok_or("the entity is not a tilemap")?;
    let key = crate::tilemap::editable_key(world, entity)?;
    let store = world.resource::<TilemapDataStore>();
    let map = (store.map_id(&key)).ok_or("the map's cells are not loaded")?;
    Ok((entity, tilemap, map))
}

/// The index of the layer the `layer` param names; the bottom one when it is
/// left out.
fn layer_index(params: &OperatorParameters, tilemap: &Tilemap) -> Result<usize, String> {
    match layer_param(tilemap, params)? {
        Some(index) => Ok(index),
        None if tilemap.layers.is_empty() => Err("the map has no layers".into()),
        None => Ok(0),
    }
}

/// Add a layer to the top of a tilemap.
#[operator(
    id = "tilemap.layer.add",
    label = "Add Tilemap Layer",
    description = "Add a layer on top of the tilemap's layers.",
    allows_undo = false,
    is_available = no_modal,
    params(
        entity(Entity, doc = "The tilemap; the selection when left out."),
        tileset(String, doc = "Tileset file; the top layer's when left out."),
        kind(String, doc = "terrain or tile; terrain when the tileset has terrain sets."),
        name(String, doc = "Name of the layer; Layer and its number when left out."),
    )
)]
pub(crate) fn tilemap_layer_add(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = add_layer(world, &params).map(|edit| push(world, edit));
    finish(world, TilemapLayerAddOp::ID, result.map(|()| String::new()))
}

fn add_layer(world: &mut World, params: &OperatorParameters) -> Result<EditTilemapLayers, String> {
    let (entity, mut tilemap, map) = target(world, params)?;
    let layers = &tilemap.layers;
    let path = (asset_path(params))
        .or_else(|| layers.last().map(|layer| layer.tileset.clone()))
        .ok_or("no tileset given")?;
    let (_, modes) = tileset_facts(world, &path)?;
    let name = text(params, "name").unwrap_or_else(|| {
        (layers.len() + 1..)
            .map(|n| format!("Layer {n}"))
            .find(|name| !layers.iter().any(|layer| layer.name == *name))
            .unwrap_or_default()
    });
    let mut layer = new_layer(&modes, path, text(params, "kind"), name)?;
    let index = layers.len();
    layer.id = tilemap.mint_layer_id();
    Ok(EditTilemapLayers {
        entity,
        map,
        index,
        label: format!("Add Layer {}", layer.name),
        before: None,
        after: Some(LayerSlot { layer, data: None }),
        cells: true,
        next_layer_id: tilemap.next_layer_id,
    })
}

/// Remove one layer of a tilemap, and its cells.
#[operator(
    id = "tilemap.layer.remove",
    label = "Remove Tilemap Layer",
    description = "Remove a layer of the tilemap and its cells. The last layer stays.",
    allows_undo = false,
    is_available = no_modal,
    params(
        entity(Entity, doc = "The tilemap; the selection when left out."),
        layer(
            String,
            doc = "The layer: its name, # and its id (#3), or its index from 0 at the bottom; \
                   the bottom layer when left out."
        ),
    )
)]
pub(crate) fn tilemap_layer_remove(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = remove_layer(world, &params).map(|edit| push(world, edit));
    finish(
        world,
        TilemapLayerRemoveOp::ID,
        result.map(|()| String::new()),
    )
}

fn remove_layer(
    world: &mut World,
    params: &OperatorParameters,
) -> Result<EditTilemapLayers, String> {
    let (entity, tilemap, map) = target(world, params)?;
    let index = layer_index(params, &tilemap)?;
    if tilemap.layers.len() == 1 {
        return Err("the map's last layer cannot be removed".into());
    }
    let layer = tilemap.layers[index].clone();
    Ok(EditTilemapLayers {
        entity,
        map,
        index,
        label: format!("Remove Layer {}", layer.name),
        before: Some(LayerSlot { layer, data: None }),
        after: None,
        cells: true,
        next_layer_id: tilemap.next_layer_id,
    })
}

/// Change a tilemap layer's name, kind, tileset or match mode.
#[operator(
    id = "tilemap.layer.set",
    label = "Set Tilemap Layer",
    description = "Rename a layer of the tilemap, or change its kind, tileset or match mode. A \
                   new kind or tileset starts the layer empty.",
    allows_undo = false,
    is_available = no_modal,
    params(
        entity(Entity, doc = "The tilemap; the selection when left out."),
        layer(
            String,
            doc = "The layer: its name, # and its id (#3), or its index from 0 at the bottom; \
                   the bottom layer when left out."
        ),
        tileset(String, doc = "Tileset file; kept when left out."),
        kind(String, doc = "terrain or tile; kept when left out."),
        name(String, doc = "Name of the layer; kept when left out."),
        terrain_match(
            String,
            doc = "auto, corners, sides or corners_and_sides; kept when left out."
        ),
    )
)]
pub(crate) fn tilemap_layer_set(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = set_layer(world, &params);
    if result.is_err()
        && let Some(entity) = target_entity(world, &params)
    {
        rebuild_card(world, entity);
    }
    finish(world, TilemapLayerSetOp::ID, result.map(|()| String::new()))
}

fn set_layer(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    let (entity, tilemap, map) = target(world, params)?;
    let index = layer_index(params, &tilemap)?;
    let old = tilemap.layers[index].clone();
    let path = asset_path(params).unwrap_or_else(|| old.tileset.clone());
    let kind = text(params, "kind");
    let mut layer = old.clone();
    let cells = path != old.tileset || kind.as_deref().is_some_and(|k| k != kind_name(old.kind));
    if cells {
        let (_, modes) = tileset_facts(world, &path)?;
        let kind = kind.or_else(|| Some(kind_name(old.kind).to_string()));
        layer = TilemapLayer {
            id: old.id,
            ..new_layer(&modes, path, kind, old.name.clone())?
        };
    }
    if let Some(name) = text(params, "name") {
        layer.name = name;
    }
    if let Some(wanted) = text(params, "terrain_match") {
        layer.terrain_match = (MATCHES.iter().find(|(name, _)| *name == wanted))
            .map(|(_, mode)| *mode)
            .ok_or_else(|| format!("terrain_match `{wanted}` is not one of auto, corners, sides or corners_and_sides"))?;
    }
    if layer == old {
        return Ok(());
    }
    let edit = EditTilemapLayers {
        entity,
        map,
        index,
        label: format!("Set Layer {}", layer.name),
        before: Some(LayerSlot {
            layer: old,
            data: None,
        }),
        after: Some(LayerSlot { layer, data: None }),
        cells,
        next_layer_id: tilemap.next_layer_id,
    };
    push(world, edit);
    Ok(())
}

fn kind_name(kind: TilemapLayerKind) -> &'static str {
    match kind {
        TilemapLayerKind::Terrain => "terrain",
        TilemapLayerKind::Tile => "tile",
    }
}

/// Paint one layer of a tilemap: enter tile edit mode on it, or move the mode
/// to it.
#[operator(
    id = "tilemap.layer.activate",
    label = "Paint Tilemap Layer",
    description = "Paint a layer of the tilemap with the tile tools.",
    allows_undo = false,
    is_available = no_modal,
    params(
        entity(Entity, doc = "The tilemap; the selection when left out."),
        layer(
            String,
            doc = "The layer: its name, # and its id (#3), or its index from 0 at the bottom; \
                   the bottom layer when left out."
        ),
    )
)]
pub(crate) fn tilemap_layer_activate(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = activate_layer(world, &params);
    finish(
        world,
        TilemapLayerActivateOp::ID,
        result.map(|()| String::new()),
    )
}

fn activate_layer(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    let entity = target_entity(world, params).ok_or("no tilemap given or selected")?;
    let tilemap = (world.get::<Tilemap>(entity).cloned()).ok_or("the entity is not a tilemap")?;
    let index = layer_index(params, &tilemap)?;
    if world
        .resource::<jackdaw_editor_core::selection::Selection>()
        .primary()
        != Some(entity)
    {
        jackdaw_editor_core::selection::select_only(world, entity);
    }
    crate::tile_tools::enter_layer(world, entity, index);
    Ok(())
}
