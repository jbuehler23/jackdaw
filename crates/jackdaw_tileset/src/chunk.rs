//! A tilemap layer as Bevy tilemap chunks: one 32 x 32 block of cells per
//! chunk, laid out the same way wherever a map is drawn. A layer whose tiles
//! sit between cells draws half a cell up and right, and on a bounded map
//! the ring of its tiles the bounds cut through draws as cropped sprites,
//! children of their chunk, from a 2D strip of the packed image's layers.

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension};
use bevy::sprite_render::{
    AlphaMode2d, TileData, TileOrientation, TilemapChunk, TilemapChunkTileData,
};
use jackdaw_tilemap::{CHUNK_AREA, CHUNK_SIZE, CellRect, ChunkCoord, Placement, TileRef};

use crate::Tileset;

/// One layer's 32 x 32 block of the tilemap it is a child of, as drawn.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilemapChunkOf {
    /// Index into the map's layers.
    pub layer: usize,
    pub chunk: ChunkCoord,
}

/// The part of a tile cut by its map's bounds, drawn as a sprite child of
/// its chunk.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilemapEdgePiece;

/// Where a layer's tiles sit on its map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileLayout {
    pub placement: Placement,
    /// The map's bounds, which tiles between cells are cropped to.
    pub bounds: Option<CellRect>,
}

impl TileLayout {
    /// Whether some tiles are drawn cropped, and so need the source sheets.
    pub fn crops(&self) -> bool {
        self.placement == Placement::BetweenCells && self.bounds.is_some()
    }

    fn whole(&self, key: jackdaw_tilemap::CellCoord) -> bool {
        self.placement.visible_part(key, self.bounds) == Some((Vec2::ZERO, Vec2::ONE))
    }
}

/// The tile a cell's [`TileRef`] draws, or `None` for an empty cell.
///
/// Both orient the tile image the same way: transpose across its top-left
/// diagonal, then mirror, so the bits map one to one.
pub fn tile_data(tile: TileRef) -> Option<TileData> {
    (!tile.is_empty()).then(|| TileData {
        tileset_index: tile.index(),
        visible: tile.visible(),
        orientation: TileOrientation::from_bools(tile.flip_x(), tile.flip_y(), tile.transpose()),
        ..default()
    })
}

/// The z of layer `layer` under its map. Layers stack one whole unit apart,
/// so y-sorted entities, which move less than half a unit, never cross from
/// one layer to the next.
pub fn layer_z(layer: usize) -> f32 {
    layer as f32
}

/// Where the chunk at `coord` of layer `layer` sits under its map: the chunk
/// mesh is centred on its entity, and cell (0,0) has its min corner at the
/// map's origin, or the tile keyed at it its min corner at that cell's
/// centre when tiles sit between cells.
pub fn chunk_transform(
    coord: ChunkCoord,
    tile_size: UVec2,
    layer: usize,
    placement: Placement,
) -> Transform {
    let centre = (coord.0 * CHUNK_SIZE).as_vec2() + CHUNK_SIZE as f32 / 2.0 + placement.offset();
    Transform::from_translation((centre * tile_size.as_vec2()).extend(layer_z(layer)))
}

/// What a layer's chunks need from its tileset.
#[derive(Clone, Debug)]
pub struct ChunkDraw {
    /// The packed array image.
    pub image: Handle<Image>,
    /// The packed layers the tileset has a tile in; a cell naming another
    /// draws nothing.
    pub known: HashSet<u16>,
    /// The packed image as a 2D strip of its layers, which cropped tiles
    /// draw from; see [`TileStrips`].
    pub strip: Option<Handle<Image>>,
    /// Pixel size of one packed layer.
    pixels: UVec2,
}

impl ChunkDraw {
    pub fn new(tileset: &Tileset, image: Handle<Image>) -> Self {
        let known = (tileset.sheets.iter())
            .flat_map(|sheet| sheet.first_layer..sheet.first_layer + sheet.columns * sheet.rows)
            .filter_map(|layer| u16::try_from(layer).ok())
            .collect();
        Self {
            image,
            known,
            strip: None,
            pixels: tileset.tile_size,
        }
    }

    /// The chunk component for tiles drawn `tile_size` world units wide.
    pub fn chunk(&self, tile_size: UVec2) -> TilemapChunk {
        TilemapChunk {
            chunk_size: UVec2::splat(CHUNK_SIZE as u32),
            tile_display_size: tile_size,
            tileset: self.image.clone(),
            alpha_mode: AlphaMode2d::Blend,
        }
    }

    /// The 1024 drawn tiles of one chunk's cells, leaving out the tiles
    /// `layout` crops.
    pub fn tiles(
        &self,
        coord: ChunkCoord,
        cells: &[TileRef; CHUNK_AREA],
        layout: TileLayout,
    ) -> TilemapChunkTileData {
        TilemapChunkTileData(
            (cells.iter().enumerate())
                .map(|(local, &tile)| {
                    let shown =
                        self.known.contains(&tile.index()) && layout.whole(coord.cell(local));
                    tile_data(tile).filter(|_| shown)
                })
                .collect(),
        )
    }

    /// Everything the chunk at `coord` of layer `layer` spawns with.
    pub fn bundle(
        &self,
        coord: ChunkCoord,
        cells: &[TileRef; CHUNK_AREA],
        tile_size: UVec2,
        layer: usize,
        layout: TileLayout,
    ) -> (TilemapChunk, TilemapChunkTileData, Transform) {
        (
            self.chunk(tile_size),
            self.tiles(coord, cells, layout),
            chunk_transform(coord, tile_size, layer, layout.placement),
        )
    }

    /// The cropped tiles of a chunk as sprites, placed under the chunk's
    /// entity. Flips are kept; corner layers never hold transposed tiles,
    /// so a transposed tile is cropped as if it were not.
    pub fn edge_pieces(
        &self,
        coord: ChunkCoord,
        cells: &[TileRef; CHUNK_AREA],
        tile_size: UVec2,
        layout: TileLayout,
    ) -> Vec<(TilemapEdgePiece, Sprite, Transform)> {
        if !layout.crops() {
            return Vec::new();
        }
        let size = tile_size.as_vec2();
        let chunk = chunk_transform(coord, tile_size, 0, layout.placement).translation;
        let mut pieces = Vec::new();
        for (local, &tile) in cells.iter().enumerate() {
            let key = coord.cell(local);
            if tile.is_empty() || !tile.visible() || layout.whole(key) {
                continue;
            }
            let (Some((min, max)), Some(strip)) = (
                layout.placement.visible_part(key, layout.bounds),
                self.strip
                    .as_ref()
                    .filter(|_| self.known.contains(&tile.index())),
            ) else {
                continue;
            };
            let (mut x, mut y) = ((min.x, max.x), (min.y, max.y));
            if tile.flip_x() {
                x = (1.0 - x.1, 1.0 - x.0);
            }
            if tile.flip_y() {
                y = (1.0 - y.1, 1.0 - y.0);
            }
            let pixels = self.pixels.as_vec2();
            let origin = Vec2::new(0.0, f32::from(tile.index()) * pixels.y);
            let rect = Rect::new(
                origin.x + x.0 * pixels.x,
                origin.y + (1.0 - y.1) * pixels.y,
                origin.x + x.1 * pixels.x,
                origin.y + (1.0 - y.0) * pixels.y,
            );
            let sprite = Sprite {
                image: strip.clone(),
                rect: Some(rect),
                custom_size: Some((max - min) * size),
                flip_x: tile.flip_x(),
                flip_y: tile.flip_y(),
                ..default()
            };
            let at = (key.0.as_vec2() + layout.placement.offset() + (min + max) / 2.0) * size;
            let transform = Transform::from_translation((at - chunk.truncate()).extend(0.0));
            pieces.push((TilemapEdgePiece, sprite, transform));
        }
        pieces
    }
}

/// Packed tileset images as 2D strips of their layers, top to bottom, so a
/// sprite can crop one tile. A strip is reserved by [`strip_for`] and
/// filled, by [`fill_strips`], once its packed image is loaded, and again
/// whenever it changes.
#[derive(Resource, Default)]
pub struct TileStrips {
    strips: HashMap<AssetId<Image>, Handle<Image>>,
    pending: HashSet<AssetId<Image>>,
}

/// The strip of `packed` from the world's [`TileStrips`], `None` in an app
/// without them.
pub fn strip_for(world: &mut World, packed: &Handle<Image>) -> Option<Handle<Image>> {
    let images = world.get_resource::<Assets<Image>>()?;
    let reserved = images.reserve_handle();
    let mut strips = world.get_resource_mut::<TileStrips>()?;
    let id = packed.id();
    if let Some(strip) = strips.strips.get(&id) {
        return Some(strip.clone());
    }
    strips.pending.insert(id);
    strips.strips.insert(id, reserved.clone());
    Some(reserved)
}

/// Fills each strip whose packed image is loaded and not yet copied, or
/// changed since. A strip whose handle went stale is forgotten, so the next
/// [`strip_for`] reserves a new one.
pub fn fill_strips(
    strips: Option<ResMut<TileStrips>>,
    images: Option<ResMut<Assets<Image>>>,
    events: Option<Res<Messages<AssetEvent<Image>>>>,
    mut cursor: Local<MessageCursor<AssetEvent<Image>>>,
) {
    let (Some(mut strips), Some(mut images), Some(events)) = (strips, images, events) else {
        return;
    };
    for event in cursor.read(&events) {
        if let AssetEvent::Modified { id } = event
            && strips.strips.contains_key(id)
        {
            strips.pending.insert(*id);
        }
    }
    if strips.pending.is_empty() {
        return;
    }
    let ready: Vec<(AssetId<Image>, Image)> = (strips.pending.iter())
        .filter_map(|id| Some((*id, strip_of(images.get(*id)?)?)))
        .collect();
    for (id, strip) in ready {
        strips.pending.remove(&id);
        let stale = (strips.strips.get(&id)).is_some_and(|h| images.insert(h.id(), strip).is_err());
        if stale {
            strips.strips.remove(&id);
        }
    }
}

/// `packed`'s layers stacked top to bottom as one 2D image: an array
/// image's data already holds them in that order.
fn strip_of(packed: &Image) -> Option<Image> {
    let data = packed.data.clone()?;
    let size = packed.texture_descriptor.size;
    let height = size.height.checked_mul(size.depth_or_array_layers.max(1))?;
    let extent = Extent3d {
        width: size.width,
        height,
        depth_or_array_layers: 1,
    };
    let format = packed.texture_descriptor.format;
    let mut strip = Image::new(
        extent,
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    strip.sampler = packed.sampler.clone();
    Some(strip)
}

impl Tileset {
    /// World size of one drawn tile: the tile size, or the map's cell size
    /// for a tileset that names none.
    pub fn display_size(&self, cell_size: UVec2) -> UVec2 {
        match self.tile_size.min_element() {
            0 => cell_size,
            _ => self.tile_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 3;
    type Pixels = [[u8; N]; N];

    fn source() -> Pixels {
        [[0, 1, 2], [3, 4, 5], [6, 7, 8]]
    }

    /// The tile as the kernel defines its bits, rows top down: transpose,
    /// then flip x, then flip y.
    fn kernel_oriented(tile: TileRef) -> Pixels {
        let image = source();
        let mut out = [[0; N]; N];
        for (y, row) in image.iter().enumerate() {
            for (x, &pixel) in row.iter().enumerate() {
                let (mut tx, mut ty) = if tile.transpose() { (y, x) } else { (x, y) };
                if tile.flip_x() {
                    tx = N - 1 - tx;
                }
                if tile.flip_y() {
                    ty = N - 1 - ty;
                }
                out[ty][tx] = pixel;
            }
        }
        out
    }

    /// The tile as the chunk shader samples it, in uv (y down): mirror h,
    /// then v, then swap the coordinates.
    fn shader_oriented(orientation: TileOrientation) -> Pixels {
        let image = source();
        let mut out = [[0; N]; N];
        for (y, row) in out.iter_mut().enumerate() {
            for (x, pixel) in row.iter_mut().enumerate() {
                let (mut u, mut v) = (x, y);
                if orientation.mirror_h() {
                    u = N - 1 - u;
                }
                if orientation.mirror_v() {
                    v = N - 1 - v;
                }
                if orientation.mirror_d() {
                    (u, v) = (v, u);
                }
                *pixel = image[v][u];
            }
        }
        out
    }

    #[test]
    fn every_orientation_draws_the_tile_the_kernel_means() {
        let mut seen = HashSet::new();
        for bits in 0..8u8 {
            let tile = TileRef::from_raw(5, bits);
            let data = tile_data(tile).expect("a filled cell draws");
            assert_eq!(data.tileset_index, 5);
            assert!(data.visible);
            assert_eq!(
                shader_oriented(data.orientation),
                kernel_oriented(tile),
                "bits {bits:03b} as {:?}",
                data.orientation
            );
            seen.insert(data.orientation);
        }
        assert_eq!(seen.len(), 8, "the eight orientations stay distinct");
    }

    #[test]
    fn hidden_and_empty_cells() {
        let hidden = tile_data(TileRef::new(2).with_visible(false)).expect("hidden still draws");
        assert!(!hidden.visible);
        assert!(tile_data(TileRef::EMPTY).is_none());
    }

    #[test]
    fn cell_zero_sits_at_the_map_origin() {
        let size = UVec2::new(16, 8);
        let at = chunk_transform(ChunkCoord(IVec2::new(-1, 2)), size, 3, Placement::OnCells);
        let half = Vec2::new(16.0 * 16.0, 16.0 * 8.0);
        let min = at.translation.truncate() - half;
        assert_eq!(min, Vec2::new(-32.0 * 16.0, 64.0 * 8.0));
        assert_eq!(at.translation.z, 3.0);
    }

    #[test]
    fn tiles_between_cells_sit_half_a_tile_up_and_right() {
        let size = UVec2::new(16, 8);
        let coord = ChunkCoord(IVec2::new(-1, 2));
        let on = chunk_transform(coord, size, 3, Placement::OnCells);
        let between = chunk_transform(coord, size, 3, Placement::BetweenCells);
        assert_eq!(
            between.translation - on.translation,
            Vec3::new(8.0, 4.0, 0.0)
        );
    }

    fn sheet_tileset() -> Tileset {
        Tileset {
            tile_size: UVec2::splat(16),
            sheets: vec![crate::TilesetSheet {
                columns: 2,
                rows: 2,
                ..default()
            }],
            ..default()
        }
    }

    #[test]
    fn bounded_tiles_between_cells_crop_into_edge_pieces() {
        let tileset = sheet_tileset();
        let mut draw = ChunkDraw::new(&tileset, Handle::default());
        draw.strip = Some(Handle::default());
        let layout = TileLayout {
            placement: Placement::BetweenCells,
            bounds: Some(CellRect {
                min: IVec2::ZERO,
                max: IVec2::splat(3),
            }),
        };
        let coord = ChunkCoord(IVec2::new(-1, 0));
        let mut cells = [TileRef::EMPTY; CHUNK_AREA];
        let edge = jackdaw_tilemap::CellCoord::new(-1, 1).local_index();
        cells[edge] = TileRef::new(3);
        assert!(draw.tiles(coord, &cells, layout)[edge].is_none());
        let pieces = draw.edge_pieces(coord, &cells, UVec2::splat(16), layout);
        assert_eq!(pieces.len(), 1);
        let (_, sprite, at) = &pieces[0];
        assert_eq!(sprite.rect, Some(Rect::new(8.0, 48.0, 16.0, 64.0)));
        assert_eq!(sprite.custom_size, Some(Vec2::new(8.0, 16.0)));
        let chunk = chunk_transform(coord, UVec2::splat(16), 0, layout.placement);
        let world = chunk.translation + at.translation;
        assert_eq!(world.truncate(), Vec2::new(4.0, 32.0));
        let on_cells = TileLayout {
            placement: Placement::OnCells,
            ..layout
        };
        assert!(draw.tiles(coord, &cells, on_cells)[edge].is_some());
        assert!(
            draw.edge_pieces(coord, &cells, UVec2::splat(16), on_cells)
                .is_empty()
        );
    }

    #[test]
    fn a_chunk_holds_every_cell_and_drops_unknown_tiles() {
        let tileset = Tileset {
            sheets: vec![crate::TilesetSheet {
                columns: 2,
                rows: 2,
                ..default()
            }],
            ..default()
        };
        let draw = ChunkDraw::new(&tileset, Handle::default());
        let mut cells = [TileRef::EMPTY; CHUNK_AREA];
        cells[0] = TileRef::new(3);
        cells[1] = TileRef::new(4);
        let tiles = draw.tiles(ChunkCoord::new(0, 0), &cells, TileLayout::default());
        assert_eq!(tiles.len(), CHUNK_AREA);
        assert_eq!(tiles[0].map(|t| t.tileset_index), Some(3));
        assert!(tiles[1].is_none(), "layer 4 is past the sheet");
        assert_eq!(tileset.display_size(UVec2::splat(8)), UVec2::splat(8));
    }

    #[test]
    fn a_strip_holds_the_packed_layers_top_to_bottom_once_loaded() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .add_plugins(crate::TilesetPlugin);
        let size = Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 3,
        };
        let data: Vec<u8> = (0..2 * 2 * 3 * 4).map(|b| b as u8).collect();
        let format = bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb;
        let usage = RenderAssetUsages::default();
        let packed = Image::new(size, TextureDimension::D2, data.clone(), format, usage);
        let packed = app.world_mut().resource_mut::<Assets<Image>>().add(packed);
        let strip = strip_for(app.world_mut(), &packed).expect("a strip");
        assert_eq!(strip_for(app.world_mut(), &packed), Some(strip.clone()));
        app.update();
        let images = app.world().resource::<Assets<Image>>();
        let strip = images.get(&strip).expect("filled");
        assert_eq!(strip.size(), UVec2::new(2, 6));
        assert_eq!(strip.data.as_deref(), Some(data.as_slice()));
    }
}
