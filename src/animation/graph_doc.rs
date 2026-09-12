//! The animation graph the Graph window is editing.
//!
//! One graph is open at a time: the file it came from, the canvas entities
//! that show it, and the edits that write it back. The definition in
//! [`AnimationGraphDoc`] is what saving writes, and the canvas is kept in step
//! with it in both directions -- an operator writes the definition and the
//! canvas follows, while a drag, a delete or a wire drawn on the canvas is
//! read back into the definition under whatever the node canvas already put in
//! the undo history.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw_animation_runtime::{
    AnimationClipRef, AnimationGraphDef, AnimationGraphState, AnimationMotion,
    AnimationParameterDef, AnimationTransitionDef, parse_animation_graph,
};
use jackdaw_bsn::{BsnPatches, SceneBsnAst, component_to_bsn_patch, emit_scene};
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_node_graph::{
    Connection, GraphCanvasView, GraphNode, NodeGraph, Terminal, TerminalDirection,
};

use crate::project::ProjectRoot;

/// Directory a graph with no folder in mind is created in.
pub const GRAPH_DIR: &str = "animation";

/// Suffix a graph created with no name in mind carries. A graph file is known
/// by the type it holds, so a graph in any folder under any name is one too.
pub const GRAPH_FILE_SUFFIX: &str = ".animgraph.bsn";

/// Registry key of the node one state is drawn as.
pub const STATE_NODE_TYPE: &str = "anim.state";

/// Registry key of the node every "from any state" transition leaves.
pub const ANY_STATE_NODE_TYPE: &str = "anim.any_state";

/// Where the first node of a laid-out graph sits.
const LAYOUT_ORIGIN: Vec2 = Vec2::new(60.0, 60.0);

/// How far apart laid-out nodes stand.
const LAYOUT_STEP: Vec2 = Vec2::new(260.0, 150.0);

/// Nodes per column of a laid-out graph.
const LAYOUT_ROWS: usize = 4;

/// How much of an undone edit is kept, so a state or a transition the canvas
/// took away comes back with what it held rather than with defaults.
const RETIRED_LIMIT: usize = 128;

/// The graph the Graph window is editing.
#[derive(Resource, Default, Debug)]
pub struct AnimationGraphDoc {
    /// Assets-relative path of the open file, or `None` when nothing is open.
    pub path: Option<String>,
    /// The states, transitions and parameters saving writes.
    pub def: AnimationGraphDef,
    /// The [`NodeGraph`] entity the canvas draws.
    pub graph: Option<Entity>,
    /// Whether the definition has moved on from the file.
    pub dirty: bool,
    /// Where the "Any State" node sits, which no state owns.
    pub any_state_position: Vec2,
    /// The node entity showing each state, by state name.
    nodes: HashMap<String, Entity>,
    /// The wire entity showing each transition, in `def.transitions` order.
    wires: Vec<Entity>,
    /// States the canvas took away, which an undo brings back.
    retired_states: Vec<AnimationGraphState>,
    /// Transitions the canvas took away, matched back by their ends.
    retired_transitions: Vec<AnimationTransitionDef>,
}

impl AnimationGraphDoc {
    /// The state of this name, if the graph holds one.
    pub fn state(&self, name: &str) -> Option<&AnimationGraphState> {
        self.def.states.iter().find(|state| state.name == name)
    }

    /// The wire drawing the transition at `at`, while one is on the canvas.
    pub fn wire(&self, at: usize) -> Option<Entity> {
        self.wires.get(at).copied()
    }

    /// A state name the graph does not already use.
    pub fn unused_state_name(&self, wanted: &str) -> String {
        let wanted = if wanted.is_empty() { "State" } else { wanted };
        if self.state(wanted).is_none() {
            return wanted.to_string();
        }
        (2..)
            .map(|n| format!("{wanted} {n}"))
            .find(|name| self.state(name).is_none())
            .unwrap_or_else(|| wanted.to_string())
    }

    /// Where a node added now should sit, clear of the ones already placed.
    pub fn free_position(&self) -> Vec2 {
        layout_position(self.def.states.len())
    }

    fn retire_state(&mut self, state: AnimationGraphState) {
        if self.retired_states.len() >= RETIRED_LIMIT {
            self.retired_states.remove(0);
        }
        self.retired_states.push(state);
    }

    fn retire_transition(&mut self, transition: AnimationTransitionDef) {
        if self.retired_transitions.len() >= RETIRED_LIMIT {
            self.retired_transitions.remove(0);
        }
        self.retired_transitions.push(transition);
    }

    /// The state a node standing at `position` was, when the canvas took one
    /// away from there. Undo respawns a node at the position it held, which is
    /// what carries a restored state's name and clip back to it.
    fn reclaim_state(&mut self, position: Vec2) -> Option<AnimationGraphState> {
        let at = self
            .retired_states
            .iter()
            .rposition(|state| state.position == position)?;
        Some(self.retired_states.remove(at))
    }

    fn reclaim_transition(&mut self, from: &str, to: &str) -> Option<AnimationTransitionDef> {
        let at = self
            .retired_transitions
            .iter()
            .rposition(|held| held.from == from && held.to == to)?;
        Some(self.retired_transitions.remove(at))
    }
}

/// The state a canvas node stands for.
#[derive(Component, Debug, Clone)]
pub struct GraphStateNode(pub String);

/// The node every "from any state" transition leaves.
#[derive(Component, Debug)]
pub struct GraphAnyStateNode;

/// Marker on everything the Graph window spawns onto the canvas.
#[derive(Component, Debug)]
pub struct GraphCanvasPart;

/// The file an assets-relative graph path names, refusing one that climbs out
/// of the project.
pub fn graph_file_path(project: &ProjectRoot, path: &str) -> Option<PathBuf> {
    match crate::project::path_within(&project.assets_dir(), Path::new(path)) {
        Ok(file) => Some(file),
        Err(err) => {
            warn!("{err}");
            None
        }
    }
}

/// The assets-relative path a graph of this name saves to.
pub fn graph_path_for_name(name: &str) -> String {
    format!(
        "{GRAPH_DIR}/{}{GRAPH_FILE_SUFFIX}",
        sanitize_graph_name(name)
    )
}

/// A file stem out of whatever the operator was given.
pub fn sanitize_graph_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() {
        "graph".to_string()
    } else {
        cleaned
    }
}

/// Every graph file the project holds, wherever it sits, as an assets-relative
/// path, sorted.
pub fn graph_files(world: &World) -> Vec<String> {
    let Some(index) = world.get_resource::<crate::asset_index::AssetIndex>() else {
        return Vec::new();
    };
    let mut paths = index.paths_of_kind(crate::definition_assets::ANIMATION_GRAPH_KIND);
    paths.sort();
    paths
}

/// The graph text of a definition, in the shape the loader reads back.
pub fn graph_to_bsn(def: &AnimationGraphDef, registry: &TypeRegistry) -> String {
    let mut ast = SceneBsnAst::default();
    let patch = component_to_bsn_patch(def.as_partial_reflect(), registry);
    let patch_entity = ast.world.spawn(patch).id();
    let root = ast.world.spawn(BsnPatches(vec![patch_entity])).id();
    ast.add_to_roots(root);
    emit_scene(&ast)
}

/// Read a graph file into a definition.
pub fn read_graph_file(world: &World, path: &str) -> Option<AnimationGraphDef> {
    let file = graph_file_path(world.get_resource::<ProjectRoot>()?, path)?;
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) => {
            warn!("could not read {}: {err}", file.display());
            return None;
        }
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    match parse_animation_graph(&text, &registry) {
        Ok(def) => Some(def),
        Err(err) => {
            warn!("could not read {}: {err}", file.display());
            None
        }
    }
}

/// Write the open graph back to its file.
pub fn write_graph_file(world: &mut World) -> Option<PathBuf> {
    let path = world.resource::<AnimationGraphDoc>().path.clone()?;
    let file = graph_file_path(world.get_resource::<ProjectRoot>()?, &path)?;
    let text = {
        let registry = world.resource::<AppTypeRegistry>().read();
        let body = graph_to_bsn(&world.resource::<AnimationGraphDoc>().def, &registry);
        crate::asset_files::asset_file_text(
            <AnimationGraphDef as bevy::reflect::TypePath>::type_path(),
            &body,
        )
    };
    if let Some(parent) = file.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        warn!("could not make {}: {err}", parent.display());
        return None;
    }
    if let Err(err) = crate::scene_io::save::write_atomic(&file, text.as_bytes()) {
        warn!("could not write {}: {err}", file.display());
        return None;
    }
    world.resource_mut::<AnimationGraphDoc>().dirty = false;
    Some(file)
}

/// Open a graph file on the canvas.
pub fn open_graph(world: &mut World, path: &str) -> bool {
    let Some(def) = read_graph_file(world, path) else {
        return false;
    };
    install_graph(world, path, def, false);
    true
}

/// Start a graph of this name, and write the empty file it saves to.
pub fn new_graph(world: &mut World, name: &str) -> Option<String> {
    let path = graph_path_for_name(name);
    install_graph(world, &path, AnimationGraphDef::default(), true);
    write_graph_file(world)?;
    Some(path)
}

/// Put a definition on the canvas as the open graph.
pub fn install_graph(world: &mut World, path: &str, mut def: AnimationGraphDef, dirty: bool) {
    let held = world.resource::<AnimationGraphDoc>();
    if held.dirty
        && let Some(open) = held.path.clone()
    {
        warn!("{open} had edits that were not saved, and is being closed");
    }
    lay_out_unplaced_states(&mut def);
    clear_canvas(world);
    let graph = spawn_graph_root(world, path);
    let mut doc = world.resource_mut::<AnimationGraphDoc>();
    doc.path = Some(path.to_string());
    doc.def = def;
    doc.graph = Some(graph);
    doc.dirty = dirty;
    doc.any_state_position = LAYOUT_ORIGIN - Vec2::new(0.0, LAYOUT_STEP.y);
    doc.nodes.clear();
    doc.wires.clear();
    doc.retired_states.clear();
    doc.retired_transitions.clear();
}

/// The entity the canvas draws one graph's nodes and wires under.
fn spawn_graph_root(world: &mut World, path: &str) -> Entity {
    let title = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(GRAPH_FILE_SUFFIX)
        .to_string();
    world
        .spawn((
            NodeGraph { title },
            GraphCanvasView::default(),
            GraphCanvasPart,
            Name::new(format!("Animation Graph {path}")),
            crate::EditorEntity,
        ))
        .id()
}

/// Take the open graph and everything it drew off the canvas.
pub fn clear_canvas(world: &mut World) {
    let held: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, With<GraphCanvasPart>>();
        query.iter(world).collect()
    };
    for entity in held {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    let mut doc = world.resource_mut::<AnimationGraphDoc>();
    doc.graph = None;
    doc.nodes.clear();
    doc.wires.clear();
}

/// Where the `index`th node of a laid-out graph sits.
fn layout_position(index: usize) -> Vec2 {
    LAYOUT_ORIGIN
        + Vec2::new(
            (index / LAYOUT_ROWS) as f32 * LAYOUT_STEP.x,
            (index % LAYOUT_ROWS) as f32 * LAYOUT_STEP.y,
        )
}

/// Place the states of a file that carries no layout, so a graph authored
/// before the canvas existed does not open as one stack of nodes.
fn lay_out_unplaced_states(def: &mut AnimationGraphDef) {
    if def.states.iter().any(|state| state.position != Vec2::ZERO) {
        return;
    }
    for (index, state) in def.states.iter_mut().enumerate() {
        state.position = layout_position(index);
    }
}

/// Change the open graph as one step of undo.
///
/// The edit is refused when nothing is open, and dropped when it leaves the
/// definition as it found it, so a control that writes what is already there
/// puts nothing on the history.
pub fn commit_graph_edit(
    world: &mut World,
    label: &'static str,
    edit: impl FnOnce(&mut AnimationGraphDef),
) -> bool {
    let doc = world.resource::<AnimationGraphDoc>();
    if doc.path.is_none() {
        return false;
    }
    let before = doc.def.clone();
    let mut after = before.clone();
    edit(&mut after);
    if after == before {
        return false;
    }
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(GraphDefEdit {
                before,
                after,
                label,
            }),
            world,
        );
    });
    true
}

/// One edit of the open graph's definition.
struct GraphDefEdit {
    before: AnimationGraphDef,
    after: AnimationGraphDef,
    label: &'static str,
}

impl GraphDefEdit {
    fn write(world: &mut World, def: &AnimationGraphDef) {
        {
            let mut doc = world.resource_mut::<AnimationGraphDoc>();
            doc.def = def.clone();
            doc.dirty = true;
        }
        resync_views(world);
    }
}

/// Match the canvas back to a definition that changed under it.
///
/// A node or a wire the definition no longer holds is taken off the canvas,
/// and what it does hold is claimed by whatever is already drawing it, so a
/// rename or an undo neither doubles a wire nor loses where a node stood.
fn resync_views(world: &mut World) {
    if world.resource::<AnimationGraphDoc>().graph.is_none() {
        return;
    }
    let names: Vec<String> = world
        .resource::<AnimationGraphDoc>()
        .def
        .states
        .iter()
        .map(|state| state.name.clone())
        .collect();
    let drawn: Vec<(Entity, String)> = {
        let mut query = world.query::<(Entity, &GraphStateNode)>();
        query
            .iter(world)
            .map(|(entity, state)| (entity, state.0.clone()))
            .collect()
    };
    let mut nodes: HashMap<String, Entity> = HashMap::new();
    for (entity, name) in drawn {
        if names.contains(&name) && !nodes.contains_key(&name) {
            nodes.insert(name, entity);
            continue;
        }
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    world.resource_mut::<AnimationGraphDoc>().nodes = nodes;

    let drawn_wires = wire_ends(world);
    let transitions: Vec<(String, String)> = world
        .resource::<AnimationGraphDoc>()
        .def
        .transitions
        .iter()
        .map(|transition| (transition.from.clone(), transition.to.clone()))
        .collect();
    let mut claimed: Vec<Entity> = Vec::new();
    let mut taken: Vec<Entity> = Vec::new();
    for (from, to) in transitions {
        let found = drawn_wires
            .iter()
            .find(|(entity, ends)| ends.0 == from && ends.1 == to && !taken.contains(entity))
            .map(|(entity, _)| *entity);
        match found {
            Some(entity) => {
                taken.push(entity);
                claimed.push(entity);
            }
            None => claimed.push(Entity::PLACEHOLDER),
        }
    }
    for (entity, _) in &drawn_wires {
        if taken.contains(entity) {
            continue;
        }
        if let Ok(entity) = world.get_entity_mut(*entity) {
            entity.despawn();
        }
    }
    world.resource_mut::<AnimationGraphDoc>().wires = claimed;
}

/// Every wire on the canvas and the state names its ends carry. A wire out of
/// the any-state node reads as leaving no state at all, which is how the
/// runtime spells a transition taken from everywhere.
fn wire_ends(world: &mut World) -> Vec<(Entity, (String, String))> {
    let Some(graph) = world.resource::<AnimationGraphDoc>().graph else {
        return Vec::new();
    };
    let named: HashMap<Entity, String> = {
        let mut query = world.query::<(Entity, &GraphStateNode)>();
        query
            .iter(world)
            .map(|(entity, state)| (entity, state.0.clone()))
            .collect()
    };
    let any_state: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, With<GraphAnyStateNode>>();
        query.iter(world).collect()
    };
    let mut wires = world.query::<(Entity, &Connection, &ChildOf)>();
    wires
        .iter(world)
        .filter(|(_, _, parent)| parent.parent() == graph)
        .filter_map(|(entity, connection, _)| {
            let from = match named.get(&connection.source_node) {
                Some(name) => name.clone(),
                None if any_state.contains(&connection.source_node) => String::new(),
                None => return None,
            };
            let to = named.get(&connection.target_node)?.clone();
            Some((entity, (from, to)))
        })
        .collect()
}

impl EditorCommand for GraphDefEdit {
    fn execute(&mut self, world: &mut World) {
        Self::write(world, &self.after);
    }

    fn undo(&mut self, world: &mut World) {
        Self::write(world, &self.before);
    }

    fn description(&self) -> &str {
        self.label
    }
}

/// Read what the canvas did back into the definition.
///
/// A node dragged, deleted or added and a wire drawn or cut all land on the
/// canvas first, under the node canvas's own undo entry. This follows them, so
/// the definition saving writes is what the canvas shows and one undo takes
/// back one edit. A canvas whose root went away is given a new one, which the
/// reconcile then draws the definition into again.
pub(super) fn read_canvas_back(world: &mut World) {
    let doc = world.resource::<AnimationGraphDoc>();
    let (Some(path), Some(graph)) = (doc.path.clone(), doc.graph) else {
        return;
    };
    if !world.entities().contains(graph) {
        clear_canvas(world);
        let graph = spawn_graph_root(world, &path);
        world.resource_mut::<AnimationGraphDoc>().graph = Some(graph);
        return;
    }
    read_states_back(world, graph);
    read_transitions_back(world);
}

fn read_states_back(world: &mut World, graph: Entity) {
    let mut live: HashMap<String, Entity> = HashMap::new();
    let mut positions: HashMap<Entity, Vec2> = HashMap::new();
    let mut unnamed: Vec<(Entity, Vec2)> = Vec::new();
    {
        let mut query = world.query::<(Entity, &GraphNode, &ChildOf, Option<&GraphStateNode>)>();
        for (entity, node, parent, state) in query.iter(world) {
            if parent.parent() != graph || node.node_type != STATE_NODE_TYPE {
                continue;
            }
            positions.insert(entity, node.position);
            match state {
                Some(state) => {
                    live.insert(state.0.clone(), entity);
                }
                None => unnamed.push((entity, node.position)),
            }
        }
    }

    let gone: Vec<String> = world
        .resource::<AnimationGraphDoc>()
        .nodes
        .iter()
        .filter(|(name, entity)| {
            !live.contains_key(name.as_str()) && !world.entities().contains(**entity)
        })
        .map(|(name, _)| name.clone())
        .collect();

    let mut doc = world.resource_mut::<AnimationGraphDoc>();
    let mut changed = false;
    for state in &mut doc.def.states {
        let Some(entity) = live.get(&state.name) else {
            continue;
        };
        let Some(&position) = positions.get(entity) else {
            continue;
        };
        if state.position != position {
            state.position = position;
            changed = true;
        }
    }

    for name in gone {
        doc.nodes.remove(&name);
        let Some(at) = doc.def.states.iter().position(|state| state.name == name) else {
            continue;
        };
        let state = doc.def.states.remove(at);
        doc.retire_state(state);
        let dropped: Vec<AnimationTransitionDef> = doc
            .def
            .transitions
            .iter()
            .filter(|transition| transition.from == name || transition.to == name)
            .cloned()
            .collect();
        doc.def
            .transitions
            .retain(|transition| transition.from != name && transition.to != name);
        for transition in dropped {
            doc.retire_transition(transition);
        }
        changed = true;
    }

    let mut adopted: Vec<(Entity, String)> = Vec::new();
    for (entity, position) in unnamed {
        let state = doc.reclaim_state(position).unwrap_or_else(|| {
            let name = doc.unused_state_name("State");
            AnimationGraphState {
                name,
                position,
                ..AnimationGraphState::default()
            }
        });
        adopted.push((entity, state.name.clone()));
        doc.nodes.insert(state.name.clone(), entity);
        doc.def.states.push(state);
        changed = true;
    }
    if changed {
        doc.dirty = true;
    }
    for (entity, name) in adopted {
        world.entity_mut(entity).insert(GraphStateNode(name));
    }
    if changed {
        resync_views(world);
    }
}

fn read_transitions_back(world: &mut World) {
    let drawn = wire_ends(world);
    let mut unclaimed: Vec<Entity> = drawn.iter().map(|(entity, _)| *entity).collect();

    let mut doc = world.resource_mut::<AnimationGraphDoc>();
    let mut changed = false;
    let mut kept: Vec<Entity> = Vec::new();
    let mut at = 0;
    while at < doc.def.transitions.len() {
        let wire = doc.wires.get(at).copied().unwrap_or(Entity::PLACEHOLDER);
        if wire == Entity::PLACEHOLDER {
            kept.push(wire);
            at += 1;
            continue;
        }
        if unclaimed.contains(&wire) {
            unclaimed.retain(|entity| *entity != wire);
            kept.push(wire);
            at += 1;
            continue;
        }
        let transition = doc.def.transitions.remove(at);
        doc.retire_transition(transition);
        changed = true;
    }
    doc.wires = kept;

    for entity in unclaimed {
        let Some((from, to)) = drawn
            .iter()
            .find(|(held, _)| *held == entity)
            .map(|(_, ends)| ends.clone())
        else {
            continue;
        };
        let transition =
            doc.reclaim_transition(&from, &to)
                .unwrap_or_else(|| AnimationTransitionDef {
                    from,
                    to,
                    ..AnimationTransitionDef::default()
                });
        doc.def.transitions.push(transition);
        doc.wires.push(entity);
        changed = true;
    }
    if changed {
        doc.dirty = true;
    }
}

/// Draw whatever the definition holds and the canvas does not.
pub(super) fn reconcile_canvas(world: &mut World) {
    let Some(graph) = world.resource::<AnimationGraphDoc>().graph else {
        return;
    };
    if !world.entities().contains(graph) {
        return;
    }
    reconcile_state_nodes(world, graph);
    reconcile_any_state_node(world, graph);
    reconcile_wires(world, graph);
}

fn reconcile_state_nodes(world: &mut World, graph: Entity) {
    let live: HashMap<String, Entity> = {
        let mut query = world.query::<(Entity, &GraphStateNode)>();
        query
            .iter(world)
            .map(|(entity, state)| (state.0.clone(), entity))
            .collect()
    };
    let wanted: Vec<(String, Vec2)> = world
        .resource::<AnimationGraphDoc>()
        .def
        .states
        .iter()
        .map(|state| (state.name.clone(), state.position))
        .collect();

    for (name, entity) in &live {
        if wanted.iter().any(|(held, _)| held == name) {
            continue;
        }
        if let Ok(entity) = world.get_entity_mut(*entity) {
            entity.despawn();
        }
    }
    for (name, position) in wanted {
        if live.contains_key(&name) {
            continue;
        }
        let entity = spawn_canvas_node(world, graph, STATE_NODE_TYPE, position, true);
        world
            .entity_mut(entity)
            .insert(GraphStateNode(name.clone()));
        world
            .resource_mut::<AnimationGraphDoc>()
            .nodes
            .insert(name, entity);
    }
}

fn reconcile_any_state_node(world: &mut World, graph: Entity) {
    let wanted = world
        .resource::<AnimationGraphDoc>()
        .def
        .transitions
        .iter()
        .any(|transition| transition.from.is_empty());
    let held: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, With<GraphAnyStateNode>>();
        query.iter(world).collect()
    };
    let drawn = !held.is_empty();
    if wanted == drawn {
        return;
    }
    if !wanted {
        for entity in held {
            if let Ok(entity) = world.get_entity_mut(entity) {
                entity.despawn();
            }
        }
        return;
    }
    let position = world.resource::<AnimationGraphDoc>().any_state_position;
    let entity = spawn_canvas_node(world, graph, ANY_STATE_NODE_TYPE, position, false);
    world.entity_mut(entity).insert(GraphAnyStateNode);
}

fn reconcile_wires(world: &mut World, graph: Entity) {
    let named: HashMap<String, Entity> = {
        let mut query = world.query::<(Entity, &GraphStateNode)>();
        query
            .iter(world)
            .map(|(entity, state)| (state.0.clone(), entity))
            .collect()
    };
    let any_state = {
        let mut query = world.query_filtered::<Entity, With<GraphAnyStateNode>>();
        query.iter(world).next()
    };
    let count = world.resource::<AnimationGraphDoc>().def.transitions.len();
    for at in 0..count {
        let held = world.resource::<AnimationGraphDoc>().wires.get(at).copied();
        if held.is_some_and(|wire| world.entities().contains(wire)) {
            continue;
        }
        let transition = world.resource::<AnimationGraphDoc>().def.transitions[at].clone();
        let source = if transition.from.is_empty() {
            any_state
        } else {
            named.get(&transition.from).copied()
        };
        let (Some(source), Some(&target)) = (source, named.get(&transition.to)) else {
            continue;
        };
        let wire = world
            .spawn((
                Connection {
                    source_node: source,
                    source_terminal: 0,
                    target_node: target,
                    target_terminal: 0,
                },
                GraphCanvasPart,
                ChildOf(graph),
            ))
            .id();
        let mut doc = world.resource_mut::<AnimationGraphDoc>();
        while doc.wires.len() <= at {
            doc.wires.push(Entity::PLACEHOLDER);
        }
        doc.wires[at] = wire;
    }
}

/// Spawn one canvas node with the terminals its type carries.
fn spawn_canvas_node(
    world: &mut World,
    graph: Entity,
    node_type: &str,
    position: Vec2,
    takes_input: bool,
) -> Entity {
    let entity = world
        .spawn((
            GraphNode {
                node_type: node_type.to_string(),
                position,
            },
            GraphCanvasPart,
            ChildOf(graph),
        ))
        .id();
    if takes_input {
        world.spawn((
            Terminal {
                direction: TerminalDirection::Input,
                data_type: STATE_TERMINAL.to_string(),
                label: "in".to_string(),
                index: 0,
            },
            ChildOf(entity),
        ));
    }
    world.spawn((
        Terminal {
            direction: TerminalDirection::Output,
            data_type: STATE_TERMINAL.to_string(),
            label: "out".to_string(),
            index: 0,
        },
        ChildOf(entity),
    ));
    entity
}

/// The terminal type a transition connects, which only another state accepts.
pub const STATE_TERMINAL: &str = "anim.state_flow";

/// Register the node types the graph canvas draws.
pub(super) fn register_graph_node_types(
    mut registry: ResMut<jackdaw_node_graph::NodeTypeRegistry>,
) {
    use jackdaw_node_graph::{NodeTypeDescriptor, TerminalDescriptor};

    let color = Color::srgb(0.55, 0.80, 0.95);
    let flow = |label: &str| TerminalDescriptor {
        label: label.into(),
        data_type: STATE_TERMINAL.into(),
        color,
    };
    registry.register(NodeTypeDescriptor {
        id: STATE_NODE_TYPE.into(),
        display_name: "State".into(),
        category: "Animation Graph".into(),
        accent_color: Color::srgb(0.38, 0.72, 1.0),
        inputs: vec![flow("in")],
        outputs: vec![flow("out")],
        body_components: Vec::new(),
    });
    registry.register(NodeTypeDescriptor {
        id: ANY_STATE_NODE_TYPE.into(),
        display_name: "Any State".into(),
        category: "Animation Graph".into(),
        accent_color: Color::srgb(0.95, 0.65, 0.35),
        inputs: Vec::new(),
        outputs: vec![flow("out")],
        body_components: Vec::new(),
    });
}

/// What a state's node says about the clip it plays.
pub fn motion_summary(motion: &AnimationMotion) -> String {
    match motion {
        AnimationMotion::Clip(clip) if clip.clip.is_empty() => "no clip".to_string(),
        AnimationMotion::Clip(clip) => clip.clip.clone(),
        AnimationMotion::Blend1d { parameter, points } => {
            format!("blend on {parameter}, {} clips", points.len())
        }
    }
}

/// A clip named as `<assets-relative file>#<clip name>`.
pub fn parse_clip_ref(spec: &str) -> Option<AnimationClipRef> {
    let (source, clip) = spec.rsplit_once('#')?;
    (!source.is_empty() && !clip.is_empty()).then(|| AnimationClipRef {
        source: source.to_string(),
        clip: clip.to_string(),
    })
}

/// What a wire says about the transition it draws.
pub fn transition_summary(transition: &AnimationTransitionDef) -> String {
    let mut parts: Vec<String> = transition
        .conditions
        .iter()
        .map(|condition| {
            format!(
                "{} {} {}",
                condition.parameter,
                condition_op_name(condition.op),
                trim_float(condition.value)
            )
        })
        .collect();
    if let Some(exit) = transition.exit_time {
        parts.push(format!("at {}", trim_float(exit)));
    }
    if parts.is_empty() {
        parts.push("always".to_string());
    }
    format!(
        "{} ({}s)",
        parts.join(if transition.require_all {
            " and "
        } else {
            " or "
        }),
        trim_float(transition.crossfade_secs)
    )
}

/// The spelling of an operator a condition is written with.
pub fn condition_op_name(op: jackdaw_animation_runtime::AnimationConditionOp) -> &'static str {
    use jackdaw_animation_runtime::AnimationConditionOp as Op;
    match op {
        Op::Greater => ">",
        Op::GreaterOrEqual => ">=",
        Op::Less => "<",
        Op::LessOrEqual => "<=",
        Op::Equal => "==",
        Op::NotEqual => "!=",
    }
}

/// The operator an authored condition spells.
pub fn condition_op_from_name(
    name: &str,
) -> Option<jackdaw_animation_runtime::AnimationConditionOp> {
    use jackdaw_animation_runtime::AnimationConditionOp as Op;
    Some(match name {
        ">" => Op::Greater,
        ">=" => Op::GreaterOrEqual,
        "<" => Op::Less,
        "<=" => Op::LessOrEqual,
        "==" | "=" => Op::Equal,
        "!=" => Op::NotEqual,
        _ => return None,
    })
}

/// A number with no trailing zeroes, which is what a node has room for.
fn trim_float(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() { "0" } else { text }.to_string()
}

/// The parameter kind an operator names.
pub fn parameter_kind_from_name(
    name: &str,
) -> Option<jackdaw_animation_runtime::AnimationParameterKind> {
    use jackdaw_animation_runtime::AnimationParameterKind as Kind;
    Some(match name {
        "float" => Kind::Float,
        "bool" => Kind::Bool,
        "trigger" => Kind::Trigger,
        _ => return None,
    })
}

/// The parameter of this name, if the graph declares one.
pub fn parameter<'a>(def: &'a AnimationGraphDef, name: &str) -> Option<&'a AnimationParameterDef> {
    def.parameters
        .iter()
        .find(|parameter| parameter.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_graph_name_becomes_a_file_stem_that_can_be_written() {
        assert_eq!(
            graph_path_for_name("Player Locomotion"),
            "animation/Player_Locomotion.animgraph.bsn"
        );
        assert_eq!(graph_path_for_name("///"), "animation/graph.animgraph.bsn");
    }

    /// The Graph window lists what the index holds, so a graph filed anywhere
    /// under the project is one it can open.
    #[test]
    fn a_graph_filed_outside_the_animation_folder_is_listed_and_reads_back() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut app = bevy::app::App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ));
        app.register_type::<AnimationGraphDef>();
        app.insert_resource(ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: crate::project::ProjectConfig::default(),
        });
        app.init_resource::<crate::asset_index::AssetIndex>();
        app.init_resource::<crate::asset_files::AssetKindCache>();
        app.init_resource::<jackdaw_api::prelude::AssetKinds>();
        app.world_mut()
            .resource_mut::<jackdaw_api::prelude::AssetKinds>()
            .register(jackdaw_api::prelude::AssetKind::compiled(
                crate::definition_assets::ANIMATION_GRAPH_KIND,
                "Animation Graph",
                <AnimationGraphDef as bevy::reflect::TypePath>::type_path(),
            ));
        let rigs = tmp.path().join("assets/rigs");
        std::fs::create_dir_all(&rigs).expect("the folder is made");
        std::fs::write(
            rigs.join("hero.bsn"),
            "#hero\njackdaw_animation_runtime::graph::AnimationGraphDef { entry: \"idle\" }\n",
        )
        .expect("the file is written");

        crate::asset_index::rescan_asset_index(app.world_mut());

        assert_eq!(graph_files(app.world()), vec!["rigs/hero.bsn".to_string()]);
        let def = read_graph_file(app.world(), "rigs/hero.bsn").expect("the graph reads back");
        assert_eq!(def.entry, "idle");
    }

    #[test]
    fn a_file_with_no_layout_opens_as_a_grid_rather_than_a_stack() {
        let mut def = AnimationGraphDef {
            states: vec![
                AnimationGraphState {
                    name: "idle".into(),
                    ..AnimationGraphState::default()
                },
                AnimationGraphState {
                    name: "run".into(),
                    ..AnimationGraphState::default()
                },
            ],
            ..AnimationGraphDef::default()
        };
        lay_out_unplaced_states(&mut def);
        assert_ne!(def.states[0].position, def.states[1].position);
    }

    #[test]
    fn a_placed_file_keeps_the_layout_it_was_saved_with() {
        let placed = Vec2::new(12.0, 34.0);
        let mut def = AnimationGraphDef {
            states: vec![AnimationGraphState {
                name: "idle".into(),
                position: placed,
                ..AnimationGraphState::default()
            }],
            ..AnimationGraphDef::default()
        };
        lay_out_unplaced_states(&mut def);
        assert_eq!(def.states[0].position, placed);
    }
}
