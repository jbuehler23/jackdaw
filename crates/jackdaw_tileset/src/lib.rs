use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use bevy::color::Srgba;
use bevy::prelude::*;
use jackdaw_tilemap::{
    GridLayout, MatchMode, Provenance, SetupSet, SetupTerrain, SetupTile, ShapePreset, Stamp,
    TerrainId, TileAddress, TileCollision, TileTerrains, TilesetSetup, Tracked,
};

#[cfg(feature = "render")]
pub mod chunk;
mod describe;
mod terrain_set;

pub use terrain_set::TerrainSetError;

/// A map's authored bounds as the kernel's cell rectangle.
pub fn map_bounds(bounds: Option<IRect>) -> Option<jackdaw_tilemap::CellRect> {
    bounds.map(|b| jackdaw_tilemap::CellRect {
        min: b.min,
        max: b.max,
    })
}

/// Registers [`Tileset`] as a reflected asset.
pub struct TilesetPlugin;

impl Plugin for TilesetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<Tileset>()
            .register_asset_reflect::<Tileset>();
        #[cfg(feature = "render")]
        app.init_resource::<chunk::TileStrips>()
            .add_systems(Update, chunk::fill_strips);
    }
}

/// Tiles of one or more source images packed into one array image, one layer
/// per tile, with the terrains, terrain sets and objects authored over them.
#[derive(Asset, Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct Tileset {
    /// The packed image, as an asset path.
    pub image: String,
    /// Tile size in pixels.
    pub tile_size: UVec2,
    /// Columns and rows of tiles in the packed image.
    pub grid: UVec2,
    /// Source images, in import order.
    pub sheets: Vec<TilesetSheet>,
    /// The sheets terrain setup covers, as indices into `sheets`, in the
    /// order the setup lists them.
    pub setup_sheets: Vec<u32>,
    pub terrains: Vec<TilesetTerrain>,
    /// Every terrain sits in a set; one set by hand, see
    /// [`Tileset::set_of`].
    pub terrain_sets: Vec<TilesetTerrainSet>,
    /// Tiles with terrain bits or collision.
    pub tiles: Vec<TilesetTile>,
    /// Blocks of tiles placed as one sprite.
    pub objects: Vec<TilesetObject>,
    /// Id the next new terrain takes; ids are never reused, so this reaches
    /// 256 once every id is spent.
    pub next_terrain_id: u16,
    /// Named values per packed layer.
    pub properties: BTreeMap<u32, BTreeMap<String, String>>,
    /// Blocks of tiles saved from the Tile Palette.
    #[reflect(default)]
    pub stamps: Vec<TilesetStamp>,
    /// Set by an import that ran auto setup, and cleared with what it made.
    #[reflect(default)]
    pub auto_setup: bool,
    /// Transitions removed from terrain sets by hand, which a re-run of auto
    /// setup does not propose again.
    #[reflect(default)]
    pub removed_terrain_sets: Vec<[TerrainId; 2]>,
    /// Names of the physics layers tile collision is on; read through
    /// [`Tileset::physics_layers`], which names one layer when this is empty.
    #[reflect(default)]
    pub physics_layers: Vec<String>,
}

impl Default for Tileset {
    fn default() -> Self {
        Self {
            image: String::new(),
            tile_size: UVec2::ZERO,
            grid: UVec2::ZERO,
            sheets: Vec::new(),
            setup_sheets: Vec::new(),
            terrains: Vec::new(),
            terrain_sets: Vec::new(),
            tiles: Vec::new(),
            objects: Vec::new(),
            next_terrain_id: 1,
            properties: BTreeMap::new(),
            stamps: Vec::new(),
            auto_setup: false,
            removed_terrain_sets: Vec::new(),
            physics_layers: vec![DEFAULT_PHYSICS_LAYER.to_string()],
        }
    }
}

/// One source image and where its tiles sit in the packed image.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilesetSheet {
    /// Asset path of the source image; auto setup knows the sheet by it.
    pub source: String,
    pub margin: UVec2,
    pub spacing: UVec2,
    pub columns: u32,
    pub rows: u32,
    /// Packed layer of its tile 0; tile `i` is layer `first_layer + i`.
    pub first_layer: u32,
}

#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilesetTerrain {
    /// Never 0, which means no terrain.
    pub id: TerrainId,
    pub name: String,
    pub name_by_user: bool,
    /// Oklab color that matches it to the terrains of a later auto setup.
    pub oklab: Vec3,
    pub swatch: Srgba,
    pub background: bool,
    /// Sheet and tile the terrain is shown by.
    #[reflect(default)]
    pub icon: Option<(u32, u32)>,
}

/// Which of a tile's peers take part in matching.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum TerrainMatch {
    #[default]
    Corners,
    Sides,
    CornersAndSides,
}

/// A named terrain set: a match mode and the terrains it owns, whose
/// transition tiles it holds.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilesetTerrainSet {
    #[reflect(default)]
    pub name: String,
    pub mode: TerrainMatch,
    /// Stacking order, bottom first.
    #[reflect(default)]
    pub terrains: Vec<TerrainId>,
    pub mode_by_user: bool,
    /// Auto setup never changes the set or the tiles it holds.
    pub locked_manual: bool,
}

#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilesetTile {
    /// Index into [`Tileset::sheets`].
    pub sheet: u32,
    /// Index in its sheet, row by row.
    pub tile: u32,
    /// Terrain on each peer, row by row from the top left; 0 is none.
    pub peers: [TerrainId; 9],
    pub peers_by_user: [bool; 9],
    /// At most one shape per physics layer, by layer.
    #[reflect(default)]
    pub collision: Vec<TilesetCollision>,
    #[reflect(default)]
    pub collision_by_user: bool,
    /// The terrain drawn by the transition the tile was laid out for, 0 for
    /// none; see [`jackdaw_tilemap::SetupTile::set`].
    pub set_inner: TerrainId,
    /// The terrain that transition draws on.
    pub set_outer: TerrainId,
}

/// Collision shapes by packed layer, built once by
/// [`Tileset::collision_lookup`] for reading per map cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionLookup(HashMap<u32, Vec<TilesetCollision>>);

impl CollisionLookup {
    /// The shapes of packed layer `layer`, the tile a map cell draws, by
    /// physics layer.
    pub fn get(&self, layer: u32) -> &[TilesetCollision] {
        self.0.get(&layer).map_or(&[], Vec::as_slice)
    }
}

/// The name the one physics layer of a new tileset takes.
pub const DEFAULT_PHYSICS_LAYER: &str = "World";

/// A tile's collision shape on one physics layer.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilesetCollision {
    /// Index into [`Tileset::physics_layers`].
    pub layer: u32,
    /// Overlaps without blocking.
    pub trigger: bool,
    /// Blocks only what comes down onto it from above.
    #[reflect(default)]
    pub one_way: bool,
    pub preset: CollisionPreset,
    /// Counter-clockwise outer rings in tile fractions, y up, from the tile's
    /// bottom-left.
    pub polygons: Vec<Vec<Vec2>>,
}

/// What a collision shape was made from.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[reflect(Default)]
pub enum CollisionPreset {
    /// The whole tile.
    #[default]
    Solid,
    TopHalf,
    BottomHalf,
    /// A slope rising to the right.
    SlopeUp,
    /// A slope falling to the right.
    SlopeDown,
    /// The tile's opaque pixels.
    Traced,
    /// Polygons drawn point by point.
    Custom,
}

impl CollisionPreset {
    pub const ALL: [CollisionPreset; 7] = [
        CollisionPreset::Solid,
        CollisionPreset::TopHalf,
        CollisionPreset::BottomHalf,
        CollisionPreset::SlopeUp,
        CollisionPreset::SlopeDown,
        CollisionPreset::Traced,
        CollisionPreset::Custom,
    ];

    /// The name operators and text take, such as `top_half`.
    pub fn name(self) -> &'static str {
        ShapePreset::from(self).name()
    }

    pub fn from_name(name: &str) -> Option<Self> {
        ShapePreset::from_name(name).map(Self::from)
    }

    /// The preset's polygons; `None` for a traced or custom shape, which
    /// depends on the art or the hand.
    pub fn polygons(self) -> Option<Vec<Vec<Vec2>>> {
        ShapePreset::from(self).polygons()
    }

    /// The preset that draws `polygons`; custom for any other shape.
    pub fn of_polygons(polygons: &[Vec<Vec2>]) -> Self {
        ShapePreset::of_polygons(polygons).into()
    }
}

/// Tiles of one sheet placed together as one sprite.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct TilesetObject {
    /// Index into [`Tileset::sheets`].
    pub sheet: u32,
    /// Column and row of the top-left tile of its bounds.
    pub origin: UVec2,
    /// Bounds in tiles.
    pub size: UVec2,
    /// Bounds cells holding a tile; the others are gaps.
    pub cells: Vec<ObjectCell>,
    /// Point the sprite is placed by, as a fraction of the bounds from their
    /// bottom-left, y up.
    pub pivot: Vec2,
    pub by_user: bool,
}

impl TilesetObject {
    pub const BOTTOM_CENTRE: Vec2 = Vec2::new(0.5, 0.0);
}

/// A named block of tiles saved for the Brush to place as one.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilesetStamp {
    pub name: String,
    /// Columns and rows.
    pub size: UVec2,
    pub cells: Vec<StampTile>,
}

/// One tile of a [`TilesetStamp`].
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub struct StampTile {
    /// Column and row from the stamp's top-left, rows downward.
    pub offset: UVec2,
    /// Packed layer.
    pub layer: u32,
    /// Orientation bits of a `jackdaw_tilemap::TileRef`.
    pub orientation: u8,
}

impl Default for TilesetObject {
    fn default() -> Self {
        Self {
            sheet: 0,
            origin: UVec2::ZERO,
            size: UVec2::ZERO,
            cells: Vec::new(),
            pivot: Self::BOTTOM_CENTRE,
            by_user: false,
        }
    }
}

#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub struct ObjectCell {
    /// From the object's origin.
    pub offset: UVec2,
    /// Index in the object's sheet.
    pub tile: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TilesetError {
    /// A setup sheet that names no source sheet of the tileset.
    UnknownSheet(String),
    /// An index into [`Tileset::sheets`] past its end.
    NoSheet(u32),
    /// A tile or object on a sheet that terrain setup does not cover.
    OutsideSetup(u32),
    /// A tile past the end of its sheet.
    NoTile { sheet: u32, tile: u32 },
}

impl fmt::Display for TilesetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TilesetError::UnknownSheet(name) => write!(f, "no sheet of the tileset is {name}"),
            TilesetError::NoSheet(index) => write!(f, "the tileset has no sheet {index}"),
            TilesetError::OutsideSetup(index) => {
                write!(f, "sheet {index} is not covered by terrain setup")
            }
            TilesetError::NoTile { sheet, tile } => {
                write!(f, "sheet {sheet} has no tile {tile}")
            }
        }
    }
}

impl std::error::Error for TilesetError {}

impl Tileset {
    /// Whether this is auto setup's output with no mark made by hand: an
    /// import ran auto setup, and no terrain, set, tile, object, property or
    /// stamp has been set by the user since.
    pub fn untouched_auto(&self) -> bool {
        let terrains = (self.terrains.iter()).all(|t| !t.name_by_user);
        let sets = (self.terrain_sets.iter()).all(|s| !s.mode_by_user && !s.locked_manual);
        let tiles =
            (self.tiles.iter()).all(|t| !t.collision_by_user && !t.peers_by_user.contains(&true));
        let objects = self.objects.iter().all(|o| !o.by_user);
        self.auto_setup
            && terrains
            && sets
            && tiles
            && objects
            && self.properties.is_empty()
            && self.stamps.is_empty()
    }

    /// Drops what auto setup made, keeping the slicing and the spent ids.
    pub fn clear_auto_setup(&mut self) {
        self.terrains.clear();
        self.terrain_sets.clear();
        self.tiles.clear();
        self.objects.clear();
        self.auto_setup = false;
    }

    /// Index of the set terrain `id` belongs to: the first listing it, since
    /// auto setup may list a terrain in sets of two match modes.
    pub fn set_of(&self, id: TerrainId) -> Option<usize> {
        (self.terrain_sets.iter()).position(|s| id != 0 && s.terrains.contains(&id))
    }

    /// The name of terrain `id`, or the id itself when no terrain has it.
    pub fn terrain_name(&self, id: TerrainId) -> String {
        (self.terrains.iter())
            .find(|t| t.id == id)
            .map_or_else(|| id.to_string(), |t| t.name.clone())
    }

    /// The names of the physics layers, by index; one default layer when
    /// none are saved.
    pub fn physics_layer_names(&self) -> Cow<'_, [String]> {
        if self.physics_layers.is_empty() {
            Cow::Owned(vec![DEFAULT_PHYSICS_LAYER.to_string()])
        } else {
            Cow::Borrowed(&self.physics_layers)
        }
    }

    /// The physics layer names to edit, the default layer filled in when
    /// none are saved.
    pub fn physics_layers_mut(&mut self) -> &mut Vec<String> {
        if self.physics_layers.is_empty() {
            self.physics_layers.push(DEFAULT_PHYSICS_LAYER.to_string());
        }
        &mut self.physics_layers
    }

    /// The collision shapes of tile `tile` of sheet `sheet`, by physics
    /// layer. Each call searches the tiles; [`Tileset::collision_lookup`]
    /// reads many.
    pub fn collision(&self, sheet: u32, tile: u32) -> &[TilesetCollision] {
        (self.tiles.iter())
            .find(|t| t.sheet == sheet && t.tile == tile)
            .map_or(&[], |t| t.collision.as_slice())
    }

    /// The collision shapes of every tile, by packed layer.
    pub fn collision_lookup(&self) -> CollisionLookup {
        let shapes = (self.tiles.iter())
            .filter(|t| !t.collision.is_empty())
            .filter_map(|t| Some((self.layer(t.sheet, t.tile)?, t.collision.clone())))
            .collect();
        CollisionLookup(shapes)
    }

    /// Puts `shape` on tile `tile` of sheet `sheet` by hand in place of its
    /// shape on the same physics layer; says whether anything changed. Any
    /// sheet takes collision, under terrain setup or not.
    pub fn set_collision(
        &mut self,
        sheet: u32,
        tile: u32,
        shape: TilesetCollision,
    ) -> Result<bool, TilesetError> {
        self.layer(sheet, tile)
            .ok_or(TilesetError::NoTile { sheet, tile })?;
        let at = match (self.tiles.iter()).position(|t| t.sheet == sheet && t.tile == tile) {
            Some(at) => at,
            None => {
                self.tiles.push(TilesetTile {
                    sheet,
                    tile,
                    ..default()
                });
                self.tiles.len() - 1
            }
        };
        let entry = &mut self.tiles[at];
        if entry.collision.contains(&shape) {
            return Ok(false);
        }
        entry.collision.retain(|s| s.layer != shape.layer);
        let slot = entry.collision.partition_point(|s| s.layer < shape.layer);
        entry.collision.insert(slot, shape);
        entry.collision_by_user = true;
        Ok(true)
    }

    /// Takes the shape on physics layer `layer` off tile `tile` of sheet
    /// `sheet` by hand; says whether it had one. A tile left holding nothing
    /// is dropped.
    pub fn clear_collision(
        &mut self,
        sheet: u32,
        tile: u32,
        layer: u32,
    ) -> Result<bool, TilesetError> {
        self.layer(sheet, tile)
            .ok_or(TilesetError::NoTile { sheet, tile })?;
        let Some(at) = (self.tiles.iter()).position(|t| t.sheet == sheet && t.tile == tile) else {
            return Ok(false);
        };
        let entry = &mut self.tiles[at];
        let before = entry.collision.len();
        entry.collision.retain(|s| s.layer != layer);
        if entry.collision.len() == before {
            return Ok(false);
        }
        entry.collision_by_user = true;
        let blank = entry.collision.is_empty()
            && entry.peers == [0; 9]
            && !entry.peers_by_user.contains(&true)
            && entry.set_inner == 0;
        if blank {
            self.tiles.remove(at);
        }
        Ok(true)
    }

    /// Packed layer of tile `tile` of sheet `sheet`.
    pub fn layer(&self, sheet: u32, tile: u32) -> Option<u32> {
        let sheet = self.sheets.get(sheet as usize)?;
        (tile < sheet.columns * sheet.rows).then_some(sheet.first_layer + tile)
    }

    /// Pixel origin and size of tile `tile` in the source image of sheet
    /// `sheet`.
    pub fn sheet_rect(&self, sheet: u32, tile: u32) -> Option<(UVec2, UVec2)> {
        Some(self.sheet_layout(sheet)?.tile_rect(tile))
    }

    /// The tile and the zone `mode` matches on under pixel `at` of sheet
    /// `sheet`.
    pub fn zone_at(
        &self,
        sheet: u32,
        at: Vec2,
        mode: MatchMode,
    ) -> Option<(u32, jackdaw_tilemap::Peer)> {
        let (tile, inside) = self.sheet_layout(sheet)?.tile_at(at)?;
        Some((tile, jackdaw_tilemap::Peer::under(inside, mode)))
    }

    pub fn sheet_layout(&self, sheet: u32) -> Option<GridLayout> {
        let sheet = self.sheets.get(sheet as usize)?;
        Some(GridLayout {
            tile: self.tile_size,
            margin: sheet.margin,
            spacing: sheet.spacing,
            columns: sheet.columns,
            rows: sheet.rows,
        })
    }

    /// Sheet and tile of packed layer `layer`.
    pub fn tile_of_layer(&self, layer: u32) -> Option<(u32, u32)> {
        (self.sheets.iter().zip(0..)).find_map(|(sheet, index)| {
            let tile = layer.checked_sub(sheet.first_layer)?;
            (tile < sheet.columns * sheet.rows).then_some((index, tile))
        })
    }

    /// Index of sheet `sheet` in the terrain setup.
    pub fn setup_sheet(&self, sheet: u32) -> Result<usize, TilesetError> {
        if sheet as usize >= self.sheets.len() {
            return Err(TilesetError::NoSheet(sheet));
        }
        (self.setup_sheets.iter().position(|&s| s == sheet))
            .ok_or(TilesetError::OutsideSetup(sheet))
    }

    /// The setup address of tile `tile` of sheet `sheet`.
    pub fn setup_address(&self, sheet: u32, tile: u32) -> Result<TileAddress, TilesetError> {
        self.layer(sheet, tile)
            .ok_or(TilesetError::NoTile { sheet, tile })?;
        Ok(TileAddress {
            sheet: self.setup_sheet(sheet)?,
            tile,
        })
    }

    /// The terrain setup the tileset holds, for the kernel to edit or re-run.
    /// Tiles of sheets outside setup, which hold only collision, are left
    /// out.
    pub fn setup(&self) -> Result<TilesetSetup, TilesetError> {
        let sheets = (self.setup_sheets.iter())
            .map(|&index| {
                let sheet = self.sheets.get(index as usize);
                sheet
                    .map(|s| s.source.clone())
                    .ok_or(TilesetError::NoSheet(index))
            })
            .collect::<Result<_, _>>()?;
        let position = |sheet: u32| {
            (self.setup_sheets.iter().position(|&s| s == sheet))
                .ok_or(TilesetError::OutsideSetup(sheet))
        };
        let mut tiles = BTreeMap::new();
        for tile in &self.tiles {
            let Ok(sheet) = position(tile.sheet) else {
                continue;
            };
            let address = TileAddress {
                sheet,
                tile: tile.tile,
            };
            tiles.insert(address, setup_tile(tile));
        }
        let mut stamps = Vec::with_capacity(self.objects.len());
        for object in &self.objects {
            let stamp = Stamp {
                sheet: position(object.sheet)?,
                origin: object.origin,
                size: object.size,
                cells: object.cells.iter().map(|c| (c.offset, c.tile)).collect(),
            };
            stamps.push(tracked(stamp, object.by_user));
        }
        Ok(TilesetSetup {
            sheets,
            terrains: self.terrains.iter().map(setup_terrain).collect(),
            sets: self.terrain_sets.iter().map(setup_set).collect(),
            tiles,
            stamps,
            next_id: self.next_terrain_id,
            removed_sets: (self.removed_terrain_sets.iter())
                .map(|&[inner, outer]| (inner, outer))
                .collect(),
        })
    }

    /// Takes the terrain setup `setup` in place of the tileset's. Objects keep
    /// their pivots where a stamp has the same bounds; new ones pivot at the
    /// bottom center. Tiles of sheets outside the new setup are kept. A
    /// terrain no set lists joins the first set, or a new one matching
    /// corners and sides when there is none.
    pub fn set_setup(&mut self, setup: &TilesetSetup) -> Result<(), TilesetError> {
        let setup_sheets = (setup.sheets.iter())
            .map(|name| {
                (self.sheets.iter().position(|s| s.source == *name))
                    .map(|index| index as u32)
                    .ok_or_else(|| TilesetError::UnknownSheet(name.clone()))
            })
            .collect::<Result<Vec<u32>, _>>()?;
        let source = |sheet: usize| {
            let index = setup_sheets.get(sheet).copied();
            index.ok_or(TilesetError::NoSheet(sheet as u32))
        };
        let mut tiles = Vec::with_capacity(setup.tiles.len());
        for (address, tile) in &setup.tiles {
            tiles.push(tileset_tile(source(address.sheet)?, address.tile, tile));
        }
        let outside = (self.tiles.iter()).filter(|t| !setup_sheets.contains(&t.sheet));
        tiles.extend(outside.cloned());
        let mut objects = Vec::with_capacity(setup.stamps.len());
        for stamp in &setup.stamps {
            let sheet = source(stamp.value.sheet)?;
            let pivot = (self.objects.iter())
                .find(|o| {
                    o.sheet == sheet && o.origin == stamp.value.origin && o.size == stamp.value.size
                })
                .map_or(TilesetObject::BOTTOM_CENTRE, |o| o.pivot);
            objects.push(TilesetObject {
                sheet,
                origin: stamp.value.origin,
                size: stamp.value.size,
                cells: (stamp.value.cells.iter())
                    .map(|&(offset, tile)| ObjectCell { offset, tile })
                    .collect(),
                pivot,
                by_user: stamp.is_user(),
            });
        }
        self.setup_sheets = setup_sheets;
        let icons: BTreeMap<TerrainId, (u32, u32)> = (self.terrains.iter())
            .filter_map(|t| t.icon.map(|icon| (t.id, icon)))
            .collect();
        self.terrains = (setup.terrains.iter())
            .map(|t| TilesetTerrain {
                icon: icons.get(&t.id).copied(),
                ..tileset_terrain(t)
            })
            .collect();
        self.terrain_sets = setup.sets.iter().map(tileset_set).collect();
        self.home_terrains();
        self.removed_terrain_sets = (setup.removed_sets.iter())
            .map(|&(inner, outer)| [inner, outer])
            .collect();
        self.tiles = tiles;
        self.objects = objects;
        self.next_terrain_id = setup.next_id;
        Ok(())
    }
}

impl Tileset {
    /// Puts each terrain no set lists in a set; see [`Tileset::set_setup`].
    fn home_terrains(&mut self) {
        let homeless: Vec<TerrainId> = (self.terrains.iter())
            .map(|t| t.id)
            .filter(|id| self.set_of(*id).is_none())
            .collect();
        if homeless.is_empty() {
            return;
        }
        if self.terrain_sets.is_empty() {
            self.terrain_sets.push(TilesetTerrainSet {
                name: jackdaw_tilemap::set_name(1),
                mode: TerrainMatch::CornersAndSides,
                ..default()
            });
        }
        if let Some(first) = self.terrain_sets.first_mut() {
            first.terrains.extend(homeless);
        }
    }
}

fn by_user(provenance: Provenance) -> bool {
    provenance == Provenance::User
}

fn tracked<T>(value: T, by_user: bool) -> Tracked<T> {
    if by_user {
        Tracked::user(value)
    } else {
        Tracked::auto(value)
    }
}

fn setup_terrain(terrain: &TilesetTerrain) -> SetupTerrain {
    let [r, g, b, _] = terrain.swatch.to_u8_array();
    SetupTerrain {
        id: terrain.id,
        name: tracked(terrain.name.clone(), terrain.name_by_user),
        oklab: terrain.oklab,
        swatch: [r, g, b],
        background: terrain.background,
    }
}

fn tileset_terrain(terrain: &SetupTerrain) -> TilesetTerrain {
    let [r, g, b] = terrain.swatch;
    TilesetTerrain {
        id: terrain.id,
        name: terrain.name.value.clone(),
        name_by_user: terrain.name.is_user(),
        oklab: terrain.oklab,
        swatch: Srgba::rgb_u8(r, g, b),
        background: terrain.background,
        icon: None,
    }
}

fn setup_set(set: &TilesetTerrainSet) -> SetupSet {
    SetupSet {
        name: set.name.clone(),
        mode: tracked(match_mode(set.mode), set.mode_by_user),
        terrains: set.terrains.clone(),
        locked_manual: set.locked_manual,
    }
}

fn tileset_set(set: &SetupSet) -> TilesetTerrainSet {
    TilesetTerrainSet {
        name: set.name.clone(),
        mode: terrain_match(set.mode.value),
        terrains: set.terrains.clone(),
        mode_by_user: set.mode.is_user(),
        locked_manual: set.locked_manual,
    }
}

fn setup_tile(tile: &TilesetTile) -> SetupTile {
    let collision = (tile.collision.iter())
        .map(|c| TileCollision {
            layer: c.layer,
            trigger: c.trigger,
            one_way: c.one_way,
            preset: c.preset.into(),
            polygons: c.polygons.clone(),
        })
        .collect();
    SetupTile {
        terrains: TileTerrains { peers: tile.peers },
        provenance: tile.peers_by_user.map(|user| {
            if user {
                Provenance::User
            } else {
                Provenance::Auto
            }
        }),
        collision: tracked(collision, tile.collision_by_user),
        set: (tile.set_inner != 0).then_some((tile.set_inner, tile.set_outer)),
    }
}

fn tileset_tile(sheet: u32, tile: u32, from: &SetupTile) -> TilesetTile {
    let collision = (from.collision.value.iter())
        .map(|c| TilesetCollision {
            layer: c.layer,
            trigger: c.trigger,
            one_way: c.one_way,
            preset: c.preset.into(),
            polygons: c.polygons.clone(),
        })
        .collect();
    TilesetTile {
        sheet,
        tile,
        peers: from.terrains.peers,
        peers_by_user: from.provenance.map(by_user),
        collision,
        collision_by_user: from.collision.is_user(),
        set_inner: from.set.map_or(0, |(inner, _)| inner),
        set_outer: from.set.map_or(0, |(_, outer)| outer),
    }
}

impl From<CollisionPreset> for ShapePreset {
    fn from(preset: CollisionPreset) -> Self {
        match preset {
            CollisionPreset::Solid => ShapePreset::Solid,
            CollisionPreset::TopHalf => ShapePreset::TopHalf,
            CollisionPreset::BottomHalf => ShapePreset::BottomHalf,
            CollisionPreset::SlopeUp => ShapePreset::SlopeUp,
            CollisionPreset::SlopeDown => ShapePreset::SlopeDown,
            CollisionPreset::Traced => ShapePreset::Traced,
            CollisionPreset::Custom => ShapePreset::Custom,
        }
    }
}

impl From<ShapePreset> for CollisionPreset {
    fn from(preset: ShapePreset) -> Self {
        match preset {
            ShapePreset::Solid => CollisionPreset::Solid,
            ShapePreset::TopHalf => CollisionPreset::TopHalf,
            ShapePreset::BottomHalf => CollisionPreset::BottomHalf,
            ShapePreset::SlopeUp => CollisionPreset::SlopeUp,
            ShapePreset::SlopeDown => CollisionPreset::SlopeDown,
            ShapePreset::Traced => CollisionPreset::Traced,
            ShapePreset::Custom => CollisionPreset::Custom,
        }
    }
}

fn match_mode(mode: TerrainMatch) -> MatchMode {
    match mode {
        TerrainMatch::Corners => MatchMode::Corners,
        TerrainMatch::Sides => MatchMode::Sides,
        TerrainMatch::CornersAndSides => MatchMode::CornersAndSides,
    }
}

fn terrain_match(mode: MatchMode) -> TerrainMatch {
    match mode {
        MatchMode::Corners => TerrainMatch::Corners,
        MatchMode::Sides => TerrainMatch::Sides,
        MatchMode::CornersAndSides => TerrainMatch::CornersAndSides,
    }
}
