//! Operators for the settings a whole scene shares: its wind and its environment.

use bevy::prelude::*;
use bevy::reflect::enums::{DynamicEnum, DynamicVariant};
use bevy::reflect::{GetPath, ReflectMut};
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{Environment, PropertyValue, Wind};

use crate::commands::EditorCommand;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EnvironmentWindOp>()
        .register_operator::<EnvironmentSetOp>();
}

/// Set the scene's wind. Every field is optional and the ones left out keep
/// what they were.
#[operator(
    id = "environment.wind",
    label = "Set Wind",
    description = "Set the wind the whole scene blows by.",
    allows_undo = false,
    params(
        direction(f64, doc = "Which way it blows, as a yaw in degrees about +Y from +X."),
        strength(f64, doc = "How hard it blows, as a multiple of a lively breeze."),
        gust(
            f64,
            doc = "How much of the strength arrives in gusts rather than steadily, 0..1."
        ),
        gust_speed(f64, doc = "How fast the pattern travels, in tiles per second."),
        turbulence_scale(f64, doc = "How many metres one tile of the pattern spans."),
    )
)]
pub(crate) fn environment_wind(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let params = params.0;
    let id = "environment.wind";
    let Some((entity, before)) = world
        .query::<(Entity, &Wind)>()
        .iter(world)
        .next()
        .map(|(entity, wind)| (entity, *wind))
    else {
        warn_caller(
            world,
            format!(
                "{id}: this scene has no wind; add one with \
                 component.add type_path=jackdaw_scene_types::types::Wind"
            ),
        );
        return OperatorResult::Cancelled;
    };

    let read = |name: &str| params.as_float(name).map(|value| value as f32);
    let after = Wind {
        direction: read("direction").unwrap_or(before.direction),
        strength: read("strength")
            .map(|strength| strength.max(0.0))
            .unwrap_or(before.strength),
        gust: read("gust")
            .map(|gust| gust.clamp(0.0, 1.0))
            .unwrap_or(before.gust),
        gust_speed: read("gust_speed").unwrap_or(before.gust_speed),
        turbulence_scale: read("turbulence_scale")
            .map(|scale| scale.max(0.01))
            .unwrap_or(before.turbulence_scale),
    };
    if after == before {
        return OperatorResult::Finished;
    }

    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(SetWind {
                entity,
                before,
                after,
            }),
            world,
        );
    });
    OperatorResult::Finished
}

/// One undo entry for the scene's wind.
struct SetWind {
    entity: Entity,
    before: Wind,
    after: Wind,
}

impl SetWind {
    fn apply(&self, world: &mut World, wind: Wind) {
        let Ok(mut node) = world.get_entity_mut(self.entity) else {
            return;
        };
        node.insert(wind);
        crate::commands::sync_component_to_ast(
            world,
            self.entity,
            "jackdaw_scene_types::types::Wind",
            &wind,
        );
    }
}

impl EditorCommand for SetWind {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, self.after);
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, self.before);
    }

    fn description(&self) -> &str {
        "Wind"
    }
}

/// Set fields of the scene's environment, each parameter named by its field path.
#[operator(
    id = "environment.set",
    label = "Set Environment",
    description = "Set fields of the scene's environment by path, such as fog.mode=ExponentialSquared \
                   or sky.zenith=0.2,0.4,0.8. Every field set in one call is one undo entry.",
    allows_undo = false
)]
pub(crate) fn environment_set(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let id = "environment.set";
    let Some((entity, before)) = world
        .query::<(Entity, &Environment)>()
        .iter(world)
        .next()
        .map(|(entity, environment)| (entity, environment.clone()))
    else {
        warn_caller(
            world,
            format!(
                "{id}: this scene has no environment; add one with \
                 component.add type_path=jackdaw_scene_types::environment::Environment"
            ),
        );
        return OperatorResult::Cancelled;
    };

    let mut after = before.clone();
    for (path, value) in &params.0.0 {
        if let Err(problem) = set_environment_field(&mut after, path, value) {
            warn_caller(world, format!("{id}: {path}: {problem}"));
            return OperatorResult::Cancelled;
        }
    }
    if after == before {
        return OperatorResult::Finished;
    }

    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(SetEnvironment {
                entity,
                before,
                after,
            }),
            world,
        );
    });
    OperatorResult::Finished
}

/// Write one value into the field at `path`, converting it to the field's type.
fn set_environment_field(
    environment: &mut Environment,
    path: &str,
    value: &PropertyValue,
) -> Result<(), String> {
    let field = environment
        .reflect_path_mut(path)
        .map_err(|_| "there is no such field".to_string())?;

    if let Some(number) = field.try_downcast_mut::<f32>() {
        *number = match value {
            PropertyValue::Float(float) => *float as f32,
            PropertyValue::Int(int) => *int as f32,
            PropertyValue::String(text) => text
                .trim()
                .parse()
                .map_err(|_| format!("{text} is not a number"))?,
            _ => return Err("expects a number".to_string()),
        };
        return Ok(());
    }
    if let Some(flag) = field.try_downcast_mut::<bool>() {
        *flag = match value {
            PropertyValue::Bool(flag) => *flag,
            PropertyValue::String(text) => text
                .trim()
                .parse()
                .map_err(|_| format!("{text} is not true or false"))?,
            _ => return Err("expects true or false".to_string()),
        };
        return Ok(());
    }
    if let Some(color) = field.try_downcast_mut::<Color>() {
        *color = match value {
            PropertyValue::Color(given) => *given,
            PropertyValue::String(text) => crate::typed_values::parse_color(text)
                .ok_or_else(|| format!("{text} is not a colour"))?,
            _ => return Err("expects a colour such as 0.2,0.4,0.8".to_string()),
        };
        return Ok(());
    }
    if let ReflectMut::Enum(choice) = field.reflect_mut() {
        let PropertyValue::String(name) = value else {
            return Err("expects the name of a choice".to_string());
        };
        let name = name.trim();
        let known = choice
            .get_represented_enum_info()
            .is_some_and(|info| info.contains_variant(name));
        if !known {
            return Err(format!("{name} is not one of its choices"));
        }
        choice.apply(&DynamicEnum::new(name, DynamicVariant::Unit));
        return Ok(());
    }
    Err("is a group; set one of its fields".to_string())
}

/// One undo entry for the scene's environment.
struct SetEnvironment {
    entity: Entity,
    before: Environment,
    after: Environment,
}

impl SetEnvironment {
    fn apply(&self, world: &mut World, environment: &Environment) {
        let Ok(mut node) = world.get_entity_mut(self.entity) else {
            return;
        };
        node.insert(environment.clone());
        crate::commands::sync_component_to_ast(
            world,
            self.entity,
            "jackdaw_scene_types::environment::Environment",
            environment,
        );
    }
}

impl EditorCommand for SetEnvironment {
    fn execute(&mut self, world: &mut World) {
        let after = self.after.clone();
        self.apply(world, &after);
    }

    fn undo(&mut self, world: &mut World) {
        let before = self.before.clone();
        self.apply(world, &before);
    }

    fn description(&self) -> &str {
        "Environment"
    }
}
