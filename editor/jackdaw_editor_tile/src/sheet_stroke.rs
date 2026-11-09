use bevy::picking::events::{
    PointerCancel, PointerDragEnd, PointerMove, PointerPress, PointerRelease,
};
use bevy::picking::pointer::{PointerButton, PointerId, PointerPressState};
use bevy::prelude::*;
use bevy::window::WindowFocused;
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{
    DockAction, DockCall, DockControl, DockView, TilesetDockViews, dispatch, dock_tileset, refill,
};
use crate::tileset_modes::TilesetModes;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<SheetStroke>()
        .add_observer(on_press)
        .add_observer(on_move)
        .add_observer(
            |end: On<PointerRelease>, stroke: Res<SheetStroke>, mut commands: Commands| {
                end_stroke(end.button, &stroke, &mut commands);
            },
        )
        .add_observer(
            |end: On<PointerDragEnd>, stroke: Res<SheetStroke>, mut commands: Commands| {
                end_stroke(end.button, &stroke, &mut commands);
            },
        )
        .add_systems(
            Update,
            cancel_on_interruption.run_if(|stroke: Res<SheetStroke>| stroke.0.is_some()),
        );
}

/// Where a stroke over a sheet starts.
pub struct StrokeStart<'a> {
    pub root: Entity,
    pub path: &'a str,
    pub tileset: &'a Tileset,
    pub view: &'a DockView,
    pub sheet: u32,
    pub button: PointerButton,
}

/// The dock, sheet and button of a stroke under way, and the tileset as the
/// stroke found it.
pub struct StrokeInfo {
    pub root: Entity,
    pub path: String,
    pub sheet: u32,
    pub button: PointerButton,
    pub tileset: Tileset,
}

/// What a mode paints along a stroke over a sheet: the points the pointer
/// crosses while held, then the operator call that applies it all once it
/// lifts.
pub trait SheetPaint: Send + Sync + 'static {
    /// Points taken along the stroke per tile side crossed.
    fn samples(&self) -> f32;
    /// Paint at `points`, in sheet pixels, and draw the result over `frame`.
    fn paint(&mut self, world: &mut World, stroke: &StrokeInfo, frame: Entity, points: &[Vec2]);
    /// The call that applies the stroke; `None` when it painted nothing.
    fn apply(&self, stroke: &StrokeInfo) -> Option<DockCall>;
    /// Runs once the stroke is applied.
    fn finished(&self, _world: &mut World, _stroke: &StrokeInfo) {}
}

/// Starts a mode's stroke; `None` leaves the press alone.
pub type StrokeStarter = fn(&mut World, &StrokeStart) -> Option<Box<dyn SheetPaint>>;

/// The stroke a pointer is painting on a sheet, applied when it lifts.
#[derive(Resource, Default)]
pub(crate) struct SheetStroke(Option<Active>);

pub(crate) struct Active {
    info: StrokeInfo,
    paint: Box<dyn SheetPaint>,
    /// The last point painted, in sheet pixels.
    last: Vec2,
}

fn end_stroke(button: PointerButton, stroke: &SheetStroke, commands: &mut Commands) {
    if stroke.0.as_ref().is_some_and(|s| s.info.button == button) {
        commands.queue(move |world: &mut World| finish(world, button));
    }
}

/// A cancelled pointer or a window losing focus drops the stroke unapplied.
fn cancel_on_interruption(
    mut focus: MessageReader<WindowFocused>,
    mut cancels: MessageReader<PointerCancel>,
    mut commands: Commands,
) {
    let lost = focus.read().any(|f| !f.focused);
    let cancelled = cancels.read().count() > 0;
    if lost || cancelled {
        commands.queue(|world: &mut World| {
            if let Some(stroke) = world.resource_mut::<SheetStroke>().0.take() {
                refill(world, &stroke.info.path);
            }
        });
    }
}

/// The dock, frame, sheet tile and point in it, from (0, 0) at its top left
/// to (1, 1), of a pointer event on a sheet tile.
type TileHit = (Entity, Entity, u32, u32, Vec2);

fn tile_hit(
    entity: Entity,
    position: Option<Vec3>,
    controls: &Query<(&DockControl, &ChildOf)>,
) -> Option<TileHit> {
    let (control, parent) = controls.get(entity).ok()?;
    let DockAction::Tile(sheet, tile) = control.action else {
        return None;
    };
    let inside = position?.truncate() + 0.5;
    Some((control.root, parent.parent(), sheet, tile, inside))
}

fn on_press(
    press: On<PointerPress>,
    controls: Query<(&DockControl, &ChildOf)>,
    stroke: Res<SheetStroke>,
    mut commands: Commands,
) {
    if stroke.0.is_some() {
        return;
    }
    if let Some(hit) = tile_hit(press.entity, press.hit.position, &controls) {
        let button = press.button;
        commands.queue(move |world: &mut World| begin(world, hit, button));
    }
}

fn on_move(
    moved: On<PointerMove>,
    controls: Query<(&DockControl, &ChildOf)>,
    pointers: Query<(&PointerId, &PointerPressState)>,
    stroke: Res<SheetStroke>,
    mut commands: Commands,
) {
    let Some(button) = stroke.0.as_ref().map(|s| s.info.button) else {
        return;
    };
    let held = (pointers.iter())
        .find(|(id, _)| **id == moved.pointer.id)
        .is_some_and(|(_, state)| match button {
            PointerButton::Primary => state.is_primary_pressed(),
            PointerButton::Secondary => state.is_secondary_pressed(),
            PointerButton::Middle => state.is_middle_pressed(),
        });
    if !held {
        return;
    }
    if let Some(hit) = tile_hit(moved.entity, moved.hit.position, &controls) {
        commands.queue(move |world: &mut World| extend(world, hit));
    }
}

/// Start a stroke of the mode the dock shows, when that mode paints.
fn begin(world: &mut World, hit: TileHit, button: PointerButton) {
    let (root, _, sheet, tile, inside) = hit;
    if button == PointerButton::Middle || world.resource::<SheetStroke>().0.is_some() {
        return;
    }
    let Some((path, tileset)) = dock_tileset(world, root) else {
        return;
    };
    let Some(view) = world.resource::<TilesetDockViews>().0.get(&path).cloned() else {
        return;
    };
    let starter = (world.get_resource::<TilesetModes>())
        .and_then(|modes| modes.get(view.mode))
        .and_then(|mode| mode.stroke);
    let Some((starter, (origin, size))) = starter.zip(tileset.sheet_rect(sheet, tile)) else {
        return;
    };
    let start = StrokeStart {
        root,
        path: &path,
        tileset: &tileset,
        view: &view,
        sheet,
        button,
    };
    let Some(paint) = starter(world, &start) else {
        return;
    };
    let info = StrokeInfo {
        root,
        path,
        sheet,
        button,
        tileset,
    };
    let last = origin.as_vec2() + inside * size.as_vec2();
    world.resource_mut::<SheetStroke>().0 = Some(Active { info, paint, last });
    extend(world, hit);
}

/// Paint the points on the line from the stroke's last point to the hit.
fn extend(world: &mut World, (root, frame, sheet, tile, inside): TileHit) {
    let Some(mut stroke) = world.resource_mut::<SheetStroke>().0.take() else {
        return;
    };
    let rect = (stroke.info.tileset.sheet_rect(sheet, tile))
        .filter(|_| stroke.info.root == root && stroke.info.sheet == sheet);
    if let Some((origin, size)) = rect
        && world.get_entity(frame).is_ok()
    {
        let at = origin.as_vec2() + inside * size.as_vec2();
        let step = size.as_vec2().min_element() / stroke.paint.samples().max(1.0);
        let steps = (stroke.last.distance(at) / step.max(0.5)).ceil().max(1.0) as u32;
        let points: Vec<Vec2> = (0..=steps)
            .map(|i| stroke.last.lerp(at, i as f32 / steps as f32))
            .collect();
        stroke.last = at;
        stroke.paint.paint(world, &stroke.info, frame, &points);
    }
    world.resource_mut::<SheetStroke>().0 = Some(stroke);
}

/// Apply the stroke `button` paints as one edit.
fn finish(world: &mut World, button: PointerButton) {
    let mut strokes = world.resource_mut::<SheetStroke>();
    let Some(stroke) = strokes.0.take_if(|s| s.info.button == button) else {
        return;
    };
    let call = stroke.paint.apply(&stroke.info);
    let Active { info, paint, .. } = stroke;
    dispatch(world, info.root, &info.path, &info.tileset, call);
    paint.finished(world, &info);
}
