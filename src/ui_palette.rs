//! Widget creation: one registered definition becomes one authored entity.
//!
//! [`instantiate_widget`] is the single path from a registered
//! [`WidgetDefinition`] to an authored entity: it decides which node adopts
//! the new widget, runs the definition, puts the result in the scene
//! document, selects it, and records one undo entry. The Add menu's UI
//! Widgets section calls that path.
//!
//! Registration in the document, rather than a bare `world.spawn`, is what
//! makes the widget visible to save, to undo, and to the outliner, which reads
//! document membership through [`crate::hierarchy`].

use std::sync::Arc;

use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::{WidgetDefinition, WidgetInstantiateContext, WidgetRegistry};
use jackdaw_commands::EditorCommand;

use crate::{EditorEntity, commands::CommandHistory, selection::Selection};

const TAB_GROUP_TYPE_PATH: &str = "bevy_input_focus::tab_navigation::TabGroup";

/// Add a registered widget to the open UI scene, as the Add menu's UI Widgets
/// rows do.
///
/// `name` is the definition id (`ui.button`), not an entity name. `parent` is
/// optional; left out, it defaults to the selection, and a selection outside
/// the UI scene falls back to the scene root.
///
/// Both failure modes are decided before anything is queued, so a request that
/// did not happen reports `Cancelled`. An unknown id is reported alongside the
/// ids that do exist.
#[operator(
    id = "widget.add",
    label = "Add Widget",
    description = "Add a registered UI widget to the open UI scene.",
    allows_undo = false,
    params(
        name(String, doc = "Widget definition id, e.g. \"ui.button\"."),
        parent(
            Entity,
            doc = "Node that adopts the widget. Defaults to the selection, then the UI scene root."
        ),
    )
)]
pub(crate) fn widget_add(
    params: In<OperatorParameters>,
    registry: Option<Res<WidgetRegistry>>,
    ui_scenes: Query<(), crate::prefab::AuthoredUiSceneRoot>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(definition_id) = params.as_str("name").map(str::to_string) else {
        warn!("widget.add: missing `name` parameter (a widget id, e.g. ui.button)");
        return OperatorResult::Cancelled;
    };
    if registry
        .as_ref()
        .and_then(|registry| registry.get(&definition_id))
        .is_none()
    {
        warn!(
            "widget.add: {}; the vocabulary is {}",
            PaletteError::UnknownDefinition(definition_id),
            registered_widget_ids(registry.as_deref())
        );
        return OperatorResult::Cancelled;
    }
    if ui_scenes.is_empty() {
        warn!("widget.add: {}", PaletteError::NoUiScene);
        return OperatorResult::Cancelled;
    }
    let parent = params.as_entity("parent");
    commands.queue(move |world: &mut World| {
        if let Err(error) = instantiate_widget_under(world, &definition_id, parent) {
            warn!("widget.add: could not add `{definition_id}`: {error}");
        }
    });
    OperatorResult::Finished
}

/// Every registered widget id, for the line that reports what an author can
/// write instead. Sorted, since the registry is a hash map and its iteration
/// order varies between runs.
fn registered_widget_ids(registry: Option<&WidgetRegistry>) -> String {
    let mut ids: Vec<&str> = registry
        .map(|registry| registry.iter().map(|definition| &*definition.id).collect())
        .unwrap_or_default();
    ids.sort_unstable();
    if ids.is_empty() {
        // The widget vocabulary ships as an extension, so an editor with it
        // switched off has no widgets at all.
        return "empty: no extension has registered a widget".to_string();
    }
    ids.join(", ")
}

/// Why a widget request produced no widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteError {
    /// No definition is registered under this id (a stale menu row, or an
    /// extension that unloaded between the click and the dispatch).
    UnknownDefinition(String),
    /// Nothing in the open document can hold a UI node.
    NoUiScene,
    /// The definition itself refused.
    Instantiate(String),
}

impl std::fmt::Display for PaletteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDefinition(id) => write!(formatter, "no widget definition `{id}`"),
            Self::NoUiScene => formatter
                .write_str("this document has no UI scene; open or add a UI scene root first"),
            Self::Instantiate(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PaletteError {}

/// Create the widget `definition_id` names, as one undoable step.
///
/// An unknown id, a document with no UI scene, and a definition that refuses
/// all return an error for the caller to report.
pub fn instantiate_widget(world: &mut World, definition_id: &str) -> Result<Entity, PaletteError> {
    let candidate = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    instantiate_widget_under(world, definition_id, candidate)
}

/// Create the widget `definition_id` names under `candidate`.
///
/// `candidate` is a preference, not a requirement: a node outside the open UI
/// scene cannot hold a UI node, so the scene root adopts the widget instead.
/// The same rule applies however the candidate arrived, from a selection or
/// from a caller naming one.
pub fn instantiate_widget_under(
    world: &mut World,
    definition_id: &str,
    candidate: Option<Entity>,
) -> Result<Entity, PaletteError> {
    let definition = world
        .get_resource::<WidgetRegistry>()
        .and_then(|registry| registry.get(definition_id))
        .ok_or_else(|| PaletteError::UnknownDefinition(definition_id.to_string()))?;
    let parent = widget_parent(world, candidate).ok_or(PaletteError::NoUiScene)?;
    backfill_focus_group(world);

    let mut command = InstantiateWidgetCommand {
        label: format!("Add {}", definition.name),
        definition,
        parent,
        spawned: None,
        error: None,
    };
    command.execute(world);
    if let Some(error) = command.error.take() {
        return Err(PaletteError::Instantiate(error));
    }
    let Some(entity) = command.spawned else {
        return Err(PaletteError::Instantiate(
            "the widget definition returned no entity".to_string(),
        ));
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
    Ok(entity)
}

/// The node a new widget is parented to: the selection when it is part of the
/// open UI scene, and the scene's root otherwise, so a 3D selection does not
/// block UI authoring. `None` means the document holds no UI scene, the one
/// case widget creation refuses.
pub fn resolve_widget_parent(world: &mut World) -> Option<Entity> {
    let candidate = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    widget_parent(world, candidate)
}

/// [`resolve_widget_parent`] with the candidate spelled out.
fn widget_parent(world: &mut World, candidate: Option<Entity>) -> Option<Entity> {
    let root = ui_scene_root(world)?;
    let inside = candidate.filter(|entity| is_in_ui_scene(world, *entity, root));
    Some(inside.unwrap_or(root))
}

/// The open UI scene's root. A document holds one, but a malformed one may hold
/// several; the lowest entity is picked so the choice is stable across a
/// session rather than following archetype order.
pub(crate) fn ui_scene_root(world: &mut World) -> Option<Entity> {
    world
        .query_filtered::<Entity, crate::prefab::AuthoredUiSceneRoot>()
        .iter(world)
        .min()
}

/// Give the open UI scene's root the focus group tab navigation gathers
/// from, unless it already declares one.
///
/// Tab navigation collects focusable nodes from a `TabGroup` ancestor, so a
/// screen whose root has none is unreachable by keyboard however many buttons
/// it holds. New scenes get theirs from [`seed_ui_scene_root`]; this covers
/// scenes authored without one, on their first added widget.
///
/// Idempotent: a root that declares its own order or modality keeps them. It
/// writes the document without an undo entry, as the seeding does, so undoing
/// the widget that triggered it does not remove keyboard reachability.
fn backfill_focus_group(world: &mut World) {
    let Some(root) = ui_scene_root(world) else {
        return;
    };
    if world.get::<TabGroup>(root).is_some() {
        return;
    }
    let group = TabGroup::default();
    world.entity_mut(root).insert(group);
    crate::commands::sync_component_to_ast(world, root, TAB_GROUP_TYPE_PATH, &group);
}

/// Whether `entity` is `root` or one of its descendants, and is an authored
/// node rather than editor chrome that happens to be parented into the tree.
fn is_in_ui_scene(world: &World, entity: Entity, root: Entity) -> bool {
    if world.get_entity(entity).is_err() || world.get::<EditorEntity>(entity).is_some() {
        return false;
    }
    let mut current = entity;
    loop {
        if current == root {
            return true;
        }
        match world.get::<ChildOf>(current) {
            Some(parent) => current = parent.parent(),
            None => return false,
        }
    }
}

/// Put `root` and everything under it into the scene document, parent
/// before children.
///
/// [`crate::scene_io::register_entity_in_ast`] links a new node to its parent
/// only if the parent already has a node, so registering a child first strands
/// it as a second document root. `register_entities_in_ast` guarantees no such
/// order, hence this walk.
pub fn register_authored_subtree(world: &mut World, root: Entity) {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        let children = world
            .get::<Children>(entity)
            .map(|children| children.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        crate::scene_io::register_entity_in_ast(world, entity);
        stack.extend(children.into_iter().rev());
    }
}

/// Give `entity` a name no other scene entity holds, the way a duplicated
/// subtree gets one.
///
/// A widget definition names its root after the kind it makes, so a second
/// Button arrives called `Button` again. Two rows reading `Button` in the
/// outliner name nothing, and an operator clause addressing one by name
/// reaches whichever the query answers with first.
///
/// Run before the subtree is registered, so the document records the name the
/// entity ends up with rather than the one the definition wrote.
fn rename_off_collisions(
    world: &mut World,
    entity: Entity,
    taken: &mut std::collections::HashSet<String>,
) {
    let Some(name) = world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_owned())
    else {
        return;
    };
    if let Some(free) = crate::entity_ops::claim_free_name(taken, &name) {
        world.entity_mut(entity).insert(Name::new(free));
    }
}

/// One widget creation: run the definition, adopt the result into the
/// document, and select it. Undo takes all three back.
struct InstantiateWidgetCommand {
    label: String,
    definition: Arc<WidgetDefinition>,
    parent: Entity,
    spawned: Option<Entity>,
    error: Option<String>,
}

impl EditorCommand for InstantiateWidgetCommand {
    fn execute(&mut self, world: &mut World) {
        self.spawned = None;
        self.error = None;
        // Read before the definition runs, so the new widget's own name is
        // not in the set it is checked against.
        let mut taken = crate::entity_ops::scene_entity_names(world);
        let context = WidgetInstantiateContext {
            parent: Some(self.parent),
        };
        match (self.definition.instantiate)(world, context) {
            Ok(entity) if world.get_entity(entity).is_ok() => {
                // A third-party definition may ignore `ctx.parent`, so
                // re-parenting here keeps every widget inside the open scene.
                if world.get::<ChildOf>(entity).map(ChildOf::parent) != Some(self.parent) {
                    world.entity_mut(entity).insert(ChildOf(self.parent));
                }
                rename_off_collisions(world, entity, &mut taken);
                register_authored_subtree(world, entity);
                crate::selection::select_only(world, entity);
                self.spawned = Some(entity);
            }
            Ok(entity) => {
                self.error = Some(format!(
                    "widget `{}` returned missing entity {entity}",
                    self.definition.id
                ));
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn undo(&mut self, world: &mut World) {
        let Some(entity) = self.spawned.take() else {
            return;
        };
        crate::commands::deselect_entities(world, &[entity]);
        // Recursive on the document side: it detaches the node and despawns
        // every AST descendant, unlinking each one's ECS mapping.
        world
            .resource_mut::<jackdaw_bsn::SceneBsnAst>()
            .remove_entity_node(entity);
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// What [`seed_ui_scene_root`] names the root it makes. Space-free, so an
/// operator clause can address it as `name=UiRoot`.
pub const UI_SCENE_ROOT_NAME: &str = "UiRoot";

/// Seed the root a new UI scene starts from: one node a widget can be parented
/// to, registered in the document and selected so the next Add lands inside it.
///
/// The one place a new UI scene's starting shape is defined, so every path that
/// makes one makes the same one.
///
/// Spawns directly rather than through `SpawnEntity`, since there is no earlier
/// state to undo to: `scene.new` is `allows_undo = false` and hands over a tab
/// whose history is empty.
///
/// The three components: `UiSceneRoot` is what the 2D stage keys on, and its
/// default `reference_size` is the design resolution that stage frames the
/// scene at; `TabGroup` is the ancestor tab navigation gathers focusable nodes
/// from; `Node` makes it a layout parent.
///
/// The name has no space in it on purpose: an operator clause has no quoting,
/// so a `name=` value cannot carry one and a root called `UI Root` would be
/// unaddressable from `JACKDAW_RUN_OP` or the command palette. See
/// [`crate::boot_ops`].
pub fn seed_ui_scene_root(world: &mut World) -> Entity {
    let root = world
        .spawn((
            Name::new(UI_SCENE_ROOT_NAME),
            jackdaw_scene_types::UiSceneRoot::default(),
            TabGroup::default(),
            Node::default(),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, root);
    crate::selection::select_only(world, root);
    root
}
