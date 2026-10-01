//! What an added point or spot light starts with.

use crate::util;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};

use crate::util::OperatorResultExt as _;

fn add(app: &mut App, operator: &str) -> Entity {
    app.world_mut()
        .operator(operator.to_string())
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .unwrap_or_else(|err| panic!("{operator}: {err}"))
        .assert_finished();
    app.update();
    *app.world()
        .resource::<jackdaw::selection::Selection>()
        .entities
        .last()
        .unwrap_or_else(|| panic!("{operator} selects what it added"))
}

fn document_text(app: &App) -> String {
    jackdaw_bsn::emit_scene(app.world().resource::<jackdaw_bsn::SceneBsnAst>())
}

#[test]
fn an_added_point_light_casts_no_shadows_and_reaches_ten_metres() {
    let mut app = util::editor_test_app();
    let light = add(&mut app, "entity.add.point_light");

    let point = app.world().get::<PointLight>(light).expect("a point light");
    assert!(!point.shadow_maps_enabled);
    assert_eq!(point.range, 10.0);
    assert!(
        document_text(&app).contains("range: 10.0"),
        "the range is written into the scene, so it survives a reload"
    );
}

#[test]
fn an_added_spot_light_casts_no_shadows_and_reaches_ten_metres() {
    let mut app = util::editor_test_app();
    let light = add(&mut app, "entity.add.spot_light");

    let spot = app.world().get::<SpotLight>(light).expect("a spot light");
    assert!(!spot.shadow_maps_enabled);
    assert_eq!(spot.range, 10.0);
}

#[test]
fn an_added_directional_light_still_casts_shadows() {
    let mut app = util::editor_test_app();
    let light = add(&mut app, "entity.add.directional_light");

    let sun = app
        .world()
        .get::<DirectionalLight>(light)
        .expect("a directional light");
    assert!(sun.shadow_maps_enabled);
}
