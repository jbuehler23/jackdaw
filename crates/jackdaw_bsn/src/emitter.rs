//! BSN text emitter: document AST to `.bsn` text.
//!
//! Pretty-prints a [`SceneBsnAst`] to BSN text compatible with the parser in
//! [`crate::parse`]. Emission order is fully determined by the document's
//! `Vec<Entity>` fields (`SceneBsnAst::roots`, `BsnPatches`, `BsnStructFields`,
//! `Children` lists): every emit function walks those vectors in their stored
//! order and never consults `ecs_to_ast`/`ast_to_ecs`, so emitting the same
//! document twice yields byte-identical text.

use std::fmt::Write;

use bevy::prelude::Entity;

use crate::{BsnField, BsnPatch, BsnStructData, BsnTupleStructData, BsnValue, SceneBsnAst};

/// Emits a complete `.bsn` file from the document AST.
///
/// One root emits its patches directly; multiple roots are wrapped in a
/// `Children [...]` relation so the result re-parses as a single top-level
/// entity.
pub fn emit_scene(ast: &SceneBsnAst) -> String {
    let mut out = String::new();

    if ast.roots.len() <= 1 {
        for &root in &ast.roots {
            // A single root whose only patch is Children re-parses identically
            // to the multi-root wrapper below and would be unwrapped, dropping
            // this grouping entity. Tag it so the loader keeps it.
            if root_has_only_children(ast, root) {
                writeln!(out, "{}", crate::loader::SCENE_ROOT_GROUP_MARKER).unwrap();
            }
            emit_patches(ast, root, 0, &mut out);
        }
    } else {
        writeln!(out, "bevy_ecs::hierarchy::Children [").unwrap();
        for (i, &root) in ast.roots.iter().enumerate() {
            emit_patches(ast, root, 1, &mut out);
            if i + 1 < ast.roots.len() {
                write_indent(1, &mut out);
                out.push_str(",\n");
            }
        }
        writeln!(out, "]").unwrap();
    }

    out
}

/// True when `root`'s sole patch is a `Children` relation, the shape that
/// collides with the synthetic multi-root wrapper on re-parse.
fn root_has_only_children(ast: &SceneBsnAst, root: Entity) -> bool {
    ast.get_patches(root).is_some_and(|p| {
        p.0.len() == 1
            && ast
                .get_patch(p.0[0])
                .is_some_and(|patch| matches!(patch, BsnPatch::Children(_)))
    })
}

/// Emits BSN text for a single entity (and its children) from the AST. Used
/// for clipboard copy: the output is valid `bsn!` macro input.
pub fn emit_entity(ast: &SceneBsnAst, patches_entity: Entity) -> String {
    let mut out = String::new();
    emit_patches(ast, patches_entity, 0, &mut out);
    out
}

/// Emits BSN text for multiple entities. A single entity emits directly;
/// multiple entities are wrapped in `Children [...]` like a multi-root scene.
pub fn emit_entities(ast: &SceneBsnAst, entities: &[Entity]) -> String {
    let mut out = String::new();
    if entities.len() <= 1 {
        for &e in entities {
            emit_patches(ast, e, 0, &mut out);
        }
    } else {
        writeln!(out, "bevy_ecs::hierarchy::Children [").unwrap();
        for (i, &e) in entities.iter().enumerate() {
            emit_patches(ast, e, 1, &mut out);
            if i + 1 < entities.len() {
                write_indent(1, &mut out);
                out.push_str(",\n");
            }
        }
        writeln!(out, "]").unwrap();
    }
    out
}

/// Emit all patches for one entity (one "block" in BSN), in the order they
/// are stored in the entity's [`crate::BsnPatches`] list.
fn emit_patches(ast: &SceneBsnAst, patches_entity: Entity, indent: usize, out: &mut String) {
    let Some(patches) = ast.get_patches(patches_entity) else {
        return;
    };

    for &patch_entity in &patches.0 {
        let Some(patch) = ast.get_patch(patch_entity) else {
            continue;
        };

        // Generic type paths cannot round-trip: the grammar has no angle
        // brackets, so emitting one would produce unparseable text. Skip the
        // patch and say so rather than corrupting the document.
        let generic_type_path = match patch {
            BsnPatch::Type(tp) | BsnPatch::Template(tp, _) => Some(tp),
            BsnPatch::Struct(data) => Some(&data.type_path),
            BsnPatch::TupleStruct(data) => Some(&data.type_path),
            _ => None,
        }
        .filter(|tp| tp.contains('<'));
        if let Some(type_path) = generic_type_path {
            log::warn!("skipping '{type_path}': generic type paths cannot be emitted as BSN");
            continue;
        }

        match patch {
            BsnPatch::Name(name) => {
                write_indent(indent, out);
                if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
                    writeln!(out, "#{name}").unwrap();
                } else {
                    writeln!(out, "#\"{}\"", escape_string(name)).unwrap();
                }
            }

            BsnPatch::Base(path) => {
                write_indent(indent, out);
                writeln!(out, ":\"{}\"", escape_string(path)).unwrap();
            }

            BsnPatch::Type(type_path) => {
                write_indent(indent, out);
                writeln!(out, "{type_path}").unwrap();
            }

            BsnPatch::Struct(data) => {
                emit_struct_patch(data, indent, out);
            }

            BsnPatch::TupleStruct(data) => {
                emit_tuple_struct_patch(data, indent, out);
            }

            BsnPatch::Template(type_path, fields) => {
                write_indent(indent, out);
                if let Some(fields) = fields {
                    if fields.0.is_empty() {
                        writeln!(out, "@{type_path}").unwrap();
                    } else {
                        writeln!(out, "@{type_path} {{").unwrap();
                        emit_fields(&fields.0, indent + 1, out);
                        write_indent(indent, out);
                        writeln!(out, "}}").unwrap();
                    }
                } else {
                    writeln!(out, "@{type_path}").unwrap();
                }
            }

            BsnPatch::Children(children) => {
                write_indent(indent, out);
                if children.is_empty() {
                    writeln!(out, "bevy_ecs::hierarchy::Children []").unwrap();
                } else {
                    writeln!(out, "bevy_ecs::hierarchy::Children [").unwrap();
                    for (i, &child) in children.iter().enumerate() {
                        emit_patches(ast, child, indent + 1, out);
                        if i + 1 < children.len() {
                            write_indent(indent + 1, out);
                            out.push_str(",\n");
                        }
                    }
                    write_indent(indent, out);
                    writeln!(out, "]").unwrap();
                }
            }
        }
    }
}

fn emit_struct_patch(data: &BsnStructData, indent: usize, out: &mut String) {
    write_indent(indent, out);
    if data.fields.0.is_empty() {
        writeln!(out, "{}", data.type_path).unwrap();
    } else {
        writeln!(out, "{} {{", data.type_path).unwrap();
        emit_fields(&data.fields.0, indent + 1, out);
        write_indent(indent, out);
        writeln!(out, "}}").unwrap();
    }
}

fn emit_tuple_struct_patch(data: &BsnTupleStructData, indent: usize, out: &mut String) {
    write_indent(indent, out);
    write!(out, "{}(", data.type_path).unwrap();
    for (i, value) in data.values.iter().enumerate() {
        if i > 0 {
            write!(out, ", ").unwrap();
        }
        emit_value(value, out);
    }
    writeln!(out, ")").unwrap();
}

fn emit_fields(fields: &[BsnField], indent: usize, out: &mut String) {
    for field in fields {
        write_indent(indent, out);
        write!(out, "{}: ", field.name).unwrap();
        emit_value_maybe_multiline(&field.value, indent, out);
        writeln!(out, ",").unwrap();
    }
}

fn emit_value(value: &BsnValue, out: &mut String) {
    match value {
        BsnValue::Float(f) => {
            // Always emit at least one decimal place so the value re-parses
            // as a float rather than an int.
            if f.fract() == 0.0 {
                write!(out, "{f:.1}").unwrap();
            } else {
                write!(out, "{f}").unwrap();
            }
        }
        BsnValue::Int(i) => write!(out, "{i}").unwrap(),
        BsnValue::Bool(b) => write!(out, "{b}").unwrap(),
        BsnValue::String(s) => write!(out, "\"{}\"", escape_string(s)).unwrap(),
        BsnValue::Type(tp) => write!(out, "{tp}").unwrap(),
        BsnValue::Struct(data) => {
            if data.fields.0.is_empty() {
                write!(out, "{}", data.type_path).unwrap();
            } else {
                write!(out, "{} {{ ", data.type_path).unwrap();
                for (i, field) in data.fields.0.iter().enumerate() {
                    if i > 0 {
                        write!(out, ", ").unwrap();
                    }
                    write!(out, "{}: ", field.name).unwrap();
                    emit_value(&field.value, out);
                }
                write!(out, " }}").unwrap();
            }
        }
        BsnValue::TupleStruct(data) => {
            write!(out, "{}(", data.type_path).unwrap();
            for (i, v) in data.values.iter().enumerate() {
                if i > 0 {
                    write!(out, ", ").unwrap();
                }
                emit_value(v, out);
            }
            write!(out, ")").unwrap();
        }
        BsnValue::List(items) => {
            write!(out, "[").unwrap();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    write!(out, ", ").unwrap();
                }
                emit_value(item, out);
            }
            write!(out, "]").unwrap();
        }
        BsnValue::Map(entries) => {
            write!(out, "map[").unwrap();
            for (i, (key, value)) in sorted_map_entries(entries).iter().enumerate() {
                if i > 0 {
                    write!(out, ", ").unwrap();
                }
                write!(out, "({key}, ").unwrap();
                emit_value(value, out);
                write!(out, ")").unwrap();
            }
            write!(out, "]").unwrap();
        }
    }
}

/// Render each map entry's key to text and return the entries sorted by that
/// key text. Sorting on the emitted key makes map emission deterministic
/// regardless of the source insertion order.
fn sorted_map_entries(entries: &[(BsnValue, BsnValue)]) -> Vec<(String, &BsnValue)> {
    let mut rendered: Vec<(String, &BsnValue)> = entries
        .iter()
        .map(|(key, value)| {
            let mut key_text = String::new();
            emit_value(key, &mut key_text);
            (key_text, value)
        })
        .collect();
    rendered.sort_by(|a, b| a.0.cmp(&b.0));
    rendered
}

/// Emit a value, using multiline format for nested structs and lists.
fn emit_value_maybe_multiline(value: &BsnValue, indent: usize, out: &mut String) {
    match value {
        BsnValue::Struct(data) if !data.fields.0.is_empty() => {
            writeln!(out, "{} {{", data.type_path).unwrap();
            emit_fields(&data.fields.0, indent + 1, out);
            write_indent(indent, out);
            write!(out, "}}").unwrap();
        }
        BsnValue::List(items) if !items.is_empty() => {
            writeln!(out, "[").unwrap();
            for (i, item) in items.iter().enumerate() {
                write_indent(indent + 1, out);
                emit_value_maybe_multiline(item, indent + 1, out);
                if i + 1 < items.len() {
                    writeln!(out, ",").unwrap();
                } else {
                    writeln!(out).unwrap();
                }
            }
            write_indent(indent, out);
            write!(out, "]").unwrap();
        }
        BsnValue::Map(entries) if !entries.is_empty() => {
            writeln!(out, "map[").unwrap();
            let sorted = sorted_map_entries(entries);
            for (i, (key, value)) in sorted.iter().enumerate() {
                write_indent(indent + 1, out);
                write!(out, "({key}, ").unwrap();
                emit_value_maybe_multiline(value, indent + 1, out);
                write!(out, ")").unwrap();
                if i + 1 < sorted.len() {
                    writeln!(out, ",").unwrap();
                } else {
                    writeln!(out).unwrap();
                }
            }
            write_indent(indent, out);
            write!(out, "]").unwrap();
        }
        _ => emit_value(value, out),
    }
}

fn write_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("    ");
    }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BsnPatches, BsnStructFields};

    #[test]
    fn emit_simple_entity() {
        let mut ast = SceneBsnAst::default();

        let name_patch = ast.world.spawn(BsnPatch::Name("Root".into())).id();
        let transform_patch = ast
            .world
            .spawn(BsnPatch::Type(
                "bevy_transform::components::transform::Transform".into(),
            ))
            .id();
        let vis_patch = ast
            .world
            .spawn(BsnPatch::Type(
                "bevy_camera::visibility::Visibility::Visible".into(),
            ))
            .id();

        let patches_entity = ast
            .world
            .spawn(BsnPatches(vec![name_patch, transform_patch, vis_patch]))
            .id();
        ast.roots.push(patches_entity);

        let text = emit_scene(&ast);
        assert!(text.contains("#Root"));
        assert!(text.contains("bevy_transform::components::transform::Transform"));
        assert!(text.contains("bevy_camera::visibility::Visibility::Visible"));
    }

    #[test]
    fn emit_struct_with_fields() {
        let mut ast = SceneBsnAst::default();

        let patch = ast
            .world
            .spawn(BsnPatch::Struct(BsnStructData {
                type_path: "bevy_light::directional_light::DirectionalLight".into(),
                fields: BsnStructFields(vec![BsnField {
                    name: "shadow_maps_enabled".into(),
                    value: BsnValue::Bool(true),
                }]),
            }))
            .id();

        let entity = ast.world.spawn(BsnPatches(vec![patch])).id();
        ast.roots.push(entity);

        let text = emit_scene(&ast);
        assert!(text.contains("DirectionalLight {"));
        assert!(text.contains("shadow_maps_enabled: true,"));
    }

    #[test]
    fn emit_children() {
        let mut ast = SceneBsnAst::default();

        let child_name = ast.world.spawn(BsnPatch::Name("Child".into())).id();
        let child = ast.world.spawn(BsnPatches(vec![child_name])).id();

        let root_name = ast.world.spawn(BsnPatch::Name("Root".into())).id();
        let children_patch = ast.world.spawn(BsnPatch::Children(vec![child])).id();
        let root = ast
            .world
            .spawn(BsnPatches(vec![root_name, children_patch]))
            .id();
        ast.roots.push(root);

        let text = emit_scene(&ast);
        assert!(text.contains("#Root"));
        assert!(text.contains("bevy_ecs::hierarchy::Children ["));
        assert!(text.contains("    #Child"));
        assert!(text.contains("]"));
    }

    #[test]
    fn emit_tuple_struct() {
        let mut ast = SceneBsnAst::default();

        let patch = ast
            .world
            .spawn(BsnPatch::TupleStruct(BsnTupleStructData {
                type_path: "bevy_scene::components::SceneRoot".into(),
                values: vec![BsnValue::String(
                    "models/FlightHelmet/FlightHelmet.gltf#Scene0".into(),
                )],
            }))
            .id();

        let entity = ast.world.spawn(BsnPatches(vec![patch])).id();
        ast.roots.push(entity);

        let text = emit_scene(&ast);
        assert!(text.contains("SceneRoot(\"models/FlightHelmet/FlightHelmet.gltf#Scene0\")"));
    }

    #[test]
    fn emit_is_deterministic_for_multi_field_document() {
        let mut ast = SceneBsnAst::default();

        let patch = ast
            .world
            .spawn(BsnPatch::Struct(BsnStructData {
                type_path: "test::Widget".into(),
                fields: BsnStructFields(vec![
                    BsnField {
                        name: "third".into(),
                        value: BsnValue::Int(3),
                    },
                    BsnField {
                        name: "first".into(),
                        value: BsnValue::Int(1),
                    },
                    BsnField {
                        name: "second".into(),
                        value: BsnValue::Int(2),
                    },
                ]),
            }))
            .id();
        let entity = ast.world.spawn(BsnPatches(vec![patch])).id();
        ast.roots.push(entity);

        let expected = "test::Widget {\n    third: 3,\n    first: 1,\n    second: 2,\n}\n";
        let first = emit_scene(&ast);
        let second = emit_scene(&ast);
        assert_eq!(first, expected);
        assert_eq!(first, second);
    }
}
