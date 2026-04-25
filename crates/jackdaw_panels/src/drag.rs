use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use jackdaw_feathers::tokens;

use crate::area::{DockArea, DockTab};
use crate::reconcile::NodeBinding;
use crate::sidebar::DockSidebarIcon;
use crate::tabs::{DockTabGrip, DockTabRow};
use crate::tree::{DockTree, Edge as TreeEdge};

const DRAG_THRESHOLD: f32 = 5.0;

#[derive(Resource, Default, Debug)]
pub enum DockDragState {
    #[default]
    Idle,
    PendingDrag {
        source_tab: Entity,
        window_id: String,
        window_name: String,
        start_pos: Vec2,
    },
    Dragging {
        source_tab: Entity,
        window_id: String,
        window_name: String,
        source_area: Entity,
        ghost_entity: Entity,
        cursor_pos: Vec2,
        drop_target: Option<DropTarget>,
        overlay_entity: Option<Entity>,
    },
}

#[derive(Clone, Debug)]
pub enum DropTarget {
    Panel(Entity),
    TabRow {
        bar: Entity,
        index: usize,
    },
    AreaEdge {
        area: Entity,
        edge: DropEdge,
    },
    /// Dropped on the editor viewport's edge. Routes to the anchor
    /// associated with that edge (`left/right_sidebar/bottom_dock`).
    ViewportEdge {
        anchor_id: String,
        edge: DropEdge,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropEdge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Component)]
pub struct DragGhost;

#[derive(Component)]
pub struct DropOverlay;

/// Marks the editor viewport entity as a drop target. The viewport is
/// not an `AnchorHost`; dropping on its edge re-populates one of the
/// side anchors (`left`, `right_sidebar`, `bottom_dock`) instead.
#[derive(Component)]
pub struct ViewportDropTarget;

pub struct DockDragPlugin;

impl Plugin for DockDragPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DockDragState>()
            .add_observer(on_tab_drag_start)
            .add_observer(on_sidebar_icon_drag_start)
            .add_observer(on_grip_drag_start)
            .add_observer(on_drag_move)
            .add_observer(on_drag_end)
            .add_systems(Update, cancel_drag_on_escape);
    }
}

fn logical_rect(computed: &ComputedNode, transform: &UiGlobalTransform) -> Rect {
    let inv = computed.inverse_scale_factor();
    let size = computed.size() * inv;
    let (_scale, _angle, center) = transform.to_scale_angle_translation();
    let center = center.trunc() * inv;
    Rect::from_center_size(center, size)
}

fn on_tab_drag_start(
    trigger: On<Pointer<DragStart>>,
    tabs: Query<&DockTab>,
    mut drag_state: ResMut<DockDragState>,
    registry: Res<crate::WindowRegistry>,
) {
    let entity = trigger.event_target();
    let Ok(tab) = tabs.get(entity) else { return };

    let display_name = registry
        .get(&tab.window_id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| tab.window_id.clone());

    *drag_state = DockDragState::PendingDrag {
        source_tab: entity,
        window_id: tab.window_id.clone(),
        window_name: display_name,
        start_pos: Vec2::new(
            trigger.event().pointer_location.position.x,
            trigger.event().pointer_location.position.y,
        ),
    };
}

fn on_sidebar_icon_drag_start(
    trigger: On<Pointer<DragStart>>,
    icons: Query<&DockSidebarIcon>,
    mut drag_state: ResMut<DockDragState>,
    registry: Res<crate::WindowRegistry>,
) {
    let entity = trigger.event_target();
    let Ok(icon) = icons.get(entity) else { return };

    let display_name = registry
        .get(&icon.window_id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| icon.window_id.clone());

    *drag_state = DockDragState::PendingDrag {
        source_tab: entity,
        window_id: icon.window_id.clone(),
        window_name: display_name,
        start_pos: Vec2::new(
            trigger.event().pointer_location.position.x,
            trigger.event().pointer_location.position.y,
        ),
    };
}

fn on_grip_drag_start(
    trigger: On<Pointer<DragStart>>,
    grips: Query<(), With<DockTabGrip>>,
    dock_areas: Query<&crate::ActiveDockWindow, With<DockArea>>,
    parent_query: Query<&ChildOf>,
    mut drag_state: ResMut<DockDragState>,
    registry: Res<crate::WindowRegistry>,
) {
    let entity = trigger.event_target();
    if grips.get(entity).is_err() {
        return;
    }

    let mut current = entity;
    let mut active_window_id = None;
    loop {
        if let Ok(active) = dock_areas.get(current) {
            active_window_id = active.0.clone();
            break;
        }
        let Ok(parent) = parent_query.get(current) else {
            break;
        };
        current = parent.parent();
    }

    let Some(window_id) = active_window_id else {
        return;
    };

    let window_name = registry
        .get(&window_id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| window_id.clone());

    *drag_state = DockDragState::PendingDrag {
        source_tab: entity,
        window_id,
        window_name,
        start_pos: Vec2::new(
            trigger.event().pointer_location.position.x,
            trigger.event().pointer_location.position.y,
        ),
    };
}

fn on_drag_move(
    mut trigger: On<Pointer<Drag>>,
    mut drag_state: ResMut<DockDragState>,
    mut commands: Commands,
    areas: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<DockArea>>,
    tab_rows: Query<
        (
            Entity,
            &ComputedNode,
            &Node,
            &UiGlobalTransform,
            &Children,
            &ChildOf,
        ),
        With<DockTabRow>,
    >,
    viewports: Query<(&ComputedNode, &UiGlobalTransform), With<ViewportDropTarget>>,
    node_query: Query<(&ComputedNode, &UiGlobalTransform)>,
    parent_query: Query<&ChildOf>,
) {
    let drag_event = trigger.event();
    let cursor = Vec2::new(
        drag_event.pointer_location.position.x,
        drag_event.pointer_location.position.y,
    );

    match &*drag_state {
        DockDragState::PendingDrag {
            source_tab,
            window_id,
            window_name,
            start_pos,
        } => {
            if cursor.distance(*start_pos) < DRAG_THRESHOLD {
                return;
            }

            let source_tab = *source_tab;
            let window_id = window_id.clone();
            let window_name = window_name.clone();

            let source_area = find_parent_area(source_tab, &parent_query, &areas);

            let ghost = commands
                .spawn((
                    DragGhost,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(cursor.x - 40.0),
                        top: Val::Px(cursor.y - 12.0),
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(tokens::MENU_BG),
                    BorderColor::all(tokens::ACCENT_BLUE),
                    GlobalZIndex(200),
                    children![(
                        Text::new(window_name.clone()),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    )],
                ))
                .id();

            *drag_state = DockDragState::Dragging {
                source_tab,
                window_id,
                window_name,
                source_area: source_area.unwrap_or(Entity::PLACEHOLDER),
                ghost_entity: ghost,
                cursor_pos: cursor,
                drop_target: None,
                overlay_entity: None,
            };

            trigger.propagate(false);
        }
        DockDragState::Dragging {
            ghost_entity,
            overlay_entity,
            ..
        } => {
            let ghost = *ghost_entity;
            let old_overlay = *overlay_entity;

            commands.entity(ghost).insert(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(cursor.x - 40.0),
                top: Val::Px(cursor.y - 12.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            });

            if let Some(old) = old_overlay {
                commands.entity(old).despawn();
            }

            let mut new_target = None;
            let mut new_overlay = None;

            for (tab_row_entity, computed, node, ui_transform, children, parent) in &tab_rows {
                let row_rect = logical_rect(computed, ui_transform);
                let parent_contains =
                    node_query
                        .get(parent.0)
                        .is_ok_and(|(parent_computed, parent_transform)| {
                            logical_rect(parent_computed, parent_transform).contains(cursor)
                        });
                if !row_rect.contains(cursor) && !parent_contains {
                    continue;
                }
                let mut closest_child: Option<(Vec2, Vec2, usize, f32)> = None;
                for (index, child) in children.iter().enumerate() {
                    let Ok((child_computed, _child_transform)) = node_query.get(child) else {
                        continue;
                    };
                    let (_scale, _angle, center) = ui_transform.to_scale_angle_translation();
                    let child_center = center.trunc() * computed.inverse_scale_factor();
                    let child_size = child_computed.size() * child_computed.inverse_scale_factor();
                    let distance = child_center.distance_squared(cursor);
                    if closest_child.is_none_or(|(_, _, _, closest_dist)| distance < closest_dist) {
                        closest_child = Some((child_center, child_size, index, distance));
                    }
                }
                let Some((child_center, child_size, mut index, _)) = closest_child else {
                    continue;
                };
                let (is_far_side, is_vertical) = is_far_side(cursor, child_center, node);
                if is_far_side {
                    index += 1;
                }

                new_target = Some(DropTarget::TabRow {
                    bar: tab_row_entity,
                    index,
                });

                let size_mult = if !is_vertical {
                    Vec2::new(0.5, 1.0)
                } else {
                    Vec2::new(1.0, 0.5)
                };

                let overlay_size = child_size * size_mult;

                let mut offset = if !is_vertical {
                    Vec2::new(child_size.x, overlay_size.y)
                } else {
                    Vec2::new(overlay_size.x, child_size.y)
                };

                offset *= -0.5;

                if is_far_side {
                    if !is_vertical {
                        offset.x = 0.0;
                    } else {
                        offset.y = 0.0;
                    }
                }
                let overlay_pos = child_center + offset;

                let overlay = commands
                    .spawn((
                        DropOverlay,
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(overlay_pos.x),
                            top: Val::Px(overlay_pos.y),
                            width: Val::Px(overlay_size.x),
                            height: Val::Px(overlay_size.y),
                            border: UiRect::all(Val::Px(2.0)),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            ..Default::default()
                        },
                        BackgroundColor(tokens::DROP_OVERLAY_BASE.with_alpha(0.25)),
                        BorderColor::all(tokens::ACCENT_BLUE),
                        GlobalZIndex(150),
                    ))
                    .id();

                new_overlay = Some(overlay);
                break;
            }

            if new_target.is_none() {
                for (area_entity, computed, ui_transform) in &areas {
                    let area_rect = logical_rect(computed, ui_transform);
                    if !area_rect.contains(cursor) {
                        continue;
                    }

                    if let Some(edge) = cursor_edge(area_rect, cursor) {
                        new_target = Some(DropTarget::AreaEdge {
                            area: area_entity,
                            edge,
                        });

                        let overlay_rect = edge_overlay_rect(area_rect, edge);
                        let overlay = commands
                            .spawn((
                                DropOverlay,
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(overlay_rect.min.x),
                                    top: Val::Px(overlay_rect.min.y),
                                    width: Val::Px(overlay_rect.size().x),
                                    height: Val::Px(overlay_rect.size().y),
                                    border: UiRect::all(Val::Px(2.0)),
                                    border_radius: BorderRadius::all(Val::Px(4.0)),
                                    ..default()
                                },
                                BackgroundColor(tokens::DROP_OVERLAY_BASE.with_alpha(0.25)),
                                BorderColor::all(tokens::ACCENT_BLUE),
                                GlobalZIndex(150),
                            ))
                            .id();
                        new_overlay = Some(overlay);
                    } else {
                        new_target = Some(DropTarget::Panel(area_entity));

                        let overlay = commands
                            .spawn((
                                DropOverlay,
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(area_rect.min.x),
                                    top: Val::Px(area_rect.min.y),
                                    width: Val::Px(area_rect.size().x),
                                    height: Val::Px(area_rect.size().y),
                                    border: UiRect::all(Val::Px(2.0)),
                                    border_radius: BorderRadius::all(Val::Px(4.0)),
                                    ..default()
                                },
                                BackgroundColor(tokens::DROP_OVERLAY_BASE.with_alpha(0.12)),
                                BorderColor::all(tokens::ACCENT_BLUE),
                                GlobalZIndex(150),
                            ))
                            .id();
                        new_overlay = Some(overlay);
                    }

                    break;
                }
            }

            // Fall through to viewport hit-test when the cursor isn't
            // over any DockArea. Lets a tab drop on the viewport's edge
            // re-populate a collapsed side panel.
            if new_target.is_none() {
                for (computed, ui_transform) in &viewports {
                    let viewport_rect = logical_rect(computed, ui_transform);
                    if !viewport_rect.contains(cursor) {
                        continue;
                    }

                    let Some(edge) = cursor_edge(viewport_rect, cursor) else {
                        break;
                    };
                    if edge == DropEdge::Top {
                        // No top anchor above the viewport.
                        break;
                    }
                    let anchor_id = match edge {
                        DropEdge::Left => "left",
                        DropEdge::Right => "right_sidebar",
                        DropEdge::Bottom => "bottom_dock",
                        DropEdge::Top => unreachable!(),
                    };

                    new_target = Some(DropTarget::ViewportEdge {
                        anchor_id: anchor_id.to_string(),
                        edge,
                    });

                    let overlay_rect = edge_overlay_rect(viewport_rect, edge);
                    let overlay = commands
                        .spawn((
                            DropOverlay,
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(overlay_rect.min.x),
                                top: Val::Px(overlay_rect.min.y),
                                width: Val::Px(overlay_rect.size().x),
                                height: Val::Px(overlay_rect.size().y),
                                border: UiRect::all(Val::Px(2.0)),
                                border_radius: BorderRadius::all(Val::Px(4.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.126, 0.431, 0.784, 0.25)),
                            BorderColor::all(tokens::ACCENT_BLUE),
                            GlobalZIndex(150),
                        ))
                        .id();
                    new_overlay = Some(overlay);
                    break;
                }
            }

            if let DockDragState::Dragging {
                drop_target,
                overlay_entity,
                cursor_pos,
                ..
            } = &mut *drag_state
            {
                *drop_target = new_target;
                *overlay_entity = new_overlay;
                *cursor_pos = cursor;
            }

            trigger.propagate(false);
        }
        _ => {}
    }
}

fn on_drag_end(
    _trigger: On<Pointer<DragEnd>>,
    mut drag_state: ResMut<DockDragState>,
    mut commands: Commands,
) {
    let state = std::mem::take(&mut *drag_state);
    match state {
        DockDragState::Dragging {
            ghost_entity,
            overlay_entity,
            drop_target,
            window_id,
            source_area,
            ..
        } => {
            commands.entity(ghost_entity).despawn();
            if let Some(overlay) = overlay_entity {
                commands.entity(overlay).despawn();
            }

            if let Some(target) = drop_target {
                match target {
                    DropTarget::Panel(target_area) => {
                        if target_area != source_area {
                            let wid = window_id.clone();
                            commands.queue(move |world: &mut World| {
                                drop_on_area(world, &wid, target_area);
                            });
                        }
                    }
                    DropTarget::AreaEdge { area, edge } => {
                        let wid = window_id.clone();
                        commands.queue(move |world: &mut World| {
                            drop_on_edge(world, &wid, area, edge);
                        });
                    }
                    DropTarget::ViewportEdge { anchor_id, edge } => {
                        let wid = window_id.clone();
                        commands.queue(move |world: &mut World| {
                            drop_on_viewport_edge(world, &wid, &anchor_id, edge);
                        });
                    }
                    DropTarget::TabRow { bar, index } => {
                        let wid = window_id.clone();
                        commands.queue(move |world: &mut World| {
                            drop_on_tab_row(world, &wid, bar, index);
                        });
                    }
                }
            }
        }
        DockDragState::PendingDrag { .. } | DockDragState::Idle => {}
    }

    *drag_state = DockDragState::Idle;
}

fn cancel_drag_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut drag_state: ResMut<DockDragState>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }

    let state = std::mem::take(&mut *drag_state);
    if let DockDragState::Dragging {
        ghost_entity,
        overlay_entity,
        ..
    } = state
    {
        commands.entity(ghost_entity).despawn();
        if let Some(overlay) = overlay_entity {
            commands.entity(overlay).despawn();
        }
    }

    *drag_state = DockDragState::Idle;
}

/// Move `window_id` into the leaf bound to `target_area`.
fn drop_on_area(world: &mut World, window_id: &str, target_area: Entity) {
    let Some(binding) = world.entity(target_area).get::<NodeBinding>().copied() else {
        return;
    };
    world
        .resource_mut::<DockTree>()
        .move_window(window_id, binding.0);
}

/// Split the leaf bound to `target_area` along `edge` and place
/// `window_id` into the new sibling.
fn drop_on_edge(world: &mut World, window_id: &str, target_area: Entity, edge: DropEdge) {
    let Some(binding) = world.entity(target_area).get::<NodeBinding>().copied() else {
        return;
    };
    let tree_edge = match edge {
        DropEdge::Top => TreeEdge::Top,
        DropEdge::Bottom => TreeEdge::Bottom,
        DropEdge::Left => TreeEdge::Left,
        DropEdge::Right => TreeEdge::Right,
    };
    let mut tree = world.resource_mut::<DockTree>();
    tree.remove_window(window_id);
    tree.split(binding.0, tree_edge, window_id.to_string());
}

/// Drop `window_id` onto the viewport's `edge`, routing into the anchor
/// that owns that edge (e.g. `Right` → `right_sidebar`). If the anchor's
/// first leaf is empty (collapsed panel), the window is added to it so
/// the reconciler un-hides the host next tick. Otherwise the leaf is
/// split at `edge` to create a sibling leaf holding the window.
fn drop_on_viewport_edge(world: &mut World, window_id: &str, anchor_id: &str, edge: DropEdge) {
    let mut tree = world.resource_mut::<DockTree>();
    let Some(root) = tree.anchor(anchor_id) else {
        return;
    };
    let Some((leaf_id, leaf_is_empty)) = tree
        .leaves_under(root)
        .first()
        .map(|(id, leaf)| (*id, leaf.windows.is_empty()))
    else {
        return;
    };

    tree.remove_window(window_id);

    if leaf_is_empty {
        if let Some(crate::tree::DockNode::Leaf(l)) = tree.get_mut(leaf_id) {
            l.windows.push(window_id.to_string());
            l.active = Some(window_id.to_string());
        }
    } else {
        let tree_edge = match edge {
            DropEdge::Top => TreeEdge::Top,
            DropEdge::Bottom => TreeEdge::Bottom,
            DropEdge::Left => TreeEdge::Left,
            DropEdge::Right => TreeEdge::Right,
        };
        tree.split(leaf_id, tree_edge, window_id.to_string());
    }
}

/// Drop `window_id` onto the leaf bound to the `tab_row`'s area at index `index`
fn drop_on_tab_row(world: &mut World, window_id: &str, tab_row: Entity, index: usize) {
    let mut parent_query = world.query::<&ChildOf>();
    let parent_query = parent_query.query(world);

    let mut binding = None;
    for parent in parent_query.iter_ancestors(tab_row) {
        if let Some(node_binding) = world.entity(parent).get::<NodeBinding>() {
            binding = Some(node_binding);
            break;
        }
    }

    let Some(binding) = binding.copied() else {
        warn!("No `NodeBinding` found in parents of tab row {tab_row}");
        return;
    };

    let mut tree = world.resource_mut::<DockTree>();
    tree.insert_window(window_id, binding.0, true, Some(index));
}

fn find_parent_area(
    entity: Entity,
    parents: &Query<&ChildOf>,
    areas: &Query<(Entity, &ComputedNode, &UiGlobalTransform), With<DockArea>>,
) -> Option<Entity> {
    let mut current = entity;
    loop {
        if areas.contains(current) {
            return Some(current);
        }
        let Ok(parent) = parents.get(current) else {
            return None;
        };
        current = parent.parent();
    }
}

fn cursor_edge(rect: Rect, cursor: Vec2) -> Option<DropEdge> {
    let rel = cursor - rect.center();
    let frac_x = rel.x / rect.size().x;
    let frac_y = rel.y / rect.size().y;

    // The center region is a no-op.
    // the outer n% of the rect's volume are the drop edges.
    const EDGE_PERCENT: f32 = 0.25;

    if frac_x < -EDGE_PERCENT {
        Some(DropEdge::Left)
    } else if frac_x > EDGE_PERCENT {
        Some(DropEdge::Right)
    } else if frac_y > EDGE_PERCENT {
        Some(DropEdge::Bottom)
    } else if frac_y < -EDGE_PERCENT {
        // Top is the lowest priority since the viewport is allowed to skip it
        Some(DropEdge::Top)
    } else {
        None
    }
}

fn edge_overlay_rect(rect: Rect, edge: DropEdge) -> Rect {
    let (axis, factor) = match edge {
        DropEdge::Top => (-Vec2::Y * rect.size().y, Vec2::new(1.0, 0.5)),
        DropEdge::Bottom => (Vec2::Y * rect.size().y, Vec2::new(1.0, 0.5)),
        DropEdge::Left => (-Vec2::X * rect.size().x, Vec2::new(0.5, 1.0)),
        DropEdge::Right => (Vec2::X * rect.size().x, Vec2::new(0.5, 1.0)),
    };
    // use half length and move the center by 25% of the axis length make the overlay
    // cover exactly half of the area along a given axis
    Rect::from_center_size(rect.center() + axis * 0.25, rect.size() * factor)
}

fn is_far_side(mouse_pos: Vec2, child_pos: Vec2, parent: &Node) -> (bool, bool) {
    return match parent.flex_direction {
        FlexDirection::Row => (is_far_side(mouse_pos, child_pos, false), false),
        FlexDirection::RowReverse => (!is_far_side(mouse_pos, child_pos, false), false),
        FlexDirection::Column => (is_far_side(mouse_pos, child_pos, true), true),
        FlexDirection::ColumnReverse => (!is_far_side(mouse_pos, child_pos, true), true),
    };

    fn is_far_side(mouse_pos: Vec2, child_pos: Vec2, is_vertical: bool) -> bool {
        let diff = if is_vertical {
            mouse_pos.y - child_pos.y
        } else {
            mouse_pos.x - child_pos.x
        };

        diff > 0.0
    }
}
