//! ECS introspection over BRP: `jackdaw/archetypes` and `jackdaw/schedules`
//! feed the explorer's ECS internals page.

use bevy::ecs::schedule::Schedules;
use bevy::prelude::*;
use bevy::remote::BrpResult;
use serde_json::{Value, json};

pub fn jackdaw_archetypes_handler(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let mut archetypes: Vec<(u32, Value)> = Vec::new();
    for archetype in world.archetypes().iter() {
        if archetype.is_empty() {
            continue;
        }
        let components: Vec<String> = archetype
            .components()
            .iter()
            .filter_map(|&id| world.components().get_info(id))
            .map(|info| info.name().to_string())
            .collect();
        let bytes_per_entity: usize = archetype
            .components()
            .iter()
            .filter_map(|&id| world.components().get_info(id))
            .map(|info| info.layout().size())
            .sum();
        archetypes.push((
            archetype.len(),
            json!({
                "components": components,
                "entity_count": archetype.len(),
                "bytes_per_entity": bytes_per_entity,
            }),
        ));
    }
    archetypes.sort_by_key(|entry| std::cmp::Reverse(entry.0));

    Ok(json!({
        "archetypes": archetypes.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    }))
}

pub fn jackdaw_schedules_handler(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Some(schedules) = world.get_resource::<Schedules>() else {
        return Ok(json!({ "schedules": [] }));
    };

    let mut out = Vec::new();
    for (label, schedule) in schedules.iter() {
        match schedule.systems() {
            Ok(iter) => {
                // systems() yields the executable in run order.
                let systems: Vec<String> = iter
                    .map(|(_key, system)| system.name().to_string())
                    .collect();
                out.push(json!({
                    "schedule": format!("{label:?}"),
                    "initialized": true,
                    "systems": systems,
                }));
            }
            Err(_) => {
                out.push(json!({
                    "schedule": format!("{label:?}"),
                    "initialized": false,
                    "systems": [],
                }));
            }
        }
    }

    Ok(json!({ "schedules": out }))
}
