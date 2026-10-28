use bevy::math::UVec2;
use jackdaw_tilemap::{
    CollisionMode, MatchMode, Peer, RgbaImage, Scope, Stamp, TileAddress, TilesetSetup, auto_setup,
    rerun,
};
use jackdaw_tileset::{Tileset, TilesetError, TilesetObject, TilesetSheet};

const SHEET: &str = "tiles/tilemap_packed.png";
const DRY_PATH: u8 = 2;

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn load() -> Self {
        let path = format!(
            "{}/../jackdaw_tilemap/tests/data/tiny_farm/tilemap_packed.png",
            env!("CARGO_MANIFEST_DIR")
        );
        let image = image::open(path).expect("fixture decodes").to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        }
    }

    fn inputs(&self) -> [(&str, RgbaImage<'_>); 1] {
        let image = RgbaImage::new(self.width, self.height, &self.pixels).expect("sized");
        [(SHEET, image)]
    }
}

fn tileset() -> Tileset {
    Tileset {
        image: "tiles/tilemap_packed.tiles.png".into(),
        tile_size: UVec2::splat(16),
        grid: UVec2::new(12, 11),
        sheets: vec![TilesetSheet {
            source: SHEET.into(),
            columns: 12,
            rows: 11,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Auto setup over Tiny Farm with traced shapes, then a hand edit of every
/// kind the setup tracks.
fn edited_setup(sheet: &Sheet) -> TilesetSetup {
    let fresh = auto_setup(&sheet.inputs(), &[DRY_PATH]).expect("auto setup runs");
    let mut setup = TilesetSetup::from_auto(&fresh);
    assert!(setup.tiles.values().any(|t| t.shape.value.is_some()));
    let tile = TileAddress { sheet: 0, tile: 1 };
    setup.set_peer(tile, Peer::Top, DRY_PATH).expect("peer");
    setup.rename_terrain(1, "Grass").expect("rename");
    (setup.set_collision_mode(3, CollisionMode::Solid)).expect("collision");
    (setup.set_match_mode(2, 1, MatchMode::CornersAndSides)).expect("mode");
    setup.lock_set(3, 1, true).expect("lock");
    setup.put_stamp(Stamp {
        sheet: 0,
        origin: UVec2::new(0, 10),
        size: UVec2::new(2, 1),
        cells: vec![(UVec2::ZERO, 120), (UVec2::new(1, 0), 121)],
    });
    setup
}

#[test]
fn a_setup_survives_the_tileset_unchanged() {
    let sheet = Sheet::load();
    let setup = edited_setup(&sheet);
    let mut tileset = tileset();
    tileset
        .set_setup(&setup)
        .expect("the setup's sheets are the tileset's");

    assert_eq!(tileset.terrains.len(), 3);
    assert_eq!(tileset.terrain_sets.len(), 2);
    assert_eq!(tileset.setup().expect("converts back"), setup);
}

#[test]
fn a_rerun_through_the_tileset_keeps_what_the_user_set() {
    let sheet = Sheet::load();
    let setup = edited_setup(&sheet);
    let mut tileset = tileset();
    tileset.set_setup(&setup).expect("converts");
    let back = tileset.setup().expect("converts back");

    let direct = rerun(&setup, &sheet.inputs(), &Scope::All).expect("rerun");
    let through = rerun(&back, &sheet.inputs(), &Scope::All).expect("rerun");
    assert_eq!(through, direct);
    assert_eq!(through.terrain(1).expect("grass").name.value, "Grass");
}

#[test]
fn objects_keep_their_pivot_and_new_ones_pivot_at_the_bottom_centre() {
    let sheet = Sheet::load();
    let setup = edited_setup(&sheet);
    let mut tileset = tileset();
    tileset.set_setup(&setup).expect("converts");
    assert!(
        (tileset.objects.iter()).all(|o| o.pivot == TilesetObject::BOTTOM_CENTRE),
        "a fresh object stands on its bottom centre"
    );
    let moved = UVec2::new(0, 10);
    let at = |t: &Tileset| t.objects.iter().position(|o| o.origin == moved);
    let index = at(&tileset).expect("the hand-made object");
    tileset.objects[index].pivot = bevy::math::Vec2::new(0.25, 0.5);

    tileset.set_setup(&setup).expect("converts again");
    let index = at(&tileset).expect("still there");
    assert_eq!(
        tileset.objects[index].pivot,
        bevy::math::Vec2::new(0.25, 0.5)
    );
}

#[test]
fn a_pivot_follows_the_object_of_the_same_bounds_not_just_the_same_origin() {
    let block = |size: UVec2, tile: u32| Stamp {
        sheet: 0,
        origin: UVec2::new(4, 4),
        size,
        cells: vec![(UVec2::ZERO, tile)],
    };
    let mut setup = TilesetSetup {
        sheets: vec![SHEET.into()],
        ..Default::default()
    };
    setup.stamps = vec![
        jackdaw_tilemap::Tracked::user(block(UVec2::new(1, 1), 52)),
        jackdaw_tilemap::Tracked::user(block(UVec2::new(2, 2), 52)),
    ];
    let mut tileset = tileset();
    tileset.set_setup(&setup).expect("converts");
    let tall = bevy::math::Vec2::new(0.5, 1.0);
    tileset.objects[1].pivot = tall;

    tileset.set_setup(&setup).expect("converts again");
    let pivots: Vec<_> = tileset.objects.iter().map(|o| o.pivot).collect();
    assert_eq!(pivots, vec![TilesetObject::BOTTOM_CENTRE, tall]);
}

#[test]
fn a_setup_over_a_sheet_the_tileset_lacks_is_refused() {
    let mut setup = TilesetSetup::default();
    setup.sheets.push("elsewhere.png".into());
    let mut tileset = tileset();
    assert_eq!(
        tileset.set_setup(&setup),
        Err(TilesetError::UnknownSheet("elsewhere.png".into()))
    );
}

#[test]
fn a_tile_layer_is_its_sheet_offset_plus_its_index() {
    let mut tileset = tileset();
    tileset.sheets.push(TilesetSheet {
        source: "b.png".into(),
        columns: 4,
        rows: 2,
        first_layer: 132,
        ..Default::default()
    });
    assert_eq!(tileset.layer(0, 131), Some(131));
    assert_eq!(tileset.layer(1, 3), Some(135));
    assert_eq!(tileset.layer(1, 8), None);
    assert_eq!(tileset.layer(2, 0), None);
}
