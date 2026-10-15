//! The Navmesh tool in the terrain tool palette and on the options bar.

use bevy::prelude::*;
use jackdaw_api::op::Operator as _;
use jackdaw_api::terrain::{TerrainEditMode, TerrainToolOptions};
use jackdaw_api_internal::lifecycle::disable_extension;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_navmesh_editor::{EXTENSION_ID, TerrainNavmeshBakeOp, TerrainToolNavmeshOp};

use crate::util;

fn buttons_calling(app: &mut App, id: &str) -> usize {
    app.world_mut()
        .query::<&ButtonOperatorCall>()
        .iter(app.world())
        .filter(|call| call.id == id)
        .count()
}

fn options_slots(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<TerrainToolOptions>>()
        .iter(app.world())
        .collect()
}

#[test]
fn picking_the_tool_puts_its_fields_on_the_options_bar() {
    let mut app = util::editor_app();
    util::selected_terrain(&mut app);
    assert!(
        buttons_calling(&mut app, TerrainToolNavmeshOp::ID) > 0,
        "the palette offers the tool"
    );

    util::call(&mut app, TerrainToolNavmeshOp::ID);
    assert_eq!(
        *app.world().resource::<TerrainEditMode>(),
        TerrainEditMode::Extension(TerrainToolNavmeshOp::ID)
    );
    let slots = options_slots(&mut app);
    assert_eq!(
        slots.len(),
        1,
        "the options bar holds one slot for the tool"
    );
    assert!(
        buttons_calling(&mut app, TerrainNavmeshBakeOp::ID) > 0,
        "the bar offers the bake"
    );

    util::call(&mut app, TerrainToolNavmeshOp::ID);
    assert_eq!(
        *app.world().resource::<TerrainEditMode>(),
        TerrainEditMode::None
    );
    assert!(
        options_slots(&mut app).is_empty(),
        "putting it away clears the bar"
    );
}

#[test]
fn disabling_the_module_takes_its_tool_away() {
    let mut app = util::editor_app();
    util::selected_terrain(&mut app);
    util::call(&mut app, TerrainToolNavmeshOp::ID);

    assert!(disable_extension(app.world_mut(), EXTENSION_ID));
    util::settle(&mut app);

    assert_eq!(buttons_calling(&mut app, TerrainToolNavmeshOp::ID), 0);
    assert_eq!(
        *app.world().resource::<TerrainEditMode>(),
        TerrainEditMode::None,
        "a tool that is gone is not left picked"
    );
    assert!(options_slots(&mut app).is_empty());
}
