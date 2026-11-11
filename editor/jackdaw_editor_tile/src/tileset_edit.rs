use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_commands::CommandHistory;
use jackdaw_tilemap::{MatchMode, Peer, TerrainId, TileAddress, TileTerrains, TilesetSetup};
use jackdaw_tileset::{Tileset, TilesetTerrain};

use jackdaw_editor_core::asset_index::{AssetValue, absolute_path, note_written};
use jackdaw_editor_core::commands::EditorCommand;
use jackdaw_editor_core::definition_assets::{
    entry_for_file, open_definition_path, rebuild_open_card, resolve_project_path, write_asset_file,
};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilesetTerrainAddOp>();
    ctx.register_operator::<TilesetTerrainRemoveOp>();
    ctx.register_operator::<TilesetTerrainRenameOp>();
    ctx.register_operator::<TilesetTerrainSetIconOp>();
    ctx.register_operator::<TilesetTerrainSetColorOp>();
    ctx.register_operator::<TilesetTerrainSplitOp>();
    ctx.register_operator::<TilesetTerrainMergeOp>();
    ctx.register_operator::<TilesetTerrainSetAddOp>();
    ctx.register_operator::<TilesetTerrainSetSetOp>();
    ctx.register_operator::<TilesetTerrainSetRemoveOp>();
    ctx.register_operator::<TilesetTerrainSetRenameOp>();
    ctx.register_operator::<TilesetTerrainSetAddTerrainOp>();
    ctx.register_operator::<TilesetTerrainSetMoveTerrainOp>();
    ctx.register_operator::<TilesetTileSetBitsOp>();
    ctx.register_operator::<TilesetClearAutoOp>();
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
        let open =
            (open_definition_path(world).map(|open| absolute_path(world, &open))).filter(|file| {
                entry_for_file(world, file).is_some_and(|e| e.type_path == Tileset::type_path())
            });
        let focused = || {
            let path = crate::tileset_dock::focused_tileset(world)?;
            Some(resolve_project_path(world, Path::new(&path)))
        };
        let front = || {
            let path = crate::tileset_dock::front_tileset(world)?;
            Some(resolve_project_path(world, Path::new(&path)))
        };
        (open.or_else(focused).or_else(front)).ok_or("no path given and no tileset is open")?
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

/// The tile `text` names as an icon for terrain `owner`, refused when another
/// terrain already shows it; none for an empty `text`.
fn free_icon(
    tileset: &Tileset,
    text: &str,
    owner: TerrainId,
) -> Result<Option<(u32, u32)>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    let icon = tile(tileset, text)?;
    let taken = (tileset.terrains.iter()).find(|t| t.id != owner && t.icon == Some(icon));
    if let Some(other) = taken {
        let (sheet, index) = icon;
        return Err(format!(
            "tile {sheet}:{index} is already the icon of `{}`",
            other.name
        ));
    }
    Ok(Some(icon))
}

/// The first free `Terrain n`, counting on from the terrains there are.
pub(crate) fn suggested_name(tileset: &Tileset) -> String {
    (tileset.terrains.len() + 1..)
        .map(|n| format!("Terrain {n}"))
        .find(|name| free_name(tileset, name).is_ok())
        .unwrap_or_default()
}

fn free_name(tileset: &Tileset, name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.parse::<u32>().is_ok()
        || name.chars().any(|c| c == '"' || c.is_control())
        || ["empty", "erase", "keep", "none"].contains(&name)
    {
        return Err(format!("`{name}` cannot name a terrain"));
    }
    if tileset.terrains.iter().any(|t| t.name == name) {
        return Err(format!("a terrain is already named `{name}`"));
    }
    Ok(())
}

/// A terrain set by index or name.
pub(crate) fn terrain_set(tileset: &Tileset, text: &str) -> Result<usize, String> {
    let sets = &tileset.terrain_sets;
    let found = (sets.iter().enumerate())
        .find(|(i, s)| text.parse::<usize>().is_ok_and(|n| n == *i) || s.name == text);
    found
        .map(|(i, _)| i)
        .ok_or_else(|| format!("no terrain set is `{text}`"))
}

/// `name` trimmed, refused when a terrain set cannot take it: a number, or
/// with a quote or a control character. Empty stands for the default name.
fn set_name_allowed(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.parse::<usize>().is_ok() || name.chars().any(|c| c == '"' || c.is_control()) {
        return Err(format!("`{name}` cannot name a terrain set"));
    }
    Ok(name.to_string())
}

/// The name of terrain set `set` of `setup`, quoted, for messages.
fn set_label(setup: &TilesetSetup, set: usize) -> String {
    setup
        .sets
        .get(set)
        .map_or_else(|| set.to_string(), |s| format!("\"{}\"", s.name))
}

/// A stacking position from an optional integer parameter.
fn position(params: &OperatorParameters, name: &str) -> Result<Option<usize>, String> {
    params
        .as_int(name)
        .map(|at| usize::try_from(at).map_err(|_| format!("{name} is 0 or more, not {at}")))
        .transpose()
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

pub(crate) fn peer(text: &str) -> Result<Peer, String> {
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
    writes_disk = true,
    label = "Add Tileset Terrain",
    description = "Add a terrain to a terrain set of a tileset; it takes the next free id, up to \
                   255. The first terrain of a tileset makes `Terrain set 1`, matching corners \
                   and sides.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        name(String, doc = "Name of the new terrain; unique, not a number."),
        set(
            String,
            doc = "Index or name of the set it joins, on top; defaults to the first set."
        ),
        swatch(
            String,
            doc = "Swatch color as #rrggbb; defaults to the next hue of a fixed palette no \
                   terrain has yet."
        ),
        background(bool, default = false, doc = "Whether it is the background terrain."),
        icon(
            String,
            doc = "Tile the terrain is shown by, as `sheet:tile` or a packed layer; none by \
                   default."
        ),
    )
)]
pub fn tileset_terrain_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = text(&params, "name");
    let set = text(&params, "set");
    let swatch = text(&params, "swatch");
    let background = params.as_bool("background").unwrap_or(false);
    let icon = text(&params, "icon");
    queue_edit(
        &mut commands,
        "tileset.terrain.add",
        text(&params, "path"),
        move |_, tileset| {
            free_name(tileset, &name)?;
            let icon = free_icon(tileset, &icon, 0)?;
            let [r, g, b] = if swatch.is_empty() {
                next_hue(tileset)
            } else {
                hex_color(&swatch)?
            };
            let index = if set.is_empty() {
                None
            } else {
                Some(terrain_set(tileset, &set)?)
            };
            let mut added = 0;
            let message = with_setup(tileset, |_, setup| {
                added = (setup.add_terrain(name.clone(), [r, g, b], background))
                    .map_err(|e| e.to_string())?;
                let index = match index {
                    Some(index) => index,
                    None if setup.sets.is_empty() => setup
                        .add_set("", MatchMode::CornersAndSides)
                        .map_err(|e| setup.explain(e))?,
                    None => 0,
                };
                setup
                    .add_set_terrain(index, added, None)
                    .map_err(|e| setup.explain(e))?;
                Ok(format!(
                    "terrain {added} \"{name}\" added to terrain set {}",
                    set_label(setup, index)
                ))
            })?;
            terrain_mut(tileset, added)?.icon = icon;
            Ok(message)
        },
    )
}

/// Remove a terrain from a tileset.
#[operator(
    id = "tileset.terrain.remove",
    writes_disk = true,
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

/// Show a terrain by a tile.
#[operator(
    id = "tileset.terrain.set_icon",
    writes_disk = true,
    label = "Set Tileset Terrain Icon",
    description = "Show a terrain by a tile of the sheet, which no other terrain shows.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        terrain(String, doc = "Terrain id or name."),
        icon(
            String,
            doc = "Tile as `sheet:tile` or a packed layer; empty shows the terrain by its \
                   color."
        ),
    )
)]
pub fn tileset_terrain_set_icon(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (name, icon) = (text(&params, "terrain"), text(&params, "icon"));
    queue_edit(
        &mut commands,
        "tileset.terrain.set_icon",
        text(&params, "path"),
        move |_, tileset| {
            let id = terrain(tileset, &name)?;
            let icon = free_icon(tileset, &icon, id)?;
            terrain_mut(tileset, id)?.icon = icon;
            Ok(match icon {
                Some((sheet, tile)) => format!("terrain {id} is shown by tile {sheet}:{tile}"),
                None => format!("terrain {id} is shown by its color"),
            })
        },
    )
}

/// Change the swatch color of a terrain.
#[operator(
    id = "tileset.terrain.set_color",
    writes_disk = true,
    label = "Set Tileset Terrain Color",
    description = "Change the color a terrain is shown in, on the sheet overlay and the map \
                   preview alike.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        terrain(String, doc = "Terrain id or name."),
        color(String, doc = "Color as #rrggbb."),
    )
)]
pub fn tileset_terrain_set_color(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (name, color) = (text(&params, "terrain"), text(&params, "color"));
    queue_edit(
        &mut commands,
        "tileset.terrain.set_color",
        text(&params, "path"),
        move |_, tileset| {
            let id = terrain(tileset, &name)?;
            let [r, g, b] = hex_color(&color)?;
            terrain_mut(tileset, id)?.swatch = Srgba::rgb_u8(r, g, b);
            Ok(format!("terrain {id} is {color}"))
        },
    )
}

/// Rename a terrain of a tileset.
#[operator(
    id = "tileset.terrain.rename",
    writes_disk = true,
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
            let renamed = terrain_mut(tileset, id)?;
            if renamed.name == name {
                return Ok(format!("terrain {id} is already \"{name}\""));
            }
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
    writes_disk = true,
    label = "Split Tileset Terrain",
    description = "Move the listed tiles' peers of a terrain to a new terrain like it, just \
                   above it in its terrain set.",
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
                if setup.set_of(new).is_none()
                    && let Some(home) = setup.set_of(id)
                {
                    let above = (setup.sets[home].terrains.iter()).position(|t| *t == id);
                    setup
                        .add_set_terrain(home, new, above.map(|at| at + 1))
                        .map_err(|e| setup.explain(e))?;
                }
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
    writes_disk = true,
    label = "Merge Tileset Terrains",
    description = "Fold one terrain into another: its peers take the other's id, and its terrain \
                   set too when the other has none.",
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
                    .map_err(|e| setup.explain(e))?;
                Ok(format!("terrain {from} merged into terrain {into}"))
            })
        },
    )
}

/// Add a terrain set to a tileset.
#[operator(
    id = "tileset.terrain_set.add",
    writes_disk = true,
    label = "Add Terrain Set",
    description = "Add an empty terrain set with a match mode; a Tileset tab adds its next \
                   terrain to it. Terrains move to it with tileset.terrain_set.add_terrain.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        mode(
            String,
            default = "corners_and_sides",
            doc = "corners, sides or corners_and_sides."
        ),
        name(
            String,
            doc = "Name of the new set; unique, not a number. Defaults to the next free \
                   `Terrain set n`."
        ),
    )
)]
pub fn tileset_terrain_set_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (mode, name) = (text(&params, "mode"), text(&params, "name"));
    let path = text(&params, "path");
    commands.queue(move |world: &mut World| {
        let mut added = String::new();
        let edit = |_: &World, tileset: &mut Tileset| {
            with_setup(tileset, |_, setup| {
                let mode = if mode.is_empty() {
                    MatchMode::CornersAndSides
                } else {
                    match_mode(&mode)?
                };
                let name = set_name_allowed(&name)?;
                let set = setup.add_set(&name, mode).map_err(|e| setup.explain(e))?;
                added.clone_from(&setup.sets[set].name);
                Ok(format!("terrain set {set} {} added", set_label(setup, set)))
            })
        };
        match apply_edit(world, "tileset.terrain_set.add", &path, edit) {
            Ok(message) => {
                crate::tileset_dock::make_active(world, &path, added);
                report_to_caller(world, message);
            }
            Err(err) => warn_caller(world, format!("tileset.terrain_set.add: {err}")),
        }
    });
    OperatorResult::Finished
}

/// Remove a terrain set from a tileset.
#[operator(
    id = "tileset.terrain_set.remove",
    writes_disk = true,
    label = "Remove Terrain Set",
    description = "Remove a terrain set and the terrains no other set holds: their peers become \
                   empty.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        set(String, doc = "Index or name of the set."),
    )
)]
pub fn tileset_terrain_set_remove(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let set = text(&params, "set");
    queue_edit(
        &mut commands,
        "tileset.terrain_set.remove",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let index = terrain_set(tileset, &set)?;
                let removed = setup.remove_set(index).map_err(|e| setup.explain(e))?;
                let owned: Vec<TerrainId> = (removed.terrains.iter().copied())
                    .filter(|id| setup.set_of(*id).is_none())
                    .collect();
                for id in &owned {
                    setup.remove_terrain(*id).map_err(|e| setup.explain(e))?;
                }
                Ok(format!(
                    "terrain set \"{}\" and the {} terrains only it held removed",
                    removed.name,
                    owned.len()
                ))
            })
        },
    )
}

/// Change the match mode of a terrain set.
#[operator(
    id = "tileset.terrain_set.set",
    writes_disk = true,
    label = "Set Terrain Set Mode",
    description = "Change which peers a terrain set matches on.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        set(String, doc = "Index or name of the set."),
        mode(String, doc = "corners, sides or corners_and_sides."),
    )
)]
pub fn tileset_terrain_set_set(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (set, mode) = (text(&params, "set"), text(&params, "mode"));
    queue_edit(
        &mut commands,
        "tileset.terrain_set.set",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let index = terrain_set(tileset, &set)?;
                setup
                    .set_match_mode(index, match_mode(&mode)?)
                    .map_err(|e| setup.explain(e))?;
                Ok(format!(
                    "terrain set {} matches {mode}",
                    set_label(setup, index)
                ))
            })
        },
    )
}

/// Rename a terrain set.
#[operator(
    id = "tileset.terrain_set.rename",
    writes_disk = true,
    label = "Rename Terrain Set",
    description = "Rename a terrain set; names are unique.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        set(String, doc = "Index or name of the set."),
        name(String, doc = "New name; unique, not a number, not empty."),
    )
)]
pub fn tileset_terrain_set_rename(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (set, name) = (text(&params, "set"), text(&params, "name"));
    queue_edit(
        &mut commands,
        "tileset.terrain_set.rename",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let index = terrain_set(tileset, &set)?;
                let name = set_name_allowed(&name)?;
                if name.is_empty() {
                    return Err("a terrain set needs a name".to_string());
                }
                setup
                    .rename_set(index, &name)
                    .map_err(|e| setup.explain(e))?;
                Ok(format!("terrain set {index} is now \"{name}\""))
            })
        },
    )
}

/// Put a terrain in a terrain set.
#[operator(
    id = "tileset.terrain_set.add_terrain",
    writes_disk = true,
    label = "Move Terrain to Set",
    description = "Move a terrain into a terrain set, at a stacking position, out of the set it \
                   was in; a terrain belongs to one set. Terrains higher in a set sit on top \
                   where they meet.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        set(String, doc = "Index or name of the set."),
        terrain(String, doc = "Terrain id or name."),
        at(
            i64,
            doc = "Stacking position, 0 at the bottom; on top when not given."
        ),
    )
)]
pub fn tileset_terrain_set_add_terrain(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (set, name) = (text(&params, "set"), text(&params, "terrain"));
    let at = position(&params, "at");
    queue_edit(
        &mut commands,
        "tileset.terrain_set.add_terrain",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (index, id) = (terrain_set(tileset, &set)?, terrain(tileset, &name)?);
                let at = at?;
                match setup.set_of(id) {
                    Some(home) if home == index => {
                        return Ok(format!(
                            "terrain {id} is already in terrain set {}",
                            set_label(setup, index)
                        ));
                    }
                    Some(home) => setup
                        .remove_set_terrain(home, id)
                        .map_err(|e| setup.explain(e))?,
                    None => {}
                }
                setup
                    .add_set_terrain(index, id, at)
                    .map_err(|e| setup.explain(e))?;
                Ok(format!(
                    "terrain {id} moved to terrain set {}",
                    set_label(setup, index)
                ))
            })
        },
    )
}

/// Move a terrain up or down its terrain set.
#[operator(
    id = "tileset.terrain_set.move_terrain",
    writes_disk = true,
    label = "Move Terrain in Set",
    description = "Move a terrain to another stacking position in its terrain set; terrains \
                   higher in a set sit on top where they meet.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        set(String, doc = "Index or name of the set."),
        terrain(String, doc = "Terrain id or name."),
        to(
            i64,
            doc = "Stacking position, 0 at the bottom; past the top moves it to the top."
        ),
    )
)]
pub fn tileset_terrain_set_move_terrain(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (set, name) = (text(&params, "set"), text(&params, "terrain"));
    let to = position(&params, "to");
    queue_edit(
        &mut commands,
        "tileset.terrain_set.move_terrain",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (index, id) = (terrain_set(tileset, &set)?, terrain(tileset, &name)?);
                let to = to?.ok_or("name the position with to")?;
                setup
                    .move_set_terrain(index, id, to)
                    .map_err(|e| setup.explain(e))?;
                let at = (setup.sets.get(index))
                    .and_then(|s| s.terrains.iter().position(|t| *t == id))
                    .unwrap_or(to);
                Ok(format!(
                    "terrain {id} is at {at} in terrain set {}",
                    set_label(setup, index)
                ))
            })
        },
    )
}

/// Set a tile's terrain bits by hand.
#[operator(
    id = "tileset.tile.set_bits",
    writes_disk = true,
    label = "Set Tile Bits",
    description = "Set the terrain of a tile's nine peers, or of one of them; a sheet outside \
                   terrain setup comes under it in the same undo entry.",
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
            let mut covered = tileset.clone();
            covered
                .cover_sheet(tile(tileset, &at)?.0)
                .map_err(|e| e.to_string())?;
            let message = with_setup(&mut covered, |tileset, setup| {
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
            })?;
            if covered.tiles != tileset.tiles {
                *tileset = covered;
            }
            Ok(message)
        },
    )
}

/// Clear what auto setup made in a tileset.
#[operator(
    id = "tileset.clear_auto",
    writes_disk = true,
    label = "Clear Automatic Setup",
    description = "Clear a tileset that holds automatic setup nobody has touched: its terrains, \
                   terrain sets, bits, collision shapes and objects go and the slicing stays. A \
                   tileset with any mark made by hand is left as it is.",
    allows_undo = false,
    params(path(
        String,
        doc = "Tileset file under the project's assets; defaults to the open tileset."
    ))
)]
pub fn tileset_clear_auto(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    queue_edit(
        &mut commands,
        "tileset.clear_auto",
        text(&params, "path"),
        |_, tileset| clear_auto(tileset),
    )
}

/// Clear what auto setup made in `tileset`, leaving it untouched when there
/// is nothing to clear.
pub(crate) fn clear_auto(tileset: &mut Tileset) -> Result<String, String> {
    if !tileset.untouched_auto() {
        return Ok("nothing was set up automatically".to_string());
    }
    tileset.clear_auto_setup();
    Ok("automatic setup cleared".to_string())
}

/// A color written as #rrggbb.
fn hex_color(text: &str) -> Result<[u8; 3], String> {
    let color = Srgba::hex(text).map_err(|_| format!("a color is #rrggbb, not `{text}`"))?;
    let [r, g, b, _] = color.to_u8_array();
    Ok([r, g, b])
}

/// The first marker color no terrain of `tileset` has, in order.
fn next_hue(tileset: &Tileset) -> [u8; 3] {
    let used: Vec<[u8; 3]> = (tileset.terrains.iter())
        .map(|t| {
            let [r, g, b, _] = t.swatch.to_u8_array();
            [r, g, b]
        })
        .collect();
    jackdaw_tilemap::next_marker_color(&used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_name_is_trimmed_before_it_is_checked() {
        assert!(set_name_allowed(" 1").is_err(), "a number once trimmed");
        assert_eq!(
            set_name_allowed("  "),
            Ok(String::new()),
            "the default name"
        );
        assert_eq!(set_name_allowed(" Paths "), Ok("Paths".to_string()));
        assert!(set_name_allowed("a\"b").is_err());
    }

    #[test]
    fn legend_keywords_cannot_name_a_terrain() {
        for name in ["empty", "erase", "keep", "none", "3", ""] {
            assert!(free_name(&Tileset::default(), name).is_err(), "{name}");
        }
        assert!(free_name(&Tileset::default(), "grass").is_ok());
    }
}
