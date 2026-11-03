//! The document as the history records it, kept from one capture to the next
//! so that a capture reads again only the nodes that changed since the last.
//!
//! A node is read again when one of its document patches changed, when a
//! handle field it took from its entity changed, or when it is a widget whose
//! saved form depends on its entity's state. A prefab instance or an embedded
//! asset is read again whole when anything under it changed. A change to
//! anything that names assets or prefabs for the whole document (the prefab
//! cache, the asset catalog, the scene's embedded assets) sends the capture
//! back to reading the whole document, as does a document that names runtime
//! assets of its own.

use std::path::Path;
use std::sync::Arc;

use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::ecs::world::WorldId;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw_bsn::{BsnPatch, BsnPatches, SceneBsnAst};

use crate::document_units::{Unit, UnitKey, Units, is_instance, key_of};

/// What the last capture read, kept so the next one can start from it.
#[derive(Resource, Default)]
pub struct DocumentCapture {
    last: Option<Captured>,
}

impl DocumentCapture {
    /// Forget the last capture, so the next one reads the whole document.
    pub fn forget(&mut self) {
        self.last = None;
    }
}

struct Captured {
    parent_path: std::path::PathBuf,
    document: WorldId,
    document_tick: Tick,
    world_tick: Tick,
    inputs: Inputs,
    units: Arc<Units>,
    handles: HashMap<Entity, Vec<ComponentId>>,
    names_new_assets: bool,
}

/// What one read of the document produced.
struct Read {
    units: Units,
    handles: HashMap<Entity, Vec<ComponentId>>,
    names_new_assets: bool,
}

/// The document cut into units as a save writes it, or `None` when it cannot
/// be cut (a node without an id).
pub(crate) fn capture(world: &mut World, parent_path: &Path) -> Option<Arc<Units>> {
    let last = world
        .get_resource_mut::<DocumentCapture>()
        .and_then(|mut capture| capture.last.take());
    let read = last
        .and_then(|last| read_changed(world, parent_path, last))
        .or_else(|| read_whole(world, parent_path));
    let inputs = Inputs::of(world);
    let marked = mark_document(world);
    let world_tick = world.change_tick();
    world.increment_change_tick();
    let (read, (document, document_tick)) = read.zip(marked)?;
    let units = Arc::new(read.units);
    world.get_resource_or_init::<DocumentCapture>().last = Some(Captured {
        parent_path: parent_path.to_path_buf(),
        document,
        document_tick,
        world_tick,
        inputs,
        units: units.clone(),
        handles: read.handles,
        names_new_assets: read.names_new_assets,
    });
    Some(units)
}

/// The live document's identity and current change tick, moving the tick on
/// so every later change to the document is newer than what this capture read.
fn mark_document(world: &mut World) -> Option<(WorldId, Tick)> {
    let mut live = world.get_resource_mut::<SceneBsnAst>()?;
    let document = &mut live.bypass_change_detection().world;
    let tick = document.change_tick();
    document.increment_change_tick();
    Some((document.id(), tick))
}

fn read_whole(world: &mut World, parent_path: &Path) -> Option<Read> {
    let authored = crate::scene_io::save::authored_live_for_history(world, parent_path)?;
    let registry = world.resource::<AppTypeRegistry>().clone();
    let units = Units::split(&authored.ast, &registry.read())?;
    Some(Read {
        units,
        handles: handle_components(world, &authored.handles),
        names_new_assets: authored.names_new_assets,
    })
}

fn read_changed(world: &mut World, parent_path: &Path, last: Captured) -> Option<Read> {
    if last.names_new_assets
        || last.parent_path != parent_path
        || Inputs::of(world).changed_since(&last.inputs, last.world_tick, world)
    {
        return None;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let plan = {
        let live = world.get_resource::<SceneBsnAst>()?;
        if live.world.id() != last.document {
            return None;
        }
        Plan::of(world, live, &registry.read(), &last)?
    };
    if plan.reread.len() > 64 && plan.reread.len() * 4 > plan.layout.len() {
        return None;
    }

    let mut partial = SceneBsnAst::default();
    let copies: Vec<(UnitKey, Entity, bool)> = {
        let live = world.resource::<SceneBsnAst>();
        plan.reread
            .iter()
            .map(|(key, node, whole)| {
                let copy = match whole {
                    true => copy_subtree(&mut partial, live, *node),
                    false => copy_node(&mut partial, live, *node),
                };
                (key.clone(), copy, *whole)
            })
            .collect()
    };
    let authored = crate::scene_io::save::author_for_history(world, partial, parent_path);
    if authored.names_new_assets {
        return None;
    }
    let mut texts: HashMap<UnitKey, Arc<str>> = copies
        .into_iter()
        .map(|(key, copy, whole)| {
            let text = match whole {
                true => jackdaw_bsn::emit_entity(&authored.ast, copy),
                false => jackdaw_bsn::emit_entity_without_children(&authored.ast, copy),
            };
            (key, Arc::from(text))
        })
        .collect();

    let units = plan
        .layout
        .into_iter()
        .map(|layout| {
            let text = texts
                .remove(&layout.key)
                .or_else(|| last.units.0.get(&layout.key).map(|unit| unit.text.clone()))?;
            let unit = Unit {
                parent: layout.parent,
                follows: layout.follows,
                whole: layout.whole,
                text,
            };
            Some((layout.key, unit))
        })
        .collect::<Option<HashMap<UnitKey, Unit>>>()?;

    let mut handles = last.handles;
    handles.retain(|entity, _| {
        !plan.reread_entities.contains(entity) && world.get_entity(*entity).is_ok()
    });
    handles.extend(handle_components(world, &authored.handles));
    Some(Read {
        units: Units(units),
        handles,
        names_new_assets: false,
    })
}

/// Where each unit of the live document sits, and which units to read again.
#[derive(Default)]
struct Plan {
    layout: Vec<Layout>,
    reread: Vec<(UnitKey, Entity, bool)>,
    reread_entities: HashSet<Entity>,
}

struct Layout {
    key: UnitKey,
    parent: Option<UnitKey>,
    follows: Option<UnitKey>,
    whole: bool,
}

impl Plan {
    /// Walk the live document the way [`Units::split`] walks a saved one, or
    /// `None` when a node has no id or shares one.
    fn of(
        world: &World,
        live: &SceneBsnAst,
        registry: &TypeRegistry,
        last: &Captured,
    ) -> Option<Self> {
        let assets: HashSet<Entity> = jackdaw_bsn::asset_roots(live, registry)
            .into_iter()
            .collect();
        let changes = Changes {
            world,
            live,
            document_since: last.document_tick,
            document_now: live.world.read_change_tick(),
            world_since: last.world_tick,
            world_now: world.read_change_tick(),
            handles: &last.handles,
        };
        let mut plan = Self::default();
        let mut seen: HashSet<UnitKey> = HashSet::new();
        let mut pending: Vec<(Entity, Option<UnitKey>, Option<UnitKey>)> = Vec::new();
        queue_siblings(&mut pending, live, &assets, &live.roots, None)?;
        while let Some((node, parent, follows)) = pending.pop() {
            let key = key_of(live, &assets, node)?;
            if !seen.insert(key.clone()) {
                return None;
            }
            let whole = assets.contains(&node) || is_instance(live, node);
            let changed = match whole {
                true => changes.subtree(node),
                false => changes.node(node),
            };
            let kept = last
                .units
                .0
                .get(&key)
                .is_some_and(|unit| unit.whole == whole);
            if changed || !kept {
                plan.reread.push((key.clone(), node, whole));
                plan.reread_entities.extend(match whole {
                    true => std::iter::once(node)
                        .chain(live.descendants_of(node))
                        .filter_map(|node| live.ecs_for_ast(node))
                        .collect::<Vec<Entity>>(),
                    false => live.ecs_for_ast(node).into_iter().collect(),
                });
            }
            if !whole {
                queue_siblings(
                    &mut pending,
                    live,
                    &assets,
                    &live.get_children_ast(node),
                    Some(key.clone()),
                )?;
            }
            plan.layout.push(Layout {
                key,
                parent,
                follows,
                whole,
            });
        }
        Some(plan)
    }
}

/// Queue `siblings` under `parent`, each following the one before it.
fn queue_siblings(
    pending: &mut Vec<(Entity, Option<UnitKey>, Option<UnitKey>)>,
    live: &SceneBsnAst,
    assets: &HashSet<Entity>,
    siblings: &[Entity],
    parent: Option<UnitKey>,
) -> Option<()> {
    let mut follows = None;
    for &sibling in siblings {
        let key = key_of(live, assets, sibling)?;
        pending.push((sibling, parent.clone(), follows.replace(key)));
    }
    Some(())
}

/// What changed in the live document and its entities since the last capture.
struct Changes<'a> {
    world: &'a World,
    live: &'a SceneBsnAst,
    document_since: Tick,
    document_now: Tick,
    world_since: Tick,
    world_now: Tick,
    handles: &'a HashMap<Entity, Vec<ComponentId>>,
}

impl Changes<'_> {
    fn subtree(&self, root: Entity) -> bool {
        self.node(root)
            || self
                .live
                .descendants_of(root)
                .into_iter()
                .any(|node| self.node(node))
    }

    fn node(&self, node: Entity) -> bool {
        self.patches_changed(node) || self.entity_changed(node)
    }

    fn patches_changed(&self, node: Entity) -> bool {
        let newer = |tick: Tick| tick.is_newer_than(self.document_since, self.document_now);
        let Some(patches) = self
            .live
            .world
            .get_entity(node)
            .ok()
            .and_then(|node| node.get_ref::<BsnPatches>())
        else {
            return true;
        };
        newer(patches.last_changed())
            || patches.0.iter().any(|&patch| {
                self.live
                    .world
                    .get_entity(patch)
                    .ok()
                    .and_then(|patch| patch.get_ref::<BsnPatch>())
                    .is_none_or(|patch| newer(patch.last_changed()))
            })
    }

    /// Whether the entity's state that its saved form reads changed: a handle
    /// field the document took from it, or anything about a widget whose
    /// saved form drops what the widget derives.
    fn entity_changed(&self, node: Entity) -> bool {
        let Some(entity) = self
            .live
            .ecs_for_ast(node)
            .and_then(|entity| self.world.get_entity(entity).ok())
        else {
            return false;
        };
        if derives_saved_fields(&entity) {
            return true;
        }
        self.handles.get(&entity.id()).is_some_and(|components| {
            components.iter().any(|&component| {
                entity
                    .get_change_ticks_by_id(component)
                    .is_none_or(|ticks| ticks.is_changed(self.world_since, self.world_now))
            })
        })
    }
}

/// Whether `entity` is a widget whose saved form depends on its own state or
/// its parent's.
fn derives_saved_fields(entity: &EntityRef) -> bool {
    entity.contains::<bevy::feathers::controls::ButtonVariant>()
        || entity.contains::<jackdaw_widgets_runtime::ProgressFill>()
        || entity.contains::<jackdaw_widgets_runtime::Separator>()
        || entity.contains::<jackdaw_widgets_runtime::NineSlice>()
}

/// The resources that name assets or prefabs for the whole document, by when
/// each last changed.
struct Inputs([Option<Tick>; 5]);

impl Inputs {
    fn of(world: &World) -> Self {
        fn changed<R: Resource>(world: &World) -> Option<Tick> {
            world
                .get_resource_ref::<R>()
                .map(|resource| resource.last_changed())
        }
        Self([
            changed::<crate::prefab::PrefabAstCache>(world),
            changed::<jackdaw_bsn::BsnSceneAssets>(world),
            changed::<jackdaw_bsn::BsnAssetPaths>(world),
            changed::<crate::asset_catalog::AssetCatalog>(world),
            changed::<crate::material_assets::MaterialRegistry>(world),
        ])
    }

    fn changed_since(&self, last: &Inputs, since: Tick, world: &World) -> bool {
        let now = world.read_change_tick();
        self.0
            .iter()
            .zip(&last.0)
            .any(|(input, was)| match (input, was) {
                (Some(tick), Some(_)) => tick.is_newer_than(since, now),
                (None, None) => false,
                _ => true,
            })
    }
}

/// The components behind each `(entity, type path)` the read took handle
/// fields from.
fn handle_components(
    world: &World,
    handles: &[(Entity, String)],
) -> HashMap<Entity, Vec<ComponentId>> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let mut components: HashMap<Entity, Vec<ComponentId>> = HashMap::new();
    for (entity, type_path) in handles {
        let Some(component) = registry
            .get_with_type_path(type_path)
            .and_then(|registration| world.components().get_id(registration.type_id()))
        else {
            continue;
        };
        components.entry(*entity).or_default().push(component);
    }
    components
}

/// Copy `node`'s own patches into `partial` as a root, linked to its entity.
fn copy_node(partial: &mut SceneBsnAst, live: &SceneBsnAst, node: Entity) -> Entity {
    let copy = partial.create_entity_node(live.cloned_component_patches(node));
    partial.add_to_roots(copy);
    if let Some(entity) = live.ecs_for_ast(node) {
        partial.link(entity, copy);
    }
    copy
}

/// Copy `node` and everything under it into `partial` as a root, each copy
/// linked to its node's entity.
fn copy_subtree(partial: &mut SceneBsnAst, live: &SceneBsnAst, node: Entity) -> Entity {
    let copy = jackdaw_bsn::clone_subtree_into(partial, live, node, None);
    link_copies(partial, live, node, copy);
    copy
}

fn link_copies(partial: &mut SceneBsnAst, live: &SceneBsnAst, node: Entity, copy: Entity) {
    if let Some(entity) = live.ecs_for_ast(node) {
        partial.link(entity, copy);
    }
    let copies = partial.get_children_ast(copy);
    for (child, child_copy) in live.get_children_ast(node).into_iter().zip(copies) {
        link_copies(partial, live, child, child_copy);
    }
}
