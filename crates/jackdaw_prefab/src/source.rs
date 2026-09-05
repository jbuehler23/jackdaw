//! The file an instance points at: where it lives, how a saved document spells
//! it, and what shape it has to be in to be instanced.
//!
//! A source is written relative to the directory of the document that names it,
//! the only spelling both sides can honour. Absolute sources are read, never
//! written.
//!
//! The editor follows a source anywhere on the machine; a game refuses anything
//! outside its asset root, enforced in
//! `jackdaw_runtime::resolve_prefab_references`.

use std::path::{Path, PathBuf};

use jackdaw_bsn::{
    BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue, SceneBsnAst,
};

use crate::components::{ISA_TYPE, PREFAB_ENTITY_ID_TYPE, PREFAB_TYPE};
use crate::resolve::{read_isa_deleted, read_isa_source, set_whole_component};

const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";
const VISIBILITY_INHERITED_TYPE: &str = "bevy_camera::visibility::Visibility::Inherited";

/// The file an `IsA.source` points at, given the directory of the document
/// that names it.
///
/// The result is normalized, so the two documents that reach one prefab by
/// different routes name it identically. Both the cycle detector and the
/// prefab cache compare paths, and neither can see through a stray `..`.
pub fn source_path(source: &Path, document_dir: &Path) -> PathBuf {
    if source.is_absolute() {
        normalize(source)
    } else {
        normalize(&document_dir.join(source))
    }
}

/// Fold away `.` and `..` without touching the filesystem. Purely lexical:
/// the target need not exist yet.
///
/// A caller deciding whether a path is inside a directory has to fold first,
/// or `assets/../../etc` reads as being under `assets`.
pub fn normalize_path(path: &Path) -> PathBuf {
    normalize(path)
}

fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Rewrite every absolute `IsA.source` in `ast` to a path relative to
/// `document_dir`, so the saved document travels.
///
/// Call it on the copy being emitted, never on the live document: the editor
/// holds instance sources as absolute paths. A source that cannot be expressed
/// relative to `document_dir` is left as it stands.
pub fn relativize_isa_sources(ast: &mut SceneBsnAst, document_dir: &Path) {
    if !document_dir.is_absolute() {
        return;
    }
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = read_isa_source(ast, node) else {
            continue;
        };
        if !source.is_absolute() {
            continue;
        }
        let Some(relative) = pathdiff::diff_paths(&source, document_dir) else {
            continue;
        };
        let deleted = read_isa_deleted(ast, node);
        let spelling = relative.to_string_lossy().replace('\\', "/");
        set_whole_component(ast, node, ISA_TYPE, isa_value(&spelling, &deleted));
    }
}

/// Rewrite every relative `IsA.source` in `ast` to the file it names, given the
/// directory `ast` was read from.
///
/// The read half of the pair [`relativize_isa_sources`] writes, so a cache keyed
/// by file can look a prefab up without carrying the document's directory.
///
/// An already-absolute source is folded rather than left alone, so every source
/// afterwards is one a caller can compare.
pub fn absolutize_isa_sources(ast: &mut SceneBsnAst, document_dir: &Path) {
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = read_isa_source(ast, node) else {
            continue;
        };
        let full = source_path(&source, document_dir);
        if full == source {
            continue;
        }
        let deleted = read_isa_deleted(ast, node);
        set_whole_component(
            ast,
            node,
            ISA_TYPE,
            isa_value(&full.to_string_lossy(), &deleted),
        );
    }
}

/// The whole-component `IsA { source, deleted }` value a document stores.
pub fn isa_value(source: &str, deleted: &[u32]) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: ISA_TYPE.to_string(),
        fields: BsnStructFields(vec![
            BsnField {
                name: "source".to_string(),
                value: BsnValue::String(source.to_string()),
            },
            BsnField {
                name: "deleted".to_string(),
                value: BsnValue::List(
                    deleted
                        .iter()
                        .map(|&d| BsnValue::Int(i128::from(d)))
                        .collect(),
                ),
            },
        ]),
    })
}

/// Make a source document instanceable as a prefab.
///
/// The resolver only materializes an `IsA` reference whose target has a single
/// root carrying the `Prefab` marker, so a plain scene is wrapped into that
/// shape: a lone root is marked in place, several roots are reparented under a
/// synthetic `Prefab` root, and every entity gets a sequential `PrefabEntityId`.
/// A file that is already a prefab passes through untouched.
///
/// The wrapping happens on the in-memory document; the authored file is never
/// rewritten. `display_name` names the synthetic root when one is made.
pub fn normalize_as_prefab_source(ast: &mut SceneBsnAst, display_name: &str) {
    let roots = ast.roots.clone();
    if roots.is_empty()
        || roots
            .iter()
            .any(|&root| ast.find_patch_by_type_path(root, PREFAB_TYPE).is_some())
    {
        return;
    }

    if roots.len() == 1 {
        let root = roots[0];
        set_whole_component(
            ast,
            root,
            PREFAB_TYPE,
            BsnValue::Type(PREFAB_TYPE.to_string()),
        );
        set_whole_component(ast, root, PREFAB_ENTITY_ID_TYPE, peid_value(0));
        for (i, desc) in ast.descendants_of(root).into_iter().enumerate() {
            set_whole_component(ast, desc, PREFAB_ENTITY_ID_TYPE, peid_value((i + 1) as u32));
        }
    } else {
        let synth = ast.create_entity_node(synthetic_root_patches(display_name));
        let mut next_id: u32 = 1;
        for root in roots {
            ast.move_to_parent(root, None, Some(synth));
            set_whole_component(ast, root, PREFAB_ENTITY_ID_TYPE, peid_value(next_id));
            next_id += 1;
            for desc in ast.descendants_of(root) {
                set_whole_component(ast, desc, PREFAB_ENTITY_ID_TYPE, peid_value(next_id));
                next_id += 1;
            }
        }
        ast.add_to_roots(synth);
    }
}

/// Read a prefab source document from disk, ready to instance.
///
/// The file is parsed as `.bsn` and normalized, so a plain scene comes back
/// instanceable, and its own prefab references are resolved against the
/// directory it came from. The file stem names a synthetic root when the
/// document needs one.
pub fn read_prefab_document(path: &Path) -> Result<SceneBsnAst, std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    let mut ast = jackdaw_bsn::parse_bsn_text(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let display_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
    normalize_as_prefab_source(&mut ast, display_name);
    absolutize_isa_sources(&mut ast, path.parent().unwrap_or(Path::new("")));
    Ok(ast)
}

/// The `PrefabEntityId(id)` whole-component value.
pub fn peid_value(id: u32) -> BsnValue {
    BsnValue::TupleStruct(BsnTupleStructData {
        type_path: PREFAB_ENTITY_ID_TYPE.to_string(),
        values: vec![BsnValue::Int(i128::from(id))],
    })
}

/// The patch list of a synthetic `Prefab` root: the marker, id zero, a name,
/// and the transform and visibility a parent of the real roots needs so they
/// still have a propagation backbone.
pub fn synthetic_root_patches(display_name: &str) -> Vec<BsnPatch> {
    vec![
        BsnPatch::Type(PREFAB_TYPE.to_string()),
        BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: PREFAB_ENTITY_ID_TYPE.to_string(),
            values: vec![BsnValue::Int(0)],
        }),
        BsnPatch::Name(display_name.to_string()),
        BsnPatch::Struct(BsnStructData {
            type_path: TRANSFORM_TYPE.to_string(),
            fields: BsnStructFields(vec![
                field("translation", vec3_value(0.0, 0.0, 0.0)),
                field("rotation", quat_value(0.0, 0.0, 0.0, 1.0)),
                field("scale", vec3_value(1.0, 1.0, 1.0)),
            ]),
        }),
        BsnPatch::Type(VISIBILITY_INHERITED_TYPE.to_string()),
    ]
}

fn field(name: &str, value: BsnValue) -> BsnField {
    BsnField {
        name: name.to_string(),
        value,
    }
}

fn vec3_value(x: f32, y: f32, z: f32) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: "glam::Vec3".to_string(),
        fields: BsnStructFields(vec![
            field("x", BsnValue::Float(f64::from(x))),
            field("y", BsnValue::Float(f64::from(y))),
            field("z", BsnValue::Float(f64::from(z))),
        ]),
    })
}

fn quat_value(x: f32, y: f32, z: f32, w: f32) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: "glam::Quat".to_string(),
        fields: BsnStructFields(vec![
            field("x", BsnValue::Float(f64::from(x))),
            field("y", BsnValue::Float(f64::from(y))),
            field("z", BsnValue::Float(f64::from(z))),
            field("w", BsnValue::Float(f64::from(w))),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_bsn::get_bsn_field;

    fn scene_with_source(source: &str) -> SceneBsnAst {
        let mut ast = SceneBsnAst::default();
        let BsnValue::Struct(data) = isa_value(source, &[]) else {
            unreachable!()
        };
        let node = ast.create_entity_node(vec![BsnPatch::Struct(data)]);
        ast.add_to_roots(node);
        ast
    }

    fn source_of(ast: &SceneBsnAst) -> String {
        match get_bsn_field(ast, ast.roots[0], ISA_TYPE, "source") {
            Some(BsnValue::String(s)) => s,
            other => panic!("expected a source string, got {other:?}"),
        }
    }

    #[test]
    fn a_relative_source_resolves_against_the_documents_own_directory() {
        assert_eq!(
            source_path(
                Path::new("props/crate.bsn"),
                Path::new("/game/assets/zones")
            ),
            PathBuf::from("/game/assets/zones/props/crate.bsn")
        );
    }

    #[test]
    fn an_absolute_source_is_taken_as_written() {
        assert_eq!(
            source_path(Path::new("/elsewhere/crate.bsn"), Path::new("/game/assets")),
            PathBuf::from("/elsewhere/crate.bsn")
        );
    }

    #[test]
    fn saving_spells_an_absolute_source_relative_to_the_scene() {
        let mut ast = scene_with_source("/game/assets/props/crate.bsn");
        relativize_isa_sources(&mut ast, Path::new("/game/assets/zones"));
        assert_eq!(source_of(&ast), "../props/crate.bsn");
    }

    #[test]
    fn a_document_with_no_directory_keeps_the_source_it_had() {
        // The unsaved-scene and in-memory-capture case: relativizing against
        // a placeholder would produce a path that resolves nowhere.
        let mut ast = scene_with_source("/game/assets/props/crate.bsn");
        relativize_isa_sources(&mut ast, Path::new("."));
        assert_eq!(source_of(&ast), "/game/assets/props/crate.bsn");
    }

    #[test]
    fn a_source_that_is_already_relative_is_left_alone() {
        let mut ast = scene_with_source("props/crate.bsn");
        relativize_isa_sources(&mut ast, Path::new("/game/assets/zones"));
        assert_eq!(source_of(&ast), "props/crate.bsn");
    }

    #[test]
    fn an_absolute_source_is_folded_so_a_caller_can_compare_it() {
        // A containment check reads `..` as an ordinary directory name, so an
        // unfolded source can point anywhere while looking like it points
        // inside.
        let mut ast = scene_with_source("/game/assets/../../etc/passwd.bsn");
        absolutize_isa_sources(&mut ast, Path::new("/game/assets"));
        assert_eq!(source_of(&ast), "/etc/passwd.bsn");
    }

    #[test]
    fn loading_names_the_file_a_relative_source_meant() {
        let mut ast = scene_with_source("../props/crate.bsn");
        absolutize_isa_sources(&mut ast, Path::new("/game/assets/zones"));
        assert_eq!(source_of(&ast), "/game/assets/props/crate.bsn");
    }

    #[test]
    fn a_saved_source_survives_the_round_trip_it_was_written_for() {
        let scene_dir = Path::new("/game/assets/zones");
        let mut ast = scene_with_source("/game/assets/props/crate.bsn");
        relativize_isa_sources(&mut ast, scene_dir);
        absolutize_isa_sources(&mut ast, scene_dir);
        relativize_isa_sources(&mut ast, scene_dir);
        assert_eq!(
            source_of(&ast),
            "../props/crate.bsn",
            "a second save writes what the first one did"
        );
    }
}
