//! Operators for the terrain colour layer: what the tint brush is loaded
//! with, a parametric stamp of it, the whole-layer noise wash, and the two
//! surface dials the splat material shades by.
//!
//! The layer itself lives in [`TerrainDataStore`] beside the heights and
//! the control words, so none of this touches the scene document. What
//! does reach a history entry is anything that changes what the terrain
//! draws: [`TerrainTintStampOp`] and [`TerrainTintVariationOp`] push one
//! for the cells they wrote, and the two surface dials push one for the
//! setting they moved. [`TerrainPaintTintOp`] does not, being brush state.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_terrain::sidecar::SurfaceSettings;

use super::paint::SetTerrainColor;
use super::scatter::resolve_terrain;
use super::stamp_ops::{StampTarget, stamp_target};
use super::store::TerrainDataStore;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainPaintTintOp>()
        .register_operator::<TerrainTintStampOp>()
        .register_operator::<TerrainTintVariationOp>()
        .register_operator::<TerrainTintStrengthOp>()
        .register_operator::<TerrainMaterialBlendSharpnessOp>();
}

/// A `0..1` float parameter as an 8-bit colour channel.
fn channel(value: Option<f64>) -> u8 {
    let value = value.unwrap_or(1.0) as f32;
    if value.is_finite() {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        255
    }
}

/// Load the tint brush with a colour.
///
/// Brush state, so it leaves no history entry of its own; the stroke it
/// produces records one. Channels are `0..1` rather than `0..255` because
/// that is what the colour picker and every other colour parameter in the
/// editor speak.
#[operator(
    id = "terrain.paint.tint",
    label = "Tint Colour",
    description = "Choose the colour the tint brush paints.",
    allows_undo = false,
    params(
        r(f64, doc = "Red, 0..1."),
        g(f64, doc = "Green, 0..1."),
        b(f64, doc = "Blue, 0..1."),
    )
)]
pub(crate) fn terrain_paint_tint(
    params: In<OperatorParameters>,
    mut paint: ResMut<super::paint::TerrainPaintState>,
) -> OperatorResult {
    paint.tint_color = [
        channel(params.as_float("r")),
        channel(params.as_float("g")),
        channel(params.as_float("b")),
    ];
    OperatorResult::Finished
}

/// Tint once at a named place, as a released stroke would.
///
/// The parametric twin of the `terrain.paint` modal in its colour domain:
/// the same [`jackdaw_terrain::apply_color_brush`] kernel, applied once,
/// pushing the same history entry. Coordinates are terrain-local metres,
/// as they are for `terrain.sculpt.stamp` and `terrain.paint.stamp`.
#[operator(
    id = "terrain.tint.stamp",
    label = "Tint Stamp",
    description = "Apply the tint brush once at a named place.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        x(f64, doc = "Terrain-local X of the stamp centre, in metres."),
        z(f64, doc = "Terrain-local Z of the stamp centre, in metres."),
        radius(f64, doc = "Brush radius in metres."),
        opacity(
            f64,
            doc = "How far each cell crosses toward the colour, 0..1. Defaults to 1, \
                   a full-strength stamp."
        ),
        r(f64, doc = "Red, 0..1. Defaults to the brush's loaded colour."),
        g(f64, doc = "Green, 0..1. Defaults to the brush's loaded colour."),
        b(f64, doc = "Blue, 0..1. Defaults to the brush's loaded colour."),
        hardness(
            f64,
            doc = "Fraction of the radius held at full strength before the falloff \
                   starts, 0..1. Defaults to 0.5."
        ),
        falloff(
            f64,
            doc = "Edge falloff power: 1 is linear, 2 is quadratic. Defaults to 2."
        ),
    )
)]
pub(crate) fn terrain_tint_stamp(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| run_tint_stamp(world, &params));
    OperatorResult::Finished
}

fn run_tint_stamp(world: &mut World, params: &OperatorParameters) {
    let Some(StampTarget {
        entity: target,
        terrain,
        placement,
        rect,
        falloff,
    }) = stamp_target(world, params, "terrain.tint.stamp")
    else {
        return;
    };
    let loaded = world
        .resource::<super::paint::TerrainPaintState>()
        .tint_color;
    let tint = [
        params.as_float("r").map_or(loaded[0], |v| channel(Some(v))),
        params.as_float("g").map_or(loaded[1], |v| channel(Some(v))),
        params.as_float("b").map_or(loaded[2], |v| channel(Some(v))),
    ];
    let opacity = params.as_float("opacity").unwrap_or(1.0) as f32;
    let hardness = params.as_float("hardness").unwrap_or(0.5) as f32;

    let outcome = world.resource_scope(|world, mut store: Mut<TerrainDataStore>| {
        // Read before the write and scoped to `rect`: the dense buffer
        // `tint_rect_mut` hands back holds only `rect`, so an entry over
        // the whole layer would undo a stamp by whitening every cell
        // tinted anywhere else.
        let old_color = rect.read(&store.tint(&terrain.data_path), placement.resolution);
        let Some(mut color) = store.tint_rect_mut(&terrain, rect) else {
            warn_caller(
                world,
                "terrain.tint.stamp: the terrain has no colour layer to write",
            );
            return None;
        };
        // dt = 1.0: a stamp is one whole application, so `opacity` reads
        // as the amount rather than as a rate.
        let changed = jackdaw_terrain::apply_color_brush(
            &mut color,
            placement.resolution,
            placement.center,
            placement.radius_cells,
            falloff,
            hardness,
            opacity,
            1.0,
            tint,
        );
        if changed == 0 {
            return None;
        }
        drop(color);
        let new_color = rect.read(&store.tint(&terrain.data_path), placement.resolution);
        Some((old_color, new_color))
    });
    let Some((old_color, new_color)) = outcome else {
        return;
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SetTerrainColor::stroke(
            target,
            rect,
            old_color,
            new_color,
            "Tint Stamp".to_string(),
        )));
}

/// Lay low-frequency noise over the whole colour layer.
///
/// What a large field of one texture needs to stop reading flat. The
/// layer is replaced rather than blended into, so the history entry holds
/// the whole of what was there: this is the one tint operation that is
/// not a brush.
#[operator(
    id = "terrain.tint.variation",
    label = "Tint Variation",
    description = "Fill the terrain's colour layer with low-frequency noise around white.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        seed(
            i64,
            doc = "Noise seed. The same seed writes the same layer. Defaults to the \
                   options bar's dial."
        ),
        frequency(
            f64,
            doc = "Noise cycles per cell: small is a broad wash, large is speckle. \
                   Defaults to the options bar's dial."
        ),
        amount(
            f64,
            doc = "How far a channel travels from white, 0..1. Defaults to the \
                   options bar's dial."
        ),
    )
)]
pub(crate) fn terrain_tint_variation(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| run_tint_variation(world, &params));
    OperatorResult::Finished
}

fn run_tint_variation(world: &mut World, params: &OperatorParameters) {
    let Some(target) = resolve_terrain(world, params.as_str("terrain")) else {
        warn_caller(
            world,
            "terrain.tint.variation: no terrain to fill; name one with terrain=<Name>",
        );
        return;
    };
    let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(target).cloned() else {
        return;
    };
    // The options bar's Apply button carries no parameters: the three dials
    // beside it live on the brush state, so an omitted parameter reads
    // whatever the bar is showing rather than a constant the bar disagrees
    // with.
    let dials = {
        let state = world.resource::<super::paint::TerrainPaintState>();
        (
            state.variation_seed,
            state.variation_frequency,
            state.variation_amount,
        )
    };
    let seed = params
        .as_int("seed")
        .map_or(dials.0, |v| v.clamp(0, u32::MAX as i64) as u32);
    let frequency = params.as_float("frequency").map_or(dials.1, |v| v as f32);
    let amount = params.as_float("amount").map_or(dials.2, |v| v as f32);

    let outcome = world.resource_scope(|world, mut store: Mut<TerrainDataStore>| {
        let resolution = store.grid_shape(&terrain).resolution;
        if resolution == 0 {
            warn_caller(
                world,
                "terrain.tint.variation: the terrain has no grid to fill",
            );
            return None;
        }
        let old_color = store.tint(&terrain.data_path).to_vec();
        // The wash covers every cell, so it goes in through the whole-layer
        // view rather than a rect: there is no footprint to save.
        let filled = jackdaw_terrain::color_variation(resolution, seed, frequency, amount);
        let mut color = store.tint_mut(&terrain)?;
        let len = color.len().min(filled.len());
        color[..len].copy_from_slice(&filled[..len]);
        drop(color);
        let new_color = store.tint(&terrain.data_path).to_vec();
        (old_color != new_color).then_some((old_color, new_color))
    });
    let Some((old_color, new_color)) = outcome else {
        return;
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SetTerrainColor::whole(
            target,
            old_color,
            new_color,
            "Tint Variation".to_string(),
        )));
}

/// Set how much of a terrain's colour layer reaches its albedo.
///
/// A surface dial rather than a slot one: it scales the whole layer, so
/// turning it to 0 puts the ground back to what its textures alone say
/// without touching a painted cell.
#[operator(
    id = "terrain.tint.strength",
    label = "Tint Strength",
    description = "Set how much of a terrain's colour layer reaches its albedo.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        value(f64, doc = "Tint strength, 0 (off) to 1."),
    )
)]
pub(crate) fn terrain_tint_strength(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| {
        set_surface(
            world,
            &params,
            "terrain.tint.strength",
            "Tint Strength",
            |surface, value| surface.tint_strength = value,
        );
    });
    OperatorResult::Finished
}

/// Set how hard a terrain's height blend cuts between two texture ids.
///
/// Was a constant in the splat material; it is authored per terrain now,
/// and lives in the sidecar's surface block beside the tint strength.
#[operator(
    id = "terrain.material.blend_sharpness",
    label = "Terrain Blend Sharpness",
    description = "Set how hard a terrain's textures cut between one another.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        value(f64, doc = "Blend sharpness, 0 (soft cross-fade) to 1 (near-binary)."),
    )
)]
pub(crate) fn terrain_material_blend_sharpness(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| {
        set_surface(
            world,
            &params,
            "terrain.material.blend_sharpness",
            "Terrain Blend Sharpness",
            |surface, value| surface.blend_sharpness = value,
        );
    });
    OperatorResult::Finished
}

/// Resolve a terrain, apply `edit` to its surface settings, and record the
/// change so a dial can be taken back like any other edit.
///
/// The store sanitizes what it is handed and refuses a quarantined
/// terrain rather than fabricating a document for it; either refusal
/// reaches the caller rather than only a terminal.
fn set_surface(
    world: &mut World,
    params: &OperatorParameters,
    id: &str,
    label: &str,
    edit: impl FnOnce(&mut SurfaceSettings, f32),
) {
    let Some(target) = resolve_terrain(world, params.as_str("terrain")) else {
        warn_caller(
            world,
            format!("{id}: no terrain to set; name one with terrain=<Name>"),
        );
        return;
    };
    let Some(data_path) = world
        .get::<jackdaw_scene_types::Terrain>(target)
        .map(|terrain| terrain.data_path.clone())
    else {
        return;
    };
    let Some(value) = params.as_float("value") else {
        warn_caller(world, format!("{id}: value= is required"));
        return;
    };

    let old = world.resource::<TerrainDataStore>().surface(&data_path);
    let mut new = old;
    edit(&mut new, value as f32);
    let new = new.sanitized();
    if old == new {
        return;
    }
    if let Err(err) = world
        .resource_mut::<TerrainDataStore>()
        .set_surface(data_path.clone(), new)
    {
        warn_caller(
            world,
            format!("{id}: terrain surface settings refused: {err}"),
        );
        return;
    }
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SetSurfaceSettings {
            data_path,
            old,
            new,
            label: label.to_string(),
        }));
}

/// One undo entry for a terrain's surface dials.
pub struct SetSurfaceSettings {
    pub data_path: String,
    pub old: SurfaceSettings,
    pub new: SurfaceSettings,
    pub label: String,
}

impl SetSurfaceSettings {
    fn apply(&self, world: &mut World, settings: SurfaceSettings) {
        // Both directions were accepted when the command was built, so a
        // failure here means the path went load-failed; the store is left
        // as it is.
        let _ = world
            .resource_mut::<TerrainDataStore>()
            .set_surface(self.data_path.clone(), settings);
    }
}

impl jackdaw_commands::EditorCommand for SetSurfaceSettings {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, self.new);
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, self.old);
    }

    fn description(&self) -> &str {
        &self.label
    }
}
