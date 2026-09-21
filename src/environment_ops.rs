//! Operators for the settings a whole scene shares, starting with its wind.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::Wind;

use crate::commands::EditorCommand;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EnvironmentWindOp>();
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
