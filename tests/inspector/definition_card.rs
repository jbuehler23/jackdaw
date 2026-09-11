//! A definition asset in the inspector. The card is the generic reflected
//! card, so what it is showing is an asset behind a handle rather than a
//! component on the selected entity: the rows have to read and write that
//! asset without a panel of their own.

use crate::util;

use bevy::asset::{Asset, Assets};
use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::definition_assets::{DefinitionAssetEdit, OpenDefinition};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;
use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::tooltip::Tooltip;

#[derive(Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Default)]
struct LootRoll {
    item: String,
    weight: u32,
}

#[derive(Asset, Reflect, Clone, Default)]
#[reflect(Default)]
struct MobDef {
    health: u32,
    loot: Vec<LootRoll>,
}

/// An editor showing the inspector, with a mob definition open in it.
fn app_with_open_definition() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    app.init_asset::<MobDef>();
    app.register_asset_reflect::<MobDef>();
    app.register_type::<MobDef>();
    app.register_type::<LootRoll>();
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
    app.update();

    let result = app
        .world_mut()
        .operator("asset.new")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("type", "mob")
        .param("name", "rat")
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..6 {
        app.update();
    }
    (app, tmp)
}

fn open_mob(app: &App) -> MobDef {
    let handle = jackdaw::definition_assets::open_definition_value(app.world())
        .expect("a definition is open")
        .handle()
        .expect("a compiled definition")
        .clone();
    app.world()
        .resource::<Assets<MobDef>>()
        .get(&handle.typed::<MobDef>())
        .expect("the definition is in its store")
        .clone()
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

fn field_widget(app: &mut App, field_path: &str) -> Entity {
    let type_path = MobDef::type_path();
    let found = all_entities(app).into_iter().find(|entity| {
        jackdaw::inspector::field_edited_by(app.world(), *entity) == Some((type_path, field_path))
    });
    found.unwrap_or_else(|| {
        let paths: Vec<String> = all_entities(app)
            .into_iter()
            .filter_map(|entity| {
                jackdaw::inspector::field_edited_by(app.world(), entity)
                    .map(|(known, field)| format!("{known}.{field}"))
            })
            .collect();
        panic!("no control on the inspector writes `{field_path}`; it writes {paths:?}")
    })
}

fn add_button(app: &mut App) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|entity| {
            app.world()
                .get::<Tooltip>(*entity)
                .is_some_and(|tip| tip.title == "Add an item to the list")
        })
        .expect("the list field offers an Add")
}

#[test]
fn a_scalar_row_on_the_card_writes_the_definition_behind_its_handle() {
    let (mut app, _tmp) = app_with_open_definition();

    let widget = field_widget(&mut app, "health");
    app.world_mut().trigger(ValueChange {
        source: widget,
        value: 40.0_f64,
        is_final: true,
    });
    for _ in 0..4 {
        app.update();
    }

    assert_eq!(open_mob(&app).health, 40, "the row wrote the asset");
    let open = app.world().resource::<OpenDefinition>().0.expect("open");
    assert!(
        app.world()
            .get::<DefinitionAssetEdit>(open)
            .is_some_and(|edit| edit.dirty),
        "and the card knows it has something to save",
    );
}

fn unsaved_marker_display(app: &mut App) -> Display {
    let marker = all_entities(app)
        .into_iter()
        .find(|entity| {
            app.world()
                .get::<Text>(*entity)
                .is_some_and(|text| text.0 == "Unsaved")
        })
        .expect("the card header carries an unsaved marker");
    app.world()
        .get::<Node>(marker)
        .expect("the marker lays out as a node")
        .display
}

#[test]
fn the_header_marks_unsaved_edits_and_clears_the_mark_once_the_file_is_written() {
    let (mut app, _tmp) = app_with_open_definition();
    assert_eq!(unsaved_marker_display(&mut app), Display::None);

    let widget = field_widget(&mut app, "health");
    app.world_mut().trigger(ValueChange {
        source: widget,
        value: 12.0_f64,
        is_final: true,
    });
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        unsaved_marker_display(&mut app),
        Display::Flex,
        "an edit that has not reached the file says so"
    );

    let result = app
        .world_mut()
        .operator("asset.save")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..4 {
        app.update();
    }

    assert_eq!(
        unsaved_marker_display(&mut app),
        Display::None,
        "and stops saying so once it has"
    );
}

#[test]
fn a_drag_across_the_card_undoes_to_the_value_it_started_from() {
    let (mut app, _tmp) = app_with_open_definition();
    let widget = field_widget(&mut app, "health");
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    for value in [10.0_f64, 20.0] {
        app.world_mut().trigger(ValueChange {
            source: widget,
            value,
            is_final: false,
        });
        app.update();
    }
    app.world_mut().trigger(ValueChange {
        source: widget,
        value: 30.0_f64,
        is_final: true,
    });
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(open_mob(&app).health, 30);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "the ticks of a drag mint no history of their own"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });

    assert_eq!(
        open_mob(&app).health,
        0,
        "undo goes back to before the drag, not to its last tick"
    );
}

#[test]
fn the_list_controls_add_a_row_to_the_definitions_list() {
    let (mut app, _tmp) = app_with_open_definition();
    assert!(open_mob(&app).loot.is_empty());

    let add = add_button(&mut app);
    app.world_mut().trigger(ButtonClickEvent { entity: add });
    for _ in 0..10 {
        app.update();
    }

    assert_eq!(
        open_mob(&app).loot.len(),
        1,
        "Add put one entry on the definition's list"
    );
    let row = field_widget(&mut app, "loot[0].weight");
    assert!(
        jackdaw::inspector::field_edited_by(app.world(), row).is_some(),
        "and the card rebuilt with a row for it",
    );
}
