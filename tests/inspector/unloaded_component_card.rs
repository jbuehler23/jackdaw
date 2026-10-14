//! A component whose type the editor has not loaded gets a read-only card
//! saying so, and the panel header names what the scene needs.

use bevy::prelude::*;
use jackdaw::selection::Selection;

use crate::util;

const SUSPENSION: &str = "vehicle_tools::Suspension";
const COLLIDER: &str = "avian3d::Collider";

const SCENE: &str = "bevy_ecs::hierarchy::Children [
    #Crate
    vehicle_tools::Suspension { stiffness: 2.5, travel: 0.3 }
]
";

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn opened(tag: &str, scene: &str) -> (App, std::path::PathBuf) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let dir = std::env::temp_dir().join(format!("jd_unloaded_card_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scene dir");
    let path = dir.join("scene.bsn");
    std::fs::write(&path, scene).expect("write scene");
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    settle(&mut app);
    (app, dir)
}

fn select_the_crate(app: &mut App) {
    let mut query = app.world_mut().query::<(Entity, &Name)>();
    let entity = query
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Crate")
        .map(|(entity, _)| entity)
        .expect("the crate");
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    settle(app);
}

#[test]
fn an_unloaded_component_has_a_read_only_card_that_says_so() {
    let (mut app, dir) = opened("card", SCENE);
    select_the_crate(&mut app);

    let showing = jackdaw::inspector::component_cards_showing(app.world_mut());
    assert!(
        showing.iter().any(|type_path| type_path == SUSPENSION),
        "the card is listed, got {showing:?}"
    );
    let lines = jackdaw::inspector::component_card_text(app.world_mut(), SUSPENSION);
    for wanted in [
        "not loaded",
        SUSPENSION,
        "From vehicle_tools, which this editor does not have.",
        "stiffness: 2.5",
        "travel: 0.3",
    ] {
        assert!(
            lines.iter().any(|line| line == wanted),
            "the card shows `{wanted}`, got {lines:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_panel_names_the_crate_the_scene_needs() {
    let (mut app, dir) = opened("notice", SCENE);
    let notice =
        jackdaw::inspector::unavailable::notice_showing(app.world_mut()).expect("a notice shows");
    assert!(
        notice
            .starts_with("This scene uses components from vehicle_tools, which is not available."),
        "got {notice}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn removing_the_last_unloaded_component_hides_the_notice() {
    let (mut app, dir) = opened("cleared", SCENE);
    select_the_crate(&mut app);
    let entity = app
        .world()
        .resource::<Selection>()
        .primary()
        .expect("selected");
    let mut call = jackdaw_api_internal::operator::OperatorWorldExt::operator(
        app.world_mut(),
        "component.remove",
    );
    call = call
        .param("entity", jackdaw_scene_types::PropertyValue::Entity(entity))
        .param("type_path", SUSPENSION);
    let result = call.call().expect("the operator dispatched");
    assert_eq!(
        result,
        jackdaw_api::prelude::OperatorResult::Finished,
        "the removal ran"
    );
    settle(&mut app);

    assert!(jackdaw::inspector::unavailable::notice_showing(app.world_mut()).is_none());
    assert!(
        !jackdaw::inspector::component_cards_showing(app.world_mut())
            .iter()
            .any(|type_path| type_path == SUSPENSION)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crate_a_module_provides_names_the_module() {
    let scene = format!("bevy_ecs::hierarchy::Children [\n    #Crate\n    {COLLIDER}\n]\n");
    let (mut app, dir) = opened("module", &scene);
    let notice =
        jackdaw::inspector::unavailable::notice_showing(app.world_mut()).expect("a notice shows");
    assert!(notice.contains("Enable the Physics module"), "got {notice}");
    let _ = std::fs::remove_dir_all(&dir);
}
