//! The scene data jackdaw itself defines, on the inspector. Every component an
//! entity carries gets a card, as Unity's and Godot's inspectors show them; the
//! namespace cull that keeps jackdaw's bookkeeping out of the list must not
//! take the scene's wind, environment or material overrides with it.

use std::collections::BTreeMap;

use crate::util;

use bevy::prelude::*;
use jackdaw::selection::Selection;
use jackdaw_scene_types::{Environment, InstanceMaterialOverrides, MaterialOverrides, Wind};

/// A selected, document-tracked entity carrying `components`.
fn app_with(components: impl Bundle) -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((Name::new("meadow"), Transform::default(), components))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    for _ in 0..4 {
        app.update();
    }
    app
}

#[test]
fn the_wind_the_environment_and_material_overrides_each_get_a_card() {
    let leaves = BTreeMap::from([("Leaves".to_string(), "materials/pine.bsn".to_string())]);
    let mut app = app_with((
        Wind::default(),
        Environment::default(),
        MaterialOverrides {
            materials: leaves.clone(),
        },
        InstanceMaterialOverrides { materials: leaves },
    ));

    let cards = jackdaw::inspector::component_cards_showing(app.world_mut());
    for type_path in [
        "jackdaw_scene_types::types::Wind",
        "jackdaw_scene_types::environment::Environment",
        "jackdaw_scene_types::types::MaterialOverrides",
        "jackdaw_scene_types::types::InstanceMaterialOverrides",
    ] {
        assert!(
            cards.iter().any(|card| card == type_path),
            "{type_path} has no card among {cards:?}"
        );
    }
    assert!(
        !cards.iter().any(|card| card.ends_with("SceneNodeId")),
        "the node id stays bookkeeping: {cards:?}"
    );
}

#[test]
fn an_override_card_shows_the_material_it_names() {
    let mut app = app_with(MaterialOverrides {
        materials: BTreeMap::from([("Bark".to_string(), "materials/bark.bsn".to_string())]),
    });
    let text = jackdaw::inspector::component_card_text(
        app.world_mut(),
        "jackdaw_scene_types::types::MaterialOverrides",
    );
    assert!(
        text.iter().any(|line| line.contains("Bark")),
        "the card names the overridden material: {text:?}"
    );
    let mut all = app.world_mut().query::<Entity>();
    let entities: Vec<Entity> = all.iter(app.world()).collect();
    let edited: Vec<String> = entities
        .into_iter()
        .filter_map(|entity| jackdaw::inspector::field_edited_by(app.world(), entity))
        .filter(|(type_path, _)| *type_path == "jackdaw_scene_types::types::MaterialOverrides")
        .map(|(_, field)| field.to_string())
        .collect();
    assert!(
        edited
            .iter()
            .any(|field| field.starts_with("materials[") && field.contains("Bark")),
        "the override's material is an editable field: {edited:?}"
    );
}
