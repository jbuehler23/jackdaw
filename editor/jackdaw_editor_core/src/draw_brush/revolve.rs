//! `mesh.add_revolve`: a brush turned from a `[radius, height]` profile
//! around an axis, checked before anything is spawned so a refused call
//! leaves the scene as it was.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::{RevolveAxis, RevolveCaps, RevolveOptions};
use jackdaw_scene_types::{Brush, PropertyValue};
use serde_json::Value;

use super::prism::point_of;
use crate::primitive_ops::{
    MAX_PRIMITIVE_FACES, Placement, add_as_one_entry, float_param, int_param,
};

const DEFAULT_SEGMENTS: usize = 12;
const MAX_SEGMENTS: i64 = 256;

/// What a `mesh.add_revolve` call asked for, checked. Kept in the undo
/// history so a redo builds the same brush again.
struct RevolveRequest {
    brush: Brush,
    placement: Placement,
}

fn read_profile(params: &OperatorParameters) -> Result<Vec<Vec2>, String> {
    let Some(raw) = params.get("profile") else {
        return Err("`profile` is required: a list of [radius, height] points".into());
    };
    let value = match raw {
        PropertyValue::String(text) => serde_json::from_str::<Value>(text)
            .map_err(|_| format!("`profile` is not a JSON list: {text}"))?,
        other => return Err(format!("`profile` must be a list, got {other}")),
    };
    let Value::Array(items) = value else {
        return Err("`profile` must be a list of [radius, height] points".into());
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            point_of(item).ok_or_else(|| {
                format!("`profile` item {index} must be two finite numbers [radius, height]")
            })
        })
        .collect()
}

impl RevolveRequest {
    fn read(world: &World, params: &OperatorParameters) -> Result<Self, String> {
        const KEYS: [&str; 10] = [
            "profile",
            "segments",
            "angle_deg",
            "axis",
            "caps",
            "pos",
            "rot",
            "material",
            "parent",
            "name",
        ];
        if let Some(key) = params.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(format!("this operator takes no `{key}`"));
        }
        let segments = int_param(params, "segments", 3..=MAX_SEGMENTS)?.unwrap_or(DEFAULT_SEGMENTS);
        let angle_deg = float_param(params, "angle_deg", 0.0..=360.0)?.unwrap_or(360.0);
        if angle_deg <= 0.0 {
            return Err("`angle_deg` must be above 0 and at most 360".into());
        }
        let axis = match params.get("axis") {
            None => RevolveAxis::Y,
            Some(_) => params
                .as_str("axis")
                .and_then(RevolveAxis::parse)
                .ok_or("`axis` must be x, y or z")?,
        };
        let caps = match params.get("caps") {
            None => RevolveCaps::Auto,
            Some(_) => params
                .as_str("caps")
                .and_then(RevolveCaps::parse)
                .ok_or("`caps` must be auto or none")?,
        };
        let placement = Placement::read(world, params)?;
        let profile = read_profile(params)?;
        let options = RevolveOptions {
            segments,
            angle_deg,
            axis,
            caps,
            max_faces: MAX_PRIMITIVE_FACES,
        };
        let mut brush =
            Brush::revolve(&profile, &options).map_err(|why| format!("the profile {why}"))?;
        placement.paint(world, &mut brush);
        Ok(Self { brush, placement })
    }
}

/// Add the brush the profile parameters describe, as one undo entry.
/// Refused parameters add nothing and report `invalid`.
pub(crate) fn add_revolved_brush(world: &mut World, params: &OperatorParameters) -> OperatorResult {
    let request = match RevolveRequest::read(world, params) {
        Ok(request) => request,
        Err(why) => {
            warn_caller(world, format!("Add Revolve: {why}"));
            set_outcome(world, Outcome::Invalid);
            return OperatorResult::Cancelled;
        }
    };
    let parent = request.placement.parent;
    add_as_one_entry(world, "Add Revolve".into(), parent, move |world, parent| {
        crate::entity_ops::spawn_brush_under(world, request.brush.clone(), parent, |entity| {
            request.placement.apply(entity, None);
        })
    })
}
