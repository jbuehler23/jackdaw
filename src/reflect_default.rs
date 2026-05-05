//! Default-construct any reflected type, including ones that
//! don't `#[derive(Default)]`.
//!
//! Walks `TypeInfo` and recurses into struct/tuple/enum fields,
//! using each type's `ReflectDefault` (auto-registered for every
//! stdlib primitive) at the leaves. The resulting `DynamicStruct`
//! is fed to `ReflectComponent::insert`, which calls
//! `from_reflect_with_fallback` to materialise the typed value
//! via `ReflectFromReflect` (auto-derived from
//! `#[derive(Reflect)]`).
//!
//! Returns `None` for shapes the editor can't synthesise: opaque
//! non-primitive types (e.g. `Box<dyn Trait>`), arrays of unknown
//! length, lists/maps/sets without `ReflectDefault`. The picker
//! pre-flights this and skips unconstructible types; the operator
//! warns.

use std::any::TypeId;

use bevy::reflect::std_traits::ReflectDefault;
use bevy::reflect::{
    DynamicEnum, DynamicStruct, DynamicTuple, DynamicTupleStruct, DynamicVariant, PartialReflect,
    Reflect, TypeInfo, TypeRegistry, VariantInfo,
};

/// Default-instance for `type_id`. Uses `ReflectDefault` when
/// present; otherwise walks the type's fields recursively.
pub fn build_reflective_default(
    type_id: TypeId,
    registry: &TypeRegistry,
) -> Option<Box<dyn Reflect>> {
    let registration = registry.get(type_id)?;

    if let Some(reflect_default) = registration.data::<ReflectDefault>() {
        return Some(reflect_default.default());
    }

    let info = registration.type_info();
    let dynamic: Box<dyn PartialReflect> = match info {
        TypeInfo::Struct(struct_info) => {
            let mut dyn_struct = DynamicStruct::default();
            for field in struct_info.iter() {
                let field_default = build_reflective_default(field.type_id(), registry)?;
                dyn_struct.insert_boxed(field.name(), field_default.into_partial_reflect());
            }
            dyn_struct.set_represented_type(Some(info));
            Box::new(dyn_struct)
        }
        TypeInfo::TupleStruct(ts_info) => {
            let mut dyn_tuple_struct = DynamicTupleStruct::default();
            for field in ts_info.iter() {
                let field_default = build_reflective_default(field.type_id(), registry)?;
                dyn_tuple_struct.insert_boxed(field_default.into_partial_reflect());
            }
            dyn_tuple_struct.set_represented_type(Some(info));
            Box::new(dyn_tuple_struct)
        }
        TypeInfo::Tuple(tuple_info) => {
            let mut dyn_tuple = DynamicTuple::default();
            for field in tuple_info.iter() {
                let field_default = build_reflective_default(field.type_id(), registry)?;
                dyn_tuple.insert_boxed(field_default.into_partial_reflect());
            }
            dyn_tuple.set_represented_type(Some(info));
            Box::new(dyn_tuple)
        }
        TypeInfo::Enum(enum_info) => {
            // First variant (matches `#[derive(Default)]`'s rule
            // when no variant carries `#[default]`).
            let variant = enum_info.iter().next()?;
            let dyn_variant = match variant {
                VariantInfo::Unit(_) => DynamicVariant::Unit,
                VariantInfo::Tuple(t) => {
                    let mut dt = DynamicTuple::default();
                    for f in t.iter() {
                        let fd = build_reflective_default(f.type_id(), registry)?;
                        dt.insert_boxed(fd.into_partial_reflect());
                    }
                    DynamicVariant::Tuple(dt)
                }
                VariantInfo::Struct(s) => {
                    let mut ds = DynamicStruct::default();
                    for f in s.iter() {
                        let fd = build_reflective_default(f.type_id(), registry)?;
                        ds.insert_boxed(f.name(), fd.into_partial_reflect());
                    }
                    DynamicVariant::Struct(ds)
                }
            };
            let mut dyn_enum = DynamicEnum::new(variant.name(), dyn_variant);
            dyn_enum.set_represented_type(Some(info));
            Box::new(dyn_enum)
        }
        // Caller skips these (no meaningful default without
        // `ReflectDefault`).
        TypeInfo::List(_)
        | TypeInfo::Array(_)
        | TypeInfo::Map(_)
        | TypeInfo::Set(_)
        | TypeInfo::Opaque(_) => return None,
    };

    // Materialise a typed `Box<dyn Reflect>` from the dynamic
    // form via `FromReflect`.
    let reflect_from_reflect = registration.data::<bevy::reflect::ReflectFromReflect>()?;
    reflect_from_reflect.from_reflect(dynamic.as_ref())
}
