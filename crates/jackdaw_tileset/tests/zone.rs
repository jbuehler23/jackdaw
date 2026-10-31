use bevy::math::{UVec2, Vec2};
use jackdaw_tilemap::{MatchMode, Peer};
use jackdaw_tileset::{Tileset, TilesetSheet};

/// One 16 pixel sheet of 4 by 3 tiles with a margin and spacing.
fn tileset() -> Tileset {
    Tileset {
        tile_size: UVec2::splat(16),
        sheets: vec![TilesetSheet {
            source: "ground.png".into(),
            columns: 4,
            rows: 3,
            margin: UVec2::splat(1),
            spacing: UVec2::splat(2),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn a_point_on_the_sheet_finds_its_tile_and_zone() {
    let tileset = tileset();
    let point = |tile: u32, at: Vec2| {
        let (origin, size) = tileset.sheet_rect(0, tile).expect("a tile");
        origin.as_vec2() + at * size.as_vec2()
    };
    let all = MatchMode::CornersAndSides;
    let zone = |at| tileset.zone_at(0, at, all);
    assert_eq!(zone(point(0, Vec2::splat(0.1))), Some((0, Peer::TopLeft)));
    assert_eq!(zone(point(5, Vec2::new(0.5, 0.9))), Some((5, Peer::Bottom)));
    assert_eq!(zone(point(11, Vec2::splat(0.5))), Some((11, Peer::Centre)));
    let corners = tileset.zone_at(0, point(6, Vec2::new(0.6, 0.2)), MatchMode::Corners);
    assert_eq!(corners, Some((6, Peer::TopRight)), "corners");
    assert_eq!(zone(Vec2::splat(0.5)), None, "the margin");
    let gap = point(0, Vec2::new(1.0, 0.5)) + Vec2::new(1.0, 0.0);
    assert_eq!(zone(gap), None, "the spacing");
}
