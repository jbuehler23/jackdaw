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

/// One scheduler-detected ambiguity: two systems the scheduler found no
/// ordering between despite conflicting data access. `a`/`b` index into the
/// same schedule's `systems` array as `edges`. An empty `components` list
/// means the conflict is on `World` access in general (e.g. an exclusive
/// system) rather than a specific component.
struct Ambiguity {
    a: usize,
    b: usize,
    components: Vec<String>,
}

/// Extract `graph.conflicting_systems()` into the reply's index space.
///
/// Maps each `SystemKey` through `key_to_index` (built the same way as for
/// `edges`); pairs where either system does not resolve to an index in this
/// schedule are skipped. Resolves each conflicting `ComponentId` to a name
/// via `world`, falling back to the numeric id when the component isn't
/// registered, the same fallback pattern used elsewhere in this file.
fn ambiguities_for(
    graph: &ScheduleGraph,
    key_to_index: &HashMap<SystemKey, usize>,
    world: &World,
) -> Vec<Ambiguity> {
    graph
        .conflicting_systems()
        .iter()
        .filter_map(|(a, b, components)| {
            let a_index = *key_to_index.get(a)?;
            let b_index = *key_to_index.get(b)?;
            let components = components
                .iter()
                .map(|&id| match world.components().get_info(id) {
                    Some(info) => info.name().to_string(),
                    None => id.index().to_string(),
                })
                .collect();
            Some(Ambiguity {
                a: a_index,
                b: b_index,
                components,
            })
        })
        .collect()
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

                let ambiguities: Vec<Value> = ambiguities_for(graph, &key_to_index, world)
                    .into_iter()
                    .map(|a| {
                        json!({
                            "a": a.a,
                            "b": a.b,
                            "components": a.components,
                        })
                    })
                    .collect();

                out.push(json!({
                    "schedule": format!("{label:?}"),
                    "initialized": true,
                    "systems": systems,
                    "edges": edges,
                    "ambiguities": ambiguities,
                }));
            }
            Err(_) => {
                out.push(json!({
                    "schedule": format!("{label:?}"),
                    "initialized": false,
                    "systems": [],
                    "edges": [],
                    "ambiguities": [],
                }));
            }
        }
    }

    Ok(json!({ "schedules": out }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::schedule::{Schedule, ScheduleLabel};

    #[derive(Resource, Default)]
    struct Counter(i32);

    fn write_a(mut counter: ResMut<Counter>) {
        counter.0 += 1;
    }

    fn write_b(mut counter: ResMut<Counter>) {
        counter.0 += 1;
    }

    #[derive(ScheduleLabel, Debug, Clone, Copy, Hash, PartialEq, Eq)]
    struct TestSchedule;

    #[test]
    fn ambiguities_for_reports_unordered_conflicting_systems() {
        let mut world = World::new();
        world.insert_resource(Counter::default());

        let mut schedule = Schedule::new(TestSchedule);
        schedule.add_systems((write_a, write_b));
        schedule.initialize(&mut world).unwrap();

        let ordered: Vec<(SystemKey, String)> = schedule
            .systems()
            .unwrap()
            .map(|(key, system)| (key, system.name().to_string()))
            .collect();
        let key_to_index: HashMap<SystemKey, usize> = ordered
            .iter()
            .enumerate()
            .map(|(index, (key, _))| (*key, index))
            .collect();

        let ambiguities = ambiguities_for(schedule.graph(), &key_to_index, &world);

        assert_eq!(ambiguities.len(), 1);
        let ambiguity = &ambiguities[0];
        let pair: std::collections::HashSet<usize> =
            [ambiguity.a, ambiguity.b].into_iter().collect();
        assert_eq!(pair, std::collections::HashSet::from([0, 1]));
        assert!(
            ambiguity.components.iter().any(|c| c.ends_with("Counter")),
            "expected a component name ending in Counter, got {:?}",
            ambiguity.components
        );
    }

    #[test]
    fn ambiguities_for_skips_pairs_missing_from_the_index() {
        let mut world = World::new();
        world.insert_resource(Counter::default());

        let mut schedule = Schedule::new(TestSchedule);
        schedule.add_systems((write_a, write_b));
        schedule.initialize(&mut world).unwrap();

        let ambiguities = ambiguities_for(schedule.graph(), &HashMap::new(), &world);
        assert!(ambiguities.is_empty());
    }
}
