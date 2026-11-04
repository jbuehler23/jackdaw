//! The brush inspector card the modelling crate registers.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_editor_core::operator_calls::call_operator;
use serde_json::json;

fn texts(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut query = world.query::<&Text>();
    query.iter(world).map(|text| text.0.clone()).collect()
}

#[test]
fn a_selected_brush_shows_its_card_and_clears_its_materials() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw_editor_core::layout::inspector_components_content(
            default(),
        ));
    app.update();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    let answer = app
        .world_mut()
        .run_system_cached_with(
            call_operator,
            Some(json!({ "id": "entity.add.cube", "params": { "name": "Block" } })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("refused: {}", err.message));
    assert_eq!(answer["result"], json!("finished"), "{answer}");
    for _ in 0..4 {
        app.update();
    }

    let shown = texts(&mut app);
    for caption in ["Topology", "Materials & Textures", "Vertices", "Polygons"] {
        assert_eq!(
            shown.iter().filter(|text| *text == caption).count(),
            1,
            "{caption} in {shown:?}"
        );
    }
    assert!(
        shown
            .iter()
            .any(|text| text == "6 faces, 8 vertices, 12 edges"),
        "{shown:?}"
    );

    app.world_mut()
        .operator("brush.clear_all_materials")
        .call()
        .expect("brush.clear_all_materials dispatches")
        .assert_finished();
}
