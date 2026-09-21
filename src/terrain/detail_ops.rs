//! Operators for a terrain's detail layers: adding and removing them, the look
//! each one draws with, and the density brush that decides where it grows.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{
    DetailLayer, DetailMesh, Terrain, TerrainChannel, TerrainChannelElement,
};

use super::detail::mark_detail_dirty;
use super::paint::{PaintDomain, SetTerrainChannel, TerrainPaintState};
use super::scatter::resolve_terrain;
use super::stamp_ops::{StampTarget, stamp_target};
use super::store::TerrainDataStore;
use crate::commands::EditorCommand;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainDetailAddOp>()
        .register_operator::<TerrainDetailRemoveOp>()
        .register_operator::<TerrainDetailSelectOp>()
        .register_operator::<TerrainDetailSetOp>()
        .register_operator::<TerrainDetailPaintOp>()
        .register_operator::<TerrainDetailStampOp>();
}

/// The terrain a detail operator addresses, or a warning to the caller naming
/// what was missing.
fn detail_terrain(world: &mut World, params: &OperatorParameters, id: &str) -> Option<Entity> {
    let entity = resolve_terrain(world, params.as_str("terrain"));
    if entity.is_none() {
        warn_caller(
            world,
            format!("{id}: no terrain named; name one with terrain=<Name>"),
        );
    }
    entity
}

/// The layer some text addresses, an index before a name, so that an index
/// still wins over a layer whose name is a number.
fn layer_by_index_or_name(layers: &[DetailLayer], addressed: &str) -> Option<usize> {
    match addressed.parse::<usize>() {
        Ok(index) if index < layers.len() => Some(index),
        _ => layers.iter().position(|layer| layer.name == addressed),
    }
}

/// The layer a `layer=` parameter addresses: an index, a name, or the selected
/// layer when it is left out. A selection past the end falls back to the last.
fn resolve_layer(
    world: &mut World,
    entity: Entity,
    params: &OperatorParameters,
    id: &str,
) -> Option<(usize, DetailLayer)> {
    let layers = world.get::<Terrain>(entity)?.detail.clone();
    let addressed = params
        .as_str("layer")
        .map(str::to_string)
        .or_else(|| params.as_int("layer").map(|index| index.to_string()));
    let index = match addressed {
        Some(addressed) => layer_by_index_or_name(&layers, addressed.trim()),
        None => {
            let selected = world.resource::<TerrainPaintState>().detail_layer;
            (!layers.is_empty()).then(|| selected.min(layers.len() - 1))
        }
    };
    let Some(index) = index else {
        warn_caller(
            world,
            format!("{id}: this terrain has no such detail layer; add one with terrain.detail.add"),
        );
        return None;
    };
    Some((index, layers[index].clone()))
}

/// Add a detail layer to a terrain, minting the density channel it grows from.
/// One undo entry covers both the layer and the channel.
#[operator(
    id = "terrain.detail.add",
    label = "Add Detail Layer",
    description = "Add a ground detail layer to a terrain.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        name(
            String,
            doc = "What to call the layer. Defaults to \"grass\", with a suffix when \
                   another layer answers to it."
        ),
        channel(
            String,
            doc = "Name of the density channel it grows from. Defaults to the layer's name."
        ),
    )
)]
pub(crate) fn terrain_detail_add(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "terrain.detail.add";
    let Some(entity) = detail_terrain(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some(terrain) = world.get::<Terrain>(entity) else {
        return OperatorResult::Cancelled;
    };
    let wanted = params
        .as_str("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| DetailLayer::default().name);
    let name = unique_layer_name(&wanted, &terrain.detail);
    let channel = params
        .as_str("channel")
        .map(str::trim)
        .filter(|channel| !channel.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| name.clone());

    let before = terrain.detail.clone();
    let index = before.len();
    let mut after = before.clone();
    after.push(DetailLayer {
        name: name.clone(),
        density_channel: channel.clone(),
        ..DetailLayer::default()
    });
    let adopted = terrain
        .channels
        .iter()
        .find(|declared| declared.name == channel);
    let adopted_palette = adopted.is_some_and(|declared| !declared.palette.is_empty());
    let missing_channel = adopted.is_none().then(|| channel.clone());
    if adopted_palette {
        warn_caller(
            world,
            format!(
                "{id}: {channel:?} already carries a palette, and a detail brush ramps \
                 its cells toward full cover rather than writing palette values"
            ),
        );
    }

    let span = world.resource_mut::<CommandHistory>().begin_span();
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(SetTerrainDetail {
                entity,
                before,
                after,
            }),
            world,
        );
        if let Some(name) = missing_channel {
            history.execute(Box::new(AddTerrainChannel { entity, name }), world);
        }
    });
    world
        .resource_mut::<CommandHistory>()
        .end_span(span, "Add Detail Layer");
    world.resource_mut::<TerrainPaintState>().detail_layer = index;
    report_to_caller(world, format!("{id}: added layer {index} named {name:?}"));
    OperatorResult::Finished
}

/// `wanted`, or the first `wanted-N` no layer already answers to.
fn unique_layer_name(wanted: &str, layers: &[DetailLayer]) -> String {
    let taken = |candidate: &str| layers.iter().any(|layer| layer.name == candidate);
    if !taken(wanted) {
        return wanted.to_string();
    }
    (1..)
        .map(|n| format!("{wanted}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("the candidate space is unbounded")
}

/// Where the selection lands once the layer at `removed` is gone: it follows
/// the layer it was on, and onto the last layer when that was the removed one.
fn selection_after_removing(selected: usize, removed: usize, layers_left: usize) -> usize {
    match selected {
        selected if selected > removed => selected - 1,
        selected => selected.min(layers_left.saturating_sub(1)),
    }
}

/// Remove one detail layer, leaving the channel it grew from painted.
#[operator(
    id = "terrain.detail.remove",
    label = "Remove Detail Layer",
    description = "Remove a ground detail layer from a terrain.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        layer(
            String,
            doc = "Which layer: its index, or its name when it is not an index. \
                   Defaults to the selected layer."
        ),
    )
)]
pub(crate) fn terrain_detail_remove(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "terrain.detail.remove";
    let Some(entity) = detail_terrain(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some((index, _)) = resolve_layer(world, entity, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some(before) = world.get::<Terrain>(entity).map(|t| t.detail.clone()) else {
        return OperatorResult::Cancelled;
    };
    let mut after = before.clone();
    after.remove(index);
    let layers_left = after.len();

    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(SetTerrainDetail {
                entity,
                before,
                after,
            }),
            world,
        );
    });
    let mut paint = world.resource_mut::<TerrainPaintState>();
    paint.detail_layer = selection_after_removing(paint.detail_layer, index, layers_left);
    OperatorResult::Finished
}

/// Make a layer the one the brush paints and the panel edits.
#[operator(
    id = "terrain.detail.select",
    label = "Select Detail Layer",
    description = "Choose which detail layer the brush and the panel act on.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        layer(
            String,
            doc = "Which layer: its index, or its name when it is not an index."
        ),
    )
)]
pub(crate) fn terrain_detail_select(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "terrain.detail.select";
    let Some(entity) = detail_terrain(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some((index, _)) = resolve_layer(world, entity, &params, id) else {
        return OperatorResult::Cancelled;
    };
    world.resource_mut::<TerrainPaintState>().detail_layer = index;
    OperatorResult::Finished
}

/// Write one field of one detail layer. Values are text: a number, `r,g,b` for
/// a colour, `min,max` for a range, and `x,z` for a direction.
#[operator(
    id = "terrain.detail.set",
    label = "Set Detail Layer",
    description = "Set one field of a terrain's detail layer.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        layer(
            String,
            doc = "Which layer: its index, or its name when it is not an index. \
                   Defaults to the selected layer."
        ),
        field(
            String,
            doc = "Which field: name, density_channel, mesh, height, width, color_base, \
                   color_tip, wind_response, bend, push_strength, push_radius, \
                   density_per_m2, cull_distance or align_to_normal."
        ),
        value(
            String,
            doc = "The value: a number, a name, \"card\" or an assets-relative model path \
                   for mesh, \"min,max\" for a range, or \"r,g,b\" for a colour."
        ),
    )
)]
pub(crate) fn terrain_detail_set(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "terrain.detail.set";
    let Some(field) = params.as_str("field").map(str::to_string) else {
        warn_caller(world, format!("{id}: field= is required"));
        return OperatorResult::Cancelled;
    };
    let Some(value) = params.as_str("value").map(str::to_string) else {
        warn_caller(world, format!("{id}: value= is required"));
        return OperatorResult::Cancelled;
    };
    let Some(entity) = detail_terrain(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some((index, before_layer)) = resolve_layer(world, entity, &params, id) else {
        return OperatorResult::Cancelled;
    };

    let mut after_layer = before_layer.clone();
    if !write_field(&mut after_layer, &field, &value) {
        warn_caller(
            world,
            format!("{id}: {field}= cannot take {value:?}; see the operator's field list"),
        );
        return OperatorResult::Cancelled;
    }
    if after_layer == before_layer {
        return OperatorResult::Finished;
    }
    let undeclared = field == "density_channel"
        && world.get::<Terrain>(entity).is_some_and(|terrain| {
            !terrain
                .channels
                .iter()
                .any(|declared| declared.name == after_layer.density_channel)
        });
    if undeclared {
        warn_caller(
            world,
            format!(
                "{id}: this terrain declares no {:?} channel, so the layer grows nothing \
                 until one is added with terrain.channel.add",
                after_layer.density_channel
            ),
        );
    }
    let Some(before) = world.get::<Terrain>(entity).map(|t| t.detail.clone()) else {
        return OperatorResult::Cancelled;
    };
    let mut after = before.clone();
    after[index] = after_layer;
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(SetTerrainDetail {
                entity,
                before,
                after,
            }),
            world,
        );
    });
    OperatorResult::Finished
}

/// Load the paint brush with one layer's density. Brush state, with no history
/// entry of its own; the stroke it produces records one.
#[operator(
    id = "terrain.detail.paint",
    label = "Paint Detail",
    description = "Point the paint brush at a detail layer's density.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        layer(
            String,
            doc = "Which layer: its index, or its name when it is not an index. \
                   Defaults to the selected layer."
        ),
        opacity(
            f64,
            doc = "How far a cell crosses toward full cover per second at full brush \
                   strength, 0..1. Defaults to what the bar is showing."
        ),
        erase(
            bool,
            doc = "Thin cells out rather than thicken them. Omit to flip the current \
                   direction."
        ),
    )
)]
pub(crate) fn terrain_detail_paint(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "terrain.detail.paint";
    let Some(entity) = detail_terrain(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some((index, _)) = resolve_layer(world, entity, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let opacity = params.as_float("opacity");
    let erase = params.as_bool("erase");
    let mut paint = world.resource_mut::<TerrainPaintState>();
    paint.domain = PaintDomain::Detail;
    paint.detail_layer = index;
    if let Some(opacity) = opacity {
        paint.detail_opacity = (opacity as f32).clamp(0.01, 1.0);
    }
    paint.detail_erase = erase.unwrap_or(!paint.detail_erase);
    OperatorResult::Finished
}

/// Thicken one layer's density once at a named place, as a released stroke
/// would. Coordinates are terrain-local metres.
#[operator(
    id = "terrain.detail.stamp",
    label = "Detail Stamp",
    description = "Apply a detail layer's density brush once at a named place.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        layer(
            String,
            doc = "Which layer: its index, or its name when it is not an index. \
                   Defaults to the selected layer."
        ),
        x(f64, doc = "Terrain-local X of the stamp centre, in metres."),
        z(f64, doc = "Terrain-local Z of the stamp centre, in metres."),
        radius(f64, doc = "Brush radius in metres."),
        opacity(
            f64,
            doc = "How far each cell crosses toward full cover, 0..1. Defaults to 1, \
                   a full-strength stamp."
        ),
        hardness(
            f64,
            doc = "Fraction of the radius held at full strength before the falloff \
                   starts, 0..1. Defaults to 0.5."
        ),
        falloff(
            f64,
            doc = "Edge falloff power: 1 is linear, 2 is quadratic. Defaults to 2."
        ),
        erase(bool, doc = "Thin the cells out rather than thicken them."),
    )
)]
pub(crate) fn terrain_detail_stamp(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "terrain.detail.stamp";
    let Some(entity) = detail_terrain(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some((_, layer)) = resolve_layer(world, entity, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let Some(StampTarget {
        entity: target,
        terrain,
        placement,
        rect,
        falloff,
    }) = stamp_target(world, &params, id)
    else {
        return OperatorResult::Cancelled;
    };
    let opacity = params.as_float("opacity").unwrap_or(1.0) as f32;
    let hardness = params.as_float("hardness").unwrap_or(0.5) as f32;
    let erase = params.as_bool("erase").unwrap_or(false);

    let outcome = world.resource_scope(|world, mut store: Mut<TerrainDataStore>| {
        let Some(mut data) = store.entry_for(&terrain) else {
            warn_caller(world, format!("{id}: the terrain has no document to write"));
            return Stamped::Refused;
        };
        let Some(channel) = data
            .channels()
            .iter()
            .position(|channel| channel.name == layer.density_channel)
        else {
            warn_caller(
                world,
                format!(
                    "{id}: the terrain declares no {:?} channel for this layer to grow from",
                    layer.density_channel
                ),
            );
            return Stamped::Refused;
        };
        let Some(element) = data.channel_mut(channel).map(|channel| channel.element) else {
            warn_caller(world, format!("{id}: the terrain has no document to write"));
            return Stamped::Refused;
        };
        let before = data.channel_values(channel);
        let mut values = before.clone();
        let one_whole_application = 1.0;
        let changed = jackdaw_terrain::apply_density_brush(
            &mut values,
            placement.resolution,
            element,
            placement.center,
            placement.radius_cells,
            hardness,
            falloff,
            opacity,
            one_whole_application,
            erase,
        );
        if changed == 0 {
            return Stamped::Unchanged;
        }
        data.set_channel_values(channel, &values);
        Stamped::Wrote {
            channel,
            before,
            after: values,
        }
    });
    let (channel, old_values, new_values) = match outcome {
        Stamped::Refused => return OperatorResult::Cancelled,
        Stamped::Unchanged => return OperatorResult::Finished,
        Stamped::Wrote {
            channel,
            before,
            after,
        } => (channel, before, after),
    };
    mark_detail_dirty(world, target, rect);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SetTerrainChannel {
            entity: target,
            channel,
            old_values,
            new_values,
            label: "Detail Stamp".to_string(),
        }));
    OperatorResult::Finished
}

/// What one detail stamp left behind.
enum Stamped {
    /// The terrain has no document, or no channel for the layer to grow from.
    Refused,
    /// The brush landed but moved no cell.
    Unchanged,
    Wrote {
        channel: usize,
        before: Vec<u16>,
        after: Vec<u16>,
    },
}

/// A scalar, a pair or a triple parsed out of `text`, or `None` when it is
/// not `N` finite numbers separated by commas.
fn numbers<const N: usize>(text: &str) -> Option<[f32; N]> {
    let mut parsed = [0.0; N];
    let mut parts = text.split(',');
    for slot in parsed.iter_mut() {
        let value: f32 = parts.next()?.trim().parse().ok()?;
        if !value.is_finite() {
            return None;
        }
        *slot = value;
    }
    parts.next().is_none().then_some(parsed)
}

/// Most instances a square metre may be asked for.
const MAX_DENSITY_PER_M2: f32 = 512.0;

/// Furthest detail may be asked to draw, in world units.
const MAX_CULL_DISTANCE: f32 = 1000.0;

/// Where an assets-relative path resolves on disk: the open project's assets
/// directory, or `assets` beside the process.
fn assets_dir() -> PathBuf {
    crate::project::open_project_assets_dir().unwrap_or_else(|| PathBuf::from("assets"))
}

/// The mesh `value` names: `card`, or a model file that is really there.
fn resolve_detail_mesh(value: &str) -> Option<DetailMesh> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("card") {
        return Some(DetailMesh::Card);
    }
    jackdaw_terrain::validate_scatter_asset(value).ok()?;
    assets_dir()
        .join(value)
        .is_file()
        .then(|| DetailMesh::Asset(value.to_string()))
}

/// Write one named field of a layer, reporting whether the name and the value
/// were both understood.
fn write_field(layer: &mut DetailLayer, field: &str, value: &str) -> bool {
    match field {
        "name" | "density_channel" => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return false;
            }
            if field == "name" {
                layer.name = trimmed.to_string();
            } else {
                layer.density_channel = trimmed.to_string();
            }
        }
        "mesh" => match resolve_detail_mesh(value) {
            Some(mesh) => layer.mesh = mesh,
            None => return false,
        },
        "align_to_normal" => match value.trim() {
            "true" => layer.align_to_normal = true,
            "false" => layer.align_to_normal = false,
            _ => return false,
        },
        "height" | "width" => match numbers::<2>(value) {
            Some([a, b]) if a >= 0.0 && b >= 0.0 => {
                let low_end_first = [a.min(b), a.max(b)];
                if field == "height" {
                    layer.height = low_end_first;
                } else {
                    layer.width = low_end_first;
                }
            }
            _ => return false,
        },
        "color_base" | "color_tip" => match numbers::<3>(value) {
            Some(rgb) => {
                let rgb = rgb.map(|channel| channel.clamp(0.0, 1.0));
                if field == "color_base" {
                    layer.color_base = rgb;
                } else {
                    layer.color_tip = rgb;
                }
            }
            None => return false,
        },
        _ => {
            let Some([scalar]) = numbers::<1>(value) else {
                return false;
            };
            match field {
                "wind_response" => layer.wind_response = scalar.max(0.0),
                "bend" => layer.bend = scalar,
                "push_strength" => layer.push_strength = scalar,
                "push_radius" => layer.push_radius = scalar.max(0.0),
                "density_per_m2" => {
                    layer.density_per_m2 = scalar.clamp(0.0, MAX_DENSITY_PER_M2);
                }
                "cull_distance" => layer.cull_distance = scalar.clamp(0.0, MAX_CULL_DISTANCE),
                _ => return false,
            }
        }
    }
    true
}

/// One undo entry for a terrain's detail layers, whatever moved them.
pub struct SetTerrainDetail {
    pub entity: Entity,
    pub before: Vec<DetailLayer>,
    pub after: Vec<DetailLayer>,
}

impl SetTerrainDetail {
    fn apply(&self, world: &mut World, detail: Vec<DetailLayer>) {
        let Some(mut terrain) = world.get_mut::<Terrain>(self.entity) else {
            return;
        };
        let layers_left = detail.len();
        terrain.detail = detail;
        let terrain = terrain.clone();
        let mut paint = world.resource_mut::<TerrainPaintState>();
        paint.detail_layer = paint.detail_layer.min(layers_left.saturating_sub(1));
        crate::commands::sync_component_to_ast(
            world,
            self.entity,
            "jackdaw_scene_types::types::Terrain",
            &terrain,
        );
    }
}

impl EditorCommand for SetTerrainDetail {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, self.after.clone());
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, self.before.clone());
    }

    fn description(&self) -> &str {
        "Detail"
    }
}

/// One undo entry for the density channel `terrain.detail.add` mints. Undo
/// drops the descriptor; the store's reconcile drops the zeroed plane.
struct AddTerrainChannel {
    entity: Entity,
    name: String,
}

impl EditorCommand for AddTerrainChannel {
    fn execute(&mut self, world: &mut World) {
        let Some(mut terrain) = world.get_mut::<Terrain>(self.entity) else {
            return;
        };
        if terrain
            .channels
            .iter()
            .any(|channel| channel.name == self.name)
        {
            return;
        }
        let continuous_coverage = Vec::new();
        terrain.channels.push(TerrainChannel {
            name: self.name.clone(),
            element: TerrainChannelElement::U8,
            palette: continuous_coverage,
        });
        super::channel_ops::commit_channels(world, self.entity);
    }

    fn undo(&mut self, world: &mut World) {
        let Some(mut terrain) = world.get_mut::<Terrain>(self.entity) else {
            return;
        };
        let Some(at) = terrain
            .channels
            .iter()
            .position(|channel| channel.name == self.name)
        else {
            return;
        };
        terrain.channels.remove(at);
        super::channel_ops::commit_channels(world, self.entity);
    }

    fn description(&self) -> &str {
        "Add Detail Density"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scalar_field_takes_a_number_and_refuses_anything_else() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "cull_distance", "80"));
        assert_eq!(layer.cull_distance, 80.0);
        assert!(!write_field(&mut layer, "cull_distance", "far"));
        assert!(!write_field(&mut layer, "cull_distance", "1,2"));
        assert_eq!(layer.cull_distance, 80.0, "a refusal writes nothing");
    }

    #[test]
    fn the_fields_a_frame_pays_for_are_capped() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "density_per_m2", "1e9"));
        assert_eq!(layer.density_per_m2, MAX_DENSITY_PER_M2);
        assert!(write_field(&mut layer, "cull_distance", "1e9"));
        assert_eq!(layer.cull_distance, MAX_CULL_DISTANCE);
        assert!(!write_field(&mut layer, "density_per_m2", "inf"));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let mut layer = DetailLayer::default();
        assert!(!write_field(&mut layer, "colour", "1,0,0"));
        assert!(!write_field(&mut layer, "", "1"));
    }

    #[test]
    fn a_range_is_stored_low_end_first() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "height", "0.9, 0.2"));
        assert_eq!(layer.height, [0.2, 0.9]);
        assert!(write_field(&mut layer, "width", "0.4,0.1"));
        assert_eq!(layer.width, [0.1, 0.4]);
        assert!(!write_field(&mut layer, "height", "0.2"));
        assert!(!write_field(&mut layer, "height", "-1,2"));
    }

    #[test]
    fn a_colour_takes_three_channels_and_clamps_them() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "color_tip", "0.5,2.0,-1"));
        assert_eq!(layer.color_tip, [0.5, 1.0, 0.0]);
        assert!(!write_field(&mut layer, "color_base", "0.5,0.5"));
    }

    #[test]
    fn the_name_and_the_density_channel_take_a_name_but_not_an_empty_one() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "name", " Meadow "));
        assert_eq!(layer.name, "Meadow");
        assert!(write_field(&mut layer, "density_channel", " meadow "));
        assert_eq!(layer.density_channel, "meadow");
        assert!(!write_field(&mut layer, "name", "  "));
    }

    #[test]
    fn the_mesh_takes_the_card_and_refuses_a_file_that_is_not_there() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "mesh", "card"));
        assert_eq!(layer.mesh, DetailMesh::Card);
        assert!(!write_field(&mut layer, "mesh", "models/absent.gltf"));
        assert!(!write_field(&mut layer, "mesh", "../outside.gltf"));
        assert!(!write_field(&mut layer, "mesh", "models/notamodel.txt"));
        assert_eq!(layer.mesh, DetailMesh::Card, "a refusal writes nothing");
    }

    #[test]
    fn aligning_to_the_normal_takes_only_a_bool() {
        let mut layer = DetailLayer::default();
        assert!(write_field(&mut layer, "align_to_normal", "true"));
        assert!(layer.align_to_normal);
        assert!(!write_field(&mut layer, "align_to_normal", "yes"));
        assert!(layer.align_to_normal);
    }
}
