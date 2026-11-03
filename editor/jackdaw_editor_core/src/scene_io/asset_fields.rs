//! The rule that keeps a saved document free of machine-specific paths.
//!
//! A document names an asset by its path under the project's assets folder,
//! one namespace whatever folder the document itself lives in. An absolute
//! path names a place on one machine, so the emitter refuses to write one and
//! names the field that holds it.

use std::path::Path;

use bevy::reflect::{TypeInfo, TypeRegistry};
use jackdaw_bsn::{BsnPatch, BsnStructData, BsnValue, SceneBsnAst};
use jackdaw_scene_types::AssetRef;

use crate::project_types::ProjectTypes;

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const ISA_SOURCE_FIELD: &str = "source";

/// A field naming a file by an absolute path, as the emitter reports it.
pub struct AbsoluteAssetField {
    /// The component field holding it, spelled `<type path>.<field>`.
    pub field: String,
    /// The path the field names.
    pub path: String,
}

/// Every field of `ast` that names an asset or a prefab source by an absolute
/// path. An empty result is a document that travels.
pub fn absolute_asset_fields(
    ast: &SceneBsnAst,
    registry: &TypeRegistry,
    project: Option<&ProjectTypes>,
) -> Vec<AbsoluteAssetField> {
    let mut found = Vec::new();
    for patches in ast.roots.iter().flat_map(|&root| {
        std::iter::once(root)
            .chain(ast.descendants_of(root))
            .filter_map(|node| ast.get_patches(node))
    }) {
        for &patch in &patches.0 {
            let Some(BsnPatch::Struct(data)) = ast.get_patch(patch) else {
                continue;
            };
            collect_from_struct(data, registry, project, &mut found);
        }
    }
    found
}

/// The message a save refuses with, naming every field that holds one.
pub fn refusal(fields: &[AbsoluteAssetField]) -> String {
    let named = fields
        .iter()
        .map(|found| format!("{} names '{}'", found.field, found.path))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "an asset is named by an absolute path, which holds only on this machine: {named}. \
         Name it relative to the project's assets folder"
    )
}

fn collect_from_struct(
    data: &BsnStructData,
    registry: &TypeRegistry,
    project: Option<&ProjectTypes>,
    found: &mut Vec<AbsoluteAssetField>,
) {
    for field in &data.fields.0 {
        if !names_an_asset(&data.type_path, &field.name, registry, project) {
            continue;
        }
        for path in spelled_paths(&field.value) {
            if names_one_machine(&path) {
                found.push(AbsoluteAssetField {
                    field: format!("{}.{}", data.type_path, field.name),
                    path,
                });
            }
        }
    }
}

/// Whether a field names a file: a prefab instance's source, or a field marked
/// with [`AssetRef`], on a type the editor knows itself or one the open project
/// reported.
fn names_an_asset(
    type_path: &str,
    field: &str,
    registry: &TypeRegistry,
    project: Option<&ProjectTypes>,
) -> bool {
    if type_path == ISA_TYPE && field == ISA_SOURCE_FIELD {
        return true;
    }
    if let Some(TypeInfo::Struct(info)) = registry
        .get_with_type_path(type_path)
        .map(bevy::reflect::TypeRegistration::type_info)
        && info
            .field(field)
            .is_some_and(|field| field.get_attribute::<AssetRef>().is_some())
    {
        return true;
    }
    project
        .and_then(|project| project.type_schema(type_path))
        .is_some_and(|schema| {
            schema
                .fields
                .iter()
                .any(|known| known.name == field && !known.asset_type_path.is_empty())
        })
}

/// Whether a spelling names a place on one machine rather than a file the
/// project holds: an absolute path, in this platform's spelling or another's.
fn names_one_machine(spelling: &str) -> bool {
    if Path::new(spelling).is_absolute() || spelling.starts_with('/') {
        return true;
    }
    if spelling.starts_with("\\\\") {
        return true;
    }
    let mut chars = spelling.chars();
    let drive = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    drive && chars.next() == Some(':') && matches!(chars.next(), Some('/' | '\\'))
}

fn spelled_paths(value: &BsnValue) -> Vec<String> {
    match value {
        BsnValue::String(spelling) => vec![spelling.clone()],
        BsnValue::List(items) => items.iter().flat_map(spelled_paths).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use jackdaw_bsn::{BsnField, BsnStructFields};

    #[derive(Reflect)]
    struct Outfit {
        #[reflect(@AssetRef("my_game::MaterialDef"))]
        materials: Vec<String>,
    }

    fn scene_with(type_path: &str, field: &str, value: BsnValue) -> SceneBsnAst {
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(vec![BsnPatch::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields(vec![BsnField {
                name: field.to_string(),
                value,
            }]),
        })]);
        ast.add_to_roots(node);
        ast
    }

    #[test]
    fn a_prefab_source_named_by_an_absolute_path_is_reported_with_its_field() {
        let spelled = std::path::absolute("/game/assets/props/crate.bsn")
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let ast = scene_with(ISA_TYPE, "source", BsnValue::String(spelled.clone()));

        let found = absolute_asset_fields(&ast, &TypeRegistry::default(), None);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].field, "jackdaw::prefab::components::IsA.source");
        assert_eq!(found[0].path, spelled);
        assert!(refusal(&found).contains("IsA.source"));
    }

    #[test]
    fn a_source_named_under_the_assets_folder_passes() {
        let ast = scene_with(
            ISA_TYPE,
            "source",
            BsnValue::String("props/crate.bsn".to_string()),
        );

        assert!(absolute_asset_fields(&ast, &TypeRegistry::default(), None).is_empty());
    }

    #[test]
    fn a_marked_field_is_reported_for_every_absolute_path_in_its_list() {
        let mut registry = TypeRegistry::default();
        registry.register::<Outfit>();
        let spelled = std::path::absolute("/game/assets/materials/worn.bsn")
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let ast = scene_with(
            Outfit::type_path(),
            "materials",
            BsnValue::List(vec![
                BsnValue::String("materials/simple.bsn".to_string()),
                BsnValue::String(spelled.clone()),
            ]),
        );

        let found = absolute_asset_fields(&ast, &registry, None);

        assert_eq!(found.len(), 1, "only the absolute row is reported");
        assert_eq!(found[0].path, spelled);
    }

    #[test]
    fn a_field_naming_no_asset_may_hold_whatever_it_likes() {
        let spelled = std::path::absolute("/var/log/messages")
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let ast = scene_with("my_game::Note", "body", BsnValue::String(spelled));

        assert!(absolute_asset_fields(&ast, &TypeRegistry::default(), None).is_empty());
    }

    /// A project type as the schema extractor reports it, with one field
    /// marked as naming an asset.
    fn schema_type(type_path: &str, field: &str) -> jackdaw_schema::TypeSchema {
        jackdaw_schema::TypeSchema {
            type_path: type_path.to_string(),
            short_name: type_path.to_string(),
            module_path: String::new(),
            category: String::new(),
            description: String::new(),
            editor_description: String::new(),
            hidden: false,
            asset: false,
            preview: String::new(),
            default_constructible: false,
            fields: vec![jackdaw_schema::FieldSchema {
                name: field.to_string(),
                type_path: String::type_path().to_string(),
                item_type_path: String::new(),
                asset_type_path: "my_game::MaterialDef".to_string(),
            }],
            kind: jackdaw_schema::TypeKind::Struct,
            default: None,
            variants: Vec::new(),
            entity_fields: Vec::new(),
            fills_gaps: false,
        }
    }

    #[test]
    fn a_field_a_project_type_marks_is_reported_though_the_editor_cannot_load_the_type() {
        let mut types = ProjectTypes::default();
        types.update(
            &jackdaw_schema::ProjectSchema {
                components: vec![schema_type("my_game::Outfit", "material")],
                ..Default::default()
            },
            &Default::default(),
        );
        let spelled = std::path::absolute("/game/assets/materials/worn.bsn")
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let ast = scene_with(
            "my_game::Outfit",
            "material",
            BsnValue::String(spelled.clone()),
        );

        let found = absolute_asset_fields(&ast, &TypeRegistry::default(), Some(&types));

        assert_eq!(
            found.len(),
            1,
            "a type only the schema describes is checked"
        );
        assert_eq!(found[0].path, spelled);
    }

    #[test]
    fn a_path_spelled_for_another_platform_is_a_machine_path_too() {
        assert!(names_one_machine("C:/game/assets/props/crate.bsn"));
        assert!(names_one_machine("C:\\game\\assets\\props\\crate.bsn"));
        assert!(names_one_machine("\\\\host\\share\\crate.bsn"));
        assert!(!names_one_machine("props/crate.bsn"));
        assert!(!names_one_machine("c/crate.bsn"));
    }
}
