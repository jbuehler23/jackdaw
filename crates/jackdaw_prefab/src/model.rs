//! The model a prefab draws, for a renderer that draws it without spawning
//! the prefab's entities.

use std::collections::BTreeMap;

use bevy::ecs::entity::Entity;
use bevy::math::{Affine3A, Quat, Vec3};
use jackdaw_bsn::{BsnValue, SceneBsnAst, get_bsn_field};

const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";
const GLTF_SOURCE_TYPE: &str = "jackdaw_scene_types::types::GltfSource";
const MATERIAL_OVERRIDES_TYPE: &str = "jackdaw_scene_types::types::MaterialOverrides";
const INSTANCE_OVERRIDES_TYPE: &str = "jackdaw_scene_types::types::InstanceMaterialOverrides";

/// The one glTF model a prefab draws, and the materials it wears.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabModel {
    /// The glTF file the model's node names, as the document spells it.
    pub source: String,
    /// Where the model stands under the prefab's root, the root's own
    /// transform left out as an instance's placement replaces it.
    pub local: Affine3A,
    /// Material asset path by glTF material name: the model's own
    /// overrides, with the instance overrides of the nodes above it laid on
    /// top.
    pub materials: BTreeMap<String, String>,
}

/// The model `prefab` draws, or `None` when it draws none or more than one.
pub fn prefab_model(prefab: &SceneBsnAst) -> Option<PrefabModel> {
    let [root] = prefab.roots.as_slice() else {
        return None;
    };
    let mut found: Vec<(Vec<Entity>, Affine3A)> = Vec::new();
    let mut open = vec![(vec![*root], Affine3A::IDENTITY)];
    while let Some((chain, at)) = open.pop() {
        let node = *chain.last()?;
        if get_bsn_field(prefab, node, GLTF_SOURCE_TYPE, "path").is_some() {
            found.push((chain.clone(), at));
        }
        for child in prefab.get_children_ast(node) {
            let mut below = chain.clone();
            below.push(child);
            open.push((below, at * node_affine(prefab, child)));
        }
    }
    let [(chain, local)] = found.as_slice() else {
        return None;
    };
    let model = *chain.last()?;
    let Some(BsnValue::String(source)) = get_bsn_field(prefab, model, GLTF_SOURCE_TYPE, "path")
    else {
        return None;
    };
    let mut materials = chain
        .iter()
        .rev()
        .find_map(|&node| material_map(prefab, node, MATERIAL_OVERRIDES_TYPE))
        .unwrap_or_default();
    for &node in chain.iter().rev() {
        if let Some(layer) = material_map(prefab, node, INSTANCE_OVERRIDES_TYPE) {
            materials.extend(layer);
        }
    }
    Some(PrefabModel {
        source,
        local: *local,
        materials,
    })
}

/// A node's `Transform` as an affine, identity where it spells nothing.
fn node_affine(prefab: &SceneBsnAst, node: Entity) -> Affine3A {
    let field = |name: &str| get_bsn_field(prefab, node, TRANSFORM_TYPE, name);
    let translation = field("translation")
        .and_then(|value| axes(&value, ["x", "y", "z"], [0.0; 3]))
        .map_or(Vec3::ZERO, Vec3::from_array);
    let rotation = field("rotation")
        .and_then(|value| axes(&value, ["x", "y", "z", "w"], [0.0, 0.0, 0.0, 1.0]))
        .map_or(Quat::IDENTITY, Quat::from_array);
    let scale = field("scale")
        .and_then(|value| axes(&value, ["x", "y", "z"], [1.0; 3]))
        .map_or(Vec3::ONE, Vec3::from_array);
    Affine3A::from_scale_rotation_translation(scale, rotation, translation)
}

/// The named number fields of a struct value, `base` where one is missing.
fn axes<const N: usize>(value: &BsnValue, names: [&str; N], base: [f32; N]) -> Option<[f32; N]> {
    let BsnValue::Struct(data) = value else {
        return None;
    };
    let mut out = base;
    for field in &data.fields.0 {
        let Some(slot) = names.iter().position(|name| *name == field.name) else {
            continue;
        };
        out[slot] = match field.value {
            BsnValue::Float(v) => v as f32,
            BsnValue::Int(v) => v as f32,
            _ => return None,
        };
    }
    Some(out)
}

/// The `materials` map of a node's `type_path` component, when it has one.
fn material_map(
    prefab: &SceneBsnAst,
    node: Entity,
    type_path: &str,
) -> Option<BTreeMap<String, String>> {
    let BsnValue::Map(pairs) = get_bsn_field(prefab, node, type_path, "materials")? else {
        return None;
    };
    Some(
        pairs
            .into_iter()
            .filter_map(|pair| match pair {
                (BsnValue::String(name), BsnValue::String(path)) => Some((name, path)),
                _ => None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKED_FIR: &str = r#"jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
#Fir
bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 5.0, y: 0.0, z: 0.0 } }
jackdaw_scene_types::types::InstanceMaterialOverrides { materials: map[("Bark", "materials/wet_bark.bsn")] }
bevy_ecs::hierarchy::Children [
    #Fir
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 2.0, z: 0.0 }, scale: glam::Vec3 { x: 2.0, y: 2.0, z: 2.0 } }
    jackdaw_scene_types::types::GltfSource { path: "models/fir.gltf", scene_index: 0 }
    jackdaw_scene_types::types::MaterialOverrides { materials: map[("Bark", "materials/bark.bsn"), ("Leaves", "materials/fir_leaves.bsn")] }
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#;

    fn parse(text: &str) -> SceneBsnAst {
        jackdaw_bsn::parse_bsn_text(text).expect("the prefab parses")
    }

    #[test]
    fn a_packed_prefab_draws_its_model_in_the_models_materials_under_the_roots() {
        let model = prefab_model(&parse(PACKED_FIR)).expect("the prefab draws one model");
        assert_eq!(model.source, "models/fir.gltf");
        assert_eq!(
            model.local,
            Affine3A::from_scale_rotation_translation(
                Vec3::splat(2.0),
                Quat::IDENTITY,
                Vec3::new(0.0, 2.0, 0.0)
            ),
            "the model's own transform counts and the root's does not"
        );
        assert_eq!(
            model.materials.get("Leaves").map(String::as_str),
            Some("materials/fir_leaves.bsn")
        );
        assert_eq!(
            model.materials.get("Bark").map(String::as_str),
            Some("materials/wet_bark.bsn"),
            "the root's instance overrides win over the model's own"
        );
    }

    #[test]
    fn a_prefab_holding_two_models_draws_none() {
        let two = PACKED_FIR.replace(
            "    jackdaw::prefab::components::PrefabEntityId(1)\n]",
            "    jackdaw::prefab::components::PrefabEntityId(1)\n    ,\n    #Stone\n    \
             jackdaw_scene_types::types::GltfSource { path: \"models/stone.gltf\", scene_index: 0 }\n    \
             jackdaw::prefab::components::PrefabEntityId(2)\n]",
        );
        assert_eq!(prefab_model(&parse(&two)), None);
    }
}
