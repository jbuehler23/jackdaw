//! Relationships view: the running game's entity hierarchy (`ChildOf`) as a
//! force-directed node graph, drawn via `graph::spawn_graph_positioned`.
//!
//! Reads the same `jackdaw/scene_snapshot` method the Remote Entities tree
//! already polls (`entity_browser.rs`), on its own slower poll timer since
//! the hierarchy changes far less often than the tree needs to refresh and
//! the snapshot payload can be large. `hierarchy_from_reply` derives nodes
//! and parent-child edges from each entity's `ChildOf` component the same
//! way `entity_browser.rs` does.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use serde::Deserialize;

use jackdaw_feathers::tokens;
use jackdaw_remote::scene_snapshot::RemoteEntity;

use super::graph::{self, GraphEdgeMaterial, GraphNodeSpec};
use super::style;

/// How a node's `ChildOf` state classifies it for coloring: `Root` has no
/// parent, `Named` has a parent and a `Name`, `Anonymous` has a parent but
/// no `Name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Named,
    Anonymous,
}

/// One node to render: its display label and hierarchy classification.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub label: String,
    pub kind: NodeKind,
}

/// The last parsed `jackdaw/scene_snapshot` reply, written by the poll
/// helper. The method returns a bare JSON array, so this wraps it
/// transparently rather than adding a named field.
#[derive(Resource, Deserialize, Default)]
#[serde(transparent)]
pub struct RelationshipsReply(pub Vec<RemoteEntity>);

/// Iterations run through `graph::force_layout` on every rebuild. The
/// hierarchy is polled slowly and rebuilt only on change, so there is no
/// per-frame cost to spend here.
const FORCE_ITERATIONS: usize = 200;

fn extract_name(entity: &RemoteEntity) -> Option<String> {
    entity
        .components
        .get("bevy_ecs::name::Name")
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn extract_parent(entity: &RemoteEntity) -> Option<u64> {
    entity
        .components
        .get("bevy_ecs::hierarchy::ChildOf")
        .and_then(serde_json::Value::as_u64)
}

/// Derive hierarchy nodes and child->parent edges from a snapshot reply.
/// Nodes are entities that appear as a parent or a child (an entity with
/// neither is not part of any relationship and is left out); edges index
/// into the returned node list.
pub fn hierarchy_from_reply(reply: &RelationshipsReply) -> (Vec<NodeInfo>, Vec<(usize, usize)>) {
    let entities = &reply.0;

    let mut parent_of: HashMap<u64, u64> = HashMap::new();
    for entity in entities {
        if let Some(parent_bits) = extract_parent(entity) {
            parent_of.insert(entity.entity, parent_bits);
        }
    }
    let is_a_parent: HashSet<u64> = parent_of.values().copied().collect();

    let included: Vec<&RemoteEntity> = entities
        .iter()
        .filter(|e| parent_of.contains_key(&e.entity) || is_a_parent.contains(&e.entity))
        .collect();

    let index_of: HashMap<u64, usize> = included
        .iter()
        .enumerate()
        .map(|(index, e)| (e.entity, index))
        .collect();

    let nodes: Vec<NodeInfo> = included
        .iter()
        .map(|e| {
            let name = extract_name(e);
            let kind = if !parent_of.contains_key(&e.entity) {
                NodeKind::Root
            } else if name.is_some() {
                NodeKind::Named
            } else {
                NodeKind::Anonymous
            };
            let label = name.unwrap_or_else(|| format!("Entity {:X}", e.entity));
            NodeInfo { label, kind }
        })
        .collect();

    let edges: Vec<(usize, usize)> = included
        .iter()
        .enumerate()
        .filter_map(|(child_index, e)| {
            let parent_bits = parent_of.get(&e.entity)?;
            let parent_index = index_of.get(parent_bits)?;
            Some((child_index, *parent_index))
        })
        .collect();

    (nodes, edges)
}

/// Node box background by hierarchy role: accent for a root, the normal
/// card surface for a named child, a dimmed tint for an anonymous one.
fn node_background(kind: NodeKind) -> Color {
    match kind {
        NodeKind::Root => tokens::ACCENT_BLUE.with_alpha(0.28),
        NodeKind::Named => tokens::COMPONENT_CARD_BG,
        NodeKind::Anonymous => Color::Srgba(tokens::TEXT_MUTED_COLOR).with_alpha(0.10),
    }
}

#[derive(Component)]
struct RelationshipsPanel;

#[derive(Component)]
pub(crate) struct RelMeta;

#[derive(Component)]
pub(crate) struct RelCanvas;

/// Build the relationships panel content (no header: the dock tab is the
/// title). Draggable node positions are a future improvement; v1 rebuilds
/// the force layout fresh every time the reply changes.
pub fn relationships_panel_content() -> impl Bundle {
    (
        RelationshipsPanel,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (
                RelMeta,
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
            ),
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    ..default()
                },
                children![(
                    RelCanvas,
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                )],
            ),
        ],
    )
}

/// Rebuild the meta line and the force-laid-out canvas when a new reply
/// arrives or the panel opens.
pub(crate) fn rebuild_relationships(
    reply: Option<Res<RelationshipsReply>>,
    mut commands: Commands,
    mut materials: ResMut<Assets<GraphEdgeMaterial>>,
    meta_containers: Query<Entity, With<RelMeta>>,
    canvas_containers: Query<Entity, With<RelCanvas>>,
    new_ui: Query<(), Or<(Added<RelMeta>, Added<RelCanvas>)>>,
) {
    let reply_changed = matches!(reply.as_ref(), Some(r) if r.is_changed());
    if !reply_changed && new_ui.is_empty() {
        return;
    }

    let (nodes, edges) = match reply.as_ref() {
        Some(r) => hierarchy_from_reply(r),
        None => (Vec::new(), Vec::new()),
    };

    let meta_right = format!("{} entities, {} links", nodes.len(), edges.len());
    for container in &meta_containers {
        commands.entity(container).despawn_children();
        commands.spawn((
            style::panel_meta("Entity hierarchy (ChildOf)", &meta_right),
            ChildOf(container),
        ));
    }

    for container in &canvas_containers {
        commands.entity(container).despawn_children();
        if nodes.is_empty() {
            continue;
        }
        let positions = graph::force_layout(nodes.len(), &edges, FORCE_ITERATIONS);
        let node_specs: Vec<GraphNodeSpec> = nodes
            .iter()
            .map(|n| GraphNodeSpec {
                label: n.label.clone(),
            })
            .collect();
        let node_colors: Vec<Color> = nodes.iter().map(|n| node_background(n.kind)).collect();
        graph::spawn_graph_positioned(
            &mut commands,
            &mut materials,
            container,
            &node_specs,
            &positions,
            &edges,
            &[],
            &node_colors,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(bits: u64, parent: Option<u64>, name: Option<&str>) -> serde_json::Value {
        let mut components = serde_json::Map::new();
        if let Some(parent) = parent {
            components.insert(
                "bevy_ecs::hierarchy::ChildOf".to_string(),
                serde_json::json!(parent),
            );
        }
        if let Some(name) = name {
            components.insert("bevy_ecs::name::Name".to_string(), serde_json::json!(name));
        }
        serde_json::json!({ "entity": bits, "components": components, "scene_node_id": null })
    }

    #[test]
    fn hierarchy_parses_roots_children_and_drops_isolated_entities() {
        let value = serde_json::json!([
            entity(1, None, None),
            entity(2, Some(1), Some("Child")),
            entity(3, Some(1), None),
            entity(4, None, None),
        ]);
        let reply: RelationshipsReply = serde_json::from_value(value).unwrap();
        let (nodes, edges) = hierarchy_from_reply(&reply);

        assert_eq!(
            nodes.len(),
            3,
            "entity 4 has no parent or child, so it is dropped"
        );
        assert_eq!(nodes[0].label, "Entity 1");
        assert_eq!(nodes[0].kind, NodeKind::Root);
        assert_eq!(nodes[1].label, "Child");
        assert_eq!(nodes[1].kind, NodeKind::Named);
        assert_eq!(nodes[2].label, "Entity 3");
        assert_eq!(nodes[2].kind, NodeKind::Anonymous);

        let mut sorted_edges = edges;
        sorted_edges.sort_unstable();
        assert_eq!(sorted_edges, vec![(1, 0), (2, 0)]);
    }

    #[test]
    fn hierarchy_from_empty_reply_is_empty() {
        let reply = RelationshipsReply::default();
        let (nodes, edges) = hierarchy_from_reply(&reply);
        assert!(nodes.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn reply_deserializes_from_bare_json_array() {
        let value = serde_json::json!([entity(7, None, Some("Solo"))]);
        let reply: RelationshipsReply = serde_json::from_value(value).unwrap();
        assert_eq!(reply.0.len(), 1);
        assert_eq!(reply.0[0].entity, 7);
    }
}
