use std::collections::HashMap as StdHashMap;

use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use jackdaw_feathers::text_edit::{
    self, EditorTextEdit, TextEditCommitEvent, TextEditConfig, TextEditProps,
};
use lucide_icons::Icon;

use crate::tree::DockTree;
use crate::workspace::WorkspaceDescriptor;
use crate::{
    IconFontHandle,
    workspace::{WorkspaceChanged, WorkspaceRegistry, WorkspaceTab, WorkspaceTabStrip},
};

const TAB_ACTIVE_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
const TAB_ACTIVE_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.1);
const TAB_ACTIVE_LABEL: Color = Color::srgba(1.0, 1.0, 1.0, 0.9);
const TAB_INACTIVE_LABEL: Color = Color::srgba(1.0, 1.0, 1.0, 0.4);

const NEW_WORKSPACE_ACCENT: Color = Color::srgba(0.55, 0.7, 1.0, 0.8);
const DOUBLE_CLICK_THRESHOLD_S: f64 = 0.35;

#[derive(Component)]
pub struct AddWorkspaceButton;

#[derive(Component)]
pub struct WorkspaceTabLabel {
    pub workspace_id: String,
}

#[derive(Component)]
pub struct WorkspaceTabCloseButton {
    pub workspace_id: String,
}

#[derive(Component)]
pub struct WorkspaceRenameInput {
    pub workspace_id: String,
    pub label_entity: Entity,
}

#[derive(Resource, Default)]
pub struct WorkspaceClickTracker {
    last_click_time: StdHashMap<Entity, f64>,
}

/// Snapshot of the workspace id list so `populate_workspace_tabs` can
/// detect structural changes (add/remove) without rebuilding on every
/// `registry.is_changed()` (which fires on per-tab tree edits + saves).
#[derive(Resource, Default)]
pub struct WorkspaceListSnapshot {
    ids: Vec<String>,
}

/// Re-populate the strip when the workspace **id list** changes
/// (add/remove) or a strip is freshly empty. Renames update labels
/// in-place via `handle_workspace_rename_commit`; per-tab `tree`
/// mutations don't touch the strip at all. This avoids despawning the
/// X / rename input out from under their deferred init systems.
pub fn populate_workspace_tabs(
    world: &mut World,
    workspace_strips: &mut QueryState<Entity, (With<WorkspaceTabStrip>, Without<WorkspaceTab>)>,
) {
    let current_ids: Vec<String> = world
        .resource::<WorkspaceRegistry>()
        .workspaces
        .iter()
        .map(|w| w.id.clone())
        .collect();
    let last_ids = world.resource::<WorkspaceListSnapshot>().ids.clone();
    let ids_changed = current_ids != last_ids;

    let mut strips: Vec<Entity> = Vec::new();
    {
        for entity in workspace_strips.iter(world) {
            let is_empty = world
                .entity(entity)
                .get::<Children>()
                .is_none_or(RelationshipTarget::is_empty);
            if is_empty || ids_changed {
                strips.push(entity);
            }
        }
    }

    if strips.is_empty() {
        return;
    }

    // Despawn existing children of any strip we're rebuilding (so
    // workspace add/remove appears without leaking old tabs).
    for &strip in &strips {
        let children: Vec<Entity> = world
            .entity(strip)
            .get::<Children>()
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        for child in children {
            if let Ok(em) = world.get_entity_mut(child) {
                em.despawn();
            }
        }
    }

    let registry = world.remove_resource::<WorkspaceRegistry>().unwrap();
    let icon_font = world.get_resource::<IconFontHandle>().map(|f| f.0.clone());

    for strip_entity in strips {
        for workspace in registry.iter() {
            spawn_workspace_tab(
                world,
                strip_entity,
                workspace,
                &registry,
                icon_font.as_ref(),
            );
        }
        spawn_add_workspace_button(world, strip_entity, icon_font.as_ref());
    }

    world.insert_resource(registry);
    world.resource_mut::<WorkspaceListSnapshot>().ids = current_ids;
}

fn spawn_workspace_tab(
    world: &mut World,
    strip: Entity,
    workspace: &WorkspaceDescriptor,
    registry: &WorkspaceRegistry,
    icon_font: Option<&Handle<Font>>,
) {
    let is_active = registry.active.as_ref() == Some(&workspace.id);
    let bg = if is_active {
        TAB_ACTIVE_BG
    } else {
        Color::NONE
    };
    let border = if is_active {
        TAB_ACTIVE_BORDER
    } else {
        Color::NONE
    };
    let label_color = if is_active {
        TAB_ACTIVE_LABEL
    } else {
        TAB_INACTIVE_LABEL
    };

    let tab_entity = world
        .spawn((
            WorkspaceTab {
                workspace_id: workspace.id.clone(),
            },
            Interaction::default(),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(7.0), Val::Px(4.0)),
                column_gap: Val::Px(5.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(bg),
            BorderColor::all(border),
            ChildOf(strip),
        ))
        .id();

    world.spawn((
        Node {
            width: Val::Px(2.5),
            height: Val::Px(12.0),
            border_radius: BorderRadius::all(Val::Px(5.0)),
            ..default()
        },
        BackgroundColor(workspace.accent_color),
        ChildOf(tab_entity),
    ));

    if let Some(icon_char) = &workspace.icon {
        let mut font = TextFont {
            font_size: 12.0,
            ..default()
        };
        if let Some(handle) = icon_font {
            font.font = handle.clone();
        }
        world.spawn((
            Text::new(icon_char.clone()),
            font,
            TextColor(label_color),
            ChildOf(tab_entity),
        ));
    }

    world.spawn((
        WorkspaceTabLabel {
            workspace_id: workspace.id.clone(),
        },
        Text::new(workspace.name.clone()),
        TextFont {
            font_size: 11.0,
            ..default()
        },
        TextColor(label_color),
        ChildOf(tab_entity),
    ));

    // Hover-visible close (X) button.
    if let Some(handle) = icon_font {
        let close_btn = world
            .spawn((
                WorkspaceTabCloseButton {
                    workspace_id: workspace.id.clone(),
                },
                Interaction::default(),
                Node {
                    width: Val::Px(14.0),
                    height: Val::Px(14.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(Val::Px(2.0)),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(Color::NONE),
                ChildOf(tab_entity),
            ))
            .id();
        world.spawn((
            Text::new(String::from(Icon::X.unicode())),
            TextFont {
                font: handle.clone(),
                font_size: 10.0,
                ..default()
            },
            TextColor(TAB_INACTIVE_LABEL),
            ChildOf(close_btn),
        ));
    }
}

fn spawn_add_workspace_button(world: &mut World, strip: Entity, icon_font: Option<&Handle<Font>>) {
    let btn = world
        .spawn((
            AddWorkspaceButton,
            Interaction::default(),
            Node {
                width: Val::Px(20.0),
                height: Val::Px(20.0),
                margin: UiRect::left(Val::Px(4.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(strip),
        ))
        .id();

    let mut font = TextFont {
        font_size: 14.0,
        ..default()
    };
    if let Some(handle) = icon_font {
        font.font = handle.clone();
    }
    world.spawn((
        Text::new(String::from(Icon::Plus.unicode())),
        font,
        TextColor(TAB_INACTIVE_LABEL),
        ChildOf(btn),
    ));
}

/// Show the close (X) child whenever the workspace tab is hovered or
/// pressed. Hide it otherwise. Mirrors `tabs::show_close_on_hover` for
/// dock tabs.
pub fn show_workspace_close_on_hover(
    tabs: Query<(&Interaction, &Children), (Changed<Interaction>, With<WorkspaceTab>)>,
    mut close_buttons: Query<&mut Node, With<WorkspaceTabCloseButton>>,
) {
    for (interaction, children) in tabs.iter() {
        let show = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        for child in children.iter() {
            if let Ok(mut node) = close_buttons.get_mut(child) {
                node.display = if show { Display::Flex } else { Display::None };
            }
        }
    }
}

pub fn handle_workspace_tab_clicks(
    tab_query: Query<(&WorkspaceTab, &Interaction), Changed<Interaction>>,
    registry: Res<WorkspaceRegistry>,
    mut commands: Commands,
) {
    for (tab, interaction) in tab_query.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }

        let new_id = &tab.workspace_id;
        if registry.active.as_ref() == Some(new_id) {
            continue;
        }

        let old = registry.active.clone();
        // The observer is the sole owner of `registry.active`; setting
        // it eagerly here would race with auto-save (save snapshots live
        // → active and would corrupt the incoming workspace).
        commands.trigger(WorkspaceChanged {
            old,
            new: new_id.clone(),
        });
    }
}

/// Click `+` to create a new workspace. Copies the current `DockTree`
/// (so the new workspace starts visually identical to the current one)
/// and switches to it.
pub fn handle_add_workspace_clicks(
    button_query: Query<&Interaction, (Changed<Interaction>, With<AddWorkspaceButton>)>,
    mut registry: ResMut<WorkspaceRegistry>,
    tree: Res<DockTree>,
    mut commands: Commands,
) {
    for interaction in button_query.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }

        let next_index = registry.workspaces.len() + 1;
        let new_id = format!("workspace_{next_index}");
        let new_name = format!("Workspace {next_index}");

        let current_active = registry.active.clone();
        if let Some(active_id) = current_active.as_ref()
            && let Some(ws) = registry.get_mut(active_id)
        {
            ws.tree = tree.clone();
        }

        registry.workspaces.push(WorkspaceDescriptor {
            id: new_id.clone(),
            name: new_name,
            icon: None,
            accent_color: NEW_WORKSPACE_ACCENT,
            layout: crate::layout::LayoutState::default(),
            tree: tree.clone(),
        });

        let old = registry.active.clone();
        commands.trigger(WorkspaceChanged { old, new: new_id });
    }
}

/// Click X on a tab → delete that workspace. Last workspace can't be
/// deleted. Active-deleted falls through to the first remaining via
/// `WorkspaceChanged`.
pub fn on_workspace_close_click(
    trigger: On<Pointer<Click>>,
    close_buttons: Query<&WorkspaceTabCloseButton>,
    mut registry: ResMut<WorkspaceRegistry>,
    tree: Res<DockTree>,
    mut commands: Commands,
) {
    let Ok(close_btn) = close_buttons.get(trigger.event_target()) else {
        return;
    };
    if registry.workspaces.len() <= 1 {
        return;
    }

    let target = close_btn.workspace_id.clone();
    let was_active = registry.active.as_deref() == Some(&target);

    if was_active && let Some(ws) = registry.get_mut(&target) {
        ws.tree = tree.clone();
    }

    registry.workspaces.retain(|w| w.id != target);

    if was_active {
        let new_active = registry.workspaces.first().map(|w| w.id.clone());
        if let Some(new_id) = new_active {
            commands.trigger(WorkspaceChanged {
                old: Some(target),
                new: new_id,
            });
        }
    }
}

/// Detect double-click on a workspace tab → start inline rename.
/// Tracks the previous click time per-entity in a `Local`-style
/// `Resource` so we can measure the gap.
pub fn detect_workspace_double_click(
    trigger: On<Pointer<Click>>,
    tabs: Query<(&WorkspaceTab, &Children)>,
    labels: Query<(Entity, &WorkspaceTabLabel)>,
    mut tracker: ResMut<WorkspaceClickTracker>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok((tab, children)) = tabs.get(entity) else {
        return;
    };

    let now = time.elapsed_secs_f64();
    let prev = tracker.last_click_time.insert(entity, now);
    let Some(prev_time) = prev else {
        return;
    };
    if now - prev_time >= DOUBLE_CLICK_THRESHOLD_S {
        return;
    }

    // Reset so a triple-click doesn't re-fire.
    tracker.last_click_time.remove(&entity);

    let label_entity: Option<Entity> = children
        .iter()
        .find_map(|child| labels.get(child).ok().map(|(e, _)| e));
    let Some(label_entity) = label_entity else {
        return;
    };
    let workspace_id = tab.workspace_id.clone();

    commands.queue(move |world: &mut World| {
        start_workspace_rename(world, entity, label_entity, &workspace_id);
    });
}

fn start_workspace_rename(
    world: &mut World,
    tab_entity: Entity,
    label_entity: Entity,
    workspace_id: &str,
) {
    let current_name = world
        .resource::<WorkspaceRegistry>()
        .get(workspace_id)
        .map(|w| w.name.clone())
        .unwrap_or_default();

    if let Some(mut node) = world.entity_mut(label_entity).get_mut::<Node>() {
        node.display = Display::None;
    }

    // Spawn the text_edit after the label, but slot it into the label's
    // child index so it visually replaces the label.
    let label_index = world
        .entity(tab_entity)
        .get::<Children>()
        .and_then(|c| c.iter().position(|e| e == label_entity));

    let rename_entity = world
        .spawn((
            WorkspaceRenameInput {
                workspace_id: workspace_id.to_string(),
                label_entity,
            },
            text_edit::text_edit(
                TextEditProps::default()
                    .with_default_value(current_name)
                    .allow_empty(),
            ),
            ChildOf(tab_entity),
        ))
        .id();

    if let Some(idx) = label_index {
        world
            .entity_mut(tab_entity)
            .insert_children(idx, &[rename_entity]);
    }
}

/// Auto-focus a freshly-spawned workspace rename `text_edit`. Walks down
/// to the inner `EditorTextEdit` (same nesting as inline renames in
/// hierarchy.rs) and sets it as the focused entity.
pub fn auto_focus_workspace_rename(
    rename_inputs: Query<(&WorkspaceRenameInput, &Children), Added<WorkspaceRenameInput>>,
    wrappers: Query<&TextEditConfig>,
    wrapper_children: Query<&Children>,
    editor_text_edits: Query<Entity, With<EditorTextEdit>>,
    mut input_focus: ResMut<InputFocus>,
) {
    for (_inline, children) in &rename_inputs {
        for child in children.iter() {
            if wrappers.contains(child) {
                continue;
            }
            if let Ok(wrapper_kids) = wrapper_children.get(child) {
                for wk in wrapper_kids.iter() {
                    if editor_text_edits.contains(wk) {
                        input_focus.0 = Some(wk);
                        return;
                    }
                }
            }
        }
    }
}

/// Commit (Enter / blur) finishes the rename: writes the new name into
/// the workspace, despawns the `text_edit`, and restores the label.
pub fn handle_workspace_rename_commit(
    event: On<TextEditCommitEvent>,
    rename_inputs: Query<(Entity, &WorkspaceRenameInput)>,
    child_of_query: Query<&ChildOf>,
    mut registry: ResMut<WorkspaceRegistry>,
    mut commands: Commands,
    mut input_focus: ResMut<InputFocus>,
) {
    let mut current = event.entity;
    let mut found = None;
    for _ in 0..4 {
        let Ok(co) = child_of_query.get(current) else {
            break;
        };
        if let Ok((rename_entity, rename)) = rename_inputs.get(co.parent()) {
            found = Some((
                rename_entity,
                rename.label_entity,
                rename.workspace_id.clone(),
            ));
            break;
        }
        current = co.parent();
    }

    let Some((rename_entity, label_entity, workspace_id)) = found else {
        return;
    };

    input_focus.clear();

    let new_name = event.text.trim().to_string();
    if !new_name.is_empty() {
        if let Some(ws) = registry.get_mut(&workspace_id) {
            ws.name = new_name.clone();
        }
        commands.entity(label_entity).insert(Text::new(new_name));
    }

    // Restore label visibility and despawn the rename input.
    commands
        .entity(label_entity)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.display = Display::Flex;
        });
    commands.entity(rename_entity).despawn();
}

/// Swap the live `DockTree` resource on workspace switch. Saves the
/// outgoing workspace's tree, loads the incoming, then promotes the
/// incoming to active. Owning `registry.active` here (instead of in the
/// click handler) ensures auto-save can never see "active is the new
/// workspace" while the live tree still belongs to the outgoing one.
pub fn on_workspace_changed_swap_tree(
    trigger: On<WorkspaceChanged>,
    mut tree: ResMut<DockTree>,
    mut registry: ResMut<WorkspaceRegistry>,
) {
    let event = trigger.event();

    if let Some(old_id) = &event.old
        && let Some(ws) = registry.get_mut(old_id)
    {
        ws.tree = tree.clone();
    }

    // If the incoming workspace has a populated tree, switch to it.
    // Otherwise inherit the current tree so the new workspace at least
    // has a valid layout to start with (caller will reseed defaults if
    // the workspace was registered with an empty tree).
    let target_tree = match registry.get(&event.new) {
        Some(ws) if ws.tree.root.is_some() => ws.tree.clone(),
        Some(_) => tree.clone(),
        None => return,
    };

    if let Some(ws) = registry.get_mut(&event.new) {
        ws.tree = target_tree.clone();
    }
    *tree = target_tree;
    registry.active = Some(event.new.clone());
}

pub fn update_workspace_tab_visuals(
    registry: Res<WorkspaceRegistry>,
    tabs: Query<(Entity, &WorkspaceTab)>,
    mut bg_query: Query<&mut BackgroundColor>,
    mut border_query: Query<&mut BorderColor>,
    children_query: Query<&Children>,
    mut text_color_query: Query<&mut TextColor>,
) {
    if !registry.is_changed() {
        return;
    }

    for (tab_entity, tab) in tabs.iter() {
        let is_active = registry.active.as_ref() == Some(&tab.workspace_id);

        if let Ok(mut bg) = bg_query.get_mut(tab_entity) {
            bg.0 = if is_active {
                TAB_ACTIVE_BG
            } else {
                Color::NONE
            };
        }
        if let Ok(mut bc) = border_query.get_mut(tab_entity) {
            *bc = BorderColor::all(if is_active {
                TAB_ACTIVE_BORDER
            } else {
                Color::NONE
            });
        }

        if let Ok(children) = children_query.get(tab_entity) {
            for child in children.iter() {
                if let Ok(mut tc) = text_color_query.get_mut(child) {
                    tc.0 = if is_active {
                        TAB_ACTIVE_LABEL
                    } else {
                        TAB_INACTIVE_LABEL
                    };
                }
            }
        }
    }
}
