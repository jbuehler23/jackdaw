//! ECS introspection over BRP: `jackdaw/archetypes` and `jackdaw/schedules`
//! feed the explorer's ECS internals page.

use bevy::ecs::schedule::graph::Direction;
use bevy::ecs::schedule::{NodeId, ScheduleGraph, Schedules, SystemKey};
use bevy::prelude::*;
use bevy::remote::BrpResult;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Resolves a dependency-graph node to the single system it represents, for
/// the purpose of reporting run-order edges between systems.
///
/// System nodes resolve directly. Set nodes only resolve when the set is an
/// implicit per-function `SystemTypeSet` (created automatically for calls
/// like `.after(some_system)`), which always contains exactly the one system
/// it was minted for; its sole hierarchy child is that system. Explicit,
/// possibly multi-member sets are left unresolved (`None`) and their edges
/// are skipped, since expanding them to every member system is out of scope
/// for this first pass.
fn resolve_edge_endpoint_to_system(graph: &ScheduleGraph, node: NodeId) -> Option<SystemKey> {
    match node {
        NodeId::System(key) => Some(key),
        NodeId::Set(key) => {
            let set = graph.system_sets.get(key)?;
            set.system_type()?;
            graph
                .hierarchy()
                .edges_directed(NodeId::Set(key), Direction::Outgoing)
                .find_map(|(_, child)| child.as_system())
        }
    }
}

/// Names of the non-anonymous sets a system directly belongs to, using each
/// set's `Debug` name. Excludes the implicit per-function `SystemTypeSet`
/// that every function system is automatically placed in.
fn system_set_names(graph: &ScheduleGraph, key: SystemKey) -> Vec<String> {
    graph
        .hierarchy()
        .edges_directed(NodeId::System(key), Direction::Incoming)
        .filter_map(|(parent, _)| {
            let NodeId::Set(set_key) = parent else {
                return None;
            };
            let set = graph.system_sets.get(set_key)?;
            if set.system_type().is_some() || set.is_anonymous() {
                return None;
            }
            Some(format!("{set:?}"))
        })
        .collect()
}

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
                // systems() yields the executable in run order; that order
                // is preserved as the index space for `edges`.
                let ordered: Vec<(SystemKey, String)> = iter
                    .map(|(key, system)| (key, system.name().to_string()))
                    .collect();
                let key_to_index: HashMap<SystemKey, usize> = ordered
                    .iter()
                    .enumerate()
                    .map(|(index, (key, _))| (*key, index))
                    .collect();

                let graph = schedule.graph();

                let systems: Vec<Value> = ordered
                    .iter()
                    .map(|(key, name)| {
                        json!({
                            "name": name,
                            "sets": system_set_names(graph, *key),
                        })
                    })
                    .collect();

                let edges: Vec<Value> = graph
                    .dependency()
                    .all_edges()
                    .filter_map(|(before, after)| {
                        let before = resolve_edge_endpoint_to_system(graph, before)?;
                        let after = resolve_edge_endpoint_to_system(graph, after)?;
                        let before_index = *key_to_index.get(&before)?;
                        let after_index = *key_to_index.get(&after)?;
                        Some(json!([before_index, after_index]))
                    })
                    .collect();

                out.push(json!({
                    "schedule": format!("{label:?}"),
                    "initialized": true,
                    "systems": systems,
                    "edges": edges,
                }));
            }
            Err(_) => {
                out.push(json!({
                    "schedule": format!("{label:?}"),
                    "initialized": false,
                    "systems": [],
                    "edges": [],
                }));
            }
        }
    }

    Ok(json!({ "schedules": out }))
}
