//! Apply BSN AST patches to ECS entities via reflection.
//!
//! [`apply_dirty_ast_patches`] processes entities marked [`AstDirty`],
//! reading their patches from the AST and inserting the corresponding
//! ECS components. Called explicitly during scene load and paste operations.

use std::any::TypeId;

use bevy::asset::{AssetServer, ReflectHandle};
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::prelude::*;
use bevy::reflect::{
    PartialReflect, ReflectMut, TypeRegistry,
    enums::{DynamicEnum, DynamicVariant},
    list::DynamicList,
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

/// Spawn ECS entities from the [`SceneBsnAst`] resource, linking them back to
/// AST nodes. All entities are marked [`AstDirty`] so a following call to
/// [`apply_dirty_ast_patches`] populates ECS components.
pub fn spawn_from_ast(world: &mut World) -> Vec<Entity> {
    let roots: Vec<Entity> = world.resource::<SceneBsnAst>().roots.clone();
    let mut spawned = Vec::new();

    for root in roots {
        spawn_ast_node(world, root, None, &mut spawned);
    }

    spawned
}

fn spawn_ast_node(
    world: &mut World,
    ast_entity: Entity,
    parent: Option<Entity>,
    spawned: &mut Vec<Entity>,
) {
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
        spawn_ast_node(world, child_ast, Some(ecs_entity), spawned);
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

/// Apply a bare type patch (unit struct or enum variant with all defaults).
fn apply_type_patch(world: &mut World, entity: Entity, type_path: &str) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    // Try as a direct type first.
    if let Some(registration) = reg.get_with_type_path(type_path) {
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };
        let value = reflect_default.default();
        reflect_component.insert(&mut world.entity_mut(entity), value.as_partial_reflect(), &reg);
        return;
    }

    // Try as an enum variant: split off last `::` segment.
    if let Some(last_sep) = type_path.rfind("::") {
        let enum_path = &type_path[..last_sep];
        let variant_name = &type_path[last_sep + 2..];

        let Some(registration) = reg.get_with_type_path(enum_path) else {
            return;
        };
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };

        let mut value = reflect_default.default();
        if let ReflectMut::Enum(e) = value.reflect_mut() {
            let dynamic_enum = DynamicEnum::new(variant_name, DynamicVariant::Unit);
            e.apply(&dynamic_enum);
        }
        reflect_component.insert(&mut world.entity_mut(entity), value.as_partial_reflect(), &reg);
    }
}

/// Apply a struct patch: merge specified fields over existing component (or
/// default if it doesn't exist yet). Nested struct fields are merged
/// recursively so that partial patches like `Transform { translation: Vec3 { x: 5.0 } }`
/// only update the specified sub-fields.
fn apply_struct_patch(world: &mut World, entity: Entity, data: &BsnStructData) {
    let asset_server = world.get_resource::<AssetServer>().cloned();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    // Direct lookup: the type_path is a struct component.
    if let Some(registration) = reg.get_with_type_path(&data.type_path) {
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };

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

        if let ReflectMut::Struct(s) = value.reflect_mut() {
            for field in &data.fields.0 {
                if let Some(target) = s.field_mut(&field.name) {
                    merge_bsn_value_into_reflect(target, &field.value, &reg, asset_server.as_ref());
                }
            }
        }

        reflect_component.insert(&mut world.entity_mut(entity), value.as_partial_reflect(), &reg);
        return;
    }

    // Enum variant lookup: type_path is "EnumType::Variant" with struct fields.
    if let Some(last_sep) = data.type_path.rfind("::") {
        let enum_path = &data.type_path[..last_sep];
        let variant_name = &data.type_path[last_sep + 2..];

        let Some(registration) = reg.get_with_type_path(enum_path) else {
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
                            asset_server.as_ref(),
                        )
                    {
                        target.apply(&*reflected);
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
                    if let Some(reflected) = bsn_value_to_reflect(
                        &field.value,
                        field_type_id,
                        &reg,
                        asset_server.as_ref(),
                    ) {
                        dynamic_struct.insert_boxed(&field.name, reflected);
                    }
                }
                let dynamic_enum =
                    DynamicEnum::new(variant_name, DynamicVariant::Struct(dynamic_struct));
                e.apply(&dynamic_enum);
            }
        }

        reflect_component.insert(&mut world.entity_mut(entity), value.as_partial_reflect(), &reg);
    }
}

/// Recursively merge a BSN value into an existing reflected value.
/// For struct values, only the specified sub-fields are updated; unmentioned
/// fields keep their current value. For primitives, the value is replaced.
fn merge_bsn_value_into_reflect(
    target: &mut dyn PartialReflect,
    value: &BsnValue,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) {
    match value {
        BsnValue::Struct(data) => {
            if let ReflectMut::Struct(s) = target.reflect_mut() {
                for field in &data.fields.0 {
                    if let Some(target_field) = s.field_mut(&field.name) {
                        merge_bsn_value_into_reflect(
                            target_field,
                            &field.value,
                            registry,
                            asset_server,
                        );
                    }
                }
            }
        }
        _ => {
            if let Some(type_info) = target.get_represented_type_info()
                && let Some(reflected) =
                    bsn_value_to_reflect(value, type_info.type_id(), registry, asset_server)
            {
                target.apply(&*reflected);
            }
        }
    }
}

/// Apply a tuple struct patch: merge over existing component (or default).
fn apply_tuple_struct_patch(world: &mut World, entity: Entity, data: &BsnTupleStructData) {
    let asset_server = world.get_resource::<AssetServer>().cloned();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    let Some(registration) = reg.get_with_type_path(&data.type_path) else {
        return;
    };
    let Some(reflect_default) = registration.data::<ReflectDefault>() else {
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };

    let Ok(tuple_info) = registration.type_info().as_tuple_struct() else {
        return;
    };

    // Start from existing component value if present, otherwise from default.
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

    if let ReflectMut::TupleStruct(ts) = value.reflect_mut() {
        for (i, bsn_val) in data.values.iter().enumerate() {
            let Some(field_info) = tuple_info.field_at(i) else {
                continue;
            };
            if let Some(reflected) =
                bsn_value_to_reflect(bsn_val, field_info.ty().id(), &reg, asset_server.as_ref())
                && let Some(target) = ts.field_mut(i)
            {
                target.apply(&*reflected);
            }
        }
    }

    reflect_component.insert(&mut world.entity_mut(entity), value.as_partial_reflect(), &reg);
}

/// Convert a [`BsnValue`] to a boxed reflected value given the expected type.
pub fn bsn_value_to_reflect(
    value: &BsnValue,
    expected: TypeId,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<Box<dyn PartialReflect>> {
    // If the expected type is a Handle<T>, resolve from an asset path string.
    if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(expected) {
        if let BsnValue::String(path) = value
            && !path.is_empty()
            && let Some(asset_server) = asset_server
        {
            let asset_type_id = reflect_handle.asset_type_id();
            let untyped = asset_server
                .load_builder()
                .load_erased(asset_type_id, path.to_owned());
            let typed = reflect_handle.typed(untyped);
            return Some(typed.into_partial_reflect());
        }
        // Empty string or no asset server: return the default handle.
        if let Some(registration) = registry.get(expected)
            && let Some(reflect_default) = registration.data::<ReflectDefault>()
        {
            return Some(reflect_default.default().into_partial_reflect());
        }
        return None;
    }

    match value {
        BsnValue::Float(f) => float_to_reflect(*f, expected),
        BsnValue::Int(i) => int_to_reflect(*i, expected),
        BsnValue::Bool(b) => Some(Box::new(*b)),
        BsnValue::String(s) => Some(Box::new(s.clone())),
        BsnValue::Type(type_path) => type_value_to_reflect(type_path, expected, registry),
        BsnValue::Struct(data) => struct_value_to_reflect(data, registry, asset_server),
        BsnValue::TupleStruct(data) => tuple_struct_value_to_reflect(data, registry, asset_server),
        BsnValue::List(items) => list_value_to_reflect(items, expected, registry, asset_server),
    }
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
    } else {
        None
    }
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
        e.apply(&dynamic_enum);
    }
    Some(value.into_partial_reflect())
}

fn struct_value_to_reflect(
    data: &BsnStructData,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get_with_type_path(&data.type_path)?;
    let reflect_default = registration.data::<ReflectDefault>()?;
    let struct_info = registration.type_info().as_struct().ok()?;

    let mut value = reflect_default.default();
    if let ReflectMut::Struct(s) = value.reflect_mut() {
        for field in &data.fields.0 {
            if let Some(field_info) = struct_info.field(&field.name)
                && let Some(reflected) =
                    bsn_value_to_reflect(&field.value, field_info.ty().id(), registry, asset_server)
                && let Some(target) = s.field_mut(&field.name)
            {
                target.apply(&*reflected);
            }
        }
    }
    Some(value.into_partial_reflect())
}

fn tuple_struct_value_to_reflect(
    data: &BsnTupleStructData,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get_with_type_path(&data.type_path)?;
    let reflect_default = registration.data::<ReflectDefault>()?;
    let tuple_info = registration.type_info().as_tuple_struct().ok()?;

    let mut value = reflect_default.default();
    if let ReflectMut::TupleStruct(ts) = value.reflect_mut() {
        for (i, bsn_val) in data.values.iter().enumerate() {
            if let Some(field_info) = tuple_info.field_at(i)
                && let Some(reflected) =
                    bsn_value_to_reflect(bsn_val, field_info.ty().id(), registry, asset_server)
                && let Some(target) = ts.field_mut(i)
            {
                target.apply(&*reflected);
            }
        }
    }
    Some(value.into_partial_reflect())
}

fn list_value_to_reflect(
    items: &[BsnValue],
    expected: TypeId,
    registry: &TypeRegistry,
    asset_server: Option<&AssetServer>,
) -> Option<Box<dyn PartialReflect>> {
    let registration = registry.get(expected)?;
    let list_info = registration.type_info().as_list().ok()?;
    let item_type_id = list_info.item_ty().id();

    let mut dynamic_list = DynamicList::default();
    for item in items {
        if let Some(reflected) = bsn_value_to_reflect(item, item_type_id, registry, asset_server) {
            dynamic_list.push_box(reflected);
        }
    }
    dynamic_list.set_represented_type(Some(registration.type_info()));
    Some(Box::new(dynamic_list))
}

/// Set a field value at a dotted path within an entity's AST patches.
///
/// Creates the struct patch and intermediate fields if they don't exist.
pub fn set_bsn_field(
    ast: &mut SceneBsnAst,
    patches_entity: Entity,
    type_path: &str,
    field_path: &str,
    value: BsnValue,
    registry: &TypeRegistry,
) {
    // Ensure a Struct patch exists for this type.
    let patch_entity = match ast.find_patch_by_type_path(patches_entity, type_path) {
        Some(pe) => pe,
        None => {
            let pe = ast
                .world
                .spawn(BsnPatch::Struct(BsnStructData {
                    type_path: type_path.to_string(),
                    fields: BsnStructFields::default(),
                }))
                .id();
            if let Some(patches) = ast.get_patches_mut(patches_entity) {
                patches.0.push(pe);
            }
            pe
        }
    };

    // If the patch is a bare Type (all defaults), promote to Struct,
    // preserving the original type path (which may be variant-qualified).
    if let Some(patch) = ast.world.get_mut::<BsnPatch>(patch_entity) {
        let patch = patch.into_inner();
        if let BsnPatch::Type(existing_tp) = patch {
            let preserved_tp = existing_tp.clone();
            *patch = BsnPatch::Struct(BsnStructData {
                type_path: preserved_tp,
                fields: BsnStructFields::default(),
            });
        }
    }

    // Navigate to the field and set the value.
    let Some(patch) = ast.world.get_mut::<BsnPatch>(patch_entity) else {
        return;
    };
    let patch = patch.into_inner();
    let BsnPatch::Struct(data) = patch else { return };

    let segments: Vec<&str> = field_path.split('.').collect();
    set_nested_field(&mut data.fields, &segments, value, type_path, registry);
}

/// Get a field value at a dotted path within an entity's AST patches.
pub fn get_bsn_field(
    ast: &SceneBsnAst,
    patches_entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<BsnValue> {
    let patch_entity = ast.find_patch_by_type_path(patches_entity, type_path)?;
    let patch = ast.get_patch(patch_entity)?;
    let BsnPatch::Struct(data) = patch else {
        return None;
    };

    let segments: Vec<&str> = field_path.split('.').collect();
    get_nested_field(&data.fields, &segments)
}

fn set_nested_field(
    fields: &mut BsnStructFields,
    segments: &[&str],
    value: BsnValue,
    parent_type_path: &str,
    registry: &TypeRegistry,
) {
    if segments.is_empty() {
        return;
    }
    let field_name = segments[0];

    if segments.len() == 1 {
        // Leaf: set or create the field.
        if let Some(field) = fields.0.iter_mut().find(|f| f.name == field_name) {
            field.value = value;
        } else {
            fields.0.push(BsnField {
                name: field_name.to_string(),
                value,
            });
        }
        return;
    }

    // Non-leaf: navigate into a nested struct value.
    let remaining = &segments[1..];
    let nested_type_path =
        get_field_type_path(parent_type_path, field_name, registry).unwrap_or_default();

    // Find or create the intermediate field.
    let existing = fields.0.iter_mut().find(|f| f.name == field_name);
    let nested_fields = if let Some(field) = existing {
        match &mut field.value {
            BsnValue::Struct(nested_data) => &mut nested_data.fields,
            other => {
                *other = BsnValue::Struct(BsnStructData {
                    type_path: nested_type_path.clone(),
                    fields: BsnStructFields::default(),
                });
                if let BsnValue::Struct(d) = other {
                    &mut d.fields
                } else {
                    unreachable!()
                }
            }
        }
    } else {
        fields.0.push(BsnField {
            name: field_name.to_string(),
            value: BsnValue::Struct(BsnStructData {
                type_path: nested_type_path.clone(),
                fields: BsnStructFields::default(),
            }),
        });
        if let BsnValue::Struct(ref mut d) = fields.0.last_mut().unwrap().value {
            &mut d.fields
        } else {
            unreachable!()
        }
    };

    set_nested_field(nested_fields, remaining, value, &nested_type_path, registry);
}

fn get_nested_field(fields: &BsnStructFields, segments: &[&str]) -> Option<BsnValue> {
    if segments.is_empty() {
        return None;
    }
    let field = fields.0.iter().find(|f| f.name == segments[0])?;

    if segments.len() == 1 {
        return Some(field.value.clone());
    }
    if let BsnValue::Struct(ref data) = field.value {
        get_nested_field(&data.fields, &segments[1..])
    } else {
        None
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

/// Parse a string (from inspector text input) into a [`BsnValue`], given the
/// expected field type.
pub fn parse_string_to_bsn_value(value_str: &str, expected: TypeId) -> Option<BsnValue> {
    if expected == TypeId::of::<f32>() || expected == TypeId::of::<f64>() {
        value_str.parse::<f64>().ok().map(BsnValue::Float)
    } else if expected == TypeId::of::<i8>()
        || expected == TypeId::of::<u8>()
        || expected == TypeId::of::<i16>()
        || expected == TypeId::of::<u16>()
        || expected == TypeId::of::<i32>()
        || expected == TypeId::of::<u32>()
        || expected == TypeId::of::<i64>()
        || expected == TypeId::of::<u64>()
        || expected == TypeId::of::<isize>()
        || expected == TypeId::of::<usize>()
    {
        value_str.parse::<i128>().ok().map(BsnValue::Int)
    } else if expected == TypeId::of::<bool>() {
        value_str.parse::<bool>().ok().map(BsnValue::Bool)
    } else if expected == TypeId::of::<String>() {
        Some(BsnValue::String(value_str.to_string()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BsnPatches, BsnStructFields};

    #[test]
    fn set_and_get_nested_field() {
        let mut ast = SceneBsnAst::default();

        // Create an entity with an empty Transform struct patch.
        let patch = ast
            .world
            .spawn(BsnPatch::Struct(BsnStructData {
                type_path: "Transform".into(),
                fields: BsnStructFields::default(),
            }))
            .id();
        let patches_entity = ast.world.spawn(BsnPatches(vec![patch])).id();

        // Set a nested field (no registry needed for leaf-only path).
        let registry = TypeRegistry::default();
        set_bsn_field(
            &mut ast,
            patches_entity,
            "Transform",
            "x",
            BsnValue::Float(5.0),
            &registry,
        );

        let val = get_bsn_field(&ast, patches_entity, "Transform", "x");
        assert!(matches!(val, Some(BsnValue::Float(f)) if (f - 5.0).abs() < f64::EPSILON));
    }

    #[test]
    fn promotes_type_patch_to_struct() {
        let mut ast = SceneBsnAst::default();

        let patch = ast.world.spawn(BsnPatch::Type("MyType".into())).id();
        let patches_entity = ast.world.spawn(BsnPatches(vec![patch])).id();

        let registry = TypeRegistry::default();
        set_bsn_field(
            &mut ast,
            patches_entity,
            "MyType",
            "value",
            BsnValue::Bool(true),
            &registry,
        );

        let val = get_bsn_field(&ast, patches_entity, "MyType", "value");
        assert!(matches!(val, Some(BsnValue::Bool(true))));
    }
}
