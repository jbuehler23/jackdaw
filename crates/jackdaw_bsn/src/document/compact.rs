//! The canonical spelling of floats and of values held in list elements.
//!
//! A list replaces whole under [`crate::apply_deltas`], and a struct read from
//! a document starts from its type's default, so a field of a struct inside a
//! list that equals that default adds nothing and is left out. Nowhere else:
//! a field left out of a struct outside a list means "inherit" to a prefab
//! override, not "default".
//!
//! A document float is an `f64`, written and compared exactly, so an `f32` is
//! held as its shortest text. Only a known type says a float is an `f32`: live
//! values are encoded that way, and [`compact_document`] converts older text
//! wherever the registry knows the field. A float of an unregistered type keeps
//! the value it was read with.
//!
//! Changing the `Default` of a type saved inside a list changes what files
//! already on disk mean, as it does for any component field left out of them.

use std::any::TypeId;
use std::rc::Rc;

use bevy::platform::collections::HashMap;
use bevy::prelude::ReflectDefault;
use bevy::reflect::enums::VariantInfo;
use bevy::reflect::{TypeInfo, TypeRegistry};

use super::{BsnPatch, BsnStructFields, BsnValue, SceneBsnAst};

/// Put `ast` in the form live values take: every float of a registered `f32`
/// field held as its shortest text, and every list element without the fields
/// equal to their type's default. Reading the result gives the same values.
pub fn compact_document(ast: &mut SceneBsnAst, registry: &TypeRegistry) {
    let mut defaults = TypeDefaults::new(registry);
    for mut patch in ast.world.query::<&mut BsnPatch>().iter_mut(&mut ast.world) {
        match patch.as_mut() {
            BsnPatch::Struct(data) => {
                let info = registered_info(&data.type_path, registry);
                for field in &mut data.fields.0 {
                    let declared = field_info(info, &data.type_path, &field.name);
                    shorten_f32s(&mut field.value, declared, registry);
                    compact_lists_in(&mut field.value, &mut defaults);
                }
            }
            BsnPatch::TupleStruct(data) => {
                let info = registered_info(&data.type_path, registry);
                for (index, value) in data.values.iter_mut().enumerate() {
                    let declared = position_info(info, &data.type_path, index);
                    shorten_f32s(value, declared, registry);
                    compact_lists_in(value, &mut defaults);
                }
            }
            BsnPatch::Template(type_path, Some(fields)) => {
                let info = registered_info(type_path, registry);
                for field in &mut fields.0 {
                    let declared = field_info(info, type_path, &field.name);
                    shorten_f32s(&mut field.value, declared, registry);
                    compact_lists_in(&mut field.value, &mut defaults);
                }
            }
            _ => {}
        }
    }
}

/// [`compact_document`] for one whole value outside a list.
pub(crate) fn compact_value(value: &mut BsnValue, registry: &TypeRegistry) {
    shorten_f32s(value, None, registry);
    compact_lists_in(value, &mut TypeDefaults::new(registry));
}

/// The registered type `type_path` names, or the enum it spells a variant of.
fn registered_info(type_path: &str, registry: &TypeRegistry) -> Option<&'static TypeInfo> {
    let info = |path: &str| {
        registry
            .get_with_type_path(path)
            .map(bevy::reflect::TypeRegistration::type_info)
    };
    info(type_path).or_else(|| {
        let (enum_path, _) = type_path.rsplit_once("::")?;
        info(enum_path).filter(|enclosing| matches!(enclosing, TypeInfo::Enum(_)))
    })
}

/// Hold each float `declared` to be an `f32` as its shortest text, reading the
/// type below from the type paths `value` spells where `declared` says nothing.
fn shorten_f32s(
    value: &mut BsnValue,
    declared: Option<&'static TypeInfo>,
    registry: &TypeRegistry,
) {
    let spelled = |type_path: &str| {
        declared
            .filter(|info| info.type_path() == type_path)
            .or_else(|| registered_info(type_path, registry))
            .or(declared)
    };
    match value {
        BsnValue::Float(held) => {
            if declared.is_some_and(|info| info.type_id() == TypeId::of::<f32>()) {
                let narrowed = *held as f32;
                let widened = f64::from(narrowed) == *held;
                if widened && narrowed.is_finite() {
                    *held = shortest_f32(narrowed);
                }
            }
        }
        BsnValue::Struct(data) => {
            let info = spelled(&data.type_path);
            for field in &mut data.fields.0 {
                let below = field_info(info, &data.type_path, &field.name);
                shorten_f32s(&mut field.value, below, registry);
            }
        }
        BsnValue::TupleStruct(data) => {
            let info = spelled(&data.type_path);
            for (index, item) in data.values.iter_mut().enumerate() {
                let below = position_info(info, &data.type_path, index);
                shorten_f32s(item, below, registry);
            }
        }
        BsnValue::List(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                shorten_f32s(item, element_info(declared, index), registry);
            }
        }
        BsnValue::Map(entries) => {
            let below = match declared {
                Some(TypeInfo::Map(info)) => info.value_info(),
                _ => None,
            };
            for (_, item) in entries {
                shorten_f32s(item, below, registry);
            }
        }
        _ => {}
    }
}

/// The variant of an enum a value spells by its last path segment.
fn variant_of<'a>(info: &'a TypeInfo, type_path: &str) -> Option<&'a VariantInfo> {
    let TypeInfo::Enum(info) = info else {
        return None;
    };
    info.variant(type_path.rsplit("::").next()?)
}

fn field_info(
    info: Option<&'static TypeInfo>,
    type_path: &str,
    name: &str,
) -> Option<&'static TypeInfo> {
    let info = info?;
    match info {
        TypeInfo::Struct(info) => info.field(name)?.type_info(),
        _ => match variant_of(info, type_path)? {
            VariantInfo::Struct(variant) => variant.field(name)?.type_info(),
            _ => None,
        },
    }
}

fn position_info(
    info: Option<&'static TypeInfo>,
    type_path: &str,
    index: usize,
) -> Option<&'static TypeInfo> {
    let info = info?;
    match info {
        TypeInfo::TupleStruct(info) => info.field_at(index)?.type_info(),
        _ => match variant_of(info, type_path)? {
            VariantInfo::Tuple(variant) => variant.field_at(index)?.type_info(),
            _ => None,
        },
    }
}

/// The type at `index` of a value a document holds as a list.
fn element_info(info: Option<&'static TypeInfo>, index: usize) -> Option<&'static TypeInfo> {
    match info? {
        TypeInfo::List(info) => info.item_info(),
        TypeInfo::Array(info) => info.item_info(),
        TypeInfo::Tuple(info) => info.field_at(index)?.type_info(),
        _ => None,
    }
}

/// Each struct type's default as document fields, computed once per pass.
pub struct TypeDefaults<'a> {
    registry: &'a TypeRegistry,
    by_type: HashMap<String, Option<Rc<BsnStructFields>>>,
}

impl<'a> TypeDefaults<'a> {
    pub fn new(registry: &'a TypeRegistry) -> Self {
        Self {
            registry,
            by_type: HashMap::default(),
        }
    }

    fn fields_of(&mut self, type_path: &str) -> Option<Rc<BsnStructFields>> {
        if let Some(known) = self.by_type.get(type_path) {
            return known.clone();
        }
        // A default can hold its own type. Below itself it counts as having
        // none, which only leaves those fields written.
        self.by_type.insert(type_path.to_string(), None);
        let fields = self.default_fields(type_path).map(Rc::new);
        self.by_type.insert(type_path.to_string(), fields.clone());
        fields
    }

    /// The fields of a type's default, in the form a list element holds them.
    fn default_fields(&mut self, type_path: &str) -> Option<BsnStructFields> {
        let registration = self.registry.get_with_type_path(type_path)?;
        if !matches!(registration.type_info(), TypeInfo::Struct(_)) {
            return None;
        }
        let default = registration.data::<ReflectDefault>()?.default();
        let encoded =
            BsnValue::from_reflect_inner(default.as_partial_reflect(), self.registry, None).ok()?;
        let BsnValue::Struct(mut data) = encoded else {
            return None;
        };
        for field in &mut data.fields.0 {
            compact_list_element(&mut field.value, self);
        }
        Some(data.fields)
    }
}

/// Compact the list elements held anywhere in `value`, which is itself outside
/// a list.
pub(crate) fn compact_lists_in(value: &mut BsnValue, defaults: &mut TypeDefaults) {
    match value {
        BsnValue::Struct(data) => {
            for field in &mut data.fields.0 {
                compact_lists_in(&mut field.value, defaults);
            }
        }
        BsnValue::TupleStruct(data) => {
            for item in &mut data.values {
                compact_lists_in(item, defaults);
            }
        }
        BsnValue::List(items) => {
            for item in items {
                compact_list_element(item, defaults);
            }
        }
        BsnValue::Map(entries) => {
            for (_, item) in entries {
                compact_lists_in(item, defaults);
            }
        }
        _ => {}
    }
}

/// Leave out of `value`, part of a list element, the fields its type's default
/// already holds. A struct with nothing left is written as its bare type, which
/// reads back as the default.
pub(crate) fn compact_list_element(value: &mut BsnValue, defaults: &mut TypeDefaults) {
    match value {
        BsnValue::Struct(data) => {
            for field in &mut data.fields.0 {
                compact_list_element(&mut field.value, defaults);
            }
            let Some(type_defaults) = defaults.fields_of(&data.type_path) else {
                return;
            };
            data.fields.0.retain(|field| {
                !type_defaults
                    .0
                    .iter()
                    .find(|other| other.name == field.name)
                    .is_some_and(|other| same_value_and_sign(&field.value, &other.value))
            });
            if data.fields.0.is_empty() {
                *value = BsnValue::Type(std::mem::take(&mut data.type_path));
            }
        }
        BsnValue::TupleStruct(data) => {
            for item in &mut data.values {
                compact_list_element(item, defaults);
            }
        }
        BsnValue::List(items) => {
            for item in items {
                compact_list_element(item, defaults);
            }
        }
        BsnValue::Map(entries) => {
            for (_, item) in entries {
                compact_list_element(item, defaults);
            }
        }
        _ => {}
    }
}

/// Equal values whose floats also agree in sign, so `-0.0` is never read back
/// as the `0.0` a default holds.
fn same_value_and_sign(a: &BsnValue, b: &BsnValue) -> bool {
    match (a, b) {
        (BsnValue::Float(x), BsnValue::Float(y)) => {
            x.is_sign_negative() == y.is_sign_negative() && x == y
        }
        (BsnValue::Struct(x), BsnValue::Struct(y)) => {
            x.type_path == y.type_path
                && x.fields.0.len() == y.fields.0.len()
                && x.fields.0.iter().all(|field| {
                    y.fields
                        .0
                        .iter()
                        .find(|other| other.name == field.name)
                        .is_some_and(|other| same_value_and_sign(&field.value, &other.value))
                })
        }
        (BsnValue::TupleStruct(x), BsnValue::TupleStruct(y)) => {
            x.type_path == y.type_path && all_same_value_and_sign(&x.values, &y.values)
        }
        (BsnValue::List(x), BsnValue::List(y)) => all_same_value_and_sign(x, y),
        (BsnValue::Map(x), BsnValue::Map(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, value)| {
                    y.iter().any(|(other_key, other_value)| {
                        key == other_key && same_value_and_sign(value, other_value)
                    })
                })
        }
        _ => a == b,
    }
}

fn all_same_value_and_sign(a: &[BsnValue], b: &[BsnValue]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value_and_sign(x, y))
}

impl BsnValue {
    /// A value of an `f32` field, in the form live values take.
    pub fn from_f32(value: f32) -> Self {
        Self::Float(shortest_f32(value))
    }
}

/// The `f64` whose shortest text is the shortest text of `value`, so an `f32`
/// field writes `0.1` rather than `0.10000000149011612`. Reading it back into
/// an `f32` gives the same bits.
pub(crate) fn shortest_f32(value: f32) -> f64 {
    const EXACT_INTEGERS: f32 = 16_777_216.0;
    if !value.is_finite() || (value.fract() == 0.0 && value.abs() < EXACT_INTEGERS) {
        return f64::from(value);
    }
    format!("{value}")
        .parse::<f64>()
        .ok()
        .filter(|short| *short as f32 == value)
        .unwrap_or(f64::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_text_reads_back_as_the_same_bits() {
        for value in [
            0.1_f32,
            -0.0,
            1.0e30,
            f32::MAX,
            f32::MIN_POSITIVE,
            1.0e-45,
            16_777_217.0,
            0.333_333_34,
        ] {
            let short = shortest_f32(value);
            assert_eq!((short as f32).to_bits(), value.to_bits(), "{value}");
        }
    }
}
