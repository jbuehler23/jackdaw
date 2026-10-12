//! Components whose types the editor has not loaded live only in the scene
//! document. Every edit that copies, moves or restores an entity must carry
//! them along, and removing one must be undoable.

use crate::util;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;

const SUSPENSION: &str = "vehicle_tools::Suspension";

const SCENE: &str = "bevy_ecs::hierarchy::Children [
    #Crate
    vehicle_tools::Suspension { stiffness: 2.5, travel: 0.3 }
]
";

struct Opened {
    app: App,
    dir: std::path::PathBuf,
    crate_entity: Entity,
}

fn opened(tag: &str) -> Opened {
    let mut app = util::editor_test_app();
    app.world_mut()
        .remove_resource::<jackdaw::entity_ops::SystemClipboard>();
    let dir = std::env::temp_dir().join(format!("jd_unloaded_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scene dir");
    let path = dir.join("scene.bsn");
    std::fs::write(&path, SCENE).expect("write scene");
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    app.update();
    let crate_entity = named(&mut app, "Crate");
    jackdaw::selection::select_only(app.world_mut(), crate_entity);
    app.update();
    Opened {
        app,
        dir,
        crate_entity,
    }
}

#[track_caller]
fn named(app: &mut App, wanted: &str) -> Entity {
    let mut query = app.world_mut().query::<(Entity, &Name)>();
    query
        .iter(app.world())
        .find(|(_, name)| name.as_str() == wanted)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

#[track_caller]
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} ran");
    for _ in 0..3 {
        app.update();
    }
}

/// How many entities the saved scene gives the unloaded component, and with
/// its authored values.
fn saved_copies(opened: &mut Opened) -> usize {
    let emitted =
        jackdaw::scene_io::emit_bsn_scene_with_inline_assets(opened.app.world_mut(), &opened.dir);
    let copies = emitted.matches(SUSPENSION).count();
    assert_eq!(
        emitted.matches("stiffness: 2.5").count(),
        copies,
        "every copy keeps its values:\n{emitted}"
    );
    copies
}

#[test]
fn a_duplicate_keeps_the_unloaded_component() {
    let mut opened = opened("duplicate");
    call(&mut opened.app, "entity.duplicate", &[]);
    assert_eq!(saved_copies(&mut opened), 2);
    let _ = std::fs::remove_dir_all(&opened.dir);
}

#[test]
fn a_pasted_copy_keeps_the_unloaded_component() {
    let mut opened = opened("paste");
    call(&mut opened.app, "entity.copy", &[]);
    call(&mut opened.app, "entity.paste", &[]);
    assert_eq!(saved_copies(&mut opened), 2);
    let _ = std::fs::remove_dir_all(&opened.dir);
}

#[test]
fn undoing_a_delete_brings_the_unloaded_component_back() {
    let mut opened = opened("delete");
    call(&mut opened.app, "entity.delete", &[]);
    assert_eq!(saved_copies(&mut opened), 0);
    call(&mut opened.app, "history.undo", &[]);
    assert_eq!(saved_copies(&mut opened), 1);
    let _ = std::fs::remove_dir_all(&opened.dir);
}

#[test]
fn a_prefab_made_from_the_entity_keeps_the_unloaded_component() {
    let mut opened = opened("prefab");
    let prefab = opened.dir.join("crate_prefab.bsn");
    let entity = opened.crate_entity;
    jackdaw::prefab::operators::save_as_prefab_from_selection(
        opened.app.world_mut(),
        &[entity],
        &prefab,
    );
    opened.app.update();
    let written = std::fs::read_to_string(&prefab).expect("the prefab is written");
    assert!(
        written.contains(SUSPENSION) && written.contains("stiffness: 2.5"),
        "the prefab dropped the component:\n{written}"
    );
    let _ = std::fs::remove_dir_all(&opened.dir);
}

#[test]
fn removing_the_unloaded_component_is_undoable() {
    let mut opened = opened("remove");
    let entity = opened.crate_entity;
    call(
        &mut opened.app,
        "component.remove",
        &[
            ("entity", PropertyValue::Entity(entity)),
            ("type_path", SUSPENSION.into()),
        ],
    );
    assert_eq!(saved_copies(&mut opened), 0);
    call(&mut opened.app, "history.undo", &[]);
    assert_eq!(saved_copies(&mut opened), 1);
    let _ = std::fs::remove_dir_all(&opened.dir);
}
