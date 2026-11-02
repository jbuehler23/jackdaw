//! The parameters the `entity.add.*` primitives take: size baked into the
//! brush, shape counts, placement, material, parent and name, checked before
//! anything is spawned so a refused call leaves the scene as it was.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::EditorCommand;
use jackdaw_scene_types::{Brush, PropertyValue};

use crate::commands::{CommandHistory, SpawnEntity};
use crate::entity_ops::EntityTemplate;

const COMMON_KEYS: [&str; 6] = ["size", "pos", "rot", "material", "parent", "name"];

/// The most faces one primitive call may build, whatever its counts. A uv
/// sphere of 64 by 32 is the largest that fits.
pub(crate) const MAX_PRIMITIVE_FACES: usize = 2048;

/// The shape parameters `template` takes beside the common ones.
fn shape_keys(template: EntityTemplate) -> &'static [&'static str] {
    match template {
        EntityTemplate::Cylinder => &["segments", "radius_top"],
        EntityTemplate::Cone => &["segments"],
        EntityTemplate::Sphere => &["kind", "segments", "rings", "subdivisions"],
        EntityTemplate::Pyramid => &["sides"],
        _ => &[],
    }
}

/// The shape a call asked for beside its size; `None` keeps the default.
#[derive(Clone, Copy, Default)]
struct Shape {
    segments: Option<usize>,
    rings: Option<usize>,
    subdivisions: Option<usize>,
    uv: bool,
    radius_top: Option<f32>,
    sides: Option<usize>,
}

impl Shape {
    fn read(params: &OperatorParameters, template: EntityTemplate) -> Result<Self, String> {
        let allowed = shape_keys(template);
        if let Some(key) = params
            .keys()
            .find(|key| !COMMON_KEYS.contains(&key.as_str()) && !allowed.contains(&key.as_str()))
        {
            return Err(format!("this primitive takes no `{key}`"));
        }
        let mut shape = Self {
            segments: int_param(params, "segments", 3..=256)?,
            rings: int_param(params, "rings", 2..=128)?,
            subdivisions: int_param(params, "subdivisions", 0..=4)?,
            sides: int_param(params, "sides", 3..=256)?,
            ..default()
        };
        if template == EntityTemplate::Sphere {
            shape.uv = match params.get("kind").map(|_| params.as_str("kind")) {
                None => shape.segments.is_some() || shape.rings.is_some(),
                Some(Some("ico")) => false,
                Some(Some("uv")) => true,
                Some(_) => return Err("`kind` must be uv or ico".into()),
            };
            if !shape.uv && (shape.segments.is_some() || shape.rings.is_some()) {
                return Err("an ico sphere takes `subdivisions`, not `segments` or `rings`".into());
            }
            if shape.uv && shape.subdivisions.is_some() {
                return Err("a uv sphere takes `segments` and `rings`, not `subdivisions`".into());
            }
        }
        if params.contains_key("radius_top") {
            match params.as_float("radius_top") {
                Some(radius) if radius.is_finite() && radius >= 0.0 => {
                    shape.radius_top = Some(radius as f32);
                }
                _ => return Err("`radius_top` must be a number of zero or more".into()),
            }
        }
        let faces = shape.face_count(template);
        if faces > MAX_PRIMITIVE_FACES {
            return Err(format!(
                "that makes {faces} faces, more than the {MAX_PRIMITIVE_FACES} one primitive may have"
            ));
        }
        Ok(shape)
    }

    fn is_default(&self) -> bool {
        !self.uv
            && self.segments.is_none()
            && self.rings.is_none()
            && self.subdivisions.is_none()
            && self.radius_top.is_none()
            && self.sides.is_none()
    }

    fn uv_counts(&self) -> (usize, usize) {
        let segments = self.segments.unwrap_or(8);
        (segments, self.rings.unwrap_or((segments / 2).max(2)))
    }

    fn face_count(&self, template: EntityTemplate) -> usize {
        match template {
            EntityTemplate::Cylinder => self.segments.unwrap_or(16) + 2,
            EntityTemplate::Cone => self.segments.unwrap_or(16) + 1,
            EntityTemplate::Pyramid => self.sides.unwrap_or(4) + 1,
            EntityTemplate::Sphere if self.uv => {
                let (segments, rings) = self.uv_counts();
                segments * rings
            }
            EntityTemplate::Sphere => 20 << (2 * self.subdivisions.unwrap_or(0)),
            EntityTemplate::Wedge => 5,
            _ => 6,
        }
    }

    /// The primitive in a unit box, `width` being the x size it is then
    /// stretched to.
    fn base_brush(&self, template: EntityTemplate, width: f32) -> Brush {
        let sides = self.segments.unwrap_or(16);
        match template {
            EntityTemplate::Plane => Brush::plane(0.5, 0.5),
            EntityTemplate::Cylinder => match self.radius_top {
                Some(top) => Brush::frustum(0.5, top / width, 0.5, sides),
                None => Brush::cylinder(0.5, 0.5, sides),
            },
            EntityTemplate::Cone => Brush::cone(0.5, 0.5, sides),
            EntityTemplate::Wedge => Brush::wedge(0.5, 0.5, 0.5),
            EntityTemplate::Pyramid => match self.sides {
                Some(sides) => sided_pyramid(sides),
                None => Brush::pyramid(0.5, 0.5, 0.5),
            },
            EntityTemplate::Sphere if self.uv => {
                let (segments, rings) = self.uv_counts();
                Brush::ring_sphere(0.5, segments, rings)
            }
            EntityTemplate::Sphere => Brush::ico_sphere(0.5, self.subdivisions.unwrap_or(0)),
            _ => Brush::cuboid(0.5, 0.5, 0.5),
        }
    }
}

/// A pyramid on a `sides`-gon base stretched to fill the unit box.
fn sided_pyramid(sides: usize) -> Brush {
    let turn = std::f32::consts::FRAC_PI_2 - std::f32::consts::PI / sides as f32;
    let ring: Vec<Vec2> = (0..sides)
        .map(|i| Vec2::from_angle(turn + i as f32 / sides as f32 * std::f32::consts::TAU))
        .collect();
    let min = ring.iter().copied().fold(Vec2::MAX, Vec2::min);
    let max = ring.iter().copied().fold(Vec2::MIN, Vec2::max);
    let base: Vec<Vec3> = ring
        .iter()
        .map(|point| {
            let fitted = (*point - min) / (max - min) - 0.5;
            Vec3::new(fitted.x, -0.5, fitted.y)
        })
        .collect();
    Brush::tapered(&base, Vec3::Y * 0.5).unwrap_or_else(|| Brush::pyramid(0.5, 0.5, 0.5))
}

/// A whole-number parameter within `range`.
fn int_param(
    params: &OperatorParameters,
    key: &str,
    range: std::ops::RangeInclusive<i64>,
) -> Result<Option<usize>, String> {
    if !params.contains_key(key) {
        return Ok(None);
    }
    match params.as_int(key) {
        Some(value) if range.contains(&value) => Ok(Some(value as usize)),
        _ => Err(format!(
            "`{key}` must be a whole number from {} to {}",
            range.start(),
            range.end()
        )),
    }
}

/// A `Vec3` parameter whose every axis is a finite number.
pub(crate) fn vec3_param(params: &OperatorParameters, key: &str) -> Result<Option<Vec3>, String> {
    if !params.contains_key(key) {
        return Ok(None);
    }
    params
        .as_vec3(key)
        .filter(|value| value.is_finite())
        .map(Some)
        .ok_or_else(|| format!("`{key}` must be three finite numbers [x, y, z]"))
}

/// What a primitive call asked for, checked. Small enough to keep in the undo
/// history: a redo builds the brush again from it.
#[derive(Clone)]
struct PrimitiveRequest {
    template: EntityTemplate,
    shape: Shape,
    size: Option<Vec3>,
    transform: Option<Transform>,
    material: Option<Handle<StandardMaterial>>,
    parent: Option<Entity>,
    name: Option<String>,
}

impl PrimitiveRequest {
    fn read(
        world: &World,
        params: &OperatorParameters,
        template: EntityTemplate,
    ) -> Result<Self, String> {
        let shape = Shape::read(params, template)?;
        let size = vec3_param(params, "size")?;
        if let Some(size) = size {
            let checked = match template {
                EntityTemplate::Plane => vec![size.x, size.z],
                _ => size.to_array().to_vec(),
            };
            if checked.iter().any(|axis| *axis <= 0.0) {
                return Err(format!(
                    "every axis of `size` must be above zero, got {size}"
                ));
            }
        }

        let pos = vec3_param(params, "pos")?;
        let rot = vec3_param(params, "rot")?;
        let transform = (pos.is_some() || rot.is_some()).then(|| {
            let degrees = rot.unwrap_or_default();
            Transform::from_translation(pos.unwrap_or_default()).with_rotation(Quat::from_euler(
                EulerRot::YXZ,
                degrees.x.to_radians(),
                degrees.y.to_radians(),
                degrees.z.to_radians(),
            ))
        });

        let material = match params.get("material") {
            None => None,
            Some(_) => Some(standard_material(
                world,
                "material",
                params.as_str("material"),
            )?),
        };

        let parent = match params.get("parent") {
            None => None,
            Some(PropertyValue::Entity(entity)) => Some(scene_node(world, *entity, "parent")?),
            Some(other) => return Err(format!("`parent` {other:?} names no node in this scene")),
        };

        let name = match params.get("name") {
            None => None,
            Some(_) => {
                let name = params.as_str("name").unwrap_or_default().trim();
                if name.is_empty() || name.chars().any(char::is_control) {
                    return Err("`name` must be printable text, not blank".into());
                }
                Some(name.to_owned())
            }
        };

        Ok(Self {
            template,
            shape,
            size,
            transform,
            material,
            parent,
            name,
        })
    }

    /// The brush to put in place of the template's, or `None` when the call
    /// asked nothing of it.
    fn brush(&self, world: &World) -> Option<Brush> {
        if self.size.is_none() && self.shape.is_default() && self.material.is_none() {
            return None;
        }
        let template = self.template;
        let base = self
            .shape
            .base_brush(template, self.size.map_or(1.0, |size| size.x));
        let mut brush = match self.size {
            None => base,
            Some(size) if template == EntityTemplate::Plane => base.scaled(size.with_y(1.0)),
            Some(size) => base.scaled(size),
        };
        let worn = self.material.clone().or_else(|| {
            world
                .get_resource::<crate::brush::LastUsedMaterial>()
                .and_then(|last| last.material.clone())
        });
        if let Some(worn) = worn {
            for face in &mut brush.faces {
                face.material = worn.clone();
            }
        }
        Some(brush)
    }

    fn apply(&self, entity: &mut EntityWorldMut, brush: Option<Brush>) {
        if let Some(brush) = brush {
            entity.insert(brush);
        }
        if let Some(transform) = self.transform {
            entity.insert(transform);
        }
        if let Some(name) = &self.name {
            entity.insert(Name::new(name.clone()));
        }
    }
}

/// The standard material `reference` names, for the `key` parameter.
pub(crate) fn standard_material(
    world: &World,
    key: &str,
    reference: Option<&str>,
) -> Result<Handle<StandardMaterial>, String> {
    let reference = reference.unwrap_or_default();
    let worn = world
        .get_resource::<crate::material_assets::MaterialRegistry>()
        .and_then(|registry| {
            crate::material_assets::worn_of_reference(
                world.get_resource::<crate::asset_index::AssetIndex>(),
                registry,
                reference,
            )
        })
        .ok_or_else(|| format!("`{key}` {reference:?} names no material in this project"))?;
    worn.standard().cloned().ok_or_else(|| {
        format!("`{key}` {reference:?} is not a standard material a brush face takes")
    })
}

/// `entity` when it is a node of the open scene document, else why not;
/// `key` names the parameter it came from.
pub(crate) fn scene_node(world: &World, entity: Entity, key: &str) -> Result<Entity, String> {
    let in_document = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .is_some_and(|doc| doc.ast_for(entity).is_some());
    if world.get_entity(entity).is_err() || !in_document {
        return Err(format!("`{key}` {entity} is not a node of this scene"));
    }
    Ok(entity)
}

/// Add the primitive `template` the parameters describe, as one undo entry.
/// Refused parameters add nothing and report `invalid`.
pub(crate) fn add_primitive(
    world: &mut World,
    params: &OperatorParameters,
    template: EntityTemplate,
) -> OperatorResult {
    let request = match PrimitiveRequest::read(world, params, template) {
        Ok(request) => request,
        Err(why) => {
            warn_caller(world, format!("Add {}: {why}", template.label()));
            set_outcome(world, Outcome::Invalid);
            return OperatorResult::Cancelled;
        }
    };
    let parent = request
        .parent
        .or_else(|| crate::sprite_ops::scene_2d_parent(world, None));
    let spawn = move |world: &mut World| {
        let brush = request.brush(world);
        crate::entity_ops::spawn_shaped_under(world, template, parent, |entity| {
            request.apply(entity, brush);
        })
    };
    let mut command: Box<dyn EditorCommand> =
        Box::new(SpawnEntity::new(spawn, format!("Add {}", template.label())));
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    set_outcome(world, Outcome::Changed);
    OperatorResult::Finished
}
