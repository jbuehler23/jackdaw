//! Materialize a [`DockTree`] into UI entities.
//!
//! The tree is the source of truth for layout. Editor code spawns one
//! `DockTreeHost` entity (typically the editor's main content area),
//! and the reconciler walks the tree from its single root and shapes
//! the entity sub-tree to match: leaves become `DockArea`s with tab
//! bar + content, splits become flex containers wrapping two child
//! anchor-style entities plus a `PanelHandle` between them.
//!
//! Drag/move/resize operations mutate the tree only; the reconciler
//! rebuilds the affected entity sub-tree on the next frame.

use bevy::prelude::*;
use jackdaw_feathers::tokens;

use crate::area::{ActiveDockWindow, DockArea, DockAreaStyle, DockTab, DockTabContent, DockWindow};
use crate::registry::WindowRegistry;
use crate::sidebar::{self, DockSidebarIcon};
use crate::split::{Panel, PanelGroup, PanelHandle};
use crate::tabs;
use crate::tree::{DockLeaf, DockNode, DockSplit, DockTree, NodeId, SplitAxis};

/// Marker for the single editor entity the reconciler renders the dock
/// tree underneath. Spawn one of these inside the editor's content
/// area; the reconciler will fill it with split / leaf entities matching
/// the current [`DockTree`].
///
/// Earlier versions had per-anchor `AnchorHost` entities. The flat
/// single-tree model has just one host.
#[derive(Component, Clone, Debug)]
pub struct DockTreeHost {
    /// Default style applied to a freshly-seeded root leaf if the tree
    /// is empty when the host is first encountered. Most callers will
    /// pre-build a default tree separately and never trigger this path.
    pub default_style: DockAreaStyle,
}

impl Default for DockTreeHost {
    fn default() -> Self {
        Self {
            default_style: DockAreaStyle::TabBar,
        }
    }
}

/// Binds an entity to a tree node. Present on both leaf-style entities
/// (`DockArea`) and split wrapper entities (`PanelGroup`).
#[derive(Component, Copy, Clone, Debug)]
pub struct NodeBinding(pub NodeId);

pub struct ReconcilePlugin;

impl Plugin for ReconcilePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DockTree>().add_systems(
            Update,
            (seed_root_from_host, reconcile_tree, sync_leaf_visuals).chain(),
        );
    }
}

/// Public entry point for the editor's `OnEnter(Editor)` chain so the
/// root entity exists before saved-layout application runs.
pub fn run_initial_reconcile(world: &mut World) {
    seed_root_from_host(world);
    reconcile_tree(world);
}

/// Public for editor flows that want to seed before applying defaults
/// then reconcile in a single materialization pass.
pub fn seed_root(world: &mut World) {
    seed_root_from_host(world);
}

/// Public for editor flows that build the final tree shape (saved or
/// defaults) up front and then materialize once.
pub fn reconcile(world: &mut World) {
    reconcile_tree(world);
}

/// If the dock tree has no root and a `DockTreeHost` exists, seed a
/// minimal root leaf from the host's default style. This is a safety
/// net for callers that forgot to build a default tree; the editor's
/// `init_layout` builds the canonical multi-region tree directly and
/// never triggers this path.
fn seed_root_from_host(world: &mut World) {
    if world.resource::<DockTree>().root.is_some() {
        return;
    }
    let default_style = {
        let mut q = world.query::<&DockTreeHost>();
        q.iter(world).next().map(|h| h.default_style.clone())
    };
    let Some(default_style) = default_style else {
        return;
    };
    let leaf = DockLeaf::new("root", default_style);
    world.resource_mut::<DockTree>().set_root_leaf(leaf);
}

fn reconcile_tree(world: &mut World) {
    if !world.is_resource_changed::<DockTree>() {
        return;
    }

    let Some(root) = world.resource::<DockTree>().root else {
        return;
    };
    let Some(host) = find_dock_tree_host(world) else {
        return;
    };
    reconcile_at(world, host, root);
}

fn find_dock_tree_host(world: &mut World) -> Option<Entity> {
    let mut q = world.query::<(Entity, &DockTreeHost)>();
    q.iter(world).next().map(|(e, _)| e)
}

fn reconcile_at(world: &mut World, entity: Entity, node_id: NodeId) {
    let node = world.resource::<DockTree>().get(node_id).cloned();
    let Some(node) = node else {
        return;
    };
    match node {
        DockNode::Leaf(leaf) => reconcile_leaf(world, entity, node_id, &leaf),
        DockNode::Split(split) => reconcile_split(world, entity, node_id, &split),
    }
}

fn reconcile_leaf(world: &mut World, entity: Entity, node_id: NodeId, leaf: &DockLeaf) {
    let current_binding = world.entity(entity).get::<NodeBinding>().map(|b| b.0);
    let was_split = world.entity(entity).contains::<PanelGroup>();
    let current_tabs = collect_content_tab_ids(world, entity);
    let leaf_tabs: Vec<crate::tree::TabId> = leaf.windows.iter().map(|t| t.id).collect();

    let needs_rebuild = was_split || current_binding != Some(node_id) || current_tabs != leaf_tabs;

    if needs_rebuild {
        despawn_children(world, entity);
        world.entity_mut(entity).remove::<PanelGroup>();

        let direction = match leaf.style {
            DockAreaStyle::IconSidebar => FlexDirection::Row,
            DockAreaStyle::TabBar | DockAreaStyle::Headless => FlexDirection::Column,
        };
        if let Some(mut node) = world.entity_mut(entity).get_mut::<Node>() {
            node.flex_direction = direction;
        }

        if let Some(mut area) = world.entity_mut(entity).get_mut::<DockArea>() {
            area.id = leaf.area_id.clone();
            area.style = leaf.style.clone();
        } else {
            world.entity_mut(entity).insert(DockArea {
                id: leaf.area_id.clone(),
                style: leaf.style.clone(),
            });
        }

        spawn_leaf_ui(world, entity, leaf);
    }

    world
        .entity_mut(entity)
        .insert(ActiveDockWindow(leaf.active));
    world.entity_mut(entity).insert(NodeBinding(node_id));

    // Auto-collapse: when a non-persistent leaf has no windows, hide
    // the host entity and its adjacent handle so siblings can reclaim
    // the space. Persistent leaves (built-in dock regions like the
    // viewport center, right sidebar, etc.) stay visible even when
    // empty so they remain drop targets and so non-window content
    // mounted inside them (the SceneViewport in Phase 1) keeps
    // rendering.
    let visible = !leaf.windows.is_empty() || leaf.is_persistent();
    set_host_visible(world, entity, visible);
}

fn reconcile_split(world: &mut World, entity: Entity, node_id: NodeId, split: &DockSplit) {
    let current_binding = world.entity(entity).get::<NodeBinding>().map(|b| b.0);

    let mut children = collect_split_children(world, entity);
    let needs_rebuild = current_binding != Some(node_id) || children.is_none();

    if needs_rebuild {
        despawn_children(world, entity);
        world.entity_mut(entity).remove::<ActiveDockWindow>();
        world.entity_mut(entity).remove::<DockArea>();

        if let Some(mut node) = world.entity_mut(entity).get_mut::<Node>() {
            node.flex_direction = match split.axis {
                SplitAxis::Horizontal => FlexDirection::Row,
                SplitAxis::Vertical => FlexDirection::Column,
            };
        }
        if !world.entity(entity).contains::<PanelGroup>() {
            world
                .entity_mut(entity)
                .insert(PanelGroup { min_ratio: 0.05 });
        }

        let child_node = || Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            ..default()
        };

        let child_a = world
            .spawn((
                Panel {
                    ratio: split.fraction,
                },
                child_node(),
                BackgroundColor(tokens::PANEL_BG),
                ChildOf(entity),
            ))
            .id();
        let handle = world
            .spawn((
                PanelHandle,
                Node {
                    min_width: Val::Px(3.0),
                    min_height: Val::Px(3.0),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                NodeBinding(node_id),
                ChildOf(entity),
            ))
            .id();
        let child_b = world
            .spawn((
                Panel {
                    ratio: 1.0 - split.fraction,
                },
                child_node(),
                BackgroundColor(tokens::PANEL_BG),
                ChildOf(entity),
            ))
            .id();
        children = Some((child_a, handle, child_b));
    }

    let (child_a, _handle, child_b) = children.expect("children exist after rebuild");

    if let Some(mut p) = world.entity_mut(child_a).get_mut::<Panel>()
        && (p.ratio - split.fraction).abs() > f32::EPSILON
    {
        p.ratio = split.fraction;
    }
    if let Some(mut p) = world.entity_mut(child_b).get_mut::<Panel>() {
        let other = 1.0 - split.fraction;
        if (p.ratio - other).abs() > f32::EPSILON {
            p.ratio = other;
        }
    }

    reconcile_at(world, child_a, split.a);
    reconcile_at(world, child_b, split.b);

    world.entity_mut(entity).insert(NodeBinding(node_id));

    // A split always has visible leaf children, so the container must
    // be visible. If the host was collapsed (Display::None + zero
    // geometry) as an empty leaf just before the transition, restore
    // it here so the freshly-reconciled children aren't hidden inside
    // a zero-sized parent.
    set_host_visible(world, entity, true);
}

fn spawn_leaf_ui(world: &mut World, entity: Entity, leaf: &DockLeaf) {
    // Iterate `leaf.windows` so two tabs of the same window kind
    // produce two entries with distinct `TabId`s.
    let snapshot: Vec<(
        crate::tree::TabId,
        String,
        String,
        Option<String>,
        crate::DockWindowBuildFn,
    )> = {
        let registry = world.resource::<WindowRegistry>();
        leaf.windows
            .iter()
            .filter_map(|tab| {
                let desc = registry.get(&tab.window_id)?;
                Some((
                    tab.id,
                    desc.id.clone(),
                    desc.name.clone(),
                    desc.icon.clone(),
                    desc.build.clone(),
                ))
            })
            .collect()
    };

    match leaf.style {
        DockAreaStyle::TabBar => {
            let tabs_data: Vec<(crate::tree::TabId, String, String)> = snapshot
                .iter()
                .map(|(tab_id, id, name, _, _)| (*tab_id, id.clone(), name.clone()))
                .collect();
            tabs::spawn_tab_bar_world(world, entity, &tabs_data);
        }
        DockAreaStyle::IconSidebar => {
            let items: Vec<(crate::tree::TabId, String, String, Option<String>)> = snapshot
                .iter()
                .map(|(tab_id, id, name, icon, _)| {
                    (*tab_id, id.clone(), name.clone(), icon.clone())
                })
                .collect();
            sidebar::spawn_icon_sidebar_world(world, entity, &items);
        }
        DockAreaStyle::Headless => {}
    }

    for (tab_id, window_id, _name, _icon, build) in &snapshot {
        let is_active = leaf.active == Some(*tab_id);
        let content_entity = world
            .spawn((
                DockWindow {
                    descriptor_id: window_id.clone(),
                    tab_id: *tab_id,
                },
                DockTabContent {
                    window_id: window_id.clone(),
                    tab_id: *tab_id,
                },
                Node {
                    flex_grow: 1.0,
                    width: Val::Percent(100.0),
                    min_height: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::clip(),
                    display: if is_active {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    ..default()
                },
                ChildOf(entity),
            ))
            .id();
        (build)(&mut ChildSpawner::new(world, content_entity));
    }
}

fn collect_content_tab_ids(world: &mut World, entity: Entity) -> Vec<crate::tree::TabId> {
    let children: Vec<Entity> = world
        .entity(entity)
        .get::<Children>()
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for child in children {
        if let Some(c) = world.entity(child).get::<DockTabContent>() {
            out.push(c.tab_id);
        }
    }
    out
}

/// If `entity` currently looks like a split host (`PanelGroup` with three
/// children: panel, handle, panel), return them in order.
fn collect_split_children(world: &mut World, entity: Entity) -> Option<(Entity, Entity, Entity)> {
    let children: Vec<Entity> = world
        .entity(entity)
        .get::<Children>()
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    if children.len() != 3 {
        return None;
    }
    let a = children[0];
    let h = children[1];
    let b = children[2];
    if !world.entity(h).contains::<PanelHandle>() {
        return None;
    }
    if !world.entity(a).contains::<Panel>() || !world.entity(b).contains::<Panel>() {
        return None;
    }
    Some((a, h, b))
}

/// Show or hide a host entity and its adjacent `PanelHandle` sibling so
/// an empty anchor doesn't leave a stub panel + dangling resize handle.
fn set_host_visible(world: &mut World, entity: Entity, visible: bool) {
    let target = if visible {
        Display::Flex
    } else {
        Display::None
    };

    // Find the adjacent PanelHandle sibling (index ±1 in the parent's
    // children) so we can hide/show it alongside the host.
    let adjacent_handle = {
        let parent = world.entity(entity).get::<ChildOf>().map(ChildOf::parent);
        parent.and_then(|parent| {
            let siblings: Vec<Entity> = world
                .entity(parent)
                .get::<Children>()
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            let idx = siblings.iter().position(|&e| e == entity)?;
            [idx.checked_sub(1), Some(idx + 1)]
                .into_iter()
                .flatten()
                .filter_map(|i| siblings.get(i).copied())
                .find(|&e| world.entity(e).contains::<PanelHandle>())
        })
    };

    let mut any_changed = false;

    // Host: toggle Display and drive geometry only when the state
    // actually transitions. Unconditionally setting width/height every
    // reconcile pass would stomp on the ratio-based percentages that
    // `recalculate_group` has already written for an already-visible
    // panel, producing a panel that fills 100% of its Row parent.
    if let Some(mut node) = world.entity_mut(entity).get_mut::<Node>() {
        if node.display != target {
            node.display = target;
            any_changed = true;
        }
        let zero = Val::Px(0.0);
        if !visible {
            // Zero the host on hide so taffy can't reserve a layout
            // floor. Skip if already zeroed.
            if node.width != zero || node.height != zero {
                node.width = zero;
                node.height = zero;
                node.min_width = zero;
                node.min_height = zero;
                any_changed = true;
            }
        } else if node.width == zero {
            // Coming back from hide: restore to 100% so
            // `recalculate_group` can overwrite the flex-axis and the
            // cross-axis fills. Only do this once per show so an
            // already-recalculated width isn't stomped.
            node.width = Val::Percent(100.0);
            node.height = Val::Percent(100.0);
            any_changed = true;
        }
    }

    // Handle: ONLY toggle Display. Don't touch width/height. A
    // `PanelHandle`'s natural size is a 3px stripe along the flex
    // axis; forcing 100% would make it fill the parent.
    if let Some(handle) = adjacent_handle
        && let Some(mut node) = world.entity_mut(handle).get_mut::<Node>()
        && node.display != target
    {
        node.display = target;
        any_changed = true;
    }

    // Flag the host's Panel component as changed so `recalculate_group`
    // redistributes sibling widths this frame. The host may carry one
    // of two Panel types: `jackdaw_widgets::split_panel::Panel` on
    // editor-spawned outer hosts, or `jackdaw_panels::split::Panel` on
    // reconciler-spawned split children. Only one is present; try both.
    if any_changed {
        if let Some(mut panel) = world
            .entity_mut(entity)
            .get_mut::<jackdaw_widgets::split_panel::Panel>()
        {
            panel.set_changed();
        }
        if let Some(mut panel) = world.entity_mut(entity).get_mut::<Panel>() {
            panel.set_changed();
        }
    }
}

fn despawn_children(world: &mut World, entity: Entity) {
    let children: Vec<Entity> = world
        .entity(entity)
        .get::<Children>()
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for child in children {
        if let Ok(em) = world.get_entity_mut(child) {
            em.despawn();
        }
    }
}

/// On every Update, sync visual state (active tab bg/border/text colors,
/// sidebar icon highlight, content `Display`) for leaf entities.
fn sync_leaf_visuals(
    leaves: Query<(Entity, &NodeBinding, &DockArea), Without<PanelGroup>>,
    tree: Res<DockTree>,
    tabs: Query<(Entity, &DockTab, &ChildOf)>,
    icons: Query<(Entity, &DockSidebarIcon, &ChildOf)>,
    contents: Query<(Entity, &DockTabContent, &ChildOf)>,
    parent_query: Query<&ChildOf>,
    children_query: Query<&Children>,
    mut nodes: Query<&mut Node>,
    mut bgs: Query<&mut BackgroundColor>,
    mut borders: Query<&mut BorderColor>,
    mut text_colors: Query<&mut TextColor>,
) {
    if !tree.is_changed() {
        return;
    }

    use std::collections::HashMap;
    let mut tab_to_area: HashMap<Entity, Entity> = HashMap::new();
    for (tab_entity, _, child_of) in &tabs {
        let tab_row = child_of.parent();
        let Ok(row_parent) = parent_query.get(tab_row) else {
            continue;
        };
        let tab_bar = row_parent.parent();
        let Ok(bar_parent) = parent_query.get(tab_bar) else {
            continue;
        };
        tab_to_area.insert(tab_entity, bar_parent.parent());
    }

    let mut icon_to_area: HashMap<Entity, Entity> = HashMap::new();
    for (icon_entity, _, child_of) in &icons {
        let icon_group = child_of.parent();
        let Ok(group_parent) = parent_query.get(icon_group) else {
            continue;
        };
        let sidebar_entity = group_parent.parent();
        let Ok(sidebar_parent) = parent_query.get(sidebar_entity) else {
            continue;
        };
        icon_to_area.insert(icon_entity, sidebar_parent.parent());
    }

    for (area_entity, binding, _) in &leaves {
        let Some(leaf) = tree.get(binding.0).and_then(|n| n.as_leaf()) else {
            continue;
        };

        for (tab_entity, tab, _) in &tabs {
            if tab_to_area.get(&tab_entity) != Some(&area_entity) {
                continue;
            }
            let is_active = leaf.active == Some(tab.tab_id);
            if let Ok(mut bg) = bgs.get_mut(tab_entity) {
                bg.0 = if is_active {
                    tokens::TAB_ACTIVE_BG
                } else {
                    Color::NONE
                };
            }
            if let Ok(mut bc) = borders.get_mut(tab_entity) {
                *bc = BorderColor::all(if is_active {
                    tokens::TAB_ACTIVE_BORDER
                } else {
                    Color::NONE
                });
            }
            if let Ok(mut node) = nodes.get_mut(tab_entity) {
                node.border.top = if is_active { Val::Px(2.0) } else { Val::ZERO };
            }
            if let Ok(tab_children) = children_query.get(tab_entity) {
                for child in tab_children.iter() {
                    if let Ok(mut tc) = text_colors.get_mut(child) {
                        tc.0 = if is_active {
                            tokens::TEXT_PRIMARY
                        } else {
                            tokens::TAB_INACTIVE_TEXT
                        };
                    }
                }
            }
        }

        for (icon_entity, icon, _) in &icons {
            if icon_to_area.get(&icon_entity) != Some(&area_entity) {
                continue;
            }
            let is_active = leaf.active == Some(icon.tab_id);
            if let Ok(mut bc) = borders.get_mut(icon_entity) {
                *bc = BorderColor::all(if is_active {
                    tokens::ACCENT_BLUE
                } else {
                    Color::NONE
                });
            }
            if let Ok(icon_children) = children_query.get(icon_entity) {
                for child in icon_children.iter() {
                    if let Ok(mut tc) = text_colors.get_mut(child) {
                        tc.0 = if is_active {
                            tokens::TEXT_PRIMARY
                        } else {
                            tokens::TAB_INACTIVE_TEXT
                        };
                    }
                }
            }
        }

        for (content_entity, content, child_of) in &contents {
            if child_of.parent() != area_entity {
                continue;
            }
            let should_show = leaf.active == Some(content.tab_id);
            let target = if should_show {
                Display::Flex
            } else {
                Display::None
            };
            if let Ok(mut node) = nodes.get_mut(content_entity)
                && node.display != target
            {
                node.display = target;
            }
        }
    }
}

// Backwards-compat alias: old code used `LeafBinding`.
pub type LeafBinding = NodeBinding;
