//! The "Add Node" popover; a category-grouped picker listing every
//! registered [`NodeTypeDescriptor`](crate::NodeTypeDescriptor).
//!
//! Opened by right-clicking on the canvas background or by pressing Tab
//! while hovering the canvas (Maya Node Editor / Blender Shift+A pattern).
//! Clicking an entry issues [`AddGraphNodeCmd`] with the cursor position
//! translated into canvas-space and closes the popover. An outside click
//! or pressing Escape also closes it.
//!
//! The popover is intentionally lightweight; no search input for Phase 3
//! so we don't need the full `text_edit` stack. Phase 6 polish can add
//! filtering and keyboard navigation to match Maya's quick-add UX more
//! closely.

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use jackdaw_commands::CommandHistory;

use crate::canvas::{GraphCanvasViewport, GraphCanvasWorld};
use crate::commands::AddGraphNodeCmd;
use crate::registry::NodeTypeRegistry;

/// Marker on the popover root. Stores the graph it belongs to, the
/// canvas-space position where a selected node should be spawned, and the
/// backdrop entity that should be despawned together with the popover.
#[derive(Component, Debug, Clone, Copy)]
pub struct AddNodePopover {
    pub graph: Entity,
    pub spawn_position: Vec2,
    pub backdrop: Entity,
}

/// Full-screen invisible overlay sitting one z-index below the popover.
/// Absorbs clicks that land outside the popover and closes it via
/// [`on_backdrop_click`].
#[derive(Component, Debug, Clone, Copy)]
pub struct AddNodeBackdrop {
    pub popover: Entity,
}

/// Marker on a single entry in the popover list. Stores its registry id so
/// the click handler can look up the descriptor.
#[derive(Component, Debug, Clone)]
pub struct AddNodeEntry {
    pub popover: Entity,
    pub node_type: String,
}

const POPOVER_BG: Color = Color::srgb(0.11, 0.12, 0.14);
const POPOVER_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.1);
const POPOVER_WIDTH: f32 = 220.0;
const POPOVER_MAX_HEIGHT: f32 = 400.0;
const HEADER_BG: Color = Color::srgb(0.08, 0.09, 0.11);
const HEADER_TEXT: Color = Color::srgb(0.6, 0.62, 0.68);
const ENTRY_TEXT: Color = Color::srgb(0.88, 0.89, 0.92);
const ENTRY_HOVER_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);

/// Open the popover at `position` (screen pixels) for `graph`.
///
/// `spawn_position` is the cursor's canvas-space position where the picked
/// node should be inserted. Previously-open popovers (and their backdrops)
/// are despawned first so only one popover lives at a time.
///
/// Structure:
/// * `AddNodeBackdrop` at `GlobalZIndex(999)`; full-screen transparent,
///   `Pickable::default()`, absorbs outside-clicks and closes on click.
/// * `AddNodePopover` at `GlobalZIndex(1000)`; the styled panel. Clicks on
///   any descendant hit the popover subtree first (higher z), never the
///   backdrop.
pub fn spawn_popover(
    commands: &mut Commands,
    registry: &NodeTypeRegistry,
    existing: &Query<Entity, With<AddNodePopover>>,
    existing_backdrops: &Query<Entity, With<AddNodeBackdrop>>,
    graph: Entity,
    screen_position: Vec2,
    spawn_position: Vec2,
) {
    // Close any existing popover + backdrop first.
    for entity in existing.iter() {
        if let Ok(mut ec) = commands.get_entity(entity) {
            ec.despawn();
        }
    }
    for entity in existing_backdrops.iter() {
        if let Ok(mut ec) = commands.get_entity(entity) {
            ec.despawn();
        }
    }

    // Reserve the popover entity id up-front so the backdrop can point at
    // it before the popover itself is spawned.
    let popover_entity = commands.spawn_empty().id();
    let backdrop_entity = commands.spawn_empty().id();

    // Backdrop; full-screen transparent overlay beneath the popover.
    commands.entity(backdrop_entity).insert((
        AddNodeBackdrop {
            popover: popover_entity,
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::NONE),
        GlobalZIndex(999),
        Pickable::default(),
    ));

    // Popover root.
    commands.entity(popover_entity).insert((
        AddNodePopover {
            graph,
            spawn_position,
            backdrop: backdrop_entity,
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(screen_position.x),
            top: Val::Px(screen_position.y),
            width: Val::Px(POPOVER_WIDTH),
            max_height: Val::Px(POPOVER_MAX_HEIGHT),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(POPOVER_BG),
        BorderColor::all(POPOVER_BORDER),
        GlobalZIndex(1000),
        Pickable::default(),
    ));

    // Title bar.
    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(POPOVER_BORDER),
        BackgroundColor(HEADER_BG),
        ChildOf(popover_entity),
        children![(
            Text::new("Add Node"),
            TextFont {
                font_size: 12.0,
                ..default()
            },
            TextColor(Color::srgb(0.88, 0.89, 0.92)),
        )],
    ));

    // Scrollable list of categories + entries.
    let list = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::scroll_y(),
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                padding: UiRect::vertical(Val::Px(4.0)),
                ..default()
            },
            ChildOf(popover_entity),
        ))
        .id();

    for (category, entries) in registry.by_category() {
        // Category header.
        commands.spawn((
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::new(Val::Px(10.0), Val::Px(10.0), Val::Px(6.0), Val::Px(2.0)),
                ..default()
            },
            ChildOf(list),
            children![(
                Text::new(category.to_uppercase()),
                TextFont {
                    font_size: 10.0,
                    ..default()
                },
                TextColor(HEADER_TEXT),
            )],
        ));

        // Category entries.
        for desc in entries {
            commands.spawn((
                AddNodeEntry {
                    popover: popover_entity,
                    node_type: desc.id.clone(),
                },
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(5.0)),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                Pickable::default(),
                ChildOf(list),
                children![
                    // Accent dot keyed off the descriptor's color.
                    (
                        Node {
                            width: Val::Px(8.0),
                            height: Val::Px(8.0),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(desc.accent_color),
                    ),
                    (
                        Text::new(desc.display_name.clone()),
                        TextFont {
                            font_size: 12.0,
                            ..default()
                        },
                        TextColor(ENTRY_TEXT),
                    ),
                ],
            ));
        }
    }
}

/// Observer on the backdrop entity that closes the popover + backdrop when
/// the backdrop itself is clicked.
///
/// Because the backdrop sits at `GlobalZIndex(999)` and the popover at
/// `1000`, clicks on any popover descendant are picked before they reach
/// the backdrop. Clicks outside the popover hit the backdrop and fire this
/// observer; straightforward, no hover-map gymnastics.
pub fn on_backdrop_click(
    mut event: On<Pointer<Click>>,
    backdrops: Query<&AddNodeBackdrop>,
    mut commands: Commands,
) {
    let Ok(backdrop) = backdrops.get(event.event_target()) else {
        return;
    };
    event.propagate(false);

    let backdrop_entity = event.event_target();
    let popover_entity = backdrop.popover;

    if let Ok(mut ec) = commands.get_entity(popover_entity) {
        ec.despawn();
    }
    if let Ok(mut ec) = commands.get_entity(backdrop_entity) {
        ec.despawn();
    }
}

/// Close the popover (and its backdrop) on Escape. Simpler than hooking
/// into the whole dismissal machinery; the backdrop handles outside-click
/// closure on its own.
pub fn handle_popover_escape(
    keys: Res<ButtonInput<KeyCode>>,
    popovers: Query<(Entity, &AddNodePopover)>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    for (popover_entity, popover) in popovers.iter() {
        if let Ok(mut ec) = commands.get_entity(popover_entity) {
            ec.despawn();
        }
        if let Ok(mut ec) = commands.get_entity(popover.backdrop) {
            ec.despawn();
        }
    }
}

/// Handle a click on a list entry: issue `AddGraphNodeCmd` + close.
pub fn on_entry_click(
    mut event: On<Pointer<Click>>,
    entries: Query<&AddNodeEntry>,
    popovers: Query<&AddNodePopover>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok(entry) = entries.get(event.event_target()) else {
        return;
    };
    let Ok(popover) = popovers.get(entry.popover) else {
        return;
    };
    let graph = popover.graph;
    let position = popover.spawn_position;
    let node_type = entry.node_type.clone();
    let popover_entity = entry.popover;
    let backdrop_entity = popover.backdrop;
    event.propagate(false);

    commands.queue(move |world: &mut World| {
        let cmd = Box::new(AddGraphNodeCmd::new(graph, node_type, position));
        let mut history = world
            .remove_resource::<CommandHistory>()
            .unwrap_or_default();
        history.execute(cmd, world);
        world.insert_resource(history);

        // Close the popover + backdrop.
        if let Ok(ec) = world.get_entity_mut(popover_entity) {
            ec.despawn();
        }
        if let Ok(ec) = world.get_entity_mut(backdrop_entity) {
            ec.despawn();
        }
    });
}

/// Hover highlighting for list entries.
pub fn on_entry_over(
    event: On<Pointer<bevy::picking::events::Over>>,
    mut bg: Query<&mut BackgroundColor, With<AddNodeEntry>>,
) {
    if let Ok(mut color) = bg.get_mut(event.event_target()) {
        color.0 = ENTRY_HOVER_BG;
    }
}

pub fn on_entry_out(
    event: On<Pointer<bevy::picking::events::Out>>,
    mut bg: Query<&mut BackgroundColor, With<AddNodeEntry>>,
) {
    if let Ok(mut color) = bg.get_mut(event.event_target()) {
        color.0 = Color::NONE;
    }
}

/// Right-click on the canvas background opens the popover at the cursor.
pub fn on_canvas_right_click(
    mut event: On<Pointer<Click>>,
    viewports: Query<(&GraphCanvasViewport, &ComputedNode, &UiGlobalTransform)>,
    canvas_worlds: Query<(&GraphCanvasWorld, &UiGlobalTransform)>,
    registry: Res<NodeTypeRegistry>,
    existing: Query<Entity, With<AddNodePopover>>,
    existing_backdrops: Query<Entity, With<AddNodeBackdrop>>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Secondary {
        return;
    }
    let Ok((viewport, _computed, _gt)) = viewports.get(event.event_target()) else {
        return;
    };
    event.propagate(false);

    let cursor = event.pointer_location.position;
    let spawn_pos = cursor_to_canvas_space(cursor, viewport.graph, &canvas_worlds);

    spawn_popover(
        &mut commands,
        &registry,
        &existing,
        &existing_backdrops,
        viewport.graph,
        cursor,
        spawn_pos,
    );
}

/// Tab key opens the popover at the cursor when hovering the canvas
/// (Maya / Blender quick-add pattern).
pub fn handle_tab_quick_add(
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    hover_map: Res<bevy::picking::hover::HoverMap>,
    viewports: Query<&GraphCanvasViewport>,
    canvas_worlds: Query<(&GraphCanvasWorld, &UiGlobalTransform)>,
    registry: Res<NodeTypeRegistry>,
    existing: Query<Entity, With<AddNodePopover>>,
    existing_backdrops: Query<Entity, With<AddNodeBackdrop>>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }

    // Find the graph whose canvas the pointer is currently over.
    let mut hovered_graph: Option<Entity> = None;
    for pointer_map in hover_map.values() {
        for &entity in pointer_map.keys() {
            if let Ok(viewport) = viewports.get(entity) {
                hovered_graph = Some(viewport.graph);
                break;
            }
        }
        if hovered_graph.is_some() {
            break;
        }
    }
    let Some(graph) = hovered_graph else {
        return;
    };

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let spawn_pos = cursor_to_canvas_space(cursor, graph, &canvas_worlds);
    spawn_popover(
        &mut commands,
        &registry,
        &existing,
        &existing_backdrops,
        graph,
        cursor,
        spawn_pos,
    );
}

/// Convert a screen-space cursor position to canvas-world local pixels,
/// accounting for the current pan and zoom of the graph.
fn cursor_to_canvas_space(
    cursor: Vec2,
    graph: Entity,
    canvas_worlds: &Query<(&GraphCanvasWorld, &UiGlobalTransform)>,
) -> Vec2 {
    for (world, gt) in canvas_worlds.iter() {
        if world.graph != graph {
            continue;
        }
        let (scale, _angle, translation) = gt.to_scale_angle_translation();
        let sx = scale.x.max(f32::EPSILON);
        let sy = scale.y.max(f32::EPSILON);
        return Vec2::new(
            (cursor.x - translation.x) / sx,
            (cursor.y - translation.y) / sy,
        );
    }
    cursor
}
