//! Editor commands for node-graph mutations.
//!
//! Every interactive mutation routes through one of these commands so
//! `jackdaw_commands::CommandHistory` can undo/redo it. Commands mutate
//! the data layer; UI is kept in sync by systems in `crate::sync`.

use bevy::prelude::*;
use jackdaw_commands::EditorCommand;

use crate::graph::{Connection, GraphNode, Terminal, TerminalDirection};
use crate::registry::NodeTypeRegistry;

/// Move one or more graph nodes by absolute position.
///
/// `moves[i]` is `(entity, old_position, new_position)`. Executing writes
/// `new` into `GraphNode::position`; undo writes `old`. The UI position
/// reacts via `apply_node_position` in [`crate::node_widget`].
pub struct MoveGraphNodesCmd {
    pub moves: Vec<(Entity, Vec2, Vec2)>,
}

impl EditorCommand for MoveGraphNodesCmd {
    fn execute(&mut self, world: &mut World) {
        for &(entity, _old, new) in &self.moves {
            if let Some(mut node) = world.get_mut::<GraphNode>(entity) {
                node.position = new;
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        for &(entity, old, _new) in &self.moves {
            if let Some(mut node) = world.get_mut::<GraphNode>(entity) {
                node.position = old;
            }
        }
    }

    fn description(&self) -> &str {
        "Move graph nodes"
    }
}

/// Add a new graph node to a graph at a canvas-space position.
///
/// Execute spawns the data entity with its `GraphNode` + `Terminal` children;
/// undo despawns them. The sync system (Phase 2) spawns/despawns the UI.
pub struct AddGraphNodeCmd {
    pub graph: Entity,
    pub node_type: String,
    pub position: Vec2,
    // Populated during execute so undo can tear down the right entities.
    spawned: Option<Entity>,
}

impl AddGraphNodeCmd {
    pub fn new(graph: Entity, node_type: impl Into<String>, position: Vec2) -> Self {
        Self {
            graph,
            node_type: node_type.into(),
            position,
            spawned: None,
        }
    }

    pub fn spawned(&self) -> Option<Entity> {
        self.spawned
    }
}

impl EditorCommand for AddGraphNodeCmd {
    fn execute(&mut self, world: &mut World) {
        // Extract the descriptor up-front so we can drop the borrow before
        // mutating the world.
        let descriptor = world
            .resource::<NodeTypeRegistry>()
            .get(&self.node_type)
            .cloned();
        let Some(descriptor) = descriptor else {
            warn!("AddGraphNodeCmd: unknown node type '{}'", self.node_type);
            return;
        };

        let data_entity = world
            .spawn((
                GraphNode {
                    node_type: self.node_type.clone(),
                    position: self.position,
                },
                ChildOf(self.graph),
            ))
            .id();

        // Spawn terminals as children of the data node.
        for (idx, input) in descriptor.inputs.iter().enumerate() {
            world.spawn((
                Terminal {
                    direction: TerminalDirection::Input,
                    data_type: input.data_type.clone(),
                    label: input.label.clone(),
                    index: idx as u32,
                },
                ChildOf(data_entity),
            ));
        }
        for (idx, output) in descriptor.outputs.iter().enumerate() {
            world.spawn((
                Terminal {
                    direction: TerminalDirection::Output,
                    data_type: output.data_type.clone(),
                    label: output.label.clone(),
                    index: idx as u32,
                },
                ChildOf(data_entity),
            ));
        }

        self.spawned = Some(data_entity);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned.take() {
            // Despawning the node also despawns all ChildOf descendants,
            // including its Terminal entities and the node-UI subtree.
            if let Ok(ec) = world.get_entity_mut(entity) {
                ec.despawn();
            }
        }
    }

    fn description(&self) -> &str {
        "Add graph node"
    }
}

/// Create a new connection between two terminals.
///
/// Caller must have already validated type compatibility. Execute spawns the
/// `Connection` entity; undo despawns it.
pub struct CreateConnectionCmd {
    pub graph: Entity,
    pub source_node: Entity,
    pub source_terminal: u32,
    pub target_node: Entity,
    pub target_terminal: u32,
    spawned: Option<Entity>,
}

impl CreateConnectionCmd {
    pub fn new(
        graph: Entity,
        source_node: Entity,
        source_terminal: u32,
        target_node: Entity,
        target_terminal: u32,
    ) -> Self {
        Self {
            graph,
            source_node,
            source_terminal,
            target_node,
            target_terminal,
            spawned: None,
        }
    }
}

impl EditorCommand for CreateConnectionCmd {
    fn execute(&mut self, world: &mut World) {
        let entity = world
            .spawn((
                Connection {
                    source_node: self.source_node,
                    source_terminal: self.source_terminal,
                    target_node: self.target_node,
                    target_terminal: self.target_terminal,
                },
                ChildOf(self.graph),
            ))
            .id();
        self.spawned = Some(entity);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned.take()
            && let Ok(ec) = world.get_entity_mut(entity)
        {
            ec.despawn();
        }
    }

    fn description(&self) -> &str {
        "Create connection"
    }
}

/// Remove one or more graph nodes and all incident connections.
///
/// Snapshot-based undo: we store enough state to respawn both the nodes
/// (with their terminals) and the connections that previously linked them.
pub struct RemoveGraphNodesCmd {
    pub graph: Entity,
    pub entities: Vec<Entity>,
    /// Snapshot populated during execute. Format:
    /// `(original_entity, node, terminals, incident_connections)`.
    snapshot: Vec<NodeSnapshot>,
}

struct NodeSnapshot {
    original: Entity,
    node: GraphNode,
    terminals: Vec<(u32, TerminalDirection, String, String)>,
    // (source_node, source_terminal, target_node, target_terminal); all by
    // original-entity ids, which may point to other nodes in the snapshot.
    incident: Vec<(Entity, u32, Entity, u32)>,
}

impl RemoveGraphNodesCmd {
    pub fn new(graph: Entity, entities: Vec<Entity>) -> Self {
        Self {
            graph,
            entities,
            snapshot: Vec::new(),
        }
    }
}

impl EditorCommand for RemoveGraphNodesCmd {
    fn execute(&mut self, world: &mut World) {
        self.snapshot.clear();

        // Snapshot nodes + their terminals.
        for &entity in &self.entities {
            let Some(node) = world.get::<GraphNode>(entity).cloned() else {
                continue;
            };

            // Gather terminal metadata before despawn.
            let mut terminals = Vec::new();
            if let Some(children) = world.get::<Children>(entity) {
                let child_entities: Vec<Entity> = children.iter().collect();
                for child in child_entities {
                    if let Some(term) = world.get::<Terminal>(child) {
                        terminals.push((
                            term.index,
                            term.direction,
                            term.data_type.clone(),
                            term.label.clone(),
                        ));
                    }
                }
            }

            // Collect every Connection that touches this node.
            let mut incident = Vec::new();
            let mut conn_query = world.query::<(&Connection, Entity)>();
            for (conn, _) in conn_query.iter(world) {
                if conn.source_node == entity || conn.target_node == entity {
                    incident.push((
                        conn.source_node,
                        conn.source_terminal,
                        conn.target_node,
                        conn.target_terminal,
                    ));
                }
            }

            self.snapshot.push(NodeSnapshot {
                original: entity,
                node,
                terminals,
                incident,
            });
        }

        // Despawn connections touching any removed node.
        let to_remove_set: std::collections::HashSet<Entity> =
            self.entities.iter().copied().collect();
        let connections_to_remove: Vec<Entity> = {
            let mut q = world.query::<(Entity, &Connection)>();
            q.iter(world)
                .filter_map(|(e, c)| {
                    if to_remove_set.contains(&c.source_node)
                        || to_remove_set.contains(&c.target_node)
                    {
                        Some(e)
                    } else {
                        None
                    }
                })
                .collect()
        };
        for e in connections_to_remove {
            if let Ok(ec) = world.get_entity_mut(e) {
                ec.despawn();
            }
        }

        // Despawn the nodes.
        for &entity in &self.entities {
            if let Ok(ec) = world.get_entity_mut(entity) {
                ec.despawn();
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Respawn nodes. Since we can't reuse original Entity ids, we build
        // an id-remap so incident connections can be re-targeted at the new
        // node entities.
        let mut remap: std::collections::HashMap<Entity, Entity> = std::collections::HashMap::new();

        for snap in &self.snapshot {
            let new_entity = world.spawn((snap.node.clone(), ChildOf(self.graph))).id();
            remap.insert(snap.original, new_entity);

            for (idx, direction, data_type, label) in &snap.terminals {
                world.spawn((
                    Terminal {
                        direction: *direction,
                        data_type: data_type.clone(),
                        label: label.clone(),
                        index: *idx,
                    },
                    ChildOf(new_entity),
                ));
            }
        }

        // Respawn the connections that touched these nodes. A connection is
        // added back only if *both* endpoints are restored; otherwise it
        // belonged to a node that wasn't part of this undo (and was already
        // despawned earlier).
        let mut seen = std::collections::HashSet::<(Entity, u32, Entity, u32)>::new();
        for snap in &self.snapshot {
            for &(src, st, tgt, tt) in &snap.incident {
                let Some(&new_src) = remap.get(&src) else {
                    continue;
                };
                let Some(&new_tgt) = remap.get(&tgt) else {
                    continue;
                };
                let key = (new_src, st, new_tgt, tt);
                if !seen.insert(key) {
                    continue;
                }
                world.spawn((
                    Connection {
                        source_node: new_src,
                        source_terminal: st,
                        target_node: new_tgt,
                        target_terminal: tt,
                    },
                    ChildOf(self.graph),
                ));
            }
        }

        self.snapshot.clear();
    }

    fn description(&self) -> &str {
        "Remove graph nodes"
    }
}

/// Remove a single connection.
pub struct RemoveConnectionCmd {
    pub connection: Entity,
    snapshot: Option<Connection>,
    graph: Entity,
}

impl RemoveConnectionCmd {
    pub fn new(graph: Entity, connection: Entity) -> Self {
        Self {
            graph,
            connection,
            snapshot: None,
        }
    }
}

impl EditorCommand for RemoveConnectionCmd {
    fn execute(&mut self, world: &mut World) {
        self.snapshot = world.get::<Connection>(self.connection).cloned();
        if let Ok(ec) = world.get_entity_mut(self.connection) {
            ec.despawn();
        }
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(conn) = self.snapshot.take() {
            world.spawn((conn, ChildOf(self.graph)));
        }
    }

    fn description(&self) -> &str {
        "Remove connection"
    }
}
