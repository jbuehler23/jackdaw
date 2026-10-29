//! Auto setup over the overworld sheet: the full report is a golden file.

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
    fn packed() -> Self {
        let path = format!(
            "{}/tests/data/punyworld/punyworld-overworld-tileset.png",
            env!("CARGO_MANIFEST_DIR")
        );
        let image = image::open(&path).expect("fixture decodes").to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        }
    }

    /// The packed sheet with a transparent pixel between neighbouring tiles.
    fn spaced() -> Self {
        let packed = Self::packed();
        let (w, h) = (packed.width, packed.height);
        let (sw, sh) = (w / 16 * 17 - 1, h / 16 * 17 - 1);
        let mut pixels = vec![0; (sw * sh * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let from = ((y * w + x) * 4) as usize;
                let to = (((y + y / 16) * sw + x + x / 16) * 4) as usize;
                pixels[to..to + 4].copy_from_slice(&packed.pixels[from..from + 4]);
            }
        }
        Self {
            width: sw,
            height: sh,
            pixels,
        }
    }

    fn image(&self) -> RgbaImage<'_> {
        RgbaImage::new(self.width, self.height, &self.pixels).expect("sized")
    }
}

fn setup(name: &str, sheet: &Sheet) -> AutoSetup {
    auto_setup(&[(name, sheet.image())], &[]).expect("auto setup runs")
}

fn golden() -> String {
    let path = format!(
        "{}/tests/golden/auto_setup/overworld_report.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("golden report")
}

#[test]
fn packed_report_matches_golden() {
    let report = setup(PACKED, &Sheet::packed()).report.to_string();
    assert_eq!(report, golden());
}

#[test]
fn spaced_report_matches_golden_but_its_grid_line() {
    let spaced = setup(SPACED, &Sheet::spaced()).report.to_string();
    let renamed = spaced.replace(&format!("\"{SPACED}\""), &format!("\"{PACKED}\""));
    let golden = golden();
    let (got, want): (Vec<&str>, Vec<&str>) = (renamed.lines().collect(), golden.lines().collect());
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        if w.starts_with("  sheet ") {
            assert!(
                g.contains("margin 0x0, spacing 1x1, 27 columns, 38 rows"),
                "{g}"
            );
        } else {
            assert_eq!(g, w, "line {i}");
        }
    }
}

#[test]
fn stamps_follow_the_art() {
    let found = setup(PACKED, &Sheet::packed());
    let stamps: Vec<Vec<u32>> = (found.stamps.iter())
        .map(|s| s.cells.iter().map(|(_, tile)| *tile).collect())
        .collect();
    assert_eq!(stamps.len(), 34);
    // The first three objects; cells of their bounds that no member fills
    // are gaps.
    assert_eq!(
        stamps[..3],
        [
            vec![
                108, 109, 110, 111, 112, 135, 137, 138, 139, 162, 163, 164, 165, 166
            ],
            vec![119, 120, 121, 146, 148, 173, 174, 175],
            vec![122, 123, 149, 150],
        ]
    );
    let block = &found.stamps[0];
    assert_eq!(
        (block.origin.to_array(), block.size.to_array()),
        ([0, 4], [5, 3])
    );
}

#[test]
fn absorbed_terrains_leave_the_proposal() {
    let found = setup(PACKED, &Sheet::packed());
    let kept: Vec<u8> = found.proposal.terrains.iter().map(|t| t.id).collect();
    assert_eq!(kept, [1, 2, 5, 6, 7, 9, 11, 12]);
    assert!(found.analysis.terrains.len() > kept.len());
    let names: Vec<&str> = found
        .analysis
        .terrains
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    // Named by hue: the yellow-olive dirt and sand ground reads as grass.
    assert_eq!(names[..3], ["grass", "grass 2", "grass 6"]);
    let distinct: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(distinct.len(), names.len(), "{names:?}");
    assert_eq!(found.sheets, [0]);
    let unassigned: Vec<u32> = found.proposal.unassigned.iter().map(|a| a.tile).collect();
    assert_eq!(found.report.unassigned, [(PACKED.to_string(), unassigned)]);
}

#[test]
fn collision_is_reported_only_for_solid_terrains() {
    let sheet = Sheet::packed();
    let found = auto_setup(&[(PACKED, sheet.image())], &[GROUND, WATER]).expect("runs");
    let text = found.report.to_string();
    let last = text.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("collision: solid terrains 2 11, "),
        "{text}"
    );
    assert!(!golden().contains("collision"));
}

const PACKED: &str = "overworld.png";
const SPACED: &str = "overworld_spaced.png";
const GRASS: u8 = 1;
const GROUND: u8 = 2;
const WATER: u8 = 11;
const DEEP_SEA: u8 = 12;

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
    let sheet = Sheet::packed();
    let fresh = run(&[(PACKED, &sheet)]);
    let mut setup = TilesetSetup::from_auto(&fresh);
    let mut user_bits = Vec::new();
    for (i, peer) in [Peer::Top, Peer::Left, Peer::BottomRight]
        .into_iter()
        .enumerate()
    {
        let address = first_with(&setup, GROUND, i);
        let auto = setup.tiles[&address].terrains.get(peer);
        let hand = if auto == GRASS { GROUND } else { GRASS };
        setup.set_peer(address, peer, hand).expect("known terrain");
        user_bits.push((address, peer, hand));
    }
    let cleared = first_with(&setup, GROUND, 3);
    for peer in Peer::ALL {
        if setup.tiles[&cleared].terrains.get(peer) == GROUND {
            setup.set_peer(cleared, peer, GRASS).expect("known terrain");
            user_bits.push((cleared, peer, GRASS));
        }
    }
    let painted = first_with(&setup, WATER, 0);
    for peer in Peer::ALL {
        setup
            .set_peer(painted, peer, GROUND)
            .expect("known terrain");
        user_bits.push((painted, peer, GROUND));
    }
    setup
        .rename_terrain(GRASS, "meadow")
        .expect("known terrain");
    (setup.set_collision_mode(GROUND, CollisionMode::Solid)).expect("known terrain");

    let after = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);

    for (address, peer, hand) in &user_bits {
        let tile = &after.tiles[address];
        assert_eq!(tile.terrains.get(*peer), *hand);
        assert_eq!(tile.provenance[peer.index()], Provenance::User);
    }
    let grass = after.terrain(GRASS).expect("grass kept");
    assert_eq!(grass.name, Tracked::user("meadow".to_string()));
    let ground = after.terrain(GROUND).expect("ground kept");
    assert_eq!(ground.collision, Tracked::user(CollisionMode::Solid));

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
            assert!(uses(tile, GROUND), "{address:?}");
        } else {
            // Unread sides of a sea tile over the ground take the ground too, so
            // only a ground centre makes an untouched tile solid.
            let all_auto = !tile.provenance.contains(&Provenance::User);
            let centre = tile.terrains.centre() == GROUND;
            assert!(!(all_auto && centre), "{address:?}");
        }
    }
    assert!(after.tiles[&cleared].shape.value.is_none());
    assert!(after.tiles[&painted].shape.value.is_some());
}

#[test]
fn an_imported_solid_terrain_stays_solid_with_its_shapes() {
    let sheet = Sheet::packed();
    let inputs = [(PACKED, sheet.image())];
    let fresh = auto_setup(&inputs, &[WATER]).expect("runs");
    let setup = TilesetSetup::from_auto(&fresh);
    let solid = Tracked::auto(CollisionMode::Solid);
    assert_eq!(setup.terrain(WATER).map(|t| &t.collision), Some(&solid));
    let after = rerun(&setup, &inputs, &Scope::All).expect("re-run");
    assert_eq!(after.terrain(WATER).map(|t| &t.collision), Some(&solid));
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
    let sheet = Sheet::packed();
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &sheet)]));
    (setup.set_collision_mode(GROUND, CollisionMode::Solid)).expect("known terrain");
    let tile = first_with(&setup, WATER, 0);
    for peer in Peer::ALL {
        setup.set_peer(tile, peer, GROUND).expect("known terrain");
    }
    let ground = setup.terrains.iter_mut().find(|t| t.id == GROUND);
    ground.expect("ground").oklab.x += 0.5;
    let swatch = setup.terrain(GROUND).map(|t| t.swatch);
    let fresh = u8::try_from(setup.next_id).expect("a terrain id");

    let after = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    let kept = after.terrain(GROUND).expect("kept for its User values");
    assert_eq!(kept.collision.value, CollisionMode::Solid);
    assert!(after.tiles[&tile].shape.value.is_some());
    assert_eq!(after.terrain(fresh).map(|t| t.swatch), swatch);
    assert_eq!(after.next_id, setup.next_id + 1);
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
    let packed = Sheet::packed();
    let wide = doubled(&packed);
    let mut setup = TilesetSetup::from_auto(&run(&[("anim.png", &wide)]));
    let tile = first_with(&setup, GROUND, 0);
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
    let packed = Sheet::packed();
    let spaced = Sheet::spaced();
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &packed), (SPACED, &spaced)]));
    setup.rename_terrain(GRASS, "meadow").expect("known");
    let on_spaced = setup.tiles.keys().copied().find(|a| a.sheet == 1);
    let on_spaced = on_spaced.expect("tiles on the spaced sheet");
    setup.set_peer(on_spaced, Peer::Top, WATER).expect("known");
    // Some colours seed a terrain only when two copies of the sheet are
    // analysed together, so a re-run over one copy keeps fewer terrains;
    // every terrain it keeps has its old id.
    let check = |sheets: &[(&str, &Sheet)], kept: usize| {
        let after = rerun_over(&setup, sheets, &Scope::All);
        let auto = TilesetSetup::from_auto(&run(sheets));
        assert_eq!((setup.terrains.len(), after.terrains.len()), (11, kept));
        for got in &after.terrains {
            let was = setup.terrain(got.id).expect("an old id");
            assert!(got.oklab.distance(was.oklab) < 0.06);
            let fresh = auto.terrains.iter().min_by(|a, b| {
                let (da, db) = (a.oklab.distance(got.oklab), b.oklab.distance(got.oklab));
                da.total_cmp(&db)
            });
            let fresh = fresh.filter(|f| f.oklab.distance(got.oklab) < 0.06);
            let expected = if got.id == GRASS {
                "meadow".to_string()
            } else if let Some(fresh) = fresh {
                fresh.name.value.clone()
            } else {
                // Seeded only by the two copies together, so a fresh run
                // over one copy has no such terrain to name it after.
                continue;
            };
            assert_eq!(got.name.value, expected, "terrain {}", got.id);
        }
        after
    };
    let reordered = check(&[(SPACED, &spaced), (PACKED, &packed)], 11);
    assert_eq!(reordered.sheets, setup.sheets);
    assert_eq!(reordered.tiles, setup.tiles);
    let ground = reordered.terrain(GROUND).map(|t| t.name.value.as_str());
    assert_eq!(ground, Some("grass 2"));
    let added = check(
        &[("copy.png", &packed), (PACKED, &packed), (SPACED, &spaced)],
        11,
    );
    assert_eq!(added.sheets, [PACKED, SPACED, "copy.png"]);
    assert_eq!(added.tiles[&on_spaced], setup.tiles[&on_spaced]);
    let single = check(&[(SPACED, &spaced)], 9);
    assert_eq!(single.sheets, [SPACED]);
    let moved = TileAddress {
        sheet: 0,
        ..on_spaced
    };
    assert_eq!(single.tiles[&moved], setup.tiles[&on_spaced]);
}

#[test]
fn reruns_are_deterministic() {
    let sheet = Sheet::packed();
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &sheet)]));
    setup
        .set_collision_mode(GROUND, CollisionMode::Solid)
        .expect("known");
    let a = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    let b = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    assert_eq!(a, b);
    assert_eq!(rerun_over(&a, &[(PACKED, &sheet)], &Scope::All), a);
}

#[test]
fn auto_names_step_around_user_names_and_stay_put() {
    let sheet = Sheet::packed();
    let mut setup = TilesetSetup::from_auto(&run(&[(PACKED, &sheet)]));
    setup
        .rename_terrain(GROUND, "grass")
        .expect("known terrain");
    let name = |s: &TilesetSetup, id| s.terrain(id).map(|t| t.name.clone());
    let once = rerun_over(&setup, &[(PACKED, &sheet)], &Scope::All);
    assert_eq!(
        name(&once, GRASS),
        Some(Tracked::auto("grass 2".to_string()))
    );
    assert_eq!(
        name(&once, GROUND),
        Some(Tracked::user("grass".to_string()))
    );
    let twice = rerun_over(&once, &[(PACKED, &sheet)], &Scope::All);
    assert_eq!(name(&twice, GRASS), name(&once, GRASS));

    let partial = Scope::Set {
        inner: DEEP_SEA,
        outer: WATER,
    };
    let kept = rerun_over(&once, &[(PACKED, &sheet)], &partial);
    assert_eq!(
        name(&kept, GRASS),
        Some(Tracked::auto("grass 2".to_string()))
    );
    let mut out_of_scope = TilesetSetup::from_auto(&run(&[(PACKED, &sheet)]));
    out_of_scope.terrains[0].name = Tracked::auto("grass 6".to_string());
    let after = rerun_over(&out_of_scope, &[(PACKED, &sheet)], &partial);
    let names: Vec<String> = after
        .terrains
        .iter()
        .map(|t| t.name.value.clone())
        .collect();
    assert_eq!(
        names,
        [
            "grass 6", "grass 2", "grass 3", "grass 4", "dirt", "grass 5", "water", "water 2"
        ]
    );
    let all = rerun_over(&out_of_scope, &[(PACKED, &sheet)], &Scope::All);
    assert_eq!(name(&all, GRASS), Some(Tracked::auto("grass".to_string())));
}
