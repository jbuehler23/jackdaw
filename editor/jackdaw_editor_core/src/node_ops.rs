//! The parameters the `entity.add.*` operators that add something other than
//! a brush take: placement, name and parent as the primitives take them, a
//! light's colour, intensity, reach, cone and aim, a volume's size and an
//! image's path, checked before anything is spawned so a refused call leaves
//! the scene as it was.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::PropertyValue;

use crate::entity_ops::{EntityTemplate, SceneReflectionProbe};
use crate::primitive_ops::{
    Placement, add_as_one_entry, bool_param, float_param, spawn_as_one_entry, vec3_param,
};

const PLACEMENT_KEYS: [&str; 4] = ["pos", "rot", "parent", "name"];

/// The largest light intensity a call may ask for, in lumens or lux.
const MAX_INTENSITY: f64 = 1e9;

/// The image files a reference image reads, as its file picker offers them.
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "ktx2", "bmp", "tga", "webp"];

/// What an add operator spawns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Template(EntityTemplate),
    /// An empty named "Group".
    Group,
    ReflectionProbe,
    Terrain,
    #[cfg(feature = "multiplayer")]
    SpawnPoint,
    Image,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            Self::Template(template) => template.label(),
            Self::Group => "Group",
            Self::ReflectionProbe => "Reflection Probe",
            Self::Terrain => "Terrain",
            #[cfg(feature = "multiplayer")]
            Self::SpawnPoint => "Spawn Point",
            Self::Image => "Image",
        }
    }

    /// The parameters this kind takes beside placement, name and parent.
    fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Template(EntityTemplate::PointLight) => {
                &["color", "intensity", "shadows", "range", "radius"]
            }
            Self::Template(EntityTemplate::SpotLight) => &[
                "color",
                "intensity",
                "shadows",
                "range",
                "radius",
                "cone",
                "target",
            ],
            Self::Template(EntityTemplate::DirectionalLight) => {
                &["color", "intensity", "shadows", "target"]
            }
            Self::Template(EntityTemplate::FogVolume) | Self::ReflectionProbe => &["size"],
            Self::Image => &["path"],
            _ => &[],
        }
    }

    /// Whether this kind spawns facing the origin, and so turns to face it
    /// again when only its position is given.
    fn faces_origin(self) -> bool {
        matches!(
            self,
            Self::Template(EntityTemplate::SpotLight | EntityTemplate::Camera3d)
        )
    }

    /// Whether this kind goes to the top level when no parent is given,
    /// rather than into an open 2D world scene.
    fn stays_top_level(self) -> bool {
        match self {
            Self::ReflectionProbe | Self::Terrain => true,
            #[cfg(feature = "multiplayer")]
            Self::SpawnPoint => true,
            _ => false,
        }
    }

    fn spawn(
        self,
        world: &mut World,
        parent: Option<Entity>,
        path: &str,
        shape: impl FnOnce(&mut EntityWorldMut),
    ) -> Entity {
        let create = |commands: &mut Commands, selection: &mut Selection| {
            let entity = match self {
                Self::Template(template) => {
                    return crate::entity_ops::create_entity(commands, template, selection);
                }
                Self::Group => {
                    let entity = crate::entity_ops::create_entity(
                        commands,
                        EntityTemplate::Empty,
                        selection,
                    );
                    commands.entity(entity).insert(Name::new("Group"));
                    return entity;
                }
                Self::ReflectionProbe => commands
                    .spawn((
                        Name::new("Reflection Probe"),
                        jackdaw_scene_types::ReflectionProbe::default(),
                        SceneReflectionProbe,
                        Transform::from_scale(Vec3::splat(10.0)),
                        Visibility::default(),
                    ))
                    .id(),
                Self::Terrain => crate::terrain::spawn_terrain_entity(commands),
                #[cfg(feature = "multiplayer")]
                Self::SpawnPoint => commands
                    .spawn((
                        Name::new("Spawn Point"),
                        jackdaw_multiplayer::SpawnPoint::default(),
                        Transform::default(),
                        Visibility::default(),
                    ))
                    .id(),
                Self::Image => {
                    return crate::reference_image::spawn_reference_image(
                        commands,
                        path,
                        Vec3::ZERO,
                        selection,
                    );
                }
            };
            selection.select_single(commands, entity);
            entity
        };
        crate::entity_ops::spawn_selected_under(world, parent, create, shape)
    }
}

/// A light's parameters, checked. `None` leaves the template's value.
#[derive(Clone, Default)]
struct LightParams {
    color: Option<Color>,
    intensity: Option<f32>,
    range: Option<f32>,
    radius: Option<f32>,
    shadows: Option<bool>,
    /// Inner and outer half-angles, in radians.
    cone: Option<(f32, f32)>,
    target: Option<Vec3>,
}

impl LightParams {
    fn read(params: &OperatorParameters) -> Result<Self, String> {
        let range = float_param(params, "range", 0.0..=1e6)?;
        if range == Some(0.0) {
            return Err("`range` must be above zero".into());
        }
        Ok(Self {
            color: color_param(params, "color")?,
            intensity: float_param(params, "intensity", 0.0..=MAX_INTENSITY)?,
            range,
            radius: float_param(params, "radius", 0.0..=1e6)?,
            shadows: bool_param(params, "shadows")?,
            cone: cone_param(params)?,
            target: vec3_param(params, "target")?,
        })
    }

    fn apply(&self, entity: &mut EntityWorldMut) {
        if let Some(mut light) = entity.get_mut::<PointLight>() {
            set(&mut light.color, self.color);
            set(&mut light.intensity, self.intensity);
            set(&mut light.range, self.range);
            set(&mut light.radius, self.radius);
            set(&mut light.shadow_maps_enabled, self.shadows);
        }
        if let Some(mut light) = entity.get_mut::<SpotLight>() {
            set(&mut light.color, self.color);
            set(&mut light.intensity, self.intensity);
            set(&mut light.range, self.range);
            set(&mut light.radius, self.radius);
            set(&mut light.shadow_maps_enabled, self.shadows);
            if let Some((inner, outer)) = self.cone {
                light.inner_angle = inner;
                light.outer_angle = outer;
            }
        }
        if let Some(mut light) = entity.get_mut::<DirectionalLight>() {
            set(&mut light.color, self.color);
            set(&mut light.illuminance, self.intensity);
            set(&mut light.shadow_maps_enabled, self.shadows);
        }
    }
}

fn set<T>(field: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *field = value;
    }
}

/// A colour as a hex code, a name, `r,g,b` or `[r, g, b]`, channels sRGB in
/// 0..1.
fn color_param(params: &OperatorParameters, key: &str) -> Result<Option<Color>, String> {
    let color = match params.get(key) {
        None => return Ok(None),
        Some(PropertyValue::Color(color)) => Some(*color),
        Some(_) => params.as_str(key).and_then(|text| {
            crate::typed_values::parse_color(
                text.trim().trim_start_matches('[').trim_end_matches(']'),
            )
        }),
    };
    let in_range = |color: &Color| {
        color
            .to_srgba()
            .to_f32_array()
            .iter()
            .all(|channel| (0.0..=1.0).contains(channel))
    };
    color
        .filter(in_range)
        .map(Some)
        .ok_or_else(|| format!("`{key}` must be a hex code, a colour name or r,g,b in 0..1"))
}

/// `cone` as `[inner, outer]` half-angles in degrees, as radians.
fn cone_param(params: &OperatorParameters) -> Result<Option<(f32, f32)>, String> {
    let pair = match params.get("cone") {
        None => return Ok(None),
        Some(PropertyValue::Vec2(pair)) => Some(*pair),
        Some(_) => params.as_str("cone").and_then(|text| {
            let parts: Vec<f32> = text
                .split([',', '[', ']', ' '])
                .filter(|part| !part.is_empty())
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            match parts[..] {
                [inner, outer] => Some(Vec2::new(inner, outer)),
                _ => None,
            }
        }),
    };
    match pair {
        Some(Vec2 { x: inner, y: outer })
            if inner.is_finite() && 0.0 <= inner && inner <= outer && outer <= 90.0 =>
        {
            Ok(Some((inner.to_radians(), outer.to_radians())))
        }
        _ => Err("`cone` must be [inner, outer] degrees with 0 <= inner <= outer <= 90".into()),
    }
}

/// What an add call asked for, checked. Kept in the undo history so a redo
/// spawns the same node again.
#[derive(Clone)]
struct NodeRequest {
    kind: NodeKind,
    placement: Placement,
    light: LightParams,
    size: Option<Vec3>,
    path: String,
    /// The point the node turns to face once placed.
    aim: Option<Vec3>,
}

impl NodeRequest {
    fn read(world: &World, params: &OperatorParameters, kind: NodeKind) -> Result<Self, String> {
        let allowed = kind.keys();
        if let Some(key) = params
            .keys()
            .find(|key| !PLACEMENT_KEYS.contains(&key.as_str()) && !allowed.contains(&key.as_str()))
        {
            return Err(format!("this takes no `{key}`"));
        }
        let placement = Placement::read(world, params)?;
        let light = LightParams::read(params)?;
        if let Some(target) = light.target {
            if placement.rot.is_some() {
                return Err("give `target` or `rot`, not both".into());
            }
            let spawned_at = match kind {
                NodeKind::Template(template) => {
                    crate::entity_ops::template_transform(template).translation
                }
                _ => Vec3::ZERO,
            };
            let from = placement.pos.unwrap_or(spawned_at);
            if target.distance_squared(from) < 1e-8 {
                return Err("`target` is where the light stands, so it aims nowhere".into());
            }
        }
        let size = vec3_param(params, "size")?;
        if let Some(size) = size
            && size.min_element() <= 0.0
        {
            return Err(format!(
                "every axis of `size` must be above zero, got {size}"
            ));
        }
        let path = match params.get("path") {
            None if kind == NodeKind::Image => {
                return Err("`path` names the image to place".into());
            }
            None => String::new(),
            Some(_) => {
                let path = params.as_str("path").unwrap_or_default().trim();
                let is_image = std::path::Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                    });
                if !is_image {
                    return Err(format!(
                        "`path` {path:?} is not an image file ({})",
                        IMAGE_EXTENSIONS.join(", ")
                    ));
                }
                path.to_owned()
            }
        };
        let faces_origin_from_pos =
            kind.faces_origin() && placement.pos.is_some() && placement.rot.is_none();
        let aim = light.target.or(faces_origin_from_pos.then_some(Vec3::ZERO));
        Ok(Self {
            kind,
            placement,
            light,
            size,
            path,
            aim,
        })
    }

    fn apply(&self, entity: &mut EntityWorldMut) {
        self.placement.apply(entity, None);
        if let Some(size) = self.size
            && let Some(mut transform) = entity.get_mut::<Transform>()
        {
            transform.scale = size;
        }
        self.light.apply(entity);
        if let Some(aim) = self.aim
            && let Some(mut transform) = entity.get_mut::<Transform>()
            && transform.translation.distance_squared(aim) > 1e-8
        {
            transform.look_at(aim, Vec3::Y);
        }
    }
}

/// Add the node `kind` the parameters describe, as one undo entry. Refused
/// parameters add nothing and report `invalid`.
pub(crate) fn add_node(
    world: &mut World,
    params: &OperatorParameters,
    kind: NodeKind,
) -> OperatorResult {
    let request = match NodeRequest::read(world, params, kind) {
        Ok(request) => request,
        Err(why) => {
            warn_caller(world, format!("Add {}: {why}", kind.label()));
            set_outcome(world, Outcome::Invalid);
            return OperatorResult::Cancelled;
        }
    };
    let label = format!("Add {}", kind.label());
    let parent = request.placement.parent;
    let spawn = move |world: &mut World, parent| {
        request
            .kind
            .spawn(world, parent, &request.path, |entity| request.apply(entity))
    };
    if parent.is_none() && kind.stays_top_level() {
        spawn_as_one_entry(world, label, None, spawn);
        set_outcome(world, Outcome::Changed);
        return OperatorResult::Finished;
    }
    add_as_one_entry(world, label, parent, spawn)
}

/// Add a group named `name` as one undo entry, by the rule `entity.add.group`
/// places one, taking the name as given.
pub(crate) fn add_named_group(world: &mut World, name: String) -> Option<Entity> {
    let parent = crate::sprite_ops::scene_2d_parent(world, None);
    let label = format!("Add Group {name}");
    spawn_as_one_entry(world, label, parent, move |world, parent| {
        NodeKind::Group.spawn(world, parent, "", |entity| {
            entity.insert(Name::new(name.clone()));
        })
    })
}
