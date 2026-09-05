//! Selection overlay and direct manipulation for the UI Canvas stage.
//!
//! Manipulation drives the *authored* `Node` from editor-space pointer deltas.
//! The overlay is ordinary editor chrome rather than part of the projection, so
//! a projection rebuild in the middle of a drag cannot interrupt the gesture.

use bevy::{
    picking::events::{Drag, DragEnd, DragStart, Pointer},
    prelude::*,
    ui::{ComputedNode, UiGlobalTransform},
};
use jackdaw_feathers::tokens;
use jackdaw_scene_types::SceneNodeId;

use crate::{
    EditorEntity,
    selection::Selection,
    ui_authoring::UiAuthoring,
    ui_canvas::{UiCanvasMode, UiCanvasPanelHost},
    ui_projection::{ProjectedFrom, UiProjection},
};

const HANDLE_SIZE: f32 = 8.0;

/// Which edges a handle drags. `0` means the edge is not moved.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
struct UiResizeHandle {
    x: i8,
    y: i8,
}

/// The outline drawn around the selected authored UI node in one panel.
#[derive(Component, Clone, Copy)]
struct UiSelectionOverlay {
    host: Entity,
}

/// The gesture in progress, if any. One gesture is one history entry.
#[derive(Resource, Default)]
struct UiManipulation {
    entity: Option<Entity>,
    before: Option<Node>,
    /// Edges being resized; `(0, 0)` is a move.
    edges: (i8, i8),
    /// Authored-pixel rect at gesture start: left, top, width, height.
    start: Vec4,
    /// Authored pixels per editor logical pixel.
    scale: f32,
}

pub struct UiStagePlugin;

impl Plugin for UiStagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiManipulation>()
            .add_observer(on_gesture_start)
            .add_observer(on_gesture_drag)
            .add_observer(on_gesture_end)
            .add_systems(
                Update,
                (cancel_manipulation, sync_selection_overlays).chain(),
            );
    }
}

/// Whether a canvas gesture is in progress. Projection refresh stands down for
/// the duration: rebuilding a projection under the pointer would drop the
/// entities the views are showing, and the gesture already mirrors its edits.
pub(crate) fn manipulating(world: &World) -> bool {
    world
        .get_resource::<UiManipulation>()
        .is_some_and(|manipulation| manipulation.entity.is_some())
}

/// Push a live authored `Node` into every projection of that node.
fn mirror_to_projections(world: &mut World, authored: Entity, node: &Node) {
    let Some(node_id) = world.get::<SceneNodeId>(authored).copied() else {
        return;
    };
    let mut canvas_hosts = world.query::<&UiCanvasPanelHost>();
    let mut handles = canvas_hosts
        .iter(world)
        .filter_map(|host| host.projection)
        .collect::<Vec<_>>();
    let mut viewport_hosts = world.query::<&crate::viewport_ui::ViewportUiHost>();
    handles.extend(
        viewport_hosts
            .iter(world)
            .flat_map(|host| host.projections.iter().map(|projection| projection.handle)),
    );
    for handle in handles {
        let Some(target) = UiProjection::projected_entity(world, handle, node_id) else {
            continue;
        };
        if let Some(mut projected) = world.get_mut::<Node>(target) {
            *projected = node.clone();
        }
    }
}

/// Authored-space rect of one node as projected into `host`'s stage, in editor
/// logical pixels relative to the stage, plus the authored-pixel scale.
struct ProjectedRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    /// Authored pixels per editor logical pixel.
    scale: f32,
}

fn projected_rect(
    world: &World,
    host: &UiCanvasPanelHost,
    authored: Entity,
) -> Option<ProjectedRect> {
    let node_id = world.get::<SceneNodeId>(authored).copied()?;
    let projection = host.projection?;
    let projected = UiProjection::projected_entity(world, projection, node_id)?;
    let computed = world.get::<ComputedNode>(projected)?;
    let transform = world.get::<UiGlobalTransform>(projected)?;
    let size = computed.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    // The stage's render target is sized from the stage's physical size, so one
    // authored pixel is one target pixel and `inverse_scale_factor` converts
    // back into the editor's logical space.
    let inverse = world
        .get::<ComputedNode>(host.stage)
        .map(ComputedNode::inverse_scale_factor)
        .unwrap_or(1.0);
    let center = transform.translation;
    Some(ProjectedRect {
        left: (center.x - size.x / 2.0) * inverse,
        top: (center.y - size.y / 2.0) * inverse,
        width: size.x * inverse,
        height: size.y * inverse,
        scale: if inverse > 0.0 { 1.0 / inverse } else { 1.0 },
    })
}

/// Keep exactly one overlay per panel, covering the selected authored node.
fn sync_selection_overlays(world: &mut World) {
    let mut host_query = world.query::<(Entity, &UiCanvasPanelHost)>();
    let panels = host_query
        .iter(world)
        .map(|(entity, host)| (entity, *host))
        .collect::<Vec<_>>();
    let mut overlay_query = world.query::<(Entity, &UiSelectionOverlay)>();
    let overlays = overlay_query
        .iter(world)
        .map(|(entity, overlay)| (entity, overlay.host))
        .collect::<Vec<_>>();
    let selected = world
        .get_resource::<Selection>()
        .and_then(Selection::primary)
        .filter(|entity| world.get::<ProjectedFrom>(*entity).is_none());

    for (host_entity, panel) in panels {
        let overlay = overlays
            .iter()
            .find(|(_, host)| *host == host_entity)
            .map(|(entity, _)| *entity);
        let Some(entity) = selected.filter(|_| panel.mode == UiCanvasMode::Ui) else {
            if let Some(overlay) = overlay
                && let Ok(overlay) = world.get_entity_mut(overlay)
            {
                overlay.despawn();
            }
            continue;
        };
        let overlay = match overlay {
            Some(overlay) => overlay,
            None if world.get_entity(panel.stage).is_ok() => {
                spawn_overlay(world, host_entity, panel.stage)
            }
            None => continue,
        };
        // A projection rebuilt this frame has no layout yet. Hold the previous
        // rect rather than dropping the overlay for a frame.
        let Some(rect) = projected_rect(world, &panel, entity) else {
            continue;
        };
        if let Some(mut node) = world.get_mut::<Node>(overlay) {
            node.left = px(rect.left);
            node.top = px(rect.top);
            node.width = px(rect.width.max(1.0));
            node.height = px(rect.height.max(1.0));
        }
    }
}

fn spawn_overlay(world: &mut World, host: Entity, stage: Entity) -> Entity {
    let overlay = world
        .spawn((
            UiSelectionOverlay { host },
            EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                border: UiRect::all(px(1)),
                ..default()
            },
            BorderColor::all(tokens::ACCENT_BLUE),
            ZIndex(50),
            ChildOf(stage),
        ))
        .id();
    for (x, y) in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
    ] {
        let half = px(-HANDLE_SIZE / 2.0);
        let mut node = Node {
            position_type: PositionType::Absolute,
            width: px(HANDLE_SIZE),
            height: px(HANDLE_SIZE),
            ..default()
        };
        match x {
            -1 => node.left = half,
            1 => node.right = half,
            _ => {
                node.left = percent(50);
                node.margin.left = px(-HANDLE_SIZE / 2.0);
            }
        }
        match y {
            -1 => node.top = half,
            1 => node.bottom = half,
            _ => {
                node.top = percent(50);
                node.margin.top = px(-HANDLE_SIZE / 2.0);
            }
        }
        world.spawn((
            UiResizeHandle { x, y },
            EditorEntity,
            node,
            BackgroundColor(tokens::ACCENT_BLUE),
            ChildOf(overlay),
        ));
    }
    overlay
}

/// Which edges a gesture on `target` drags, if `target` is part of an overlay.
///
/// Resolved synchronously so the observer can stop propagation before the event
/// climbs out of the canvas panel and into the dock.
fn gesture_edges(
    target: Entity,
    handles: &Query<&UiResizeHandle>,
    overlays: &Query<&UiSelectionOverlay>,
    parents: &Query<&ChildOf>,
) -> Option<(Entity, (i8, i8))> {
    if let Ok(handle) = handles.get(target) {
        let overlay = parents.get(target).ok().map(ChildOf::parent)?;
        overlays.get(overlay).ok()?;
        return Some((overlay, (handle.x, handle.y)));
    }
    overlays.get(target).ok()?;
    Some((target, (0, 0)))
}

fn on_gesture_start(
    mut event: On<Pointer<DragStart>>,
    handles: Query<&UiResizeHandle>,
    overlays: Query<&UiSelectionOverlay>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let target = event.event_target();
    let Some((overlay, edges)) = gesture_edges(target, &handles, &overlays, &parents) else {
        return;
    };
    event.propagate(false);
    commands.queue(move |world: &mut World| {
        let Some(host) = world
            .get::<UiSelectionOverlay>(overlay)
            .map(|overlay| overlay.host)
        else {
            return;
        };
        let Some(panel) = world.get::<UiCanvasPanelHost>(host).copied() else {
            return;
        };
        let Some(entity) = world
            .get_resource::<Selection>()
            .and_then(Selection::primary)
        else {
            return;
        };
        let Some(before) = world.get::<Node>(entity).cloned() else {
            return;
        };
        let Some(rect) = projected_rect(world, &panel, entity) else {
            return;
        };
        // Absolute placement is what a free move edits. Promote on first drag
        // so a flex child can be positioned instead of silently refusing.
        let (left, top) = authored_offset(world, entity, &rect);
        let mut manipulation = world.resource_mut::<UiManipulation>();
        manipulation.entity = Some(entity);
        manipulation.before = Some(before);
        manipulation.edges = edges;
        manipulation.start =
            Vec4::new(left, top, rect.width * rect.scale, rect.height * rect.scale);
        manipulation.scale = rect.scale;
    });
}

/// Authored left/top for the gesture, in authored pixels.
///
/// A node with no explicit offset starts from where layout actually put it, so
/// promoting a flex child to absolute placement does not make it jump.
fn authored_offset(world: &World, entity: Entity, rect: &ProjectedRect) -> (f32, f32) {
    if let Some(node) = world.get::<Node>(entity)
        && node.position_type == PositionType::Absolute
        && let (Val::Px(left), Val::Px(top)) = (node.left, node.top)
    {
        return (left, top);
    }
    (rect.left * rect.scale, rect.top * rect.scale)
}

fn on_gesture_drag(
    mut event: On<Pointer<Drag>>,
    handles: Query<&UiResizeHandle>,
    overlays: Query<&UiSelectionOverlay>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let target = event.event_target();
    if gesture_edges(target, &handles, &overlays, &parents).is_none() {
        return;
    }
    event.propagate(false);
    let distance = event.distance;
    commands.queue(move |world: &mut World| {
        let manipulation = world.resource::<UiManipulation>();
        let Some(entity) = manipulation.entity else {
            return;
        };
        let (edges, start, scale) = (manipulation.edges, manipulation.start, manipulation.scale);
        let delta = distance * scale;
        let (mut left, mut top, mut width, mut height) = (start.x, start.y, start.z, start.w);
        match edges {
            (0, 0) => {
                left += delta.x;
                top += delta.y;
            }
            (x, y) => {
                if x < 0 {
                    left += delta.x;
                    width -= delta.x;
                } else if x > 0 {
                    width += delta.x;
                }
                if y < 0 {
                    top += delta.y;
                    height -= delta.y;
                } else if y > 0 {
                    height += delta.y;
                }
            }
        }
        let Some(mut node) = world.get_mut::<Node>(entity) else {
            return;
        };
        node.position_type = PositionType::Absolute;
        node.left = px(left.round());
        node.top = px(top.round());
        node.right = Val::Auto;
        node.bottom = Val::Auto;
        if edges != (0, 0) {
            node.width = px(width.max(1.0).round());
            node.height = px(height.max(1.0).round());
        }
        let updated = node.clone();
        mirror_to_projections(world, entity, &updated);
    });
}

fn on_gesture_end(
    mut event: On<Pointer<DragEnd>>,
    handles: Query<&UiResizeHandle>,
    overlays: Query<&UiSelectionOverlay>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if gesture_edges(event.event_target(), &handles, &overlays, &parents).is_none() {
        return;
    }
    event.propagate(false);
    commands.queue(|world: &mut World| finish_manipulation(world, true));
}

fn finish_manipulation(world: &mut World, commit: bool) {
    let mut manipulation = world.resource_mut::<UiManipulation>();
    let Some(entity) = manipulation.entity.take() else {
        return;
    };
    let Some(before) = manipulation.before.take() else {
        return;
    };
    if !commit {
        if let Some(mut node) = world.get_mut::<Node>(entity) {
            *node = before;
        }
        return;
    }
    let Some(after) = world.get::<Node>(entity).cloned() else {
        return;
    };
    UiAuthoring::push_layout_edit(world, entity, before, after);
}

/// Escape restores the exact node the gesture started from.
fn cancel_manipulation(
    keys: Res<ButtonInput<KeyCode>>,
    manipulation: Res<UiManipulation>,
    mut commands: Commands,
) {
    if manipulation.entity.is_some() && keys.just_pressed(KeyCode::Escape) {
        commands.queue(|world: &mut World| finish_manipulation(world, false));
    }
}
