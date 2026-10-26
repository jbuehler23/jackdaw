//! Serde's data model spelled as document values.
//!
//! Bevy reflects some types as opaque: a `Duration`, an `Entity`, a `char`, a
//! `u128`. The reflect walk sees nothing inside them, but their `Serialize`
//! and `Deserialize` impls do, so the document holds what a value serializes
//! as and reads it back through its own `Deserialize`.
//!
//! A struct is written under its serde name (`Duration { secs: 1, nanos: 0 }`),
//! an enum variant as `Name::Variant`, a sequence or tuple as a list, a map as
//! a map, `None` as `None`, and `Some(v)` as `Some(v)`.

use std::fmt::Display;

use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};
use serde::ser::{self, Serialize};

use super::{BsnField, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue};

/// Why a value did not go to or come back from its document form.
#[derive(Debug)]
pub(crate) struct ValueError(String);

impl Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

impl ser::Error for ValueError {
    fn custom<T: Display>(msg: T) -> Self {
        ValueError(msg.to_string())
    }
}

impl de::Error for ValueError {
    fn custom<T: Display>(msg: T) -> Self {
        ValueError(msg.to_string())
    }
}

/// Serializes a value into the [`BsnValue`] the document stores.
pub(crate) struct ValueSerializer;

pub(crate) struct SeqSerializer(Vec<BsnValue>);

pub(crate) struct TupleStructSerializer {
    type_path: String,
    values: Vec<BsnValue>,
}

pub(crate) struct StructSerializer {
    type_path: String,
    fields: Vec<BsnField>,
}

pub(crate) struct MapSerializer {
    entries: Vec<(BsnValue, BsnValue)>,
    key: Option<BsnValue>,
}

fn variant_path(name: &str, variant: &str) -> String {
    format!("{name}::{variant}")
}

impl ser::Serializer for ValueSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    type SerializeSeq = SeqSerializer;
    type SerializeTuple = SeqSerializer;
    type SerializeTupleStruct = TupleStructSerializer;
    type SerializeTupleVariant = TupleStructSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = StructSerializer;
    type SerializeStructVariant = StructSerializer;

    fn serialize_bool(self, v: bool) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_i16(self, v: i16) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_i32(self, v: i32) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_i64(self, v: i64) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_i128(self, v: i128) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_u16(self, v: u16) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_u32(self, v: u32) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    fn serialize_u64(self, v: u64) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Int(v.into()))
    }
    /// A `u128` past what the document's integers hold is written as its
    /// decimal digits, which [`ValueDeserializer`] reads back.
    fn serialize_u128(self, v: u128) -> Result<BsnValue, ValueError> {
        Ok(i128::try_from(v).map_or_else(|_| BsnValue::String(v.to_string()), BsnValue::Int))
    }
    fn serialize_f32(self, v: f32) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Float(v.into()))
    }
    fn serialize_f64(self, v: f64) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::String(v.to_owned()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::List(
            v.iter().map(|byte| BsnValue::Int((*byte).into())).collect(),
        ))
    }
    fn serialize_none(self) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Type("None".to_string()))
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::TupleStruct(BsnTupleStructData {
            type_path: "Some".to_string(),
            values: vec![value.serialize(ValueSerializer)?],
        }))
    }
    fn serialize_unit(self) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::List(Vec::new()))
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Type(name.to_string()))
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Type(variant_path(name, variant)))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<BsnValue, ValueError> {
        value.serialize(ValueSerializer)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::TupleStruct(BsnTupleStructData {
            type_path: variant_path(name, variant),
            values: vec![value.serialize(ValueSerializer)?],
        }))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSerializer, ValueError> {
        Ok(SeqSerializer(Vec::with_capacity(len.unwrap_or_default())))
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSerializer, ValueError> {
        Ok(SeqSerializer(Vec::with_capacity(len)))
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<TupleStructSerializer, ValueError> {
        Ok(TupleStructSerializer {
            type_path: name.to_string(),
            values: Vec::with_capacity(len),
        })
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleStructSerializer, ValueError> {
        Ok(TupleStructSerializer {
            type_path: variant_path(name, variant),
            values: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapSerializer, ValueError> {
        Ok(MapSerializer {
            entries: Vec::with_capacity(len.unwrap_or_default()),
            key: None,
        })
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<StructSerializer, ValueError> {
        Ok(StructSerializer {
            type_path: name.to_string(),
            fields: Vec::with_capacity(len),
        })
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructSerializer, ValueError> {
        Ok(StructSerializer {
            type_path: variant_path(name, variant),
            fields: Vec::with_capacity(len),
        })
    }
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), ValueError> {
        self.0.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::List(self.0))
    }
}

impl ser::SerializeTuple for SeqSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), ValueError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for TupleStructSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), ValueError> {
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::TupleStruct(BsnTupleStructData {
            type_path: self.type_path,
            values: self.values,
        }))
    }
}

impl ser::SerializeTupleVariant for TupleStructSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), ValueError> {
        ser::SerializeTupleStruct::serialize_field(self, value)
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        ser::SerializeTupleStruct::end(self)
    }
}

impl ser::SerializeStruct for StructSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), ValueError> {
        self.fields.push(BsnField {
            name: key.to_string(),
            value: value.serialize(ValueSerializer)?,
        });
        Ok(())
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Struct(BsnStructData {
            type_path: self.type_path,
            fields: BsnStructFields(self.fields),
        }))
    }
}

impl ser::SerializeStructVariant for StructSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), ValueError> {
        ser::SerializeStruct::serialize_field(self, key, value)
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        ser::SerializeStruct::end(self)
    }
}

impl ser::SerializeMap for MapSerializer {
    type Ok = BsnValue;
    type Error = ValueError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), ValueError> {
        self.key = Some(key.serialize(ValueSerializer)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), ValueError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| ValueError("a map value came without its key".to_string()))?;
        self.entries.push((key, value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<BsnValue, ValueError> {
        Ok(BsnValue::Map(self.entries))
    }
}

/// Reads a document value back through a type's `Deserialize` impl.
pub(crate) struct ValueDeserializer<'a>(pub &'a BsnValue);

/// The last segment of `Name::Variant`.
fn variant_of(type_path: &str) -> &str {
    type_path.rsplit("::").next().unwrap_or(type_path)
}

fn is_some(data: &BsnTupleStructData) -> bool {
    data.type_path == "Some" && data.values.len() == 1
}

impl<'de> de::Deserializer<'de> for ValueDeserializer<'_> {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self.0 {
            BsnValue::Float(f) => visitor.visit_f64(*f),
            BsnValue::Int(i) => match (u64::try_from(*i), i64::try_from(*i)) {
                (Ok(unsigned), _) => visitor.visit_u64(unsigned),
                (_, Ok(signed)) => visitor.visit_i64(signed),
                _ => visitor.visit_i128(*i),
            },
            BsnValue::Bool(b) => visitor.visit_bool(*b),
            BsnValue::String(s) => visitor.visit_str(s),
            BsnValue::Type(path) if path == "None" => visitor.visit_none(),
            BsnValue::Type(path) => visitor.visit_str(variant_of(path)),
            BsnValue::TupleStruct(data) if is_some(data) => {
                visitor.visit_some(ValueDeserializer(&data.values[0]))
            }
            BsnValue::TupleStruct(data) => visitor.visit_seq(Seq(data.values.iter())),
            BsnValue::List(items) => visitor.visit_seq(Seq(items.iter())),
            BsnValue::Struct(data) => visitor.visit_map(Fields {
                fields: data.fields.0.iter(),
                value: None,
            }),
            BsnValue::Map(entries) => visitor.visit_map(Entries {
                entries: entries.iter(),
                value: None,
            }),
        }
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self.0 {
            BsnValue::Int(i) => visitor.visit_i128(*i),
            BsnValue::String(s) => visitor.visit_i128(s.parse().map_err(de::Error::custom)?),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self.0 {
            BsnValue::Int(i) => visitor.visit_u128(u128::try_from(*i).map_err(de::Error::custom)?),
            BsnValue::String(s) => visitor.visit_u128(s.parse().map_err(de::Error::custom)?),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self.0 {
            BsnValue::Int(i) => visitor.visit_f64(*i as f64),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        if let BsnValue::String(s) = self.0
            && let mut chars = s.chars()
            && let (Some(c), None) = (chars.next(), chars.next())
        {
            return visitor.visit_char(c);
        }
        self.deserialize_any(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self.0 {
            BsnValue::Type(path) if path == "None" => visitor.visit_none(),
            BsnValue::TupleStruct(data) if is_some(data) => {
                visitor.visit_some(ValueDeserializer(&data.values[0]))
            }
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self.0 {
            BsnValue::List(items) if items.is_empty() => visitor.visit_unit(),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        let variant = match self.0 {
            BsnValue::Type(path) => variant_of(path),
            BsnValue::String(name) => name,
            BsnValue::TupleStruct(data) => variant_of(&data.type_path),
            BsnValue::Struct(data) => variant_of(&data.type_path),
            _ => return self.deserialize_any(visitor),
        };
        visitor.visit_enum(Variant {
            name: variant,
            value: self.0,
        })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 str string bytes byte_buf unit_struct
        seq tuple tuple_struct map struct identifier ignored_any
    }
}

struct Seq<'a>(std::slice::Iter<'a, BsnValue>);

impl<'de> de::SeqAccess<'de> for Seq<'_> {
    type Error = ValueError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, ValueError> {
        self.0
            .next()
            .map(|value| seed.deserialize(ValueDeserializer(value)))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

struct Fields<'a> {
    fields: std::slice::Iter<'a, BsnField>,
    value: Option<&'a BsnValue>,
}

impl<'de> de::MapAccess<'de> for Fields<'_> {
    type Error = ValueError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, ValueError> {
        let Some(field) = self.fields.next() else {
            return Ok(None);
        };
        self.value = Some(&field.value);
        seed.deserialize(field.name.as_str().into_deserializer())
            .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, ValueError> {
        let value = self
            .value
            .take()
            .ok_or_else(|| ValueError("a field value came without its name".to_string()))?;
        seed.deserialize(ValueDeserializer(value))
    }
}

struct Entries<'a> {
    entries: std::slice::Iter<'a, (BsnValue, BsnValue)>,
    value: Option<&'a BsnValue>,
}

impl<'de> de::MapAccess<'de> for Entries<'_> {
    type Error = ValueError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, ValueError> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.value = Some(value);
        seed.deserialize(ValueDeserializer(key)).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, ValueError> {
        let value = self
            .value
            .take()
            .ok_or_else(|| ValueError("a map value came without its key".to_string()))?;
        seed.deserialize(ValueDeserializer(value))
    }
}

/// One enum value: the variant's name and the value that carries its data.
struct Variant<'a> {
    name: &'a str,
    value: &'a BsnValue,
}

impl<'de> de::EnumAccess<'de> for Variant<'_> {
    type Error = ValueError;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self), ValueError> {
        let name = seed.deserialize(self.name.into_deserializer())?;
        Ok((name, self))
    }
}

impl<'de> de::VariantAccess<'de> for Variant<'_> {
    type Error = ValueError;

    fn unit_variant(self) -> Result<(), ValueError> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, ValueError> {
        match self.value {
            BsnValue::TupleStruct(data) if data.values.len() == 1 => {
                seed.deserialize(ValueDeserializer(&data.values[0]))
            }
            other => Err(ValueError(format!(
                "variant '{}' carries one value, not {other:?}",
                self.name
            ))),
        }
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        match self.value {
            BsnValue::TupleStruct(data) => visitor.visit_seq(Seq(data.values.iter())),
            other => Err(ValueError(format!(
                "variant '{}' carries a tuple, not {other:?}",
                self.name
            ))),
        }
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ValueError> {
        match self.value {
            BsnValue::Struct(data) => visitor.visit_map(Fields {
                fields: data.fields.0.iter(),
                value: None,
            }),
            other => Err(ValueError(format!(
                "variant '{}' carries named fields, not {other:?}",
                self.name
            ))),
        }
    }
}
