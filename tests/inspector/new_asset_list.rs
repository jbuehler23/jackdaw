//! The New Asset list a folder puts up, and what the inspector shows once a
//! kind is picked from it: the new file's card, ready to be filled in.

use crate::util;

use bevy::asset::Asset;
use bevy::prelude::*;
use jackdaw::definition_assets::{DefinitionAssetEdit, OpenDefinition};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::picker::{PickerItems, PickerSelect};

#[derive(Asset, Reflect, Clone, Default)]
#[reflect(Default)]
struct MobDef {
    health: u32,
    move_speed: f32,
}

/// An editor showing the inspector, with a project of its own and one asset
/// type registered.
fn editor_with_mobs() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets/content/mobs")).expect("a content folder");
    let mut app = util::editor_test_app();
    app.init_asset::<MobDef>();
    app.register_asset_reflect::<MobDef>();
    app.register_type::<MobDef>();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<AssetKinds>()
        .register(AssetKind::extension("mob", "Mob", MobDef::type_path()));
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);
    (app, tmp)
}

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Every control on the panel writing a field of a type.
fn rows_of(app: &mut App, type_path: &str) -> Vec<String> {
    let entities: Vec<Entity> = app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect();
    entities
        .into_iter()
        .filter_map(|entity| jackdaw::inspector::field_edited_by(app.world(), entity))
        .filter(|(known, _)| *known == type_path)
        .map(|(_, field)| field.to_string())
        .collect()
}

#[test]
fn choosing_a_kind_from_a_folder_writes_the_file_there_and_shows_its_card() {
    let (mut app, tmp) = editor_with_mobs();
    let mobs = tmp.path().join("assets/content/mobs");

    let result = app
        .world_mut()
        .operator("asset.new_picker")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .param("path", mobs.to_string_lossy().into_owned())
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);

    let picker = app
        .world_mut()
        .query_filtered::<Entity, With<PickerItems<String>>>()
        .iter(app.world())
        .next()
        .expect("the folder put up the list of kinds");
    let index = app
        .world()
        .get::<PickerItems<String>>(picker)
        .expect("the list holds its lines")
        .items()
        .iter()
        .position(|line| line.starts_with("Mob  "))
        .expect("the list offers the mob kind");
    app.world_mut().trigger(PickerSelect {
        entity: picker,
        index,
    });
    settle(&mut app);

    assert!(
        mobs.join("mob_1.bsn").is_file(),
        "the file lands in the folder the list was opened on"
    );
    let open = app
        .world()
        .resource::<OpenDefinition>()
        .0
        .expect("the new asset is open in the inspector");
    let edit = app
        .world()
        .get::<DefinitionAssetEdit>(open)
        .expect("the card stands for the file just written");
    assert_eq!(edit.name, "mob_1");
    assert_eq!(edit.path, std::path::Path::new("content/mobs/mob_1.bsn"));

    let fields = rows_of(&mut app, MobDef::type_path());
    for expected in ["health", "move_speed"] {
        assert!(
            fields.iter().any(|field| field == expected),
            "the card has a row for {expected}, got {fields:?}"
        );
    }
}
