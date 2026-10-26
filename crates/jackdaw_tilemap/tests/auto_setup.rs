//! Auto setup over the Tiny Farm sheets: the full report is a golden file.

use std::collections::BTreeSet;

use jackdaw_tilemap::{
    AutoSetup, CollisionMode, Peer, Provenance, RgbaImage, Scope, SetupTile, TileAddress,
    TilesetSetup, Tracked, auto_setup, rerun,
};

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn load(name: &str) -> Self {
        let path = format!("{}/tests/data/tiny_farm/{name}", env!("CARGO_MANIFEST_DIR"));
        let image = image::open(&path).expect("fixture decodes").to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        }
    }

    fn image(&self) -> RgbaImage<'_> {
        RgbaImage::new(self.width, self.height, &self.pixels).expect("sized")
    }
}

fn setup(name: &str) -> AutoSetup {
    let sheet = Sheet::load(name);
    auto_setup(&[(name, sheet.image())], &[]).expect("auto setup runs")
}

fn golden() -> String {
    let path = format!(
        "{}/tests/data/tiny_farm/expected_report.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("golden report")
}

#[test]
fn packed_report_matches_golden() {
    assert_eq!(setup("tilemap_packed.png").report.to_string(), golden());
}

#[test]
fn spaced_report_matches_golden_but_its_grid_line() {
    let spaced = setup("tilemap.png").report.to_string();
    let renamed = spaced
        .replace("\"tilemap.png\"", "\"tilemap_packed.png\"")
        .replace("\"tilemap ", "\"tilemap_packed ");
    let golden = golden();
    let (got, want): (Vec<&str>, Vec<&str>) = (renamed.lines().collect(), golden.lines().collect());
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        if w.starts_with("  sheet ") {
            assert!(
                g.contains("margin 0x0, spacing 1x1, 12 columns, 11 rows"),
                "{g}"
            );
        } else {
            assert_eq!(g, w, "line {i}");
        }
    }
}

#[test]
fn stamps_follow_the_art() {
    let found = setup("tilemap_packed.png");
    let stamps: Vec<Vec<u32>> = (found.stamps.iter())
        .map(|s| s.cells.iter().map(|(_, tile)| *tile).collect())
        .collect();
    let barn = vec![90, 91, 92, 102, 103, 104, 114, 115, 116, 126, 127, 128];
    let roof = vec![82, 93, 94, 95, 105, 106, 107, 117, 118, 119, 129, 130, 131];
    let expected = [
        vec![2, 14],
        vec![3, 15],
        vec![26, 38],
        roof,
        barn,
        vec![98, 99],
        vec![100, 101],
        vec![110, 111],
        vec![112, 113],
    ];
    assert_eq!(stamps, expected);
    let roof = &found.stamps[3];
    assert_eq!(
        (roof.origin.to_array(), roof.size.to_array()),
        ([9, 6], [3, 5])
    );
}

#[test]
fn absorbed_terrains_leave_the_proposal() {
    let found = setup("tilemap_packed.png");
    let kept: Vec<u8> = found.proposal.terrains.iter().map(|t| t.id).collect();
    assert_eq!(kept, [1, 2, 3]);
    assert_eq!(found.sheets, [0]);
    let unassigned: Vec<u32> = found.proposal.unassigned.iter().map(|a| a.tile).collect();
    assert_eq!(
        found.report.unassigned,
        [("tilemap_packed.png".to_string(), unassigned)]
    );
}

#[test]
fn collision_is_reported_only_for_solid_terrains() {
    let sheet = Sheet::load("tilemap_packed.png");
    let found = auto_setup(&[("tilemap_packed.png", sheet.image())], &[2, 3]).expect("runs");
    let text = found.report.to_string();
    let last = text.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("collision: solid terrains 2 3, "),
        "{text}"
    );
    assert!(!golden().contains("collision"));
}

const PACKED: &str = "tilemap_packed.png";
const SPACED: &str = "tilemap.png";
const GRASS: u8 = 1;
const DRY_PATH: u8 = 2;
const DIRT_PATH: u8 = 3;

fn inputs<'a>(sheets: &[(&'a str, &'a Sheet)]) -> Vec<(&'a str, RgbaImage<'a>)> {
    sheets.iter().map(|(n, s)| (*n, s.image())).collect()
}

fn run(sheets: &[(&str, &Sheet)]) -> AutoSetup {
    auto_setup(&inputs(sheets), &[]).expect("auto setup runs")
}

fn rerun_over(previous: &TilesetSetup, sheets: &[(&str, &Sheet)], scope: &Scope) -> TilesetSetup {
    rerun(previous, &inputs(sheets), scope).expect("re-run")
}

fn uses(tile: &SetupTile, id: u8) -> bool {
    tile.terrains.peers.contains(&id)
}

fn first_with(setup: &TilesetSetup, id: u8, skip: usize) -> TileAddress {
    let found = setup.tiles.iter().filter(|(_, t)| uses(t, id)).nth(skip);
    *found.expect("a tile with the terrain").0
}

#[test]
fn user_bits_names_and_modes_survive_a_rerun() {
    let sheet = Sheet::load(PACKED);
    let fresh = run(&[(PACKED, &sheet)]);
    let mut setup = TilesetSetup::from_auto(&fresh);
    let mut user_bits = Vec::new();
    for (i, peer) in [Peer::Top, Peer::Left, Peer::BottomRight]
        .into_iter()
        .enumerate()
    {
        let address = first_with(&setup, DRY_PATH, i);
        let auto = setup.tiles[&address].terrains.get(peer);
        let hand = if auto == GRASS { DRY_PATH } else { GRASS };
        setup.set_peer(address, peer, hand).expect("known terrain");
        user_bits.push((address, peer, hand));
    }
    let cleared = first_with(&setup, DRY_PATH, 3);
    for peer in Peer::ALL {
        if setup.tiles[&cleared].terrains.get(peer) == DRY_PATH {
            setup.set_peer(cleared, peer, GRASS).expect("known terrain");
            user_bits.push((cleared, peer, GRASS));
        }
    }
    let painted = first_with(&setup, DIRT_PATH, 0);
    for peer in Peer::ALL {
        setup
            .set_peer(painted, peer, DRY_PATH)
            .expect("known terrain");
        user_bits.push((painted, peer, DRY_PATH));
    }
    setup
        .rename_terrain(GRASS, "meadow")
        .expect("known terrain");
    (setup.set_collision_mode(DRY_PATH, CollisionMode::Solid)).expect("known terrain");

    let after = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);

    for (address, peer, hand) in &user_bits {
        let tile = &after.tiles[address];
        assert_eq!(tile.terrains.get(*peer), *hand);
        assert_eq!(tile.provenance[peer.index()], Provenance::User);
    }
    let grass = after.terrain(GRASS).expect("grass kept");
    assert_eq!(grass.name, Tracked::user("meadow".to_string()));
    let dry = after.terrain(DRY_PATH).expect("dry path kept");
    assert_eq!(dry.collision, Tracked::user(CollisionMode::Solid));

    let auto = TilesetSetup::from_auto(&fresh);
    assert_eq!((&after.sets, &after.stamps), (&auto.sets, &auto.stamps));
    assert_eq!((&after.sheets, after.next_id), (&auto.sheets, auto.next_id));
    for t in &after.terrains {
        let a = auto.terrain(t.id).expect("same terrains");
        assert_eq!((t.oklab, t.background), (a.oklab, a.background));
    }
    let keys = after.tiles.keys().chain(auto.tiles.keys());
    for address in keys.copied().collect::<BTreeSet<TileAddress>>() {
        let tile = after.tiles.get(&address).cloned().unwrap_or_default();
        let fresh_tile = auto.tiles.get(&address).cloned().unwrap_or_default();
        for peer in Peer::ALL {
            if tile.provenance[peer.index()] == Provenance::Auto {
                let got = tile.terrains.get(peer);
                assert_eq!(got, fresh_tile.terrains.get(peer), "{address:?}");
            }
        }
    }

    for (address, tile) in &after.tiles {
        assert_eq!(tile.shape.provenance, Provenance::Auto);
        if tile.shape.value.is_some() {
            assert!(uses(tile, DRY_PATH), "{address:?}");
        } else {
            let all_auto = !tile.provenance.contains(&Provenance::User);
            assert!(!(all_auto && uses(tile, DRY_PATH)), "{address:?}");
        }
    }
    assert!(after.tiles[&cleared].shape.value.is_none());
    assert!(after.tiles[&painted].shape.value.is_some());
}

#[test]
fn an_imported_solid_terrain_stays_solid_with_its_shapes() {
    let sheet = Sheet::load(PACKED);
    let inputs = [(PACKED, sheet.image())];
    let fresh = auto_setup(&inputs, &[DIRT_PATH]).expect("runs");
    let setup = TilesetSetup::from_auto(&fresh);
    let solid = Tracked::auto(CollisionMode::Solid);
    assert_eq!(setup.terrain(DIRT_PATH).map(|t| &t.collision), Some(&solid));
    let after = rerun(&setup, &inputs, &Scope::All).expect("re-run");
    assert_eq!(after.terrain(DIRT_PATH).map(|t| &t.collision), Some(&solid));
    let shapes = |s: &TilesetSetup| -> Vec<(TileAddress, bool)> {
        (s.tiles.iter())
            .map(|(a, t)| (*a, t.shape.value.is_some()))
            .collect()
    };
    assert!(shapes(&after).iter().any(|(_, s)| *s));
    assert_eq!(shapes(&after), shapes(&setup));
}

#[test]
fn a_user_solid_terrain_with_no_match_still_collides() {
    let sheet = Sheet::load(PACKED);
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &sheet)]));
    (setup.set_collision_mode(DRY_PATH, CollisionMode::Solid)).expect("known terrain");
    let tile = first_with(&setup, DIRT_PATH, 0);
    for peer in Peer::ALL {
        setup.set_peer(tile, peer, DRY_PATH).expect("known terrain");
    }
    let dry = setup.terrains.iter_mut().find(|t| t.id == DRY_PATH);
    dry.expect("dry path").oklab.x += 0.5;
    let sand = setup.terrain(DRY_PATH).map(|t| t.swatch);

    let after = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    let kept = after.terrain(DRY_PATH).expect("kept for its User values");
    assert_eq!(kept.collision.value, CollisionMode::Solid);
    assert!(after.tiles[&tile].shape.value.is_some());
    assert_eq!(after.terrain(4).map(|t| t.swatch), sand);
    assert_eq!(after.next_id, 5);
}

/// `sheet` drawn twice side by side, as a two-frame animation strip.
fn doubled(sheet: &Sheet) -> Sheet {
    let row = sheet.width as usize * 4;
    let pixels = (sheet.pixels.chunks(row))
        .flat_map(|r| r.iter().chain(r).copied())
        .collect();
    Sheet {
        width: sheet.width * 2,
        height: sheet.height,
        pixels,
    }
}

#[test]
fn a_sheet_that_becomes_a_strip_keeps_its_values() {
    let packed = Sheet::load(PACKED);
    let wide = doubled(&packed);
    let mut setup = TilesetSetup::from_auto(&run(&[("anim.png", &wide)]));
    let tile = first_with(&setup, DRY_PATH, 0);
    setup
        .set_peer(tile, Peer::Top, GRASS)
        .expect("known terrain");
    let both = [("base.png", &packed), ("anim.png", &wide)];
    assert!(run(&both).report.sheets[1].animates.is_some());

    let after = rerun_over(&setup, &both, &Scope::All);
    assert_eq!(after.sheets, ["anim.png", "base.png"]);
    let on = |s: &TilesetSetup, sheet: usize| -> Vec<(TileAddress, SetupTile)> {
        (s.tiles.iter())
            .filter(|(a, _)| a.sheet == sheet)
            .map(|(a, t)| (*a, t.clone()))
            .collect()
    };
    assert_eq!(on(&after, 0), on(&setup, 0));
    assert!(!on(&after, 1).is_empty());
}

#[test]
fn terrain_ids_and_tiles_follow_their_sheets() {
    let packed = Sheet::load(PACKED);
    let spaced = Sheet::load(SPACED);
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &packed), (SPACED, &spaced)]));
    setup.rename_terrain(GRASS, "meadow").expect("known");
    let on_spaced = setup.tiles.keys().copied().find(|a| a.sheet == 1);
    let on_spaced = on_spaced.expect("tiles on the spaced sheet");
    setup
        .set_peer(on_spaced, Peer::Top, DIRT_PATH)
        .expect("known");
    let check = |sheets: &[(&str, &Sheet)]| {
        let after = rerun_over(&setup, sheets, &Scope::All);
        let auto = TilesetSetup::from_auto(&run(sheets));
        assert_eq!(after.terrains.len(), setup.terrains.len());
        for (got, was) in after.terrains.iter().zip(&setup.terrains) {
            assert_eq!(got.id, was.id);
            assert!(got.oklab.distance(was.oklab) < 0.06);
            let fresh = auto.terrains.iter().min_by(|a, b| {
                let (da, db) = (a.oklab.distance(got.oklab), b.oklab.distance(got.oklab));
                da.total_cmp(&db)
            });
            let expected = if got.id == GRASS {
                "meadow".to_string()
            } else {
                fresh.expect("fresh terrain").name.value.clone()
            };
            assert_eq!(got.name.value, expected);
        }
        after
    };
    let reordered = check(&[(SPACED, &spaced), (PACKED, &packed)]);
    assert_eq!(reordered.sheets, setup.sheets);
    assert_eq!(reordered.tiles, setup.tiles);
    let sand = reordered.terrain(DRY_PATH).map(|t| t.name.value.as_str());
    assert_eq!(sand, Some("tilemap sand"));
    let added = check(&[("copy.png", &packed), (PACKED, &packed), (SPACED, &spaced)]);
    assert_eq!(added.sheets, [PACKED, SPACED, "copy.png"]);
    assert_eq!(added.tiles[&on_spaced], setup.tiles[&on_spaced]);
    let single = check(&[(SPACED, &spaced)]);
    assert_eq!(single.sheets, [SPACED]);
    let moved = TileAddress {
        sheet: 0,
        ..on_spaced
    };
    assert_eq!(single.tiles[&moved], setup.tiles[&on_spaced]);
}

#[test]
fn reruns_are_deterministic() {
    let sheet = Sheet::load(PACKED);
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &sheet)]));
    setup
        .set_collision_mode(DRY_PATH, CollisionMode::Solid)
        .expect("known");
    let a = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    let b = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    assert_eq!(a, b);
    assert_eq!(rerun_over(&a, &[(PACKED, &sheet)], &Scope::All), a);
}
