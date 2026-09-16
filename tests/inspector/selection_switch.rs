//! Moving the inspector from one target to the next.
//!
//! A rebuild takes the old rows down while the controls on them still have
//! work queued against them, and an empty selection has to leave the panel
//! empty rather than keep the card the last selection put there.

use std::sync::{Mutex, MutexGuard};

use bevy::ecs::error::{BevyError, ErrorContext, FallbackErrorHandler};
use bevy::prelude::*;
use jackdaw::inspector::file_card::{OpenFileCard, SelectedFile};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};

use crate::util;

static ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RECORDING: Mutex<()> = Mutex::new(());

fn record(error: BevyError, ctx: ErrorContext) {
    ERRORS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(format!("{ctx}: {error}"));
}

/// Holds the recorder for one test and hands back what the app reported.
struct Recorder {
    _guard: MutexGuard<'static, ()>,
}

impl Recorder {
    fn start() -> Self {
        let guard = RECORDING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ERRORS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Self { _guard: guard }
    }

    fn install(&self, app: &mut App) {
        app.insert_resource(FallbackErrorHandler(record));
    }

    fn reported(&self) -> Vec<String> {
        ERRORS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn show_category(app: &mut App, category: &'static str) {
    let result = app
        .world_mut()
        .operator("inspector.category")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .param(
            "category",
            jackdaw_scene_types::PropertyValue::from(category),
        )
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "the category is shown");
    settle(app);
}

fn select(app: &mut App, entity: Entity) {
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    settle(app);
}

/// An editor with an inspector panel and two document entities, neither
/// selected yet.
fn app_with_two_entities(recorder: &Recorder) -> (App, Entity, Entity) {
    let mut app = util::editor_test_app();
    recorder.install(&mut app);
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let mut spawn_one = |name: &str, x: f32| {
        let entity = app
            .world_mut()
            .spawn((
                Name::new(name.to_string()),
                Transform::from_xyz(x, 0.0, 0.0),
                Node {
                    left: px(x),
                    ..default()
                },
            ))
            .id();
        jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
        entity
    };
    let first = spawn_one("first", 1.0);
    let second = spawn_one("second", 2.0);
    settle(&mut app);
    (app, first, second)
}

fn assert_quiet(recorder: &Recorder) {
    let reported = recorder.reported();
    assert!(
        reported.is_empty(),
        "the inspector reported errors: {reported:?}",
    );
}

#[test]
fn switching_selection_between_entities_reports_no_error() {
    let recorder = Recorder::start();
    let (mut app, first, second) = app_with_two_entities(&recorder);

    select(&mut app, first);
    select(&mut app, second);
    select(&mut app, first);

    assert_quiet(&recorder);
}

#[test]
fn rebuilding_a_card_in_place_reports_no_error() {
    let recorder = Recorder::start();
    let (mut app, first, _second) = app_with_two_entities(&recorder);

    select(&mut app, first);
    app.world_mut()
        .entity_mut(first)
        .insert(PointLight::default());
    settle(&mut app);
    app.world_mut()
        .entity_mut(first)
        .remove::<PointLight>()
        .insert(DirectionalLight::default());
    settle(&mut app);

    assert_quiet(&recorder);
}

#[test]
fn opening_a_file_card_over_an_entity_reports_no_error() {
    let recorder = Recorder::start();
    let (mut app, first, _second) = app_with_two_entities(&recorder);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "notes").expect("the file writes");

    select(&mut app, first);
    let world = app.world_mut();
    jackdaw::inspector::file_card::show_file(world, &path);
    settle(&mut app);

    assert_quiet(&recorder);
}

#[test]
fn clearing_the_selection_takes_the_file_card_down() {
    let recorder = Recorder::start();
    let (mut app, _first, _second) = app_with_two_entities(&recorder);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "notes").expect("the file writes");

    jackdaw::inspector::file_card::show_file(app.world_mut(), &path);
    settle(&mut app);
    assert!(
        app.world().resource::<OpenFileCard>().0.is_some(),
        "the card opened",
    );

    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle(&mut app);

    assert!(
        app.world().resource::<OpenFileCard>().0.is_none(),
        "clearing the selection closes the file card",
    );
    assert_eq!(
        app.world_mut()
            .query::<&SelectedFile>()
            .iter(app.world())
            .count(),
        0,
        "the entity the card was carried on goes with it",
    );
    assert_quiet(&recorder);
}

#[test]
fn moving_between_category_tabs_reports_no_error() {
    let recorder = Recorder::start();
    let (mut app, first, _second) = app_with_two_entities(&recorder);

    select(&mut app, first);
    show_category(&mut app, "components");
    show_category(&mut app, "object");
    show_category(&mut app, "components");

    assert_quiet(&recorder);
}

#[test]
fn selecting_an_entity_over_a_file_card_reports_no_error() {
    let recorder = Recorder::start();
    let (mut app, first, _second) = app_with_two_entities(&recorder);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "notes").expect("the file writes");

    jackdaw::inspector::file_card::show_file(app.world_mut(), &path);
    settle(&mut app);
    select(&mut app, first);

    assert!(
        app.world().resource::<OpenFileCard>().0.is_none(),
        "selecting an entity takes the file card down",
    );
    assert_quiet(&recorder);
}
