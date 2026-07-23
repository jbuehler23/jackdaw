//! World-to-text BSN scene writer.
//!
//! Tracks the `dynamic_bsn_writer` module from bevyengine/bevy#23639 (the
//! `bevy_scene2` dynamic BSN writer): [`BsnWriterConfig`] and the
//! `serialize_to_bsn` entry points keep the upstream API shape so a future
//! refresh of that PR is a copy-out of this module.
//!
//! Deltas from the PR, so the refresh diff is self-describing:
//!
//! - Value emission is not duplicated here. The writer builds a transient
//!   document ([`SceneBsnAst`]) via [`component_to_bsn_patch_with_assets`] and
//!   emits it with [`emit_scene`], which is the same machinery the editor's
//!   live-document path uses. Field-level default eliding, enum variants,
//!   `PathBuf`-as-plain-string, and list/map emission all live there instead
//!   of in private `emit_value`/`emit_enum_variant` helpers.
//! - Handle resolution goes through [`BsnAssetContext`]: paths are made
//!   relative to the emitted file's directory with `pathdiff`, and assets
//!   without a filesystem path resolve through an `asset_names` map to their
//!   `#Name`/`@Name` reference strings. The PR instead stripped an
//!   `"/assets/"` prefix from absolute paths and had no reference names.
//! - Components whose value equals their type's default still emit as a bare
//!   type path. BSN documents are sparse patches, so component presence is
//!   meaningful (marker components must survive a save/load round trip); the
//!   PR omitted such components entirely.
//! - [`BsnWriterConfig`] gains `always_save_paths`, an allow-list that
//!   overrides every skip rule. The editor needs it to keep e.g. `Visibility`
//!   while skipping the rest of `bevy_camera::visibility::`.
//! - `Name`, `ChildOf`, and `Children` are always skipped as component
//!   patches, independent of config: `Name` emits as a `#name` reference and
//!   hierarchy emits structurally as `Children [...]`. The PR relied on its
//!   default config's `bevy_ecs::hierarchy::` prefix for the latter, which
//!   `include_all()` would have disabled.
//! - Component enumeration walks the type registry (whose iteration order is
//!   stable across `App` instances of one binary) rather than the entity's
//!   archetype. Archetype component order follows lazy `ComponentId`
//!   assignment, which multithreaded loading makes nondeterministic, and the
//!   converter's determinism test requires byte-stable output.
//! - `serialize_assets_to_bsn` is the evolved implementation in
//!   [`crate::catalog`], re-exported here. It takes [`crate::CatalogAssetRef`]
//!   values instead of the PR's `(String, TypeId, UntypedAssetId)` tuples.
//! - `serialize_world_scene` and [`append_world_to_ast`] are extra
//!   lower-level entry points for callers that serialize an explicit entity
//!   set (the editor's `.jsn` converter) rather than every named world entity.

use std::any::TypeId;
use std::collections::HashSet;
use std::path::Path;

use bevy::asset::{AssetServer, UntypedAssetId};
use bevy::ecs::hierarchy::{ChildOf, Children};
use bevy::ecs::name::Name;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::ecs::world::World;
use bevy::platform::collections::HashMap;
use bevy::prelude::Entity;

use crate::{
    BsnAssetContext, BsnPatch, SceneBsnAst, component_to_bsn_patch,
    component_to_bsn_patch_with_assets, emit_scene,
};

pub use crate::catalog::serialize_assets_to_bsn;

/// Configuration for the BSN scene writer.
///
/// Controls which components and entities are included in the serialized
/// output. Create with [`BsnWriterConfig::default()`] for sensible defaults
/// that skip internal Bevy runtime components, or use
/// [`BsnWriterConfig::include_all()`] to serialize everything.
#[derive(Debug, Clone)]
pub struct BsnWriterConfig {
    /// Component type path prefixes to exclude (e.g. `"bevy_render::"`).
    pub skip_prefixes: Vec<String>,
    /// Exact component type paths to exclude.
    pub skip_paths: Vec<String>,
    /// Entity name prefixes to exclude (e.g. `"LineGizmoRenderer"`). Applies
    /// only when the writer collects entities itself
    /// ([`serialize_to_bsn_with_config`]), not to explicit entity sets.
    pub skip_entity_names: Vec<String>,
    /// Exact component type paths that are always written, overriding every
    /// skip rule. Jackdaw delta: not present in the upstream PR.
    pub always_save_paths: Vec<String>,
}

impl Default for BsnWriterConfig {
    /// Returns a config that skips internal Bevy runtime components and
    /// entities.
    fn default() -> Self {
        Self {
            skip_prefixes: vec![
                "bevy_render::".into(),
                "bevy_picking::".into(),
                "bevy_window::".into(),
                "bevy_ecs::observer::".into(),
                "bevy_ecs::hierarchy::".into(),
                "bevy_camera::primitives::".into(),
                "bevy_camera::visibility::".into(),
            ],
            skip_paths: vec![
                "bevy_transform::components::global_transform::GlobalTransform".into(),
                "bevy_transform::components::transform::TransformTreeChanged".into(),
                "bevy_light::cascade::Cascades".into(),
            ],
            skip_entity_names: vec![
                "LineGizmoRenderer".into(),
                "LineStripGizmoRenderer".into(),
                "LineJointGizmoRenderer".into(),
            ],
            always_save_paths: Vec::new(),
        }
    }
}

impl BsnWriterConfig {
    /// Returns a config that includes all components and entities.
    pub fn include_all() -> Self {
        Self {
            skip_prefixes: Vec::new(),
            skip_paths: Vec::new(),
            skip_entity_names: Vec::new(),
            always_save_paths: Vec::new(),
        }
    }

    /// Add a component type path prefix to skip during serialization.
    pub fn skip_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.skip_prefixes.push(prefix.into());
        self
    }

    /// Add an exact component type path to skip during serialization.
    pub fn skip_path(mut self, path: impl Into<String>) -> Self {
        self.skip_paths.push(path.into());
        self
    }

    /// Add an entity name prefix to skip during serialization.
    pub fn skip_entity_name(mut self, prefix: impl Into<String>) -> Self {
        self.skip_entity_names.push(prefix.into());
        self
    }

    /// Add an exact component type path that is always written, overriding
    /// every skip rule.
    pub fn always_save(mut self, path: impl Into<String>) -> Self {
        self.always_save_paths.push(path.into());
        self
    }

    fn should_skip_component(&self, type_path: &str) -> bool {
        if self.always_save_paths.iter().any(|p| p == type_path) {
            return false;
        }
        for prefix in &self.skip_prefixes {
            if type_path.starts_with(prefix.as_str()) {
                return true;
            }
        }
        self.skip_paths.iter().any(|p| p == type_path)
    }

    fn should_skip_entity(&self, name: &str) -> bool {
        self.skip_entity_names
            .iter()
            .any(|p| name.starts_with(p.as_str()))
    }
}

/// Serialize all named scene entities from the world to BSN text, using the
/// default config that skips internal Bevy components.
pub fn serialize_to_bsn(world: &World) -> String {
    serialize_to_bsn_with_config(world, &BsnWriterConfig::default())
}

/// Serialize all named scene entities from the world to BSN text.
///
/// Only entities carrying a [`Name`] are written; names matching
/// `config.skip_entity_names` are excluded.
pub fn serialize_to_bsn_with_config(world: &World, config: &BsnWriterConfig) -> String {
    let entities: Vec<Entity> = world
        .iter_entities()
        .filter(|e| {
            e.get::<Name>()
                .is_some_and(|name| !config.should_skip_entity(name.as_str()))
        })
        .map(|e| e.id())
        .collect();
    serialize_world_scene(world, &entities, Path::new(""), None, config)
}

/// Serialize an explicit set of world entities to BSN text.
///
/// `parent_path` is the directory the emitted `.bsn` file will live in; asset
/// paths are emitted relative to it. `asset_names` maps assets without a
/// filesystem path to their `#Name`/`@Name` reference strings. Hierarchy
/// among the given entities emits as nested `Children [...]` blocks; a parent
/// outside the set is treated as absent.
pub(crate) fn serialize_world_scene(
    world: &World,
    entities: &[Entity],
    parent_path: &Path,
    asset_names: Option<&HashMap<UntypedAssetId, String>>,
    config: &BsnWriterConfig,
) -> String {
    let mut ast = SceneBsnAst::default();
    append_world_to_ast(&mut ast, world, entities, parent_path, asset_names, config);
    emit_scene(&ast)
}

/// Reflect an explicit set of world entities into an existing document as
/// nodes (roots, or children of other nodes in the set).
///
/// Each entity becomes one node: a `#name` reference when it has a [`Name`],
/// then one component patch per reflectable component that passes `config`.
/// `Name`, `ChildOf`, and `Children` never emit as component patches; the
/// hierarchy is written structurally instead. Callers that pre-populate the
/// document (e.g. with named asset roots) get those roots ahead of the
/// entity roots.
pub fn append_world_to_ast(
    ast: &mut SceneBsnAst,
    world: &World,
    entities: &[Entity],
    parent_path: &Path,
    asset_names: Option<&HashMap<UntypedAssetId, String>>,
    config: &BsnWriterConfig,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>();
    let entity_set: HashSet<Entity> = entities.iter().copied().collect();
    let structural = [
        TypeId::of::<Name>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
    ];

    for entity in parent_first_order(world, entities) {
        let entity_ref = world.entity(entity);

        let mut patches = Vec::new();
        if let Some(name) = entity_ref.get::<Name>() {
            patches.push(BsnPatch::Name(name.to_string()));
        }

        for registration in reg.iter() {
            if structural.contains(&registration.type_id()) {
                continue;
            }
            let type_path = registration.type_info().type_path();
            if config.should_skip_component(type_path) {
                continue;
            }
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                continue;
            };
            let Some(reflected) = reflect_component.reflect(entity_ref) else {
                continue;
            };

            let patch = if let Some(server) = asset_server {
                let ctx = BsnAssetContext {
                    asset_server: server,
                    parent_path,
                    asset_names,
                };
                component_to_bsn_patch_with_assets(reflected.as_partial_reflect(), &reg, &ctx)
            } else {
                component_to_bsn_patch(reflected.as_partial_reflect(), &reg)
            };
            patches.push(patch);
        }

        let node = ast.create_entity_node(patches);
        let parent_ast = world
            .get::<ChildOf>(entity)
            .map(ChildOf::parent)
            .filter(|p| entity_set.contains(p))
            .and_then(|p| ast.ast_for(p));
        match parent_ast {
            Some(parent_ast) => ast.add_child_to_ast(parent_ast, node),
            None => ast.add_to_roots(node),
        }
        ast.link(entity, node);
    }
}

/// Order entities so every entity comes after its ancestors within the set.
/// Stable by original index within the same depth, so root order is the
/// caller's order.
fn parent_first_order(world: &World, entities: &[Entity]) -> Vec<Entity> {
    let set: HashSet<Entity> = entities.iter().copied().collect();
    let mut order = entities.to_vec();
    order.sort_by_key(|&entity| {
        let mut depth = 0usize;
        let mut current = entity;
        while let Some(child_of) = world.get::<ChildOf>(current) {
            let parent = child_of.parent();
            if !set.contains(&parent) {
                break;
            }
            depth += 1;
            current = parent;
            if depth > entities.len() {
                break;
            }
        }
        depth
    });
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::component::Component;
    use bevy::reflect::{Reflect, TypePath};
    use bevy::transform::components::{GlobalTransform, Transform};

    /// Stand-in for a runtime-only component the config filters by path.
    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct RuntimeState {
        frames: u32,
    }

    fn test_world() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut reg = registry.write();
            reg.register::<Transform>();
            reg.register::<GlobalTransform>();
            reg.register::<RuntimeState>();
            reg.register::<Name>();
            reg.register::<ChildOf>();
            reg.register::<Children>();
        }
        world.insert_resource(registry);
        world
    }

    #[test]
    fn serialize_to_bsn_writes_named_entities_and_hierarchy() {
        let mut world = test_world();
        let parent = world
            .spawn((Name::new("Parent"), Transform::from_xyz(1.0, 2.0, 3.0)))
            .id();
        world.spawn((Name::new("Child"), Transform::default(), ChildOf(parent)));
        world.spawn(Transform::default());

        let text = serialize_to_bsn(&world);
        assert!(text.contains("#Parent"), "{text}");
        assert!(text.contains("bevy_ecs::hierarchy::Children ["), "{text}");
        assert!(text.contains("    #Child"), "{text}");
        assert!(
            text.contains("translation:"),
            "non-default transform fields emit:\n{text}"
        );
        assert!(
            !text.contains("GlobalTransform"),
            "default config skips GlobalTransform:\n{text}"
        );
    }

    #[test]
    fn default_valued_components_still_emit_as_bare_type_paths() {
        let mut world = test_world();
        world.spawn((Name::new("Marker"), Transform::default()));

        let text = serialize_to_bsn(&world);
        assert!(
            text.contains("bevy_transform::components::transform::Transform\n"),
            "a default component must keep its presence patch:\n{text}"
        );
    }

    #[test]
    fn skip_entity_name_prefixes_exclude_entities() {
        let mut world = test_world();
        world.spawn((Name::new("Keep"), Transform::default()));
        world.spawn((Name::new("LineGizmoRenderer_0"), Transform::default()));

        let text = serialize_to_bsn(&world);
        assert!(text.contains("#Keep"), "{text}");
        assert!(!text.contains("LineGizmoRenderer"), "{text}");
    }

    #[test]
    fn always_save_overrides_skip_rules() {
        let mut world = test_world();
        world.spawn((Name::new("Thing"), RuntimeState { frames: 3 }));

        let skipping = BsnWriterConfig::include_all().skip_prefix("jackdaw_bsn::");
        let skipped = serialize_to_bsn_with_config(&world, &skipping);
        assert!(
            !skipped.contains("RuntimeState"),
            "prefix skip must exclude the component:\n{skipped}"
        );

        let config = BsnWriterConfig::include_all()
            .skip_prefix("jackdaw_bsn::")
            .always_save(RuntimeState::type_path());
        let kept = serialize_to_bsn_with_config(&world, &config);
        assert!(
            kept.contains("RuntimeState"),
            "always_save must win over the prefix skip:\n{kept}"
        );
    }

    #[test]
    fn include_all_still_emits_hierarchy_structurally() {
        let mut world = test_world();
        let parent = world.spawn((Name::new("P"), Transform::default())).id();
        world.spawn((Name::new("C"), ChildOf(parent)));

        let text = serialize_to_bsn_with_config(&world, &BsnWriterConfig::include_all());
        assert!(
            !text.contains("ChildOf"),
            "ChildOf must never emit as a component patch:\n{text}"
        );
        assert!(text.contains("bevy_ecs::hierarchy::Children ["), "{text}");
    }
}
