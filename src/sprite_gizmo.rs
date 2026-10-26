//! The move gizmo for sprites on a 2D world stage, and the drags that run it.
//! The handles are stage UI nodes like the outlines of [`crate::sprite_stage`],
//! so they stay crisp at any zoom and stay out of the panel's render image.
//!
//! A move takes the whole selection. Each drag lands as one history entry;
//! Escape puts back what the drag started from.

use bevy::{
    asset::AssetEvent,
    ecs::system::SystemParam,
    math::Affine3A,
    picking::{
        events::{PointerDrag, PointerDragEnd, PointerDragStart, PointerPress},
        prelude::Pickable,
    },
    prelude::*,
    sprite::Anchor,
    transform::TransformSystems,
    ui::{ComputedNode, UiGlobalTransform, UiSystems, UiTransform},
};
use jackdaw_feathers::tokens;
use jackdaw_scene_types::{Locked, Scene2dRoot};

use crate::{
    EditorEntity,
    active_tool::ActiveTool,
    commands::{CommandGroup, CommandHistory, EditorCommand, SetTransform},
    keybind_focus::KeybindFocus,
    prefab::AuthoredUiSceneRoot,
    selection::Selection,
    sprite_stage::{
        Scene2dRoots, SpritePixels, SpritePress, in_2d_scene, shows_2d_world, stage_world_point,
    },
    ui_stage::{stage_pixels_per_target_pixel, without_selected_ancestors},
    viewport_2d::{
        Scene2dViewport, Viewport2dMode, Viewport2dPanelHost, target_pixel_to_world,
        target_pixels_per_stage_pixel, world_to_target_pixel,
    },
    viewport_select::BOX_SELECT_DRAG_THRESHOLD,
};

/// Stage logical pixels from the anchor to the tip of a move axis.
const AXIS_LENGTH: f32 = 64.0;
/// Stage logical pixels from the anchor to where a move axis's shaft begins,
/// leaving the sprite around its anchor free for a body drag.
const AXIS_START: f32 = 12.0;
const SHAFT_WIDTH: f32 = 3.0;
const HANDLE_SIZE: f32 = 12.0;
const PIVOT_SIZE: f32 = 8.0;
/// Draw order of the gizmo inside the stage, above the selection outlines.
const GIZMO_Z: i32 = 60;

pub struct SpriteGizmoPlugin;

impl Plugin for SpriteGizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpriteManipulation>()
            .add_observer(on_handle_press)
            .add_observer(on_drag_start)
            .add_observer(on_drag)
            .add_observer(on_drag_end)
            .add_systems(
                Update,
                end_sprite_drag_off_input.run_if(|m: Res<SpriteManipulation>| m.is_running()),
            )
            .add_systems(
                PostUpdate,
                sync_sprite_gizmos
                    .after(TransformSystems::Propagate)
                    .before(UiSystems::Prepare),
            );
    }
}

/// What a drag on a 2D world stage takes hold of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpriteHandle {
    /// A selected sprite itself: a free move of the selection.
    Body,
    AxisX,
    AxisY,
}

/// One grabbable part of the gizmo.
#[derive(Component, Clone, Copy, Debug)]
pub struct SpriteGizmoHandle {
    pub kind: SpriteHandle,
    /// Where the handle is grabbed, in world units.
    pub world: Vec2,
}

/// The gizmo shown for the primary selected sprite on one panel's stage.
#[derive(Component)]
struct SpriteGizmo {
    host: Entity,
    sprite: Entity,
    tool: ActiveTool,
    parts: Vec<Entity>,
}

/// The sprite drag in progress, if any.
#[derive(Resource, Default)]
pub struct SpriteManipulation {
    drag: Option<SpriteDrag>,
}

impl SpriteManipulation {
    pub fn is_running(&self) -> bool {
        self.drag.is_some()
    }
}

struct SpriteDrag {
    /// The entity the pointer gesture targets: a handle, or the stage.
    target: Entity,
    host: Entity,
    kind: SpriteHandle,
    /// Whether the pointer has gone past the drag threshold; until it has,
    /// nothing moves.
    started: bool,
    items: Vec<DragItem>,
}

struct DragItem {
    entity: Entity,
    start: Transform,
    /// World to the parent's space.
    to_parent: Affine3A,
}

/// One node of the gizmo: a rect of `size` about `centre`, turned by `angle`,
/// in stage logical pixels.
struct Part {
    centre: Vec2,
    size: Vec2,
    angle: f32,
    color: Color,
    round: bool,
    /// The handle this part grabs and its grab point.
    handle: Option<(SpriteHandle, Vec2)>,
}

impl Part {
    fn square(centre: Vec2, size: f32, color: Color) -> Self {
        Self {
            centre,
            size: Vec2::splat(size),
            angle: 0.0,
            color,
            round: false,
            handle: None,
        }
    }

    fn bar(from: Vec2, to: Vec2, color: Color) -> Self {
        let delta = to - from;
        Self {
            size: Vec2::new(delta.length(), SHAFT_WIDTH),
            angle: delta.to_angle(),
            ..Self::square((from + to) / 2.0, 0.0, color)
        }
    }

    fn round(self) -> Self {
        Self {
            round: true,
            ..self
        }
    }

    fn grabs(self, kind: SpriteHandle, at: Vec2) -> Self {
        Self {
            handle: Some((kind, at)),
            ..self
        }
    }

    fn place(&self, node: &mut Node, transform: &mut UiTransform) {
        node.left = px(self.centre.x - self.size.x / 2.0);
        node.top = px(self.centre.y - self.size.y / 2.0);
        node.width = px(self.size.x);
        node.height = px(self.size.y);
        transform.rotation = Rot2::radians(self.angle);
    }
}

/// The parts of the gizmo for `tool` about a sprite anchored at `pivot`, in
/// stage logical pixels.
fn gizmo_parts(tool: ActiveTool, pivot: Vec2) -> Vec<Part> {
    match tool {
        ActiveTool::Translate => {
            let mut parts = vec![Part::square(pivot, PIVOT_SIZE, tokens::TEXT_PRIMARY).round()];
            for (kind, direction, color) in [
                (SpriteHandle::AxisX, Vec2::X, tokens::AXIS_X_COLOR),
                (SpriteHandle::AxisY, Vec2::NEG_Y, tokens::AXIS_Y_COLOR),
            ] {
                let tip = pivot + direction * AXIS_LENGTH;
                parts.push(Part::bar(pivot + direction * AXIS_START, tip, color).grabs(kind, tip));
                parts.push(Part::square(tip, HANDLE_SIZE, color).grabs(kind, tip));
            }
            parts
        }
        ActiveTool::Select | ActiveTool::Rotate | ActiveTool::Scale => Vec::new(),
    }
}

/// One gizmo the stages should be showing.
struct Wanted {
    host: Entity,
    stage: Entity,
    sprite: Entity,
    parts: Vec<Part>,
}

/// Keep the gizmo of the primary selected sprite on each 2D panel in
/// [`Viewport2dMode::Edit`], following the sprite, the view and the tool.
/// Updated in place, so a handle being dragged keeps its entity; does
/// nothing on a frame where none of its inputs changed.
fn sync_sprite_gizmos(
    mut commands: Commands,
    selection: Res<Selection>,
    tool: Res<ActiveTool>,
    hosts: Query<(Entity, Ref<Viewport2dPanelHost>)>,
    stages: Query<Ref<ComputedNode>, With<Scene2dViewport>>,
    scene_roots: Scene2dRoots,
    parents: Query<&ChildOf>,
    sprites: Query<(Ref<GlobalTransform>, Ref<Sprite>, Option<&Anchor>), Without<Locked>>,
    mut image_events: MessageReader<AssetEvent<Image>>,
    pixels: SpritePixels,
    gizmos: Query<(Entity, &SpriteGizmo)>,
    mut nodes: Query<(&mut Node, &mut UiTransform, Option<&mut SpriteGizmoHandle>)>,
    manipulation: Res<SpriteManipulation>,
) {
    let images_changed = !image_events.is_empty();
    image_events.clear();
    let primary = selection.primary();
    let moved = primary
        .and_then(|entity| sprites.get(entity).ok())
        .is_some_and(|(global, sprite, _)| global.is_changed() || sprite.is_changed());
    let changed = selection.is_changed()
        || tool.is_changed()
        || moved
        || images_changed
        || hosts.iter().any(|(_, host)| host.is_changed())
        || stages.iter().any(|stage| stage.is_changed())
        || manipulation.is_changed();
    if !changed {
        return;
    }

    let mut wanted = Vec::new();
    for (host_entity, host) in &hosts {
        let Some(primary) = primary else {
            break;
        };
        if host.mode != Viewport2dMode::Edit || !in_2d_scene(primary, &parents, &scene_roots) {
            continue;
        }
        let (Ok(stage), Ok((global, sprite, anchor))) =
            (stages.get(host.stage), sprites.get(primary))
        else {
            continue;
        };
        // Drawn only once the sprite has a size, as its outline is.
        if pixels
            .local_rect(&sprite, anchor.copied().unwrap_or_default())
            .is_none()
        {
            continue;
        }
        let scale = stage_pixels_per_target_pixel(
            target_pixels_per_stage_pixel(stage.size(), host.target_size),
            stage.inverse_scale_factor(),
        );
        let to_stage =
            |world: Vec2| world_to_target_pixel(host.view, host.target_size, world) * scale;
        let mut parts = gizmo_parts(*tool, to_stage(global.translation().xy()));
        for part in &mut parts {
            if let Some((_, at)) = part.handle.as_mut() {
                *at = target_pixel_to_world(host.view, host.target_size, *at / scale);
            }
        }
        if !parts.is_empty() {
            wanted.push(Wanted {
                host: host_entity,
                stage: host.stage,
                sprite: primary,
                parts,
            });
        }
    }

    for (entity, gizmo) in &gizmos {
        let found = wanted.iter().position(|want| {
            want.host == gizmo.host
                && want.sprite == gizmo.sprite
                && gizmo.tool == *tool
                && want.parts.len() == gizmo.parts.len()
        });
        let Some(found) = found else {
            // The handle a running drag holds stays until the drag ends.
            if !manipulation.is_running() {
                commands.entity(entity).despawn();
            }
            continue;
        };
        let want = wanted.swap_remove(found);
        for (part, &part_entity) in want.parts.iter().zip(&gizmo.parts) {
            let Ok((mut node, mut transform, handle)) = nodes.get_mut(part_entity) else {
                continue;
            };
            part.place(&mut node, &mut transform);
            if let Some(mut handle) = handle
                && let Some((_, world)) = part.handle
            {
                handle.world = world;
            }
        }
    }

    for want in wanted {
        spawn_gizmo(&mut commands, want, *tool);
    }
}

fn spawn_gizmo(commands: &mut Commands, want: Wanted, tool: ActiveTool) {
    let root = commands
        .spawn((
            EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            ZIndex(GIZMO_Z),
            Pickable::IGNORE,
            ChildOf(want.stage),
        ))
        .id();
    let parts = want
        .parts
        .iter()
        .map(|part| {
            let mut node = Node {
                position_type: PositionType::Absolute,
                border_radius: if part.round {
                    BorderRadius::MAX
                } else {
                    BorderRadius::ZERO
                },
                ..default()
            };
            let mut transform = UiTransform::default();
            part.place(&mut node, &mut transform);
            let mut entity = commands.spawn((
                EditorEntity,
                node,
                transform,
                BackgroundColor(part.color),
                ChildOf(root),
            ));
            match part.handle {
                Some((kind, world)) => {
                    entity.insert((SpriteGizmoHandle { kind, world }, Pickable::default()));
                }
                None => {
                    entity.insert(Pickable::IGNORE);
                }
            }
            entity.id()
        })
        .collect();
    commands.entity(root).insert(SpriteGizmo {
        host: want.host,
        sprite: want.sprite,
        tool,
        parts,
    });
}

/// Everything needed to tell whether a pointer gesture is a sprite drag.
#[derive(SystemParam)]
struct GrabTargets<'w, 's> {
    handles: Query<'w, 's, (&'static SpriteGizmoHandle, &'static ChildOf)>,
    gizmos: Query<'w, 's, &'static SpriteGizmo>,
    hosts: Query<'w, 's, (Entity, &'static Viewport2dPanelHost)>,
    ui_roots: Query<'w, 's, &'static UiTargetCamera, AuthoredUiSceneRoot>,
    scene_roots: Scene2dRoots<'w, 's>,
    parents: Query<'w, 's, &'static ChildOf>,
    movable: Query<'w, 's, (), (With<Sprite>, Without<Locked>, Without<EditorEntity>)>,
    press: Res<'w, SpritePress>,
    selection: Res<'w, Selection>,
    tool: Res<'w, ActiveTool>,
}

impl GrabTargets<'_, '_> {
    /// The panel and handle a primary-button gesture on `target` drags: a
    /// gizmo handle, or with the select or move tool a 2D world stage whose
    /// last press landed on a selected sprite.
    fn grab(&self, target: Entity, button: PointerButton) -> Option<(Entity, SpriteHandle)> {
        if button != PointerButton::Primary {
            return None;
        }
        if let Ok((handle, parent)) = self.handles.get(target) {
            let host = self.gizmos.get(parent.parent()).ok()?.host;
            return Some((host, handle.kind));
        }
        let (panel, host) = self.hosts.iter().find(|(_, host)| host.stage == target)?;
        let body = matches!(*self.tool, ActiveTool::Select | ActiveTool::Translate)
            && host.mode == Viewport2dMode::Edit
            && shows_2d_world(host, self.ui_roots.iter(), &self.scene_roots)
            && self.press.stage == Some(target)
            && self.press.sprite.is_some_and(|sprite| {
                self.selection.is_selected(sprite)
                    && self.movable.contains(sprite)
                    && in_2d_scene(sprite, &self.parents, &self.scene_roots)
            });
        body.then_some((panel, SpriteHandle::Body))
    }
}

/// A press on a handle is the handle's: it neither picks the sprite under
/// it nor reaches the dock.
fn on_handle_press(mut event: On<PointerPress>, handles: Query<(), With<SpriteGizmoHandle>>) {
    if handles.contains(event.event_target()) {
        event.propagate(false);
    }
}

fn on_drag_start(mut event: On<PointerDragStart>, targets: GrabTargets, mut commands: Commands) {
    let target = event.event_target();
    let Some((host, kind)) = targets.grab(target, event.button) else {
        return;
    };
    event.propagate(false);
    commands.queue(move |world: &mut World| {
        let drag = begin_drag(world, target, host, kind);
        world.resource_mut::<SpriteManipulation>().drag = drag;
    });
}

/// The target a running drag answers to, if one is running.
fn running_target(manipulation: &SpriteManipulation) -> Option<Entity> {
    manipulation.drag.as_ref().map(|drag| drag.target)
}

/// Follow a running drag. Keyed off the drag in progress, so a tool or
/// selection change mid-drag cannot strand it; the very first move can
/// arrive before the drag has begun, which [`GrabTargets`] answers for.
fn on_drag(
    mut event: On<PointerDrag>,
    targets: GrabTargets,
    manipulation: Res<SpriteManipulation>,
    ui_scale: Res<UiScale>,
    mut commands: Commands,
) {
    let target = event.event_target();
    if running_target(&manipulation) != Some(target) && targets.grab(target, event.button).is_none()
    {
        return;
    }
    event.propagate(false);
    let now = event.pointer.position / ui_scale.0;
    let from = (event.pointer.position - event.distance) / ui_scale.0;
    commands.queue(move |world: &mut World| drag_to(world, target, from, now));
}

fn on_drag_end(
    mut event: On<PointerDragEnd>,
    manipulation: Res<SpriteManipulation>,
    mut commands: Commands,
) {
    if running_target(&manipulation) != Some(event.event_target()) {
        return;
    }
    event.propagate(false);
    commands.queue(|world: &mut World| finish_drag(world, true));
}

/// Escape puts back what the drag started from. A drag whose release never
/// reached it (its target gone, the pointer let go elsewhere) ends as it
/// stands.
fn end_sprite_drag_off_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    focus: KeybindFocus,
    mut commands: Commands,
) {
    if !focus.keyboard_is_spoken_for() && keys.just_pressed(KeyCode::Escape) {
        commands.queue(|world: &mut World| finish_drag(world, false));
    } else if !mouse.pressed(MouseButton::Left) {
        commands.queue(|world: &mut World| finish_drag(world, true));
    }
}

/// Put back whatever a sprite drag is in the middle of, recording nothing.
pub(crate) fn cancel_sprite_drag(world: &mut World) {
    finish_drag(world, false);
}

/// Whether `entity` is a sprite of the open 2D world scene that the stage
/// may move.
fn movable_sprite(world: &World, entity: Entity) -> bool {
    if world.get::<Sprite>(entity).is_none()
        || world.get::<Locked>(entity).is_some()
        || world.get::<EditorEntity>(entity).is_some()
    {
        return false;
    }
    let mut current = entity;
    while let Some(parent) = world.get::<ChildOf>(current) {
        current = parent.parent();
        if world.get::<Scene2dRoot>(current).is_some() && world.get::<ChildOf>(current).is_none() {
            return true;
        }
    }
    false
}

/// The sprites of `entities` a drag or nudge takes, each once: movable
/// sprites of the open 2D world scene, less any whose ancestor among them
/// already carries it.
fn movable_items(world: &World, entities: &[Entity]) -> Vec<DragItem> {
    let movable: Vec<Entity> = entities
        .iter()
        .copied()
        .filter(|&entity| movable_sprite(world, entity))
        .collect();
    without_selected_ancestors(world, &movable)
        .into_iter()
        .filter_map(|entity| {
            let start = *world.get::<Transform>(entity)?;
            let to_parent = world
                .get::<ChildOf>(entity)
                .and_then(|parent| world.get::<GlobalTransform>(parent.parent()))
                .map_or(Affine3A::IDENTITY, |parent| parent.affine().inverse());
            Some(DragItem {
                entity,
                start,
                to_parent,
            })
        })
        .collect()
}

fn begin_drag(
    world: &World,
    target: Entity,
    host: Entity,
    kind: SpriteHandle,
) -> Option<SpriteDrag> {
    let items = movable_items(world, &world.get_resource::<Selection>()?.entities);
    if items.is_empty() {
        return None;
    }
    Some(SpriteDrag {
        target,
        host,
        kind,
        started: false,
        items,
    })
}

/// Follow the drag on `target` from the pointer at `from` to `now`, both
/// ui-logical, writing the live transforms.
fn drag_to(world: &mut World, target: Entity, from: Vec2, now: Vec2) {
    let poses = {
        let Some(drag) = world
            .resource::<SpriteManipulation>()
            .drag
            .as_ref()
            .filter(|drag| drag.target == target)
        else {
            return;
        };
        if !drag.started && (now - from).length() <= BOX_SELECT_DRAG_THRESHOLD {
            return;
        }
        let Some(host) = world.get::<Viewport2dPanelHost>(drag.host) else {
            return;
        };
        let Some(computed) = world.get::<ComputedNode>(host.stage) else {
            return;
        };
        let Some(transform) = world.get::<UiGlobalTransform>(host.stage) else {
            return;
        };
        let from = stage_world_point(from, host, computed, transform).0;
        let now = stage_world_point(now, host, computed, transform).0;
        drag.items
            .iter()
            .filter_map(|item| Some((item.entity, drag.pose(item, from, now)?)))
            .collect::<Vec<_>>()
    };
    if let Some(drag) = world.resource_mut::<SpriteManipulation>().drag.as_mut() {
        drag.started = true;
    }
    for (entity, pose) in poses {
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            *transform = pose;
        }
    }
}

/// The part of a world-space drag `delta` a move handle follows. Snapping a
/// move belongs here.
fn move_delta(kind: SpriteHandle, delta: Vec2) -> Vec2 {
    match kind {
        SpriteHandle::AxisX => Vec2::new(delta.x, 0.0),
        SpriteHandle::AxisY => Vec2::new(0.0, delta.y),
        _ => delta,
    }
}

impl SpriteDrag {
    /// `item`'s transform for a pointer that went from `from` to `now`, in
    /// world units.
    fn pose(&self, item: &DragItem, from: Vec2, now: Vec2) -> Option<Transform> {
        match self.kind {
            SpriteHandle::Body | SpriteHandle::AxisX | SpriteHandle::AxisY => {
                let delta = move_delta(self.kind, now - from).extend(0.0);
                Some(Transform {
                    translation: item.start.translation + item.to_parent.transform_vector3(delta),
                    ..item.start
                })
            }
        }
    }
}

/// End the drag: record what it wrote as one history entry, or put back
/// what it started from.
fn finish_drag(world: &mut World, commit: bool) {
    let Some(drag) = world.resource_mut::<SpriteManipulation>().drag.take() else {
        return;
    };
    let mut edits: Vec<Box<dyn EditorCommand>> = Vec::new();
    for item in drag.items {
        let Some(mut transform) = world.get_mut::<Transform>(item.entity) else {
            continue;
        };
        if !commit {
            *transform = item.start;
            continue;
        }
        let now = *transform;
        if now == item.start {
            continue;
        }
        let command = SetTransform {
            entity: item.entity,
            old_transform: item.start,
            new_transform: now,
        };
        command.sync_after_external_execute(world);
        edits.push(Box::new(command));
    }
    if edits.is_empty() {
        return;
    }
    let label = "Move sprites";
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(CommandGroup {
            commands: edits,
            label: label.to_string(),
        }));
}

/// Step the selected sprites of the open 2D world scene along `direction`
/// (screen axes, y down): one world unit, or the 2D grid with Shift held. The
/// same sprites a body drag takes, each offset in its parent's space, as one
/// history entry. Says whether the selection was sprites; `None` (the page
/// keys) moves nothing, as no 2D handle changes a sprite's z.
pub(crate) fn nudge_sprites(world: &mut World, direction: Option<Vec2>) -> bool {
    let primary = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    if !primary.is_some_and(|primary| movable_sprite(world, primary)) {
        return false;
    }
    let Some(direction) = direction else {
        return true;
    };
    let coarse = world
        .get_resource::<ButtonInput<KeyCode>>()
        .is_some_and(|keys| keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]));
    let step = if coarse {
        let mut hosts = world.query::<&Viewport2dPanelHost>();
        hosts
            .iter(world)
            .find(|host| host.mode == Viewport2dMode::Edit)
            .map_or(1.0, |host| host.view.grid)
    } else {
        1.0
    };
    let delta = Vec3::new(direction.x, -direction.y, 0.0) * step;
    let mut edits: Vec<Box<dyn EditorCommand>> = Vec::new();
    let selected = world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();
    for item in movable_items(world, &selected) {
        let mut command = SetTransform {
            entity: item.entity,
            old_transform: item.start,
            new_transform: Transform {
                translation: item.start.translation + item.to_parent.transform_vector3(delta),
                ..item.start
            },
        };
        command.execute(world);
        edits.push(Box::new(command));
    }
    if !edits.is_empty() {
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(CommandGroup {
                commands: edits,
                label: "Nudge".to_string(),
            }));
    }
    true
}
