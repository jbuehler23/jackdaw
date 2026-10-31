use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_commands::CommandHistory;
use jackdaw_tilemap::{MatchMode, Peer, TerrainId, TileAddress, TileTerrains, TilesetSetup};
use jackdaw_tileset::{TerrainCollision, Tileset, TilesetTerrain};

use crate::asset_index::{AssetValue, absolute_path, note_written};
use crate::commands::EditorCommand;
use crate::definition_assets::{
    entry_for_file, open_definition_path, rebuild_open_card, resolve_project_path, write_asset_file,
};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilesetTerrainAddOp>();
    ctx.register_operator::<TilesetTerrainRemoveOp>();
    ctx.register_operator::<TilesetTerrainRenameOp>();
    ctx.register_operator::<TilesetTerrainSplitOp>();
    ctx.register_operator::<TilesetTerrainMergeOp>();
    ctx.register_operator::<TilesetTerrainSetCollisionOp>();
    ctx.register_operator::<TilesetTerrainSetAddOp>();
    ctx.register_operator::<TilesetTerrainSetSetOp>();
    ctx.register_operator::<TilesetTileSetBitsOp>();
}

/// Replace a tileset with a new value, as one undo entry. Keyed by the file,
/// so undo still reaches it after the tileset was closed.
pub struct SetTileset {
    pub path: PathBuf,
    pub before: Tileset,
    pub after: Tileset,
    pub label: String,
}

impl EditorCommand for SetTileset {
    fn execute(&mut self, world: &mut World) {
        if let Err(err) = put_tileset(world, &self.path, self.after.clone()) {
            warn!("{}: {err}", self.label);
        }
    }

    fn undo(&mut self, world: &mut World) {
        if let Err(err) = put_tileset(world, &self.path, self.before.clone()) {
            warn!("{}: {err}", self.label);
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// The indexed path and value of the tileset `path` names, or of the open
/// tileset when `path` is empty.
pub(crate) fn open_tileset(world: &mut World, path: &str) -> Result<(PathBuf, Tileset), String> {
    let file = if path.trim().is_empty() {
        let open = open_definition_path(world).ok_or("no path given and no tileset is open")?;
        absolute_path(world, &open)
    } else {
        resolve_project_path(world, Path::new(path.trim()))
    };
    let entry = entry_for_file(world, &file)
        .ok_or_else(|| format!("{} is not an asset file of this project", file.display()))?;
    if entry.type_path != Tileset::type_path() {
        return Err(format!("{} is not a tileset", entry.path.display()));
    }
    let handle = entry.value.handle().ok_or("the tileset is not loaded")?;
    let tileset = (world.resource::<Assets<Tileset>>())
        .get(handle.id().typed::<Tileset>())
        .cloned()
        .ok_or("the tileset is not loaded")?;
    Ok((entry.path, tileset))
}

/// Write `value` to the file of the tileset at `path`, then put it in the
/// store. A failed write leaves both as they were.
fn put_tileset(world: &mut World, path: &Path, value: Tileset) -> Result<(), String> {
    let entry = entry_for_file(world, path).ok_or("the tileset file is gone")?;
    let id = (entry.value.handle())
        .ok_or("the tileset is not loaded")?
        .id()
        .typed::<Tileset>();
    let staged = world.resource_mut::<Assets<Tileset>>().add(value.clone());
    let file = absolute_path(world, path);
    let staged_value = AssetValue::Handle(staged.clone().untyped());
    let written = write_asset_file(world, &entry.name(), &staged_value, &file);
    world.resource_mut::<Assets<Tileset>>().remove(&staged);
    written.map_err(|e| e.to_string())?;
    note_written(world, &file);
    {
        let mut tilesets = world.resource_mut::<Assets<Tileset>>();
        let mut slot = tilesets.get_mut(id).ok_or("the tileset is not loaded")?;
        *slot = value;
    }
    if open_definition_path(world).as_deref() == Some(path) {
        rebuild_open_card(world);
    }
    Ok(())
}

/// Queue `edit` on the tileset `path` names: a change is stored, written and
/// pushed as one undo entry, and the message goes to the caller.
pub(crate) fn queue_edit<F>(
    commands: &mut Commands,
    op: &'static str,
    path: String,
    edit: F,
) -> OperatorResult
where
    F: FnOnce(&World, &mut Tileset) -> Result<String, String> + Send + 'static,
{
    commands.queue(
        move |world: &mut World| match apply_edit(world, op, &path, edit) {
            Ok(message) => report_to_caller(world, message),
            Err(err) => warn_caller(world, format!("{op}: {err}")),
        },
    );
    OperatorResult::Finished
}

/// Run `edit` on the tileset `path` names now: a change is stored, written
/// and pushed as one undo entry labelled `op`.
pub(crate) fn apply_edit(
    world: &mut World,
    op: &str,
    path: &str,
    edit: impl FnOnce(&World, &mut Tileset) -> Result<String, String>,
) -> Result<String, String> {
    let (path, before) = open_tileset(world, path)?;
    let mut after = before.clone();
    let message = edit(world, &mut after)?;
    if after != before {
        put_tileset(world, &path, after.clone())?;
        let command = SetTileset {
            path,
            before,
            after,
            label: op.to_string(),
        };
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(command));
    }
    Ok(message)
}

/// Run `edit` on the tileset's terrain setup and take the result back.
pub(crate) fn with_setup(
    tileset: &mut Tileset,
    edit: impl FnOnce(&Tileset, &mut TilesetSetup) -> Result<String, String>,
) -> Result<String, String> {
    let mut setup = tileset.setup().map_err(|e| e.to_string())?;
    let message = edit(tileset, &mut setup)?;
    tileset.set_setup(&setup).map_err(|e| e.to_string())?;
    Ok(message)
}

pub(crate) fn text(params: &OperatorParameters, name: &str) -> String {
    params.as_str(name).unwrap_or_default().trim().to_string()
}

/// A terrain by id or name.
pub(crate) fn terrain(tileset: &Tileset, text: &str) -> Result<TerrainId, String> {
    let found = (tileset.terrains.iter())
        .find(|t| text.parse::<TerrainId>().is_ok_and(|id| id == t.id) || t.name == text);
    found
        .map(|t| t.id)
        .ok_or_else(|| format!("no terrain is `{text}`"))
}

fn terrain_mut(tileset: &mut Tileset, id: TerrainId) -> Result<&mut TilesetTerrain, String> {
    (tileset.terrains.iter_mut())
        .find(|t| t.id == id)
        .ok_or_else(|| format!("no terrain is `{id}`"))
}

/// A terrain or the empty terrain 0, spelled empty, none, 0 or nothing.
pub(crate) fn terrain_or_empty(tileset: &Tileset, text: &str) -> Result<TerrainId, String> {
    if ["", "0", "empty", "none"].contains(&text) {
        return Ok(0);
    }
    terrain(tileset, text)
}

fn free_name(tileset: &Tileset, name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.parse::<u32>().is_ok()
        || ["empty", "erase", "keep", "none"].contains(&name)
    {
        return Err(format!("`{name}` cannot name a terrain"));
    }
    if tileset.terrains.iter().any(|t| t.name == name) {
        return Err(format!("a terrain is already named `{name}`"));
    }
    Ok(())
}

/// A sheet by index or source path.
pub(crate) fn sheet(tileset: &Tileset, text: &str) -> Result<u32, String> {
    let found = (tileset.sheets.iter().zip(0..))
        .find(|(s, i)| text.parse::<u32>().is_ok_and(|n| n == *i) || s.source == text);
    found
        .map(|(_, i)| i)
        .ok_or_else(|| format!("no sheet is `{text}`"))
}

/// A tile as a packed layer or `sheet:tile`.
pub(crate) fn tile(tileset: &Tileset, text: &str) -> Result<(u32, u32), String> {
    let found = match text.rsplit_once(':') {
        Some((on, index)) => {
            let on = sheet(tileset, on.trim())?;
            let index: u32 = (index.trim().parse()).map_err(|_| format!("`{text}` is no tile"))?;
            tileset.layer(on, index).map(|_| (on, index))
        }
        None => text
            .parse()
            .ok()
            .and_then(|layer| tileset.tile_of_layer(layer)),
    };
    found.ok_or_else(|| format!("tile `{text}` is out of range"))
}

pub(crate) fn tile_list(tileset: &Tileset, text: &str) -> Result<Vec<(u32, u32)>, String> {
    let tiles: Vec<(u32, u32)> = (text.split(',').map(str::trim))
        .filter(|t| !t.is_empty())
        .map(|t| tile(tileset, t))
        .collect::<Result<_, _>>()?;
    if tiles.is_empty() {
        return Err("no tiles given".into());
    }
    Ok(tiles)
}

pub(crate) fn setup_tiles(tileset: &Tileset, text: &str) -> Result<BTreeSet<TileAddress>, String> {
    (tile_list(tileset, text)?.into_iter())
        .map(|(sheet, tile)| {
            tileset
                .setup_address(sheet, tile)
                .map_err(|e| e.to_string())
        })
        .collect()
}

fn match_mode(text: &str) -> Result<MatchMode, String> {
    match text {
        "corners" => Ok(MatchMode::Corners),
        "sides" => Ok(MatchMode::Sides),
        "corners_and_sides" => Ok(MatchMode::CornersAndSides),
        other => Err(format!(
            "mode is corners, sides or corners_and_sides, not `{other}`"
        )),
    }
}

fn collision_mode(text: &str) -> Result<TerrainCollision, String> {
    match text {
        "solid" => Ok(TerrainCollision::Solid),
        "trigger" => Ok(TerrainCollision::Trigger),
        "passable" => Ok(TerrainCollision::Passable),
        other => Err(format!("mode is solid, trigger or passable, not `{other}`")),
    }
}

const PEER_NAMES: [&str; 9] = [
    "top_left",
    "top",
    "top_right",
    "left",
    "centre",
    "right",
    "bottom_left",
    "bottom",
    "bottom_right",
];

fn peer(text: &str) -> Result<Peer, String> {
    let index = (PEER_NAMES.iter().position(|p| *p == text))
        .or_else(|| text.parse::<usize>().ok().filter(|i| *i < 9));
    index.map(|i| Peer::ALL[i]).ok_or_else(|| {
        format!(
            "peer is 0 to 8 or one of {}, not `{text}`",
            PEER_NAMES.join(", ")
        )
    })
}

/// Add a terrain to a tileset.
#[operator(
    id = "tileset.terrain.add",
    label = "Add Tileset Terrain",
    description = "Add a terrain to a tileset; it takes the next free id, up to 255.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        name(String, doc = "Name of the new terrain; unique, not a number."),
        swatch(String, default = "#808080", doc = "Swatch colour as #rrggbb."),
        background(bool, default = false, doc = "Whether it is the background terrain."),
    )
)]
pub fn tileset_terrain_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = text(&params, "name");
    let swatch = text(&params, "swatch");
    let background = params.as_bool("background").unwrap_or(false);
    queue_edit(
        &mut commands,
        "tileset.terrain.add",
        text(&params, "path"),
        move |_, tileset| {
            free_name(tileset, &name)?;
            let colour = Srgba::hex(if swatch.is_empty() {
                "#808080"
            } else {
                &swatch
            })
            .map_err(|_| format!("swatch is #rrggbb, not `{swatch}`"))?;
            let [r, g, b, _] = colour.to_u8_array();
            with_setup(tileset, |_, setup| {
                let id = (setup.add_terrain(name.clone(), [r, g, b], background))
                    .map_err(|e| e.to_string())?;
                Ok(format!("terrain {id} \"{name}\" added"))
            })
        },
    )
}

/// Remove a terrain from a tileset.
#[operator(
    id = "tileset.terrain.remove",
    label = "Remove Tileset Terrain",
    description = "Remove a terrain: its peers become empty and its terrain sets go.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        terrain(String, doc = "Terrain id or name."),
    )
)]
pub fn tileset_terrain_remove(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = text(&params, "terrain");
    queue_edit(
        &mut commands,
        "tileset.terrain.remove",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let id = terrain(tileset, &name)?;
                setup.remove_terrain(id).map_err(|e| e.to_string())?;
                Ok(format!("terrain {id} removed"))
            })
        },
    )
}

/// Rename a terrain of a tileset.
#[operator(
    id = "tileset.terrain.rename",
    label = "Rename Tileset Terrain",
    description = "Rename a terrain; a re-run of auto setup keeps the new name.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        terrain(String, doc = "Terrain id or name."),
        name(String, doc = "New name; unique, not a number."),
    )
)]
pub fn tileset_terrain_rename(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (from, name) = (text(&params, "terrain"), text(&params, "name"));
    queue_edit(
        &mut commands,
        "tileset.terrain.rename",
        text(&params, "path"),
        move |_, tileset| {
            let id = terrain(tileset, &from)?;
            free_name(tileset, &name)?;
            let renamed = terrain_mut(tileset, id)?;
            renamed.name.clone_from(&name);
            renamed.name_by_user = true;
            Ok(format!("terrain {id} is now \"{name}\""))
        },
    )
}

/// Split some tiles off a terrain into a new one.
#[operator(
    id = "tileset.terrain.split",
    label = "Split Tileset Terrain",
    description = "Move the listed tiles' peers of a terrain to a new terrain like it, with \
                   copies of the terrain sets they border.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        terrain(String, doc = "Terrain id or name."),
        tiles(
            String,
            doc = "Comma-separated tiles, each a packed layer or sheet:tile."
        ),
        name(
            String,
            doc = "Name of the new terrain; defaults to the old name and \"split\"."
        ),
    )
)]
pub fn tileset_terrain_split(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (from, tiles, name) = (
        text(&params, "terrain"),
        text(&params, "tiles"),
        text(&params, "name"),
    );
    queue_edit(
        &mut commands,
        "tileset.terrain.split",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let id = terrain(tileset, &from)?;
                let tiles = setup_tiles(tileset, &tiles)?;
                let name = if name.is_empty() {
                    let old = setup.terrain(id).map(|t| t.name.value.clone());
                    format!("{} split", old.unwrap_or_default())
                } else {
                    name
                };
                free_name(tileset, &name)?;
                let new = setup
                    .split_terrain(id, &tiles, name.clone())
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "terrain {new} \"{name}\" split from terrain {id} on {} tiles",
                    tiles.len()
                ))
            })
        },
    )
}

/// Merge one terrain into another.
#[operator(
    id = "tileset.terrain.merge",
    label = "Merge Tileset Terrains",
    description = "Fold one terrain into another: its peers and terrain sets take the other's id.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        from(String, doc = "Terrain id or name that goes."),
        into(String, doc = "Terrain id or name that stays."),
    )
)]
pub fn tileset_terrain_merge(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (from, into) = (text(&params, "from"), text(&params, "into"));
    queue_edit(
        &mut commands,
        "tileset.terrain.merge",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (from, into) = (terrain(tileset, &from)?, terrain(tileset, &into)?);
                setup
                    .merge_terrains(from, into)
                    .map_err(|e| e.to_string())?;
                Ok(format!("terrain {from} merged into terrain {into}"))
            })
        },
    )
}

/// Choose how a terrain collides.
#[operator(
    id = "tileset.terrain.set_collision",
    label = "Set Terrain Collision",
    description = "Choose whether a terrain is solid, a trigger or passable.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        terrain(String, doc = "Terrain id or name."),
        mode(String, doc = "solid, trigger or passable."),
    )
)]
pub fn tileset_terrain_set_collision(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (name, mode) = (text(&params, "terrain"), text(&params, "mode"));
    queue_edit(
        &mut commands,
        "tileset.terrain.set_collision",
        text(&params, "path"),
        move |_, tileset| {
            let id = terrain(tileset, &name)?;
            let collision = collision_mode(&mode)?;
            let changed = terrain_mut(tileset, id)?;
            changed.collision = collision;
            changed.collision_by_user = true;
            Ok(format!("terrain {id} is {mode}"))
        },
    )
}

/// Add a terrain set to a tileset.
#[operator(
    id = "tileset.terrain_set.add",
    label = "Add Terrain Set",
    description = "Add the transition set of an inner terrain over an outer one.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        inner(String, doc = "Inner terrain id or name."),
        outer(String, doc = "Outer terrain id or name, or empty."),
        mode(
            String,
            default = "corners_and_sides",
            doc = "corners, sides or corners_and_sides."
        ),
    )
)]
pub fn tileset_terrain_set_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (inner, outer, mode) = (
        text(&params, "inner"),
        text(&params, "outer"),
        text(&params, "mode"),
    );
    queue_edit(
        &mut commands,
        "tileset.terrain_set.add",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (inner, outer) = (
                    terrain(tileset, &inner)?,
                    terrain_or_empty(tileset, &outer)?,
                );
                let mode = if mode.is_empty() {
                    MatchMode::CornersAndSides
                } else {
                    match_mode(&mode)?
                };
                setup
                    .add_set(inner, outer, mode)
                    .map_err(|e| e.to_string())?;
                Ok(format!("terrain set {inner} over {outer} added"))
            })
        },
    )
}

/// Change the match mode of a terrain set.
#[operator(
    id = "tileset.terrain_set.set",
    label = "Set Terrain Set Mode",
    description = "Change which peers a terrain set matches on.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        inner(String, doc = "Inner terrain id or name."),
        outer(String, doc = "Outer terrain id or name, or empty."),
        mode(String, doc = "corners, sides or corners_and_sides."),
    )
)]
pub fn tileset_terrain_set_set(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (inner, outer, mode) = (
        text(&params, "inner"),
        text(&params, "outer"),
        text(&params, "mode"),
    );
    queue_edit(
        &mut commands,
        "tileset.terrain_set.set",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (inner, outer) = (
                    terrain(tileset, &inner)?,
                    terrain_or_empty(tileset, &outer)?,
                );
                setup
                    .set_match_mode(inner, outer, match_mode(&mode)?)
                    .map_err(|e| e.to_string())?;
                Ok(format!("terrain set {inner} over {outer} matches {mode}"))
            })
        },
    )
}

/// Set a tile's terrain bits by hand.
#[operator(
    id = "tileset.tile.set_bits",
    label = "Set Tile Bits",
    description = "Set the terrain of a tile's nine peers, or of one of them.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tile(String, doc = "Packed layer or sheet:tile."),
        peers(
            String,
            doc = "Nine comma-separated terrain ids or names, row by row from the top \
                             left; an empty entry is no terrain."
        ),
        peer(
            String,
            doc = "One peer instead: 0 to 8 or top_left, top, ..., bottom_right."
        ),
        terrain(String, doc = "Terrain of that one peer; empty is no terrain."),
    )
)]
pub fn tileset_tile_set_bits(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (at, peers) = (
        text(&params, "tile"),
        params.as_str("peers").map(str::to_owned),
    );
    let (one, on) = (text(&params, "peer"), text(&params, "terrain"));
    queue_edit(
        &mut commands,
        "tileset.tile.set_bits",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (sheet, index) = tile(tileset, &at)?;
                let address = tileset
                    .setup_address(sheet, index)
                    .map_err(|e| e.to_string())?;
                match peers {
                    Some(peers) if !peers.trim().is_empty() => {
                        let ids: Vec<TerrainId> = (peers
                            .split(',')
                            .map(|p| terrain_or_empty(tileset, p.trim())))
                        .collect::<Result<_, _>>()?;
                        let count = ids.len();
                        let bits = TileTerrains {
                            peers: ids
                                .try_into()
                                .map_err(|_| format!("peers lists 9 terrains, not {count}"))?,
                        };
                        setup.set_peers(address, bits).map_err(|e| e.to_string())?;
                    }
                    _ => {
                        let (peer, id) = (peer(&one)?, terrain_or_empty(tileset, &on)?);
                        setup
                            .set_peer_by_hand(address, peer, id)
                            .map_err(|e| e.to_string())?;
                    }
                }
                let bits = setup
                    .tiles
                    .get(&address)
                    .map(|t| t.terrains)
                    .unwrap_or_default();
                let shown: Vec<String> = bits.peers.iter().map(u8::to_string).collect();
                Ok(format!(
                    "tile {index} of sheet {sheet} is {}",
                    shown.join(",")
                ))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_keywords_cannot_name_a_terrain() {
        for name in ["empty", "erase", "keep", "none", "3", ""] {
            assert!(free_name(&Tileset::default(), name).is_err(), "{name}");
        }
        assert!(free_name(&Tileset::default(), "grass").is_ok());
    }
}
