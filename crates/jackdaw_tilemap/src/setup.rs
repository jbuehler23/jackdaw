//! Tileset setup: the sheets, terrains, terrain sets, tile bits, collision
//! shapes and stamps that auto setup proposes and the user edits, each value
//! marked Auto or User.
//!
//! - Sheets: named; tiles and stamps index the setup's own sheet list. A
//!   re-run maps its sheets by name and appends new ones; a sheet no longer
//!   in the inputs goes with all its values; a sheet still in the inputs but
//!   not analysed (no grid, or now an animation strip) keeps its values.
//! - Ids: fresh terrains match the setup's by Oklab colour within the
//!   clustering radius, one to one, closest pairs first (ties by setup id,
//!   then fresh id), and keep the setup id. Others take ids from `next_id`
//!   in fresh id order; ids are never reused. A full run drops a terrain with
//!   no match unless something User or anything kept refers to it.
//! - Merge: User values are kept; Auto values in scope take the fresh
//!   proposal; values out of scope are kept. Collision modes are the setup's
//!   (new terrains start Passable). A tile merges peer by peer; a tile left
//!   with no terrain, no shape and nothing User is unassigned.
//! - Sets: a set holds the transition tiles among its terrains, and the pure
//!   tiles of one of them when no other set uses it. Auto setup proposes
//!   transition pairs; pairs of one mode that share a terrain form one set.
//!   Locked sets, sets with a User mode and the tiles they hold are never
//!   changed by a re-run. A set the fresh run lacks stays while a kept tile
//!   is one of its transitions, and a set rebuilt by a re-run keeps its name.
//! - Shapes: each Auto shape in scope is traced from the tile's merged peers
//!   with the setup's colours and Solid terrains.
//! - Stamps: User stamps stay; Auto stamps in scope give way to fresh ones,
//!   less those sharing a tile with a stamp that stays.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use glam::Vec3;

use crate::collision_trace::{TileTrace, trace_tile};
use crate::report::auto_setup_reserving;
use crate::terrain_find::CLUSTER_RADIUS;
use crate::{
    AutoSetup, MatchMode, Peer, RgbaImage, SetupError, SheetInput, Stamp, TerrainId, TileAddress,
    TileShape, TileTerrains,
};

/// The terrain a transition draws, then the terrain it draws on (0 for
/// empty).
pub type Pair = (TerrainId, TerrainId);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Provenance {
    #[default]
    Auto,
    User,
}

/// A value and who set it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracked<T> {
    pub value: T,
    pub provenance: Provenance,
}

impl<T> Tracked<T> {
    pub fn auto(value: T) -> Self {
        Self {
            value,
            provenance: Provenance::Auto,
        }
    }

    pub fn user(value: T) -> Self {
        Self {
            value,
            provenance: Provenance::User,
        }
    }

    pub fn is_user(&self) -> bool {
        self.provenance == Provenance::User
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CollisionMode {
    Solid,
    Trigger,
    #[default]
    Passable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetupTerrain {
    pub id: TerrainId,
    pub name: Tracked<String>,
    /// Colour that matches it to the terrains of a re-run and classifies
    /// pixels when tracing.
    pub oklab: Vec3,
    pub swatch: [u8; 3],
    pub background: bool,
    pub collision: Tracked<CollisionMode>,
}

/// A named terrain set: a match mode and the terrains whose transitions it
/// holds.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupSet {
    pub name: String,
    pub mode: Tracked<MatchMode>,
    /// Stacking order, bottom first; never 0, never repeated. The empty
    /// terrain is below every set's terrains.
    pub terrains: Vec<TerrainId>,
    /// A re-run never changes the set or the tiles it holds.
    pub locked_manual: bool,
}

impl SetupSet {
    /// Whether the set holds `id`; every set holds the empty terrain 0.
    pub fn has(&self, id: TerrainId) -> bool {
        id == 0 || self.terrains.contains(&id)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupTile {
    pub terrains: TileTerrains,
    /// Per peer, by [`Peer::index`].
    pub provenance: [Provenance; 9],
    pub shape: Tracked<Option<TileShape>>,
    /// The transition the tile was laid out for, as the terrain drawn and
    /// the terrain drawn on; `None` when only its peers say which set holds
    /// it.
    pub set: Option<Pair>,
}

impl SetupTile {
    /// Forgets the recorded set once the peers hold a terrain outside it.
    pub(crate) fn drop_set_it_left(&mut self) {
        if self
            .set
            .is_some_and(|(a, b)| !within(&[a, b], &self.terrains))
        {
            self.set = None;
        }
    }

    fn has_user(&self) -> bool {
        self.shape.is_user() || self.provenance.contains(&Provenance::User)
    }

    fn is_blank(&self) -> bool {
        self.terrains == TileTerrains::default() && self.shape.value.is_none()
    }
}

/// What a re-run may change; ids and addresses are the setup's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Everything but locked sets and the tiles they hold.
    All,
    /// The set at this index: its match mode, its terrains and the tiles it
    /// holds before or after the run.
    Set(usize),
    /// The tiles' bits and shapes and the Auto stamps holding any of them.
    Tiles(BTreeSet<TileAddress>),
}

impl Scope {
    fn covers_terrain(&self, id: TerrainId, sets: &[SetupSet]) -> bool {
        match self {
            Scope::All => true,
            Scope::Set(i) => sets.get(*i).is_some_and(|s| s.terrains.contains(&id)),
            Scope::Tiles(_) => false,
        }
    }

    fn covers_set(&self, index: usize) -> bool {
        match self {
            Scope::All => true,
            Scope::Set(i) => *i == index,
            Scope::Tiles(_) => false,
        }
    }

    fn covers_pair(&self, (a, b): Pair, sets: &[SetupSet]) -> bool {
        match self {
            Scope::All => true,
            Scope::Set(i) => sets.get(*i).is_some_and(|s| s.has(a) && s.has(b)),
            Scope::Tiles(_) => false,
        }
    }

    fn covers_tile(
        &self,
        at: TileAddress,
        tiles: [Option<&SetupTile>; 2],
        sets: &[SetupSet],
    ) -> bool {
        match self {
            Scope::All => true,
            Scope::Set(i) => sets.get(*i).is_some_and(|set| {
                (tiles.iter().flatten()).any(|t| set_holds(&set.terrains, &t.terrains, sets))
            }),
            Scope::Tiles(chosen) => chosen.contains(&at),
        }
    }

    fn covers_stamp(&self, stamp: &Stamp) -> bool {
        match self {
            Scope::All => true,
            Scope::Set { .. } => false,
            Scope::Tiles(chosen) => stamp.tiles().any(|a| chosen.contains(&a)),
        }
    }

    fn moved(&self, sheet: &dyn Fn(usize) -> Option<usize>) -> Scope {
        match self {
            Scope::Tiles(chosen) => {
                Scope::Tiles(chosen.iter().filter_map(|a| moved(*a, sheet)).collect())
            }
            other => other.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TilesetSetup {
    /// Sheet names; [`TileAddress::sheet`] and [`Stamp::sheet`] index this.
    pub sheets: Vec<String>,
    /// By id.
    pub terrains: Vec<SetupTerrain>,
    /// In the order the user sees them; names are unique.
    pub sets: Vec<SetupSet>,
    /// Assigned tiles; a tile absent here is unassigned.
    pub tiles: BTreeMap<TileAddress, SetupTile>,
    /// By sheet, origin row, origin column.
    pub stamps: Vec<Tracked<Stamp>>,
    /// Id the next new terrain takes; ids are never reused.
    pub next_id: u16,
    /// Transitions removed by hand, either way round; a re-run does not
    /// propose them.
    pub removed_sets: BTreeSet<Pair>,
}

impl TilesetSetup {
    /// Everything `fresh` proposes, all Auto, in its ids, with the shapes it
    /// traced for the import's Solid terrains on the tiles it assigns.
    pub fn from_auto(fresh: &AutoSetup) -> Self {
        proposed(fresh, &fresh.shapes)
    }

    pub fn terrain(&self, id: TerrainId) -> Option<&SetupTerrain> {
        self.terrains.iter().find(|t| t.id == id)
    }

    /// The first set holding both `a` and `b`; 0 is in every set.
    pub fn set(&self, a: TerrainId, b: TerrainId) -> Option<&SetupSet> {
        self.sets.get(self.set_index(a, b)?)
    }

    /// Index of the first set holding both `a` and `b`, at least one of them
    /// a terrain.
    pub fn set_index(&self, a: TerrainId, b: TerrainId) -> Option<usize> {
        if a == 0 && b == 0 {
            return None;
        }
        (self.sets.iter()).position(|s| s.has(a) && s.has(b))
    }

    /// Index of the set named `name`.
    pub fn set_named(&self, name: &str) -> Option<usize> {
        self.sets.iter().position(|s| s.name == name)
    }

    /// `err` worded with the names of the terrains and sets it names.
    pub fn explain(&self, err: SetupError) -> String {
        let terrain = |id: TerrainId| {
            self.terrain(id)
                .map_or_else(|| id.to_string(), |t| format!("\"{}\"", t.name.value))
        };
        let set = |at: usize| {
            self.sets
                .get(at)
                .map_or_else(|| at.to_string(), |s| format!("\"{}\"", s.name))
        };
        match err {
            SetupError::SetNameTaken(at) => format!("a terrain set is already named {}", set(at)),
            SetupError::TerrainInSet {
                terrain: id,
                set: at,
            } => format!(
                "terrain {} already belongs to terrain set {}",
                terrain(id),
                set(at)
            ),
            SetupError::NotInSet {
                terrain: id,
                set: at,
            } => {
                format!(
                    "terrain set {} does not hold terrain {}",
                    set(at),
                    terrain(id)
                )
            }
            SetupError::SameTerrain(id) => format!("terrain {} cannot border itself", terrain(id)),
            other => other.to_string(),
        }
    }

    /// The first free `Terrain set n`, counting on from the sets there are.
    pub fn next_set_name(&self) -> String {
        free_set_name(self.sets.len() + 1, |name| self.set_named(name).is_some())
    }

    /// Sets one peer of `tile` by hand; `0` clears it.
    pub fn set_peer(
        &mut self,
        tile: TileAddress,
        peer: Peer,
        terrain: TerrainId,
    ) -> Result<(), SetupError> {
        if terrain != 0 && self.terrain(terrain).is_none() {
            return Err(SetupError::UnknownTerrain(terrain));
        }
        let entry = self.tiles.entry(tile).or_default();
        entry.terrains = entry.terrains.with(peer, terrain);
        entry.provenance[peer.index()] = Provenance::User;
        entry.drop_set_it_left();
        Ok(())
    }

    /// Sets the collision shape of `tile` by hand; `None` means no collision.
    pub fn set_shape(&mut self, tile: TileAddress, shape: Option<TileShape>) {
        self.tiles.entry(tile).or_default().shape = Tracked::user(shape);
    }

    pub fn rename_terrain(
        &mut self,
        id: TerrainId,
        name: impl Into<String>,
    ) -> Result<(), SetupError> {
        self.terrain_mut(id)?.name = Tracked::user(name.into());
        Ok(())
    }

    pub fn set_collision_mode(
        &mut self,
        id: TerrainId,
        mode: CollisionMode,
    ) -> Result<(), SetupError> {
        self.terrain_mut(id)?.collision = Tracked::user(mode);
        Ok(())
    }

    pub fn set_match_mode(&mut self, set: usize, mode: MatchMode) -> Result<(), SetupError> {
        self.set_mut(set)?.mode = Tracked::user(mode);
        Ok(())
    }

    pub fn lock_set(&mut self, set: usize, locked: bool) -> Result<(), SetupError> {
        self.set_mut(set)?.locked_manual = locked;
        Ok(())
    }

    /// Adds `stamp` as User in place of every stamp sharing a tile with it.
    pub fn put_stamp(&mut self, stamp: Stamp) {
        let tiles: BTreeSet<TileAddress> = stamp.tiles().collect();
        (self.stamps).retain(|s| !s.value.tiles().any(|a| tiles.contains(&a)));
        self.stamps.push(Tracked::user(stamp));
        sort_stamps(&mut self.stamps);
    }

    fn terrain_mut(&mut self, id: TerrainId) -> Result<&mut SetupTerrain, SetupError> {
        (self.terrains.iter_mut())
            .find(|t| t.id == id)
            .ok_or(SetupError::UnknownTerrain(id))
    }

    pub(crate) fn set_mut(&mut self, set: usize) -> Result<&mut SetupSet, SetupError> {
        self.sets.get_mut(set).ok_or(SetupError::UnknownSet(set))
    }

    /// The setup over `sheets`, its tiles and stamps moved by `sheet`; those
    /// it maps to `None` are dropped.
    fn moved(self, sheets: Vec<String>, sheet: &dyn Fn(usize) -> Option<usize>) -> Self {
        let tiles = (self.tiles.into_iter())
            .filter_map(|(a, t)| Some((moved(a, sheet)?, t)))
            .collect();
        let mut stamps: Vec<Tracked<Stamp>> = (self.stamps.into_iter())
            .filter_map(|mut s| {
                s.value.sheet = sheet(s.value.sheet)?;
                Some(s)
            })
            .collect();
        sort_stamps(&mut stamps);
        Self {
            sheets,
            tiles,
            stamps,
            ..self
        }
    }
}

/// Runs auto setup over `inputs` and replaces the Auto values of `previous`
/// in `scope` with its proposal.
pub fn rerun(
    previous: &TilesetSetup,
    inputs: &[(&str, RgbaImage)],
    scope: &Scope,
) -> Result<TilesetSetup, SetupError> {
    let fresh = auto_setup_reserving(inputs, &[], &terrain_names(previous))?;
    merge(previous, &fresh, inputs, scope, false)
}

/// The terrain names of `previous`, which a re-run keeps and new terrains
/// step around.
fn terrain_names(previous: &TilesetSetup) -> Vec<String> {
    (previous.terrains.iter())
        .map(|t| t.name.value.clone())
        .collect()
}

/// Runs auto setup over `inputs` and hands the bits and shapes of `tiles`
/// back to it, even in locked sets; a tile it proposes nothing for becomes
/// unassigned, and a tile on a sheet it did not analyse is left as it is.
/// Stamps, sets and existing terrains are left as they are.
pub fn reset_to_auto(
    setup: &TilesetSetup,
    inputs: &[(&str, RgbaImage)],
    tiles: &BTreeSet<TileAddress>,
) -> Result<TilesetSetup, SetupError> {
    let fresh = auto_setup_reserving(inputs, &[], &terrain_names(setup))?;
    reset(setup, &fresh, inputs, tiles)
}

fn reset(
    setup: &TilesetSetup,
    fresh: &AutoSetup,
    inputs: &[(&str, RgbaImage)],
    tiles: &BTreeSet<TileAddress>,
) -> Result<TilesetSetup, SetupError> {
    let mut handed_back = setup.clone();
    for address in tiles {
        if let Some(tile) = handed_back.tiles.get_mut(address) {
            tile.provenance = [Provenance::Auto; 9];
            tile.shape.provenance = Provenance::Auto;
        }
    }
    let scope = Scope::Tiles(tiles.clone());
    merge(&handed_back, fresh, inputs, &scope, true)
}

/// `previous` merged with `fresh`, which auto setup made from `inputs`.
fn merge(
    previous: &TilesetSetup,
    fresh: &AutoSetup,
    inputs: &[(&str, RgbaImage)],
    scope: &Scope,
    reset: bool,
) -> Result<TilesetSetup, SetupError> {
    let names: Vec<&str> = fresh
        .report
        .sheets
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    if let Some(i) = (0..names.len()).find(|&i| names[..i].contains(&names[i])) {
        return Err(SetupError::DuplicateSheet(i));
    }
    let mut sheets: Vec<String> = (previous.sheets.iter())
        .filter(|s| names.contains(&s.as_str()))
        .cloned()
        .collect();
    let mut analysed = Vec::with_capacity(fresh.sheets.len());
    for &i in &fresh.sheets {
        let name = *names.get(i).ok_or(SetupError::UnknownSheet(i))?;
        if !sheets.iter().any(|s| s == name) {
            sheets.push(name.to_string());
        }
        analysed.push(name);
    }
    let index_of = |name: &str| sheets.iter().position(|s| s == name);
    let from_previous = |i: usize| previous.sheets.get(i).and_then(|n| index_of(n));
    let fresh_sheets: Vec<Option<usize>> = analysed.iter().map(|n| index_of(n)).collect();
    let from_fresh = |s: usize| fresh_sheets.get(s).copied().flatten();
    let live: BTreeSet<usize> = fresh_sheets.iter().flatten().copied().collect();
    let scope = scope.moved(&from_previous);
    let prev = previous.clone().moved(sheets.clone(), &from_previous);
    let mut auto = proposed(fresh, &BTreeMap::new()).moved(sheets.clone(), &from_fresh);

    let (ids, start) = match_ids(&prev.terrains, &auto.terrains, prev.next_id)?;
    let remap = |f: TerrainId| ids.get(&f).copied().unwrap_or(0);
    for tile in auto.tiles.values_mut() {
        for peer in &mut tile.terrains.peers {
            *peer = remap(*peer);
        }
    }
    let fresh_pairs: Vec<(Pair, MatchMode)> = (fresh.proposal.sets.iter())
        .map(|s| ((remap(s.inner), remap(s.outer)), s.mode))
        .collect();
    let fresh_terrains: BTreeMap<TerrainId, SetupTerrain> = (auto.terrains.iter())
        .map(|t| {
            (
                remap(t.id),
                SetupTerrain {
                    id: remap(t.id),
                    ..t.clone()
                },
            )
        })
        .collect();
    let mut terrains = BTreeMap::new();
    for p in &prev.terrains {
        let refreshed = (fresh_terrains.get(&p.id))
            .filter(|_| scope.covers_terrain(p.id, &prev.sets))
            .map(|f| SetupTerrain {
                name: p.name.clone(),
                collision: p.collision.clone(),
                ..f.clone()
            });
        terrains.insert(p.id, refreshed.unwrap_or_else(|| p.clone()));
    }
    for (id, f) in &fresh_terrains {
        terrains.entry(*id).or_insert_with(|| f.clone());
    }

    let background = |id: TerrainId| terrains.get(&id).is_some_and(|t| t.background);
    let mut rebuilt = merge_sets(&prev, &fresh_pairs, &scope);
    let mut sets = rebuilt.assemble(&prev, &background);
    let locked: Vec<&SetupSet> = (prev.sets.iter())
        .filter(|s| !reset && s.locked_manual)
        .collect();
    let solid = |t: &SetupTerrain| t.collision.value == CollisionMode::Solid;
    let pixels = if terrains.values().any(solid) {
        sheet_inputs(fresh, inputs, &from_fresh)?
    } else {
        BTreeMap::new()
    };
    let palette = |id: TerrainId| terrains.get(&id).map(|t| (t.oklab, solid(t)));
    let mode_of = |peers: &TileTerrains| {
        let set = sets.iter().find(|s| transition(&s.terrains, peers));
        set.map_or(MatchMode::CornersAndSides, |s| s.mode.value)
    };
    let keys = prev.tiles.keys().chain(auto.tiles.keys());
    let addresses: BTreeSet<TileAddress> = keys.copied().collect();
    let mut tiles = BTreeMap::new();
    for address in addresses {
        let before = prev.tiles.get(&address);
        let both = [before, auto.tiles.get(&address)];
        let held = |set: &SetupSet| {
            (both.iter().flatten()).any(|t| set_holds(&set.terrains, &t.terrains, &prev.sets))
        };
        let fixed = !live.contains(&address.sheet)
            || !scope.covers_tile(address, both, &prev.sets)
            || locked.iter().any(|set| held(set));
        if fixed {
            if let Some(tile) = before {
                tiles.insert(address, tile.clone());
            }
            continue;
        }
        let mut merged = merge_tile(before, both[1]);
        if !merged.shape.is_user() {
            let trace = TileTrace {
                tile: address.tile,
                peers: &merged.terrains,
                mode: mode_of(&merged.terrains),
                terrain: true,
                decor_solid: false,
            };
            let input = pixels.get(&address.sheet);
            let shape = input.and_then(|i| trace_tile(i, &trace, &palette));
            merged.shape = Tracked::auto(shape);
        }
        if merged.has_user() || !merged.is_blank() {
            tiles.insert(address, merged);
        }
    }

    let mut restored = false;
    for set in &rebuilt.dropped.clone() {
        let lost = |t: &&SetupTile| {
            transition(&set.terrains, &t.terrains)
                && !sets.iter().any(|s| within(&s.terrains, &t.terrains))
        };
        if tiles.values().any(|t| lost(&t)) {
            rebuilt.pairs.extend(stacked_pairs(set));
            restored = true;
        }
    }
    if restored {
        sets = rebuilt.assemble(&prev, &background);
    }

    let mut stamps = prev.stamps.clone();
    if !reset {
        let in_scope = |s: &Stamp| live.contains(&s.sheet) && scope.covers_stamp(s);
        stamps.retain(|s| s.is_user() || !in_scope(&s.value));
        let held: BTreeSet<TileAddress> = stamps.iter().flat_map(|s| s.value.tiles()).collect();
        for s in &auto.stamps {
            if in_scope(&s.value) && !s.value.tiles().any(|a| held.contains(&a)) {
                stamps.push(s.clone());
            }
        }
        sort_stamps(&mut stamps);
    }

    let full = scope == Scope::All;
    let set_ids = sets.iter().flat_map(|s| s.terrains.clone());
    let used: BTreeSet<TerrainId> = (tiles.values())
        .flat_map(|t| t.terrains.peers)
        .chain(set_ids)
        .collect();
    let user_ids = user_terrains(&prev);
    terrains.retain(|id, _| {
        let kept = used.contains(id) || user_ids.contains(id);
        if u16::from(*id) >= start {
            full || kept
        } else {
            fresh_terrains.contains_key(id) || !full || kept
        }
    });
    let mut renumber = BTreeMap::new();
    let mut next_id = start;
    for id in terrains.keys().filter(|id| u16::from(**id) >= start) {
        let new = TerrainId::try_from(next_id).map_err(|_| SetupError::TooManyTerrains)?;
        renumber.insert(*id, new);
        next_id += 1;
    }
    let renumbered = |id: TerrainId| renumber.get(&id).copied().unwrap_or(id);
    for tile in tiles.values_mut() {
        for peer in &mut tile.terrains.peers {
            *peer = renumbered(*peer);
        }
    }
    let sets = (sets.into_iter())
        .map(|s| SetupSet {
            terrains: s.terrains.iter().map(|&t| renumbered(t)).collect(),
            ..s
        })
        .collect();
    let terrains = (terrains.into_values())
        .map(|t| SetupTerrain {
            id: renumbered(t.id),
            ..t
        })
        .collect();
    Ok(TilesetSetup {
        sheets,
        terrains,
        sets,
        tiles,
        stamps,
        next_id,
        removed_sets: prev.removed_sets.clone(),
    })
}

/// The setup `fresh` proposes, in its ids, with `shapes` as Auto shapes.
fn proposed(fresh: &AutoSetup, shapes: &BTreeMap<TileAddress, TileShape>) -> TilesetSetup {
    let solid = fresh.report.collision.as_ref().map(|c| c.solid.as_slice());
    let mut terrains: Vec<SetupTerrain> = (fresh.proposal.terrains.iter())
        .map(|p| {
            let found = fresh.analysis.terrain(p.id);
            let mode = if solid.is_some_and(|s| s.contains(&p.id)) {
                CollisionMode::Solid
            } else {
                CollisionMode::Passable
            };
            SetupTerrain {
                id: p.id,
                name: Tracked::auto(found.map(|t| t.name.clone()).unwrap_or_default()),
                oklab: found.map_or(Vec3::ZERO, |t| t.oklab),
                swatch: found.map_or([0; 3], |t| t.swatch),
                background: p.background,
                collision: Tracked::auto(mode),
            }
        })
        .collect();
    terrains.sort_by_key(|t| t.id);
    let auto_tile = |terrains| SetupTile {
        terrains,
        ..SetupTile::default()
    };
    let mut tiles = BTreeMap::new();
    for set in &fresh.proposal.sets {
        for t in &set.tiles {
            (tiles.entry(t.address)).or_insert_with(|| SetupTile {
                set: Some((set.inner, set.outer)),
                ..auto_tile(t.terrains)
            });
        }
    }
    for p in &fresh.proposal.terrains {
        for address in &p.pure_tiles {
            let pure = TileTerrains::filled(p.id);
            tiles.entry(*address).or_insert_with(|| auto_tile(pure));
        }
    }
    for (address, shape) in shapes {
        if let Some(tile) = tiles.get_mut(address) {
            tile.shape = Tracked::auto(Some(shape.clone()));
        }
    }
    let pairs: Vec<(Pair, MatchMode)> = (fresh.proposal.sets.iter())
        .map(|s| ((s.inner, s.outer), s.mode))
        .collect();
    let background =
        |id: TerrainId| (fresh.proposal.terrains.iter()).any(|p| p.id == id && p.background);
    let sets = (group_pairs(&pairs, &background).into_iter().zip(1..))
        .map(|(group, n)| SetupSet {
            name: set_name(n),
            mode: Tracked::auto(group.mode),
            terrains: group.terrains,
            locked_manual: false,
        })
        .collect();
    let mut stamps: Vec<Tracked<Stamp>> = fresh.stamps.iter().cloned().map(Tracked::auto).collect();
    sort_stamps(&mut stamps);
    let sheets = (fresh.sheets.iter())
        .map(|&i| fresh.report.sheets.get(i).map(|s| s.name.clone()))
        .map(Option::unwrap_or_default)
        .collect();
    let next_id = terrains.iter().map(|t| u16::from(t.id) + 1).max();
    TilesetSetup {
        sheets,
        terrains,
        sets,
        tiles,
        stamps,
        next_id: next_id.unwrap_or(1),
        removed_sets: BTreeSet::new(),
    }
}

/// Setup id of each fresh terrain, and the first id a new terrain took.
fn match_ids(
    previous: &[SetupTerrain],
    fresh: &[SetupTerrain],
    next_id: u16,
) -> Result<(BTreeMap<TerrainId, TerrainId>, u16), SetupError> {
    let mut pairs = Vec::new();
    for p in previous {
        for f in fresh {
            let d = p.oklab.distance(f.oklab);
            if d <= CLUSTER_RADIUS {
                pairs.push((d, p.id, f.id));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut ids = BTreeMap::new();
    let mut taken = BTreeSet::new();
    for (_, p, f) in pairs {
        if !taken.contains(&p) && !ids.contains_key(&f) {
            taken.insert(p);
            ids.insert(f, p);
        }
    }
    let largest = previous.iter().map(|t| u16::from(t.id)).max().unwrap_or(0);
    let start = next_id.max(largest + 1);
    let mut next = start;
    for f in fresh {
        if let Entry::Vacant(e) = ids.entry(f.id) {
            e.insert(TerrainId::try_from(next).map_err(|_| SetupError::TooManyTerrains)?);
            next += 1;
        }
    }
    Ok((ids, start))
}

/// Terrains a User value or a locked or User-mode set refers to.
fn user_terrains(setup: &TilesetSetup) -> BTreeSet<TerrainId> {
    let named = (setup.terrains.iter())
        .filter(|t| t.name.is_user() || t.collision.is_user())
        .map(|t| t.id);
    let sets = (setup.sets.iter())
        .filter(|s| s.locked_manual || s.mode.is_user())
        .flat_map(|s| s.terrains.clone());
    let peers = setup.tiles.values().flat_map(|t| {
        (t.terrains.peers.iter().zip(&t.provenance))
            .filter(|(_, p)| **p == Provenance::User)
            .map(|(id, _)| *id)
    });
    named.chain(sets).chain(peers).collect()
}

/// The sets of a re-run before its tiles are merged: the previous sets it
/// keeps, those it rebuilds, and the transition pairs it rebuilds them from.
struct MergedSets {
    /// Indices of the previous sets kept as they are.
    kept: Vec<usize>,
    /// Previous sets in scope that the run rebuilds.
    dropped: Vec<SetupSet>,
    pairs: Vec<(Pair, MatchMode)>,
}

impl MergedSets {
    /// The kept sets and the sets the pairs form, in the previous order; a
    /// rebuilt set takes the name of the dropped set it shares the most
    /// terrains with, and new sets take free default names.
    fn assemble(
        &self,
        previous: &TilesetSetup,
        background: &dyn Fn(TerrainId) -> bool,
    ) -> Vec<SetupSet> {
        let groups = group_pairs(&self.pairs, background);
        let mut names: Vec<Option<String>> = vec![None; groups.len()];
        for dropped in &self.dropped {
            let shared = |g: &PairGroup| {
                let count = g.terrains.iter().filter(|t| dropped.terrains.contains(t));
                (count.count(), g.mode == dropped.mode.value)
            };
            let heir = (0..groups.len())
                .filter(|&g| names[g].is_none() && shared(&groups[g]).0 > 0)
                .max_by_key(|&g| (shared(&groups[g]), std::cmp::Reverse(g)));
            if let Some(heir) = heir {
                names[heir] = Some(dropped.name.clone());
            }
        }
        let mut taken: Vec<String> = (self.kept.iter())
            .filter_map(|&i| previous.sets.get(i))
            .map(|s| s.name.clone())
            .chain(names.iter().flatten().cloned())
            .collect();
        for name in &mut names {
            if name.is_none() {
                let fresh = free_set_name(1, |candidate| taken.iter().any(|t| t == candidate));
                taken.push(fresh.clone());
                *name = Some(fresh);
            }
        }
        let built = |index: usize| SetupSet {
            name: names[index].clone().unwrap_or_default(),
            mode: Tracked::auto(groups[index].mode),
            terrains: groups[index].terrains.clone(),
            locked_manual: false,
        };
        let mut placed = vec![false; groups.len()];
        let mut sets = Vec::new();
        for (index, set) in previous.sets.iter().enumerate() {
            if self.kept.contains(&index) {
                sets.push(set.clone());
                continue;
            }
            let heir = names
                .iter()
                .position(|n| n.as_deref() == Some(set.name.as_str()));
            if let Some(heir) = heir.filter(|h| !placed[*h]) {
                placed[heir] = true;
                sets.push(built(heir));
            }
        }
        for (index, done) in placed.into_iter().enumerate() {
            if !done {
                sets.push(built(index));
            }
        }
        sets
    }
}

/// Keeps the previous sets outside `scope`, locked or of a User mode, and
/// rebuilds the others from the fresh pairs in scope that no kept set holds
/// and that were not removed by hand.
fn merge_sets(previous: &TilesetSetup, fresh: &[(Pair, MatchMode)], scope: &Scope) -> MergedSets {
    let keeps = |(index, set): &(usize, &SetupSet)| {
        !scope.covers_set(*index) || set.locked_manual || set.mode.is_user()
    };
    let (kept, dropped): (Vec<_>, Vec<_>) = previous.sets.iter().enumerate().partition(keeps);
    let removed = |(a, b): Pair| {
        previous.removed_sets.contains(&(a, b)) || previous.removed_sets.contains(&(b, a))
    };
    let held = |(a, b): Pair| kept.iter().any(|(_, s)| s.has(a) && s.has(b));
    let pairs = (fresh.iter())
        .filter(|(pair, _)| {
            scope.covers_pair(*pair, &previous.sets) && !removed(*pair) && !held(*pair)
        })
        .copied()
        .collect();
    MergedSets {
        kept: kept.into_iter().map(|(i, _)| i).collect(),
        dropped: dropped.into_iter().map(|(_, s)| s.clone()).collect(),
        pairs,
    }
}

/// Every transition of `set` as a pair, upper terrain first: each terrain
/// on empty and on each terrain below it, which [`group_pairs`] stacks back
/// in the set's order.
pub fn stacked_pairs(set: &SetupSet) -> Vec<(Pair, MatchMode)> {
    let (mode, terrains) = (set.mode.value, &set.terrains);
    (0..terrains.len())
        .flat_map(|upper| {
            let lower = std::iter::once(0).chain(terrains[..upper].iter().copied());
            lower.map(move |lower| ((terrains[upper], lower), mode))
        })
        .collect()
}

/// Terrain sets formed from transition pairs, as [`group_pairs`] returns them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairGroup {
    pub mode: MatchMode,
    /// Bottom first.
    pub terrains: Vec<TerrainId>,
    /// Indices of the pairs the set took.
    pub pairs: Vec<usize>,
}

/// The sets `pairs` form: pairs of one mode that share a terrain join one
/// set, in the order of their first pair. A set stacks its background
/// terrains first, then each pair's second terrain below its first, then by
/// id.
pub fn group_pairs(
    pairs: &[(Pair, MatchMode)],
    background: &dyn Fn(TerrainId) -> bool,
) -> Vec<PairGroup> {
    let mut groups: Vec<PairGroup> = Vec::new();
    for (index, &((a, b), mode)) in pairs.iter().enumerate() {
        if a == b || (a == 0 && b == 0) {
            continue;
        }
        let shares = |g: &PairGroup| {
            g.mode == mode && [a, b].iter().any(|t| *t != 0 && g.terrains.contains(t))
        };
        let joined: Vec<usize> = (0..groups.len()).filter(|&g| shares(&groups[g])).collect();
        let into = match joined.first() {
            Some(&first) => {
                for &other in joined[1..].iter().rev() {
                    let gone = groups.remove(other);
                    for t in gone.terrains {
                        if !groups[first].terrains.contains(&t) {
                            groups[first].terrains.push(t);
                        }
                    }
                    groups[first].pairs.extend(gone.pairs);
                }
                first
            }
            None => {
                groups.push(PairGroup {
                    mode,
                    terrains: Vec::new(),
                    pairs: Vec::new(),
                });
                groups.len() - 1
            }
        };
        let group = &mut groups[into];
        group.pairs.push(index);
        for t in [a, b] {
            if t != 0 && !group.terrains.contains(&t) {
                group.terrains.push(t);
            }
        }
    }
    for group in &mut groups {
        group.pairs.sort_unstable();
        let terrains = group.terrains.clone();
        let index = |id: TerrainId| terrains.iter().position(|t| *t == id);
        let mut depth = vec![0usize; terrains.len()];
        for _ in 0..terrains.len() {
            for &p in &group.pairs {
                let ((upper, lower), _) = pairs[p];
                if let (Some(upper), Some(lower)) = (index(upper), index(lower)) {
                    depth[upper] = depth[upper].max(depth[lower] + 1).min(terrains.len());
                }
            }
        }
        let mut order: Vec<(bool, usize, TerrainId)> = (terrains.iter().zip(&depth))
            .map(|(&id, &depth)| (!background(id), depth, id))
            .collect();
        order.sort_unstable();
        group.terrains = order.into_iter().map(|(.., id)| id).collect();
    }
    groups.sort_by_key(|g| g.pairs.first().copied());
    groups
}

/// `Terrain set n`.
pub fn set_name(n: usize) -> String {
    format!("Terrain set {n}")
}

/// The first `Terrain set n` from `from` on that `taken` refuses.
pub fn free_set_name(from: usize, taken: impl Fn(&str) -> bool) -> String {
    (from.max(1)..)
        .map(set_name)
        .find(|name| !taken(name))
        .unwrap_or_default()
}

fn merge_tile(previous: Option<&SetupTile>, fresh: Option<&SetupTile>) -> SetupTile {
    let mut tile = fresh.cloned().unwrap_or_default();
    let Some(p) = previous else {
        return tile;
    };
    for peer in Peer::ALL {
        let i = peer.index();
        if p.provenance[i] == Provenance::User {
            tile.terrains = tile.terrains.with(peer, p.terrains.get(peer));
            tile.provenance[i] = Provenance::User;
        }
    }
    if p.shape.is_user() {
        tile.shape = p.shape.clone();
    }
    if p.provenance.contains(&Provenance::User) {
        tile.set = p.set.or(tile.set);
    }
    tile.drop_set_it_left();
    tile
}

/// Whether the terrain solver of `mode` takes a tile with `peers` laid out
/// for the transition `recorded`. While the peers fit it, the sets holding
/// its drawn terrain above the other decide: it goes to the tightest of them,
/// the one with the fewest other terrains, if that is of `mode`. With none,
/// any set of `mode` holding both takes it. Otherwise the solver takes it
/// when its peers fit a set of `mode` and, unless one terrain fills the tile,
/// no set of another mode fits them as tightly. `sets` are the terrains,
/// bottom first, and the mode of each set.
pub fn solver_takes(
    mode: MatchMode,
    peers: &[TerrainId; 9],
    recorded: Option<Pair>,
    sets: &[(&[TerrainId], MatchMode)],
) -> bool {
    let rank = |terrains: &[TerrainId], id: TerrainId| match id {
        0 => Some(0),
        id => terrains.iter().position(|t| *t == id).map(|at| at + 1),
    };
    if let Some((a, b)) = recorded.filter(|&(a, b)| fits(&[a, b], peers)) {
        let ranks = |t: &[TerrainId]| rank(t, a).zip(rank(t, b));
        let held = [a, b].iter().filter(|t| **t != 0).count();
        let stacked: Vec<(usize, bool)> = (sets.iter())
            .filter(|(t, _)| ranks(t).is_some_and(|(a, b)| a > b))
            .map(|(t, m)| (t.len().saturating_sub(held), *m == mode))
            .collect();
        return match stacked.iter().map(|(extra, _)| *extra).min() {
            Some(tight) => (stacked.iter()).any(|&(extra, ours)| extra == tight && ours),
            None => (sets.iter()).any(|(t, m)| *m == mode && ranks(t).is_some()),
        };
    }
    let pure = peers.iter().all(|&p| p == peers[0]);
    let shown = present(&TileTerrains { peers: *peers }).len();
    let fitting: Vec<(usize, bool)> = (sets.iter())
        .filter(|(t, _)| fits(t, peers))
        .map(|(t, m)| (t.len().saturating_sub(shown), *m == mode))
        .collect();
    let Some(tight) = fitting.iter().map(|(extra, _)| *extra).min() else {
        return false;
    };
    let at = |ours: bool| (fitting.iter()).any(|&(extra, o)| extra == tight && o == ours);
    fitting.iter().any(|(_, ours)| *ours) && (pure || (at(true) && !at(false)))
}

/// Whether `peers` has a terrain and all of them are of `terrains` or empty.
pub fn fits(terrains: &[TerrainId], peers: &[TerrainId; 9]) -> bool {
    peers.iter().all(|&p| p == 0 || terrains.contains(&p)) && peers.iter().any(|&p| p != 0)
}

pub(crate) fn within(terrains: &[TerrainId], peers: &TileTerrains) -> bool {
    peers.peers.iter().all(|&p| p == 0 || terrains.contains(&p))
}

/// Whether `peers` lie within `terrains` and the empty terrain and hold more
/// than one of them.
pub(crate) fn transition(terrains: &[TerrainId], peers: &TileTerrains) -> bool {
    within(terrains, peers) && peers.peers.iter().any(|&p| p != peers.peers[0])
}

/// The terrains `peers` hold, empty left out.
fn present(peers: &TileTerrains) -> Vec<TerrainId> {
    let mut found: Vec<TerrainId> = Vec::new();
    for &p in &peers.peers {
        if p != 0 && !found.contains(&p) {
            found.push(p);
        }
    }
    found
}

/// Whether the set of `terrains` holds a tile with `peers`: a transition
/// among its terrains, or a tile of one of them no other set in `sets` uses.
pub(crate) fn set_holds(terrains: &[TerrainId], peers: &TileTerrains, sets: &[SetupSet]) -> bool {
    if !within(terrains, peers) {
        return false;
    }
    if transition(terrains, peers) {
        return true;
    }
    let [only] = present(peers)[..] else {
        return false;
    };
    let other = |s: &SetupSet| s.terrains != terrains && s.terrains.contains(&only);
    !sets.iter().any(other)
}

fn moved(a: TileAddress, sheet: &dyn Fn(usize) -> Option<usize>) -> Option<TileAddress> {
    Some(TileAddress {
        sheet: sheet(a.sheet)?,
        tile: a.tile,
    })
}

fn sort_stamps(stamps: &mut [Tracked<Stamp>]) {
    stamps.sort_by_key(|s| {
        let first = s.value.cells.first().map(|c| c.1);
        (s.value.sheet, s.value.origin.y, s.value.origin.x, first)
    });
}

/// The analysed sheets of `fresh` with their pixels, by setup sheet.
fn sheet_inputs<'a>(
    fresh: &AutoSetup,
    inputs: &[(&'a str, RgbaImage<'a>)],
    sheet: &dyn Fn(usize) -> Option<usize>,
) -> Result<BTreeMap<usize, SheetInput<'a>>, SetupError> {
    let mut by_sheet = BTreeMap::new();
    for (s, &i) in fresh.sheets.iter().enumerate() {
        let &(name, image) = inputs.get(i).ok_or(SetupError::UnknownSheet(i))?;
        let grid = fresh.report.sheets.get(i).and_then(|r| r.grid);
        let (layout, _) = grid.ok_or(SetupError::UnknownSheet(i))?;
        if let Some(sheet) = sheet(s) {
            let input = SheetInput {
                name,
                image,
                layout,
            };
            by_sheet.insert(sheet, input);
        }
    }
    Ok(by_sheet)
}

#[cfg(test)]
mod tests {
    use glam::{UVec2, Vec2};

    use super::*;
    use crate::{
        CollisionReport, FittedTile, FoundTerrain, ProposedSet, ProposedTerrain, SetupReport,
        SheetReport, TerrainAnalysis, TilesetProposal,
    };

    const GRASS: TerrainId = 1;
    const SAND: TerrainId = 2;
    const DIRT: TerrainId = 3;
    const COLOURS: [(TerrainId, f32); 3] = [(GRASS, 0.8), (SAND, 0.5), (DIRT, 0.2)];

    type Tiles<'a> = &'a [(u32, TileTerrains)];

    /// Tile `n % 100` of sheet `n / 100`.
    fn at(n: u32) -> TileAddress {
        TileAddress {
            sheet: (n / 100) as usize,
            tile: n % 100,
        }
    }

    fn mix(inner: TerrainId, outer: TerrainId, peers: &[Peer]) -> TileTerrains {
        let base = TileTerrains::filled(outer).with(Peer::Centre, inner);
        peers.iter().fold(base, |t, p| t.with(*p, inner))
    }

    fn stamp(tiles: &[u32]) -> Stamp {
        Stamp {
            sheet: 0,
            origin: UVec2::new(tiles[0] % 10, tiles[0] / 10),
            size: UVec2::new(tiles.len() as u32, 1),
            cells: (0..)
                .zip(tiles)
                .map(|(i, t)| (UVec2::new(i, 0), *t))
                .collect(),
        }
    }

    /// A run over sheets `names` of which `analysed` were analysed, with
    /// terrains `(id, lightness)` of ids `1..=n` and pure grass `grass`.
    struct Run<'a> {
        names: &'a [&'a str],
        analysed: &'a [usize],
        terrains: &'a [(TerrainId, f32)],
        sets: &'a [(TerrainId, TerrainId, MatchMode, Tiles<'a>)],
        grass: &'a [u32],
        stamps: &'a [&'a [u32]],
    }

    impl Run<'_> {
        fn fresh(&self) -> AutoSetup {
            let found = (self.terrains.iter())
                .map(|&(id, l)| FoundTerrain {
                    id,
                    oklab: Vec3::new(l, 0.0, 0.0),
                    swatch: [0; 3],
                    variance: 0.0,
                    pure_tiles: Vec::new(),
                    name: format!("auto {l}"),
                })
                .collect();
            let fitted = |&(tile, terrains): &(u32, TileTerrains)| FittedTile {
                address: at(tile),
                terrains,
                confidence: [1.0; 9],
                corrected: [false; 9],
            };
            let sets = (self.sets.iter())
                .map(|&(inner, outer, mode, tiles)| ProposedSet {
                    mode,
                    inner,
                    outer,
                    tiles: tiles.iter().map(fitted).collect(),
                    covered: 0,
                    total: 0,
                    missing: Vec::new(),
                    template: None,
                })
                .collect();
            let proposed = (self.terrains.iter())
                .map(|&(id, _)| ProposedTerrain {
                    id,
                    background: id == GRASS,
                    pure_tiles: (self.grass.iter())
                        .filter(|_| id == GRASS)
                        .map(|t| at(*t))
                        .collect(),
                })
                .collect();
            let sheets = (self.names.iter())
                .map(|n| SheetReport {
                    name: n.to_string(),
                    grid: None,
                    animates: None,
                })
                .collect();
            AutoSetup {
                sheets: self.analysed.to_vec(),
                analysis: TerrainAnalysis {
                    terrains: found,
                    tiles: Vec::new(),
                },
                proposal: TilesetProposal {
                    sets,
                    terrains: proposed,
                    unassigned: Vec::new(),
                    low_confidence: Vec::new(),
                },
                stamps: self.stamps.iter().map(|s| stamp(s)).collect(),
                shapes: BTreeMap::new(),
                report: SetupReport {
                    sheets,
                    ..SetupReport::default()
                },
            }
        }
    }

    fn sand_first() -> Vec<(u32, TileTerrains)> {
        vec![
            (0, mix(SAND, GRASS, &[Peer::Top])),
            (1, mix(SAND, GRASS, &[Peer::Left])),
            (2, TileTerrains::filled(SAND)),
        ]
    }

    fn dirt_first() -> Vec<(u32, TileTerrains)> {
        vec![
            (10, mix(DIRT, GRASS, &[Peer::Top])),
            (11, mix(DIRT, GRASS, &[Peer::Right])),
            (12, TileTerrains::filled(DIRT)),
        ]
    }

    fn first() -> AutoSetup {
        let sets = [
            (SAND, GRASS, MatchMode::Sides, &sand_first()[..]),
            (DIRT, GRASS, MatchMode::Corners, &dirt_first()[..]),
        ];
        let run = Run {
            names: &["a.png"],
            analysed: &[0],
            terrains: &COLOURS,
            sets: &sets,
            grass: &[5],
            stamps: &[&[20, 21]],
        };
        run.fresh()
    }

    fn second() -> AutoSetup {
        let sand = [
            (0, mix(SAND, GRASS, &[Peer::Top, Peer::Bottom])),
            (2, TileTerrains::filled(SAND)),
            (3, mix(SAND, GRASS, &[Peer::Right])),
        ];
        let dirt = [(10, mix(DIRT, GRASS, &[Peer::Bottom]))];
        let sets = [
            (SAND, GRASS, MatchMode::Corners, &sand[..]),
            (DIRT, GRASS, MatchMode::CornersAndSides, &dirt[..]),
        ];
        let run = Run {
            names: &["a.png"],
            analysed: &[0],
            terrains: &COLOURS,
            sets: &sets,
            grass: &[6],
            stamps: &[&[20, 21, 22]],
        };
        run.fresh()
    }

    /// `setup` after checking every id it uses names one of its terrains
    /// below `next_id`.
    fn checked(setup: TilesetSetup) -> TilesetSetup {
        let known: BTreeSet<TerrainId> = setup.terrains.iter().map(|t| t.id).collect();
        let peers = setup.tiles.values().flat_map(|t| t.terrains.peers);
        let sets = setup.sets.iter().flat_map(|s| s.terrains.clone());
        for id in peers.chain(sets).filter(|id| *id != 0) {
            assert!(known.contains(&id), "terrain {id} dangles");
        }
        assert!(known.iter().all(|id| u16::from(*id) < setup.next_id));
        setup
    }

    fn run(previous: &TilesetSetup, fresh: &AutoSetup, scope: Scope) -> TilesetSetup {
        checked(merge(previous, fresh, &[], &scope, false).expect("nothing to trace"))
    }

    fn reset_tiles(setup: &TilesetSetup, fresh: &AutoSetup, list: &[u32]) -> TilesetSetup {
        checked(reset(setup, fresh, &[], &tiles(list)).expect("nothing to trace"))
    }

    fn tiles(list: &[u32]) -> BTreeSet<TileAddress> {
        list.iter().map(|t| at(*t)).collect()
    }

    fn square() -> Option<TileShape> {
        let ring = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
        Some(TileShape {
            polygons: vec![ring.to_vec()],
            half_tile: false,
        })
    }

    #[test]
    fn from_auto_marks_everything_auto() {
        let setup = checked(TilesetSetup::from_auto(&first()));
        assert_eq!(setup.sheets, ["a.png"]);
        let ids: Vec<TerrainId> = setup.terrains.iter().map(|t| t.id).collect();
        assert_eq!((ids, setup.next_id), (vec![GRASS, SAND, DIRT], 4));
        assert!(setup.terrains.iter().all(|t| !t.name.is_user()));
        assert!(
            setup
                .sets
                .iter()
                .all(|s| !s.mode.is_user() && !s.locked_manual)
        );
        assert_eq!(setup.tiles.len(), 7);
        assert!(setup.tiles.values().all(|t| !t.has_user()));
        assert_eq!(setup.tiles[&at(5)].terrains, TileTerrains::filled(GRASS));
        assert_eq!(setup.stamps, [Tracked::auto(stamp(&[20, 21]))]);
    }

    #[test]
    fn from_auto_takes_the_imports_solid_terrains() {
        let mut fresh = first();
        fresh.report.collision = Some(CollisionReport {
            solid: vec![DIRT],
            shapes: 0,
            half_tiles: 0,
        });
        let setup = TilesetSetup::from_auto(&fresh);
        let dirt = setup.terrain(DIRT).map(|t| &t.collision);
        assert_eq!(dirt, Some(&Tracked::auto(CollisionMode::Solid)));
    }

    #[test]
    fn a_full_rerun_keeps_user_values_and_takes_fresh_auto_values() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.set_peer(at(0), Peer::Left, SAND).expect("known");
        setup.rename_terrain(GRASS, "meadow").expect("known");
        let dirt = setup.set_index(DIRT, GRASS).expect("a set");
        (setup.set_match_mode(dirt, MatchMode::Corners)).expect("known");
        (setup.set_collision_mode(DIRT, CollisionMode::Trigger)).expect("known");
        setup.set_shape(at(11), square());
        setup.put_stamp(stamp(&[30, 31]));
        let after = run(&setup, &second(), Scope::All);

        let zero = &after.tiles[&at(0)];
        let auto = mix(SAND, GRASS, &[Peer::Top, Peer::Bottom]);
        assert_eq!(zero.terrains, auto.with(Peer::Left, SAND));
        assert_eq!(zero.provenance[Peer::Left.index()], Provenance::User);
        let users = zero.provenance.iter().filter(|p| **p == Provenance::User);
        assert_eq!(users.count(), 1);
        assert!(!after.tiles.contains_key(&at(1)));
        assert_eq!(
            after.tiles[&at(3)].terrains,
            mix(SAND, GRASS, &[Peer::Right])
        );
        assert!(!after.tiles.contains_key(&at(5)));
        assert_eq!(after.tiles[&at(6)].terrains, TileTerrains::filled(GRASS));
        let eleven = &after.tiles[&at(11)];
        assert_eq!(eleven.terrains, TileTerrains::default());
        assert_eq!(eleven.shape, Tracked::user(square()));
        let mode = |inner| after.set(inner, GRASS).map(|s| s.mode.clone());
        assert_eq!(mode(SAND), Some(Tracked::auto(MatchMode::Corners)));
        assert_eq!(mode(DIRT), Some(Tracked::user(MatchMode::Corners)));
        let grass = after.terrain(GRASS).expect("kept");
        assert_eq!(grass.name, Tracked::user("meadow".to_string()));
        let dirt = after.terrain(DIRT).expect("kept");
        assert_eq!(dirt.collision, Tracked::user(CollisionMode::Trigger));
        assert_eq!(
            after.terrain(SAND).map(|t| t.name.value.as_str()),
            Some("auto 0.5")
        );
        let stamps = [
            Tracked::auto(stamp(&[20, 21, 22])),
            Tracked::user(stamp(&[30, 31])),
        ];
        assert_eq!(after.stamps, stamps);
    }

    #[test]
    fn collision_modes_are_the_setups() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.terrains[2].collision = Tracked::auto(CollisionMode::Trigger);
        let after = run(&setup, &second(), Scope::All);
        let dirt = after.terrain(DIRT).map(|t| &t.collision);
        assert_eq!(dirt, Some(&Tracked::auto(CollisionMode::Trigger)));
    }

    #[test]
    fn terrain_ids_follow_colour_not_fresh_ids() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.rename_terrain(SAND, "sand by hand").expect("known");
        let sand = [(0, mix(3, 2, &[Peer::Top]))];
        let sets = [
            (3, 2, MatchMode::Sides, &sand[..]),
            (4, 2, MatchMode::Sides, &[][..]),
        ];
        let permuted = Run {
            names: &["a.png"],
            analysed: &[0],
            terrains: &[(1, 0.2), (2, 0.8), (3, 0.5), (4, 0.35)],
            sets: &sets,
            grass: &[],
            stamps: &[],
        };
        let after = run(&setup, &permuted.fresh(), Scope::All);
        let ids: Vec<(TerrainId, f32)> = after.terrains.iter().map(|t| (t.id, t.oklab.x)).collect();
        assert_eq!(ids, [(GRASS, 0.8), (SAND, 0.5), (DIRT, 0.2), (4, 0.35)]);
        assert_eq!(after.next_id, 5);
        assert_eq!(after.terrain(SAND).map(|t| t.name.is_user()), Some(true));
        assert_eq!(after.tiles[&at(0)].terrains, mix(SAND, GRASS, &[Peer::Top]));
        let sets: Vec<_> = (after.sets.iter())
            .map(|s| (s.name.as_str(), s.terrains.as_slice()))
            .collect();
        assert_eq!(sets, [("Terrain set 1", &[GRASS, SAND, 4][..])]);
    }

    #[test]
    fn unmatched_terrains_drop_unless_used_and_ids_are_never_reused() {
        let only = |terrains| Run {
            names: &["a.png"],
            analysed: &[0],
            terrains,
            sets: &[],
            grass: &[],
            stamps: &[],
        };
        let again = only(&[(1, 0.8), (2, 0.5)]).fresh();
        let setup = TilesetSetup::from_auto(&first());
        let dropped = run(&setup, &again, Scope::All);
        assert!(dropped.terrain(DIRT).is_none());
        assert_eq!(dropped.next_id, 4);
        let mut named = setup.clone();
        named.rename_terrain(DIRT, "mud").expect("known");
        assert!(run(&named, &again, Scope::All).terrain(DIRT).is_some());
        let mut painted = setup.clone();
        painted.set_peer(at(15), Peer::Top, DIRT).expect("known");
        assert!(run(&painted, &again, Scope::All).terrain(DIRT).is_some());
        assert!(
            run(&setup, &again, Scope::Tiles(tiles(&[0])))
                .terrain(DIRT)
                .is_some()
        );
        let darker = only(&[(1, 0.8), (2, 0.5), (3, 0.05)]).fresh();
        let back = run(&dropped, &darker, Scope::All);
        let ids: Vec<TerrainId> = back.terrains.iter().map(|t| t.id).collect();
        assert_eq!((ids, back.next_id), (vec![1, 2, 4], 5));
    }

    #[test]
    fn a_scoped_run_spends_ids_only_on_terrains_it_keeps() {
        let setup = TilesetSetup::from_auto(&first());
        let new = Run {
            names: &["a.png"],
            analysed: &[0],
            terrains: &[(1, 0.8), (2, 0.5), (3, 0.2), (4, 0.35), (5, 0.65)],
            sets: &[(5, 1, MatchMode::Sides, &[(40, TileTerrains::filled(5))])],
            grass: &[],
            stamps: &[],
        };
        let after = run(&setup, &new.fresh(), Scope::Tiles(tiles(&[40])));
        let ids: Vec<TerrainId> = after.terrains.iter().map(|t| t.id).collect();
        assert_eq!((ids, after.next_id), (vec![1, 2, 3, 4], 5));
        assert_eq!(after.tiles[&at(40)].terrains, TileTerrains::filled(4));
        assert_eq!(after.terrain(4).map(|t| t.oklab.x), Some(0.65));
    }

    #[test]
    fn running_out_of_ids_is_an_error() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.next_id = 256;
        let new = Run {
            names: &["a.png"],
            analysed: &[0],
            terrains: &[(1, 0.8), (2, 0.5), (3, 0.2), (4, 0.35)],
            sets: &[],
            grass: &[],
            stamps: &[],
        };
        let result = merge(&setup, &new.fresh(), &[], &Scope::All, false);
        assert_eq!(result, Err(SetupError::TooManyTerrains));
    }

    #[test]
    fn a_set_scoped_run_changes_only_that_set() {
        let setup = TilesetSetup::from_auto(&first());
        let dirt = setup.set_index(DIRT, GRASS).expect("a set");
        let after = run(&setup, &second(), Scope::Set(dirt));
        let all = run(&setup, &second(), Scope::All);
        assert_eq!(after.set(DIRT, GRASS), all.set(DIRT, GRASS));
        assert_eq!(after.set(SAND, GRASS), setup.set(SAND, GRASS));
        for t in [10, 11, 12] {
            assert_eq!(after.tiles.get(&at(t)), all.tiles.get(&at(t)), "{t}");
        }
        for t in [0, 1, 2, 3, 5, 6] {
            assert_eq!(after.tiles.get(&at(t)), setup.tiles.get(&at(t)), "{t}");
        }
        assert_eq!(after.stamps, setup.stamps);
    }

    #[test]
    fn a_tile_scoped_run_changes_only_those_tiles() {
        let setup = TilesetSetup::from_auto(&first());
        let after = run(&setup, &second(), Scope::Tiles(tiles(&[0, 10])));
        let all = run(&setup, &second(), Scope::All);
        for t in [0, 10] {
            assert_eq!(after.tiles.get(&at(t)), all.tiles.get(&at(t)), "{t}");
        }
        for t in [1, 2, 3, 5, 6, 11] {
            assert_eq!(after.tiles.get(&at(t)), setup.tiles.get(&at(t)), "{t}");
        }
        assert_eq!((&after.sets, &after.stamps), (&setup.sets, &setup.stamps));
        let stamped = run(&setup, &second(), Scope::Tiles(tiles(&[20])));
        assert_eq!(stamped.stamps, all.stamps);
        assert_eq!(stamped.tiles, setup.tiles);
    }

    #[test]
    fn a_locked_set_is_untouched_by_every_scope_but_shared_pure_tiles_are_not_its() {
        let mut setup = TilesetSetup::from_auto(&first());
        let sand = setup.set_index(SAND, GRASS).expect("a set");
        setup.lock_set(sand, true).expect("known");
        let scopes = [
            Scope::All,
            Scope::Set(sand),
            Scope::Tiles(tiles(&[0, 1, 2, 3, 5, 6, 10])),
        ];
        for scope in scopes {
            let after = run(&setup, &second(), scope.clone());
            assert_eq!(after.set(SAND, GRASS), setup.set(SAND, GRASS), "{scope:?}");
            for t in [0, 1, 2, 3] {
                assert_eq!(
                    after.tiles.get(&at(t)),
                    setup.tiles.get(&at(t)),
                    "{scope:?}"
                );
            }
            let refreshed = scope != Scope::Set(sand);
            let dirt = mix(DIRT, GRASS, &[Peer::Bottom]);
            assert_eq!(
                after.tiles[&at(10)].terrains == dirt,
                refreshed,
                "{scope:?}"
            );
            let grass = (
                !after.tiles.contains_key(&at(5)),
                after.tiles.contains_key(&at(6)),
            );
            assert_eq!(grass, (refreshed, refreshed), "{scope:?}");
        }
    }

    #[test]
    fn reset_to_auto_hands_tiles_back_and_leaves_stamps() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.set_peer(at(0), Peer::Left, SAND).expect("known");
        setup.set_peer(at(40), Peer::Centre, SAND).expect("known");
        setup.set_shape(at(1), square());
        setup.set_peer(at(11), Peer::Top, SAND).expect("known");
        setup.set_peer(at(20), Peer::Top, SAND).expect("known");
        let sand = setup.set_index(SAND, GRASS).expect("a set");
        setup.lock_set(sand, true).expect("known");
        let after = reset_tiles(&setup, &first(), &[0, 1, 20, 40]);
        let auto = TilesetSetup::from_auto(&first());
        assert_eq!(after.tiles[&at(0)], auto.tiles[&at(0)]);
        assert_eq!(after.tiles[&at(1)], auto.tiles[&at(1)]);
        assert!(!after.tiles.contains_key(&at(20)));
        assert!(!after.tiles.contains_key(&at(40)));
        assert_eq!(after.tiles[&at(11)], setup.tiles[&at(11)]);
        assert_eq!(after.sets, setup.sets);
        let moved = reset_tiles(&setup, &second(), &[20]);
        assert_eq!(moved.stamps, setup.stamps);
    }

    #[test]
    fn a_user_stamp_replaces_what_it_overlaps_and_survives() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.put_stamp(stamp(&[21, 22]));
        assert_eq!(setup.stamps, [Tracked::user(stamp(&[21, 22]))]);
        let after = run(&setup, &second(), Scope::All);
        assert_eq!(after.stamps, setup.stamps);
    }

    #[test]
    fn edits_name_known_terrains_and_sets() {
        let mut setup = TilesetSetup::from_auto(&first());
        let unknown = Err(SetupError::UnknownTerrain(9));
        assert_eq!(setup.set_peer(at(0), Peer::Top, 9), unknown);
        assert_eq!(setup.rename_terrain(9, "x"), unknown);
        assert_eq!(setup.set_collision_mode(9, CollisionMode::Solid), unknown);
        let no_set = Err(SetupError::UnknownSet(5));
        let mode = setup.set_match_mode(5, MatchMode::Corners);
        assert_eq!(mode, no_set);
        assert_eq!(setup.lock_set(5, true), no_set);
        assert_eq!(setup.set_peer(at(0), Peer::Top, 0), Ok(()));
    }

    #[test]
    fn reruns_are_deterministic_and_settle() {
        let mut setup = TilesetSetup::from_auto(&first());
        setup.set_peer(at(1), Peer::Right, SAND).expect("known");
        let once = run(&setup, &second(), Scope::All);
        assert_eq!(once, run(&setup, &second(), Scope::All));
        assert_eq!(run(&once, &second(), Scope::All), once);
    }

    fn on_b() -> Vec<(u32, TileTerrains)> {
        vec![
            (100, mix(DIRT, GRASS, &[Peer::Top])),
            (101, mix(DIRT, GRASS, &[Peer::Right])),
        ]
    }

    fn sheets_run(names: &[&str], sand: Tiles, dirt: Tiles) -> AutoSetup {
        let mut sets = vec![(SAND, GRASS, MatchMode::Sides, sand)];
        let mut terrains = COLOURS[..2].to_vec();
        if !dirt.is_empty() {
            sets.push((DIRT, GRASS, MatchMode::Sides, dirt));
            terrains.push(COLOURS[2]);
        }
        let analysed: Vec<usize> = (0..names.len()).collect();
        let run = Run {
            names,
            analysed: &analysed,
            terrains: &terrains,
            sets: &sets,
            grass: &[],
            stamps: &[],
        };
        run.fresh()
    }

    fn on_two_sheets() -> TilesetSetup {
        let mut setup =
            TilesetSetup::from_auto(&sheets_run(&["a.png", "b.png"], &sand_first(), &on_b()));
        setup.set_peer(at(101), Peer::Bottom, DIRT).expect("known");
        setup
    }

    #[test]
    fn sheets_keep_their_tiles_when_reordered_or_joined() {
        let setup = on_two_sheets();
        let sand_on_b: Vec<(u32, TileTerrains)> = sand_first()
            .into_iter()
            .map(|(t, p)| (t + 100, p))
            .collect();
        let dirt_on_a: Vec<(u32, TileTerrains)> =
            on_b().into_iter().map(|(t, p)| (t - 100, p)).collect();
        let reordered = sheets_run(&["b.png", "a.png"], &sand_on_b, &dirt_on_a);
        assert_eq!(run(&setup, &reordered, Scope::All), setup);

        let sand_on_c: Vec<(u32, TileTerrains)> = (sand_first().into_iter())
            .map(|(t, p)| (t + 100, p))
            .chain([(7, TileTerrains::filled(SAND))])
            .collect();
        let dirt_on_c: Vec<(u32, TileTerrains)> =
            on_b().into_iter().map(|(t, p)| (t + 100, p)).collect();
        let joined = sheets_run(&["new.png", "a.png", "b.png"], &sand_on_c, &dirt_on_c);
        let after = run(&setup, &joined, Scope::All);
        assert_eq!(after.sheets, ["a.png", "b.png", "new.png"]);
        let new = TileTerrains::filled(SAND);
        assert_eq!(after.tiles.get(&at(207)).map(|t| t.terrains), Some(new));
        let mut without = after.clone();
        without.tiles.remove(&at(207));
        without.sheets.pop();
        assert_eq!(without, setup);
    }

    #[test]
    fn a_removed_sheet_takes_its_values_and_an_unanalysed_one_keeps_them() {
        let setup = on_two_sheets();
        let removed = run(
            &setup,
            &sheets_run(&["a.png"], &sand_first(), &[]),
            Scope::All,
        );
        assert_eq!(removed.sheets, ["a.png"]);
        assert!(removed.tiles.keys().all(|a| a.sheet == 0));
        assert!(removed.terrain(DIRT).is_none());

        let mut unanalysed = sheets_run(&["a.png", "b.png"], &sand_first(), &[]);
        unanalysed.sheets = vec![0];
        let kept = run(&setup, &unanalysed, Scope::All);
        assert_eq!(kept.sheets, setup.sheets);
        for t in [100, 101] {
            assert_eq!(kept.tiles.get(&at(t)), setup.tiles.get(&at(t)), "{t}");
        }
        assert!(kept.terrain(DIRT).is_some());
        assert_eq!(kept.set(DIRT, GRASS), setup.set(DIRT, GRASS));
    }
}
