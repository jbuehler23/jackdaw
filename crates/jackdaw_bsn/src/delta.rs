//! Sparse override merge and diff over [`BsnValue`] documents.
//!
//! These mirror the JSON-value override helpers the prefab resolver uses on the
//! `.jsn` side, reproduced here on the BSN value tree. The struct variant is the
//! map-like value that carries named fields, so it is the only shape that merges
//! and diffs recursively; every other shape (tuple struct, list, map, scalar) is
//! replaced whole. [`apply_deltas`] and [`shallow_diff`] round-trip: for struct
//! values, applying the diff of `base` against `current` onto a clone of `base`
//! reconstructs `current`.

use crate::document::{BsnField, BsnStructData, BsnStructFields, BsnValue};

impl PartialEq for BsnValue {
    /// Structural equality: struct fields and map entries compare independent
    /// of order (matching how a JSON object compares), so two structs with
    /// the same named fields in a different order are equal. Lists and
    /// tuple-struct values compare in order.
    fn eq(&self, other: &Self) -> bool {
        bsn_value_eq(self, other)
    }
}

/// Structural equality of two [`BsnValue`] trees; the body behind
/// [`BsnValue`]'s `PartialEq`. Prefer `==` at call sites.
pub fn bsn_value_eq(a: &BsnValue, b: &BsnValue) -> bool {
    match (a, b) {
        (BsnValue::Float(x), BsnValue::Float(y)) => x == y,
        (BsnValue::Int(x), BsnValue::Int(y)) => x == y,
        (BsnValue::Bool(x), BsnValue::Bool(y)) => x == y,
        (BsnValue::String(x), BsnValue::String(y)) | (BsnValue::Type(x), BsnValue::Type(y)) => {
            x == y
        }
        (BsnValue::Struct(x), BsnValue::Struct(y)) => {
            x.type_path == y.type_path && struct_fields_eq(&x.fields, &y.fields)
        }
        (BsnValue::TupleStruct(x), BsnValue::TupleStruct(y)) => {
            x.type_path == y.type_path
                && x.values.len() == y.values.len()
                && x.values
                    .iter()
                    .zip(&y.values)
                    .all(|(xv, yv)| bsn_value_eq(xv, yv))
        }
        (BsnValue::List(x), BsnValue::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(xv, yv)| bsn_value_eq(xv, yv))
        }
        (BsnValue::Map(x), BsnValue::Map(y)) => {
            x.len() == y.len()
                && x.iter().all(|(xk, xv)| {
                    y.iter()
                        .any(|(yk, yv)| bsn_value_eq(xk, yk) && bsn_value_eq(xv, yv))
                })
        }
        _ => false,
    }
}

/// Order-independent equality of two named-field lists: the same set of field
/// names, each with an equal value.
fn struct_fields_eq(a: &BsnStructFields, b: &BsnStructFields) -> bool {
    if a.0.len() != b.0.len() {
        return false;
    }
    a.0.iter().all(|af| {
        b.0.iter()
            .find(|bf| bf.name == af.name)
            .is_some_and(|bf| bsn_value_eq(&af.value, &bf.value))
    })
}

/// Overlay `delta` onto `base` in place. When both are structs, each field
/// present in `delta` is merged into `base`: matching struct fields recurse,
/// any other matching field is replaced, and a field absent from `base` is
/// appended. Fields of `base` that `delta` does not mention are left untouched.
/// When either side is not a struct, `base` is replaced wholesale by a clone of
/// `delta`.
pub fn apply_deltas(base: &mut BsnValue, delta: &BsnValue) {
    let (BsnValue::Struct(base_data), BsnValue::Struct(delta_data)) = (&mut *base, delta) else {
        *base = delta.clone();
        return;
    };

    for delta_field in &delta_data.fields.0 {
        match base_data
            .fields
            .0
            .iter_mut()
            .find(|bf| bf.name == delta_field.name)
        {
            Some(base_field) => {
                let both_structs = matches!(
                    (&base_field.value, &delta_field.value),
                    (BsnValue::Struct(_), BsnValue::Struct(_))
                );
                if both_structs {
                    apply_deltas(&mut base_field.value, &delta_field.value);
                } else {
                    base_field.value = delta_field.value.clone();
                }
            }
            None => base_data.fields.0.push(BsnField {
                name: delta_field.name.clone(),
                value: delta_field.value.clone(),
            }),
        }
    }
}

/// The minimal delta such that applying it onto a clone of `base` with
/// [`apply_deltas`] reconstructs `current`. Returns `None` when `base` and
/// `current` are equal. When both are structs, only the fields of `current`
/// that differ from `base` are kept, recursing into nested structs so a single
/// changed leaf yields a narrow nested delta rather than the whole struct. When
/// either side is not a struct, the whole of `current` is the delta.
pub fn shallow_diff(base: &BsnValue, current: &BsnValue) -> Option<BsnValue> {
    if bsn_value_eq(base, current) {
        return None;
    }

    let (BsnValue::Struct(base_data), BsnValue::Struct(current_data)) = (base, current) else {
        return Some(current.clone());
    };

    let mut delta_fields = Vec::new();
    for current_field in &current_data.fields.0 {
        match base_data
            .fields
            .0
            .iter()
            .find(|bf| bf.name == current_field.name)
        {
            Some(base_field) if bsn_value_eq(&base_field.value, &current_field.value) => {}
            Some(base_field) => {
                if let Some(sub) = shallow_diff(&base_field.value, &current_field.value) {
                    delta_fields.push(BsnField {
                        name: current_field.name.clone(),
                        value: sub,
                    });
                }
            }
            None => delta_fields.push(BsnField {
                name: current_field.name.clone(),
                value: current_field.value.clone(),
            }),
        }
    }

    if delta_fields.is_empty() {
        None
    } else {
        Some(BsnValue::Struct(BsnStructData {
            type_path: current_data.type_path.clone(),
            fields: BsnStructFields(delta_fields),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, value: BsnValue) -> BsnField {
        BsnField {
            name: name.to_string(),
            value,
        }
    }

    fn strukt(type_path: &str, fields: Vec<BsnField>) -> BsnValue {
        BsnValue::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields(fields),
        })
    }

    #[test]
    fn diff_of_equal_structs_is_none() {
        let base = strukt("Transform", vec![field("x", BsnValue::Float(1.0))]);
        let current = strukt("Transform", vec![field("x", BsnValue::Float(1.0))]);
        assert!(shallow_diff(&base, &current).is_none());
    }

    #[test]
    fn diff_keeps_only_changed_leaf_of_nested_struct() {
        let base = strukt(
            "Transform",
            vec![field(
                "translation",
                strukt(
                    "Vec3",
                    vec![
                        field("x", BsnValue::Float(1.0)),
                        field("y", BsnValue::Float(2.0)),
                        field("z", BsnValue::Float(3.0)),
                    ],
                ),
            )],
        );
        let current = strukt(
            "Transform",
            vec![field(
                "translation",
                strukt(
                    "Vec3",
                    vec![
                        field("x", BsnValue::Float(1.0)),
                        field("y", BsnValue::Float(9.0)),
                        field("z", BsnValue::Float(3.0)),
                    ],
                ),
            )],
        );

        let delta = shallow_diff(&base, &current).expect("values differ");
        let expected = strukt(
            "Transform",
            vec![field(
                "translation",
                strukt("Vec3", vec![field("y", BsnValue::Float(9.0))]),
            )],
        );
        assert!(
            bsn_value_eq(&delta, &expected),
            "delta should carry only translation.y"
        );
    }

    #[test]
    fn apply_deltas_inserts_missing_field() {
        let mut base = strukt("Marker", vec![field("a", BsnValue::Int(1))]);
        let delta = strukt("Marker", vec![field("b", BsnValue::Int(2))]);
        apply_deltas(&mut base, &delta);
        let expected = strukt(
            "Marker",
            vec![field("a", BsnValue::Int(1)), field("b", BsnValue::Int(2))],
        );
        assert!(bsn_value_eq(&base, &expected));
    }

    #[test]
    fn apply_deltas_non_struct_replaces_whole() {
        let mut base = BsnValue::Int(1);
        apply_deltas(&mut base, &BsnValue::Int(7));
        assert!(bsn_value_eq(&base, &BsnValue::Int(7)));
    }

    #[test]
    fn nested_struct_round_trips() {
        let base = strukt(
            "Transform",
            vec![
                field(
                    "translation",
                    strukt(
                        "Vec3",
                        vec![
                            field("x", BsnValue::Float(1.0)),
                            field("y", BsnValue::Float(2.0)),
                            field("z", BsnValue::Float(3.0)),
                        ],
                    ),
                ),
                field("scale", BsnValue::Float(1.0)),
            ],
        );
        let current = strukt(
            "Transform",
            vec![
                field(
                    "translation",
                    strukt(
                        "Vec3",
                        vec![
                            field("x", BsnValue::Float(5.0)),
                            field("y", BsnValue::Float(2.0)),
                            field("z", BsnValue::Float(8.0)),
                        ],
                    ),
                ),
                field("scale", BsnValue::Float(2.0)),
            ],
        );

        let delta = shallow_diff(&base, &current).expect("values differ");
        let mut reconstructed = base.clone();
        apply_deltas(&mut reconstructed, &delta);
        assert!(
            bsn_value_eq(&reconstructed, &current),
            "apply_deltas(base, shallow_diff(base, current)) must equal current"
        );
    }

    #[test]
    fn round_trip_over_varied_shapes() {
        // A spread of leaf shapes exercises the whole-replace branches (list,
        // tuple struct, string, bool) alongside the recursive struct branch.
        let base = strukt(
            "Bag",
            vec![
                field("flag", BsnValue::Bool(false)),
                field("label", BsnValue::String("old".into())),
                field(
                    "items",
                    BsnValue::List(vec![BsnValue::Int(1), BsnValue::Int(2)]),
                ),
                field(
                    "id",
                    BsnValue::TupleStruct(crate::document::BsnTupleStructData {
                        type_path: "PrefabEntityId".into(),
                        values: vec![BsnValue::Int(4)],
                    }),
                ),
                field("kept", BsnValue::Float(0.5)),
            ],
        );
        let current = strukt(
            "Bag",
            vec![
                field("flag", BsnValue::Bool(true)),
                field("label", BsnValue::String("new".into())),
                field(
                    "items",
                    BsnValue::List(vec![BsnValue::Int(3), BsnValue::Int(4), BsnValue::Int(5)]),
                ),
                field(
                    "id",
                    BsnValue::TupleStruct(crate::document::BsnTupleStructData {
                        type_path: "PrefabEntityId".into(),
                        values: vec![BsnValue::Int(9)],
                    }),
                ),
                field("kept", BsnValue::Float(0.5)),
            ],
        );

        let delta = shallow_diff(&base, &current).expect("values differ");
        // The unchanged `kept` field must not appear in the delta.
        if let BsnValue::Struct(ref data) = delta {
            assert!(
                !data.fields.0.iter().any(|f| f.name == "kept"),
                "unchanged fields are excluded from the delta"
            );
        } else {
            panic!("struct base yields a struct delta");
        }

        let mut reconstructed = base.clone();
        apply_deltas(&mut reconstructed, &delta);
        assert!(bsn_value_eq(&reconstructed, &current));
    }
}
