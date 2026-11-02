//! `jackdaw/brush_topology`: a brush's authored topology as compact arrays,
//! for a caller that needs element indices. Reads only; no undo entry.

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult};
use jackdaw_geometry::AttributeData;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

use super::server::{check_enabled, entity_from_params, invalid_params};

const FIELDS: [&str; 5] = ["positions", "polygons", "edges", "normals", "colors"];
const DEFAULT_FIELDS: [&str; 2] = ["positions", "polygons"];

/// The topology of the brush named by `entity` or `name`, in its own space:
/// `positions` per vertex, `polygons` as vertex rings, `edges` as vertex
/// pairs (the order edge indices count in), `normals` per face and `colors`
/// per vertex or corner when painted. `revision` changes whenever the
/// elements are renumbered.
pub fn brush_topology_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected an \"entity\" or a \"name\""))?;
    let fields = fields_from_params(params.get("fields"))?;
    let entity = entity_from_params(world, &params)?;
    let Some(brush) = world.get::<Brush>(entity) else {
        return Err(invalid_params(format!("entity {entity} is not a brush")));
    };
    let topology = &brush.topology;
    let positions: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();

    let mut answer = json!({
        "entity": entity.to_bits(),
        "revision": jackdaw_select::address::topology_revision(topology),
        "counts": {
            "vertices": topology.vertices.len(),
            "edges": topology.edges.len(),
            "faces": topology.polygons.len(),
        },
    });
    for field in fields {
        let value = match field {
            "positions" => Value::Array(positions.iter().map(|p| vector(*p)).collect()),
            "polygons" => Value::Array(
                (0..topology.polygons.len())
                    .map(|face| json!(topology.face_ring(face).collect::<Vec<u32>>()))
                    .collect(),
            ),
            "edges" => Value::Array(topology.edges.iter().map(|edge| json!(edge.v)).collect()),
            "normals" => Value::Array(
                (0..topology.polygons.len())
                    .map(|face| {
                        if topology.polygons[face].loop_total < 3 {
                            vector(Vec3::ZERO)
                        } else {
                            vector(topology.face_normal_with(&positions, face))
                        }
                    })
                    .collect(),
            ),
            _ => colors(topology),
        };
        answer[field] = value;
    }
    Ok(answer)
}

fn fields_from_params(given: Option<&Value>) -> Result<Vec<&'static str>, BrpError> {
    let Some(given) = given.filter(|given| !given.is_null()) else {
        return Ok(DEFAULT_FIELDS.to_vec());
    };
    let names = given
        .as_array()
        .ok_or_else(|| invalid_params("`fields` is a list of field names"))?;
    names
        .iter()
        .map(|name| {
            let name = name.as_str().unwrap_or_default();
            FIELDS
                .iter()
                .find(|known| **known == name)
                .copied()
                .ok_or_else(|| {
                    invalid_params(format!(
                        "unknown field `{name}`; one of {}",
                        FIELDS.join(", ")
                    ))
                })
        })
        .collect()
}

/// The painted colours, per vertex or per face corner, or `null` when the
/// brush has none.
fn colors(topology: &jackdaw_geometry::BrushTopology) -> Value {
    let stacks = [
        ("vertex", &topology.attributes.vert),
        ("corner", &topology.attributes.loop_),
    ];
    for (domain, stack) in stacks {
        if let Some(AttributeData::Color(values)) = stack.get("color") {
            let values: Vec<Value> = values
                .iter()
                .map(|rgba| Value::Array(rgba.iter().map(|c| round(*c)).collect()))
                .collect();
            return json!({ "domain": domain, "values": values });
        }
    }
    Value::Null
}

fn round(value: f32) -> Value {
    let rounded = (f64::from(value) * 1000.0).round() / 1000.0;
    json!(if rounded == 0.0 { 0.0 } else { rounded })
}

fn vector(value: Vec3) -> Value {
    json!([round(value.x), round(value.y), round(value.z)])
}
