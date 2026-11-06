//! What the live document calls an asset.
//!
//! A scene stores a path or a name and the handle is made at spawn. Written
//! `""` means the field was cleared; a field left out means its default. Paths
//! are relative to the assets folder, which is what a handle's own path already
//! is. A name, or a document path with no file, that has no asset behind it is held
//! by a placeholder handle (see [`UnresolvedReferences`]), and an asset whose file
//! went keeps that file's path.
//! An asset with no path or name, such as an unsaved material, reads as `""` until
//! an emit embeds it.

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use bevy::ecs::change_detection::Tick;

use bevy::asset::{AssetId, AssetPath, LoadState, UntypedAssetId};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::reflect::{PartialReflect, TypeRegistry};
use jackdaw_bsn::{
    AssetNames, BsnAssetContext, BsnAssetPaths, BsnPatch, BsnPatches, BsnSceneAssets, BsnValue,
    SceneBsnAst, TypeDefaults, UnencodableValue, UnresolvedReferences,
};

use crate::asset_catalog::AssetCatalog;

/// The references the editor holds right now, read one asset at a time so a
/// sync costs what the value it syncs holds. Build one per batch: the two sets
/// the lookups fall back on are built once, on first use, and an asset read once
/// is not read again.
pub(crate) struct AssetReferences<'w> {
    world: &'w World,
    server: Option<&'w AssetServer>,
    paths: Option<&'w BsnAssetPaths>,
    catalog: Option<&'w AssetCatalog>,
    unresolved: Option<&'w UnresolvedReferences>,
    embedded: OnceCell<Arc<HashMap<UntypedAssetId, String>>>,
    unsaved: OnceCell<HashSet<UntypedAssetId>>,
    spelled: RefCell<HashMap<UntypedAssetId, String>>,
}

impl<'w> AssetReferences<'w> {
    pub(crate) fn new(world: &'w World) -> Self {
        Self {
            world,
            server: world.get_resource(),
            paths: world.get_resource(),
            catalog: world.get_resource(),
            unresolved: world.get_resource(),
            embedded: OnceCell::new(),
            unsaved: OnceCell::new(),
            spelled: RefCell::default(),
        }
    }

    fn embedded(&self) -> &HashMap<UntypedAssetId, String> {
        self.embedded.get_or_init(|| embedded_names(self.world))
    }

    /// The context that names handles the way the document spells them. `None`
    /// without an asset server, which leaves a handle nothing to read a path
    /// from.
    pub(crate) fn context(&self) -> Option<BsnAssetContext<'_>> {
        Some(BsnAssetContext {
            asset_server: self.server?,
            parent_path: std::path::Path::new(""),
            asset_names: Some(self),
        })
    }

    /// Every asset [`AssetNames::name_of`] might have a name for.
    pub(super) fn known_assets(&self) -> HashSet<UntypedAssetId> {
        let paths = self.paths.into_iter().flat_map(|paths| paths.0.keys());
        let catalog = self
            .catalog
            .into_iter()
            .flat_map(|catalog| catalog.id_to_name.keys());
        let mut known: HashSet<UntypedAssetId> = paths
            .chain(self.embedded().keys())
            .chain(catalog)
            .copied()
            .collect();
        if let Some(unresolved) = self.unresolved {
            known.extend(unresolved.named_ids());
        }
        known
    }

    pub(crate) fn document_value(
        &self,
        value: &dyn PartialReflect,
        registry: &TypeRegistry,
    ) -> Result<BsnValue, UnencodableValue> {
        match self.context() {
            Some(context) => BsnValue::try_from_reflect_with_assets(value, registry, &context),
            None => BsnValue::try_from_reflect(value, registry),
        }
    }

    /// `value` as `entity`'s patch of its type. `None` when the entity has no
    /// document node.
    pub(crate) fn document_patch(
        &self,
        entity: Entity,
        value: &dyn PartialReflect,
    ) -> Option<BsnPatch> {
        let registry = self.world.resource::<AppTypeRegistry>().read();
        self.document_patch_in_pass(entity, value, &registry, &mut TypeDefaults::new(&registry))
    }

    /// [`Self::document_patch`] for a batch, which computes each type's default
    /// once through `defaults`. Defaults hold no names, so a batch can share them.
    pub(crate) fn document_patch_in_pass(
        &self,
        entity: Entity,
        value: &dyn PartialReflect,
        registry: &TypeRegistry,
        defaults: &mut TypeDefaults,
    ) -> Option<BsnPatch> {
        let ast = self.world.get_resource::<SceneBsnAst>()?;
        let node = ast.ast_for(entity)?;
        let info = value.get_represented_type_info()?;
        let mut patch = match self.context() {
            Some(context) => {
                jackdaw_bsn::component_to_bsn_patch_in_pass(value, registry, &context, defaults)
            }
            None => jackdaw_bsn::component_to_bsn_patch(value, registry),
        };
        if let Some(authored) = ast
            .find_patch_by_type_path(node, info.type_path())
            .and_then(|patch| ast.get_patch(patch))
        {
            super::save::write_clears(registry, info.type_id(), authored, &mut patch);
        }
        Some(patch)
    }
}

/// The `#Name`s of [`BsnSceneAssets`] by asset, kept until that map changes:
/// [`AssetReferences`] is made per call on many paths, and the map holds every
/// project file too.
#[derive(Resource, Default)]
pub(crate) struct EmbeddedNames(Mutex<Option<(Tick, usize, Arc<HashMap<UntypedAssetId, String>>)>>);

fn embedded_names(world: &World) -> Arc<HashMap<UntypedAssetId, String>> {
    let Some(scene) = world.get_resource_ref::<BsnSceneAssets>() else {
        return Arc::default();
    };
    let build = || {
        Arc::new(
            scene
                .0
                .iter()
                .filter(|(reference, _)| reference.starts_with('#'))
                .map(|(reference, handle)| (handle.id(), reference.clone()))
                .collect(),
        )
    };
    let Some(cache) = world.get_resource::<EmbeddedNames>() else {
        return build();
    };
    let mut cache = cache.0.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((tick, len, names)) = &*cache
        && *tick == scene.last_changed()
        && *len == scene.0.len()
    {
        return names.clone();
    }
    let names = build();
    *cache = Some((scene.last_changed(), scene.0.len(), names.clone()));
    names
}

impl AssetNames for AssetReferences<'_> {
    /// The file the index holds the asset under, else the `#Name` the document
    /// embeds it as, else the name its placeholder stands for or the path its file
    /// left, else its catalog name. An unsaved material has none.
    fn name_of(&self, id: UntypedAssetId) -> Option<String> {
        if matches!(id, UntypedAssetId::Uuid { uuid, .. } if uuid == AssetId::<Image>::DEFAULT_UUID)
        {
            return None;
        }
        if let Some(path) = self.paths.and_then(|paths| paths.0.get(&id)) {
            return Some(path.clone());
        }
        if let Some(reference) = self.embedded().get(&id) {
            return Some(reference.clone());
        }
        if let Some(name) = self.unresolved.and_then(|table| table.name_of(id)) {
            return Some(name);
        }
        let name = self.catalog?.id_to_name.get(&id)?;
        let unsaved = self
            .unsaved
            .get_or_init(|| crate::material_assets::ephemeral_material_ids(self.world));
        (!unsaved.contains(&id)).then(|| name.clone())
    }

    fn remembered(&self, id: UntypedAssetId) -> Option<String> {
        self.spelled.borrow().get(&id).cloned()
    }

    fn remember(&self, id: UntypedAssetId, reference: &str) {
        self.spelled.borrow_mut().insert(id, reference.to_string());
    }
}

/// Swap the placeholders of `brush` whose names now resolve for the assets they
/// name, as an undo record made before a name arrived brings one back. Whether
/// anything changed.
pub(crate) fn relink_brush_faces(world: &World, brush: &mut jackdaw_scene_types::Brush) -> bool {
    let (Some(unresolved), Some(scene)) = (
        world.get_resource::<UnresolvedReferences>(),
        world.get_resource::<BsnSceneAssets>(),
    ) else {
        return false;
    };
    let mut changed = false;
    for face in &mut brush.faces {
        if jackdaw_bsn::is_placeholder(&face.material)
            && let Some(handle) = unresolved
                .name_of(face.material.id().untyped())
                .and_then(|name| scene.0.get(&name))
            && handle.type_id() == std::any::TypeId::of::<StandardMaterial>()
        {
            face.material = handle.clone().typed();
            changed = true;
        }
    }
    changed
}

/// Link the live handles waiting on a reference that now resolves, field by
/// field. It reads the document only when a reference some apply left unresolved
/// is among the changed ones. A path the asset server went on to load holds its
/// asset already.
pub(crate) fn relink_arrived_references(world: &mut World) {
    let (Some(unresolved), Some(scene)) = (
        world.get_resource::<UnresolvedReferences>(),
        world.get_resource::<BsnSceneAssets>(),
    ) else {
        return;
    };
    let mut arrived = unresolved.take_arrived(|reference, type_id| {
        scene
            .0
            .get(reference)
            .is_some_and(|handle| handle.type_id() == type_id)
    });
    if let Some(server) = world.get_resource::<AssetServer>() {
        arrived.retain(|reference| !loaded_from_its_path(server, reference));
    }
    if arrived.is_empty() {
        return;
    }
    relink_references(world, &arrived.into_iter().collect());
}

/// Link the live fields the document spells `names` in. An entity holding the
/// game's values waits: a write would read as the editor's own and take those
/// values into the document, so the names are kept for when it stops holding them.
pub(crate) fn relink_references(
    world: &mut World,
    names: &bevy::platform::collections::HashSet<String>,
) {
    let mut held_back = false;
    for (entity, type_path, fields) in fields_naming_in_document(world, names) {
        if world
            .get_resource::<crate::pie_projection::PieProjection>()
            .is_some_and(|projection| projection.overlaid.contains(&entity))
        {
            held_back = true;
            continue;
        }
        let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
        jackdaw_bsn::apply_component_patch_fields(world, entity, &type_path, &fields);
    }
    if held_back
        && let Some(mut projection) =
            world.get_resource_mut::<crate::pie_projection::PieProjection>()
    {
        projection.relink_after_overlays(names);
    }
}

fn loaded_from_its_path(server: &AssetServer, reference: &str) -> bool {
    if reference.starts_with(['#', '@']) {
        return false;
    }
    AssetPath::try_parse(reference)
        .ok()
        .and_then(|path| server.get_path_id(path))
        .is_some_and(|id| matches!(server.get_load_state(id), Some(LoadState::Loaded)))
}

/// The live components whose patches hold one of `names`, and the fields of each
/// that do and whose live handle is not already what the name resolves to.
fn fields_naming_in_document(
    world: &World,
    names: &bevy::platform::collections::HashSet<String>,
) -> Vec<(Entity, String, Vec<String>)> {
    let Some(ast) = world.get_resource::<SceneBsnAst>() else {
        return Vec::new();
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    let scene = world.get_resource::<BsnSceneAssets>();
    let mut found = Vec::new();
    for (node, entity) in &ast.ast_to_ecs {
        let Ok(entity_ref) = world.get_entity(*entity) else {
            continue;
        };
        let Some(patches) = ast.world.get::<BsnPatches>(*node) else {
            continue;
        };
        for patch in patches.0.iter().filter_map(|patch| ast.get_patch(*patch)) {
            let fields = jackdaw_bsn::fields_naming(patch, names);
            if fields.is_empty() {
                continue;
            }
            let Some(type_path) = jackdaw_bsn::patch_type_path(patch) else {
                continue;
            };
            let Some(live) = registry
                .get_with_type_path(type_path)
                .and_then(|registration| registration.data::<ReflectComponent>())
                .and_then(|component| component.reflect(entity_ref))
            else {
                continue;
            };
            let fields: Vec<String> = fields
                .into_iter()
                .filter(|field| {
                    !already_linked(ast, *node, type_path, field, live, &registry, scene)
                })
                .collect();
            if !fields.is_empty() {
                found.push((*entity, type_path.to_string(), fields));
            }
        }
    }
    found
}

/// Whether the handle at `field` of `live` is the asset the document's name for it
/// resolves to, so linking it would change nothing and only remesh.
fn already_linked(
    ast: &SceneBsnAst,
    node: Entity,
    type_path: &str,
    field: &str,
    live: &dyn Reflect,
    registry: &TypeRegistry,
    scene: Option<&BsnSceneAssets>,
) -> bool {
    use bevy::reflect::GetPath as _;

    let Some(BsnValue::String(name)) = jackdaw_bsn::get_bsn_field(ast, node, type_path, field)
    else {
        return false;
    };
    let Some(resolved) = scene.and_then(|scene| scene.0.get(&name)) else {
        return false;
    };
    let Ok(held) = live.reflect_path(field) else {
        return false;
    };
    let Some(held) = held.try_as_reflect() else {
        return false;
    };
    registry
        .get_type_data::<bevy::asset::ReflectHandle>(held.reflect_type_info().type_id())
        .and_then(|handle| handle.downcast_handle_untyped(held.as_any()))
        .is_some_and(|handle| handle.id() == resolved.id())
}
