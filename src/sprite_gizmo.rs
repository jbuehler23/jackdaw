//! The move, rotate and scale gizmo for sprites on a 2D world stage, and the
//! drags that run it. The handles are stage UI nodes like the outlines of
//! [`crate::sprite_stage`], so they stay crisp at any zoom and stay out of the
//! panel's render image.
//!
//! Move takes the whole selection; rotate and scale take the primary sprite
//! alone. Rotation turns about the sprite's anchor, and a scale handle holds
//! the opposite side of the sprite's rect still. Each drag lands as one
//! history entry; Escape puts back what the drag started from.

use std::collections::HashSet;

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
use jackdaw_scene_types::{CanvasGuides, Locked, Scene2dRoot, unsorted_global};

use crate::{
    EditorEntity,
    active_tool::ActiveTool,
    canvas_snap::{CanvasSnap, GuideScene, GuideSpace},
    commands::{CommandGroup, CommandHistory, EditorCommand, SetTransform},
    gizmos::MIN_SCALE,
    keybind_focus::KeybindFocus,
    prefab::AuthoredUiSceneRoot,
    selection::Selection,
    snapping::SnapSettings,
    sprite_snap::{SpriteLanding, SpriteSnapRules, SpriteSnapTargets, land},
    sprite_stage::{
        OpenScene2dRoot, Scene2dRoots, SpritePixels, SpritePress, in_2d_scene, shows_2d_world,
        sprite_local_rect, sprite_world_corners, stage_world_point,
    },
    ui_stage::{
        CanvasAxis, EDGE_SNAP_PIXELS, HighlightLine, stage_pixels_per_target_pixel,
        without_selected_ancestors,
    },
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
/// Stage logical pixels from the anchor to the rotate knob.
const KNOB_REACH: f32 = 56.0;
const KNOB_SIZE: f32 = 14.0;
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
    /// The knob that turns the primary sprite about its anchor.
    Rotate,
    /// A corner or edge of the primary sprite's rect, by its side on the
    /// sprite's local x and y: -1, 0 or 1.
    Scale(i8, i8),
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
    /// The primary sprite as the drag found it, for a rotate or scale.
    frame: Option<PrimaryFrame>,
    /// Whether the pointer has gone past the drag threshold; until it has,
    /// nothing moves.
    started: bool,
    items: Vec<DragItem>,
    /// What a move or scale can land on; `None` for a rotate.
    snap: Option<DragSnap>,
    /// Where the last drag event landed, for the snap highlight.
    landing: SpriteLanding,
}

/// What a move or scale can land on, gathered as the drag starts.
enum DragSnap {
    Move {
        /// The world bounds of the dragged sprites whose size is known.
        bounds: Option<Rect>,
        /// Where the grid and pixel are measured from when no size is
        /// known: the primary sprite's anchor if it moves, else the first
        /// dragged sprite's.
        anchor: Vec2,
        targets: SpriteSnapTargets,
    },
    Scale {
        /// The side of the primary's rect the handle pulls, on its local x
        /// and y.
        side: [i8; 2],
        targets: SpriteSnapTargets,
    },
}

/// The primary sprite a rotate turns about and a scale measures against.
struct PrimaryFrame {
    /// Placed as authored, without any y-sort offset.
    global: GlobalTransform,
    /// The sprite's own scale, in its parent's space.
    scale: Vec3,
    /// The sprite's rect in its local frame.
    rect: Rect,
}

struct DragItem {
    entity: Entity,
    start: Transform,
    /// Where the sprite's anchor started, in the world, without any y-sort
    /// offset.
    world: Vec3,
    /// World to the parent's space, without any y-sort offset.
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

/// The parts of the gizmo for `tool` about a sprite anchored at `pivot`.
/// `corners` gives the sprite's rect corners, counter-clockwise from the local
/// bottom-left; only the rotate and scale handles need them. All in stage
/// logical pixels.
fn gizmo_parts(
    tool: ActiveTool,
    pivot: Vec2,
    corners: impl FnOnce() -> Option<[Vec2; 4]>,
) -> Vec<Part> {
    if tool == ActiveTool::Select {
        return Vec::new();
    }
    let mut parts = vec![Part::square(pivot, PIVOT_SIZE, tokens::TEXT_PRIMARY).round()];
    match tool {
        ActiveTool::Select => {}
        ActiveTool::Translate => {
            for (kind, direction, color) in [
                (SpriteHandle::AxisX, Vec2::X, tokens::AXIS_X_COLOR),
                (SpriteHandle::AxisY, Vec2::NEG_Y, tokens::AXIS_Y_COLOR),
            ] {
                let tip = pivot + direction * AXIS_LENGTH;
                parts.push(Part::bar(pivot + direction * AXIS_START, tip, color).grabs(kind, tip));
                parts.push(Part::square(tip, HANDLE_SIZE, color).grabs(kind, tip));
            }
        }
        ActiveTool::Rotate => {
            let Some(corners) = corners() else {
                return Vec::new();
            };
            let up = ((corners[2] + corners[3]) - (corners[0] + corners[1]))
                .try_normalize()
                .unwrap_or(Vec2::NEG_Y);
            let knob = pivot + up * KNOB_REACH;
            parts.push(Part::bar(pivot, knob, tokens::ACCENT_BLUE));
            parts.push(
                Part::square(knob, KNOB_SIZE, tokens::ACCENT_BLUE)
                    .round()
                    .grabs(SpriteHandle::Rotate, knob),
            );
        }
        ActiveTool::Scale => {
            let Some([bottom_left, bottom_right, top_right, top_left]) = corners() else {
                return Vec::new();
            };
            let angle = (bottom_right - bottom_left).to_angle();
            for x in -1..=1_i8 {
                for y in (-1..=1_i8).filter(|&y| x != 0 || y != 0) {
                    let u = f32::from(x + 1) / 2.0;
                    let at = bottom_left
                        .lerp(bottom_right, u)
                        .lerp(top_left.lerp(top_right, u), f32::from(y + 1) / 2.0);
                    let handle = Part::square(at, HANDLE_SIZE, tokens::TEXT_PRIMARY);
                    parts.push(Part { angle, ..handle }.grabs(SpriteHandle::Scale(x, y), at));
                }
            }
        }
    }
    parts
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
        let scale = stage_pixels_per_target_pixel(
            target_pixels_per_stage_pixel(stage.size(), host.target_size),
            stage.inverse_scale_factor(),
        );
        let to_stage =
            |world: Vec2| world_to_target_pixel(host.view, host.target_size, world) * scale;
        let mut parts = gizmo_parts(*tool, to_stage(global.translation().xy()), || {
            pixels
                .world_corners(&sprite, anchor.copied().unwrap_or_default(), &global)
                .map(|corners| corners.map(to_stage))
        });
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

fn on_drag_start(
    mut event: On<PointerDragStart>,
    targets: GrabTargets,
    scene: GuideScene,
    mut commands: Commands,
) {
    let target = event.event_target();
    let Some((host, kind)) = targets.grab(target, event.button) else {
        return;
    };
    event.propagate(false);
    let guides = targets
        .hosts
        .get(host)
        .ok()
        .and_then(|(_, panel)| scene.open(Some(panel)))
        .and_then(|(root, space)| (space == GuideSpace::World).then_some(root));
    commands.queue(move |world: &mut World| {
        let drag = begin_drag(world, target, host, kind, guides);
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
pub(crate) fn movable_sprite(world: &World, entity: Entity) -> bool {
    world.get::<Sprite>(entity).is_some()
        && world.get::<Locked>(entity).is_none()
        && world.get::<EditorEntity>(entity).is_none()
        && in_open_2d_scene(world, entity)
}

/// Whether `entity` sits under the open 2D world scene's root.
pub(crate) fn in_open_2d_scene(world: &World, entity: Entity) -> bool {
    let mut current = entity;
    while let Some(parent) = world.get::<ChildOf>(current) {
        current = parent.parent();
        if world.get::<Scene2dRoot>(current).is_some() && world.get::<ChildOf>(current).is_none() {
            return true;
        }
    }
    false
}

/// The world bounds of `entity`'s sprite, `None` while its size is unknown.
fn sprite_bounds(world: &World, entity: Entity) -> Option<Rect> {
    let corners = sprite_world_corners(
        world.get::<Sprite>(entity)?,
        world.get::<Anchor>(entity).copied().unwrap_or_default(),
        world.get::<GlobalTransform>(entity)?,
        world.get_resource::<Assets<Image>>()?,
        world.get_resource::<Assets<TextureAtlasLayout>>()?,
    )?;
    crate::sprite_stage::bounds_of(corners)
}

/// The lines of the open 2D world scene a drag can land on: the edges and
/// centres of its visible sprites, less the `dragged` ones and everything they
/// carry, and the guides on `guides`, the root the drag's panel draws them for.
fn snap_targets(
    world: &World,
    dragged: &HashSet<Entity>,
    guides: Option<Entity>,
) -> SpriteSnapTargets {
    let mut stack: Vec<Entity> = world
        .try_query_filtered::<Entity, OpenScene2dRoot>()
        .map(|mut roots| roots.iter(world).collect())
        .unwrap_or_default();
    let mut rects = Vec::new();
    while let Some(entity) = stack.pop() {
        if world.get::<EditorEntity>(entity).is_some() {
            continue;
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(
                children
                    .into_iter()
                    .copied()
                    .filter(|child| !dragged.contains(child)),
            );
        }
        let visible = world
            .get::<InheritedVisibility>(entity)
            .is_none_or(|visible| visible.get());
        if visible && let Some(rect) = sprite_bounds(world, entity) {
            rects.push(rect);
        }
    }
    let mut targets = SpriteSnapTargets::from_rects(rects);
    if let Some(guides) = guides.and_then(|root| world.get::<CanvasGuides>(root)) {
        targets.add_guides(guides);
    }
    targets
}

/// What a drag of `kind` on `items` can land on: nothing for a rotate.
fn gather_snap(
    world: &World,
    kind: SpriteHandle,
    items: &[DragItem],
    primary: Option<Entity>,
    guides: Option<Entity>,
) -> Option<DragSnap> {
    let dragged: HashSet<Entity> = items.iter().map(|item| item.entity).collect();
    match kind {
        SpriteHandle::Rotate => None,
        SpriteHandle::Scale(x, y) => Some(DragSnap::Scale {
            side: [x, y],
            targets: snap_targets(world, &dragged, guides),
        }),
        SpriteHandle::Body | SpriteHandle::AxisX | SpriteHandle::AxisY => {
            let moving_primary = primary.filter(|&primary| {
                let mut current = Some(primary);
                while let Some(at) = current {
                    if dragged.contains(&at) {
                        return true;
                    }
                    current = world.get::<ChildOf>(at).map(ChildOf::parent);
                }
                false
            });
            let anchor = moving_primary
                .and_then(|primary| world.get::<GlobalTransform>(primary))
                .map(|global| global.translation().xy())
                .or_else(|| items.first().map(|item| item.world.xy()))?;
            Some(DragSnap::Move {
                bounds: items
                    .iter()
                    .filter_map(|item| sprite_bounds(world, item.entity))
                    .reduce(|a, b| a.union(b)),
                anchor,
                targets: snap_targets(world, &dragged, guides),
            })
        }
    }
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
            let at = unsorted_global(world, entity)?.translation();
            let to_parent = world
                .get::<ChildOf>(entity)
                .and_then(|parent| unsorted_global(world, parent.parent()))
                .map_or(Affine3A::IDENTITY, |parent| parent.affine().inverse());
            Some(DragItem {
                entity,
                start,
                world: at,
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
    guides: Option<Entity>,
) -> Option<SpriteDrag> {
    let selection = world.get_resource::<Selection>()?;
    let items = movable_items(world, &selection.entities);
    if items.is_empty() {
        return None;
    }
    let frame = match kind {
        SpriteHandle::Body | SpriteHandle::AxisX | SpriteHandle::AxisY => None,
        SpriteHandle::Rotate | SpriteHandle::Scale(..) => {
            let primary = selection.primary()?;
            let sprite = world.get::<Sprite>(primary)?;
            let anchor = world.get::<Anchor>(primary).copied().unwrap_or_default();
            Some(PrimaryFrame {
                global: unsorted_global(world, primary)?,
                scale: world.get::<Transform>(primary)?.scale,
                rect: sprite_local_rect(
                    sprite,
                    anchor,
                    world.get_resource::<Assets<Image>>()?,
                    world.get_resource::<Assets<TextureAtlasLayout>>()?,
                )?,
            })
        }
    };
    let snap = gather_snap(world, kind, &items, selection.primary(), guides);
    Some(SpriteDrag {
        target,
        host,
        kind,
        frame,
        started: false,
        items,
        snap,
        landing: SpriteLanding::default(),
    })
}

/// Follow the drag on `target` from the pointer at `from` to `now`, both
/// ui-logical, writing the live transforms.
fn drag_to(world: &mut World, target: Entity, from: Vec2, now: Vec2) {
    let (poses, landing) = {
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
        let keys = world.get_resource::<ButtonInput<KeyCode>>();
        let held = |pair: [KeyCode; 2]| keys.is_some_and(|keys| keys.any_pressed(pair));
        let shift = held([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let ctrl = held([KeyCode::ControlLeft, KeyCode::ControlRight]);
        let snap = world
            .get_resource::<SnapSettings>()
            .map(|snap| snap.0.clone())
            .unwrap_or_default();
        let kinds = world
            .get_resource::<CanvasSnap>()
            .copied()
            .unwrap_or_default();
        // Ctrl inverts the canvas magnet for a move; a scale takes Ctrl for
        // its own increments, so there it only suspends the canvas snapping.
        let magnet = match drag.kind {
            SpriteHandle::Scale(..) => kinds.enabled && !ctrl,
            _ => kinds.magnet(ctrl),
        };
        let ui_scale = world.get_resource::<UiScale>().map_or(1.0, |scale| scale.0);
        let pixel = world
            .resource::<crate::world_units::WorldUnits>()
            .pixel_size();
        let rules = SpriteSnapRules::new(
            &kinds,
            magnet,
            host.view.grid,
            pixel,
            snap_radius(host, computed, ui_scale),
        );
        let mut landing = drag.landing(from, now, shift, &rules);
        if let Some(reached) = drag.scale_reach(now + landing.nudge, shift, ctrl, &snap) {
            landing = landing.keep_reached(reached);
        }
        (
            drag.poses(from, now + landing.nudge, shift, ctrl, &snap),
            landing,
        )
    };
    if let Some(drag) = world.resource_mut::<SpriteManipulation>().drag.as_mut() {
        drag.started = true;
        drag.landing = landing;
    }
    for (entity, pose) in poses {
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            *transform = pose;
        }
    }
}

/// Whether `global` keeps a sprite's rect square to the world: both local
/// axes along world axes and at right angles, so nothing shears it. `Some`
/// with whether its local x runs along the world's y.
fn square_to_world(global: &GlobalTransform) -> Option<bool> {
    let matrix = global.affine().matrix3;
    let x = matrix.x_axis.truncate().normalize_or_zero();
    let y = matrix.y_axis.truncate().normalize_or_zero();
    let flat = |axis: Vec2| axis != Vec2::ZERO && (axis.x.abs() < 1e-4 || axis.y.abs() < 1e-4);
    (flat(x) && flat(y) && x.dot(y).abs() < 1e-4).then_some(x.x.abs() < x.y.abs())
}

/// [`EDGE_SNAP_PIXELS`] in world units on `host`'s stage, read the way the UI
/// canvas reads it: pointer pixels.
pub(crate) fn snap_radius(
    host: &Viewport2dPanelHost,
    computed: &ComputedNode,
    ui_scale: f32,
) -> f32 {
    let stage = stage_pixels_per_target_pixel(
        target_pixels_per_stage_pixel(computed.size(), host.target_size),
        computed.inverse_scale_factor(),
    );
    let pointer_per_world = stage * host.view.zoom * ui_scale;
    if pointer_per_world > 0.0 && pointer_per_world.is_finite() {
        EDGE_SNAP_PIXELS / pointer_per_world
    } else {
        0.0
    }
}

/// The part of a world-space drag `delta` a move handle follows.
fn move_delta(kind: SpriteHandle, delta: Vec2) -> Vec2 {
    match kind {
        SpriteHandle::AxisX => Vec2::new(delta.x, 0.0),
        SpriteHandle::AxisY => Vec2::new(0.0, delta.y),
        _ => delta,
    }
}

/// `value` kept at least [`MIN_SCALE`] from zero, on the side `hint` (else
/// `fallback`) is on, so a scale never collapses and a flip skips over zero.
fn floored(value: f32, hint: f32, fallback: f32) -> f32 {
    if value.abs() >= MIN_SCALE {
        return value;
    }
    let side = [value, hint, fallback]
        .into_iter()
        .find(|side| *side != 0.0)
        .unwrap_or(1.0);
    MIN_SCALE.copysign(side)
}

impl SpriteDrag {
    /// Where the canvas snapping takes a pointer that went from `from` to
    /// `now`: a move lands the selection's bounds (its anchor alone while no
    /// size is known), a scale handle lands the edge or corner it pulls. A
    /// scale snaps only while the sprite is square to the world and not a
    /// Shift corner, where the edge would not follow the pointer.
    fn landing(
        &self,
        from: Vec2,
        now: Vec2,
        shift: bool,
        rules: &SpriteSnapRules,
    ) -> SpriteLanding {
        match &self.snap {
            None => SpriteLanding::default(),
            Some(DragSnap::Move {
                bounds,
                anchor,
                targets,
            }) => {
                let delta = move_delta(self.kind, now - from);
                let axes = BVec2::new(
                    self.kind != SpriteHandle::AxisY,
                    self.kind != SpriteHandle::AxisX,
                );
                match bounds {
                    Some(bounds) => {
                        let moving = Rect {
                            min: bounds.min + delta,
                            max: bounds.max + delta,
                        };
                        land(moving, axes, targets, rules)
                    }
                    None => {
                        let at = *anchor + delta;
                        let point = Rect::from_corners(at, at);
                        land(point, axes, targets, &rules.without_edges())
                    }
                }
            }
            Some(DragSnap::Scale { side, targets }) => {
                let turned = self
                    .frame
                    .as_ref()
                    .and_then(|frame| square_to_world(&frame.global));
                let Some(turned) = turned else {
                    return SpriteLanding::default();
                };
                let (x, y) = (side[0] != 0, side[1] != 0);
                if shift && x && y {
                    return SpriteLanding::default();
                }
                let axes = if turned {
                    BVec2::new(y, x)
                } else {
                    BVec2::new(x, y)
                };
                land(Rect::from_corners(now, now), axes, targets, rules)
            }
        }
    }

    /// Where a scale handle pulled to `now` actually leaves its edge or
    /// corner once the scale is stepped and floored, in the world; `None` for
    /// a drag that is not a scale.
    fn scale_reach(
        &self,
        now: Vec2,
        shift: bool,
        ctrl: bool,
        snap: &jackdaw_snap::SnapSettings,
    ) -> Option<Vec2> {
        let SpriteHandle::Scale(x, y) = self.kind else {
            return None;
        };
        let frame = self.frame.as_ref()?;
        let side = Vec2::new(f32::from(x), f32::from(y));
        let factor = Self::scale_factor(frame, side, now, shift, ctrl, snap)?;
        let (fixed, stretch) = Self::stretch(frame, side, factor);
        let handle = frame.rect.center() + frame.rect.half_size() * side;
        let handle = frame.global.transform_point(handle.extend(0.0)).xy();
        Some(fixed + stretch * (handle - fixed))
    }

    /// Each dragged sprite's transform for a pointer that went from `from` to
    /// `now`, in world units. A rotate or scale is the primary sprite's, and
    /// the rest of the selection turns or scales with it about the same point.
    fn poses(
        &self,
        from: Vec2,
        now: Vec2,
        shift: bool,
        ctrl: bool,
        snap: &jackdaw_snap::SnapSettings,
    ) -> Vec<(Entity, Transform)> {
        match self.kind {
            SpriteHandle::Rotate => self
                .frame
                .as_ref()
                .map(|frame| self.rotated(frame, from, now, ctrl, snap))
                .unwrap_or_default(),
            SpriteHandle::Scale(x, y) => {
                let side = Vec2::new(f32::from(x), f32::from(y));
                self.frame
                    .as_ref()
                    .map(|frame| self.scaled(frame, side, now, shift, ctrl, snap))
                    .unwrap_or_default()
            }
            SpriteHandle::Body | SpriteHandle::AxisX | SpriteHandle::AxisY => {
                let delta = move_delta(self.kind, now - from).extend(0.0);
                self.items
                    .iter()
                    .map(|item| {
                        let translation =
                            item.start.translation + item.to_parent.transform_vector3(delta);
                        (
                            item.entity,
                            Transform {
                                translation,
                                ..item.start
                            },
                        )
                    })
                    .collect()
            }
        }
    }

    /// `item` with its anchor carried to the world point `to`, at the z it
    /// started at.
    fn placed(item: &DragItem, to: Vec2) -> Vec3 {
        item.to_parent
            .transform_point3(to.extend(item.world.z))
            .xy()
            .extend(item.start.translation.z)
    }

    /// Turn the selection about the primary's anchor by the angle the pointer
    /// swept. Ctrl lands the primary's own angle on a rotate increment.
    fn rotated(
        &self,
        frame: &PrimaryFrame,
        from: Vec2,
        now: Vec2,
        ctrl: bool,
        snap: &jackdaw_snap::SnapSettings,
    ) -> Vec<(Entity, Transform)> {
        let pivot = frame.global.translation().xy();
        let swept = (from - pivot).angle_to(now - pivot);
        if !swept.is_finite() {
            return Vec::new();
        }
        let start = frame.global.affine().matrix3.x_axis.truncate().to_angle();
        let angle = snap.snap_rotate_if(start + swept, ctrl) - start;
        let turn = Rot2::radians(angle);
        self.items
            .iter()
            .map(|item| {
                // A mirroring parent turns the other way in its own space.
                let handed = item.to_parent.matrix3.determinant().signum();
                let at = pivot + turn * (item.world.xy() - pivot);
                let pose = Transform {
                    translation: Self::placed(item, at),
                    rotation: Quat::from_rotation_z(angle * handed) * item.start.rotation,
                    ..item.start
                };
                (item.entity, pose)
            })
            .collect()
    }

    /// The primary's scale factor on its local axes for the handle on `side`
    /// pulled to the world point `now`. Shift keeps proportions; Ctrl steps
    /// the axes the handle drives (the shared factor, with Shift). No axis
    /// reaches zero.
    fn scale_factor(
        frame: &PrimaryFrame,
        side: Vec2,
        now: Vec2,
        shift: bool,
        ctrl: bool,
        snap: &jackdaw_snap::SnapSettings,
    ) -> Option<Vec2> {
        let primary = frame.scale;
        let half = frame.rect.half_size() * side;
        let held = frame.rect.center() - half;
        let span = 2.0 * half;
        let local = frame
            .global
            .affine()
            .inverse()
            .transform_point3(now.extend(frame.global.translation().z))
            .xy();
        let reach = local - held;
        let step = |value: f32| snap.snap_scale_vec3_if(Vec3::splat(value), ctrl).x;
        let ratio = |reach: f32, span: f32| if span == 0.0 { 1.0 } else { reach / span };
        let raw = Vec2::new(ratio(reach.x, span.x), ratio(reach.y, span.y));
        let driven = BVec2::new(side.x != 0.0 || shift, side.y != 0.0 || shift);
        let target = if shift {
            let uniform = if side.x != 0.0 && side.y != 0.0 {
                reach.dot(span) / span.length_squared()
            } else if side.x != 0.0 {
                raw.x
            } else {
                raw.y
            };
            primary.xy() * step(uniform)
        } else {
            Vec2::new(step(primary.x * raw.x), step(primary.y * raw.y))
        };
        let pick = |axis: usize| {
            if !driven.test(axis) || primary[axis] == 0.0 {
                return 1.0;
            }
            floored(target[axis], primary[axis] * raw[axis], primary[axis]) / primary[axis]
        };
        let factor = Vec2::new(pick(0), pick(1));
        factor.is_finite().then_some(factor)
    }

    /// The world point the scale about `side`'s opposite side holds still,
    /// and the world-space stretch `factor` makes about it.
    fn stretch(frame: &PrimaryFrame, side: Vec2, factor: Vec2) -> (Vec2, Mat2) {
        let held = frame.rect.center() - frame.rect.half_size() * side;
        let x_axis = frame.global.affine().matrix3.x_axis.truncate();
        let y_axis = frame.global.affine().matrix3.y_axis.truncate();
        let axes = Mat2::from_cols(
            x_axis.try_normalize().unwrap_or(Vec2::X),
            y_axis.try_normalize().unwrap_or(Vec2::Y),
        );
        let stretch = axes * Mat2::from_diagonal(factor) * axes.inverse();
        let fixed = frame.global.transform_point(held.extend(0.0)).xy();
        (fixed, stretch)
    }

    /// Scale the selection by the handle on `side` of the primary's rect
    /// pulled to the world point `now`, about the primary's opposite side.
    fn scaled(
        &self,
        frame: &PrimaryFrame,
        side: Vec2,
        now: Vec2,
        shift: bool,
        ctrl: bool,
        snap: &jackdaw_snap::SnapSettings,
    ) -> Vec<(Entity, Transform)> {
        let Some(factor) = Self::scale_factor(frame, side, now, shift, ctrl, snap) else {
            return Vec::new();
        };
        let (fixed, stretch) = Self::stretch(frame, side, factor);
        self.items
            .iter()
            .map(|item| {
                let at = fixed + stretch * (item.world.xy() - fixed);
                let scale = item.start.scale;
                let pose = Transform {
                    translation: Self::placed(item, at),
                    scale: Vec3::new(
                        floored(scale.x * factor.x, scale.x, scale.x),
                        floored(scale.y * factor.y, scale.y, scale.y),
                        scale.z,
                    ),
                    ..item.start
                };
                (item.entity, pose)
            })
            .collect()
    }
}

/// The lines the running sprite drag is landing on, in its stage's logical
/// pixels, for the canvas's snap highlight.
pub(crate) fn snap_highlight_lines(
    manipulation: &SpriteManipulation,
    hosts: &Query<&Viewport2dPanelHost>,
    stages: &Query<&ComputedNode, With<Scene2dViewport>>,
) -> Vec<HighlightLine> {
    let Some(drag) = manipulation.drag.as_ref().filter(|drag| drag.started) else {
        return Vec::new();
    };
    let Ok(host) = hosts.get(drag.host) else {
        return Vec::new();
    };
    let Ok(stage) = stages.get(host.stage) else {
        return Vec::new();
    };
    if host.mode != Viewport2dMode::Edit {
        return Vec::new();
    }
    let scale = stage_pixels_per_target_pixel(
        target_pixels_per_stage_pixel(stage.size(), host.target_size),
        stage.inverse_scale_factor(),
    );
    let on_stage = |world: Vec2| world_to_target_pixel(host.view, host.target_size, world) * scale;
    let line = |axis: CanvasAxis, at: f32| HighlightLine {
        host: drag.host,
        stage: host.stage,
        axis,
        at,
    };
    let landing = drag.landing;
    landing
        .x
        .map(|x| line(CanvasAxis::Vertical, on_stage(Vec2::new(x, 0.0)).x))
        .into_iter()
        .chain(
            landing
                .y
                .map(|y| line(CanvasAxis::Horizontal, on_stage(Vec2::new(0.0, y)).y)),
        )
        .collect()
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
    let label = match drag.kind {
        SpriteHandle::Rotate => "Rotate sprite",
        SpriteHandle::Scale(..) => "Scale sprite",
        _ => "Move sprites",
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(CommandGroup {
            commands: edits,
            label: label.to_string(),
        }));
}

/// Step the selected sprites of the open 2D world scene along `direction`
/// (screen axes, y down): one sprite pixel at the project's pixels per unit,
/// or the 2D grid with Shift held. The
/// same sprites a body drag takes, each offset in its parent's space, as one
/// history entry. Says whether the selection was sprites; `None` (the page
/// keys) moves nothing here, as on a sprite they step z through
/// [`step_sprites_z`] instead.
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
        world
            .resource::<crate::world_units::WorldUnits>()
            .pixel_size()
    };
    let delta = Vec3::new(direction.x, -direction.y, 0.0) * step;
    move_selected_sprites(world, "Nudge", |item| {
        item.start.translation + item.to_parent.transform_vector3(delta)
    });
    true
}

/// Step the selected sprites of the open 2D world scene `delta` along their
/// own `Transform` z, as one history entry. The same sprites a nudge takes.
pub(crate) fn step_sprites_z(world: &mut World, delta: f32) {
    move_selected_sprites(world, "Step Z", |item| {
        item.start.translation + Vec3::Z * delta
    });
}

/// Move the selected sprites a nudge takes to the translation `moved` gives
/// each, as one history entry under `label`.
fn move_selected_sprites(world: &mut World, label: &str, moved: impl Fn(&DragItem) -> Vec3) {
    let selected = world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();
    let mut edits: Vec<Box<dyn EditorCommand>> = Vec::new();
    for item in movable_items(world, &selected) {
        let mut command = SetTransform {
            entity: item.entity,
            old_transform: item.start,
            new_transform: Transform {
                translation: moved(&item),
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
                label: label.to_owned(),
            }));
    }
}
