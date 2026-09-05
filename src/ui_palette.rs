//! Widget creation: one registered definition becomes one authored entity.
//!
//! [`instantiate_widget`] is the single path from a registered
//! [`WidgetDefinition`] to an authored entity: it picks the parent, runs the
//! definition, registers the result in the scene document, selects it, and
//! records one undo entry.

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
/// optional; left out, the widget goes inside the selection when the
/// selection is a container and beside it when it is a leaf, and a selection
/// outside the UI scene falls back to the scene root.
///
/// Both failure modes are decided before anything is queued, so a request
/// that did not happen reports `Cancelled`.
#[operator(
    id = "widget.add",
    label = "Add Widget",
    description = "Add a registered UI widget to the open UI scene.",
    allows_undo = false,
    params(
        name(String, doc = "Widget definition id, e.g. \"ui.button\"."),
        parent(
            Entity,
            doc = "Node that adopts the widget. Left out, the widget goes inside the selection when it is a container and beside it when it is not."
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

/// Every registered widget id, sorted, for the line that reports what an
/// author can write instead.
fn registered_widget_ids(registry: Option<&WidgetRegistry>) -> String {
    let mut ids: Vec<&str> = registry
        .map(|registry| registry.iter().map(|definition| &*definition.id).collect())
        .unwrap_or_default();
    ids.sort_unstable();
    if ids.is_empty() {
        return "empty: no extension has registered a widget".to_string();
    }
    ids.join(", ")
}

/// Why a widget request produced no widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteError {
    /// No definition is registered under this id.
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

/// Create the widget `definition_id` names where the selection says, as one
/// undoable step: inside it when the selection is a container, beside it
/// when it is not; a container is a Panel, a Row, a Column, a Grid or a
/// scroll area.
///
/// An unknown id, a document with no UI scene, and a definition that refuses
/// all return an error for the caller to report.
pub fn instantiate_widget(world: &mut World, definition_id: &str) -> Result<Entity, PaletteError> {
    let candidate = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    let slot = widget_slot(world, candidate).ok_or(PaletteError::NoUiScene)?;
    instantiate_at(world, definition_id, slot)
}

/// Create the widget `definition_id` names inside `parent`, as its last child.
///
/// A caller naming a parent means that node, so the widget goes in it rather
/// than beside it; `None` falls back to [`instantiate_widget`]'s rule. A
/// named node outside the open UI scene is replaced by the scene root.
pub fn instantiate_widget_under(
    world: &mut World,
    definition_id: &str,
    parent: Option<Entity>,
) -> Result<Entity, PaletteError> {
    let Some(parent) = parent else {
        return instantiate_widget(world, definition_id);
    };
    let root = ui_scene_root(world).ok_or(PaletteError::NoUiScene)?;
    let parent = if is_in_ui_scene(world, parent, root) {
        parent
    } else {
        root
    };
    instantiate_at(
        world,
        definition_id,
        WidgetSlot {
            parent,
            index: usize::MAX,
        },
    )
}

/// Run the definition and put the result in `slot`.
fn instantiate_at(
    world: &mut World,
    definition_id: &str,
    slot: WidgetSlot,
) -> Result<Entity, PaletteError> {
    let (entity, command) = instantiate_command_at(world, definition_id, slot)?;
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    Ok(entity)
}

/// Create the widget `definition_id` names inside `parent`, handing the
/// caller the entry that undoes it rather than recording one. For a caller
/// whose own command owns the creation, since its `execute` may run while
/// the history is out of the world for a redo.
pub fn instantiate_widget_command_under(
    world: &mut World,
    definition_id: &str,
    parent: Option<Entity>,
) -> Result<(Entity, Box<dyn EditorCommand>), PaletteError> {
    let root = ui_scene_root(world).ok_or(PaletteError::NoUiScene)?;
    let parent = match parent {
        Some(parent) if is_in_ui_scene(world, parent, root) => parent,
        _ => root,
    };
    instantiate_command_at(
        world,
        definition_id,
        WidgetSlot {
            parent,
            index: usize::MAX,
        },
    )
}

fn instantiate_command_at(
    world: &mut World,
    definition_id: &str,
    slot: WidgetSlot,
) -> Result<(Entity, Box<dyn EditorCommand>), PaletteError> {
    let definition = world
        .get_resource::<WidgetRegistry>()
        .and_then(|registry| registry.get(definition_id))
        .ok_or_else(|| PaletteError::UnknownDefinition(definition_id.to_string()))?;
    backfill_focus_group(world);
    backfill_ui_root_size(world);

    let mut command = InstantiateWidgetCommand {
        label: format!("Add {}", definition.name),
        definition,
        slot,
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
    Ok((entity, Box::new(command)))
}

/// The node a new widget is parented to: the selection itself when it is a
/// container, its parent when it is a leaf, and the scene's root when it is
/// outside the open UI scene, so a 3D selection does not block UI
/// authoring. `None` means the document holds no UI scene, the one case
/// widget creation refuses.
pub fn resolve_widget_parent(world: &mut World) -> Option<Entity> {
    let candidate = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    Some(widget_slot(world, candidate)?.parent)
}

/// Where a new widget goes: inside `candidate` when `candidate` is a
/// container, and beside it when it is not. The scene root, a candidate
/// outside the open UI scene, and no candidate at all are all the container
/// case.
fn widget_slot(world: &mut World, candidate: Option<Entity>) -> Option<WidgetSlot> {
    let root = ui_scene_root(world)?;
    let inside = candidate.filter(|entity| is_in_ui_scene(world, *entity, root));
    let Some(primary) = inside.filter(|&entity| entity != root) else {
        return Some(WidgetSlot {
            parent: root,
            index: usize::MAX,
        });
    };
    if is_layout_container(world, primary) {
        return Some(WidgetSlot {
            parent: primary,
            index: usize::MAX,
        });
    }
    let location = crate::commands::HierarchyLocation::from_world(world, primary);
    Some(WidgetSlot {
        parent: location.parent.unwrap_or(root),
        index: location.index + 1,
    })
}

/// Whether `entity` is a node a widget is added *into* rather than beside.
///
/// A container is a laid-out node with nothing more particular on it.
/// Everything else is a leaf, including widgets whose chrome is rebuilt from
/// their own data, where a child added by hand would be thrown away by the
/// next rebuild.
fn is_layout_container(world: &World, entity: Entity) -> bool {
    use bevy::ui_widgets::{Button, Checkbox, RadioButton, ScrollArea, Slider};
    use jackdaw_widgets_runtime as runtime;

    let Ok(entity) = world.get_entity(entity) else {
        return false;
    };
    let Some(node) = entity.get::<Node>() else {
        return false;
    };
    if entity.contains::<ScrollArea>() {
        return true;
    }
    let leaf = entity.contains::<Text>()
        || entity.contains::<ImageNode>()
        || entity.contains::<Button>()
        || entity.contains::<Checkbox>()
        || entity.contains::<RadioButton>()
        || entity.contains::<Slider>()
        || entity.contains::<runtime::TextValue>()
        || entity.contains::<runtime::ToggleSwitch>()
        || entity.contains::<runtime::Dropdown>()
        || entity.contains::<runtime::RadioOptions>()
        || entity.contains::<runtime::TabStrip>()
        || entity.contains::<runtime::NineSlice>()
        || entity.contains::<runtime::Progress>()
        || entity.contains::<runtime::Spacer>()
        || entity.contains::<runtime::Separator>();
    !leaf && matches!(node.display, Display::Flex | Display::Grid)
}

/// The parent and sibling slot a new widget takes.
#[derive(Clone, Copy)]
struct WidgetSlot {
    parent: Entity,
    /// [`usize::MAX`] is the end of the parent's child list.
    index: usize,
}

/// The open UI scene's root.
///
/// Only a root the open document holds: another tab's scene keeps its
/// entities alive in the same world, and a bare query over the marker could
/// answer with that one. A malformed document holding several picks the
/// lowest entity, so the choice is stable across a session.
pub fn ui_scene_root(world: &mut World) -> Option<Entity> {
    let candidates: Vec<Entity> = world
        .query_filtered::<Entity, crate::prefab::AuthoredUiSceneRoot>()
        .iter(world)
        .collect();
    let document = world.resource::<jackdaw_bsn::SceneBsnAst>();
    candidates
        .into_iter()
        .filter(|&root| document.ast_for(root).is_some())
        .min()
}

/// Give the open UI scene's root the `TabGroup` tab navigation gathers from,
/// unless it already declares one. Covers scenes authored before
/// [`seed_ui_scene_root`] added it, on their first added widget, and writes
/// the document without an undo entry.
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

/// Give the open UI scene's root the canvas-sized box the presets resolve
/// against, unless it states a size of its own.
///
/// A root authored before [`ui_scene_root_node`] stated `100%` carries
/// `Auto` on both axes, which shrinks it to fit its contents and leaves every
/// placement downstream resolving against that shrunken box. Written without
/// an undo entry, so undoing the first edit after an open does not put the
/// shrunken box back.
pub fn backfill_ui_root_size(world: &mut World) {
    let Some(root) = ui_scene_root(world) else {
        return;
    };
    let Some(node) = world.get::<Node>(root) else {
        return;
    };
    if node.width != Val::Auto || node.height != Val::Auto {
        return;
    }
    let mut node = node.clone();
    let sized = ui_scene_root_node();
    node.width = sized.width;
    node.height = sized.height;
    // Once the root is the canvas's own height its default cross-axis
    // alignment would stretch every child down it. Only when the document
    // states nothing of its own.
    if node.align_items == AlignItems::Default {
        node.align_items = sized.align_items;
    }
    if let Some(mut live) = world.get_mut::<Node>(root) {
        *live = node.clone();
    }
    crate::commands::sync_component_to_ast(
        world,
        root,
        crate::inspector::node_card::node_type_path(),
        &node,
    );
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

/// Put `root` and everything under it into the scene document, parent before
/// children. [`crate::scene_io::register_entity_in_ast`] links a node to its
/// parent only if the parent already has one, and
/// `register_entities_in_ast` guarantees no such order.
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

/// Give `root` and everything under it names no other scene entity holds.
/// A definition names its root after the kind it makes, so a second Button
/// would otherwise arrive called `Button` again, and an operator clause
/// addressing it by name would be ambiguous.
///
/// Run before the subtree is registered, so the document records the names
/// the entities end up with.
fn rename_off_collisions(
    world: &mut World,
    root: Entity,
    taken: &mut std::collections::HashSet<String>,
) {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        stack.extend(
            world
                .get::<Children>(entity)
                .map(|children| children.iter().collect::<Vec<_>>())
                .unwrap_or_default(),
        );
        let Some(name) = world
            .get::<Name>(entity)
            .map(|name| name.as_str().to_owned())
        else {
            continue;
        };
        if let Some(free) = crate::entity_ops::claim_free_name(taken, &name) {
            world.entity_mut(entity).insert(Name::new(free));
        }
    }
}

/// One widget creation: run the definition, adopt the result into the
/// document, and select it. Undo takes all three back.
struct InstantiateWidgetCommand {
    label: String,
    definition: Arc<WidgetDefinition>,
    slot: WidgetSlot,
    spawned: Option<Entity>,
    error: Option<String>,
}

impl EditorCommand for InstantiateWidgetCommand {
    fn execute(&mut self, world: &mut World) {
        self.spawned = None;
        self.error = None;
        let mut taken = crate::entity_ops::scene_entity_names(world);
        let context = WidgetInstantiateContext {
            parent: Some(self.slot.parent),
        };
        match (self.definition.instantiate)(world, context) {
            Ok(entity) if world.get_entity(entity).is_ok() => {
                if world.get::<ChildOf>(entity).map(ChildOf::parent) != Some(self.slot.parent) {
                    world.entity_mut(entity).insert(ChildOf(self.slot.parent));
                }
                rename_off_collisions(world, entity, &mut taken);
                register_authored_subtree(world, entity);
                crate::commands::place_entity(
                    world,
                    entity,
                    crate::commands::HierarchyLocation {
                        parent: Some(self.slot.parent),
                        index: self.slot.index,
                    },
                    crate::commands::WorldTransform::Unplaced,
                );
                crate::hierarchy::sync_outliner_row_order(world, Some(self.slot.parent));
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

/// Seed the root a new UI scene starts from: one node a widget can be
/// parented to, registered in the document and selected so the next Add
/// lands inside it. The one place a new UI scene's starting shape is defined.
///
/// Spawns directly rather than through `SpawnEntity`, since `scene.new` is
/// `allows_undo = false` and hands over a tab whose history is empty.
pub fn seed_ui_scene_root(world: &mut World) -> Entity {
    let root = world
        .spawn((
            Name::new(UI_SCENE_ROOT_NAME),
            jackdaw_scene_types::UiSceneRoot::default(),
            TabGroup::default(),
            ui_scene_root_node(),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, root);
    crate::selection::select_only(world, root);
    root
}

/// The `Node` a UI scene root carries: the canvas box itself.
///
/// `100%` on both axes rather than `Auto`, since the implicit viewport node
/// Bevy puts around a root start-aligns without stretching, and an `Auto`
/// root would shrink to fit its contents. That makes the root the target's
/// own size, which [`crate::viewport_2d::size_targets_to_reference`] holds at
/// the scene's `reference_size`. `align_items` is `Start` so a child in the
/// root's flow is not stretched down the whole canvas.
pub fn ui_scene_root_node() -> Node {
    Node {
        width: percent(100.0),
        height: percent(100.0),
        align_items: AlignItems::Start,
        ..default()
    }
}
