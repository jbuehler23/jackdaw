//! Override queries over the BSN document model. These decide whether a
//! node inside a prefab instance diverges from its inherited baseline, and
//! what prefab entry it inherits from.
//!
//! Values compare through [`jackdaw_bsn::bsn_value_eq`], since
//! [`jackdaw_bsn::BsnValue`] has no derived `PartialEq`. Prefabs are
//! supplied through the same caller-owned lookup the resolver uses.

use std::path::PathBuf;

use bevy::ecs::entity::Entity;

use jackdaw_bsn::{BsnValue, SceneBsnAst, bsn_value_eq, get_bsn_field};

use crate::prefab::resolver_bsn::{
    ISA_TYPE, PREFAB_ENTITY_ID_TYPE, PrefabLookup, read_isa_source, read_prefab_entity_id,
};

/// True if `node` sits inside a prefab-instance subtree: it carries a
/// `PrefabEntityId` (tracked as a prefab descendant) and has an `IsA`
/// ancestor (so it inherits from a prefab file). The ancestry check is
/// inclusive of `node`, so an instance root that carries both components
/// counts.
pub fn is_inside_prefab_instance(ast: &SceneBsnAst, node: Entity) -> bool {
    ast.find_patch_by_type_path(node, PREFAB_ENTITY_ID_TYPE)
        .is_some()
        && ast.ancestor_with_component(node, ISA_TYPE).is_some()
}

/// The `(source, prefab_entity_id)` a node inherits from: the source path
/// of its nearest `IsA` ancestor (inclusive of the node), paired with the
/// node's own `PrefabEntityId`. `None` when the node has no id or no `IsA`
/// ancestor.
pub fn resolve_inheritance(ast: &SceneBsnAst, node: Entity) -> Option<(PathBuf, u32)> {
    let prefab_entity_id = read_prefab_entity_id(ast, node)?;
    let isa_node = ast.ancestor_with_component(node, ISA_TYPE)?;
    let source = read_isa_source(ast, isa_node)?;
    Some((source, prefab_entity_id))
}

/// True if `node`'s component value at `type_path` (or, when `field_path`
/// is `Some`, its subfield at that dotted path) differs from the value on
/// the prefab entry the node inherits from. False when the node authors no
/// such component, is not inside a prefab instance, or the prefab is not
/// cached. True when the component exists on the node but not on the prefab
/// (an addition).
pub fn field_is_overridden(
    ast: &SceneBsnAst,
    get_prefab: &PrefabLookup,
    node: Entity,
    type_path: &str,
    field_path: Option<&str>,
) -> bool {
    if get_bsn_field(ast, node, type_path, "").is_none() {
        return false;
    }

    let Some((prefab_path, prefab_entity_id)) = resolve_inheritance(ast, node) else {
        return false;
    };
    let Some(prefab) = get_prefab(&prefab_path) else {
        return false;
    };
    let Some(prefab_node) =
        prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(prefab_entity_id))
    else {
        return false;
    };

    if get_bsn_field(prefab, prefab_node, type_path, "").is_none() {
        // Component exists on the instance but not on the prefab: an addition.
        return true;
    }

    let path = field_path.unwrap_or("");
    let scene_leaf = get_bsn_field(ast, node, type_path, path);
    let prefab_leaf = get_bsn_field(prefab, prefab_node, type_path, path);
    match (scene_leaf, prefab_leaf) {
        (Some(a), Some(b)) => !bsn_value_eq(&a, &b),
        (None, None) => false,
        _ => true,
    }
}

/// Returns `(dot_path, leaf)` pairs for every leaf in `scene` that differs
/// from `prefab`'s value at the same path. Struct branches recurse so a
/// single Vec3 axis difference produces `translation.x` rather than a full
/// Vec3 blob; every other shape (tuple struct, list, map, scalar) is a leaf.
/// When `prefab` is `None` (the component itself was added on the instance),
/// every leaf is reported.
pub fn collect_overridden_paths(
    scene: &BsnValue,
    prefab: Option<&BsnValue>,
) -> Vec<(String, BsnValue)> {
    let mut out = Vec::new();
    walk_leaves(scene, prefab, String::new(), &mut out);
    out
}

fn walk_leaves(
    scene: &BsnValue,
    prefab: Option<&BsnValue>,
    path: String,
    out: &mut Vec<(String, BsnValue)>,
) {
    match scene {
        BsnValue::Struct(data) => {
            let prefab_fields = match prefab {
                Some(BsnValue::Struct(p)) => Some(&p.fields),
                _ => None,
            };
            for field in &data.fields.0 {
                let next_path = if path.is_empty() {
                    field.name.clone()
                } else {
                    format!("{path}.{}", field.name)
                };
                let prefab_field = prefab_fields
                    .and_then(|fields| fields.0.iter().find(|f| f.name == field.name))
                    .map(|f| &f.value);
                walk_leaves(&field.value, prefab_field, next_path, out);
            }
        }
        leaf => {
            let differs = match prefab {
                Some(p) => !bsn_value_eq(p, leaf),
                None => true,
            };
            if differs && !path.is_empty() {
                out.push((path, leaf.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_bsn::{
        BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    };
    use std::path::Path;

    const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

    fn f(name: &str, value: BsnValue) -> BsnField {
        BsnField {
            name: name.to_string(),
            value,
        }
    }

    fn peid_patch(id: i128) -> BsnPatch {
        BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: PREFAB_ENTITY_ID_TYPE.to_string(),
            values: vec![BsnValue::Int(id)],
        })
    }

    fn isa_patch(source: &str) -> BsnPatch {
        BsnPatch::Struct(BsnStructData {
            type_path: ISA_TYPE.to_string(),
            fields: BsnStructFields(vec![
                f("source", BsnValue::String(source.to_string())),
                f("deleted", BsnValue::List(Vec::new())),
            ]),
        })
    }

    fn vec3(x: f64, y: f64, z: f64) -> BsnValue {
        BsnValue::Struct(BsnStructData {
            type_path: "glam::Vec3".to_string(),
            fields: BsnStructFields(vec![
                f("x", BsnValue::Float(x)),
                f("y", BsnValue::Float(y)),
                f("z", BsnValue::Float(z)),
            ]),
        })
    }

    fn transform_patch(translation: BsnValue) -> BsnPatch {
        BsnPatch::Struct(BsnStructData {
            type_path: TRANSFORM.to_string(),
            fields: BsnStructFields(vec![f("translation", translation)]),
        })
    }

    fn make_prefab() -> SceneBsnAst {
        let mut ast = SceneBsnAst::default();
        let root =
            ast.create_entity_node(vec![BsnPatch::Type(PREFAB_TYPE.to_string()), peid_patch(0)]);
        let child =
            ast.create_entity_node(vec![peid_patch(1), transform_patch(vec3(0.0, 1.0, 0.0))]);
        ast.add_to_roots(root);
        ast.add_child_to_ast(root, child);
        ast
    }

    /// A scene with an instance root of prefab.bsn and a child override
    /// carrying PrefabEntityId(1) with the given translation.
    fn make_scene(translation: BsnValue) -> (SceneBsnAst, Entity, Entity) {
        let mut scene = SceneBsnAst::default();
        let instance = scene.create_entity_node(vec![isa_patch("prefab.bsn")]);
        let child = scene.create_entity_node(vec![peid_patch(1), transform_patch(translation)]);
        scene.add_to_roots(instance);
        scene.add_child_to_ast(instance, child);
        (scene, instance, child)
    }

    fn lookup<'a>(
        map: &'a std::collections::HashMap<PathBuf, SceneBsnAst>,
    ) -> impl Fn(&Path) -> Option<&'a SceneBsnAst> {
        move |p: &Path| map.get(p)
    }

    #[test]
    fn inside_prefab_instance_detects_descendant() {
        let (scene, instance, child) = make_scene(vec3(0.0, 1.0, 0.0));
        assert!(is_inside_prefab_instance(&scene, child));
        // The instance root carries IsA but no PrefabEntityId here, so it is
        // not itself counted as inside.
        assert!(!is_inside_prefab_instance(&scene, instance));
    }

    #[test]
    fn resolve_inheritance_reports_source_and_id() {
        let (scene, _instance, child) = make_scene(vec3(0.0, 1.0, 0.0));
        assert_eq!(
            resolve_inheritance(&scene, child),
            Some((PathBuf::from("prefab.bsn"), 1))
        );
    }

    #[test]
    fn field_is_overridden_true_for_diverged_field() {
        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("prefab.bsn"), make_prefab());
        let get = lookup(&prefabs);

        // Child overrides translation.y to 9.0 (baseline is 1.0).
        let (scene, _instance, child) = make_scene(vec3(0.0, 9.0, 0.0));
        assert!(field_is_overridden(
            &scene,
            &get,
            child,
            TRANSFORM,
            Some("translation.y"),
        ));
        // The whole component also differs.
        assert!(field_is_overridden(&scene, &get, child, TRANSFORM, None));
    }

    #[test]
    fn field_is_overridden_false_for_inherited_field() {
        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("prefab.bsn"), make_prefab());
        let get = lookup(&prefabs);

        // Child matches the baseline exactly.
        let (scene, _instance, child) = make_scene(vec3(0.0, 1.0, 0.0));
        assert!(!field_is_overridden(
            &scene,
            &get,
            child,
            TRANSFORM,
            Some("translation.y"),
        ));
        assert!(!field_is_overridden(&scene, &get, child, TRANSFORM, None));
    }
}
