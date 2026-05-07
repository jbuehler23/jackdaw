use std::collections::HashSet;

use bevy::{
    ecs::system::SystemState, input_focus::InputFocus, prelude::*,
    ui::ui_transform::UiGlobalTransform,
};
use bevy_enhanced_input::prelude::{Press, *};
use bevy_monitors::prelude::{Mutation, NotifyChanged};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    context_menu::spawn_context_menu,
    icons::IconFont,
    text_edit::{self, EditorTextEdit, TextEditCommitEvent, TextEditProps, TextEditValue},
    tokens,
    tree_view::{ROW_BG, TreeRowStyle, tree_row},
};
use jackdaw_widgets::context_menu::{ContextMenuAction, ContextMenuState};
use jackdaw_widgets::tree_view::{
    EntityCategory, TreeChildrenPopulated, TreeFocused, TreeIndex, TreeNode, TreeNodeExpanded,
    TreeRowChildren, TreeRowClicked, TreeRowContent, TreeRowDropped, TreeRowDroppedOnRoot,
    TreeRowInlineRename, TreeRowLabel, TreeRowRenamed, TreeRowSelected, TreeRowStartRename,
    TreeRowVisibilityToggled,
};

use crate::{
    EditorEntity, EditorHidden, OP_PREFIX,
    commands::{CommandHistory, EditorCommand, ReparentEntity, SetJsnField},
    entity_ops,
    layout::HierarchyFilter,
    selection::{Selected, Selection},
};
use jackdaw_feathers::dialog::{DialogActionEvent, DialogChildrenSlot};
use jackdaw_jsn::BrushGroup;

/// Stores the default name for the template save dialog.
#[derive(Resource, Default)]
struct PendingTemplateDefaultName(String);

/// Marker for the template name text input inside the dialog.
#[derive(Component)]
struct TemplateNameInput;

/// Marker for the hierarchy panel
#[derive(Component)]
#[require(EditorEntity)]
pub struct HierarchyPanel;

/// Marker for the container that holds tree rows
#[derive(Component)]
#[require(EditorEntity)]
pub struct HierarchyTreeContainer;

/// Controls whether the hierarchy shows all entities or only named ones.
/// `false` = named only (default), `true` = all entities (minus `EditorEntity`).
#[derive(Resource, Default)]
pub struct HierarchyShowAll(pub bool);

/// Marker for the show-all toggle button in the hierarchy panel.
#[derive(Component)]
pub struct HierarchyShowAllButton;

pub struct HierarchyPlugin;

impl Plugin for HierarchyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ContextMenuState>()
            .init_resource::<PendingTemplateDefaultName>()
            .init_resource::<HierarchyShowAll>()
            .add_systems(Startup, setup_tree_node_expanded_watcher)
            .add_systems(OnEnter(crate::AppState::Editor), setup_name_watcher)
            .add_systems(
                Update,
                (
                    apply_hierarchy_filter,
                    auto_focus_inline_rename,
                    populate_template_dialog,
                    toggle_show_all_button,
                    update_show_all_button_appearance,
                    on_show_all_changed,
                    jackdaw_feathers::tree_view::tree_keyboard_navigation,
                    style_game_spawned_rows,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                PostUpdate,
                rebuild_hierarchy_on_container_added
                    .after(jackdaw_widgets::tree_view::maintain_tree_index),
            )
            .add_observer(handle_inline_rename_commit)
            .add_observer(on_root_entity_added)
            .add_observer(on_entity_reparented)
            .add_observer(on_entity_deparented)
            .add_observer(on_tree_node_expanded)
            .add_observer(on_tree_row_clicked)
            .add_observer(on_entity_removed)
            .add_observer(on_name_changed)
            .add_observer(on_entity_selected)
            .add_observer(on_entity_deselected)
            .add_observer(on_tree_row_dropped)
            .add_observer(on_tree_row_dropped_on_root)
            .add_observer(on_tree_row_start_rename)
            .add_observer(on_tree_row_renamed)
            .add_observer(on_context_menu_action)
            .add_observer(on_visibility_toggled)
            .add_observer(on_template_dialog_action)
            .add_observer(on_entity_hidden);
    }
}

/// Classify a scene entity by its primary component for tree display.
fn classify_entity(world: &World, entity: Entity) -> EntityCategory {
    if world.get::<BrushGroup>(entity).is_some() {
        return EntityCategory::Mesh;
    }
    if world.get::<Camera>(entity).is_some() {
        return EntityCategory::Camera;
    }
    if world.get::<PointLight>(entity).is_some()
        || world.get::<DirectionalLight>(entity).is_some()
        || world.get::<SpotLight>(entity).is_some()
    {
        return EntityCategory::Light;
    }
    if world.get::<Mesh3d>(entity).is_some() {
        return EntityCategory::Mesh;
    }
    if world.get::<SceneRoot>(entity).is_some() {
        return EntityCategory::Scene;
    }
    EntityCategory::Entity
}

/// Check if an entity has any non-editor children.
fn has_visible_children(world: &World, entity: Entity) -> bool {
    let Some(children) = world.get::<Children>(entity) else {
        return false;
    };
    children.iter().any(|child| {
        world.get::<EditorEntity>(child).is_none() && world.get::<EditorHidden>(child).is_none()
    })
}

/// Spawn a single (non-recursive) tree row for a source entity.
/// Updates `TreeIndex` immediately.
fn spawn_single_tree_row(world: &mut World, source: Entity, parent_container: Entity) -> Entity {
    let label = world
        .get::<Name>(source)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| format!("Entity {source}"));
    let has_children = has_visible_children(world, source);
    let category = classify_entity(world, source);
    let icon_font = world.resource::<IconFont>().0.clone();
    let style = TreeRowStyle { icon_font };

    let tree_row_entity = world
        .spawn((
            tree_row(&label, has_children, false, source, category, &style),
            ChildOf(parent_container),
        ))
        .id();

    world
        .resource_mut::<TreeIndex>()
        .insert(source, tree_row_entity);
    tree_row_entity
}

// This has to be a system instead of an observer because it must run after `tree_view::maintain_tree_index`
fn rebuild_hierarchy_on_container_added(
    added: Query<Entity, Added<HierarchyTreeContainer>>,
    mut commands: Commands,
) {
    if !added.is_empty() {
        commands.queue(rebuild_hierarchy);
    }
}

fn rebuild_hierarchy(world: &mut World) -> Result {
    fn rebuild_hierarchy_inner(
        world: &mut World,
        container: &mut SystemState<Option<Single<Entity, With<HierarchyTreeContainer>>>>,
        roots: &mut QueryState<
            Entity,
            (
                With<Transform>,
                Without<EditorEntity>,
                Without<EditorHidden>,
                Without<ChildOf>,
            ),
        >,
    ) {
        // No container means the editor UI tree isn't mounted (e.g.
        // headless integration tests, or pre-`OnEnter(Editor)` state).
        // Nothing to rebuild against; bail.
        let Some(container) = container.get(world) else {
            return;
        };
        let container = *container;

        // Collect all root scene entities (Transform, no ChildOf, no editor markers)
        let roots: Vec<Entity> = roots.iter(world).collect();

        let show_all = world.resource::<HierarchyShowAll>().0;

        // Sort by (category, name) for consistent ordering
        let mut root_data: Vec<(Entity, EntityCategory, String)> = roots
            .into_iter()
            .filter(|&e| !world.resource::<TreeIndex>().contains(e))
            .filter(|&e| show_all || world.get::<Name>(e).is_some())
            .map(|e| {
                let category = classify_entity(world, e);
                let name = world
                    .get::<Name>(e)
                    .map(|n| n.as_str().to_string())
                    .unwrap_or_else(|| format!("Entity {e}"));
                (e, category, name)
            })
            .collect();

        root_data.sort_by(|(_, cat_a, name_a), (_, cat_b, name_b)| {
            cat_a.cmp(cat_b).then_with(|| name_a.cmp(name_b))
        });

        for (entity, _category, _name) in root_data {
            spawn_single_tree_row(world, entity, container);
        }
    }
    world
        .run_system_cached(rebuild_hierarchy_inner)
        .map_err(BevyError::from)
}

/// When a new entity gets Transform and has no parent, create a root tree row.
fn on_root_entity_added(
    trigger: On<Add, Transform>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
    container: Option<Single<Entity, With<HierarchyTreeContainer>>>,
    editor_check: Query<(), Or<(With<EditorEntity>, With<EditorHidden>)>>,
    child_of_check: Query<(), With<ChildOf>>,
) {
    let entity = trigger.event_target();
    let Some(container) = container else {
        return;
    };

    if editor_check.contains(entity)
        || child_of_check.contains(entity)
        || tree_index.contains(entity)
    {
        return;
    }

    let container = *container;
    commands.queue(move |world: &mut World| {
        if world.resource::<TreeIndex>().contains(entity) {
            return;
        }
        // Re-check: ChildOf may have been added between observer and command flush
        if world.get::<ChildOf>(entity).is_some() {
            return;
        }
        if world.get::<EditorEntity>(entity).is_some()
            || world.get::<EditorHidden>(entity).is_some()
        {
            return;
        }
        // In named-only mode, skip entities without a Name
        if !world.resource::<HierarchyShowAll>().0 && world.get::<Name>(entity).is_none() {
            return;
        }
        spawn_single_tree_row(world, entity, container);
    });
}

/// When an entity's Name is added/changed, update its tree row label.
/// Also creates a tree row if the entity is a root without one.
fn on_name_changed(
    trigger: On<Add, Name>,
    mut commands: Commands,
    name_query: Query<&Name>,
    tree_index: Res<TreeIndex>,
    tree_nodes: Query<&Children, With<TreeNode>>,
    content_query: Query<&Children, With<TreeRowContent>>,
    mut label_query: Query<&mut Text, With<TreeRowLabel>>,
    container: Option<Single<Entity, With<HierarchyTreeContainer>>>,
    editor_check: Query<(), Or<(With<EditorEntity>, With<EditorHidden>)>>,
    child_of_check: Query<(), With<ChildOf>>,
) {
    let entity = trigger.event_target();
    let Ok(name) = name_query.get(entity) else {
        return;
    };

    if let Some(tree_entity) = tree_index.get(entity) {
        // Update existing label: TreeNode → Children → TreeRowContent → Children → TreeRowLabel
        let Ok(children) = tree_nodes.get(tree_entity) else {
            return;
        };
        for child in children.iter() {
            if let Ok(content_children) = content_query.get(child) {
                for grandchild in content_children.iter() {
                    if let Ok(mut text) = label_query.get_mut(grandchild) {
                        text.0 = name.as_str().to_string();
                        return;
                    }
                }
            }
        }
    } else {
        // Entity has no tree row. Create one if it's a visible root
        let Some(container) = container else {
            return;
        };
        if editor_check.contains(entity) || child_of_check.contains(entity) {
            return;
        }

        let container = *container;
        commands.queue(move |world: &mut World| {
            if world.resource::<TreeIndex>().contains(entity) {
                return;
            }
            // Re-check: ChildOf may have been added between observer and command flush
            if world.get::<ChildOf>(entity).is_some() {
                return;
            }
            if world.get::<EditorEntity>(entity).is_some()
                || world.get::<EditorHidden>(entity).is_some()
            {
                return;
            }
            spawn_single_tree_row(world, entity, container);
        });
    }
}

/// Spawn a watcher entity that notifies us when Name is mutated in-place.
fn setup_name_watcher(mut commands: Commands) {
    commands
        .spawn((EditorEntity, NotifyChanged::<Name>::default()))
        .observe(on_name_mutated);
}

/// Pre-register the `NotifyChanged<TreeNodeExpanded>` hook during
/// Startup. `bevy_monitors`'s add-hook queues a command that calls
/// `world.schedule_scope(Update, ...)` the first time any entity with
/// `NotifyChanged<C>` spawns. If that first spawn happens while `Update`
/// is already executing (e.g. `reconcile_tree` spawning scene tree rows
/// on workspace switch), the queued command panics with "Schedule
/// Update not found". Registering a watcher entity here in Startup
/// flushes the hook before any `Update` tick runs, so subsequent spawns
/// take the `DetectingChanges<TreeNodeExpanded>` early-return branch.
fn setup_tree_node_expanded_watcher(mut commands: Commands) {
    commands.spawn(NotifyChanged::<TreeNodeExpanded>::default());
}

/// When an entity's Name is mutated in-place (e.g. via inspector), update the tree row label.
fn on_name_mutated(
    trigger: On<Mutation<Name>>,
    name_query: Query<&Name>,
    tree_index: Res<TreeIndex>,
    tree_nodes: Query<&Children, With<TreeNode>>,
    content_query: Query<&Children, With<TreeRowContent>>,
    mut label_query: Query<&mut Text, With<TreeRowLabel>>,
) {
    let entity = trigger.mutated;
    let Ok(name) = name_query.get(entity) else {
        return;
    };
    let Some(tree_entity) = tree_index.get(entity) else {
        return;
    };
    let Ok(children) = tree_nodes.get(tree_entity) else {
        return;
    };
    for child in children.iter() {
        let Ok(content_children) = content_query.get(child) else {
            continue;
        };
        for grandchild in content_children.iter() {
            if let Ok(mut text) = label_query.get_mut(grandchild) {
                text.0 = name.as_str().to_string();
                return;
            }
        }
    }
}

/// When an entity gets a parent (`ChildOf` added or changed), reparent or create its tree row.
fn on_entity_reparented(
    trigger: On<Add, ChildOf>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
    editor_check: Query<(), Or<(With<EditorEntity>, With<EditorHidden>)>>,
    tree_node_check: Query<(), With<TreeNode>>,
    child_of_query: Query<&ChildOf>,
    children_query: Query<&Children>,
    tree_row_children: Query<Entity, With<TreeRowChildren>>,
    populated_query: Query<&TreeChildrenPopulated>,
) {
    let entity = trigger.event_target();

    // Skip editor/hidden entities and tree row UI entities
    if editor_check.contains(entity) || tree_node_check.contains(entity) {
        return;
    }

    let Ok(&ChildOf(new_parent)) = child_of_query.get(entity) else {
        return;
    };

    // Find the new parent's TreeRowChildren container via TreeIndex + child walk
    let parent_container = tree_index.get(new_parent).and_then(|parent_tree| {
        children_query
            .get(parent_tree)
            .ok()
            .and_then(|children| children.iter().find(|c| tree_row_children.contains(*c)))
    });

    // If tree row already exists for this entity → reparent it
    if let Some(tree_entity) = tree_index.get(entity) {
        if let Some(container) = parent_container {
            commands.entity(tree_entity).insert(ChildOf(container));
        } else {
            // Parent has no tree row yet. Remove this incorrectly-rooted tree row.
            // Lazy loading will re-create it when the parent is expanded.
            let source = entity;
            commands.queue(move |world: &mut World| {
                world.resource_mut::<TreeIndex>().remove(source);
                if let Ok(ec) = world.get_entity_mut(tree_entity) {
                    ec.despawn();
                }
            });
        }
        return;
    }

    // No tree row exists. Only spawn if the parent's children are already populated
    let Some(parent_container) = parent_container else {
        return;
    };
    let parent_tree = tree_index.get(new_parent).unwrap(); // safe: we found parent_container from it
    let populated = populated_query
        .get(parent_tree)
        .map(|p| p.0)
        .unwrap_or(false);
    if !populated {
        return; // Lazy loading will handle it when parent is expanded
    }

    let container = parent_container;
    commands.queue(move |world: &mut World| {
        if world.resource::<TreeIndex>().contains(entity) {
            return;
        }
        // In named-only mode, skip entities without a Name
        if !world.resource::<HierarchyShowAll>().0 && world.get::<Name>(entity).is_none() {
            return;
        }
        spawn_single_tree_row(world, entity, container);
    });
}

/// When `ChildOf` is removed (entity deparented back to root, e.g. via undo of
/// a reparent), move its tree row back to the root container. Without this,
/// the tree UI shows stale parent information after an undo.
fn on_entity_deparented(
    trigger: On<Remove, ChildOf>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
    container: Option<Single<Entity, With<HierarchyTreeContainer>>>,
    editor_check: Query<(), Or<(With<EditorEntity>, With<EditorHidden>)>>,
    tree_node_check: Query<(), With<TreeNode>>,
) {
    let entity = trigger.event_target();
    if editor_check.contains(entity) || tree_node_check.contains(entity) {
        return;
    }
    let Some(container) = container else {
        return;
    };
    let root_container = *container;
    if let Some(tree_entity) = tree_index.get(entity) {
        commands.entity(tree_entity).insert(ChildOf(root_container));
    }
}

/// When an entity's Name is removed, despawn its tree row.
fn on_entity_removed(
    trigger: On<Despawn, Name>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
) {
    let entity = trigger.event_target();

    if let Some(tree_entity) = tree_index.get(entity)
        && let Ok(mut ec) = commands.get_entity(tree_entity)
    {
        ec.despawn();
    }
}

/// When `EditorHidden` is added, remove the tree row if one exists (handles race with observers).
fn on_entity_hidden(
    trigger: On<Add, EditorHidden>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
) {
    let entity = trigger.event_target();

    if let Some(tree_entity) = tree_index.get(entity)
        && let Ok(mut ec) = commands.get_entity(tree_entity)
    {
        ec.despawn();
    }
}

/// When a tree node is expanded for the first time, spawn tree rows for its children.
fn on_tree_node_expanded(
    trigger: On<Mutation<TreeNodeExpanded>>,
    mut commands: Commands,
    tree_query: Query<(
        &TreeNodeExpanded,
        &TreeChildrenPopulated,
        &TreeNode,
        &Children,
    )>,
    tree_row_children_marker: Query<Entity, With<TreeRowChildren>>,
    remote_check: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) {
    let entity = trigger.event_target();
    let Ok((expanded, populated, tree_node, children)) = tree_query.get(entity) else {
        return;
    };

    // Only populate on first expansion
    if !expanded.0 || populated.0 {
        return;
    }

    let source = tree_node.0;

    // Skip remote entity proxies, handled by entity_browser observer
    if remote_check.contains(source) {
        return;
    }

    let Some(container) = children
        .iter()
        .find(|c| tree_row_children_marker.contains(*c))
    else {
        return;
    };
    let tree_row_entity = entity;

    commands.queue(move |world: &mut World| {
        // Double-check populated flag (guard against duplicate events)
        if let Some(pop) = world.get::<TreeChildrenPopulated>(tree_row_entity)
            && pop.0
        {
            return;
        }

        // Mark as populated
        if let Some(mut pop) = world.get_mut::<TreeChildrenPopulated>(tree_row_entity) {
            pop.0 = true;
        }

        // Collect visible children with classification
        let source_children: Vec<Entity> = world
            .get::<Children>(source)
            .map(|c| c.iter().collect())
            .unwrap_or_default();

        let mut child_data: Vec<(Entity, String, EntityCategory)> = Vec::new();
        for child in source_children {
            if world.get::<EditorEntity>(child).is_some()
                || world.get::<EditorHidden>(child).is_some()
            {
                continue;
            }
            // Skip children that already have tree rows
            if world.resource::<TreeIndex>().contains(child) {
                continue;
            }
            let name = world
                .get::<Name>(child)
                .map(|n| n.as_str().to_string())
                .unwrap_or_else(|| format!("Entity {child}"));
            let category = classify_entity(world, child);
            child_data.push((child, name, category));
        }

        // Sort by (category, name)
        child_data.sort_by(|(_, name_a, cat_a), (_, name_b, cat_b)| {
            cat_a.cmp(cat_b).then_with(|| name_a.cmp(name_b))
        });

        // Spawn tree rows
        for (child_entity, _name, _category) in child_data {
            spawn_single_tree_row(world, child_entity, container);
        }
    });
}

/// Handle tree row click → select the source entity.
/// Plain click on selected entity → deselect. Ctrl+Click → toggle.
fn on_tree_row_clicked(
    event: On<TreeRowClicked>,
    mut commands: Commands,
    mut selection: ResMut<Selection>,
    mut focused: ResMut<TreeFocused>,
    keyboard: Res<ButtonInput<KeyCode>>,
    parent_query: Query<&ChildOf>,
    tree_nodes: Query<Entity, With<TreeNode>>,
    remote_check: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) {
    // Skip remote entity proxies, handled by entity_browser observer
    if remote_check.contains(event.source_entity) {
        return;
    }

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    if ctrl {
        selection.toggle(&mut commands, event.source_entity);
    } else if selection.is_selected(event.source_entity) {
        selection.clear(&mut commands);
    } else {
        selection.select_single(&mut commands, event.source_entity);
    }

    // Set keyboard focus to the tree row containing this content
    let content_entity = event.entity;
    if let Ok(&ChildOf(tree_row)) = parent_query.get(content_entity)
        && tree_nodes.contains(tree_row)
    {
        focused.0 = Some(tree_row);
    }
}

/// When Selected is added, highlight the corresponding tree row.
fn on_entity_selected(
    trigger: On<Add, Selected>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
    tree_nodes: Query<&Children, With<TreeNode>>,
    tree_row_contents: Query<Entity, With<TreeRowContent>>,
    mut bg_query: Query<&mut BackgroundColor>,
    mut border_query: Query<&mut BorderColor>,
) {
    let entity = trigger.event_target();

    let Some(tree_entity) = tree_index.get(entity) else {
        return;
    };
    let Ok(children) = tree_nodes.get(tree_entity) else {
        return;
    };

    for child in children.iter() {
        if tree_row_contents.contains(child) {
            if let Ok(mut ec) = commands.get_entity(child) {
                ec.insert(TreeRowSelected);
            }
            if let Ok(mut bg) = bg_query.get_mut(child) {
                bg.0 = tokens::SELECTED_BG;
            }
            if let Ok(mut border) = border_query.get_mut(child) {
                *border = BorderColor::all(tokens::SELECTED_BORDER);
            }
            return;
        }
    }
}

/// When Selected is removed, unhighlight the corresponding tree row.
fn on_entity_deselected(
    trigger: On<Remove, Selected>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
    tree_nodes: Query<&Children, With<TreeNode>>,
    tree_row_contents: Query<Entity, With<TreeRowContent>>,
    mut bg_query: Query<&mut BackgroundColor>,
    mut border_query: Query<&mut BorderColor>,
) {
    let entity = trigger.event_target();

    let Some(tree_entity) = tree_index.get(entity) else {
        return;
    };
    let Ok(children) = tree_nodes.get(tree_entity) else {
        return;
    };

    for child in children.iter() {
        if tree_row_contents.contains(child) {
            if let Ok(mut ec) = commands.get_entity(child) {
                ec.remove::<TreeRowSelected>();
            }
            if let Ok(mut bg) = bg_query.get_mut(child) {
                bg.0 = ROW_BG;
            }
            if let Ok(mut border) = border_query.get_mut(child) {
                *border = BorderColor::all(Color::NONE);
            }
            return;
        }
    }
}

/// Handle tree row dropped → reparent the scene entity with undo support.
fn on_tree_row_dropped(
    event: On<TreeRowDropped>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
) {
    let dragged = event.dragged_source;
    let target = event.target_source;

    if dragged == target {
        return;
    }

    // Cycle check: walk up from target, ensure dragged is not an ancestor
    let mut current = target;
    while let Ok(&ChildOf(parent)) = parent_query.get(current) {
        if parent == dragged {
            return;
        }
        current = parent;
    }

    let old_parent = parent_query.get(dragged).ok().map(|c| c.0);

    let mut cmd = ReparentEntity {
        entity: dragged,
        old_parent,
        new_parent: Some(target),
    };

    commands.queue(move |world: &mut World| {
        cmd.execute(world);
        world
            .resource_mut::<CommandHistory>()
            .undo_stack
            .push(Box::new(cmd));
        world.resource_mut::<CommandHistory>().redo_stack.clear();
    });
}

/// Handle tree row dropped on root container → deparent the scene entity.
fn on_tree_row_dropped_on_root(
    event: On<TreeRowDroppedOnRoot>,
    mut commands: Commands,
    parent_query: Query<&ChildOf, Without<EditorEntity>>,
    tree_index: Res<TreeIndex>,
    container: Option<Single<Entity, With<HierarchyTreeContainer>>>,
) {
    let Some(container) = container else {
        // No tree container mounted (e.g. inside a headless test).
        return;
    };
    let dragged = event.dragged_source;

    let old_parent = match parent_query.get(dragged) {
        Ok(child_of) => Some(child_of.0),
        Err(_) => return,
    };

    let container_entity = *container;

    let mut cmd = ReparentEntity {
        entity: dragged,
        old_parent,
        new_parent: None,
    };

    commands.queue(move |world: &mut World| {
        cmd.execute(world);
        world
            .resource_mut::<CommandHistory>()
            .undo_stack
            .push(Box::new(cmd));
        world.resource_mut::<CommandHistory>().redo_stack.clear();
    });

    // Move the tree row to the root container
    if let Some(tree_entity) = tree_index.get(dragged) {
        commands
            .entity(tree_entity)
            .insert(ChildOf(container_entity));
    }
}

/// Open the hierarchy row context menu under the cursor (RMB).
#[operator(
    id = "hierarchy.open_context_menu",
    label = "Open Context Menu",
    description = "Show the context menu for the entity under the cursor.",
    allows_undo = false
)]
pub(crate) fn hierarchy_open_context_menu(
    _: In<OperatorParameters>,
    mut commands: Commands,
    mut state: ResMut<ContextMenuState>,
    windows: Query<&Window>,
    selection: Res<Selection>,
    tree_row_contents: Query<(Entity, &ChildOf), With<TreeRowContent>>,
    tree_nodes: Query<&TreeNode>,
    computed_nodes: Query<(&ComputedNode, &UiGlobalTransform), With<TreeRowContent>>,
    extension_add_entries: Query<&jackdaw_api_internal::lifecycle::RegisteredMenuEntry>,
) -> OperatorResult {
    let Ok(window) = windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };

    // Close any existing context menu
    if let Some(menu) = state.menu_entity.take()
        && let Ok(mut ec) = commands.get_entity(menu)
    {
        ec.despawn();
    }

    // Find which tree row content the cursor is over by hit testing
    let mut target_source = None;
    for (content_entity, child_of) in &tree_row_contents {
        let Ok((computed, global_transform)) = computed_nodes.get(content_entity) else {
            continue;
        };
        let size = computed.size();
        let (_, _, translation) = global_transform.to_scale_angle_translation();
        let pos = translation;
        let half = size / 2.0;
        let rect = Rect::from_center_half_size(pos, half);
        if rect.contains(cursor_pos)
            && let Ok(tree_node) = tree_nodes.get(child_of.0)
        {
            target_source = Some(tree_node.0);
            break;
        }
    }

    let Some(target) = target_source else {
        return OperatorResult::Cancelled;
    };

    // If the right-clicked entity isn't selected, select it
    if !selection.is_selected(target) {
        commands.queue(move |world: &mut World| {
            let old_entities: Vec<Entity> = world.resource::<Selection>().entities.clone();
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.clear();
            selection.entities.push(target);

            for &e in &old_entities {
                if e != target
                    && let Ok(mut ec) = world.get_entity_mut(e)
                {
                    ec.remove::<Selected>();
                }
            }
            if let Ok(mut ec) = world.get_entity_mut(target) {
                ec.insert(Selected);
            }
        });
    }

    // Built-in context menu items. The "Add Child ..." entries are the
    // parent-aware variant: they spawn the entity and reparent it under
    // the right-clicked target.
    let mut owned_items: Vec<(String, String)> = vec![
        (
            "hierarchy.focus".into(),
            "Focus                    F".into(),
        ),
        ("hierarchy.rename".into(), "Rename              F2".into()),
        (
            "hierarchy.duplicate".into(),
            "Duplicate        Ctrl+D".into(),
        ),
        ("hierarchy.delete".into(), "Delete             Del".into()),
        (
            "hierarchy.save_template".into(),
            "Save as Template...".into(),
        ),
        ("hierarchy.add_cube".into(), "Add Child Cube".into()),
        ("hierarchy.add_sphere".into(), "Add Child Sphere".into()),
        ("hierarchy.add_light".into(), "Add Child Light".into()),
        ("hierarchy.add_empty".into(), "Add Child Empty".into()),
    ];

    // Append extension-contributed Add entries from the same source the
    // toolbar Add menu and the Add Entity picker use. One
    // `register_menu_entry` call therefore surfaces in all three places.
    let mut ext_rows: Vec<(String, String)> = extension_add_entries
        .iter()
        .filter(|entry| entry.menu == TopLevelMenu::Add)
        .map(|entry| {
            (
                format!("{OP_PREFIX}{}", entry.operator_id),
                format!("Add {}", entry.label),
            )
        })
        .collect();
    ext_rows.sort_by(|a, b| a.1.cmp(&b.1));
    owned_items.extend(ext_rows);

    let items: Vec<(&str, &str)> = owned_items
        .iter()
        .map(|(a, l)| (a.as_str(), l.as_str()))
        .collect();

    let menu = spawn_context_menu(&mut commands, cursor_pos, Some(target), &items);
    state.menu_entity = Some(menu);
    state.target_entity = Some(target);
    OperatorResult::Finished
}

/// Handle context menu actions for hierarchy operations.
fn on_context_menu_action(
    event: On<ContextMenuAction>,
    mut commands: Commands,
    global_transforms: Query<&GlobalTransform>,
    mut camera_query: Query<&mut Transform, With<jackdaw_camera::JackdawCameraSettings>>,
) {
    let target_entity = event.target_entity;

    match event.action.as_str() {
        "hierarchy.focus" => {
            if let Some(target) = target_entity
                && let Ok(global_tf) = global_transforms.get(target)
            {
                let target_pos = global_tf.translation();
                let scale = global_tf.compute_transform().scale;
                let dist = (scale.length() * 3.0).max(5.0);

                for mut transform in &mut camera_query {
                    let forward = transform.forward().as_vec3();
                    transform.translation = target_pos - forward * dist;
                    *transform = transform.looking_at(target_pos, Vec3::Y);
                }
            }
        }
        "hierarchy.rename" => {
            if let Some(target) = target_entity {
                commands
                    .operator(RenameBeginOp::ID)
                    .param("entity", target)
                    .call();
            }
        }
        "hierarchy.duplicate" => {
            commands.queue(|world: &mut World| {
                entity_ops::duplicate_selected(world);
            });
        }
        "hierarchy.delete" => {
            commands.queue(|world: &mut World| {
                entity_ops::delete_selected(world);
            });
        }
        "hierarchy.add_cube" => {
            if let Some(parent) = target_entity {
                commands.queue(move |world: &mut World| {
                    entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Cube);
                    // Reparent the newly created entity under the target
                    let selection = world.resource::<Selection>();
                    if let Some(new_entity) = selection.primary() {
                        world.entity_mut(new_entity).insert(ChildOf(parent));
                    }
                });
            }
        }
        "hierarchy.add_sphere" => {
            if let Some(parent) = target_entity {
                commands.queue(move |world: &mut World| {
                    entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Sphere);
                    let selection = world.resource::<Selection>();
                    if let Some(new_entity) = selection.primary() {
                        world.entity_mut(new_entity).insert(ChildOf(parent));
                    }
                });
            }
        }
        "hierarchy.add_light" => {
            if let Some(parent) = target_entity {
                commands.queue(move |world: &mut World| {
                    entity_ops::create_entity_in_world(
                        world,
                        entity_ops::EntityTemplate::PointLight,
                    );
                    let selection = world.resource::<Selection>();
                    if let Some(new_entity) = selection.primary() {
                        world.entity_mut(new_entity).insert(ChildOf(parent));
                    }
                });
            }
        }
        "hierarchy.add_empty" => {
            if let Some(parent) = target_entity {
                commands.queue(move |world: &mut World| {
                    entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Empty);
                    let selection = world.resource::<Selection>();
                    if let Some(new_entity) = selection.primary() {
                        world.entity_mut(new_entity).insert(ChildOf(parent));
                    }
                });
            }
        }
        "hierarchy.save_template" => {
            if let Some(target) = target_entity {
                // Store the target entity and open a dialog for template name
                commands.queue(move |world: &mut World| {
                    world
                        .resource_mut::<crate::entity_templates::PendingTemplateSave>()
                        .entity = Some(target);
                    // Get the entity name as default template name
                    let default_name = world
                        .get::<Name>(target)
                        .map(|n| n.as_str().to_string())
                        .unwrap_or_else(|| "template".to_string());
                    world.resource_mut::<PendingTemplateDefaultName>().0 = default_name;
                });
                commands.trigger(jackdaw_feathers::dialog::OpenDialogEvent::new(
                    "Save as Template",
                    "Save",
                ));
            }
        }
        action if action.starts_with(OP_PREFIX) => {
            // Extension-contributed Add entry. Dispatch through the same
            // path as the toolbar Add menu and the Add Entity picker so
            // operators behave identically regardless of which surface
            // invoked them.
            let operator_id = action.strip_prefix(OP_PREFIX).unwrap().to_string();
            commands.queue(move |world: &mut World| {
                world
                    .operator(operator_id)
                    .settings(CallOperatorSettings {
                        execution_context: ExecutionContext::Invoke,
                        creates_history_entry: true,
                    })
                    .call()
            });
        }
        _ => {}
    }
}

/// Toggle entity visibility when the eye icon is clicked.
fn on_visibility_toggled(
    event: On<TreeRowVisibilityToggled>,
    mut commands: Commands,
    visibility_query: Query<&Visibility>,
) {
    let source = event.source_entity;

    let current = visibility_query
        .get(source)
        .copied()
        .unwrap_or(Visibility::Inherited);

    let new_visibility = match current {
        Visibility::Hidden => Visibility::Inherited,
        _ => Visibility::Hidden,
    };

    let old_json = serde_json::Value::String(format!("{current:?}"));
    let new_json = serde_json::Value::String(format!("{new_visibility:?}"));

    let cmd = SetJsnField {
        entity: source,
        type_path: "bevy_camera::visibility::Visibility".to_string(),
        field_path: String::new(),
        old_value: old_json,
        new_value: new_json,
        was_derived: false,
    };

    commands.queue(move |world: &mut World| {
        let mut cmd = Box::new(cmd);
        cmd.execute(world);
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(cmd);
    });
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<RenameBeginOp>()
        .register_operator::<HierarchyOpenContextMenuOp>();
    let ext = ctx.id();
    ctx.spawn((
        Action::<HierarchyOpenContextMenuOp>::new(),
        ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
        bindings![(MouseButton::Right, Press::default())],
    ));
    ctx.spawn((
        Action::<RenameBeginOp>::new(),
        ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::F2, Press::default())],
    ));
}

/// Marker for inline rename `text_edit` entity, linking back to the label entity and source entity.
#[derive(Component)]
struct InlineRenameInput {
    label_entity: Entity,
    source_entity: Entity,
}

fn on_tree_row_start_rename(event: On<TreeRowStartRename>, mut commands: Commands) {
    let target = event.source_entity;
    commands
        .operator(RenameBeginOp::ID)
        .param("entity", target)
        .call();
}

/// `is_available` for `hierarchy.rename_begin`: only fires when no
/// inline rename is already in progress.
fn no_rename_in_progress(rename_check: Query<(), With<InlineRenameInput>>) -> bool {
    rename_check.is_empty()
}

/// Pick the entity to rename: the explicit `entity` operator
/// parameter wins (used by the context-menu "Rename" action and the
/// `TreeRowStartRename` event), otherwise fall back to the primary
/// selection so a bare F2 press renames whatever the user has
/// highlighted in the outliner. Pulled out so the regression check
/// for the F2-without-selection path can run as a unit test.
pub(crate) fn resolve_rename_target(
    params: &OperatorParameters,
    selection: &Selection,
) -> Option<Entity> {
    params.as_entity("entity").or_else(|| selection.primary())
}

fn entity_name(names: &Query<&Name>, entity: Entity) -> String {
    names
        .get(entity)
        .map(|n| n.as_str().to_string())
        .unwrap_or_default()
}

/// Resolve the label entity and its containing row for a scene entity's tree node.
fn find_rename_targets(
    source: Entity,
    tree_index: &TreeIndex,
    tree_nodes: &Query<&Children, With<TreeNode>>,
    content_query: &Query<(Entity, &Children), With<TreeRowContent>>,
    label_query: &Query<Entity, With<TreeRowLabel>>,
) -> Option<(Entity, Entity)> {
    let tree_entity = tree_index.get(source)?;
    let children = tree_nodes.get(tree_entity).ok()?;
    for child in children.iter() {
        if let Ok((content_e, content_children)) = content_query.get(child) {
            for grandchild in content_children.iter() {
                if label_query.contains(grandchild) {
                    return Some((grandchild, content_e));
                }
            }
        }
    }
    None
}

/// Custom command: drop the inline-rename marker from a tree-row label
/// and restore its displayed text + visibility. Issued from rename
/// commit/cancel paths so the queue boundary is explicit.
struct RestoreLabel {
    label_entity: Entity,
    text: String,
}

impl Command for RestoreLabel {
    fn apply(self, world: &mut World) {
        let Ok(mut ec) = world.get_entity_mut(self.label_entity) else {
            return;
        };
        ec.remove::<TreeRowInlineRename>();
        ec.insert(Text::new(self.text));
        if let Some(mut node) = ec.get_mut::<Node>() {
            node.display = Display::Flex;
        }
    }
}

/// Begin inline rename of an entity in the hierarchy tree.
#[operator(
    id = "hierarchy.rename_begin",
    label = "Rename Entity",
    description = "Rename the selected entity in the hierarchy.",
    modal = true,
    cancel = cancel_rename_begin,
    is_available = no_rename_in_progress,
    params(entity(Entity, doc = "Scene entity to rename.")),
)]
pub fn rename_begin(
    params: In<OperatorParameters>,
    mut commands: Commands,
    tree_index: Res<TreeIndex>,
    tree_nodes: Query<&Children, With<TreeNode>>,
    content_query: Query<(Entity, &Children), With<TreeRowContent>>,
    label_query: Query<Entity, With<TreeRowLabel>>,
    names: Query<&Name>,
    rename_inputs: Query<(), With<InlineRenameInput>>,
    active: ActiveModalQuery,
    selection: Res<Selection>,
) -> OperatorResult {
    if active.is_modal_running() {
        return if rename_inputs.is_empty() {
            OperatorResult::Finished
        } else {
            OperatorResult::Running
        };
    }

    let Some(source) = resolve_rename_target(&params, &selection) else {
        return OperatorResult::Cancelled;
    };
    let Some((label_entity, content_entity)) = find_rename_targets(
        source,
        &tree_index,
        &tree_nodes,
        &content_query,
        &label_query,
    ) else {
        return OperatorResult::Cancelled;
    };

    commands.entity(label_entity).insert(TreeRowInlineRename);
    commands
        .entity(label_entity)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.display = Display::None;
        });

    commands.spawn((
        InlineRenameInput {
            label_entity,
            source_entity: source,
        },
        text_edit::text_edit(
            TextEditProps::default()
                .with_default_value(entity_name(&names, source))
                .allow_empty(),
        ),
        ChildOf(content_entity),
    ));
    OperatorResult::Running
}

fn cancel_rename_begin(
    mut commands: Commands,
    rename_query: Query<(Entity, &InlineRenameInput)>,
    names: Query<&Name>,
    mut input_focus: ResMut<InputFocus>,
) {
    for (rename_entity, inline_rename) in &rename_query {
        input_focus.clear();
        let original = entity_name(&names, inline_rename.source_entity);
        commands.queue(RestoreLabel {
            label_entity: inline_rename.label_entity,
            text: original,
        });
        commands.entity(rename_entity).despawn();
    }
}

/// Auto-focus inline rename `text_edit` inputs one frame after spawn.
fn auto_focus_inline_rename(
    rename_inputs: Query<(Entity, &InlineRenameInput, &Children)>,
    wrappers: Query<&jackdaw_feathers::text_edit::TextEditConfig>,
    wrapper_children: Query<&Children>,
    editor_text_edits: Query<Entity, With<EditorTextEdit>>,
    mut input_focus: ResMut<InputFocus>,
) {
    for (_rename_entity, _inline, children) in &rename_inputs {
        // The text_edit outer entity has children: [wrapper] which has children: [..., EditorTextEdit]
        for child in children.iter() {
            if wrappers.contains(child) {
                // this is the label/wrapper -- skip, we need the actual wrapper node
                continue;
            }
            // child might be the wrapper entity (has TextEditWrapper inside)
            if let Ok(wrapper_kids) = wrapper_children.get(child) {
                for wk in wrapper_kids.iter() {
                    if editor_text_edits.contains(wk) {
                        if input_focus.0 != Some(wk) {
                            input_focus.0 = Some(wk);
                        }
                        return;
                    }
                }
            }
        }
    }
}

/// Handle `TextEditCommitEvent` for inline renames.
fn handle_inline_rename_commit(
    event: On<TextEditCommitEvent>,
    rename_inputs: Query<(Entity, &InlineRenameInput)>,
    child_of_query: Query<&ChildOf>,
    mut commands: Commands,
    mut input_focus: ResMut<InputFocus>,
) {
    // Walk up from the committed entity to find if it belongs to an InlineRenameInput
    // event.entity is the inner EditorTextEdit → parent is wrapper → parent is text_edit outer → parent is content
    // The InlineRenameInput is on the text_edit outer entity
    let mut current = event.entity;
    let mut found = None;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        if let Ok((rename_entity, inline_rename)) = rename_inputs.get(child_of.parent()) {
            found = Some((
                rename_entity,
                inline_rename.label_entity,
                inline_rename.source_entity,
            ));
            break;
        }
        current = child_of.parent();
    }

    let Some((rename_entity, label_entity, source_entity)) = found else {
        return;
    };

    input_focus.clear();
    commands.queue(RestoreLabel {
        label_entity,
        text: event.text.clone(),
    });
    commands.entity(rename_entity).despawn();

    // Trigger the rename
    commands.trigger(TreeRowRenamed {
        entity: label_entity,
        source_entity,
        new_name: event.text.clone(),
    });
}

/// Commit inline rename: update Name with undo.
fn on_tree_row_renamed(event: On<TreeRowRenamed>, mut commands: Commands, names: Query<&Name>) {
    let source = event.source_entity;
    let new_name = event.new_name.clone();

    // Apply name change with undo
    let old_name = names
        .get(source)
        .map(|n| n.as_str().to_string())
        .unwrap_or_default();

    if old_name == new_name {
        return;
    }

    commands.queue(move |world: &mut World| {
        let cmd = SetJsnField {
            entity: source,
            type_path: "bevy_ecs::name::Name".to_string(),
            field_path: String::new(),
            old_value: serde_json::Value::String(old_name),
            new_value: serde_json::Value::String(new_name),
            was_derived: false,
        };
        let mut cmd = Box::new(cmd);
        cmd.execute(world);
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(cmd);
    });
}

/// When the template dialog opens, populate its children slot with a name input.
fn populate_template_dialog(
    mut commands: Commands,
    pending: Res<crate::entity_templates::PendingTemplateSave>,
    default_name: Res<PendingTemplateDefaultName>,
    slots: Query<(Entity, &Children), (With<DialogChildrenSlot>, Changed<Children>)>,
    existing_inputs: Query<(), With<TemplateNameInput>>,
) {
    // Only act when there's a pending template save
    if pending.entity.is_none() {
        return;
    }
    // Don't re-populate if we already have an input
    if !existing_inputs.is_empty() {
        return;
    }
    for (slot_entity, children) in &slots {
        if children.is_empty() {
            commands.spawn((
                TemplateNameInput,
                text_edit::text_edit(
                    TextEditProps::default()
                        .with_placeholder("Template name...")
                        .with_default_value(default_name.0.clone())
                        .allow_empty(),
                ),
                ChildOf(slot_entity),
            ));
        }
    }
}

/// When the dialog's action button is clicked, save the template.
fn on_template_dialog_action(
    _event: On<DialogActionEvent>,
    mut commands: Commands,
    pending: Res<crate::entity_templates::PendingTemplateSave>,
    name_inputs: Query<&TextEditValue, With<TemplateNameInput>>,
) {
    let Some(_entity) = pending.entity else {
        return;
    };

    let name = name_inputs
        .iter()
        .next()
        .map(|input| input.0.trim().to_string())
        .unwrap_or_default();

    if name.is_empty() {
        return;
    }

    commands.queue(move |world: &mut World| {
        crate::entity_templates::save_entity_template(world, &name);
        world
            .resource_mut::<crate::entity_templates::PendingTemplateSave>()
            .entity = None;
    });
}

/// Toggle the show-all state when the button is pressed.
fn toggle_show_all_button(
    mut show_all: ResMut<HierarchyShowAll>,
    interactions: Query<&Interaction, (Changed<Interaction>, With<HierarchyShowAllButton>)>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            show_all.0 = !show_all.0;
        }
    }
}

/// Style hierarchy rows whose source entity was spawned during
/// `PlayState::Playing` (i.e. has the `GameSpawned` marker) in
/// italic, so the user can tell at a glance which rows are
/// authored vs transient runtime state that'll disappear on Stop.
///
/// Uses the `EditorFontItalic` handle loaded by
/// `jackdaw_feathers::icons`. Text colour stays at the theme's
/// primary foreground; only the font handle changes. Runs every
/// frame while in `Editor` state; the body is a few pointer-chasing
/// lookups per game-spawned entity; cheap enough to skip change
/// detection. We only write when the font differs to keep bevy's
/// `Changed<TextFont>` quiet for downstream consumers.
fn style_game_spawned_rows(
    game_spawned: Query<Entity, With<crate::pie::GameSpawned>>,
    index: Res<jackdaw_widgets::tree_view::TreeIndex>,
    italic_font: Option<Res<jackdaw_feathers::icons::EditorFontItalic>>,
    children_q: Query<&Children>,
    row_content_q: Query<(), With<jackdaw_widgets::tree_view::TreeRowContent>>,
    label_q: Query<(), With<jackdaw_widgets::tree_view::TreeRowLabel>>,
    mut text_fonts: Query<&mut TextFont>,
) {
    let Some(italic_font) = italic_font else {
        return;
    };
    for source in &game_spawned {
        let Some(row_entity) = index.get(source) else {
            continue;
        };
        let Ok(row_children) = children_q.get(row_entity) else {
            continue;
        };
        for content in row_children.iter() {
            if !row_content_q.contains(content) {
                continue;
            }
            let Ok(content_children) = children_q.get(content) else {
                continue;
            };
            for maybe_label in content_children.iter() {
                if !label_q.contains(maybe_label) {
                    continue;
                }
                if let Ok(mut tf) = text_fonts.get_mut(maybe_label)
                    && tf.font != italic_font.0
                {
                    tf.font = italic_font.0.clone();
                }
            }
        }
    }
}

/// Update the show-all button icon color based on active state.
fn update_show_all_button_appearance(
    show_all: Res<HierarchyShowAll>,
    buttons: Query<&Children, With<HierarchyShowAllButton>>,
    mut text_colors: Query<&mut TextColor>,
) {
    if !show_all.is_changed() {
        return;
    }
    let color = if show_all.0 {
        tokens::TEXT_PRIMARY
    } else {
        tokens::TEXT_SECONDARY
    };
    for children in &buttons {
        for child in children.iter() {
            if let Ok(mut tc) = text_colors.get_mut(child) {
                tc.0 = color;
            }
        }
    }
}

/// When the show-all toggle changes, clear and rebuild the hierarchy.
fn on_show_all_changed(show_all: Res<HierarchyShowAll>, mut commands: Commands) {
    if show_all.is_changed() && !show_all.is_added() {
        commands.queue(|world: &mut World| {
            if let Err(err) = world.run_system_cached(clear_all_tree_rows) {
                error!("Failed to clear tree rows: {err}");
            }
            rebuild_hierarchy(world)
        });
    }
}

/// Despawn all tree rows and clear the `TreeIndex`.
pub fn clear_all_tree_rows(
    world: &mut World,
    container: &mut SystemState<Option<Single<Entity, With<HierarchyTreeContainer>>>>,
) {
    let Some(container) = container.get(world) else {
        // No tree container mounted (e.g. headless test); nothing to clear.
        return;
    };
    let container = *container;

    // Collect tree row children of the container
    let tree_rows: Vec<Entity> = world
        .get::<Children>(container)
        .map(|c| c.iter().collect())
        .unwrap_or_default();

    for row in tree_rows {
        if let Ok(ec) = world.get_entity_mut(row) {
            ec.despawn();
        }
    }

    world.resource_mut::<TreeIndex>().clear();
}

/// Filter hierarchy tree rows based on the filter text input.
fn apply_hierarchy_filter(
    filter_input: Query<&TextEditValue, (With<HierarchyFilter>, Changed<TextEditValue>)>,
    tree_nodes: Query<(Entity, &TreeNode)>,
    names: Query<&Name>,
    parent_query: Query<&ChildOf>,
    tree_row_children_query: Query<(), With<TreeRowChildren>>,
    mut display_query: Query<&mut Node>,
) {
    let Ok(text_edit_value) = filter_input.single() else {
        return;
    };

    let filter = text_edit_value.0.trim().to_lowercase();

    if filter.is_empty() {
        for (tree_entity, _) in &tree_nodes {
            if let Ok(mut node) = display_query.get_mut(tree_entity) {
                node.display = Display::Flex;
            }
        }
        return;
    }

    // First pass: determine which source entities match the filter
    let mut visible_tree_entities: HashSet<Entity> = HashSet::new();

    for (tree_entity, tree_node) in &tree_nodes {
        let label = names
            .get(tree_node.0)
            .map(|n| n.as_str().to_lowercase())
            .unwrap_or_else(|_| format!("entity {}", tree_node.0).to_lowercase());
        let matches = label.contains(&filter);

        if matches {
            visible_tree_entities.insert(tree_entity);

            // Walk up ancestors: tree row → ChildOf → TreeRowChildren → ChildOf → parent tree row
            let mut current = tree_entity;
            while let Ok(&ChildOf(parent)) = parent_query.get(current) {
                if tree_row_children_query.contains(parent) {
                    if let Ok(&ChildOf(grandparent)) = parent_query.get(parent) {
                        visible_tree_entities.insert(grandparent);
                        current = grandparent;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }

    // Second pass: set display on all tree rows
    for (tree_entity, _) in &tree_nodes {
        if let Ok(mut node) = display_query.get_mut(tree_entity) {
            node.display = if visible_tree_entities.contains(&tree_entity) {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_api_internal::operator::OperatorParameters;
    use jackdaw_jsn::PropertyValue;
    use std::collections::BTreeMap;

    fn empty_params() -> OperatorParameters {
        OperatorParameters(BTreeMap::new())
    }

    fn params_with_entity(key: &str, entity: Entity) -> OperatorParameters {
        let mut map = BTreeMap::new();
        map.insert(key.to_string(), PropertyValue::Entity(entity));
        OperatorParameters(map)
    }

    /// `RenameBeginOp` dispatched with an explicit `entity` param
    /// (the path the context-menu "Rename" item and the
    /// `TreeRowStartRename` event use) returns that entity. The
    /// param wins over any selection state.
    #[test]
    fn resolve_rename_target_prefers_entity_param() {
        let target = Entity::from_raw_u32(7).unwrap();
        let other = Entity::from_raw_u32(42).unwrap();
        let params = params_with_entity("entity", target);
        let selection = Selection {
            entities: vec![other],
        };
        assert_eq!(resolve_rename_target(&params, &selection), Some(target));
    }

    /// F2 keybind regression cover: the bare keypress dispatches
    /// `RenameBeginOp` with no params, and the operator must read
    /// the primary selection. Before the fix, the op early-returned
    /// `Cancelled` whenever no `entity` param was supplied, so F2
    /// silently did nothing even with a selected outliner row.
    #[test]
    fn resolve_rename_target_falls_back_to_selection_primary() {
        let primary = Entity::from_raw_u32(11).unwrap();
        let params = empty_params();
        let selection = Selection {
            // The last entry is the primary selection.
            entities: vec![Entity::from_raw_u32(99).unwrap(), primary],
        };
        assert_eq!(resolve_rename_target(&params, &selection), Some(primary));
    }

    /// No param, no selection: the op cancels. Confirms the early
    /// bail still fires, so a stray F2 in an empty scene doesn't
    /// fall into find-rename-targets with a garbage entity.
    #[test]
    fn resolve_rename_target_returns_none_without_selection_or_param() {
        let params = empty_params();
        let selection = Selection::default();
        assert_eq!(resolve_rename_target(&params, &selection), None);
    }
}
