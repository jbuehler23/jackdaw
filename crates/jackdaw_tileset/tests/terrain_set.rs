//! Solver sets built from a tileset, checked against the overworld sheet.

use std::collections::{BTreeMap, BTreeSet};

use bevy::math::UVec2;
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, LayoutTemplate, Links, MatchMode, Peer, RgbaImage, TerrainId,
    TerrainSet, TileRef, TileTerrains, TilesetSetup, auto_setup, match_score, required_peers,
    resolve_region,
};
use jackdaw_tileset::{
    TerrainMatch, TerrainSetError, Tileset, TilesetSheet, TilesetTerrain, TilesetTerrainSet,
    TilesetTile,
};

const SHEET: &str = "tiles/overworld.png";

fn overworld() -> Tileset {
    let path = format!(
        "{}/../jackdaw_tilemap/tests/data/punyworld/punyworld-overworld-tileset.png",
        env!("CARGO_MANIFEST_DIR")
    );
    let image = image::open(path).expect("fixture decodes").to_rgba8();
    let rgba = RgbaImage::new(image.width(), image.height(), image.as_raw()).expect("sized");
    let fresh = auto_setup(&[(SHEET, rgba)], &[]).expect("auto setup runs");
    let mut tileset = Tileset {
        image: "tiles/overworld.tiles.png".into(),
        tile_size: UVec2::splat(16),
        grid: UVec2::new(27, 38),
        sheets: vec![TilesetSheet {
            source: SHEET.into(),
            columns: 27,
            rows: 38,
            ..Default::default()
        }],
        ..Default::default()
    };
    tileset
        .set_setup(&TilesetSetup::from_auto(&fresh))
        .expect("converts");
    tileset
}

fn terrain_named(tileset: &Tileset, name: &str) -> TerrainId {
    (tileset.terrains.iter())
        .find(|t| t.name == name)
        .map(|t| t.id)
        .expect("terrain exists")
}

fn peers_of(tileset: &Tileset, index: u16) -> [TerrainId; 9] {
    let (sheet, tile) = tileset.tile_of_layer(u32::from(index)).expect("a layer");
    (tileset.tiles.iter())
        .find(|t| t.sheet == sheet && t.tile == tile)
        .map(|t| t.peers)
        .expect("a terrain tile")
}

/// A 9x9 field of `outer` with a 5x5 block of `inner` at (2, 2), resolved.
fn resolve_block(
    set: &TerrainSet,
    outer: TerrainId,
    inner: TerrainId,
) -> BTreeMap<CellCoord, TileRef> {
    let mut terrain = ChunkedLayer::<TerrainId>::new();
    for y in 0..9 {
        for x in 0..9 {
            let inside = (2..7).contains(&x) && (2..7).contains(&y);
            terrain.set(CellCoord::new(x, y), if inside { inner } else { outer });
        }
    }
    let dirty: Vec<_> = terrain.cells().map(|(cell, _)| cell).collect();
    resolve_region(
        set,
        &terrain,
        &ChunkedLayer::<Links>::new(),
        &BTreeMap::new(),
        dirty,
    )
    .into_iter()
    .collect()
}

#[test]
fn a_block_of_one_grass_on_another_resolves_to_its_transition_tiles() {
    let tileset = overworld();
    let grass = terrain_named(&tileset, "Terrain 1");
    let raised = terrain_named(&tileset, "Terrain 2");
    let water = terrain_named(&tileset, "Terrain 7");
    let set = tileset
        .terrain_set(Some(TerrainMatch::CornersAndSides))
        .expect("the sheet has sets of this mode");
    assert_eq!(set.mode, MatchMode::CornersAndSides);
    assert_eq!(
        set.order[..3],
        [grass, raised, water],
        "a terrain of plain tiles alone joins the first set on top"
    );

    let resolved = resolve_block(&set, grass, raised);
    let index = |x, y| resolved[&CellCoord::new(x, y)].index();
    assert_eq!(
        [index(2, 6), index(4, 6), index(2, 4), index(4, 4)],
        [10, 11, 49, 38],
        "north-west corner, north edge, west edge and inside of the block"
    );
    let west = peers_of(&tileset, index(2, 4));
    for peer in [Peer::TopLeft, Peer::Left, Peer::BottomLeft] {
        assert_eq!(west[peer.index()], grass, "{peer:?} of the west edge");
    }
    for peer in [Peer::Top, Peer::Centre, Peer::Right, Peer::Bottom] {
        assert_eq!(west[peer.index()], raised, "{peer:?} of the west edge");
    }
    assert_eq!(peers_of(&tileset, index(4, 4)), [raised; 9]);
    assert_eq!(
        peers_of(&tileset, index(0, 0)),
        [grass; 9],
        "open ground stays plain"
    );
    let edge = (tileset.tiles.iter())
        .find(|t| tileset.layer(t.sheet, t.tile) == Some(u32::from(index(2, 4))))
        .expect("the edge tile");
    assert_eq!(
        (edge.set_inner, edge.set_outer),
        (raised, grass),
        "it records the set it was made for"
    );
}

#[test]
fn every_set_of_the_mode_shares_one_layer_and_a_mixed_sheet_needs_a_mode() {
    let tileset = overworld();
    assert!(matches!(
        tileset.terrain_set(None),
        Err(TerrainSetError::MixedModes(_))
    ));
    let set = tileset
        .terrain_set(Some(TerrainMatch::CornersAndSides))
        .expect("a set");
    for name in ["Terrain 2", "Terrain 7"] {
        let id = terrain_named(&tileset, name);
        assert!(
            set.tiles.iter().any(|t| t.terrains.centre() == id),
            "{name} has tiles"
        );
    }
}

/// A set of `terrains`, bottom first.
fn set_of(terrains: &[TerrainId], mode: TerrainMatch) -> TilesetTerrainSet {
    TilesetTerrainSet {
        terrains: terrains.to_vec(),
        mode,
        ..Default::default()
    }
}

#[test]
fn mixed_modes_need_the_layer_to_pick_one() {
    let mut tileset = Tileset {
        sheets: vec![TilesetSheet {
            columns: 4,
            rows: 1,
            ..Default::default()
        }],
        terrain_sets: vec![
            set_of(&[2], TerrainMatch::Sides),
            set_of(&[1, 3], TerrainMatch::Corners),
        ],
        tiles: vec![
            TilesetTile {
                tile: 0,
                peers: [1; 9],
                ..Default::default()
            },
            TilesetTile {
                tile: 1,
                peers: [3; 9],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let err = tileset.terrain_set(None).expect_err("ambiguous");
    assert!(matches!(err, TerrainSetError::MixedModes(_)));
    assert!(err.to_string().contains("pick one match mode"));

    let corners = tileset
        .terrain_set(Some(TerrainMatch::Corners))
        .expect("named mode");
    assert_eq!(corners.mode, MatchMode::Corners);
    assert_eq!(corners.tiles.len(), 2, "grass and the corner terrain");
    assert_eq!(corners.order, vec![1, 3]);

    assert_eq!(
        tileset.terrain_set(Some(TerrainMatch::CornersAndSides)),
        Err(TerrainSetError::NoSetWithMode(
            TerrainMatch::CornersAndSides
        ))
    );
    tileset.terrain_sets.clear();
    assert_eq!(
        tileset.terrain_set(None),
        Err(TerrainSetError::NoTerrainSets)
    );
}

fn tile(index: u32, peers: [TerrainId; 9], set: (TerrainId, TerrainId)) -> TilesetTile {
    TilesetTile {
        tile: index,
        peers,
        set_inner: set.0,
        set_outer: set.1,
        ..Default::default()
    }
}

/// Auto setup may list a terrain in sets of two match modes.
#[test]
fn sets_of_two_modes_over_the_same_terrains_keep_their_own_tiles() {
    let edge = [1, 2, 1, 1, 2, 1, 1, 2, 1];
    let tileset = Tileset {
        sheets: vec![TilesetSheet {
            columns: 4,
            rows: 1,
            ..Default::default()
        }],
        terrain_sets: vec![
            set_of(&[1, 2], TerrainMatch::Sides),
            set_of(&[2, 1], TerrainMatch::Corners),
        ],
        tiles: vec![
            tile(0, edge, (2, 1)),
            tile(1, edge, (1, 2)),
            tile(2, [1; 9], (0, 0)),
            tile(3, edge, (0, 0)),
        ],
        ..Default::default()
    };
    let indices = |mode| -> Vec<u16> {
        let set = tileset.terrain_set(Some(mode)).expect("a set");
        set.tiles.iter().map(|t| t.tile.index()).collect()
    };
    assert_eq!(indices(TerrainMatch::Sides), vec![0, 2]);
    assert_eq!(
        indices(TerrainMatch::Corners),
        vec![1, 2],
        "an unrecorded transition tile could be either set's, so neither takes it"
    );
    assert_eq!(tileset.set_of(1), Some(0), "the first set listing it");
}

#[test]
fn a_tile_edited_off_its_recorded_set_is_placed_by_its_peers() {
    let tileset = Tileset {
        sheets: vec![TilesetSheet {
            columns: 2,
            rows: 1,
            ..Default::default()
        }],
        terrain_sets: vec![set_of(&[1, 2, 3], TerrainMatch::Sides)],
        tiles: vec![tile(0, [1, 3, 1, 1, 3, 1, 1, 3, 1], (2, 1))],
        ..Default::default()
    };
    let set = tileset.terrain_set(None).expect("one mode");
    assert_eq!(
        set.tiles.len(),
        1,
        "its peers fit the set, so it joins instead of dropping out"
    );
}

const GRASS: TerrainId = 1;
const DIRT: TerrainId = 2;
const WATER: TerrainId = 3;

/// The corner tiles of each transition, drawn terrain first, one per sheet
/// tile; tiles two transitions share appear once.
fn corner_tiles(pairs: &[(TerrainId, TerrainId)]) -> Vec<TilesetTile> {
    let mut found: Vec<[TerrainId; 9]> = Vec::new();
    for &(inner, outer) in pairs {
        for shape in LayoutTemplate::Corner16.tiles(inner, outer) {
            if !found.contains(&shape.peers) {
                found.push(shape.peers);
            }
        }
    }
    (found.into_iter().zip(0..))
        .map(|(peers, tile)| TilesetTile {
            tile,
            peers,
            ..Default::default()
        })
        .collect()
}

fn ground_terrains() -> Vec<TilesetTerrain> {
    [
        (GRASS, "grass", true),
        (DIRT, "dirt", false),
        (WATER, "water", false),
    ]
    .map(|(id, name, background)| TilesetTerrain {
        id,
        name: name.into(),
        background,
        ..Default::default()
    })
    .to_vec()
}

/// Grass, dirt and water with corner tiles for every transition among them.
fn three_terrains(terrain_sets: Vec<TilesetTerrainSet>) -> Tileset {
    let tiles = corner_tiles(&[(DIRT, GRASS), (WATER, GRASS), (WATER, DIRT)]);
    Tileset {
        sheets: vec![TilesetSheet {
            columns: tiles.len() as u32,
            rows: 1,
            ..Default::default()
        }],
        setup_sheets: vec![0],
        terrains: ground_terrains(),
        terrain_sets,
        tiles,
        ..Default::default()
    }
}

fn ground_set() -> TilesetTerrainSet {
    TilesetTerrainSet {
        name: "Ground".into(),
        mode: TerrainMatch::Corners,
        terrains: vec![GRASS, DIRT, WATER],
        ..Default::default()
    }
}

/// Grass with a block of dirt, a block of water, and a block of dirt around
/// water, so each pair of terrains meets and no corner holds all three.
fn ground_map() -> ChunkedLayer<TerrainId> {
    let mut terrain = ChunkedLayer::<TerrainId>::new();
    let inside = |x: i32, y: i32, (x0, y0, x1, y1): (i32, i32, i32, i32)| {
        (x0..x1).contains(&x) && (y0..y1).contains(&y)
    };
    for y in 0..16 {
        for x in 0..16 {
            let id = if inside(x, y, (4, 11, 6, 13)) || inside(x, y, (10, 2, 14, 6)) {
                WATER
            } else if inside(x, y, (2, 2, 6, 6)) || inside(x, y, (2, 9, 8, 15)) {
                DIRT
            } else {
                GRASS
            };
            terrain.set(CellCoord::new(x, y), id);
        }
    }
    terrain
}

fn resolve_map(set: &TerrainSet, terrain: &ChunkedLayer<TerrainId>) -> Vec<(CellCoord, TileRef)> {
    let dirty: Vec<_> = terrain.cells().map(|(cell, _)| cell).collect();
    resolve_region(
        set,
        terrain,
        &ChunkedLayer::<Links>::new(),
        &BTreeMap::new(),
        dirty,
    )
}

#[test]
fn a_three_terrain_corner_set_paints_every_transition_it_has_tiles_for() {
    let tileset = three_terrains(vec![ground_set()]);
    let set = tileset.terrain_set(None).expect("one mode");
    assert_eq!(set.order, vec![GRASS, DIRT, WATER]);
    let terrain = ground_map();
    let resolved = resolve_map(&set, &terrain);
    assert!(!resolved.is_empty());
    let mut shown = BTreeSet::new();
    for (cell, tile) in &resolved {
        assert_ne!(tile.index(), TileRef::EMPTY_INDEX, "{cell:?} has a tile");
        let peers = peers_of(&tileset, tile.index());
        let required = required_peers(&set, &terrain, &ChunkedLayer::new(), *cell);
        let wanted = required.iter().filter(|r| !r.is_any()).count();
        let score = match_score(&required, &TileTerrains { peers });
        assert_eq!(usize::from(score), wanted, "{cell:?} matches exactly");
        let mut held: Vec<TerrainId> = Peer::CORNERS.iter().map(|p| peers[p.index()]).collect();
        held.sort_unstable();
        held.dedup();
        if let [a, b] = held[..] {
            shown.insert((a, b));
        }
    }
    let every = BTreeSet::from([(GRASS, DIRT), (GRASS, WATER), (DIRT, WATER)]);
    assert_eq!(shown, every);
}

#[test]
fn a_setup_taken_back_puts_every_terrain_in_a_set() {
    let mut tileset = three_terrains(vec![set_of(&[GRASS, DIRT], TerrainMatch::Corners)]);
    let setup = tileset.setup().expect("converts");
    tileset.set_setup(&setup).expect("converts back");
    assert_eq!(
        tileset.terrain_sets[0].terrains,
        [GRASS, DIRT, WATER],
        "water joins the first set"
    );
    assert_eq!(tileset.set_of(WATER), Some(0));

    let mut setup = tileset.setup().expect("converts");
    setup.sets.clear();
    tileset.set_setup(&setup).expect("converts back");
    assert_eq!(tileset.terrain_sets.len(), 1, "a set for the homeless");
    let home = &tileset.terrain_sets[0];
    assert_eq!(home.name, "Terrain set 1");
    assert_eq!(home.mode, TerrainMatch::CornersAndSides);
    assert_eq!(home.terrains, [GRASS, DIRT, WATER]);
}
