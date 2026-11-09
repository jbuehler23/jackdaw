use std::collections::BTreeSet;

use crate::setup::transition;
use crate::terrain_find::srgb_to_oklab;
use crate::{
    CollisionMode, MatchMode, Peer, Provenance, SetupError, SetupSet, SetupTerrain, Stamp,
    TerrainId, TileAddress, TileTerrains, TilesetSetup, Tracked,
};

impl TilesetSetup {
    /// Adds a terrain by hand with the next free id.
    pub fn add_terrain(
        &mut self,
        name: impl Into<String>,
        swatch: [u8; 3],
        background: bool,
    ) -> Result<TerrainId, SetupError> {
        let id = self.take_id()?;
        let [r, g, b] = swatch;
        self.terrains.push(SetupTerrain {
            id,
            name: Tracked::user(name.into()),
            oklab: srgb_to_oklab([r, g, b, 255]),
            swatch,
            background,
            collision: Tracked::user(CollisionMode::default()),
        });
        Ok(id)
    }

    /// Removes a terrain: its peers become empty by hand and it leaves its
    /// sets. A background terrain takes the flag with it; no other terrain
    /// gets it.
    pub fn remove_terrain(&mut self, id: TerrainId) -> Result<(), SetupError> {
        self.known(id)?;
        self.replace_peers(id, 0, None);
        for set in &mut self.sets {
            set.terrains.retain(|t| *t != id);
        }
        self.terrains.retain(|t| t.id != id);
        Ok(())
    }

    /// Moves the peers of `id` on `tiles` to a new terrain like it, named
    /// `name`. The new terrain joins the set of `id`, just above it, when one
    /// of the tiles is now a transition of that set.
    pub fn split_terrain(
        &mut self,
        id: TerrainId,
        tiles: &BTreeSet<TileAddress>,
        name: impl Into<String>,
    ) -> Result<TerrainId, SetupError> {
        let source = self.known(id)?.clone();
        for address in tiles {
            let holds = self
                .tiles
                .get(address)
                .is_some_and(|t| t.terrains.peers.contains(&id));
            if !holds {
                return Err(SetupError::NotOnTile(*address, id));
            }
        }
        let new = self.take_id()?;
        self.terrains.push(SetupTerrain {
            id: new,
            name: Tracked::user(name.into()),
            background: false,
            ..source
        });
        self.replace_peers(id, new, Some(tiles));
        let joined = (self.sets.iter()).position(|s| {
            let Some(at) = s.terrains.iter().position(|t| *t == id) else {
                return false;
            };
            let mut terrains = s.terrains.clone();
            terrains.insert(at + 1, new);
            (self.tiles.values())
                .any(|t| t.terrains.peers.contains(&new) && transition(&terrains, &t.terrains))
        });
        if let Some(set) = joined.and_then(|i| self.sets.get_mut(i))
            && let Some(at) = set.terrains.iter().position(|t| *t == id)
        {
            set.terrains.insert(at + 1, new);
        }
        Ok(new)
    }

    /// Folds `from` into `into`: its peers take `into`, and in its sets
    /// `into` takes its place unless `into` already has a set of its own,
    /// which wins.
    pub fn merge_terrains(&mut self, from: TerrainId, into: TerrainId) -> Result<(), SetupError> {
        if from == into {
            return Err(SetupError::SameTerrain(from));
        }
        self.known(from)?;
        self.known(into)?;
        self.replace_peers(from, into, None);
        let owned = self.sets.iter().any(|s| s.terrains.contains(&into));
        for set in &mut self.sets {
            let Some(at) = set.terrains.iter().position(|t| *t == from) else {
                continue;
            };
            if owned || set.terrains.contains(&into) {
                set.terrains.remove(at);
            } else {
                set.terrains[at] = into;
            }
        }
        let background = self.known(from)?.background;
        self.terrains.retain(|t| t.id != from);
        if background && let Some(kept) = self.terrains.iter_mut().find(|t| t.id == into) {
            kept.background = true;
        }
        Ok(())
    }

    /// Adds an empty terrain set by hand, named `name` or, when it is empty,
    /// the next free `Terrain set n`. Returns its index.
    pub fn add_set(&mut self, name: &str, mode: MatchMode) -> Result<usize, SetupError> {
        let name = match name.trim() {
            "" => self.next_set_name(),
            name => name.to_string(),
        };
        if let Some(taken) = self.set_named(&name) {
            return Err(SetupError::SetNameTaken(taken));
        }
        self.sets.push(SetupSet {
            name,
            mode: Tracked::user(mode),
            terrains: Vec::new(),
            locked_manual: false,
        });
        Ok(self.sets.len() - 1)
    }

    /// Renames the set at `set`; names are unique.
    pub fn rename_set(&mut self, set: usize, name: &str) -> Result<(), SetupError> {
        let name = name.trim();
        if let Some(taken) = self.set_named(name).filter(|t| *t != set) {
            return Err(SetupError::SetNameTaken(taken));
        }
        self.set_mut(set)?.name = name.to_string();
        Ok(())
    }

    /// Puts terrain `terrain`, which belongs to no set, in the set at `set`
    /// at stacking position `at` (bottom 0), or on top when `None`.
    pub fn add_set_terrain(
        &mut self,
        set: usize,
        terrain: TerrainId,
        at: Option<usize>,
    ) -> Result<(), SetupError> {
        self.join_set(set, terrain, at, false)
    }

    /// [`Self::add_set_terrain`], letting `terrain` keep sets of other modes
    /// when `other_modes` is set.
    pub(crate) fn join_set(
        &mut self,
        set: usize,
        terrain: TerrainId,
        at: Option<usize>,
        other_modes: bool,
    ) -> Result<(), SetupError> {
        let mode = self.set_mut(set)?.mode.value;
        self.known(terrain)?;
        let clashes =
            |s: &SetupSet| s.terrains.contains(&terrain) && (!other_modes || s.mode.value == mode);
        if let Some(other) = (self.sets.iter()).position(clashes) {
            return Err(SetupError::TerrainInSet {
                terrain,
                set: other,
            });
        }
        let members = self.set_mut(set)?;
        let at = at
            .unwrap_or(members.terrains.len())
            .min(members.terrains.len());
        members.terrains.insert(at, terrain);
        let members = members.terrains.clone();
        (self.removed_sets)
            .retain(|&(a, b)| !(members.contains(&a) && (b == 0 || members.contains(&b))));
        Ok(())
    }

    /// Takes terrain `terrain` out of the set at `set`, remembered so a re-run
    /// does not put it back; tiles laid out for its transitions there leave
    /// them.
    pub fn remove_set_terrain(&mut self, set: usize, terrain: TerrainId) -> Result<(), SetupError> {
        let members = self.set_mut(set)?;
        let at = (members.terrains.iter().position(|t| *t == terrain))
            .ok_or(SetupError::NotInSet { terrain, set })?;
        members.terrains.remove(at);
        let others: Vec<TerrainId> = std::iter::once(0).chain(members.terrains.clone()).collect();
        for other in others {
            self.removed_sets.insert((terrain, other));
        }
        for tile in self.tiles.values_mut() {
            if tile.set.is_some_and(|(a, b)| a == terrain || b == terrain) {
                tile.set = None;
            }
        }
        Ok(())
    }

    /// Moves terrain `terrain` of the set at `set` to stacking position `to`
    /// (bottom 0), clamped to the set.
    pub fn move_set_terrain(
        &mut self,
        set: usize,
        terrain: TerrainId,
        to: usize,
    ) -> Result<(), SetupError> {
        let members = self.set_mut(set)?;
        let at = (members.terrains.iter().position(|t| *t == terrain))
            .ok_or(SetupError::NotInSet { terrain, set })?;
        members.terrains.remove(at);
        let to = to.min(members.terrains.len());
        members.terrains.insert(to, terrain);
        Ok(())
    }

    /// Removes the set at `set`, its transitions remembered so a re-run does
    /// not propose them again; tiles laid out for it keep their peers and
    /// leave it.
    pub fn remove_set(&mut self, set: usize) -> Result<SetupSet, SetupError> {
        self.set_mut(set)?;
        let removed = self.sets.remove(set);
        for (pair, _) in crate::setup::stacked_pairs(&removed) {
            self.removed_sets.insert(pair);
        }
        for tile in self.tiles.values_mut() {
            if tile
                .set
                .is_some_and(|(a, b)| removed.has(a) && removed.has(b))
            {
                tile.set = None;
            }
        }
        Ok(removed)
    }

    /// Sets every peer of `tile` by hand; a tile of a stamp is refused.
    pub fn set_peers(&mut self, tile: TileAddress, peers: TileTerrains) -> Result<(), SetupError> {
        self.check_loose(tile)?;
        for id in peers.peers {
            if id != 0 {
                self.known(id)?;
            }
        }
        let entry = self.tiles.entry(tile).or_default();
        entry.terrains = peers;
        entry.provenance = [Provenance::User; 9];
        entry.drop_set_it_left();
        Ok(())
    }

    /// Sets one peer of `tile` by hand, leaving who set the others; a tile
    /// of a stamp is refused.
    pub fn set_peer_by_hand(
        &mut self,
        tile: TileAddress,
        peer: Peer,
        terrain: TerrainId,
    ) -> Result<(), SetupError> {
        self.check_loose(tile)?;
        self.set_peer(tile, peer, terrain)
    }

    /// The stamp holding `tile`, if any.
    pub fn stamp_holding(&self, tile: TileAddress) -> Option<&Stamp> {
        (self.stamps.iter())
            .map(|s| &s.value)
            .find(|s| s.tiles().any(|a| a == tile))
    }

    /// Refuses a tile that belongs to a stamp.
    pub(crate) fn check_loose(&self, tile: TileAddress) -> Result<(), SetupError> {
        match self.stamp_holding(tile) {
            Some(stamp) => Err(SetupError::InObject {
                tile,
                origin: stamp.origin,
            }),
            None => Ok(()),
        }
    }

    fn known(&self, id: TerrainId) -> Result<&SetupTerrain, SetupError> {
        self.terrain(id).ok_or(SetupError::UnknownTerrain(id))
    }

    fn take_id(&mut self) -> Result<TerrainId, SetupError> {
        let next = self.next_id.max(1);
        let id = TerrainId::try_from(next).map_err(|_| SetupError::TooManyTerrains)?;
        self.next_id = next + 1;
        Ok(id)
    }

    /// Sets each peer of `from` to `to` by hand, on `only` or on every tile.
    fn replace_peers(
        &mut self,
        from: TerrainId,
        to: TerrainId,
        only: Option<&BTreeSet<TileAddress>>,
    ) {
        for (address, tile) in &mut self.tiles {
            if only.is_some_and(|only| !only.contains(address)) {
                continue;
            }
            for peer in Peer::ALL {
                if tile.terrains.get(peer) == from {
                    tile.terrains = tile.terrains.with(peer, to);
                    tile.provenance[peer.index()] = Provenance::User;
                }
            }
            tile.drop_set_it_left();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use glam::UVec2;

    use super::*;
    use crate::SetupTile;

    pub(crate) const GRASS: TerrainId = 1;
    pub(crate) const SAND: TerrainId = 2;

    pub(crate) fn at(tile: u32) -> TileAddress {
        TileAddress { sheet: 0, tile }
    }

    pub(crate) fn edge(inner: TerrainId, outer: TerrainId) -> TileTerrains {
        TileTerrains::filled(inner).with(Peer::Top, outer)
    }

    /// A new set of `lower` then `upper`, skipping the empty terrain; either
    /// may keep a set of another mode.
    pub(crate) fn pair_set(
        setup: &mut TilesetSetup,
        upper: TerrainId,
        lower: TerrainId,
        mode: MatchMode,
    ) -> usize {
        let set = setup.add_set("", mode).expect("a set");
        for terrain in [lower, upper].into_iter().filter(|t| *t != 0) {
            setup
                .join_set(set, terrain, None, true)
                .expect("free in this mode");
        }
        set
    }

    /// Grass and sand with a set of sand on grass, a pure grass tile 0, a
    /// transition tile 1 and a pure sand tile 2.
    pub(crate) fn farm() -> TilesetSetup {
        let mut setup = TilesetSetup {
            sheets: vec!["farm.png".into()],
            next_id: 1,
            ..TilesetSetup::default()
        };
        assert_eq!(setup.add_terrain("grass", [0, 200, 0], true), Ok(GRASS));
        assert_eq!(setup.add_terrain("sand", [230, 160, 100], false), Ok(SAND));
        pair_set(&mut setup, SAND, GRASS, MatchMode::Sides);
        for (tile, peers) in [
            (0, TileTerrains::filled(GRASS)),
            (1, edge(SAND, GRASS)),
            (2, TileTerrains::filled(SAND)),
        ] {
            let auto = SetupTile {
                terrains: peers,
                ..SetupTile::default()
            };
            setup.tiles.insert(at(tile), auto);
        }
        setup
    }

    #[test]
    fn a_hand_edit_that_leaves_a_tiles_set_forgets_it() {
        let mut setup = farm();
        let water = setup
            .add_terrain("water", [0, 0, 255], false)
            .expect("an id");
        setup.tiles.get_mut(&at(1)).expect("tile").set = Some((SAND, GRASS));
        setup.set_peer(at(1), Peer::Top, SAND).expect("peer");
        assert_eq!(setup.tiles[&at(1)].set, Some((SAND, GRASS)), "still within");
        setup.set_peer(at(1), Peer::Top, water).expect("peer");
        assert_eq!(setup.tiles[&at(1)].set, None);
        setup.tiles.get_mut(&at(2)).expect("tile").set = Some((SAND, GRASS));
        setup
            .set_peers(at(2), TileTerrains::filled(water))
            .expect("peers");
        assert_eq!(setup.tiles[&at(2)].set, None);
        setup.tiles.get_mut(&at(0)).expect("tile").set = Some((SAND, GRASS));
        setup.merge_terrains(GRASS, water).expect("merged");
        assert_eq!(
            setup.tiles[&at(0)].set,
            None,
            "a merge that moves it off its set"
        );
    }

    #[test]
    fn added_terrains_take_fresh_ids_until_they_run_out() {
        let mut setup = farm();
        let id = setup
            .add_terrain("water", [0, 0, 255], false)
            .expect("an id");
        assert_eq!((id, setup.next_id), (3, 4));
        let water = setup.terrain(id).expect("added");
        assert!(water.name.is_user() && water.oklab.x > 0.0);
        setup.next_id = 255;
        assert_eq!(setup.add_terrain("last", [0; 3], false), Ok(255));
        assert_eq!(
            setup.add_terrain("one more", [0; 3], false),
            Err(SetupError::TooManyTerrains)
        );
        let mut fresh = TilesetSetup::default();
        assert_eq!(
            fresh.add_terrain("first", [0; 3], false),
            Ok(1),
            "0 is the empty terrain"
        );
        assert_eq!(fresh.next_id, 2);
    }

    #[test]
    fn removing_a_terrain_empties_its_peers_by_hand_and_takes_it_out_of_its_set() {
        let mut setup = farm();
        setup.remove_terrain(SAND).expect("removed");
        assert!(setup.terrain(SAND).is_none());
        assert_eq!(setup.sets[0].terrains, [GRASS]);
        let edge = &setup.tiles[&at(1)];
        assert_eq!(
            edge.terrains,
            TileTerrains::filled(0).with(Peer::Top, GRASS)
        );
        assert_eq!(edge.provenance[Peer::Centre.index()], Provenance::User);
        assert_eq!(edge.provenance[Peer::Top.index()], Provenance::Auto);
        assert_eq!(setup.next_id, 3, "ids are never reused");
        assert_eq!(
            setup.remove_terrain(SAND),
            Err(SetupError::UnknownTerrain(SAND))
        );
    }

    #[test]
    fn a_split_moves_the_chosen_tiles_and_joins_the_set_they_border() {
        let mut setup = farm();
        let tiles: BTreeSet<_> = [at(1)].into();
        let wet = setup
            .split_terrain(SAND, &tiles, "wet sand")
            .expect("split");
        assert_eq!(wet, 3);
        assert_eq!(setup.tiles[&at(1)].terrains, edge(wet, GRASS));
        assert_eq!(setup.tiles[&at(2)].terrains, TileTerrains::filled(SAND));
        assert_eq!(setup.sets.len(), 1);
        assert_eq!(
            setup.sets[0].terrains,
            [GRASS, SAND, wet],
            "just above sand"
        );
        assert_eq!(setup.terrain(wet).expect("new").swatch, [230, 160, 100]);

        let refused = setup.split_terrain(SAND, &[at(0)].into(), "x");
        assert_eq!(refused, Err(SetupError::NotOnTile(at(0), SAND)));
    }

    #[test]
    fn a_merge_folds_peers_and_its_own_set_wins() {
        let mut setup = farm();
        let wet = setup.add_terrain("wet", [1, 2, 3], false).expect("id");
        pair_set(&mut setup, wet, 0, MatchMode::Corners);
        setup.set_peers(at(3), edge(wet, GRASS)).expect("bits");

        setup.merge_terrains(wet, SAND).expect("merged");
        assert!(setup.terrain(wet).is_none());
        assert_eq!(setup.tiles[&at(3)].terrains, edge(SAND, GRASS));
        let sets: Vec<_> = (setup.sets.iter())
            .map(|s| (s.terrains.clone(), s.mode.value))
            .collect();
        assert_eq!(
            sets,
            [
                (vec![GRASS, SAND], MatchMode::Sides),
                (vec![], MatchMode::Corners)
            ],
            "sand's own set wins"
        );
        let mut setup = farm();
        let wet = setup.add_terrain("wet", [1, 2, 3], false).expect("id");
        let alone = pair_set(&mut setup, wet, 0, MatchMode::Corners);
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        setup.merge_terrains(wet, water).expect("merged");
        assert_eq!(setup.sets[alone].terrains, [water], "takes its place");
        assert_eq!(
            setup.merge_terrains(SAND, SAND),
            Err(SetupError::SameTerrain(SAND))
        );
    }

    #[test]
    fn a_new_set_takes_a_free_default_name_and_each_terrain_once() {
        let mut setup = farm();
        assert_eq!(setup.sets[0].name, "Terrain set 1");
        let added = setup.add_set("", MatchMode::Corners).expect("a set");
        assert_eq!(added, 1);
        assert_eq!(setup.sets[1].name, "Terrain set 2");
        assert!(setup.sets[1].mode.is_user() && setup.sets[1].terrains.is_empty());
        assert_eq!(
            setup.add_set("Terrain set 1", MatchMode::Corners),
            Err(SetupError::SetNameTaken(0))
        );
        assert_eq!(
            setup.add_set_terrain(1, GRASS, None),
            Err(SetupError::TerrainInSet {
                terrain: GRASS,
                set: 0
            })
        );
        assert_eq!(
            setup.add_set_terrain(1, 9, None),
            Err(SetupError::UnknownTerrain(9))
        );
        assert_eq!(
            setup.add_set_terrain(4, GRASS, None),
            Err(SetupError::UnknownSet(4))
        );
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        setup.add_set_terrain(1, water, None).expect("free");
        assert_eq!(setup.set_index(water, 0), Some(1), "on empty");
    }

    #[test]
    fn a_set_is_renamed_and_its_terrains_reordered() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        setup.add_set_terrain(0, water, Some(1)).expect("free");
        assert_eq!(setup.sets[0].terrains, [GRASS, water, SAND]);
        setup.move_set_terrain(0, water, 9).expect("held");
        assert_eq!(
            setup.sets[0].terrains,
            [GRASS, SAND, water],
            "clamped on top"
        );
        setup.move_set_terrain(0, water, 0).expect("held");
        assert_eq!(setup.sets[0].terrains, [water, GRASS, SAND]);
        assert_eq!(
            setup.move_set_terrain(0, 9, 0),
            Err(SetupError::NotInSet { terrain: 9, set: 0 })
        );
        setup.rename_set(0, " Ground ").expect("free");
        assert_eq!(setup.sets[0].name, "Ground");
        setup.add_set("Paths", MatchMode::Sides).expect("a set");
        assert_eq!(
            setup.rename_set(0, "Paths"),
            Err(SetupError::SetNameTaken(1))
        );
        assert_eq!(setup.rename_set(0, "Ground"), Ok(()), "its own name");
    }

    #[test]
    fn a_terrain_taken_out_of_a_set_is_not_proposed_again() {
        let mut setup = farm();
        setup.tiles.get_mut(&at(1)).expect("tile").set = Some((SAND, GRASS));
        setup.remove_set_terrain(0, SAND).expect("held");
        assert_eq!(setup.sets[0].terrains, [GRASS]);
        assert!(setup.removed_sets.contains(&(SAND, GRASS)));
        assert_eq!(setup.tiles[&at(1)].set, None, "it leaves the transition");
        setup.add_set_terrain(0, SAND, None).expect("free again");
        assert!(setup.removed_sets.is_empty(), "added back by hand");
        let removed = setup.remove_set(0).expect("known");
        assert_eq!(removed.terrains, [GRASS, SAND]);
        assert!(setup.sets.is_empty());
        assert!(setup.removed_sets.contains(&(SAND, GRASS)));
        assert!(
            setup.removed_sets.contains(&(GRASS, 0)),
            "and each on empty"
        );
        assert_eq!(setup.remove_set(0), Err(SetupError::UnknownSet(0)));
    }

    #[test]
    fn set_peers_marks_every_peer_by_hand_and_refuses_unknown_terrains() {
        let mut setup = farm();
        setup.set_peers(at(5), edge(GRASS, 0)).expect("bits");
        assert_eq!(setup.tiles[&at(5)].provenance, [Provenance::User; 9]);
        assert_eq!(
            setup.set_peers(at(5), TileTerrains::filled(7)),
            Err(SetupError::UnknownTerrain(7))
        );
        assert_eq!(setup.tiles[&at(5)].terrains, edge(GRASS, 0));
    }

    #[test]
    fn a_merge_hands_the_background_on_and_a_removal_takes_it_away() {
        let mut setup = farm();
        setup.merge_terrains(GRASS, SAND).expect("merged");
        assert!(setup.terrain(SAND).expect("kept").background);
        let mut setup = farm();
        setup.remove_terrain(GRASS).expect("removed");
        assert!(setup.terrains.iter().all(|t| !t.background));
    }

    #[test]
    fn one_peer_by_hand_leaves_the_others_auto() {
        let mut setup = farm();
        setup.set_peer_by_hand(at(1), Peer::Top, SAND).expect("bit");
        let tile = &setup.tiles[&at(1)];
        assert_eq!(tile.terrains, TileTerrains::filled(SAND));
        assert_eq!(tile.provenance[Peer::Top.index()], Provenance::User);
        assert_eq!(tile.provenance[Peer::Centre.index()], Provenance::Auto);
    }

    #[test]
    fn bits_of_a_stamp_tile_are_refused() {
        let mut setup = farm();
        let stamp = Stamp {
            sheet: 0,
            origin: UVec2::new(1, 0),
            size: UVec2::new(1, 1),
            cells: vec![(UVec2::ZERO, 1)],
        };
        setup.put_stamp(stamp);
        let refused = Err(SetupError::InObject {
            tile: at(1),
            origin: UVec2::new(1, 0),
        });
        assert_eq!(setup.set_peers(at(1), TileTerrains::filled(SAND)), refused);
        assert_eq!(setup.set_peer_by_hand(at(1), Peer::Top, SAND), refused);
        assert_eq!(setup.tiles[&at(1)].terrains, edge(SAND, GRASS));
    }
}
