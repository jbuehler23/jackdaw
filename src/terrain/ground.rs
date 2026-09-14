//! The ground under a point, and dropping entities onto it.
//!
//! A caller with no viewport cannot see where the surface is, so the height
//! is readable on its own and placing something on it is one call rather
//! than a guess at `y`.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::{CommandGroup, EditorCommand};

use super::TerrainDataStore;
use crate::commands::{CommandHistory, SetTransform};
use crate::selection::Selection;

/// Terrains as a ground query reads them.
pub type TerrainSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        &'static jackdaw_scene_types::Terrain,
        &'static GlobalTransform,
    ),
>;

/// Height of the terrain surface under a world-space XZ point, or `None`
/// when no terrain covers it.
///
/// The point is read in each terrain's own space and the height answered back
/// in world space, so a terrain that stands somewhere other than the origin,
/// or at a scale of its own, reads correctly. The highest surface wins, so a
/// point over two overlapping terrains lands on the one a falling object would
/// meet first.
pub fn height_under(
    terrains: &TerrainSurfaces,
    store: &TerrainDataStore,
    point: Vec2,
) -> Option<f32> {
    terrains
        .iter()
        .filter_map(|(terrain, transform)| {
            let to_local = transform.affine().inverse();
            let local = to_local.transform_point3(Vec3::new(point.x, 0.0, point.y));
            let height = store.heightmap(terrain).map.height_at_local(local.xz())?;
            let surface = transform.transform_point(Vec3::new(local.x, height, local.z));
            Some(surface.y)
        })
        .max_by(f32::total_cmp)
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainHeightOp>()
        .register_operator::<EntitySnapToGroundOp>();
}

/// Report the ground height at a point, for a caller placing something on it.
#[operator(
    id = "terrain.height",
    label = "Terrain Height",
    description = "Report the terrain height under a world-space point.",
    allows_undo = false,
    params(
        x(f64, doc = "World-space X of the point."),
        z(f64, doc = "World-space Z of the point."),
    )
)]
pub(crate) fn terrain_height(
    params: In<OperatorParameters>,
    terrains: TerrainSurfaces,
    store: Res<TerrainDataStore>,
    mut commands: Commands,
) -> OperatorResult {
    let (Some(x), Some(z)) = (params.as_float("x"), params.as_float("z")) else {
        commands.queue(|world: &mut World| {
            warn_caller(world, "terrain.height: give both `x` and `z`");
        });
        return OperatorResult::Cancelled;
    };
    let point = Vec2::new(x as f32, z as f32);
    let Some(height) = height_under(&terrains, &store, point) else {
        commands.queue(move |world: &mut World| {
            warn_caller(world, format!("terrain.height: no terrain covers {x}, {z}"));
        });
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        report_to_caller(world, format!("{height}"));
    });
    OperatorResult::Finished
}

/// Drop entities onto the terrain under them, as one undo entry.
#[operator(
    id = "entity.snap_to_ground",
    label = "Snap To Ground",
    description = "Drop entities onto the terrain under them.",
    allows_undo = false,
    params(
        entity(Entity, doc = "Entity to drop. Defaults to the selection."),
        entities(
            String,
            doc = "Entities to drop, as a comma-separated list of ids. Defaults \
                   to the selection."
        ),
    )
)]
pub(crate) fn entity_snap_to_ground(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    authored: Query<(), Without<crate::EditorEntity>>,
    mut commands: Commands,
) -> OperatorResult {
    let targets = match crate::boot_ops::target_entities(&params, &selection, &authored) {
        Ok(targets) => targets,
        Err(refusal) => {
            commands.queue(move |world: &mut World| {
                warn_caller(world, format!("entity.snap_to_ground: {refusal}"));
            });
            return OperatorResult::Cancelled;
        }
    };
    if targets.is_empty() {
        commands.queue(|world: &mut World| {
            warn_caller(
                world,
                "entity.snap_to_ground: nothing is selected and no entity was named",
            );
        });
        return OperatorResult::Cancelled;
    }
    commands.queue(move |world: &mut World| snap_to_ground(world, &targets));
    OperatorResult::Finished
}

/// Drop each entity onto the ground under it, the whole run as one history
/// entry.
fn snap_to_ground(world: &mut World, targets: &[Entity]) {
    let mut moves: Vec<Box<dyn EditorCommand>> = Vec::new();
    let mut missed = Vec::new();
    for &entity in targets {
        let Some(here) = world
            .get::<GlobalTransform>(entity)
            .map(GlobalTransform::translation)
        else {
            missed.push(entity);
            continue;
        };
        let Some(ground) = world
            .run_system_cached_with(ground_height, here.xz())
            .ok()
            .flatten()
        else {
            missed.push(entity);
            continue;
        };
        let Some(old) = world.get::<Transform>(entity).copied() else {
            missed.push(entity);
            continue;
        };
        let mut new = old;
        new.translation = local_for(world, entity, Vec3::new(here.x, ground, here.z));
        if new.translation == old.translation {
            continue;
        }
        moves.push(Box::new(SetTransform {
            entity,
            old_transform: old,
            new_transform: new,
        }));
    }
    for command in &mut moves {
        command.execute(world);
    }
    let moved = moves.len();
    if !moves.is_empty() {
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(CommandGroup {
                label: "Snap To Ground".to_string(),
                commands: moves,
            }));
    }
    report_to_caller(world, format!("snapped {moved}"));
    if !missed.is_empty() {
        let missed: Vec<String> = missed.iter().map(|e| e.to_bits().to_string()).collect();
        warn_caller(
            world,
            format!(
                "entity.snap_to_ground: no ground under {}",
                missed.join(", ")
            ),
        );
    }
}

/// A world-space point as the entity's own transform spells it, with whatever
/// its parent contributes measured out.
fn local_for(world: &World, entity: Entity, point: Vec3) -> Vec3 {
    world
        .get::<ChildOf>(entity)
        .map(ChildOf::parent)
        .and_then(|parent| world.get::<GlobalTransform>(parent))
        .map_or(point, |parent| {
            parent.affine().inverse().transform_point3(point)
        })
}

/// [`height_under`] as a cached system, for a caller holding a `World`.
fn ground_height(
    point: In<Vec2>,
    terrains: TerrainSurfaces,
    store: Res<TerrainDataStore>,
) -> Option<f32> {
    height_under(&terrains, &store, *point)
}
