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

    /// Removes a terrain: its peers become empty by hand and its sets go. A
    /// background terrain takes the flag with it; no other terrain gets it.
    pub fn remove_terrain(&mut self, id: TerrainId) -> Result<(), SetupError> {
        self.known(id)?;
        self.replace_peers(id, 0, None);
        self.sets.retain(|s| s.inner != id && s.outer != id);
        self.terrains.retain(|t| t.id != id);
        Ok(())
    }

    /// Moves the peers of `id` on `tiles` to a new terrain like it, named
    /// `name`, with a copy of each set of `id` that one of them now borders.
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
        let copies: Vec<SetupSet> = (self.sets.iter())
            .filter(|s| s.inner == id || s.outer == id)
            .map(|s| SetupSet {
                inner: if s.inner == id { new } else { s.inner },
                outer: if s.outer == id { new } else { s.outer },
                ..s.clone()
            })
            .filter(|s| {
                let pair = (s.inner, s.outer);
                self.tiles.values().any(|t| transition(pair, &t.terrains))
            })
            .collect();
        self.sets.extend(copies);
        self.sets.sort_by_key(|s| (s.inner, s.outer));
        Ok(new)
    }

    /// Folds `from` into `into`: its peers and sets take `into`, a set of
    /// `into` over itself goes, and `into`'s own sets win over the copies.
    pub fn merge_terrains(&mut self, from: TerrainId, into: TerrainId) -> Result<(), SetupError> {
        if from == into {
            return Err(SetupError::SameTerrain(from));
        }
        self.known(from)?;
        self.known(into)?;
        self.replace_peers(from, into, None);
        let swap = |id: TerrainId| if id == from { into } else { id };
        let (moved, mut sets): (Vec<SetupSet>, Vec<SetupSet>) =
            (self.sets.drain(..)).partition(|s| s.inner == from || s.outer == from);
        for set in moved {
            let (inner, outer) = (swap(set.inner), swap(set.outer));
            let taken = sets.iter().any(|s| (s.inner, s.outer) == (inner, outer));
            if inner != outer && !taken {
                sets.push(SetupSet {
                    inner,
                    outer,
                    ..set
                });
            }
        }
        sets.sort_by_key(|s| (s.inner, s.outer));
        self.sets = sets;
        let background = self.known(from)?.background;
        self.terrains.retain(|t| t.id != from);
        if background && let Some(kept) = self.terrains.iter_mut().find(|t| t.id == into) {
            kept.background = true;
        }
        Ok(())
    }

    /// Adds a terrain set by hand; `outer` may be 0, the empty terrain.
    pub fn add_set(
        &mut self,
        inner: TerrainId,
        outer: TerrainId,
        mode: MatchMode,
    ) -> Result<(), SetupError> {
        if inner == outer {
            return Err(SetupError::SameTerrain(inner));
        }
        self.known(inner)?;
        if outer != 0 {
            self.known(outer)?;
        }
        if self.set(inner, outer).is_some() {
            return Err(SetupError::SetExists(inner, outer));
        }
        self.sets.push(SetupSet {
            inner,
            outer,
            mode: Tracked::user(mode),
            locked_manual: false,
        });
        self.sets.sort_by_key(|s| (s.inner, s.outer));
        Ok(())
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
        let id = TerrainId::try_from(self.next_id).map_err(|_| SetupError::TooManyTerrains)?;
        if id == 0 {
            return Err(SetupError::TooManyTerrains);
        }
        self.next_id += 1;
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

    /// Grass and sand with a sand-over-grass set, a pure grass tile 0, a
    /// transition tile 1 and a pure sand tile 2.
    pub(crate) fn farm() -> TilesetSetup {
        let mut setup = TilesetSetup {
            sheets: vec!["farm.png".into()],
            next_id: 1,
            ..TilesetSetup::default()
        };
        assert_eq!(setup.add_terrain("grass", [0, 200, 0], true), Ok(GRASS));
        assert_eq!(setup.add_terrain("sand", [230, 160, 100], false), Ok(SAND));
        setup.add_set(SAND, GRASS, MatchMode::Sides).expect("a set");
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
    }

    #[test]
    fn removing_a_terrain_empties_its_peers_by_hand_and_drops_its_sets() {
        let mut setup = farm();
        setup.remove_terrain(SAND).expect("removed");
        assert!(setup.terrain(SAND).is_none() && setup.sets.is_empty());
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
    fn a_split_moves_the_chosen_tiles_and_copies_the_sets_they_border() {
        let mut setup = farm();
        let tiles: BTreeSet<_> = [at(1)].into();
        let wet = setup
            .split_terrain(SAND, &tiles, "wet sand")
            .expect("split");
        assert_eq!(wet, 3);
        assert_eq!(setup.tiles[&at(1)].terrains, edge(wet, GRASS));
        assert_eq!(setup.tiles[&at(2)].terrains, TileTerrains::filled(SAND));
        let pairs: Vec<_> = setup.sets.iter().map(|s| (s.inner, s.outer)).collect();
        assert_eq!(pairs, [(SAND, GRASS), (wet, GRASS)]);
        assert_eq!(setup.terrain(wet).expect("new").swatch, [230, 160, 100]);

        let refused = setup.split_terrain(SAND, &[at(0)].into(), "x");
        assert_eq!(refused, Err(SetupError::NotOnTile(at(0), SAND)));
    }

    #[test]
    fn a_merge_folds_peers_and_sets_and_drops_a_set_over_itself() {
        let mut setup = farm();
        let wet = setup.add_terrain("wet", [1, 2, 3], false).expect("id");
        setup.add_set(wet, GRASS, MatchMode::Corners).expect("set");
        setup.add_set(wet, SAND, MatchMode::Corners).expect("set");
        setup.set_peers(at(3), edge(wet, GRASS)).expect("bits");

        setup.merge_terrains(wet, SAND).expect("merged");
        assert!(setup.terrain(wet).is_none());
        assert_eq!(setup.tiles[&at(3)].terrains, edge(SAND, GRASS));
        let sets: Vec<_> = (setup.sets.iter())
            .map(|s| (s.inner, s.outer, s.mode.value))
            .collect();
        assert_eq!(
            sets,
            [(SAND, GRASS, MatchMode::Sides)],
            "sand's own set wins"
        );
        assert_eq!(
            setup.merge_terrains(SAND, SAND),
            Err(SetupError::SameTerrain(SAND))
        );
    }

    #[test]
    fn sets_need_two_known_terrains_and_may_lie_over_empty() {
        let mut setup = farm();
        assert_eq!(
            setup.add_set(SAND, GRASS, MatchMode::Corners),
            Err(SetupError::SetExists(SAND, GRASS))
        );
        assert_eq!(
            setup.add_set(GRASS, GRASS, MatchMode::Corners),
            Err(SetupError::SameTerrain(GRASS))
        );
        assert_eq!(
            setup.add_set(9, GRASS, MatchMode::Corners),
            Err(SetupError::UnknownTerrain(9))
        );
        setup
            .add_set(GRASS, 0, MatchMode::Corners)
            .expect("over empty");
        let set = setup.set(GRASS, 0).expect("added");
        assert!(set.mode.is_user());
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
