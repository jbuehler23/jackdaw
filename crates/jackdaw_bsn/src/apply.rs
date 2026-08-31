//! Apply BSN AST patches to ECS entities via reflection.
//!
//! [`apply_dirty_ast_patches`] processes entities marked [`AstDirty`],
//! reading their patches from the AST and inserting the corresponding
//! ECS components. Called explicitly during scene load and paste operations.

use std::any::TypeId;

use bevy::asset::{AssetServer, ReflectHandle};
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::reflect::{
    PartialReflect, ReflectFromReflect, ReflectMut, TypeRegistry,
    enums::{DynamicEnum, DynamicVariant, Enum},
    list::DynamicList,
    map::{DynamicMap, Map},
    prelude::ReflectDefault,
};

use crate::{
    AstNodeRef, BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};

/// Marker component: the entity's AST patches have changed and need to be
/// applied to ECS.
#[derive(Component)]
pub struct AstDirty;

/// Assets available while applying document values: the asset server for
/// path-string handles, plus the scene's local named assets so `#Name` and
/// `@Name` reference strings resolve to their loaded handles.
pub struct BsnApplyAssets<'a> {
    pub server: &'a AssetServer,
    pub local: Option<&'a bevy::platform::collections::HashMap<String, bevy::asset::UntypedHandle>>,
}

/// The scene's named local assets (`#Name` inline entries and `@Name`
/// catalog entries), kept as a resource so document applies after load can
/// resolve reference strings.
#[derive(Resource, Default)]
pub struct BsnSceneAssets(
    pub bevy::platform::collections::HashMap<String, bevy::asset::UntypedHandle>,
);

/// Type paths the open project's extracted schema reports and the editor has
/// no ECS registration for. Project components live in the document and become
/// real components only in the game binary, so apply skips a patch naming one
/// without the "not in the registry" warning an unknown type earns.
///
/// Absent in the game runtime, which registers the project's types and so
/// treats an unregistered type as a fault. The set is only as fresh as the
/// last extraction: a schema still listing a type the game has since deleted
/// silences a document naming it, until a rebuild.
#[derive(Resource, Default)]
pub struct DocumentOnlyTypes {
    types: HashSet<String>,
    /// The enums among them. An authored variant spells a path the schema
    /// never lists on its own (`Team::Red` against a reported `Team`), so only
    /// these answer for a name one segment longer.
    enums: HashSet<String>,
}

impl DocumentOnlyTypes {
    /// Build the set from the project's component type paths and the subset of
    /// those that are enums.
    pub fn new(types: HashSet<String>, enums: HashSet<String>) -> Self {
        Self { types, enums }
    }

    /// Whether the project's schema accounts for `type_path`, directly or as a
    /// variant of one of its enums.
    pub fn covers(&self, type_path: &str) -> bool {
        if self.types.contains(type_path) {
            return true;
        }
        type_path
            .rsplit_once("::")
            .is_some_and(|(owner, _)| self.enums.contains(owner))
    }
}

/// Type paths the last apply resolved to nothing: absent from the registry and
/// from [`DocumentOnlyTypes`] both. Loaders read this to report once that the
/// project needs rebuilding. Absent in the game runtime, alongside
/// [`DocumentOnlyTypes`].
#[derive(Resource, Default)]
pub struct UnresolvedTypes {
    types: std::collections::BTreeSet<String>,
    /// What the last emitted remedy named. A scene reloads on every undo
    /// restore and prefab watcher reload, so an unchanged remedy stays quiet
    /// instead of repeating per reload.
    reported: std::collections::BTreeSet<String>,
}

impl UnresolvedTypes {
    /// The type paths the last apply could not resolve.
    pub fn types(&self) -> &std::collections::BTreeSet<String> {
        &self.types
    }

    /// Whether the last apply resolved everything it was given.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Forget the previous scene's findings, keeping what was last reported so
    /// an unchanged remedy stays quiet across reloads.
    pub fn start_scene(&mut self) {
        self.types.clear();
    }

    /// The remedy to log, or `None` when there is nothing new to say.
    pub fn take_remedy(&mut self) -> Option<String> {
        if self.types.is_empty() || self.types == self.reported {
            return None;
        }
        self.reported = self.types.clone();
        Some(unresolved_remedy(&self.types))
    }
}

/// The line a load reports when a type resolved to nothing: what is missing,
/// and the rebuild that would supply it.
fn unresolved_remedy(types: &std::collections::BTreeSet<String>) -> String {
    let plural = if types.len() == 1 { "" } else { "s" };
    format!(
        "{} type{plural} in this scene {} in neither the editor's registry nor \
         the project's extracted schema ({}); rebuild the project so the editor \
         picks up its types.",
        types.len(),
        if types.len() == 1 { "is" } else { "are" },
        types
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Record `type_path` as unapplied and report whether it is document-only.
/// `true` means the caller stays quiet: the type is a known project component
/// and the document is where it belongs.
fn note_unapplied(world: &mut World, type_path: &str) -> bool {
    if world
        .get_resource::<DocumentOnlyTypes>()
        .is_some_and(|known| known.covers(type_path))
    {
        return true;
    }
    if let Some(mut unresolved) = world.get_resource_mut::<UnresolvedTypes>() {
        unresolved.types.insert(type_path.to_string());
    }
    false
}

/// Spawn ECS entities from the [`SceneBsnAst`] resource, linking them back to
/// AST nodes. All entities are marked [`AstDirty`] so a following call to
/// [`apply_dirty_ast_patches`] populates ECS components.
pub fn spawn_from_ast(world: &mut World) -> Vec<Entity> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let roots = {
        let reg = registry.read();
        let ast = world.resource::<SceneBsnAst>();
        // Named asset entries stay in the document for round-trip save but
        // are not scene entities; the loader routes them into asset stores.
        crate::catalog::entity_roots(ast, &reg)
    };
    let mut spawned = Vec::new();

    for root in roots {
        spawn_ast_node(world, root, None, &mut spawned);
    }

    spawned
}

/// Spawn an ECS entity for `ast_entity` (and recursively its authored children),
/// link them in [`SceneBsnAst`], and mark them [`AstDirty`]. `parent` is the
/// live ECS parent (`ChildOf`), when any. Callers must follow with
/// [`apply_dirty_ast_patches`].
pub fn spawn_ast_node(
    world: &mut World,
    ast_entity: Entity,
    parent: Option<Entity>,
    spawned: &mut Vec<Entity>,
) {
    spawn_ast_node_to_depth(world, ast_entity, parent, spawned, 0);
}

/// [`spawn_ast_node`] counting how deep it has gone, so a document whose
/// `Children` lists form a cycle stops at [`crate::MAX_AST_DEPTH`] rather
/// than spawning entities until memory runs out.
fn spawn_ast_node_to_depth(
    world: &mut World,
    ast_entity: Entity,
    parent: Option<Entity>,
    spawned: &mut Vec<Entity>,
    depth: usize,
) {
    if depth >= crate::MAX_AST_DEPTH {
        log::warn!(
            "document node {ast_entity} is deeper than {}; it was not spawned",
            crate::MAX_AST_DEPTH
        );
        return;
    }
    let ecs_entity = world
        .spawn((
            AstNodeRef {
                patches_entity: ast_entity,
            },
            AstDirty,
        ))
        .id();

    if let Some(parent) = parent {
        world.entity_mut(ecs_entity).insert(ChildOf(parent));
    }

    // Link ECS to AST in the resource.
    world
        .resource_mut::<SceneBsnAst>()
        .link(ecs_entity, ast_entity);

    spawned.push(ecs_entity);

    // Recurse into children.
    let children_ast = {
        let ast = world.resource::<SceneBsnAst>();
        let Some(patches) = ast.get_patches(ast_entity) else {
            return;
        };
        let mut children = Vec::new();
        for &pe in &patches.0 {
            if let Some(BsnPatch::Children(child_list)) = ast.get_patch(pe) {
                children.extend(child_list.iter().copied());
            }
        }
        children
    };

    for child_ast in children_ast {
        spawn_ast_node_to_depth(world, child_ast, Some(ecs_entity), spawned, depth + 1);
    }
}

/// Applies AST patches to all dirty entities, then removes the marker. Called
/// explicitly during scene load; it is not registered as a per-frame system.
pub fn apply_dirty_ast_patches(world: &mut World) {
    let dirty: Vec<Entity> = world
        .query_filtered::<Entity, With<AstDirty>>()
        .iter(world)
        .collect();

    for entity in dirty {
        apply_ast_to_ecs(world, entity);
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<AstDirty>();
        }
    }
}

/// Read an entity's AST patches and apply them to the ECS entity via reflection.
pub fn apply_ast_to_ecs(world: &mut World, entity: Entity) {
    let Some(ast_ref) = world.get::<AstNodeRef>(entity) else {
        return;
    };
    let patches_entity = ast_ref.patches_entity;

    let ast = world.resource::<SceneBsnAst>();
    let Some(patches) = ast.get_patches(patches_entity) else {
        return;
    };

    // Clone patch data to avoid borrow conflicts with world mutations.
    let patch_entities: Vec<Entity> = patches.0.clone();
    let mut patch_data = Vec::new();
    for &pe in &patch_entities {
        if let Some(patch) = ast.get_patch(pe) {
            patch_data.push(patch.clone());
        }
    }

    // Sibling entities are comma-separated; a hand-edited file that
    // drops the comma silently folds the next entity's patches into
    // this one, with only the extra Name patch as evidence. Merged
    // camera-plus-light entities from exactly this mistake shipped in
    // the scaffold template once, so the trap deserves a loud pointer.
    let names = patch_data
        .iter()
        .filter(|p| matches!(p, BsnPatch::Name(_)))
        .count();
    if names > 1 {
        log::warn!(
            "BSN entity has {names} #names; a missing comma between sibling \
             entities merges them into one. Check the scene file for a \
             patch group holding more than one #name."
        );
    }

    for patch in patch_data {
        match patch {
            BsnPatch::Name(name) => {
                world.entity_mut(entity).insert(Name::new(name));
            }
            BsnPatch::Type(ref type_path) => {
                apply_type_patch(world, entity, type_path);
            }
            BsnPatch::Struct(ref data) => {
                apply_struct_patch(world, entity, data);
            }
            BsnPatch::TupleStruct(ref data) => {
                apply_tuple_struct_patch(world, entity, data);
            }
            // Base, Template, Children handled elsewhere.
            _ => {}
        }
    }
}

/// Apply a single component patch to an ECS entity. The editor's field-edit
/// command uses this to mirror a document change onto the live entity through
/// the same code paths as scene load.
pub fn apply_component_patch(world: &mut World, entity: Entity, patch: &BsnPatch) {
    match patch {
        BsnPatch::Type(type_path) => apply_type_patch(world, entity, type_path),
        BsnPatch::Struct(data) => apply_struct_patch(world, entity, data),
        BsnPatch::TupleStruct(data) => apply_tuple_struct_patch(world, entity, data),
        BsnPatch::Name(_)
        | BsnPatch::Base(_)
        | BsnPatch::Template(_, _)
        | BsnPatch::Children(_) => {}
    }
}

/// Apply a bare type patch (unit struct or enum variant with all defaults).
fn apply_type_patch(world: &mut World, entity: Entity, type_path: &str) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    // Try as a direct type first.
    if let Some(registration) = reg.get_with_type_path(type_path) {
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            log::warn!("cannot apply '{type_path}': no ReflectDefault registered");
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            log::warn!("cannot apply '{type_path}': not a reflectable component");
            return;
        };
        let value = reflect_default.default();
        reflect_component.insert(
            &mut world.entity_mut(entity),
            value.as_partial_reflect(),
            &reg,
        );
        return;
    }

    // Try as an enum variant: split off last `::` segment.
    if let Some(last_sep) = type_path.rfind("::") {
        let enum_path = &type_path[..last_sep];
        let variant_name = &type_path[last_sep + 2..];

        let Some(registration) = reg.get_with_type_path(enum_path) else {
            if !note_unapplied(world, type_path) {
                log::warn!("cannot apply '{type_path}': type not in the registry");
            }
            return;
        };
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            log::warn!("cannot apply '{enum_path}': no ReflectDefault registered");
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            log::warn!("cannot apply '{enum_path}': not a reflectable component");
            return;
        };

        let mut value = reflect_default.default();
        if let ReflectMut::Enum(e) = value.reflect_mut() {
            let dynamic_enum = DynamicEnum::new(variant_name, DynamicVariant::Unit);
            if !apply_enum_variant(e, &dynamic_enum, type_path) {
                return;
            }
        }
        reflect_component.insert(
            &mut world.entity_mut(entity),
            value.as_partial_reflect(),
            &reg,
        );
    }
}

/// Switch `target` to the variant `dynamic` names, refusing rather than dying
/// on one the enum cannot take.
///
/// `PartialReflect::apply` panics on every mismatch a hand-edited document can
/// write: a variant the type does not declare (an associated constant such as
/// `Color::WHITE` reads exactly like a unit variant in a type path), or a
/// variant named bare or half-filled when it carries fields. The editor's own
/// writer always emits the full field set, so those spellings come from a
/// person typing into the file.
///
/// Returns whether the variant was applied.
fn apply_enum_variant(target: &mut dyn Enum, dynamic: &DynamicEnum, type_path: &str) -> bool {
    match target.try_apply(dynamic) {
        Ok(()) => true,
        Err(err) => {
            // The error names the variant and, for an unknown one, what was
            // looked for; the enum's own list of variants is what it cannot
            // know to add.
            let variants = match target.get_represented_type_info() {
                Some(bevy::reflect::TypeInfo::Enum(info)) => info.variant_names().join(", "),
                _ => String::new(),
            };
            log::warn!("cannot apply '{type_path}': {err} (the type has {variants})");
            false
        }
    }
}

/// Apply a struct patch: merge specified fields over existing component (or
/// default if it doesn't exist yet). Nested struct fields are merged
/// recursively so that partial patches like `Transform { translation: Vec3 { x: 5.0 } }`
/// only update the specified sub-fields.
/// Build a `DynamicStruct` carrying the patch's fields, typed from the struct's
/// reflected field info and tagged with its represented type. Used when a struct
/// component has no `ReflectDefault` to seed from and is not already present on
/// the entity, so the patch fields alone define the value. Fields the struct
/// does not declare are ignored; fields the patch omits are left unset, which a
/// strict `FromReflect` will reject for a type without a default.
fn dynamic_struct_from_patch(
    registration: &bevy::reflect::TypeRegistration,
    data: &BsnStructData,
    reg: &TypeRegistry,
    assets_ctx: Option<&BsnApplyAssets>,
) -> bevy::reflect::structs::DynamicStruct {
    let field_types: std::collections::HashMap<String, std::any::TypeId> =
        match registration.type_info() {
            bevy::reflect::TypeInfo::Struct(struct_info) => (0..struct_info.field_len())
                .filter_map(|i| struct_info.field_at(i))
                .map(|field_info| (field_info.name().to_string(), field_info.type_id()))
                .collect(),
            _ => std::collections::HashMap::new(),
        };

    let mut dynamic_struct = bevy::reflect::structs::DynamicStruct::default();
    dynamic_struct.set_represented_type(Some(registration.type_info()));
    for field in &data.fields.0 {
        let Some(&field_type_id) = field_types.get(&field.name) else {
            continue;
        };
        if let Some(reflected) = bsn_value_to_reflect(&field.value, field_type_id, reg, assets_ctx)
        {
            dynamic_struct.insert_boxed(&field.name, reflected);
        }
    }
    dynamic_struct
}

fn apply_struct_patch(world: &mut World, entity: Entity, data: &BsnStructData) {
    let server = world.get_resource::<AssetServer>().cloned();
    let local = world.get_resource::<BsnSceneAssets>().map(|r| r.0.clone());
    let assets_ctx = server.as_ref().map(|s| BsnApplyAssets {
        server: s,
        local: local.as_ref(),
    });
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    // Direct lookup: the type_path is a struct component.
    if let Some(registration) = reg.get_with_type_path(&data.type_path) {
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };
        let reflect_default = registration.data::<ReflectDefault>();

        let mut value: Box<dyn PartialReflect> = {
            let Ok(entity_ref) = world.get_entity(entity) else {
                return;
            };
            if let Some(existing) = reflect_component.reflect(entity_ref) {
                existing.to_dynamic()
            } else if let Some(reflect_default) = reflect_default {
                reflect_default.default().into_partial_reflect()
            } else {
                // No existing component and no ReflectDefault to seed from:
                // build the struct fieldwise from the patch so components that
                // do not derive Default (such as the prefab IsA marker) still
                // apply instead of being silently dropped.
                Box::new(dynamic_struct_from_patch(
                    registration,
                    data,
                    &reg,
                    assets_ctx.as_ref(),
                ))
            }
        };

        if let ReflectMut::Struct(s) = value.reflect_mut() {
            for field in &data.fields.0 {
                if let Some(target) = s.field_mut(&field.name) {
                    merge_bsn_value_into_reflect(target, &field.value, &reg, assets_ctx.as_ref());
                } else {
                    log::warn!("unknown field '{}' on '{}'", field.name, data.type_path);
                }
            }
        }

        reflect_component.insert(
            &mut world.entity_mut(entity),
            value.as_partial_reflect(),
            &reg,
        );
        return;
    }

    // Enum variant lookup: type_path is "EnumType::Variant" with struct fields.
    if let Some(last_sep) = data.type_path.rfind("::") {
        let enum_path = &data.type_path[..last_sep];
        let variant_name = &data.type_path[last_sep + 2..];

        let Some(registration) = reg.get_with_type_path(enum_path) else {
            if !note_unapplied(world, &data.type_path) {
                log::warn!(
                    "cannot apply '{}': type not in the registry",
                    data.type_path
                );
            }
            return;
        };
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };

        // Read the existing component if present (preserves current variant).
        // Only fall back to default() if the component doesn't exist yet.
        let mut value: Box<dyn PartialReflect> = {
            let Ok(entity_ref) = world.get_entity(entity) else {
                return;
            };
            if let Some(existing) = reflect_component.reflect(entity_ref) {
                existing.to_dynamic()
            } else {
                reflect_default.default().into_partial_reflect()
            }
        };

        if let ReflectMut::Enum(e) = value.reflect_mut() {
            if e.variant_name() == variant_name {
                // Same variant: update fields in place (common case: field edit).
                for field in &data.fields.0 {
                    if let Some(target) = e.field_mut(&field.name)
                        && let Some(type_info) = target.get_represented_type_info()
                        && let Some(reflected) = bsn_value_to_reflect(
                            &field.value,
                            type_info.type_id(),
                            &reg,
                            assets_ctx.as_ref(),
                        )
                    {
                        apply_authored_value(target, &*reflected);
                    }
                }
            } else {
                // Different variant: switch to it (variant change via BSN apply).
                let variant_field_types: std::collections::HashMap<String, std::any::TypeId> = e
                    .get_represented_type_info()
                    .and_then(|info| {
                        if let bevy::reflect::TypeInfo::Enum(enum_info) = info
                            && let Some(bevy::reflect::enums::VariantInfo::Struct(struct_info)) =
                                enum_info.variant(variant_name)
                        {
                            let mut map = std::collections::HashMap::new();
                            for i in 0..struct_info.field_len() {
                                let fi = struct_info.field_at(i).unwrap();
                                map.insert(fi.name().to_string(), fi.type_id());
                            }
                            return Some(map);
                        }
                        None
                    })
                    .unwrap_or_default();

                let mut dynamic_struct = bevy::reflect::structs::DynamicStruct::default();
                for field in &data.fields.0 {
                    let field_type_id = variant_field_types
                        .get(&field.name)
                        .copied()
                        .unwrap_or(std::any::TypeId::of::<f32>());
                    if let Some(reflected) =
                        bsn_value_to_reflect(&field.value, field_type_id, &reg, assets_ctx.as_ref())
                    {
                        dynamic_struct.insert_boxed(&field.name, reflected);
                    }
                }
                let dynamic_enum =
                    DynamicEnum::new(variant_name, DynamicVariant::Struct(dynamic_struct));
                if !apply_enum_variant(e, &dynamic_enum, &data.type_path) {
                    return;
                }
            }
        }

        reflect_component.insert(
            &mut world.entity_mut(entity),
            value.as_partial_reflect(),
            &reg,
        );
    }
}

/// Recursively merge a BSN value into an existing reflected value.
/// For struct values, only the specified sub-fields are updated; unmentioned
/// fields keep their current value. For primitives, the value is replaced.
fn merge_bsn_value_into_reflect(
    target: &mut dyn PartialReflect,
    value: &BsnValue,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) {
    if let BsnValue::Struct(data) = value
        && let ReflectMut::Struct(s) = target.reflect_mut()
    {
        for field in &data.fields.0 {
            if let Some(target_field) = s.field_mut(&field.name) {
                merge_bsn_value_into_reflect(target_field, &field.value, registry, assets);
            }
        }
        return;
    }
    // Everything else is converted whole and written over the target,
    // including a braced value on a field that is not a struct, which is an
    // enum variant carrying fields. Merging field-by-field is not open to
    // those: the target may be sitting on a different variant entirely.
    let Some(type_info) = target.get_represented_type_info() else {
        log::warn!("cannot apply an authored value: the field has no type of its own");
        return;
    };
    let Some(reflected) = bsn_value_to_reflect(value, type_info.type_id(), registry, assets) else {
        log::warn!(
            "cannot apply an authored value to '{}': it does not read as that type",
            type_info.type_path(),
        );
        return;
    };
    apply_authored_value(target, &*reflected);
}

/// Apply a tuple struct patch: merge over existing component (or default).
/// Apply a tuple patch whose type path names an enum's tuple variant rather
/// than a tuple struct, such as `EntityCursor::System(Pointer)`, which is how
/// the writer spells a data variant.
///
/// Answers whether the patch named such a variant, so the caller reports an
/// unknown type only when it did not. A variant that is named but cannot be
/// built still answers `true`: it is reported here and not again.
fn apply_tuple_variant_patch(
    world: &mut World,
    entity: Entity,
    data: &BsnTupleStructData,
    reg: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> bool {
    let Some(separator) = data.type_path.rfind("::") else {
        return false;
    };
    let Some(registration) = reg.get_with_type_path(&data.type_path[..separator]) else {
        return false;
    };
    let bevy::reflect::TypeInfo::Enum(enum_info) = registration.type_info() else {
        return false;
    };
    let variant_name = variant_name_of(&data.type_path);
    let Some(variant) = enum_info.variant(variant_name) else {
        return false;
    };
    // A variant of the wrong shape belongs to this arm even though it cannot
    // be built here: the conversion below answers a kind mismatch with `None`,
    // and nothing else would say which of the two spellings was wrong.
    if !matches!(variant, bevy::reflect::enums::VariantInfo::Tuple(_)) {
        log::warn!(
            "cannot apply '{}': '{variant_name}' is not a tuple variant of '{}', so it takes no \
             values in parentheses",
            data.type_path,
            enum_info.type_path(),
        );
        return true;
    }
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        log::warn!(
            "cannot apply '{}': not a reflectable component",
            data.type_path
        );
        return true;
    };
    let value = BsnValue::TupleStruct(data.clone());
    let Some(built) = enum_variant_value_to_reflect(&value, enum_info, registration, reg, assets)
    else {
        // The conversion names what it refused for every arm reachable from
        // here: the variant's shape was checked above.
        return true;
    };
    // The dynamic enum carries whatever the document's values converted to,
    // and a conversion that could not read a value hands back one of the wrong
    // type rather than nothing. `ReflectComponent::insert` would take that as
    // far as `apply` and die on the mismatch, so the value is made concrete
    // here first, the same guard the tuple-struct arm below uses.
    let Some(from_reflect) = registration.data::<ReflectFromReflect>() else {
        log::warn!(
            "cannot apply '{}': no FromReflect registered",
            data.type_path
        );
        return true;
    };
    let Some(concrete) = from_reflect.from_reflect(&*built) else {
        log::warn!(
            "cannot apply '{}': its authored values do not fit the variant",
            data.type_path
        );
        return true;
    };
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return true;
    };
    reflect_component.insert(&mut entity_mut, concrete.as_partial_reflect(), reg);
    true
}

fn apply_tuple_struct_patch(world: &mut World, entity: Entity, data: &BsnTupleStructData) {
    let server = world.get_resource::<AssetServer>().cloned();
    let local = world.get_resource::<BsnSceneAssets>().map(|r| r.0.clone());
    let assets_ctx = server.as_ref().map(|s| BsnApplyAssets {
        server: s,
        local: local.as_ref(),
    });
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    let Some(registration) = reg.get_with_type_path(&data.type_path) else {
        // `Enum::Variant(value)` is not itself a registered type. The struct
        // patch has the same fallback for a braced variant; without this one an
        // authored `EntityCursor::System(Pointer)` reads as an unknown type and
        // the component is dropped on load.
        if apply_tuple_variant_patch(world, entity, data, &reg, assets_ctx.as_ref()) {
            return;
        }
        if !note_unapplied(world, &data.type_path) {
            log::warn!(
                "cannot apply '{}': type not in the registry",
                data.type_path
            );
        }
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        log::warn!(
            "cannot apply '{}': not a reflectable component",
            data.type_path
        );
        return;
    };

    let Ok(tuple_info) = registration.type_info().as_tuple_struct() else {
        log::warn!("cannot apply '{}': not a tuple struct", data.type_path);
        return;
    };

    // Start from the existing component value if present, else from the
    // type's default. A tuple struct without a default (like a stable-id
    // newtype) is fully specified by its patch values: build it directly
    // from the converted values and insert.
    let existing: Option<Box<dyn PartialReflect>> = {
        let Ok(entity_ref) = world.get_entity(entity) else {
            return;
        };
        reflect_component
            .reflect(entity_ref)
            .map(bevy::prelude::PartialReflect::to_dynamic)
    };
    let mut value: Box<dyn PartialReflect> = match existing {
        Some(existing) => existing,
        None => match registration.data::<ReflectDefault>() {
            Some(reflect_default) => reflect_default.default().into_partial_reflect(),
            None => {
                let mut dynamic = bevy::reflect::tuple_struct::DynamicTupleStruct::default();
                for (i, bsn_val) in data.values.iter().enumerate() {
                    let Some(field_info) = tuple_info.field_at(i) else {
                        log::warn!(
                            "cannot apply '{}': it has {} values but the type takes {}",
                            data.type_path,
                            data.values.len(),
                            tuple_info.field_len(),
                        );
                        return;
                    };
                    let Some(reflected) = bsn_value_to_reflect(
                        bsn_val,
                        field_info.ty().id(),
                        &reg,
                        assets_ctx.as_ref(),
                    ) else {
                        log::warn!(
                            "cannot apply '{}': value {i} does not read as '{}'",
                            data.type_path,
                            field_info.ty().path(),
                        );
                        return;
                    };
                    dynamic.insert_boxed(reflected);
                }
                if dynamic.field_len() != tuple_info.field_len() {
                    log::warn!(
                        "cannot apply '{}': it has {} values but the type takes {}",
                        data.type_path,
                        dynamic.field_len(),
                        tuple_info.field_len(),
                    );
                    return;
                }
                dynamic.set_represented_type(Some(registration.type_info()));
                // `ReflectComponent::insert` panics when the dynamic value
                // cannot be made concrete, which a document is free to ask
                // for: an opaque field with no conversion reaches here as a
                // mismatched type, and a load must not die on that.
                let Some(from_reflect) = registration.data::<ReflectFromReflect>() else {
                    log::warn!(
                        "cannot apply '{}': no default and no FromReflect registered",
                        data.type_path,
                    );
                    return;
                };
                let Some(concrete) = from_reflect.from_reflect(dynamic.as_partial_reflect()) else {
                    log::warn!(
                        "cannot apply '{}': its authored value does not fit the type",
                        data.type_path,
                    );
                    return;
                };
                reflect_component.insert(
                    &mut world.entity_mut(entity),
                    concrete.as_partial_reflect(),
                    &reg,
                );
                return;
            }
        },
    };

    if let ReflectMut::TupleStruct(ts) = value.reflect_mut() {
        for (i, bsn_val) in data.values.iter().enumerate() {
            let Some(field_info) = tuple_info.field_at(i) else {
                log::warn!(
                    "cannot apply value {i} of '{}': the type takes {}",
                    data.type_path,
                    tuple_info.field_len(),
                );
                continue;
            };
            let Some(reflected) =
                bsn_value_to_reflect(bsn_val, field_info.ty().id(), &reg, assets_ctx.as_ref())
            else {
                log::warn!(
                    "cannot apply value {i} of '{}': it does not read as '{}'",
                    data.type_path,
                    field_info.ty().path(),
                );
                continue;
            };
            if let Some(target) = ts.field_mut(i) {
                apply_authored_value(target, &*reflected);
            }
        }
    }

    reflect_component.insert(
        &mut world.entity_mut(entity),
        value.as_partial_reflect(),
        &reg,
    );
}

/// Apply an authored value onto a target field. Lists and maps replace the
/// target's contents instead of merging: reflection `apply` writes elements
/// by position/key and never removes the rest, which would leave stale
/// entries from the seeded default (or the previous value) behind.
fn apply_authored_value(target: &mut dyn PartialReflect, value: &dyn PartialReflect) {
    // What clearing is about to throw away, held until the write lands. A
    // refused value costs the field nothing, so a cleared list or map is
    // restored when the write is refused.
    let clears = matches!(
        target.reflect_mut(),
        ReflectMut::List(_) | ReflectMut::Map(_)
    );
    let prior = clears.then(|| target.to_dynamic());

    if let ReflectMut::List(list) = target.reflect_mut() {
        while !list.is_empty() {
            list.remove(list.len() - 1);
        }
    } else if let ReflectMut::Map(map) = target.reflect_mut() {
        map.drain();
    }
    // Every field write funnels through here, and plain `apply` panics on a
    // value the document shaped wrong. Refusing the write costs the field;
    // panicking costs the document.
    if let Err(err) = target.try_apply(value) {
        log::warn!("cannot apply a value to '{}': {err}", type_path_of(target));
        if let Some(prior) = prior
            && let Err(err) = target.try_apply(&*prior)
        {
            log::warn!(
                "and could not put back what '{}' was holding: {err}",
                type_path_of(target),
            );
        }
    }
}

/// What a reflected value calls itself, for a message about it.
fn type_path_of(value: &dyn PartialReflect) -> &str {
    value
        .get_represented_type_info()
        .map_or("<dynamic>", bevy::reflect::TypeInfo::type_path)
}

/// Convert a [`BsnValue`] to a boxed reflected value given the expected type.
pub fn bsn_value_to_reflect(
    value: &BsnValue,
    expected: TypeId,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> Option<Box<dyn PartialReflect>> {
    // If the expected type is a Handle<T>, resolve from an asset path string.
    if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(expected) {
        if let BsnValue::String(path) = value
            && !path.is_empty()
            && let Some(assets) = assets
        {
            if path.starts_with('#') || path.starts_with('@') {
                if let Some(local) = assets.local
                    && let Some(handle) = local.get(path)
                {
                    let typed = reflect_handle.typed(handle.clone());
                    return Some(typed.into_partial_reflect());
                }
                log::warn!(
                    "asset reference '{path}' did not resolve in the project catalog or \
                     embedded assets; using the default handle"
                );
            } else {
                let asset_type_id = reflect_handle.asset_type_id();
                let untyped = assets
                    .server
                    .load_builder()
                    .load_erased(asset_type_id, path.to_owned());
                let typed = reflect_handle.typed(untyped);
                return Some(typed.into_partial_reflect());
            }
        }
        // Empty string or no asset server: return the default handle.
        if let Some(registration) = registry.get(expected)
            && let Some(reflect_default) = registration.data::<ReflectDefault>()
        {
            return Some(reflect_default.default().into_partial_reflect());
        }
        return None;
    }

    // If the expected type is `Option<Handle<T>>`, resolve a path string into
    // `Some(handle)` (or `None` for an empty string / missing value).
    if let Some(registration) = registry.get(expected)
        && let bevy::reflect::TypeInfo::Enum(enum_info) = registration.type_info()
        && enum_info.type_path().starts_with("core::option::Option<")
        && let Some(bevy::reflect::enums::VariantInfo::Tuple(some_var)) = enum_info.variant("Some")
        && let Some(inner_field) = some_var.field_at(0)
        && registry
            .get_type_data::<ReflectHandle>(inner_field.type_id())
            .is_some()
    {
        let inner_ty = inner_field.type_id();
        if let BsnValue::String(path) = value
            && !path.is_empty()
            && let Some(inner) = bsn_value_to_reflect(value, inner_ty, registry, assets)
        {
            let mut dynamic_tuple = bevy::reflect::tuple::DynamicTuple::default();
            dynamic_tuple.insert_boxed(inner);
            let mut dynamic_enum = DynamicEnum::new("Some", DynamicVariant::Tuple(dynamic_tuple));
            dynamic_enum.set_represented_type(Some(registration.type_info()));
            return Some(Box::new(dynamic_enum));
        }
        let mut dynamic_enum = DynamicEnum::new("None", DynamicVariant::Unit);
        dynamic_enum.set_represented_type(Some(registration.type_info()));
        return Some(Box::new(dynamic_enum));
    }

    // If the expected type is an enum and the value names one of its variants
    // (a data variant as Struct/TupleStruct, or a unit variant as a bare Type),
    // build the variant value directly. `component_to_bsn_patch` emits a
    // variant's value with a `Enum::Variant` type path, which is not itself a
    // registered type, so the plain struct/tuple-struct paths below would fail
    // to resolve it.
    if let Some(registration) = registry.get(expected)
        && let bevy::reflect::TypeInfo::Enum(enum_info) = registration.type_info()
        && let Some(result) =
            enum_variant_value_to_reflect(value, enum_info, registration, registry, assets)
    {
        return Some(result);
    }

    match value {
        BsnValue::Float(f) => float_to_reflect(*f, expected),
        BsnValue::Int(i) => int_to_reflect(*i, expected),
        BsnValue::Bool(b) => Some(Box::new(*b)),
        BsnValue::String(s) => {
            if expected == TypeId::of::<std::borrow::Cow<'static, str>>() {
                Some(Box::new(std::borrow::Cow::<'static, str>::Owned(s.clone())))
            } else if expected == TypeId::of::<std::path::PathBuf>() {
                Some(Box::new(std::path::PathBuf::from(s.clone())))
            } else if expected == TypeId::of::<smol_str::SmolStr>() {
                Some(Box::new(smol_str::SmolStr::new(s)))
            } else {
                Some(Box::new(s.clone()))
            }
        }
        BsnValue::Type(type_path) => type_value_to_reflect(type_path, expected, registry),
        BsnValue::Struct(data) => struct_value_to_reflect(data, registry, assets),
        BsnValue::TupleStruct(data) => tuple_struct_value_to_reflect(data, registry, assets),
        BsnValue::List(items) => list_value_to_reflect(items, expected, registry, assets),
        BsnValue::Map(entries) => map_value_to_reflect(entries, expected, registry, assets),
    }
}

/// Say why a variant the document named could not be built, and refuse it. The
/// message attributes the fault to the document rather than to the type, which
/// is the other way round from reflection's own wording.
fn refused_variant(
    enum_info: &bevy::reflect::enums::EnumInfo,
    variant: &str,
    reason: &str,
) -> Option<Box<dyn PartialReflect>> {
    log::warn!(
        "cannot read '{}::{variant}': {reason}",
        enum_info.type_path(),
    );
    None
}

/// The trailing `Variant` segment of an `Enum::Variant` type path (or the whole
/// string if it has no `::`).
fn variant_name_of(type_path: &str) -> &str {
    match type_path.rfind("::") {
        Some(sep) => &type_path[sep + 2..],
        None => type_path,
    }
}

/// Build a value of the expected enum type from a `BsnValue` that names one of
/// its variants. Returns `None` when the value does not name a variant of this
/// enum (so the caller can fall through to other conversions).
fn enum_variant_value_to_reflect(
    value: &BsnValue,
    enum_info: &bevy::reflect::enums::EnumInfo,
    enum_registration: &bevy::reflect::TypeRegistration,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> Option<Box<dyn PartialReflect>> {
    use bevy::reflect::enums::VariantInfo;
    // `field_len` on the dynamic forms is the trait's, not inherent.
    use bevy::reflect::tuple::Tuple;

    let (variant_name, dynamic_variant) = match value {
        BsnValue::Type(type_path) => {
            let name = variant_name_of(type_path);
            match enum_info.variant(name)? {
                VariantInfo::Unit(_) => (name.to_string(), DynamicVariant::Unit),
                _ => return None,
            }
        }
        BsnValue::TupleStruct(data) => {
            let name = variant_name_of(&data.type_path);
            let VariantInfo::Tuple(tuple_var) = enum_info.variant(name)? else {
                return None;
            };
            let mut dynamic_tuple = bevy::reflect::tuple::DynamicTuple::default();
            for (i, item) in data.values.iter().enumerate() {
                let Some(field) = tuple_var.field_at(i) else {
                    return refused_variant(enum_info, name, "it has more values than the variant");
                };
                let Some(reflected) = bsn_value_to_reflect(item, field.type_id(), registry, assets)
                else {
                    return refused_variant(
                        enum_info,
                        name,
                        &format!("value {i} does not read as '{}'", field.type_path()),
                    );
                };
                dynamic_tuple.insert_boxed(reflected);
            }
            // A variant is written whole or not at all: reflection reads a gap
            // as "the variant has no such field", blaming the type for what
            // the document left out.
            if dynamic_tuple.field_len() != tuple_var.field_len() {
                return refused_variant(
                    enum_info,
                    name,
                    &format!(
                        "it gives {} of the variant's {} values",
                        dynamic_tuple.field_len(),
                        tuple_var.field_len(),
                    ),
                );
            }
            (name.to_string(), DynamicVariant::Tuple(dynamic_tuple))
        }
        BsnValue::Struct(data) => {
            let name = variant_name_of(&data.type_path);
            let VariantInfo::Struct(struct_var) = enum_info.variant(name)? else {
                return None;
            };
            let mut dynamic_struct = bevy::reflect::structs::DynamicStruct::default();
            for field in &data.fields.0 {
                let Some(field_info) = struct_var.field(&field.name) else {
                    return refused_variant(
                        enum_info,
                        name,
                        &format!("the variant has no field '{}'", field.name),
                    );
                };
                let Some(reflected) =
                    bsn_value_to_reflect(&field.value, field_info.type_id(), registry, assets)
                else {
                    return refused_variant(
                        enum_info,
                        name,
                        &format!(
                            "field '{}' does not read as '{}'",
                            field.name,
                            field_info.type_path(),
                        ),
                    );
                };
                dynamic_struct.insert_boxed(&field.name, reflected);
            }
            if dynamic_struct.field_len() != struct_var.field_len() {
                return refused_variant(
                    enum_info,
                    name,
                    &format!(
                        "it gives {} of the variant's {} fields",
                        dynamic_struct.field_len(),
                        struct_var.field_len(),
                    ),
                );
            }
            (name.to_string(), DynamicVariant::Struct(dynamic_struct))
        }
        _ => return None,
    };

    let mut dynamic_enum = DynamicEnum::new(variant_name, dynamic_variant);
    dynamic_enum.set_represented_type(Some(enum_registration.type_info()));
    Some(Box::new(dynamic_enum))
}

fn float_to_reflect(f: f64, expected: TypeId) -> Option<Box<dyn PartialReflect>> {
    if expected == TypeId::of::<f32>() {
        Some(Box::new(f as f32))
    } else if expected == TypeId::of::<f64>() {
        Some(Box::new(f))
    } else {
        None
    }
}

fn int_to_reflect(i: i128, expected: TypeId) -> Option<Box<dyn PartialReflect>> {
    if expected == TypeId::of::<i8>() {
        Some(Box::new(i as i8))
    } else if expected == TypeId::of::<u8>() {
        Some(Box::new(i as u8))
    } else if expected == TypeId::of::<i16>() {
        Some(Box::new(i as i16))
    } else if expected == TypeId::of::<u16>() {
        Some(Box::new(i as u16))
    } else if expected == TypeId::of::<i32>() {
        Some(Box::new(i as i32))
    } else if expected == TypeId::of::<u32>() {
        Some(Box::new(i as u32))
    } else if expected == TypeId::of::<i64>() {
        Some(Box::new(i as i64))
    } else if expected == TypeId::of::<u64>() {
        Some(Box::new(i as u64))
    } else if expected == TypeId::of::<isize>() {
        Some(Box::new(i as isize))
    } else if expected == TypeId::of::<usize>() {
        Some(Box::new(i as usize))
    } else if expected == TypeId::of::<f32>() {
        Some(Box::new(i as f32))
    } else if expected == TypeId::of::<f64>() {
        Some(Box::new(i as f64))
    // `NonZero` refuses rather than wraps, unlike the casting arms above. A
    // wrapped value would disagree with the reflect side, which refuses it;
    // converting to nothing leaves the field as it was.
    } else if expected == TypeId::of::<std::num::NonZeroI16>() {
        nonzero_to_reflect(i16::try_from(i).ok().and_then(std::num::NonZeroI16::new))
    } else if expected == TypeId::of::<std::num::NonZeroU16>() {
        nonzero_to_reflect(u16::try_from(i).ok().and_then(std::num::NonZeroU16::new))
    } else if expected == TypeId::of::<std::num::NonZeroI32>() {
        nonzero_to_reflect(i32::try_from(i).ok().and_then(std::num::NonZeroI32::new))
    } else if expected == TypeId::of::<std::num::NonZeroU32>() {
        nonzero_to_reflect(u32::try_from(i).ok().and_then(std::num::NonZeroU32::new))
    } else {
        None
    }
}

fn nonzero_to_reflect<T: PartialReflect>(value: Option<T>) -> Option<Box<dyn PartialReflect>> {
    value.map(|v| Box::new(v) as Box<dyn PartialReflect>)
}

fn type_value_to_reflect(
    type_path: &str,
    expected: TypeId,
    registry: &TypeRegistry,
) -> Option<Box<dyn PartialReflect>> {
    // Try as a direct type (unit struct).
    if let Some(registration) = registry.get_with_type_path(type_path) {
        let reflect_default = registration.data::<ReflectDefault>()?;
        return Some(reflect_default.default().into_partial_reflect());
    }

    // Try as an enum variant.
    let last_sep = type_path.rfind("::")?;
    let enum_path = &type_path[..last_sep];
    let variant_name = &type_path[last_sep + 2..];

    let registration = registry
        .get(expected)
        .or_else(|| registry.get_with_type_path(enum_path))?;
    let reflect_default = registration.data::<ReflectDefault>()?;
    let mut value = reflect_default.default();
    if let ReflectMut::Enum(e) = value.reflect_mut() {
        let dynamic_enum = DynamicEnum::new(variant_name, DynamicVariant::Unit);
        if !apply_enum_variant(e, &dynamic_enum, type_path) {
            return None;
        }
    }
    Some(value.into_partial_reflect())
}

fn struct_value_to_reflect(
    data: &BsnStructData,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get_with_type_path(&data.type_path)?;
    let struct_info = registration.type_info().as_struct().ok()?;

    // Without a default to seed, build a dynamic struct from the emitted
    // fields; the receiving container converts it via `FromReflect` (ignored
    // fields fall back to their own defaults).
    let Some(reflect_default) = registration.data::<ReflectDefault>() else {
        let mut dynamic = bevy::reflect::structs::DynamicStruct::default();
        for field in &data.fields.0 {
            let field_info = struct_info.field(&field.name)?;
            let reflected =
                bsn_value_to_reflect(&field.value, field_info.ty().id(), registry, assets)?;
            dynamic.insert_boxed(&field.name, reflected);
        }
        dynamic.set_represented_type(Some(registration.type_info()));
        return Some(Box::new(dynamic));
    };

    let mut value = reflect_default.default();
    if let ReflectMut::Struct(s) = value.reflect_mut() {
        for field in &data.fields.0 {
            if let Some(field_info) = struct_info.field(&field.name)
                && let Some(reflected) =
                    bsn_value_to_reflect(&field.value, field_info.ty().id(), registry, assets)
                && let Some(target) = s.field_mut(&field.name)
            {
                apply_authored_value(target, &*reflected);
            }
        }
    }
    Some(value.into_partial_reflect())
}

fn tuple_struct_value_to_reflect(
    data: &BsnTupleStructData,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get_with_type_path(&data.type_path)?;
    let tuple_info = registration.type_info().as_tuple_struct().ok()?;

    let Some(reflect_default) = registration.data::<ReflectDefault>() else {
        let mut dynamic = bevy::reflect::tuple_struct::DynamicTupleStruct::default();
        for (i, bsn_val) in data.values.iter().enumerate() {
            let field_info = tuple_info.field_at(i)?;
            let reflected = bsn_value_to_reflect(bsn_val, field_info.ty().id(), registry, assets)?;
            dynamic.insert_boxed(reflected);
        }
        dynamic.set_represented_type(Some(registration.type_info()));
        return Some(Box::new(dynamic));
    };

    let mut value = reflect_default.default();
    if let ReflectMut::TupleStruct(ts) = value.reflect_mut() {
        for (i, bsn_val) in data.values.iter().enumerate() {
            if let Some(field_info) = tuple_info.field_at(i)
                && let Some(reflected) =
                    bsn_value_to_reflect(bsn_val, field_info.ty().id(), registry, assets)
                && let Some(target) = ts.field_mut(i)
            {
                apply_authored_value(target, &*reflected);
            }
        }
    }
    Some(value.into_partial_reflect())
}

fn list_value_to_reflect(
    items: &[BsnValue],
    expected: TypeId,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get(expected)?;

    // Tuples take the same `[a, b]` literal too. BSN has no tuple syntax of
    // its own, so without this a `Vec<(String, T)>` field is authored as its
    // `Debug` text and dropped on the way back in.
    if let Ok(tuple_info) = registration.type_info().as_tuple() {
        let mut dynamic = bevy::reflect::tuple::DynamicTuple::default();
        for (index, item) in items.iter().enumerate() {
            let field_type_id = tuple_info.field_at(index)?.type_id();
            dynamic.insert_boxed(bsn_value_to_reflect(item, field_type_id, registry, assets)?);
        }
        dynamic.set_represented_type(Some(registration.type_info()));
        return Some(Box::new(dynamic));
    }

    // Fixed-size arrays take the same `[a, b]` literal as lists.
    if let Ok(array_info) = registration.type_info().as_array() {
        let item_type_id = array_info.item_ty().id();
        let mut converted = Vec::new();
        for item in items {
            converted.push(bsn_value_to_reflect(item, item_type_id, registry, assets)?);
        }
        let mut dynamic = bevy::reflect::array::DynamicArray::new(converted.into_boxed_slice());
        dynamic.set_represented_type(Some(registration.type_info()));
        return Some(Box::new(dynamic));
    }

    let list_info = registration.type_info().as_list().ok()?;
    let item_type_id = list_info.item_ty().id();

    // Each element is made concrete here rather than left dynamic. Applying a
    // dynamic list onto a real `Vec<T>` pushes through `T::from_reflect`, and
    // bevy's `Vec` impl panics when that conversion fails, so an element the
    // document spells wrong (a field missing from an enum variant, a value of
    // the wrong shape) would take the editor down on load. Converting here
    // turns that into a warning and one dropped element. An item type with no
    // `FromReflect` registration keeps the dynamic value.
    let from_reflect = registry.get_type_data::<ReflectFromReflect>(item_type_id);
    let mut dynamic_list = DynamicList::default();
    let list_path = registration.type_info().type_path();
    let item_path = list_info.item_ty().path();
    for (index, item) in items.iter().enumerate() {
        // Both drops are reported the same way: which list, which position,
        // which element type. Dropping in silence leaves the user with a
        // shorter list and nothing naming what went missing.
        let Some(reflected) = bsn_value_to_reflect(item, item_type_id, registry, assets) else {
            log::warn!(
                "{list_path}[{index}]: the document's value is not a {item_path}; dropping it"
            );
            continue;
        };
        match from_reflect {
            Some(from_reflect) => match from_reflect.from_reflect(reflected.as_ref()) {
                Some(concrete) => dynamic_list.push_box(concrete.into_partial_reflect()),
                None => log::warn!("{list_path}[{index}]: does not fit {item_path}; dropping it"),
            },
            None => dynamic_list.push_box(reflected),
        }
    }
    dynamic_list.set_represented_type(Some(registration.type_info()));
    Some(Box::new(dynamic_list))
}

/// Build a [`DynamicMap`] for a `map[(k, v), ...]` value against the expected
/// concrete map type. The dynamic map carries the target type info, so applying
/// it onto a concrete `HashMap<K, V>` field converts each key/value through that
/// map's `FromReflect`-backed `insert_boxed`.
fn map_value_to_reflect(
    entries: &[(BsnValue, BsnValue)],
    expected: TypeId,
    registry: &TypeRegistry,
    assets: Option<&BsnApplyAssets>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get(expected)?;
    let map_info = registration.type_info().as_map().ok()?;
    let key_type_id = map_info.key_ty().id();
    let value_type_id = map_info.value_ty().id();

    let mut dynamic_map = DynamicMap::default();
    for (key, value) in entries {
        if let Some(reflected_key) = bsn_value_to_reflect(key, key_type_id, registry, assets)
            && let Some(reflected_value) =
                bsn_value_to_reflect(value, value_type_id, registry, assets)
        {
            dynamic_map.insert_boxed(reflected_key, reflected_value);
        }
    }
    dynamic_map.set_represented_type(Some(registration.type_info()));
    Some(Box::new(dynamic_map))
}

/// Set a field value at a dotted path within an entity's AST patches.
///
/// Mirrors the JSON-path layer's `set_field_in_component_json`:
/// - an unregistered `type_path` is a no-op (no patch is created),
/// - an empty `field_path` replaces the whole component value,
/// - dotted segments navigate named struct fields, numeric tuple-struct
///   indices, `field[i]` list elements, and `field[key]` map entries.
///
/// Struct patches and intermediate struct fields are created on demand so a
/// value can be written into an as-yet-empty patch.
pub fn set_bsn_field(
    ast: &mut SceneBsnAst,
    patches_entity: Entity,
    type_path: &str,
    field_path: &str,
    value: BsnValue,
    registry: &TypeRegistry,
) {
    // Refuse to write a field for a type the registry does not know about,
    // matching the JSON-path layer (which resolves the type before writing).
    if registry.get_with_type_path(type_path).is_none() {
        return;
    }

    // Ensure a patch exists for this type.
    let patch_entity = match ast.find_patch_by_type_path(patches_entity, type_path) {
        Some(pe) => pe,
        None => {
            let pe = ast
                .world
                .spawn(empty_component_patch_for_type(type_path, registry))
                .id();
            if let Some(patches) = ast.get_patches_mut(patches_entity) {
                patches.0.push(pe);
            }
            pe
        }
    };

    let Some(patch) = ast.world.get_mut::<BsnPatch>(patch_entity) else {
        return;
    };
    let patch = patch.into_inner();

    // Empty path: replace the whole component value.
    if field_path.is_empty() {
        if let Some(replacement) = value_to_patch(value) {
            *patch = replacement;
        }
        return;
    }

    // If the patch is a bare Type (all defaults), promote to Struct/TupleStruct,
    // preserving the original type path (which may be variant-qualified).
    if let BsnPatch::Type(existing_tp) = patch {
        let preserved_tp = existing_tp.clone();
        *patch = empty_component_patch_for_type(&preserved_tp, registry);
    }

    // View the patch as a navigable value, descend the path, and store it back.
    let (mut root, is_tuple) = match std::mem::replace(patch, BsnPatch::Type(String::new())) {
        BsnPatch::Struct(data) => (BsnValue::Struct(data), false),
        BsnPatch::TupleStruct(data) => (BsnValue::TupleStruct(data), true),
        other => {
            *patch = other;
            return;
        }
    };
    let root_type_path = match &root {
        BsnValue::Struct(d) => d.type_path.clone(),
        BsnValue::TupleStruct(d) => d.type_path.clone(),
        _ => String::new(),
    };

    let segments: Vec<&str> = field_path.split('.').filter(|s| !s.is_empty()).collect();
    set_nested_value(&mut root, &segments, value, &root_type_path, registry);

    *patch = match root {
        BsnValue::Struct(data) => BsnPatch::Struct(data),
        BsnValue::TupleStruct(data) => BsnPatch::TupleStruct(data),
        // The value view was created from a Struct or TupleStruct and the
        // navigator never changes the root's kind, so this is unreachable in
        // practice; fall back to preserve the original tuple/struct shape.
        other => {
            if is_tuple {
                BsnPatch::TupleStruct(BsnTupleStructData {
                    type_path: root_type_path,
                    values: vec![other],
                })
            } else {
                BsnPatch::Struct(BsnStructData {
                    type_path: root_type_path,
                    fields: BsnStructFields::default(),
                })
            }
        }
    };
}

/// Get a field value at a dotted path within an entity's AST patches.
///
/// Mirrors `get_field_in_component_json`: an empty `field_path` returns the
/// whole component value; dotted segments navigate named struct fields,
/// numeric tuple-struct indices, `field[i]` list elements, and `field[key]`
/// map entries.
pub fn get_bsn_field(
    ast: &SceneBsnAst,
    patches_entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<BsnValue> {
    let patch_entity = ast.find_patch_by_type_path(patches_entity, type_path)?;
    let patch = ast.get_patch(patch_entity)?;
    let root = patch_to_value(patch)?;

    let segments: Vec<&str> = field_path.split('.').filter(|s| !s.is_empty()).collect();
    get_nested_value(&root, &segments).cloned()
}

/// The whole-component [`BsnValue`] view of a patch, or `None` for patches that
/// carry no addressable value (Name, Base, Template, Children).
fn patch_to_value(patch: &BsnPatch) -> Option<BsnValue> {
    match patch {
        BsnPatch::Struct(data) => Some(BsnValue::Struct(data.clone())),
        BsnPatch::TupleStruct(data) => Some(BsnValue::TupleStruct(data.clone())),
        BsnPatch::Type(tp) => Some(BsnValue::Type(tp.clone())),
        _ => None,
    }
}

/// Convert a whole-component [`BsnValue`] back into the patch that stores it.
/// Used by the empty-path set case.
fn value_to_patch(value: BsnValue) -> Option<BsnPatch> {
    match value {
        BsnValue::Struct(data) => Some(BsnPatch::Struct(data)),
        BsnValue::TupleStruct(data) => Some(BsnPatch::TupleStruct(data)),
        BsnValue::Type(tp) => Some(BsnPatch::Type(tp)),
        _ => None,
    }
}

/// Whether a map key `BsnValue` equals the textual key `segment` from a path.
fn key_matches(key: &BsnValue, segment: &str) -> bool {
    match key {
        BsnValue::String(s) => s == segment,
        BsnValue::Type(t) => t == segment,
        BsnValue::Int(i) => segment.parse::<i128>().ok() == Some(*i),
        BsnValue::Bool(b) => segment.parse::<bool>().ok() == Some(*b),
        _ => false,
    }
}

/// Read a nested value by path segments, navigating structs (named fields),
/// tuple structs / lists (numeric or `[i]` indices), and maps (`[key]`).
fn get_nested_value<'a>(root: &'a BsnValue, segments: &[&str]) -> Option<&'a BsnValue> {
    let mut current = root;
    for segment in segments {
        current = navigate_value(current, segment)?;
    }
    Some(current)
}

/// Navigate one path segment into a value. A segment may carry a trailing
/// bracket index (`name[i]`, `name[key]`, or `[i]`/`[key]` with an empty name).
fn navigate_value<'a>(value: &'a BsnValue, segment: &str) -> Option<&'a BsnValue> {
    if let Some(bracket_pos) = segment.find('[') {
        if !segment.ends_with(']') {
            return None;
        }
        let key = &segment[..bracket_pos];
        let inner = &segment[bracket_pos + 1..segment.len() - 1];
        let base = if key.is_empty() {
            value
        } else {
            navigate_named(value, key)?
        };
        return index_into_value(base, inner);
    }
    navigate_named(value, segment)
}

/// Navigate a named struct field or a numeric tuple-struct/list index.
fn navigate_named<'a>(value: &'a BsnValue, name: &str) -> Option<&'a BsnValue> {
    match value {
        BsnValue::Struct(data) => data
            .fields
            .0
            .iter()
            .find(|f| f.name == name)
            .map(|f| &f.value),
        BsnValue::TupleStruct(data) => name.parse::<usize>().ok().and_then(|i| data.values.get(i)),
        BsnValue::List(items) => name.parse::<usize>().ok().and_then(|i| items.get(i)),
        BsnValue::Map(entries) => entries
            .iter()
            .find(|(k, _)| key_matches(k, name))
            .map(|(_, v)| v),
        _ => None,
    }
}

/// Index into a list (by number), tuple struct (by number), or map (by key).
fn index_into_value<'a>(value: &'a BsnValue, inner: &str) -> Option<&'a BsnValue> {
    match value {
        BsnValue::List(items) => inner.parse::<usize>().ok().and_then(|i| items.get(i)),
        BsnValue::TupleStruct(data) => inner.parse::<usize>().ok().and_then(|i| data.values.get(i)),
        BsnValue::Map(entries) => entries
            .iter()
            .find(|(k, _)| key_matches(k, inner))
            .map(|(_, v)| v),
        _ => None,
    }
}

/// Remove one named field from a component's sparse patch: the inverse of a
/// [`set_bsn_field`] that authored a previously-absent field. Only dotted
/// struct paths are supported; the leaf field is dropped from its enclosing
/// struct value, and intermediate structs emptied by the removal stay (they
/// are harmless in a sparse patch). A missing patch, path, or field is a
/// no-op.
pub fn remove_bsn_field(
    ast: &mut SceneBsnAst,
    patches_entity: Entity,
    type_path: &str,
    field_path: &str,
) {
    if field_path.is_empty() {
        ast.remove_component_patch(patches_entity, type_path);
        return;
    }
    let Some(patch_entity) = ast.find_patch_by_type_path(patches_entity, type_path) else {
        return;
    };
    let Some(patch) = ast.world.get_mut::<BsnPatch>(patch_entity) else {
        return;
    };
    let patch = patch.into_inner();
    let BsnPatch::Struct(data) = patch else {
        return;
    };

    let segments: Vec<&str> = field_path.split('.').filter(|s| !s.is_empty()).collect();
    let Some((leaf, parents)) = segments.split_last() else {
        return;
    };

    // Walk to the struct that holds the leaf field.
    let mut fields = &mut data.fields;
    for segment in parents {
        let Some(next) = fields.0.iter_mut().find(|f| f.name == *segment) else {
            return;
        };
        let BsnValue::Struct(inner) = &mut next.value else {
            return;
        };
        fields = &mut inner.fields;
    }
    fields.0.retain(|f| f.name != *leaf);
}

fn set_nested_value(
    current: &mut BsnValue,
    segments: &[&str],
    value: BsnValue,
    current_type_path: &str,
    registry: &TypeRegistry,
) {
    let Some((segment, rest)) = segments.split_first() else {
        *current = value;
        return;
    };

    // Bracket navigation: `name[inner]` or `[inner]`.
    if let Some(bracket_pos) = segment.find('[') {
        if !segment.ends_with(']') {
            return;
        }
        let key = &segment[..bracket_pos];
        let inner = &segment[bracket_pos + 1..segment.len() - 1];
        let base: Option<&mut BsnValue> = if key.is_empty() {
            Some(current)
        } else {
            navigate_named_mut(current, key)
        };
        let Some(base) = base else { return };
        if let Some(target) = index_into_value_mut(base, inner) {
            set_nested_value(target, rest, value, "", registry);
        }
        return;
    }

    match current {
        BsnValue::Struct(data) => {
            if rest.is_empty() {
                if let Some(field) = data.fields.0.iter_mut().find(|f| f.name == *segment) {
                    field.value = value;
                } else {
                    data.fields.0.push(BsnField {
                        name: segment.to_string(),
                        value,
                    });
                }
                return;
            }
            let nested_type_path =
                get_field_type_path(current_type_path, segment, registry).unwrap_or_default();
            let pos = match data.fields.0.iter().position(|f| f.name == *segment) {
                Some(p) => p,
                None => {
                    data.fields.0.push(BsnField {
                        name: segment.to_string(),
                        value: empty_container_value_for_type(&nested_type_path, registry),
                    });
                    data.fields.0.len() - 1
                }
            };
            let field = &mut data.fields.0[pos];
            if !matches!(
                field.value,
                BsnValue::Struct(_)
                    | BsnValue::TupleStruct(_)
                    | BsnValue::List(_)
                    | BsnValue::Map(_)
            ) {
                field.value = empty_container_value_for_type(&nested_type_path, registry);
            }
            set_nested_value(&mut field.value, rest, value, &nested_type_path, registry);
        }
        BsnValue::TupleStruct(data) => {
            let Ok(index) = segment.parse::<usize>() else {
                return;
            };
            while data.values.len() <= index {
                let field_index = data.values.len();
                let nested_type_path =
                    get_tuple_field_type_path(current_type_path, field_index, registry)
                        .unwrap_or_default();
                data.values
                    .push(empty_container_value_for_type(&nested_type_path, registry));
            }
            let nested_type_path =
                get_tuple_field_type_path(current_type_path, index, registry).unwrap_or_default();
            if rest.is_empty() {
                data.values[index] = value;
                return;
            }
            if !matches!(
                data.values[index],
                BsnValue::Struct(_)
                    | BsnValue::TupleStruct(_)
                    | BsnValue::List(_)
                    | BsnValue::Map(_)
            ) {
                data.values[index] = empty_container_value_for_type(&nested_type_path, registry);
            }
            set_nested_value(
                &mut data.values[index],
                rest,
                value,
                &nested_type_path,
                registry,
            );
        }
        BsnValue::List(items) => {
            if let Some(target) = segment.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
                set_nested_value(target, rest, value, "", registry);
            }
        }
        BsnValue::Map(entries) => {
            if let Some((_, v)) = entries.iter_mut().find(|(k, _)| key_matches(k, segment)) {
                set_nested_value(v, rest, value, "", registry);
            }
        }
        _ => {}
    }
}

/// Mutable analog of [`navigate_named`].
fn navigate_named_mut<'a>(value: &'a mut BsnValue, name: &str) -> Option<&'a mut BsnValue> {
    match value {
        BsnValue::Struct(data) => data
            .fields
            .0
            .iter_mut()
            .find(|f| f.name == name)
            .map(|f| &mut f.value),
        BsnValue::TupleStruct(data) => name
            .parse::<usize>()
            .ok()
            .and_then(move |i| data.values.get_mut(i)),
        BsnValue::List(items) => name
            .parse::<usize>()
            .ok()
            .and_then(move |i| items.get_mut(i)),
        BsnValue::Map(entries) => entries
            .iter_mut()
            .find(|(k, _)| key_matches(k, name))
            .map(|(_, v)| v),
        _ => None,
    }
}

/// Mutable analog of [`index_into_value`].
fn index_into_value_mut<'a>(value: &'a mut BsnValue, inner: &str) -> Option<&'a mut BsnValue> {
    match value {
        BsnValue::List(items) => inner
            .parse::<usize>()
            .ok()
            .and_then(move |i| items.get_mut(i)),
        BsnValue::TupleStruct(data) => inner
            .parse::<usize>()
            .ok()
            .and_then(move |i| data.values.get_mut(i)),
        BsnValue::Map(entries) => entries
            .iter_mut()
            .find(|(k, _)| key_matches(k, inner))
            .map(|(_, v)| v),
        _ => None,
    }
}

fn get_field_type_path(
    parent_type_path: &str,
    field_name: &str,
    registry: &TypeRegistry,
) -> Option<String> {
    let registration = registry.get_with_type_path(parent_type_path)?;
    let struct_info = registration.type_info().as_struct().ok()?;
    let field_info = struct_info.field(field_name)?;
    let field_reg = registry.get(field_info.ty().id())?;
    Some(field_reg.type_info().type_path().to_string())
}

fn get_tuple_field_type_path(
    parent_type_path: &str,
    index: usize,
    registry: &TypeRegistry,
) -> Option<String> {
    let registration = registry.get_with_type_path(parent_type_path)?;
    let tuple_info = registration.type_info().as_tuple_struct().ok()?;
    let field_info = tuple_info.field_at(index)?;
    let field_reg = registry.get(field_info.ty().id())?;
    Some(field_reg.type_info().type_path().to_string())
}

/// Empty component patch shaped like the reflected type kind.
fn empty_component_patch_for_type(type_path: &str, registry: &TypeRegistry) -> BsnPatch {
    match registry
        .get_with_type_path(type_path)
        .map(bevy::reflect::TypeRegistration::type_info)
    {
        Some(bevy::reflect::TypeInfo::TupleStruct(_)) => {
            BsnPatch::TupleStruct(BsnTupleStructData {
                type_path: type_path.to_string(),
                values: Vec::new(),
            })
        }
        _ => BsnPatch::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields::default(),
        }),
    }
}

/// Empty nested value for a type, used when minting intermediate path segments.
fn empty_container_value_for_type(type_path: &str, registry: &TypeRegistry) -> BsnValue {
    match registry
        .get_with_type_path(type_path)
        .map(bevy::reflect::TypeRegistration::type_info)
    {
        Some(bevy::reflect::TypeInfo::TupleStruct(_)) => {
            BsnValue::TupleStruct(BsnTupleStructData {
                type_path: type_path.to_string(),
                values: Vec::new(),
            })
        }
        Some(bevy::reflect::TypeInfo::List(_)) => BsnValue::List(Vec::new()),
        Some(bevy::reflect::TypeInfo::Map(_)) => BsnValue::Map(Vec::new()),
        _ => BsnValue::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields::default(),
        }),
    }
}

#[cfg(test)]
mod document_only_tests {
    use super::*;

    fn set(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    /// `structs` are project components of any non-enum shape; `enums` are the
    /// ones whose variants a document may name.
    fn world_knowing(structs: &[&str], enums: &[&str]) -> World {
        let mut world = World::new();
        let mut all = set(structs);
        all.extend(set(enums));
        world.insert_resource(DocumentOnlyTypes::new(all, set(enums)));
        world.init_resource::<UnresolvedTypes>();
        world
    }

    fn unresolved(world: &World) -> Vec<String> {
        world
            .resource::<UnresolvedTypes>()
            .types()
            .iter()
            .cloned()
            .collect()
    }

    #[test]
    fn a_reported_project_component_is_quiet_and_unrecorded() {
        let mut world = world_knowing(&["mygame::Health"], &[]);
        assert!(note_unapplied(&mut world, "mygame::Health"));
        assert!(world.resource::<UnresolvedTypes>().is_empty());
    }

    #[test]
    fn a_variant_of_a_reported_project_enum_answers_through_its_owner() {
        let mut world = world_knowing(&[], &["mygame::Team"]);
        assert!(note_unapplied(&mut world, "mygame::Team::Red"));
        assert!(world.resource::<UnresolvedTypes>().is_empty());
    }

    /// A struct has no variants, so a name one segment longer than a reported
    /// struct is a different type, not a shape of it.
    #[test]
    fn a_name_below_a_reported_struct_is_not_a_variant_of_it() {
        let mut world = world_knowing(&["mygame::Health"], &[]);
        assert!(!note_unapplied(&mut world, "mygame::Health::Bogus"));
        assert_eq!(unresolved(&world), ["mygame::Health::Bogus"]);
    }

    /// The owner check does not extend to a module path: only a type the
    /// schema reported silences the warning.
    #[test]
    fn an_unreported_type_in_a_reported_module_is_still_recorded() {
        let mut world = world_knowing(&[], &["mygame::Team"]);
        assert!(!note_unapplied(&mut world, "mygame::Ghost"));
        assert_eq!(unresolved(&world), ["mygame::Ghost"]);
    }

    #[test]
    fn a_world_without_the_resources_records_nothing_and_stays_loud() {
        let mut world = World::new();
        assert!(!note_unapplied(&mut world, "mygame::Health"));
    }

    #[test]
    fn the_remedy_names_what_is_missing_and_the_rebuild() {
        let mut types = UnresolvedTypes::default();
        types.types.insert("mygame::Ghost".to_string());
        let remedy = types.take_remedy().expect("a missing type earns a remedy");
        assert!(remedy.contains("mygame::Ghost"), "{remedy}");
        assert!(remedy.contains("rebuild the project"), "{remedy}");
        assert!(
            remedy.starts_with("1 type in this scene is in neither"),
            "one missing type reads singular: {remedy}",
        );
    }

    #[test]
    fn two_missing_types_read_plural() {
        let mut types = UnresolvedTypes::default();
        types.types.insert("mygame::Ghost".to_string());
        types.types.insert("mygame::Wraith".to_string());
        let remedy = types.take_remedy().expect("a remedy");
        assert!(
            remedy.starts_with("2 types in this scene are in neither"),
            "{remedy}",
        );
    }

    /// A scene reloads on every undo restore and prefab reload, and the remedy
    /// stays quiet across those reloads while it is unchanged.
    #[test]
    fn an_unchanged_remedy_is_not_repeated() {
        let mut types = UnresolvedTypes::default();
        types.types.insert("mygame::Ghost".to_string());
        assert!(types.take_remedy().is_some());

        types.start_scene();
        types.types.insert("mygame::Ghost".to_string());
        assert!(
            types.take_remedy().is_none(),
            "the same gap must not re-announce itself",
        );
    }

    /// Opening a second scene does not inherit the first one's findings.
    #[test]
    fn a_following_scene_reports_only_its_own_gaps() {
        let mut types = UnresolvedTypes::default();
        types.types.insert("mygame::Ghost".to_string());
        assert!(types.take_remedy().is_some());

        types.start_scene();
        assert!(types.is_empty(), "scene B starts with nothing recorded");
        types.types.insert("mygame::Wraith".to_string());
        let remedy = types.take_remedy().expect("scene B has its own gap");
        assert!(remedy.contains("mygame::Wraith"), "{remedy}");
        assert!(!remedy.contains("Ghost"), "scene A's gap leaked: {remedy}");
    }

    /// A clean scene after a dirty one has nothing to report.
    #[test]
    fn a_clean_scene_says_nothing() {
        let mut types = UnresolvedTypes::default();
        types.types.insert("mygame::Ghost".to_string());
        assert!(types.take_remedy().is_some());

        types.start_scene();
        assert!(types.take_remedy().is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BsnPatches, BsnStructFields};
    use bevy::reflect::Reflect;

    // A registered struct so `set_bsn_field`'s registry check passes.
    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Default)]
    struct Marker {
        x: f32,
        value: bool,
    }

    fn marker_registry() -> (TypeRegistry, String) {
        let mut registry = TypeRegistry::new();
        registry.register::<Marker>();
        registry.register::<f32>();
        registry.register::<bool>();
        let type_path = registry
            .get(std::any::TypeId::of::<Marker>())
            .unwrap()
            .type_info()
            .type_path()
            .to_string();
        (registry, type_path)
    }

    /// Bevy stores grid lines as `NonZero`, and a value the type cannot hold
    /// leaves the field alone rather than landing on a neighbouring number.
    #[test]
    fn a_nonzero_field_takes_an_int_it_can_hold() {
        use std::num::{NonZeroI16, NonZeroU16};

        let value =
            int_to_reflect(3, std::any::TypeId::of::<NonZeroI16>()).expect("three is a grid line");
        assert_eq!(
            value.try_downcast_ref::<NonZeroI16>().map(|v| v.get()),
            Some(3),
        );

        let span = int_to_reflect(2, std::any::TypeId::of::<NonZeroU16>()).expect("a span of two");
        assert_eq!(
            span.try_downcast_ref::<NonZeroU16>().map(|v| v.get()),
            Some(2),
        );
    }

    #[test]
    fn a_nonzero_field_refuses_zero_and_out_of_range() {
        use std::num::{NonZeroI16, NonZeroU16};

        assert!(
            int_to_reflect(0, std::any::TypeId::of::<NonZeroI16>()).is_none(),
            "zero is the one number this type cannot hold",
        );
        assert!(
            int_to_reflect(70_000, std::any::TypeId::of::<NonZeroI16>()).is_none(),
            "and a number past the width refuses rather than wrapping to 4464",
        );
        assert!(
            int_to_reflect(-1, std::any::TypeId::of::<NonZeroU16>()).is_none(),
            "an unsigned line has no negative side to wrap onto",
        );
    }

    /// A `NonZero` field reads back as a plain number rather than reaching the
    /// `Debug` fallback, whose quoted string the reader above refuses.
    #[test]
    fn a_nonzero_value_round_trips_through_the_document() {
        use std::num::NonZeroI16;

        use std::num::{NonZeroI32, NonZeroU16, NonZeroU32};

        let registry = TypeRegistry::new();
        let line = NonZeroI16::new(-3).expect("a negative grid line");
        let value = BsnValue::from_reflect(&line, &registry);
        assert_eq!(value, BsnValue::Int(-3), "it reads back as a plain number");

        // All four widths, since all four are readable and writable.
        for (read, expected) in [
            (
                BsnValue::from_reflect(&NonZeroU16::new(7).unwrap(), &registry),
                7,
            ),
            (
                BsnValue::from_reflect(&NonZeroI32::new(-9).unwrap(), &registry),
                -9,
            ),
            (
                BsnValue::from_reflect(&NonZeroU32::new(11).unwrap(), &registry),
                11,
            ),
        ] {
            assert_eq!(read, BsnValue::Int(expected));
        }
        assert!(
            int_to_reflect(11, std::any::TypeId::of::<NonZeroU32>()).is_some(),
            "and the widest one comes back too",
        );

        let BsnValue::Int(i) = value else {
            unreachable!("just asserted");
        };
        let back = int_to_reflect(i, std::any::TypeId::of::<NonZeroI16>())
            .expect("and the number goes back to the value it came from");
        assert_eq!(
            back.try_downcast_ref::<NonZeroI16>().map(|v| v.get()),
            Some(-3),
        );
    }

    #[test]
    fn set_and_get_nested_field() {
        let mut ast = SceneBsnAst::default();
        let (registry, type_path) = marker_registry();

        // Create an entity with an empty Marker struct patch.
        let patch = ast
            .world
            .spawn(BsnPatch::Struct(BsnStructData {
                type_path: type_path.clone(),
                fields: BsnStructFields::default(),
            }))
            .id();
        let patches_entity = ast.world.spawn(BsnPatches(vec![patch])).id();

        set_bsn_field(
            &mut ast,
            patches_entity,
            &type_path,
            "x",
            BsnValue::Float(5.0),
            &registry,
        );

        let val = get_bsn_field(&ast, patches_entity, &type_path, "x");
        assert!(matches!(val, Some(BsnValue::Float(f)) if (f - 5.0).abs() < f64::EPSILON));
    }

    #[test]
    fn promotes_type_patch_to_struct() {
        let mut ast = SceneBsnAst::default();
        let (registry, type_path) = marker_registry();

        let patch = ast.world.spawn(BsnPatch::Type(type_path.clone())).id();
        let patches_entity = ast.world.spawn(BsnPatches(vec![patch])).id();

        set_bsn_field(
            &mut ast,
            patches_entity,
            &type_path,
            "value",
            BsnValue::Bool(true),
            &registry,
        );

        let val = get_bsn_field(&ast, patches_entity, &type_path, "value");
        assert!(matches!(val, Some(BsnValue::Bool(true))));
    }

    #[derive(Reflect, Default, Clone)]
    #[reflect(Default)]
    struct Axis {
        x: f32,
        y: f32,
        z: f32,
    }

    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Default, Component)]
    struct Spin(Axis);

    fn spin_registry() -> (TypeRegistry, String) {
        let mut registry = TypeRegistry::new();
        registry.register::<Spin>();
        registry.register::<Axis>();
        registry.register::<f32>();
        let type_path = registry
            .get(std::any::TypeId::of::<Spin>())
            .unwrap()
            .type_info()
            .type_path()
            .to_string();
        (registry, type_path)
    }

    #[test]
    fn set_field_on_newtype_tuple_struct_authors_tuple_patch() {
        let mut ast = SceneBsnAst::default();
        let (registry, type_path) = spin_registry();
        let patches_entity = ast.world.spawn(BsnPatches(Vec::new())).id();

        set_bsn_field(
            &mut ast,
            patches_entity,
            &type_path,
            "0.x",
            BsnValue::Float(1.5),
            &registry,
        );

        let patch_entity = ast
            .find_patch_by_type_path(patches_entity, &type_path)
            .expect("patch should be created");
        let patch = ast.get_patch(patch_entity).expect("patch entity");
        assert!(
            matches!(patch, BsnPatch::TupleStruct(_)),
            "newtype components must author as TupleStruct, got {patch:?}"
        );

        let val = get_bsn_field(&ast, patches_entity, &type_path, "0.x");
        assert!(matches!(val, Some(BsnValue::Float(f)) if (f - 1.5).abs() < f64::EPSILON));
    }

    // A struct component that registers no ReflectDefault, standing in for the
    // prefab IsA marker.
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component)]
    struct NoDefaultMarker {
        source: String,
        count: u32,
    }

    #[test]
    fn applies_default_less_struct_fieldwise() {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut reg = registry.write();
            reg.register::<NoDefaultMarker>();
            reg.register::<String>();
            reg.register::<u32>();
        }
        let type_path = {
            let reg = registry.read();
            reg.get(std::any::TypeId::of::<NoDefaultMarker>())
                .unwrap()
                .type_info()
                .type_path()
                .to_string()
        };
        world.insert_resource(registry);

        let entity = world.spawn_empty().id();
        let patch = BsnPatch::Struct(BsnStructData {
            type_path,
            fields: BsnStructFields(vec![
                BsnField {
                    name: "source".to_string(),
                    value: BsnValue::String("prefabs/tree.bsn".to_string()),
                },
                BsnField {
                    name: "count".to_string(),
                    value: BsnValue::Int(3),
                },
            ]),
        });

        apply_component_patch(&mut world, entity, &patch);

        let applied = world
            .get::<NoDefaultMarker>(entity)
            .expect("Default-less struct component must apply, not be dropped");
        assert_eq!(applied.source, "prefabs/tree.bsn");
        assert_eq!(applied.count, 3);
    }

    /// A struct on the far side of a pair, so the shape under test is
    /// `Vec<(String, BindPath)>`, how a binding maps an event's fields. A pair
    /// of primitives would not show the second half surviving as a value
    /// rather than as text.
    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Default)]
    struct Leg {
        raw: String,
    }

    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Component, Default)]
    struct Pairs {
        entries: Vec<(String, Leg)>,
    }

    fn one_pair() -> Vec<(String, Leg)> {
        vec![(
            "amount".to_string(),
            Leg {
                raw: "game::Health.current".to_string(),
            },
        )]
    }

    #[test]
    fn a_tuple_field_round_trips_through_the_document() {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut reg = registry.write();
            reg.register::<Pairs>();
        }
        let patch = {
            let reg = registry.read();
            crate::component_to_bsn_patch(
                &Pairs {
                    entries: one_pair(),
                },
                &reg,
            )
        };

        // The pair reaches the document as data, not as its `Debug` text. Both
        // halves are checked: the `Debug` text contains the same words, and
        // only the shape tells the two apart.
        let BsnPatch::Struct(data) = &patch else {
            panic!("a struct component authors as a struct patch, got {patch:?}");
        };
        let BsnValue::List(entries) = &data.fields.0[0].value else {
            panic!(
                "the entries author as a list, got {:?}",
                data.fields.0[0].value
            );
        };
        let Some(BsnValue::List(pair)) = entries.first() else {
            panic!("a pair authors as a list of its parts, got {entries:?}");
        };
        assert!(
            matches!(pair.first(), Some(BsnValue::String(name)) if name == "amount"),
            "the first half is the name as a string: {pair:?}",
        );
        assert!(
            matches!(pair.get(1), Some(BsnValue::Struct(leg)) if leg.type_path.ends_with("Leg")),
            "the second half is a struct value, not its Debug text: {pair:?}",
        );

        world.insert_resource(registry);
        let entity = world.spawn_empty().id();
        apply_component_patch(&mut world, entity, &patch);

        let applied = world
            .get::<Pairs>(entity)
            .expect("a component with a tuple field must apply");
        assert_eq!(applied.entries, one_pair());
    }
}

/// Field-navigation matrix for `set_bsn_field`/`get_bsn_field`. Each case
/// covers one of the reflect-JSON shapes field editing must handle (struct,
/// unit enum variant, tuple, map), or (where the primitives don't yet cover a
/// case) documents the gap with an ignored test.
#[cfg(test)]
mod field_navigation_matrix {
    use super::*;
    use crate::{BsnPatches, component_to_bsn_patch};
    use bevy::reflect::Reflect;

    fn type_path_of<T: Reflect>(registry: &TypeRegistry) -> String {
        registry
            .get(std::any::TypeId::of::<T>())
            .expect("type should be registered")
            .type_info()
            .type_path()
            .to_string()
    }

    fn one_patch_ast(patch: BsnPatch) -> (SceneBsnAst, Entity) {
        let mut ast = SceneBsnAst::default();
        let patch_entity = ast.world.spawn(patch).id();
        let patches_entity = ast.world.spawn(BsnPatches(vec![patch_entity])).id();
        (ast, patches_entity)
    }

    /// A list of enum values, the shape `Bindings(Vec<Binding>)` has.
    #[derive(Reflect, Debug, PartialEq)]
    enum TestChoice {
        Sized { size: f32 },
        Plain,
    }

    /// A document can hold a list element that does not fit its type, from a
    /// hand edit or a variant whose shape has changed. Applying a dynamic list
    /// onto a real `Vec` pushes through `from_reflect`, and bevy's `Vec` impl
    /// panics when that fails, so the element is dropped with a warning
    /// instead.
    #[test]
    fn a_list_element_that_does_not_fit_is_dropped_not_panicked_on() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestChoice>();
        registry.register::<Vec<TestChoice>>();
        registry.register::<f32>();

        let items = BsnValue::List(vec![
            BsnValue::Struct(BsnStructData {
                type_path: "TestChoice::Sized".into(),
                fields: BsnStructFields(vec![BsnField {
                    name: "size".into(),
                    // A size that is not a number: the variant cannot be built.
                    value: BsnValue::String("wide".into()),
                }]),
            }),
            BsnValue::Type("TestChoice::Plain".into()),
        ]);

        let reflected = bsn_value_to_reflect(
            &items,
            std::any::TypeId::of::<Vec<TestChoice>>(),
            &registry,
            None,
        )
        .expect("a list of a registered enum converts");

        let mut target: Vec<TestChoice> = Vec::new();
        apply_authored_value(&mut target, reflected.as_ref());

        assert_eq!(
            target,
            vec![TestChoice::Plain],
            "the element that fits survives; the one that does not is dropped",
        );
    }

    /// A `translation.x` style nested read: a struct component holding a
    /// nested struct field, read via a dotted path. Mirrors
    /// `get_field_reads_named_struct_field`.
    #[test]
    fn get_field_reads_named_struct_field() {
        let (ast, patches_entity) = one_patch_ast(BsnPatch::Struct(BsnStructData {
            type_path: "Transform".into(),
            fields: BsnStructFields(vec![BsnField {
                name: "translation".into(),
                value: BsnValue::Struct(BsnStructData {
                    type_path: "Vec3".into(),
                    fields: BsnStructFields(vec![
                        BsnField {
                            name: "x".into(),
                            value: BsnValue::Float(1.0),
                        },
                        BsnField {
                            name: "y".into(),
                            value: BsnValue::Float(2.0),
                        },
                        BsnField {
                            name: "z".into(),
                            value: BsnValue::Float(3.0),
                        },
                    ]),
                }),
            }]),
        }));

        let x = get_bsn_field(&ast, patches_entity, "Transform", "translation.x");
        assert!(matches!(x, Some(BsnValue::Float(f)) if (f - 1.0).abs() < f64::EPSILON));

        // Reading the intermediate struct returns the nested value whole.
        let translation = get_bsn_field(&ast, patches_entity, "Transform", "translation");
        assert!(matches!(translation, Some(BsnValue::Struct(_))));
    }

    /// An empty path returns the whole component value, matching the JSON
    /// layer's `get_field_empty_path_returns_whole_component`.
    #[test]
    fn get_field_empty_path_returns_whole_component() {
        let (ast, patches_entity) = one_patch_ast(BsnPatch::Struct(BsnStructData {
            type_path: "Transform".into(),
            fields: BsnStructFields(vec![BsnField {
                name: "x".into(),
                value: BsnValue::Float(5.0),
            }]),
        }));

        let whole = get_bsn_field(&ast, patches_entity, "Transform", "");
        assert!(
            matches!(whole, Some(BsnValue::Struct(ref data)) if data.type_path == "Transform"),
            "empty path should return the whole component value"
        );
    }

    /// A path segment that doesn't exist on the struct returns `None`.
    /// Mirrors `get_field_missing_path_returns_none`.
    #[test]
    fn get_field_missing_path_returns_none() {
        let (ast, patches_entity) = one_patch_ast(BsnPatch::Struct(BsnStructData {
            type_path: "Transform".into(),
            fields: BsnStructFields(vec![BsnField {
                name: "x".into(),
                value: BsnValue::Float(5.0),
            }]),
        }));

        let result = get_bsn_field(&ast, patches_entity, "Transform", "does_not_exist");
        assert!(result.is_none());
    }

    /// Querying a type path with no matching patch in the AST returns `None`.
    /// This is the BSN analog of `get_field_unregistered_type_returns_none`:
    /// there is no reflect `TypeRegistry` lookup involved in `get_bsn_field`
    /// at all (it addresses patches purely by their stored type-path string),
    /// so "unregistered" here means "no patch of that type exists yet".
    #[test]
    fn get_field_unregistered_type_returns_none() {
        let (ast, patches_entity) = one_patch_ast(BsnPatch::Struct(BsnStructData {
            type_path: "Transform".into(),
            fields: BsnStructFields::default(),
        }));

        let result = get_bsn_field(
            &ast,
            patches_entity,
            "not::A::RegisteredType",
            "translation",
        );
        assert!(result.is_none());
    }

    /// A `PathBuf` component field (as in the prefab `IsA.source` field)
    /// serializes through `component_to_bsn_patch` as a plain
    /// `BsnValue::String`, reads back through `get_bsn_field` without literal
    /// quote characters, and applies back onto a reflected `PathBuf` field. The
    /// prefab BSN resolver reads `IsA.source` as `BsnValue::String`, so a Debug
    /// fallback (which wraps the path in quotes) would break cache lookups.
    #[derive(bevy::reflect::Reflect, Default, PartialEq, Debug)]
    struct PathBufProbe {
        source: std::path::PathBuf,
        deleted: Vec<u32>,
    }

    #[test]
    fn pathbuf_field_round_trips_as_plain_string() {
        let mut registry = TypeRegistry::new();
        registry.register::<PathBufProbe>();
        registry.register::<std::path::PathBuf>();
        registry.register::<Vec<u32>>();
        let tp = type_path_of::<PathBufProbe>(&registry);

        let probe = PathBufProbe {
            source: std::path::PathBuf::from("prefabs/tree.bsn"),
            deleted: vec![1, 2],
        };
        let patch = component_to_bsn_patch(probe.as_partial_reflect(), &registry);

        let mut ast = SceneBsnAst::default();
        let patch_entity = ast.world.spawn(patch).id();
        let patches_entity = ast.world.spawn(BsnPatches(vec![patch_entity])).id();

        let source = get_bsn_field(&ast, patches_entity, &tp, "source");
        match source {
            Some(BsnValue::String(s)) => {
                assert_eq!(
                    s, "prefabs/tree.bsn",
                    "PathBuf must serialize as a plain string with no quote characters"
                );
            }
            other => panic!("expected BsnValue::String, got {}", describe_value(&other)),
        }

        // The plain string applies back onto a reflected PathBuf field.
        let reflected = bsn_value_to_reflect(
            &BsnValue::String("prefabs/tree.bsn".to_string()),
            std::any::TypeId::of::<std::path::PathBuf>(),
            &registry,
            None,
        )
        .expect("string converts to a reflected PathBuf");
        let path = reflected
            .try_downcast_ref::<std::path::PathBuf>()
            .expect("reflected value is a PathBuf");
        assert_eq!(path, &std::path::PathBuf::from("prefabs/tree.bsn"));
    }

    fn describe_value(value: &Option<BsnValue>) -> String {
        match value {
            None => "None".to_string(),
            Some(BsnValue::String(s)) => format!("String({s:?})"),
            Some(BsnValue::Struct(d)) => format!("Struct({})", d.type_path),
            Some(BsnValue::TupleStruct(d)) => format!("TupleStruct({})", d.type_path),
            Some(BsnValue::Type(t)) => format!("Type({t})"),
            Some(_) => "Other".to_string(),
        }
    }

    /// `set_bsn_field` then `get_bsn_field` round-trips a nested field.
    /// Mirrors `get_field_round_trips_with_set_field`.
    #[test]
    fn get_field_round_trips_with_set_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();
        let tp = type_path_of::<Transform>(&registry);

        let (mut ast, patches_entity) = one_patch_ast(BsnPatch::Struct(BsnStructData {
            type_path: tp.clone(),
            fields: BsnStructFields(vec![BsnField {
                name: "translation".into(),
                value: BsnValue::Struct(BsnStructData {
                    type_path: "Vec3".into(),
                    fields: BsnStructFields::default(),
                }),
            }]),
        }));

        set_bsn_field(
            &mut ast,
            patches_entity,
            &tp,
            "translation.y",
            BsnValue::Float(7.0),
            &registry,
        );

        let result = get_bsn_field(&ast, patches_entity, &tp, "translation.y");
        assert!(matches!(result, Some(BsnValue::Float(f)) if (f - 7.0).abs() < f64::EPSILON));
    }

    /// An empty path replaces the whole component value in one call, matching
    /// the JSON layer's `set_field_in_component_json_empty_path_replaces_value`.
    #[test]
    fn set_field_empty_path_replaces_whole_value() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();
        let tp = type_path_of::<Transform>(&registry);

        let (mut ast, patches_entity) = one_patch_ast(BsnPatch::Struct(BsnStructData {
            type_path: tp.clone(),
            fields: BsnStructFields(vec![BsnField {
                name: "x".into(),
                value: BsnValue::Float(0.0),
            }]),
        }));

        let replacement = BsnValue::Struct(BsnStructData {
            type_path: tp.clone(),
            fields: BsnStructFields(vec![BsnField {
                name: "x".into(),
                value: BsnValue::Float(9.0),
            }]),
        });
        set_bsn_field(&mut ast, patches_entity, &tp, "", replacement, &registry);

        let whole = get_bsn_field(&ast, patches_entity, &tp, "x");
        assert!(matches!(whole, Some(BsnValue::Float(f)) if (f - 9.0).abs() < f64::EPSILON));
    }

    /// `set_bsn_field` refuses to write a field for a type the reflect
    /// `TypeRegistry` does not know about, matching the JSON layer's
    /// `set_field_in_component_json_unregistered_type_is_noop`.
    #[test]
    fn set_field_on_unregistered_type_is_noop() {
        let mut ast = SceneBsnAst::default();
        let patches_entity = ast.world.spawn(BsnPatches(Vec::new())).id();

        let registry = TypeRegistry::default();
        set_bsn_field(
            &mut ast,
            patches_entity,
            "not::A::RegisteredType",
            "translation",
            BsnValue::Float(1.0),
            &registry,
        );

        assert!(
            ast.find_patch_by_type_path(patches_entity, "not::A::RegisteredType")
                .is_none(),
            "no patch should be created for an unregistered type"
        );
    }

    // A small reflect-derived enum with a struct variant, mirroring the
    // `TestShape` pattern from the JSN-path test suite.
    #[derive(Component, Reflect, Clone)]
    #[reflect(Default)]
    enum TestShape {
        Sphere { radius: f32 },
        Box { half_x: f32, half_y: f32 },
    }

    impl Default for TestShape {
        fn default() -> Self {
            TestShape::Sphere { radius: 0.0 }
        }
    }

    /// Reading a field of the active enum variant navigates through the
    /// flattened struct-variant patch. `find_patch_by_type_path` matches the
    /// base enum type path against the stored `Enum::Variant` path, and the
    /// variant's fields land directly on that `BsnPatch::Struct`. Mirrors
    /// `get_field_unwraps_enum_variant_and_reads_field`.
    #[test]
    fn get_field_unwraps_enum_variant_and_reads_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestShape>();
        registry.register::<f32>();
        let base_type_path = type_path_of::<TestShape>(&registry);

        let shape = TestShape::Sphere { radius: 1.0 };
        let patch = component_to_bsn_patch(&shape, &registry);
        let (ast, patches_entity) = one_patch_ast(patch);

        let result = get_bsn_field(&ast, patches_entity, &base_type_path, "radius");
        assert!(matches!(result, Some(BsnValue::Float(f)) if (f - 1.0).abs() < f64::EPSILON));
    }

    // A reflect-derived struct wrapped in a tuple ("newtype") enum variant,
    // mirroring the `TestModifier::Wrap(TestInner)` pattern from the JSN
    // test suite.
    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Default)]
    struct TestInner {
        flag: bool,
        amount: f32,
    }

    #[derive(Component, Reflect, Clone)]
    #[reflect(Default)]
    enum TestModifier {
        Wrap(TestInner),
    }

    impl Default for TestModifier {
        fn default() -> Self {
            TestModifier::Wrap(TestInner::default())
        }
    }

    /// Reads and writes a newtype-wrapped variant's inner field through index
    /// `0` (`"0.flag"`). Bevy reflects a tuple-variant enum as a
    /// `BsnPatch::TupleStruct`, and the navigator descends its values by index,
    /// matching the JSON layer's
    /// `newtype_variant_field_round_trips_through_index_zero`.
    #[test]
    fn newtype_variant_field_round_trips_through_index_zero() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestModifier>();
        registry.register::<TestInner>();
        registry.register::<bool>();
        registry.register::<f32>();
        let base_type_path = type_path_of::<TestModifier>(&registry);

        let modifier = TestModifier::Wrap(TestInner {
            flag: false,
            amount: 1.5,
        });
        let patch = component_to_bsn_patch(&modifier, &registry);
        let (mut ast, patches_entity) = one_patch_ast(patch);

        let read = get_bsn_field(&ast, patches_entity, &base_type_path, "0.flag");
        assert!(matches!(read, Some(BsnValue::Bool(false))));

        set_bsn_field(
            &mut ast,
            patches_entity,
            &base_type_path,
            "0.flag",
            BsnValue::Bool(true),
            &registry,
        );
        let read_back = get_bsn_field(&ast, patches_entity, &base_type_path, "0.flag");
        assert!(matches!(read_back, Some(BsnValue::Bool(true))));
    }

    // A reflect-derived struct with a list field, mirroring the `TestList`
    // pattern from the JSN-path test suite.
    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Default)]
    struct TestList {
        items: Vec<f32>,
    }

    /// Reads a list element via `items[1]` bracket syntax, matching the JSON
    /// layer's `get_field_bracket_index_reads_list_element`.
    #[test]
    fn get_field_bracket_index_reads_list_element() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestList>();
        registry.register::<Vec<f32>>();
        registry.register::<f32>();
        let base_type_path = type_path_of::<TestList>(&registry);

        let list = TestList {
            items: vec![10.0, 20.0, 30.0],
        };
        let patch = component_to_bsn_patch(&list, &registry);
        let (ast, patches_entity) = one_patch_ast(patch);

        let result = get_bsn_field(&ast, patches_entity, &base_type_path, "items[1]");
        assert!(matches!(result, Some(BsnValue::Float(f)) if (f - 20.0).abs() < f64::EPSILON));
    }

    /// A struct with a map field, whose entries are read and written via
    /// `field[key]` bracket-key syntax (the shape the inspector emits for
    /// map-valued components like custom properties).
    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Default)]
    struct TestMap {
        props: std::collections::HashMap<String, f32>,
    }

    #[test]
    fn bracket_key_reads_and_writes_map_entry() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestMap>();
        registry.register::<std::collections::HashMap<String, f32>>();
        registry.register::<String>();
        registry.register::<f32>();
        let base_type_path = type_path_of::<TestMap>(&registry);

        let mut props = std::collections::HashMap::new();
        props.insert("hp".to_string(), 10.0_f32);
        let map = TestMap { props };
        let patch = component_to_bsn_patch(&map, &registry);
        let (mut ast, patches_entity) = one_patch_ast(patch);

        let read = get_bsn_field(&ast, patches_entity, &base_type_path, "props[hp]");
        assert!(matches!(read, Some(BsnValue::Float(f)) if (f - 10.0).abs() < f64::EPSILON));

        set_bsn_field(
            &mut ast,
            patches_entity,
            &base_type_path,
            "props[hp]",
            BsnValue::Float(25.0),
            &registry,
        );
        let read_back = get_bsn_field(&ast, patches_entity, &base_type_path, "props[hp]");
        assert!(matches!(read_back, Some(BsnValue::Float(f)) if (f - 25.0).abs() < f64::EPSILON));
    }
}

/// Value-level reflection tests: applying BSN patches that carry enum-variant
/// values (including maps of enums) back onto concrete components.
#[cfg(test)]
mod apply_value_tests {
    use super::*;
    use crate::component_to_bsn_patch;
    use bevy::reflect::Reflect;
    use std::collections::HashMap;

    #[derive(Reflect, Clone, PartialEq, Debug, Default)]
    #[reflect(Default)]
    enum Prop {
        Number(f32),
        Flag(bool),
        #[default]
        Empty,
    }

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct EnumFieldHolder {
        prop: Prop,
    }

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct PropMapHolder {
        props: HashMap<String, Prop>,
    }

    fn base_world() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut w = registry.write();
            w.register::<f32>();
            w.register::<bool>();
            w.register::<String>();
            w.register::<Prop>();
            w.register::<EnumFieldHolder>();
            w.register::<PropMapHolder>();
            w.register::<HashMap<String, Prop>>();
        }
        world.insert_resource(registry);
        world.insert_resource(SceneBsnAst::default());
        world
    }

    /// Emit a component to a BSN patch, apply it onto a fresh entity, and
    /// return that entity so the caller can read the reconstructed component.
    fn round_trip_patch(world: &mut World, patch: BsnPatch) -> Entity {
        let patches_entity = world
            .resource_mut::<SceneBsnAst>()
            .create_entity_node(vec![patch]);
        let entity = world.spawn(AstNodeRef { patches_entity }).id();
        world
            .resource_mut::<SceneBsnAst>()
            .link(entity, patches_entity);
        apply_ast_to_ecs(world, entity);
        entity
    }

    #[test]
    fn enum_typed_field_data_variant_round_trips_through_apply() {
        let mut world = base_world();

        let patch = {
            let registry = world.resource::<AppTypeRegistry>().read();
            component_to_bsn_patch(
                &EnumFieldHolder {
                    prop: Prop::Number(7.0),
                },
                &registry,
            )
        };
        let entity = round_trip_patch(&mut world, patch);

        let holder = world
            .get::<EnumFieldHolder>(entity)
            .expect("holder applied");
        assert_eq!(holder.prop, Prop::Number(7.0));
    }

    #[test]
    fn map_of_enum_round_trips_through_apply() {
        let mut world = base_world();

        let mut props = HashMap::new();
        props.insert("hp".to_string(), Prop::Number(7.0));
        props.insert("alive".to_string(), Prop::Flag(true));

        let patch = {
            let registry = world.resource::<AppTypeRegistry>().read();
            component_to_bsn_patch(&PropMapHolder { props }, &registry)
        };
        let entity = round_trip_patch(&mut world, patch);

        let holder = world.get::<PropMapHolder>(entity).expect("holder applied");
        assert_eq!(holder.props.get("hp"), Some(&Prop::Number(7.0)));
        assert_eq!(holder.props.get("alive"), Some(&Prop::Flag(true)));
    }
}
