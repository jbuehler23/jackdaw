use std::collections::{HashMap, HashSet};

use bevy::{prelude::*, tasks::Task, tasks::futures_lite::future};
use bevy_monitors::prelude::{Mutation, NotifyChanged};
use jackdaw_feathers::{
    panel_header, tokens,
    tree_view::{TreeRowStyle, tree_row},
};
use jackdaw_remote::scene_snapshot::RemoteEntity;
use jackdaw_widgets::tree_view::{
    EntityCategory, TreeChildrenPopulated, TreeNode, TreeNodeExpanded, TreeRowChildren,
    TreeRowClicked, TreeRowContent, TreeRowLabel, TreeRowSelected,
};

use super::connection::ConnectionManager;

/// Marker component for remote entity proxies (prevents local hierarchy from acting on them).
#[derive(Component)]
pub struct RemoteEntityProxy {
    pub remote_bits: u64,
}

/// Display name extracted from snapshot, stored on proxy entities.
/// `bevy_monitors` watches this for Mutation events.
#[derive(Component, Default)]
pub struct RemoteEntityName(pub Option<String>);

/// Marker for the remote hierarchy panel container.
#[derive(Component)]
pub struct RemoteHierarchyPanel;

/// Marker for the scrollable tree container inside the remote panel.
#[derive(Component)]
pub struct RemoteTreeContainer;

/// Marker for the entity count / status text.
#[derive(Component)]
pub struct RemoteEntityStatusText;

/// Cached scene snapshot from the remote game.
#[derive(Resource, Default)]
pub(crate) struct RemoteSceneCache {
    pub(crate) entities: Vec<RemoteEntity>,
}

/// Maps remote entity bits → local proxy entity.
#[derive(Resource, Default)]
pub(crate) struct RemoteProxyIndex {
    pub(crate) map: HashMap<u64, Entity>,
}

/// Reverse lookup: proxy entity → tree row entity.
#[derive(Resource, Default)]
pub(crate) struct RemoteTreeRowIndex {
    pub(crate) map: HashMap<Entity, Entity>,
}

/// Tracks the currently selected remote entity.
#[derive(Resource, Default)]
pub(crate) struct RemoteSelection {
    pub(crate) selected: Option<u64>,
}

/// In-flight snapshot request task.
#[derive(Resource)]
pub struct RemoteSnapshotTask(pub Task<Result<serde_json::Value, anyhow::Error>>);

/// Timer controlling snapshot poll frequency.
#[derive(Resource)]
pub struct RemoteSnapshotPollTimer {
    pub timer: Timer,
}

impl Default for RemoteSnapshotPollTimer {
    fn default() -> Self {
        Self {
            timer: Timer::from_seconds(0.5, TimerMode::Repeating),
        }
    }
}

// ─────────────────────────── Helpers ───────────────────────────

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

fn display_name(entity: &RemoteEntity) -> String {
    extract_name(entity).unwrap_or_else(|| format!("Entity {:X}", entity.entity))
}

fn display_name_from_component(name: &RemoteEntityName, bits: u64) -> String {
    name.0
        .clone()
        .unwrap_or_else(|| format!("Entity {:X}", bits))
}

// ─────────────────────────── UI Layout ───────────────────────────

/// Build the remote debug workspace content.
pub fn remote_debug_workspace_content() -> impl Bundle {
    (
        RemoteHierarchyPanel,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            panel_header::panel_header("Remote Entities"),
            // Status row
            (
                RemoteEntityStatusText,
                Text::new("Not connected"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS),),
                    ..Default::default()
                },
            ),
            // Scrollable tree container
            (
                RemoteTreeContainer,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    overflow: Overflow::scroll_y(),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    ..Default::default()
                },
            )
        ],
    )
}

// ─────────────────────────── Reactive Name Watcher ───────────────────────────

/// Spawn a watcher entity that notifies us when `RemoteEntityName` is mutated.
pub fn setup_remote_name_watcher(mut commands: Commands) {
    commands
        .spawn(NotifyChanged::<RemoteEntityName>::default())
        .observe(on_remote_name_mutated);
}

fn on_remote_name_mutated(
    trigger: On<Mutation<RemoteEntityName>>,
    name_query: Query<(&RemoteEntityName, &RemoteEntityProxy)>,
    tree_row_index: Res<RemoteTreeRowIndex>,
    tree_nodes: Query<&Children, With<TreeNode>>,
    content_query: Query<&Children, With<TreeRowContent>>,
    mut label_query: Query<&mut Text, With<TreeRowLabel>>,
) {
    let proxy_entity = trigger.mutated;
    let Ok((name, proxy)) = name_query.get(proxy_entity) else {
        return;
    };
    let Some(&tree_entity) = tree_row_index.map.get(&proxy_entity) else {
        return;
    };
    let Ok(children) = tree_nodes.get(tree_entity) else {
        return;
    };
    let new_label = display_name_from_component(name, proxy.remote_bits);
    for child in children.iter() {
        let Ok(content_children) = content_query.get(child) else {
            continue;
        };
        for grandchild in content_children.iter() {
            if let Ok(mut text) = label_query.get_mut(grandchild) {
                text.0 = new_label;
                return;
            }
        }
    }
}

// ─────────────────────────── Polling Systems ───────────────────────────

/// Tick the poll timer and fire a snapshot request when ready.
pub fn snapshot_poll_timer(
    mut commands: Commands,
    manager: Res<ConnectionManager>,
    active: Res<crate::layout::ActiveDocument>,
    time: Res<Time>,
    mut poll_timer: ResMut<RemoteSnapshotPollTimer>,
    existing_task: Option<Res<RemoteSnapshotTask>>,
) {
    // Only poll when connected, on the Schedule Explorer tab, and no
    // task in flight.
    if !manager.is_connected() {
        return;
    }
    if active.kind != crate::layout::TabKind::ScheduleExplorer {
        return;
    }
    if existing_task.is_some() {
        return;
    }

    poll_timer.timer.tick(time.delta());
    if poll_timer.timer.just_finished() {
        let task = super::brp::brp_request(&manager.endpoint, "jackdaw/scene_snapshot", None);
        commands.insert_resource(RemoteSnapshotTask(task));
    }
}

/// Poll the in-flight snapshot task for completion.
pub fn poll_snapshot_task(mut commands: Commands, task: Option<ResMut<RemoteSnapshotTask>>) {
    let Some(mut task) = task else { return };

    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<RemoteSnapshotTask>();

    match result {
        Ok(value) => match serde_json::from_value::<Vec<RemoteEntity>>(value) {
            Ok(entities) => {
                commands.queue(move |world: &mut World| {
                    apply_scene_snapshot(world, entities);
                });
            }
            Err(e) => {
                warn!("Failed to parse scene snapshot: {e}");
            }
        },
        Err(e) => {
            warn!("Scene snapshot request failed: {e}");
        }
    }
}

// ─────────────────────────── Snapshot Application ───────────────────────────

/// Apply a new scene snapshot using diff & patch.
///
/// Existing tree rows are never despawned unless the entity truly disappears.
/// Name changes are applied by mutating `RemoteEntityName` on proxy entities,
/// which triggers the reactive watcher to update tree labels.
fn apply_scene_snapshot(world: &mut World, entities: Vec<RemoteEntity>) {
    let entity_count = entities.len();

    // Build lookup structures for the new snapshot
    let new_bits: HashSet<u64> = entities.iter().map(|e| e.entity).collect();
    let entity_map: HashMap<u64, &RemoteEntity> = entities.iter().map(|e| (e.entity, e)).collect();

    // Get current proxy set
    let current_bits: HashSet<u64> = {
        let index = world.resource::<RemoteProxyIndex>();
        index.map.keys().copied().collect()
    };

    // ── Removed: in current but not in new ──
    let removed: Vec<u64> = current_bits.difference(&new_bits).copied().collect();
    for bits in &removed {
        let proxy_entity = {
            let index = world.resource::<RemoteProxyIndex>();
            index.map.get(bits).copied()
        };
        if let Some(proxy) = proxy_entity {
            // Despawn tree row if it exists
            let tree_row = {
                let row_index = world.resource::<RemoteTreeRowIndex>();
                row_index.map.get(&proxy).copied()
            };
            if let Some(row) = tree_row {
                if let Ok(ec) = world.get_entity_mut(row) {
                    ec.despawn();
                }
                world
                    .resource_mut::<RemoteTreeRowIndex>()
                    .map
                    .remove(&proxy);
            }
            // Despawn proxy
            if let Ok(ec) = world.get_entity_mut(proxy) {
                ec.despawn();
            }
            world.resource_mut::<RemoteProxyIndex>().map.remove(bits);
        }
    }

    // Existing: in both, update RemoteEntityName if changed.
    let existing: Vec<u64> = current_bits.intersection(&new_bits).copied().collect();
    for bits in &existing {
        let Some(remote) = entity_map.get(bits) else {
            continue;
        };
        let new_name = extract_name(remote);
        let proxy_entity = {
            let index = world.resource::<RemoteProxyIndex>();
            index.map.get(bits).copied()
        };
        if let Some(proxy) = proxy_entity {
            // Only mutate if the name actually changed (avoids spurious Mutation events)
            let current_name = world.get::<RemoteEntityName>(proxy).map(|n| n.0.clone());
            if current_name != Some(new_name.clone())
                && let Some(mut name_comp) = world.get_mut::<RemoteEntityName>(proxy)
            {
                name_comp.0 = new_name;
            }
        }
    }

    // ── Compute new root set and compare with current root tree rows ──
    let new_root_bits: Vec<u64> = {
        let mut roots: Vec<(u64, String)> = entities
            .iter()
            .filter(|e| match extract_parent(e) {
                Some(parent_bits) => !new_bits.contains(&parent_bits),
                None => true,
            })
            .map(|e| (e.entity, display_name(e)))
            .collect();
        roots.sort_by(|a, b| a.1.cmp(&b.1));
        roots.into_iter().map(|(bits, _)| bits).collect()
    };

    // Get current root tree row bits (children of RemoteTreeContainer)
    let container = world
        .query_filtered::<Entity, With<RemoteTreeContainer>>()
        .iter(world)
        .next();

    let current_root_bits: Vec<u64> = if let Some(container) = container {
        world
            .get::<Children>(container)
            .map(|c| {
                c.iter()
                    .filter_map(|child| {
                        world
                            .get::<TreeNode>(child)
                            .and_then(|tn| world.get::<RemoteEntityProxy>(tn.0))
                            .map(|p| p.remote_bits)
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let roots_changed = current_root_bits != new_root_bits;

    if roots_changed {
        // Teardown existing root tree rows (but keep non-root proxies alive)
        if let Some(container) = container {
            let children: Vec<Entity> = world
                .get::<Children>(container)
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            for child in children {
                // Remove tree row index entries for this tree row and its descendants
                remove_tree_row_from_index(world, child);
                if let Ok(ec) = world.get_entity_mut(child) {
                    ec.despawn();
                }
            }
        }

        // Spawn root tree rows for all new roots
        let Some(container) = container else {
            // Update cache and status even if no container
            world.insert_resource(RemoteSceneCache { entities });
            update_status_text(world, entity_count);
            return;
        };
        let icon_font = world
            .get_resource::<jackdaw_feathers::icons::IconFont>()
            .map(|f| f.0.clone());
        let Some(icon_font) = icon_font else {
            world.insert_resource(RemoteSceneCache { entities });
            update_status_text(world, entity_count);
            return;
        };

        // Build children_of map to know which roots have children
        let children_of: HashSet<u64> = entities
            .iter()
            .filter_map(|e| extract_parent(e).filter(|p| new_bits.contains(p)))
            .collect();

        for &root_bits in &new_root_bits {
            let Some(remote) = entity_map.get(&root_bits) else {
                continue;
            };
            let has_children = children_of.contains(&root_bits);
            spawn_remote_tree_row(world, remote, has_children, container, &icon_font);
        }
    }

    // Added: in new but not in current. Spawn proxy (tree row only if root and roots not just rebuilt).
    let added: Vec<u64> = new_bits.difference(&current_bits).copied().collect();
    if !added.is_empty() {
        let icon_font = world
            .get_resource::<jackdaw_feathers::icons::IconFont>()
            .map(|f| f.0.clone());

        // Build children_of for has_children check
        let children_of: HashSet<u64> = entities
            .iter()
            .filter_map(|e| extract_parent(e).filter(|p| new_bits.contains(p)))
            .collect();

        for bits in &added {
            let Some(remote) = entity_map.get(bits) else {
                continue;
            };

            let is_root = match extract_parent(remote) {
                Some(parent_bits) => !new_bits.contains(&parent_bits),
                None => true,
            };

            if is_root && !roots_changed {
                // Root was added but we didn't rebuild roots above. Spawn its tree row.
                if let (Some(container), Some(icon_font)) = (container, &icon_font) {
                    let has_children = children_of.contains(bits);
                    spawn_remote_tree_row(world, remote, has_children, container, icon_font);
                }
            } else if !is_root {
                // Non-root added: just spawn the proxy, lazy expansion handles the tree row
                let name = extract_name(remote);
                let proxy = world
                    .spawn((
                        RemoteEntityProxy {
                            remote_bits: remote.entity,
                        },
                        RemoteEntityName(name),
                    ))
                    .id();
                world
                    .resource_mut::<RemoteProxyIndex>()
                    .map
                    .insert(remote.entity, proxy);
            }
            // If is_root && roots_changed, it was already spawned in the roots rebuild above
        }
    }

    // ── Handle expanded children whose child set changed ──
    update_expanded_children(world, &entities, &new_bits);

    // Update cache and status
    world.insert_resource(RemoteSceneCache { entities });
    update_status_text(world, entity_count);
}

/// For expanded+populated tree nodes, check if the child set changed and reset if so.
fn update_expanded_children(world: &mut World, entities: &[RemoteEntity], new_bits: &HashSet<u64>) {
    // Collect expanded+populated remote tree rows
    let expanded_rows: Vec<(Entity, u64)> = {
        let mut results = Vec::new();
        let mut query =
            world.query::<(Entity, &TreeNodeExpanded, &TreeChildrenPopulated, &TreeNode)>();
        for (entity, expanded, populated, tree_node) in query.iter(world) {
            if !expanded.0 || !populated.0 {
                continue;
            }
            if let Some(proxy) = world.get::<RemoteEntityProxy>(tree_node.0) {
                results.push((entity, proxy.remote_bits));
            }
        }
        results
    };

    for (tree_row_entity, parent_bits) in expanded_rows {
        let new_child_bits: HashSet<u64> = entities
            .iter()
            .filter(|e| extract_parent(e) == Some(parent_bits) && new_bits.contains(&e.entity))
            .map(|e| e.entity)
            .collect();

        let tree_row_children_containers: Vec<Entity> = world
            .get::<Children>(tree_row_entity)
            .map(|c| c.iter().collect())
            .unwrap_or_default();

        for container in tree_row_children_containers {
            if world.get::<TreeRowChildren>(container).is_none() {
                continue;
            }

            let existing_child_bits: HashSet<u64> = world
                .get::<Children>(container)
                .map(|c| {
                    c.iter()
                        .filter_map(|child_tree_row| {
                            world
                                .get::<TreeNode>(child_tree_row)
                                .and_then(|tn| world.get::<RemoteEntityProxy>(tn.0))
                                .map(|p| p.remote_bits)
                        })
                        .collect()
                })
                .unwrap_or_default();

            if existing_child_bits != new_child_bits {
                // Children changed, despawn old child tree rows and reset.
                let old_children: Vec<Entity> = world
                    .get::<Children>(container)
                    .map(|c| c.iter().collect())
                    .unwrap_or_default();
                for old_child in old_children {
                    remove_tree_row_from_index(world, old_child);
                    if let Ok(ec) = world.get_entity_mut(old_child) {
                        ec.despawn();
                    }
                }
                if let Some(mut pop) = world.get_mut::<TreeChildrenPopulated>(tree_row_entity) {
                    pop.0 = false;
                }
                if let Some(mut exp) = world.get_mut::<TreeNodeExpanded>(tree_row_entity) {
                    exp.0 = false;
                }
            }
        }
    }
}

/// Remove a tree row (and its descendants) from `RemoteTreeRowIndex`.
fn remove_tree_row_from_index(world: &mut World, tree_row: Entity) {
    // Remove mapping for this tree row's proxy
    if let Some(tn) = world.get::<TreeNode>(tree_row) {
        let proxy = tn.0;
        world
            .resource_mut::<RemoteTreeRowIndex>()
            .map
            .remove(&proxy);
    }
}

fn update_status_text(world: &mut World, entity_count: usize) {
    let status_entities: Vec<Entity> = world
        .query_filtered::<Entity, With<RemoteEntityStatusText>>()
        .iter(world)
        .collect();
    for status_entity in status_entities {
        if let Some(mut text) = world.get_mut::<Text>(status_entity) {
            let new_text = format!("{entity_count} entities");
            if text.0 != new_text {
                text.0 = new_text;
            }
        }
    }
}

/// Spawn a single proxy entity + tree row for a remote entity.
fn spawn_remote_tree_row(
    world: &mut World,
    remote_entity: &RemoteEntity,
    has_children: bool,
    parent_container: Entity,
    icon_font: &Handle<Font>,
) -> Entity {
    let name = extract_name(remote_entity);
    let label = name
        .clone()
        .unwrap_or_else(|| format!("Entity {:X}", remote_entity.entity));
    let style = TreeRowStyle {
        icon_font: icon_font.clone(),
    };

    // Spawn a local proxy entity for the TreeNode source
    let proxy = world
        .spawn((
            RemoteEntityProxy {
                remote_bits: remote_entity.entity,
            },
            RemoteEntityName(name),
        ))
        .id();

    let tree_row_entity = world
        .spawn((
            tree_row(
                &label,
                has_children,
                false,
                proxy,
                EntityCategory::Entity,
                &style,
            ),
            ChildOf(parent_container),
        ))
        .id();

    world
        .resource_mut::<RemoteProxyIndex>()
        .map
        .insert(remote_entity.entity, proxy);

    world
        .resource_mut::<RemoteTreeRowIndex>()
        .map
        .insert(proxy, tree_row_entity);

    tree_row_entity
}

// ─────────────────────────── Lazy Child Expansion ───────────────────────────

/// When a remote tree node is expanded, populate children from the cache.
pub fn on_remote_tree_node_expanded(
    trigger: On<Mutation<TreeNodeExpanded>>,
    mut commands: Commands,
    tree_query: Query<(
        &TreeNodeExpanded,
        &TreeChildrenPopulated,
        &TreeNode,
        &Children,
    )>,
    tree_row_children_marker: Query<Entity, With<TreeRowChildren>>,
    proxies: Query<&RemoteEntityProxy>,
) {
    let entity = trigger.event_target();
    let Ok((expanded, populated, tree_node, children)) = tree_query.get(entity) else {
        return;
    };

    if !expanded.0 || populated.0 {
        return;
    }

    let source = tree_node.0;

    // Only handle remote proxy entities
    let Ok(proxy) = proxies.get(source) else {
        return;
    };

    let parent_bits = proxy.remote_bits;

    let Some(container) = children
        .iter()
        .find(|c| tree_row_children_marker.contains(*c))
    else {
        return;
    };

    let tree_row_entity = entity;

    commands.queue(move |world: &mut World| {
        // Guard against duplicate events
        if let Some(pop) = world.get::<TreeChildrenPopulated>(tree_row_entity)
            && pop.0
        {
            return;
        }

        if let Some(mut pop) = world.get_mut::<TreeChildrenPopulated>(tree_row_entity) {
            pop.0 = true;
        }

        // Clone data from cache to release the borrow before spawning
        let (child_entities, has_grandchildren_map) = {
            let cache = world.resource::<RemoteSceneCache>();
            let entity_bits_set: std::collections::HashSet<u64> =
                cache.entities.iter().map(|e| e.entity).collect();

            let mut children: Vec<RemoteEntity> = cache
                .entities
                .iter()
                .filter(|e| extract_parent(e) == Some(parent_bits))
                .cloned()
                .collect();

            children.sort_by_key(display_name);

            let gc_map: HashMap<u64, bool> = children
                .iter()
                .map(|child| {
                    let has_gc = cache.entities.iter().any(|e| {
                        extract_parent(e) == Some(child.entity)
                            && entity_bits_set.contains(&child.entity)
                    });
                    (child.entity, has_gc)
                })
                .collect();

            (children, gc_map)
        };

        let icon_font = world
            .get_resource::<jackdaw_feathers::icons::IconFont>()
            .map(|f| f.0.clone());
        let Some(icon_font) = icon_font else { return };

        for child_remote in &child_entities {
            let has_grandchildren = has_grandchildren_map
                .get(&child_remote.entity)
                .copied()
                .unwrap_or(false);
            spawn_remote_tree_row(
                world,
                child_remote,
                has_grandchildren,
                container,
                &icon_font,
            );
        }
    });
}

// ─────────────────────────── Remote Selection ───────────────────────────

/// Handle tree row click for remote entity proxies.
pub(crate) fn on_remote_tree_row_clicked(
    event: On<TreeRowClicked>,
    mut commands: Commands,
    proxies: Query<&RemoteEntityProxy>,
    mut selection: ResMut<RemoteSelection>,
    tree_row_contents: Query<Entity, With<TreeRowContent>>,
    mut bg_query: Query<&mut BackgroundColor>,
    all_proxy_tree_nodes: Query<(&TreeNode, &Children)>,
) {
    let Ok(proxy) = proxies.get(event.source_entity) else {
        return;
    };

    let bits = proxy.remote_bits;

    // Clear all remote selections visually
    for (tree_node, children) in &all_proxy_tree_nodes {
        if proxies.get(tree_node.0).is_ok() {
            for child in children.iter() {
                if tree_row_contents.contains(child) {
                    if let Ok(mut bg) = bg_query.get_mut(child) {
                        bg.0 = jackdaw_feathers::tree_view::ROW_BG;
                    }
                    if let Ok(mut ec) = commands.get_entity(child) {
                        ec.remove::<TreeRowSelected>();
                    }
                }
            }
        }
    }

    // Toggle selection
    if selection.selected == Some(bits) {
        selection.selected = None;
    } else {
        selection.selected = Some(bits);

        // Highlight the clicked row
        let content_entity = event.entity;
        if let Ok(mut bg) = bg_query.get_mut(content_entity) {
            bg.0 = tokens::SELECTED_BG;
        }
        if let Ok(mut ec) = commands.get_entity(content_entity) {
            ec.insert(TreeRowSelected);
        }
    }
}

// ─────────────────────────── Cleanup ───────────────────────────

/// When connection is lost, clean up all remote proxy state.
pub(crate) fn cleanup_remote_proxies(
    mut commands: Commands,
    manager: Res<ConnectionManager>,
    mut active: ResMut<crate::layout::ActiveDocument>,
    proxies: Query<Entity, With<RemoteEntityProxy>>,
    mut proxy_index: ResMut<RemoteProxyIndex>,
    mut tree_row_index: ResMut<RemoteTreeRowIndex>,
    mut cache: ResMut<RemoteSceneCache>,
    mut selection: ResMut<RemoteSelection>,
    status_texts: Query<Entity, With<RemoteEntityStatusText>>,
) {
    if !manager.is_changed() {
        return;
    }

    if manager.is_connected() {
        return;
    }

    // Despawn all proxies
    for proxy in &proxies {
        if let Ok(mut ec) = commands.get_entity(proxy) {
            ec.despawn();
        }
    }

    // Clear tree rows inside the remote tree container
    // (they reference proxies that were just despawned, so they'll be cleaned up
    //  by Bevy's despawn propagation via `TreeNode` relationship)

    proxy_index.map.clear();
    tree_row_index.map.clear();
    cache.entities.clear();
    selection.selected = None;

    // Reset status text
    for entity in &status_texts {
        if let Some(mut text) = commands.get_entity(entity).ok().and(None::<Mut<Text>>) {
            text.0 = "Not connected".to_string();
        }
    }

    // Update status text via command queue since we can't get mut Text through commands
    commands.queue(move |world: &mut World| {
        let status_entities: Vec<Entity> = world
            .query_filtered::<Entity, With<RemoteEntityStatusText>>()
            .iter(world)
            .collect();
        for entity in status_entities {
            if let Some(mut text) = world.get_mut::<Text>(entity) {
                text.0 = "Not connected".to_string();
            }
        }
    });

    // Switch back to the Scene document if the user was viewing
    // Schedule Explorer when the connection dropped.
    if active.kind == crate::layout::TabKind::ScheduleExplorer {
        active.kind = crate::layout::TabKind::Scene;
    }

    commands.remove_resource::<RemoteSnapshotTask>();
}
