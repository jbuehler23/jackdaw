//! One-shot brush stamps: the sculpt and paint brushes, without a drag.
//!
//! `terrain.sculpt` and `terrain.paint` are modal operators driven by the
//! pointer, so a caller with no pointer cannot enter them at all. These are
//! the same kernels applied once at a named place, pushing the same history
//! entry a released stroke pushes.
//!
//! Coordinates are terrain-local metres, not grid cells.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_terrain::{Control, GridRect, SculptTool};

use super::paint::SetTerrainControl;
use super::scatter::resolve_terrain;
use super::sculpt::SetTerrainHeights;
use super::{CHUNK_SIZE, TerrainDirtyChunks};
use crate::terrain::store::TerrainDataStore;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainSculptStampOp>()
        .register_operator::<TerrainPaintStampOp>();
}

/// Where a stamp lands and how wide it is, in grid cells.
pub(crate) struct StampPlacement {
    pub center: Vec2,
    pub radius_cells: f32,
    pub resolution: u32,
}

/// Convert a terrain-local XZ position and a metre radius into the grid space
/// the brush kernels work in.
///
/// The grid is derived from the regions the terrain has allocated, so this
/// asks the store rather than the component: a stamp reaches wherever ground
/// exists, the same rule the pointer strokes follow.
pub(crate) fn place(
    store: &TerrainDataStore,
    terrain: &jackdaw_scene_types::Terrain,
    x: f32,
    z: f32,
    radius: f32,
) -> Option<StampPlacement> {
    let shape = store.grid_shape(terrain);
    if shape.resolution < 2 {
        return None;
    }
    let cell = shape.size / (shape.resolution - 1) as f32;
    if cell.x <= 0.0 || cell.y <= 0.0 {
        return None;
    }
    let center = (Vec2::new(x, z) - shape.origin) / cell;
    let radius_cells = radius / cell.x.max(cell.y);
    (radius_cells > 0.0).then_some(StampPlacement {
        center,
        radius_cells,
        resolution: shape.resolution,
    })
}

/// Everything a stamp needs before it touches a layer: which terrain, where
/// on its grid, and the block of cells the brush will write.
pub(crate) struct StampTarget {
    pub entity: Entity,
    pub terrain: jackdaw_scene_types::Terrain,
    pub placement: StampPlacement,
    pub rect: GridRect,
    pub falloff: f32,
}

/// Resolve the terrain and footprint every stamp operator starts from.
///
/// One preamble rather than three: the stamps differ only in the kernel they
/// run and the layer they write. Whatever step could not be taken is reported
/// to the caller under `id`, a scripted caller having no terminal to read.
pub(crate) fn stamp_target(
    world: &mut World,
    params: &OperatorParameters,
    id: &str,
) -> Option<StampTarget> {
    let Some(entity) = resolve_terrain(world, params.as_str("terrain")) else {
        warn_caller(
            world,
            format!("{id}: no terrain to stamp; name one with terrain=<Name>"),
        );
        return None;
    };
    let terrain = world.get::<jackdaw_scene_types::Terrain>(entity).cloned()?;
    let x = params.as_float("x").unwrap_or(0.0) as f32;
    let z = params.as_float("z").unwrap_or(0.0) as f32;
    let radius = params.as_float("radius").unwrap_or(5.0) as f32;
    let falloff = params.as_float("falloff").unwrap_or(2.0) as f32;

    let Some(placement) = place(world.resource::<TerrainDataStore>(), &terrain, x, z, radius)
    else {
        warn_caller(world, format!("{id}: the terrain has no grid to stamp"));
        return None;
    };
    let Some(rect) = GridRect::brush(
        placement.resolution,
        placement.center,
        placement.radius_cells,
    ) else {
        warn_caller(world, format!("{id}: the stamp falls outside the terrain"));
        return None;
    };
    Some(StampTarget {
        entity,
        terrain,
        placement,
        rect,
        falloff,
    })
}

/// The sculpt tool a `mode=` names.
fn sculpt_tool(mode: &str) -> Option<SculptTool> {
    match mode {
        "raise" => Some(SculptTool::Raise),
        "lower" => Some(SculptTool::Lower),
        "flatten" => Some(SculptTool::Flatten),
        "smooth" => Some(SculptTool::Smooth),
        "noise" => Some(SculptTool::Noise),
        _ => None,
    }
}

/// Sculpt once at a named place, as a released stroke would.
///
/// `strength` is metres at the centre for `raise` and `lower`, and a 0..1
/// blend for `flatten` and `smooth`, which is what the kernel's
/// `strength * falloff * dt` means with a stamp's one-frame `dt`.
#[operator(
    id = "terrain.sculpt.stamp",
    label = "Sculpt Stamp",
    description = "Apply the sculpt brush once at a named place.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        x(f64, doc = "Terrain-local X of the stamp centre, in metres."),
        z(f64, doc = "Terrain-local Z of the stamp centre, in metres."),
        radius(f64, doc = "Brush radius in metres."),
        mode(
            String,
            doc = "raise, lower, flatten, smooth or noise. Defaults to raise."
        ),
        strength(
            f64,
            doc = "Metres at the centre for raise and lower; a 0..1 blend for \
                   flatten and smooth. Defaults to 1."
        ),
        falloff(
            f64,
            doc = "Edge falloff power: 1 is linear, 2 is quadratic. Defaults to 2."
        ),
    )
)]
pub(crate) fn terrain_sculpt_stamp(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| run_sculpt_stamp(world, &params));
    OperatorResult::Finished
}

fn run_sculpt_stamp(world: &mut World, params: &OperatorParameters) {
    let Some(tool) = sculpt_tool(params.as_str("mode").unwrap_or("raise")) else {
        warn_caller(
            world,
            "terrain.sculpt.stamp: mode= must be raise, lower, flatten, smooth or noise",
        );
        return;
    };
    let Some(StampTarget {
        entity: target,
        terrain,
        placement,
        rect,
        falloff,
    }) = stamp_target(world, params, "terrain.sculpt.stamp")
    else {
        return;
    };
    let strength = params.as_float("strength").unwrap_or(1.0) as f32;

    let outcome = world.resource_scope(|_world, mut store: Mut<TerrainDataStore>| {
        let before = rect.read(&store.heights(&terrain.data_path), placement.resolution);
        let step = terrain.quantization.active_height_step();
        // dt = 1.0: a stamp is one whole application, so `strength` reads
        // as the amount rather than as a rate.
        let wrote = store.brush_heights(&terrain, rect, |heights| {
            jackdaw_terrain::apply_brush_at(
                heights,
                placement.resolution,
                tool,
                placement.center,
                placement.radius_cells,
                strength,
                falloff,
                1.0,
                None,
            );
            if let Some(step) = step {
                jackdaw_terrain::quantize_region(
                    heights,
                    placement.resolution,
                    placement.center,
                    placement.radius_cells,
                    step,
                );
            }
        });
        if !wrote {
            return None;
        }
        let after = rect.read(&store.heights(&terrain.data_path), placement.resolution);
        Some((before, after))
    });
    let Some((before, after)) = outcome else {
        return;
    };

    if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(target) {
        for chunk in jackdaw_terrain::affected_chunks_at(
            placement.resolution,
            placement.center,
            placement.radius_cells,
            CHUNK_SIZE,
        ) {
            dirty.dirty.insert(chunk);
        }
    }
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SetTerrainHeights::stroke(
            target,
            rect,
            before,
            after,
            format!("Terrain {tool:?}"),
        )));
}

/// Paint one texture slot once at a named place, as a released stroke would.
#[operator(
    id = "terrain.paint.stamp",
    label = "Paint Stamp",
    description = "Apply the texture brush once at a named place.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        x(f64, doc = "Terrain-local X of the stamp centre, in metres."),
        z(f64, doc = "Terrain-local Z of the stamp centre, in metres."),
        radius(f64, doc = "Brush radius in metres."),
        slot(i64, doc = "Material slot index to paint. Defaults to 0."),
        opacity(
            f64,
            doc = "How far the blend moves, 0..1. Defaults to 1, a full-strength stamp."
        ),
        overlay(
            bool,
            doc = "Paint the slot as the overlay layer rather than the base, \
                   the way holding Ctrl does. Defaults to false."
        ),
        falloff(
            f64,
            doc = "Edge falloff power: 1 is linear, 2 is quadratic. Defaults to 2."
        ),
    )
)]
pub(crate) fn terrain_paint_stamp(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| run_paint_stamp(world, &params));
    OperatorResult::Finished
}

fn run_paint_stamp(world: &mut World, params: &OperatorParameters) {
    let Some(StampTarget {
        entity: target,
        terrain,
        placement,
        rect,
        falloff,
    }) = stamp_target(world, params, "terrain.paint.stamp")
    else {
        return;
    };
    let opacity = params.as_float("opacity").unwrap_or(1.0) as f32;
    let overlay = params.as_bool("overlay").unwrap_or(false);
    let slot = params.as_int("slot").unwrap_or(0);

    let slot_count = world
        .resource::<TerrainDataStore>()
        .materials(&terrain.data_path)
        .len();
    let Ok(texture_id) = u8::try_from(slot) else {
        warn_caller(
            world,
            format!("terrain.paint.stamp: slot={slot} is not a material slot"),
        );
        return;
    };
    if slot_count > 0 && usize::from(texture_id) >= slot_count {
        warn_caller(
            world,
            format!("terrain.paint.stamp: slot={slot} but the terrain has {slot_count} slots"),
        );
        return;
    }

    let outcome = world.resource_scope(|world, mut store: Mut<TerrainDataStore>| {
        // Read before the write and scoped to `rect`: the dense buffer
        // `control_rect_mut` hands back holds only `rect`, so an entry
        // over the whole map would undo a stamp by erasing every cell
        // painted anywhere else.
        let old_control: Vec<Control> =
            rect.read(&store.control(&terrain.data_path), placement.resolution);
        let Some(mut control) = store.control_rect_mut(&terrain, rect) else {
            warn_caller(world, "terrain.paint.stamp: the terrain has no control map");
            return None;
        };
        let changed = jackdaw_terrain::apply_control_brush(
            &mut control,
            placement.resolution,
            placement.center,
            placement.radius_cells,
            falloff,
            opacity,
            1.0,
            overlay,
            texture_id,
        );
        if changed == 0 {
            return None;
        }
        drop(control);
        let new_control = rect.read(&store.control(&terrain.data_path), placement.resolution);
        Some((old_control, new_control))
    });
    let Some((old_control, new_control)) = outcome else {
        return;
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SetTerrainControl::stroke(
            target,
            rect,
            old_control,
            new_control,
            "Paint Texture".to_string(),
        )));
}
