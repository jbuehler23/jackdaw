//! The Preview Context panel: design-time binding preview.
//!
//! Preview gives the editor a scratch subject entity, points the open UI scene
//! at it with `BindContext`, and runs `jackdaw_bind`'s evaluator for as long as
//! the toggle is on, so scrubbing a number redraws the health bar in the
//! viewport without building or running the game.
//!
//! What is pinned here:
//!
//! 1. Preview on spawns the scratch subject, points the scene at it, and
//!    drives width, text, and visibility from a scrubbed field.
//! 2. Preview off puts the authored values back, drops the context, and
//!    stops evaluating.
//! 3. A preview session leaves the saved document byte-identical: neither
//!    the scratch entity nor the editor's `BindContext` reaches the AST, and
//!    an edit gesture aimed at a bound property is refused rather than baked.
//! 4. A type the editor knows only as schema cannot become a real component
//!    here, so its rows render disabled instead of lying.
//! 5. The session follows the scene it is previewing: a tab switch, a
//!    structural undo, or a scene that stops being the open one moves the
//!    context with it and strands nothing behind.

use std::path::Path;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::preview_context::{
    self, PREVIEW_CONTEXT_WINDOW_ID, PreviewAvailability, PreviewField, PreviewFieldKind,
    PreviewValue,
};
use jackdaw::project_types::ProjectTypes;
use jackdaw::selection::Selection;
use jackdaw_bind::{BindContext, BindFailures, BindPath, Binding, Bindings};
use jackdaw_commands::CommandHistory;
use jackdaw_feathers::number_input::ScrubNumberInput;
use jackdaw_panels::registry::WindowRegistry;
use jackdaw_scene_types::UiSceneRoot;
use jackdaw_schema::{FieldSchema, ProjectSchema, TypeKind, TypeSchema};

mod util;

/// The subject a health bar reads. Registered natively by the test, which is
/// exactly the case the editor can preview: a real Rust type it links.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Vitals {
    ratio: f32,
    current: f32,
    alive: bool,
    tag: String,
}

/// The authored width of the fill bar, before any binding runs.
const AUTHORED_WIDTH: Val = Val::Px(10.0);

struct HealthBar {
    root: Entity,
    fill: Entity,
    label: Entity,
    veil: Entity,
}

/// A health-bar scene in the document: a fill whose width reads
/// `Vitals.ratio`, a caption that formats `Vitals.current`, and a veil whose
/// visibility follows `Vitals.alive`.
fn health_bar_app() -> (App, HealthBar) {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();

    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node {
                width: AUTHORED_WIDTH,
                ..default()
            },
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Vitals.ratio")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    let label = world
        .spawn((
            Name::new("Label"),
            Node::default(),
            Text::new("HP"),
            ChildOf(root),
            Bindings(vec![Binding::Text {
                format: "{} HP".to_string(),
                args: vec![BindPath::new("Vitals.current")],
            }]),
        ))
        .id();
    let veil = world
        .spawn((
            Name::new("Veil"),
            Node::default(),
            Visibility::Inherited,
            ChildOf(root),
            Bindings(vec![Binding::Visible {
                read: BindPath::new("Vitals.alive"),
                via: None,
            }]),
        ))
        .id();

    for entity in [root, fill, label, veil] {
        jackdaw::scene_io::register_entity_in_ast(world, entity);
    }
    app.update();
    (
        app,
        HealthBar {
            root,
            fill,
            label,
            veil,
        },
    )
}

fn scratch(app: &mut App) -> Option<Entity> {
    app.world_mut()
        .query_filtered::<(Entity, &Name), With<jackdaw::EditorEntity>>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Preview Subject")
        .map(|(entity, _)| entity)
}

fn width(app: &App, entity: Entity) -> Val {
    app.world().get::<Node>(entity).expect("a node").width
}

fn document(app: &mut App) -> String {
    jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), Path::new("."))
}

/// One UI scene: an unparented root and a fill bound to `Vitals.ratio`,
/// registered in the document the way a load leaves it.
fn spawn_bound_scene(world: &mut World, name: &str) -> (Entity, Entity) {
    let root = world
        .spawn((
            Name::new(name.to_string()),
            UiSceneRoot::default(),
            Node::default(),
        ))
        .id();
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node {
                width: AUTHORED_WIDTH,
                ..default()
            },
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Vitals.ratio")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, fill);
    (root, fill)
}

fn scrub_ratio(app: &mut App, value: f64) {
    preview_context::write_scratch_field(
        app.world_mut(),
        &PreviewField::new("Vitals", "ratio"),
        PreviewValue::Number(value),
    )
    .expect("a native field is writable");
}

// ---------------------------------------------------------------------------
// Preview on
// ---------------------------------------------------------------------------

#[test]
fn preview_on_drives_the_scene_from_a_scratch_subject() {
    let (mut app, bar) = health_bar_app();
    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "nothing evaluates before the user asks for a preview",
    );

    preview_context::set_preview(app.world_mut(), true);
    app.update();

    let subject = scratch(&mut app).expect("preview spawns one scratch subject");
    assert!(
        app.world().get::<Vitals>(subject).is_some(),
        "the type the bindings read is attached to the subject",
    );
    assert_eq!(
        app.world().get::<BindContext>(bar.root).map(|c| c.0),
        Some(subject),
        "the scene root points at the scratch subject while preview is on",
    );

    preview_context::write_scratch_field(
        app.world_mut(),
        &PreviewField::new("Vitals", "ratio"),
        PreviewValue::Number(0.5),
    )
    .expect("a native field is writable");
    preview_context::write_scratch_field(
        app.world_mut(),
        &PreviewField::new("Vitals", "current"),
        PreviewValue::Number(87.0),
    )
    .expect("a native field is writable");
    preview_context::write_scratch_field(
        app.world_mut(),
        &PreviewField::new("Vitals", "alive"),
        PreviewValue::Bool(false),
    )
    .expect("a native field is writable");
    app.update();

    assert_eq!(
        width(&app, bar.fill),
        Val::Percent(50.0),
        "the scrubbed ratio drives the fill",
    );
    assert_eq!(
        app.world().get::<Text>(bar.label).map(|t| t.0.clone()),
        Some("87 HP".to_string()),
        "and the caption",
    );
    assert_eq!(
        app.world().get::<Visibility>(bar.veil),
        Some(&Visibility::Hidden),
        "and the veil",
    );
}

// ---------------------------------------------------------------------------
// Preview off
// ---------------------------------------------------------------------------

#[test]
fn preview_off_restores_the_scene_and_stops_evaluating() {
    let (mut app, bar) = health_bar_app();
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    preview_context::write_scratch_field(
        app.world_mut(),
        &PreviewField::new("Vitals", "ratio"),
        PreviewValue::Number(0.5),
    )
    .expect("a native field is writable");
    app.update();
    assert_eq!(width(&app, bar.fill), Val::Percent(50.0));

    preview_context::set_preview(app.world_mut(), false);
    app.update();

    assert!(
        scratch(&mut app).is_none(),
        "the scratch subject is gone with the session",
    );
    assert!(
        app.world().get::<BindContext>(bar.root).is_none(),
        "and so is the context the editor inserted",
    );
    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "the authored width is what the user is editing again",
    );
    assert_eq!(
        app.world().get::<Text>(bar.label).map(|t| t.0.clone()),
        Some("HP".to_string()),
        "and the authored caption",
    );

    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "with preview off nothing evaluates any more",
    );
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[test]
fn a_preview_session_leaves_the_document_untouched() {
    let (mut app, _bar) = health_bar_app();
    let before = document(&mut app);

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    preview_context::write_scratch_field(
        app.world_mut(),
        &PreviewField::new("Vitals", "ratio"),
        PreviewValue::Number(0.75),
    )
    .expect("a native field is writable");
    app.update();

    let during = document(&mut app);
    assert_eq!(
        during, before,
        "saving mid-preview writes the authored scene, not the preview",
    );
    assert!(
        !during.contains("Preview Subject") && !during.contains("BindContext"),
        "neither the scratch subject nor the editor's context is in the document:\n{during}",
    );
    // Evaluating puts a `ResolvedBindings` on every bound widget. It is derived
    // from what the document already says and means nothing on reload.
    assert!(
        !during.contains("ResolvedBindings"),
        "the evaluator's own bookkeeping reached the document:\n{during}",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert_eq!(
        document(&mut app),
        before,
        "and the session leaves no trace"
    );
}

// ---------------------------------------------------------------------------
// The native / schema-only boundary
// ---------------------------------------------------------------------------

#[test]
fn a_schema_only_type_previews_as_a_disabled_row() {
    let mut app = util::editor_test_app();
    let schema = ProjectSchema {
        components: vec![TypeSchema {
            type_path: "demo_game::Health".to_string(),
            short_name: "Health".to_string(),
            module_path: String::new(),
            category: String::new(),
            description: String::new(),
            hidden: false,
            default_constructible: true,
            fields: vec![FieldSchema {
                name: "current".to_string(),
                type_path: "f32".to_string(),
            }],
            kind: TypeKind::Struct,
            default: None,
            variants: Vec::new(),
            entity_fields: Vec::new(),
            fills_gaps: true,
        }],
        resources: Vec::new(),
        events: Vec::new(),
        functions: Vec::new(),
    };
    {
        let native = jackdaw::project_types::native_type_paths(
            &app.world().resource::<AppTypeRegistry>().read(),
        );
        app.world_mut()
            .resource_mut::<ProjectTypes>()
            .update(&schema, &native);
    }

    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node::default(),
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("demo_game::Health.current")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, fill);

    preview_context::set_preview(app.world_mut(), true);
    app.update();

    let subjects = preview_context::preview_layout(app.world_mut());
    let health = subjects
        .iter()
        .find(|subject| subject.type_path == "demo_game::Health")
        .expect("the referenced type is listed");
    assert_eq!(
        health.availability,
        PreviewAvailability::SchemaOnly,
        "the editor has no Rust type for a project component, so it cannot construct one",
    );
    assert!(
        !health.note.is_empty(),
        "the row says why it is disabled instead of looking broken",
    );

    let subject = scratch(&mut app).expect("the session still has a subject entity");
    assert!(
        app.world().get::<Node>(subject).is_none(),
        "nothing unrelated is attached to the scratch entity",
    );

    // What the user actually sees: the field is there, as a row nothing can
    // touch, under the reason it cannot be touched.
    app.world_mut()
        .spawn(Node::default())
        .with_children(preview_context::build_preview_context_panel);
    app.update();

    let disabled: Vec<PreviewField> = app
        .world_mut()
        .query::<&preview_context::PreviewDisabledField>()
        .iter(app.world())
        .map(|field| field.0.clone())
        .collect();
    assert_eq!(
        disabled,
        vec![PreviewField::new("demo_game::Health", "current")],
        "the schema field renders as a disabled row",
    );
    assert!(
        app.world_mut()
            .query::<&preview_context::PreviewField>()
            .iter(app.world())
            .next()
            .is_none(),
        "and no scrubbable row is offered for a type the editor cannot construct",
    );
    let notes: Vec<String> = app
        .world_mut()
        .query::<&Text>()
        .iter(app.world())
        .map(|text| text.0.clone())
        .collect();
    assert!(
        notes.iter().any(|text| text.contains("PIE")),
        "the panel says preview needs the game running; got {notes:?}",
    );
}

#[test]
fn a_native_type_lists_one_row_per_field() {
    let (mut app, _bar) = health_bar_app();
    preview_context::set_preview(app.world_mut(), true);
    app.update();

    let subjects = preview_context::preview_layout(app.world_mut());
    let vitals = subjects
        .iter()
        .find(|subject| subject.type_path.ends_with("Vitals"))
        .expect("the referenced type is listed");
    assert_eq!(vitals.availability, PreviewAvailability::Native);
    let kinds: Vec<(&str, PreviewFieldKind)> = vitals
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("ratio", PreviewFieldKind::Number),
            ("current", PreviewFieldKind::Number),
            ("alive", PreviewFieldKind::Bool),
            ("tag", PreviewFieldKind::Text),
        ],
        "every scalar field gets the control its type asks for",
    );
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

#[test]
fn the_panel_registers_in_the_right_sidebar_and_builds_its_rows() {
    let mut app = util::editor_test_app();
    {
        let registry = app.world().resource::<WindowRegistry>();
        let window = registry
            .get(PREVIEW_CONTEXT_WINDOW_ID)
            .expect("the preview panel is a registered dock window");
        assert_eq!(window.default_area, "right_sidebar");
    }

    app.world_mut()
        .spawn(Node::default())
        .with_children(preview_context::build_preview_context_panel);
    app.update();

    assert!(
        app.world_mut()
            .query::<&preview_context::PreviewToggle>()
            .iter(app.world())
            .next()
            .is_some(),
        "the panel opens with a preview toggle",
    );
    assert!(
        !preview_context::preview_is_running(app.world()),
        "and the toggle starts off",
    );
}

/// The panel end to end: its toggle starts the session and its scrub rows
/// move the scene, through the same events the widgets emit.
#[test]
fn the_panel_toggle_and_its_rows_drive_the_scene() {
    let (mut app, bar) = health_bar_app();
    app.world_mut()
        .spawn(Node::default())
        .with_children(preview_context::build_preview_context_panel);
    app.update();

    let toggle = app
        .world_mut()
        .query_filtered::<Entity, With<preview_context::PreviewToggle>>()
        .iter(app.world())
        .next()
        .expect("the panel has a toggle");
    app.world_mut().trigger(ValueChange::<bool> {
        source: toggle,
        value: true,
        is_final: true,
    });
    app.update();
    app.update();
    assert!(
        preview_context::preview_is_running(app.world()),
        "the toggle starts a session",
    );

    let ratio = app
        .world_mut()
        .query::<(Entity, &PreviewField)>()
        .iter(app.world())
        .find(|(_, field)| field.field == "ratio")
        .map(|(entity, _)| entity)
        .expect("the panel built a row for every field of the referenced type");
    app.world_mut().trigger(ValueChange::<f32> {
        source: ratio,
        value: 0.25,
        is_final: true,
    });
    app.update();

    assert_eq!(
        width(&app, bar.fill),
        Val::Percent(25.0),
        "scrubbing the row moves the bound widget",
    );
}

// ---------------------------------------------------------------------------
// Following the scene
// ---------------------------------------------------------------------------

/// Swapping scenes (a tab switch, or the respawn a structural undo does)
/// leaves a session pointing at a root that is gone. The referenced types do
/// not change, so nothing but the root itself says the session went stale.
#[test]
fn preview_follows_a_scene_swap() {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();
    let (first_root, first_fill) = spawn_bound_scene(app.world_mut(), "Scene A");
    app.update();

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();
    assert_eq!(width(&app, first_fill), Val::Percent(50.0));
    let subject = scratch(&mut app).expect("a subject");

    // The tab switch: the open scene is despawned and another is spawned in
    // its place.
    app.world_mut().entity_mut(first_root).despawn();
    let (second_root, second_fill) = spawn_bound_scene(app.world_mut(), "Scene B");
    app.update();
    app.update();

    assert_eq!(
        app.world().get::<BindContext>(second_root).map(|c| c.0),
        Some(subject),
        "the session follows the scene the user is now editing",
    );
    assert_eq!(
        scratch(&mut app),
        Some(subject),
        "and keeps the same subject, so scrubbed values survive the swap",
    );
    assert_eq!(
        width(&app, second_fill),
        Val::Percent(50.0),
        "the value scrubbed before the swap still drives the new scene",
    );
}

/// The same move with the old root still alive: it stopped being the open
/// scene (parented under something else), so it must not keep a context
/// pointing at a subject nothing maintains for it.
#[test]
fn a_root_that_stops_being_the_open_scene_keeps_no_context() {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();
    let (first_root, _) = spawn_bound_scene(app.world_mut(), "Scene A");
    app.update();
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    assert!(app.world().get::<BindContext>(first_root).is_some());

    let (second_root, second_fill) = spawn_bound_scene(app.world_mut(), "Scene B");
    let holder = app.world_mut().spawn(Node::default()).id();
    app.world_mut()
        .entity_mut(first_root)
        .insert(ChildOf(holder));
    app.update();
    scrub_ratio(&mut app, 0.25);
    app.update();

    assert!(
        app.world().get::<BindContext>(first_root).is_none(),
        "the scene that is no longer open is left as the user authored it",
    );
    assert_eq!(
        app.world().get::<BindContext>(second_root).map(|c| c.0),
        scratch(&mut app),
        "and the open one carries the session",
    );
    assert_eq!(width(&app, second_fill), Val::Percent(25.0));
}

// ---------------------------------------------------------------------------
// The session's write targets move under it
// ---------------------------------------------------------------------------

/// A binding authored mid-session on a widget nothing was bound to yet. Its
/// reads name a type the panel already lists, so the session's read-derived
/// list does not move at all, yet the evaluator owns a second widget's `Node`,
/// which has to be snapshotted and guarded like the first.
#[test]
fn a_binding_added_mid_session_is_put_back_and_guarded() {
    let (mut app, bar) = health_bar_app();
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();
    assert_eq!(width(&app, bar.fill), Val::Percent(50.0));

    let second = app
        .world_mut()
        .spawn((
            Name::new("Second Fill"),
            Node {
                width: AUTHORED_WIDTH,
                ..default()
            },
            ChildOf(bar.root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Vitals.ratio")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), second);
    app.update();
    app.update();

    assert_eq!(
        width(&app, second),
        Val::Percent(50.0),
        "the evaluator drives the widget bound mid-session",
    );
    assert!(
        preview_context::preview_writes_type_path(app.world(), second, "Node"),
        "so an authored edit to it has to be refused too",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert_eq!(
        width(&app, second),
        AUTHORED_WIDTH,
        "and the authored width comes back when the session ends",
    );
}

/// The same case from the other side: the binding is the one the session
/// started with, but its write path names a different component. The property
/// it left keeps its snapshot and the one it moved to gets its own.
#[test]
fn a_write_repointed_mid_session_is_put_back_and_guarded() {
    let (mut app, bar) = health_bar_app();
    app.world_mut()
        .entity_mut(bar.fill)
        .insert(Transform::from_xyz(7.0, 0.0, 0.0));
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();
    assert_eq!(width(&app, bar.fill), Val::Percent(50.0));

    if let Some(mut bindings) = app.world_mut().get_mut::<Bindings>(bar.fill)
        && let Some(Binding::Field {
            write, as_percent, ..
        }) = bindings.0.first_mut()
    {
        *write = BindPath::new("Transform.translation.x");
        *as_percent = false;
    }
    app.update();
    app.update();

    assert_eq!(
        app.world()
            .get::<Transform>(bar.fill)
            .map(|t| t.translation.x),
        Some(0.5),
        "the evaluator drives the component the write was moved to",
    );
    assert!(
        preview_context::preview_writes_type_path(app.world(), bar.fill, "Transform"),
        "so an authored edit to it has to be refused too",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert_eq!(
        app.world()
            .get::<Transform>(bar.fill)
            .map(|t| t.translation.x),
        Some(7.0),
        "the authored transform comes back",
    );
    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "and so does the width the write used to own",
    );
}

/// A property the session gives up mid-flight: the guard over it comes down
/// the moment the bindings stop naming it, so from then on the user can author
/// it. The snapshot has to be spent at that moment too: held to the end of the
/// session it would be written back over whatever the user did in between,
/// leaving the document holding one value and the component another.
#[test]
fn a_property_the_session_gives_up_is_put_back_before_it_can_be_edited() {
    let (mut app, bar) = health_bar_app();
    app.world_mut().resource_mut::<Selection>().entities = vec![bar.fill];
    app.world_mut()
        .entity_mut(bar.fill)
        .insert(Transform::from_xyz(7.0, 0.0, 0.0));
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();
    assert_eq!(width(&app, bar.fill), Val::Percent(50.0));

    repoint_write_to_transform(&mut app, bar.fill);

    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "the width is given back the moment the session stops writing it",
    );
    assert!(
        !preview_context::preview_writes_type_path(app.world(), bar.fill, "Node"),
        "and it is the user's to author again",
    );

    // So they author it, with the stage gesture the guard refuses while the
    // session owns the property: the gesture moves the live component and the
    // commit follows it, the way a drag does.
    let before = app.world().get::<Node>(bar.fill).cloned().expect("a node");
    let after = Node {
        width: Val::Px(999.0),
        ..before.clone()
    };
    app.world_mut().entity_mut(bar.fill).insert(after.clone());
    jackdaw::commands::push_layout_edit(app.world_mut(), bar.fill, before, after);
    app.update();
    assert_eq!(
        width(&app, bar.fill),
        Val::Px(999.0),
        "the edit lands, because nothing is driving the width any more",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert_eq!(
        width(&app, bar.fill),
        Val::Px(999.0),
        "and the session must not write its old snapshot over it",
    );
    assert!(
        document(&mut app).contains("999"),
        "the document and the component have to agree on what the user authored",
    );
}

/// The other half of the same move: a write that leaves and comes back. What
/// the user authored is still what the session owes them at the end.
#[test]
fn a_write_that_comes_back_still_gives_back_what_was_authored() {
    let (mut app, bar) = health_bar_app();
    app.world_mut()
        .entity_mut(bar.fill)
        .insert(Transform::from_xyz(7.0, 0.0, 0.0));
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();

    repoint_write_to_transform(&mut app, bar.fill);
    repoint_write_to_width(&mut app, bar.fill);
    assert_eq!(
        width(&app, bar.fill),
        Val::Percent(50.0),
        "the width is driven again once the write comes back to it",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "and the authored width is what the session gives back",
    );
}

/// Move the fill's field binding off `Node.width` and onto a plain number.
fn repoint_write_to_transform(app: &mut App, fill: Entity) {
    if let Some(mut bindings) = app.world_mut().get_mut::<Bindings>(fill)
        && let Some(Binding::Field {
            write, as_percent, ..
        }) = bindings.0.first_mut()
    {
        *write = BindPath::new("Transform.translation.x");
        *as_percent = false;
    }
    app.update();
    app.update();
}

fn repoint_write_to_width(app: &mut App, fill: Entity) {
    if let Some(mut bindings) = app.world_mut().get_mut::<Bindings>(fill)
        && let Some(Binding::Field {
            write, as_percent, ..
        }) = bindings.0.first_mut()
    {
        *write = BindPath::new("Node.width");
        *as_percent = true;
    }
    app.update();
    app.update();
}

/// A scene root that stops being the open one takes the session's subject with
/// it, but not the widgets: those are still in the world, still holding what
/// the evaluator put there. Letting go of the subject has to put them back,
/// because the guard that refuses an authored edit goes down with it.
#[test]
fn losing_the_scene_root_puts_back_what_the_session_wrote() {
    let (mut app, bar) = health_bar_app();
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();
    assert_eq!(width(&app, bar.fill), Val::Percent(50.0));

    // The document closed: the root is still alive but is no longer a scene
    // root of its own, so the session has nothing to point at.
    let holder = app.world_mut().spawn(Node::default()).id();
    app.world_mut().entity_mut(bar.root).insert(ChildOf(holder));
    app.update();
    app.update();

    assert_eq!(
        width(&app, bar.fill),
        AUTHORED_WIDTH,
        "the widget is left as the user authored it",
    );
    assert_eq!(
        app.world().get::<Text>(bar.label).map(|t| t.0.clone()),
        Some("HP".to_string()),
        "caption included",
    );
    assert!(
        !preview_context::preview_writes_type_path(app.world(), bar.fill, "Node"),
        "and nothing is left claiming to own it",
    );
}

// ---------------------------------------------------------------------------
// Bound properties are read-only during a session
// ---------------------------------------------------------------------------

/// The bake-through case: while the evaluator owns `Node.width`, an authored
/// edit to it (a stage gesture or an inspector commit) is refused, so the
/// previewed number cannot reach the document as if the user had typed it.
#[test]
fn an_edit_to_a_bound_property_is_refused_mid_preview() {
    let (mut app, bar) = health_bar_app();
    app.world_mut().resource_mut::<Selection>().entities = vec![bar.fill];
    let authored = document(&mut app);

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();

    // A stage gesture that let go on a previewed width.
    let before = app.world().get::<Node>(bar.fill).cloned().expect("a node");
    let after = Node {
        width: Val::Px(999.0),
        ..before.clone()
    };
    jackdaw::commands::push_layout_edit(app.world_mut(), bar.fill, before.clone(), after);
    app.update();
    assert_eq!(
        width(&app, bar.fill),
        Val::Percent(50.0),
        "the gesture is put back where the user found it, not committed",
    );

    // The inspector's own width row, driven the way the widget drives it.
    let row = spawn_width_field(&mut app, bar.fill);
    scrub_number(&mut app, row, 42.0);

    assert_eq!(
        document(&mut app),
        authored,
        "neither edit reached the document",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        0,
        "and neither minted an undo entry",
    );

    // Not vacuous: the same row commits once the session is over.
    preview_context::set_preview(app.world_mut(), false);
    app.update();
    let row = spawn_width_field(&mut app, bar.fill);
    scrub_number(&mut app, row, 42.0);
    assert_ne!(
        document(&mut app),
        authored,
        "with preview off the inspector authors the width as usual",
    );
}

/// Deleting a bound widget mid-preview and undoing must put back what the user
/// authored, not what the evaluator last wrote onto it.
///
/// The undo entry snapshots live ECS, which during a session carries the
/// preview's values on every bound property. Taken unsuspended, the delete/undo
/// pair is a way to launder a previewed number into the document, the same
/// bake-through the edit paths refuse arriving by a different route.
#[test]
fn deleting_a_bound_widget_mid_preview_undoes_to_the_authored_value() {
    let (mut app, bar) = health_bar_app();

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();
    assert_eq!(
        width(&app, bar.fill),
        Val::Percent(50.0),
        "the fixture has to be previewing, or the test proves nothing",
    );

    // Delete the previewed widget, the way the editor deletes a selection.
    app.world_mut().resource_mut::<Selection>().entities = vec![bar.fill];
    jackdaw::entity_ops::delete_selected(app.world_mut());
    app.update();

    // ...and take it back.
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();

    preview_context::set_preview(app.world_mut(), false);
    app.update();

    let restored = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Fill")
        .map(|(entity, _)| entity)
        .expect("undo puts the widget back");

    assert_eq!(
        app.world().get::<Node>(restored).map(|node| node.width),
        Some(AUTHORED_WIDTH),
        "undo restored the previewed width as if the user had authored it",
    );
}

/// The inspector's `Val` row for a `Node`'s width, as the inspector builds it.
fn spawn_width_field(app: &mut App, entity: Entity) -> Entity {
    let parent = app.world_mut().spawn(Node::default()).id();
    let world = app.world_mut();
    let row = {
        let mut commands = world.commands();
        jackdaw::inspector::val_field::spawn_val_field(
            &mut commands,
            parent,
            "width",
            Val::Px(10.0),
            0,
            "width".to_string(),
            entity,
            "bevy_ui::ui_node::Node",
        )
    };
    world.flush();
    app.update();
    row
}

fn scrub_number(app: &mut App, row: Entity, value: f64) {
    let input = descendants(app.world_mut(), row)
        .into_iter()
        .find(|entity| app.world().get::<ScrubNumberInput>(*entity).is_some())
        .expect("the val row has a number input");
    app.world_mut().trigger(ValueChange {
        source: input,
        value,
        is_final: true,
    });
    app.update();
}

fn descendants(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut query = world.query::<&Children>();
    while let Some(entity) = stack.pop() {
        if entity != root {
            out.push(entity);
        }
        if let Ok(children) = query.get(world, entity) {
            stack.extend(children.iter());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Scrub coverage
// ---------------------------------------------------------------------------

/// One field per type the panel advertises a control for, generated from the
/// panel's own width list so the fixture cannot fall behind what is
/// advertised: a width added to `numeric_widths!` grows this struct, and the
/// parity table below walks it.
///
/// A tuple struct, because a list of types has no field names to invent;
/// walking it exercises the `.0` paths at the same time.
macro_rules! every_scalar {
    ($($width:ty),*) => {
        #[derive(Component, Reflect, Default)]
        #[reflect(Component, Default)]
        struct EveryScalar($(pub $width,)* pub bool, pub String);
    };
}
jackdaw::numeric_widths!(every_scalar);

/// How many numeric widths that list holds, for the table's own arithmetic.
macro_rules! width_count {
    ($($width:ty),*) => {
        [$(std::mem::size_of::<$width>()),*].len()
    };
}
const NUMERIC_WIDTHS: usize = jackdaw::numeric_widths!(width_count);

/// A tuple struct, whose one element the panel has to render as a row rather
/// than as a section header with nothing under it.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health(pub f32);

/// A linked type with no fields at all, which is a header with nothing under
/// it however well the panel does its job.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Beacon;

/// An opaque type carrying no `ReflectDefault`, which is the one shape the
/// default builder cannot synthesise a value for: it has no fields to recurse
/// into and nothing to ask for a default.
#[derive(Reflect, Clone, Debug)]
#[reflect(opaque)]
#[reflect(Clone, Debug)]
struct Sealed(
    #[expect(
        dead_code,
        reason = "the payload exists to make the type opaque, not to be read"
    )]
    u32,
);

/// A component the editor links and still cannot build: reflection reaches the
/// field and then has nothing to put in it.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct Locked {
    seal: Sealed,
}

/// Editor state a scene has no business writing, with a field the value
/// write-back can write: `set_target` takes `f32`.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource, Default)]
struct EditorTuning {
    volume: f32,
}

/// A form held as a resource, which no context entity can carry.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource, Default)]
struct CharCreateForm {
    name: String,
    class: u32,
    class_name: String,
}

/// A previewing editor whose one scene reads `read`.
fn previewing(read: &str, register: impl FnOnce(&mut App)) -> App {
    let mut app = util::editor_test_app();
    register(&mut app);
    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node::default(),
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new(read)],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, fill);
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    app
}

/// One schema'd type as the extractor would report it, with the boilerplate
/// every field of it needs but no test cares about.
fn schema_type(type_path: &str, kind: TypeKind, fields: &[(&str, &str)]) -> TypeSchema {
    TypeSchema {
        type_path: type_path.to_string(),
        short_name: type_path
            .rsplit("::")
            .next()
            .unwrap_or(type_path)
            .to_string(),
        module_path: String::new(),
        category: String::new(),
        description: String::new(),
        hidden: false,
        default_constructible: true,
        fields: fields
            .iter()
            .map(|(name, type_path)| FieldSchema {
                name: (*name).to_string(),
                type_path: (*type_path).to_string(),
            })
            .collect(),
        kind,
        default: None,
        variants: Vec::new(),
        entity_fields: Vec::new(),
        fills_gaps: true,
    }
}

/// A previewing editor that knows `schema` as project types and nothing of
/// those types natively, with one scene reading `read`.
fn previewing_schema(schema: ProjectSchema, read: &str) -> App {
    let mut app = util::editor_test_app();
    {
        let native = jackdaw::project_types::native_type_paths(
            &app.world().resource::<AppTypeRegistry>().read(),
        );
        app.world_mut()
            .resource_mut::<ProjectTypes>()
            .update(&schema, &native);
    }
    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node::default(),
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new(read)],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, fill);
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    app
}

/// The one listed subject whose short name is `short_name`.
fn subject_named(app: &mut App, short_name: &str) -> jackdaw::preview_context::PreviewSubject {
    preview_context::preview_layout(app.world_mut())
        .into_iter()
        .find(|subject| subject.short_name == short_name)
        .unwrap_or_else(|| panic!("`{short_name}` is listed as a previewed subject"))
}

/// Every `(field path, control)` the panel actually built a scrub row for.
fn scrub_rows(app: &mut App) -> Vec<(String, String)> {
    app.world_mut()
        .spawn(Node::default())
        .with_children(preview_context::build_preview_context_panel);
    app.update();
    app.world_mut()
        .query::<&PreviewField>()
        .iter(app.world())
        .map(|field| (field.type_path.clone(), field.field.clone()))
        .collect()
}

/// The panel offers a control for a type only if that control can actually put
/// a value into the field and get it back out. A row that advertises a number
/// and drops it is worse than no row: the scene stops matching the panel and
/// nothing says so.
#[test]
fn every_advertised_field_is_writable_and_readable_back() {
    let mut app = previewing("EveryScalar.0", |app| {
        app.register_type::<EveryScalar>();
    });
    let subject = subject_named(&mut app, "EveryScalar");
    let advertised: Vec<(String, PreviewFieldKind)> = subject
        .fields
        .iter()
        .map(|field| (field.path.clone(), field.kind))
        .collect();
    let expected: Vec<(String, PreviewFieldKind)> = (0..NUMERIC_WIDTHS)
        .map(|index| (format!(".{index}"), PreviewFieldKind::Number))
        .chain([
            (format!(".{NUMERIC_WIDTHS}"), PreviewFieldKind::Bool),
            (format!(".{}", NUMERIC_WIDTHS + 1), PreviewFieldKind::Text),
        ])
        .collect();
    assert_eq!(
        advertised, expected,
        "the panel advertises a control for every width on the shared list, and nothing else",
    );

    for (path, kind) in advertised {
        let field = PreviewField::new(subject.type_path.clone(), path.clone());
        let written = match kind {
            PreviewFieldKind::Number => PreviewValue::Number(7.0),
            PreviewFieldKind::Bool => PreviewValue::Bool(true),
            PreviewFieldKind::Text => PreviewValue::Text("scrubbed".to_string()),
            other => panic!("`{path}` is a scalar but the panel advertised {other:?}"),
        };
        preview_context::write_scratch_field(app.world_mut(), &field, written.clone())
            .unwrap_or_else(|error| panic!("`{path}` is advertised but not writable: {error}"));
        assert_eq!(
            preview_context::read_scratch_value(app.world(), &field),
            Some(written),
            "`{path}` is advertised but does not read back what it was given",
        );
    }
}

/// A tuple struct's elements have no names, so the panel names each row by its
/// index.
#[test]
fn a_tuple_struct_field_renders_as_a_row() {
    let mut app = previewing("Health.0", |app| {
        app.register_type::<Health>();
    });
    let subject = subject_named(&mut app, "Health");
    assert_eq!(
        subject
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.path.as_str(), field.kind))
            .collect::<Vec<_>>(),
        vec![(".0", ".0", PreviewFieldKind::Number)],
        "the one element of a tuple struct is a row, named by its index",
    );

    let field = PreviewField::new(subject.type_path.clone(), ".0");
    preview_context::write_scratch_field(app.world_mut(), &field, PreviewValue::Number(4.0))
        .expect("a tuple element is writable");
    assert_eq!(
        preview_context::read_scratch_value(app.world(), &field),
        Some(PreviewValue::Number(4.0)),
    );
    assert!(
        scrub_rows(&mut app).iter().any(|(_, path)| path == ".0"),
        "and the panel builds a control for it",
    );
}

/// A math vector field becomes one row of axis inputs, each writing its own
/// sub-path, rather than a single row with no control.
#[test]
fn a_vector_field_scrubs_one_axis_at_a_time() {
    let mut app = previewing("Transform.translation.x", |_| {});
    let subject = subject_named(&mut app, "Transform");
    assert_eq!(
        subject
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.kind))
            .collect::<Vec<_>>(),
        vec![
            ("translation", PreviewFieldKind::Vector3),
            ("rotation", PreviewFieldKind::Vector4),
            ("scale", PreviewFieldKind::Vector3),
        ],
        "a math vector asks for one row of axis inputs, as the inspector draws it",
    );

    let field = PreviewField::new(subject.type_path.clone(), "translation.y");
    preview_context::write_scratch_field(app.world_mut(), &field, PreviewValue::Number(3.0))
        .expect("an axis of a vector field is writable");
    assert_eq!(
        preview_context::read_scratch_value(app.world(), &field),
        Some(PreviewValue::Number(3.0)),
    );

    let rows = scrub_rows(&mut app);
    let paths: Vec<&str> = rows.iter().map(|(_, path)| path.as_str()).collect();
    for axis in [
        "translation.x",
        "translation.y",
        "translation.z",
        "rotation.w",
        "scale.x",
    ] {
        assert!(
            paths.contains(&axis),
            "the panel builds an input per axis; `{axis}` missing from {paths:?}",
        );
    }
}

/// A resource read is listed and scrubbed like a component read. A scene whose
/// bindings read only resources would otherwise have nothing in the panel.
#[test]
fn a_resource_read_previews_like_a_component() {
    let mut app = previewing("Res(CharCreateForm).class", |app| {
        app.register_type::<CharCreateForm>();
    });
    let subject = subject_named(&mut app, "CharCreateForm");
    assert_eq!(subject.availability, PreviewAvailability::Native);
    assert_eq!(
        subject
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.kind))
            .collect::<Vec<_>>(),
        vec![
            ("name", PreviewFieldKind::Text),
            ("class", PreviewFieldKind::Number),
            ("class_name", PreviewFieldKind::Text),
        ],
    );

    let field = PreviewField::new(subject.type_path.clone(), "class");
    preview_context::write_scratch_field(app.world_mut(), &field, PreviewValue::Number(2.0))
        .expect("a previewed resource field is writable");
    assert_eq!(
        preview_context::read_scratch_value(app.world(), &field),
        Some(PreviewValue::Number(2.0)),
    );
}

/// Teardown order: the stand-in is only a resource for as long as the session
/// runs, and taking it back out has to clear bevy's resource cache before the
/// entity behind it goes. A despawn while it still counts as the resource
/// entity queues a removal against an entity that is already gone.
#[test]
fn stopping_preview_takes_the_stand_in_resource_back_out() {
    let mut app = previewing("Res(CharCreateForm).class", |app| {
        app.register_type::<CharCreateForm>();
    });
    assert!(
        app.world().get_resource::<CharCreateForm>().is_some(),
        "the session stood one up to read",
    );
    let before = live_entities(&mut app);

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert!(
        app.world().get_resource::<CharCreateForm>().is_none(),
        "the stand-in resource leaves with the session",
    );
    assert!(
        live_entities(&mut app) < before,
        "and so does the entity it was standing on",
    );

    // Not a one-way door: the next session stands a fresh one up, which is
    // only possible because the cache was cleared rather than left pointing at
    // a dead entity.
    preview_context::set_preview(app.world_mut(), true);
    app.update();
    assert!(
        app.world().get_resource::<CharCreateForm>().is_some(),
        "a second session gets its own stand-in",
    );
}

/// The other end of the same rule: if the editor itself stands the resource up
/// while a session is running, that value is not the session's to drop.
#[test]
fn a_resource_the_editor_claims_mid_session_survives_the_teardown() {
    let mut app = previewing("Res(CharCreateForm).class", |app| {
        app.register_type::<CharCreateForm>();
    });
    let stand_in = app
        .world()
        .resource_entities()
        .iter()
        .find(|(id, _)| {
            app.world()
                .components()
                .get_id(std::any::TypeId::of::<CharCreateForm>())
                == Some(*id)
        })
        .map(|(_, entity)| entity)
        .expect("the session stood one up");

    // The editor takes the resource over: the session's entity stops being the
    // one the world calls the resource entity.
    app.world_mut()
        .entity_mut(stand_in)
        .remove::<bevy::ecs::resource::IsResource>();
    app.world_mut().flush();
    app.world_mut().insert_resource(CharCreateForm {
        name: "Editor".to_string(),
        class: 9,
        class_name: "Warden".to_string(),
    });

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    assert_eq!(
        app.world()
            .get_resource::<CharCreateForm>()
            .map(|f| f.class),
        Some(9),
        "the editor's own resource is left exactly where it was",
    );
    assert!(
        app.world().get_entity(stand_in).is_err(),
        "and the session still cleans up the entity it spawned",
    );
}

/// How many entities are actually alive, which is not what `Entities::len`
/// reports: indices are allocated in blocks.
fn live_entities(app: &mut App) -> usize {
    app.world_mut().query::<Entity>().iter(app.world()).count()
}

/// A resource the editor already holds is real editor state. Scrubbing it
/// would move the editor, so the row says so instead of offering a control.
#[test]
fn a_resource_the_editor_owns_is_not_stood_in_for() {
    let mut app = previewing("Res(CharCreateForm).class", |app| {
        app.register_type::<CharCreateForm>();
        app.insert_resource(CharCreateForm {
            name: "Editor".to_string(),
            class: 9,
            class_name: "Warden".to_string(),
        });
    });
    let subject = subject_named(&mut app, "CharCreateForm");
    assert_eq!(subject.availability, PreviewAvailability::EditorOwned);
    assert!(!subject.note.is_empty(), "and it says why");
    assert!(
        scrub_rows(&mut app).is_empty(),
        "no control is offered for editor state",
    );
    assert_eq!(
        app.world().resource::<CharCreateForm>().class,
        9,
        "and the editor's own value is untouched",
    );

    // Read-only is not the same as unknown: the value is really there, so the
    // row shows it rather than the placeholder a schema-only row wears.
    let shown: Vec<String> = app
        .world_mut()
        .query::<(&preview_context::PreviewDisabledField, &Text)>()
        .iter(app.world())
        .map(|(_, text)| text.0.clone())
        .collect();
    assert!(
        shown.contains(&"9".to_string()) && shown.contains(&"Warden".to_string()),
        "the row reads the live value out of the editor's resource; got {shown:?}",
    );

    let field = PreviewField::new(subject.type_path.clone(), "class");
    assert_eq!(
        preview_context::write_scratch_field(app.world_mut(), &field, PreviewValue::Number(2.0)),
        Err(preview_context::PreviewError::EditorOwned(
            subject.type_path.clone()
        )),
        "and a write aimed at it by hand is refused, not silently applied",
    );
}

/// The other half of that rule, on the side the panel does not control: a
/// running preview cannot write the editor's resource from a binding either. A
/// write path names a component on the widget or it does not resolve, which is
/// why the session's guard list is built from component paths alone. The
/// two-way write-back that does reach resources runs from observers the editor
/// does not register.
#[test]
fn a_previewing_scene_cannot_write_a_resource_the_editor_holds() {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();
    app.register_type::<CharCreateForm>();
    app.insert_resource(CharCreateForm {
        name: "Editor".to_string(),
        class: 9,
        class_name: "Warden".to_string(),
    });

    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    // Two bindings off one read. The second is the control: its width really
    // moves, so the first one's failure is the write it names and not a read
    // that never resolved.
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node::default(),
            ChildOf(root),
            Bindings(vec![
                Binding::Field {
                    read: vec![BindPath::new("Vitals.ratio")],
                    via: None,
                    write: BindPath::new("Res(CharCreateForm).class"),
                    as_percent: false,
                },
                Binding::Field {
                    read: vec![BindPath::new("Vitals.ratio")],
                    via: None,
                    write: BindPath::new("Node.width"),
                    as_percent: true,
                },
            ]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, fill);

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, 0.5);
    app.update();

    assert_eq!(
        width(&app, fill),
        Val::Percent(50.0),
        "the read half really resolved and drove a write, so what fails below \
         is the resource path and not the binding as a whole",
    );
    let failures = &app.world().resource::<BindFailures>().0;
    assert!(
        !failures.contains(&(fill, 1)),
        "the control binding is not among the failures",
    );
    assert!(
        failures.contains(&(fill, 0)),
        "and the resource write is refused by name rather than missed by luck",
    );
    assert_eq!(
        app.world().resource::<CharCreateForm>().class,
        9,
        "a preview session must not move the editor's own state",
    );
}

/// The same rule on the path that really can reach a resource. A two-way
/// `Value` binding is written back by `jackdaw_bind`'s `ValueChange`
/// observers, which go through `write_source_path` and resolve `Res(...)` for
/// real. The editor does not register those observers, so a widget edit during
/// preview moves nothing; registering them without giving the session a
/// resource guard fails this test.
///
/// The field this aims at is an `f32`. `set_target` writes `Val`, `f32`,
/// `bool`, `String` and `Visibility` and refuses everything else, so a binding
/// pointed at an integer field is turned away by the type before any guard is
/// consulted. The `u32` field is asserted below as the type-mismatch case.
#[test]
fn a_widget_edit_during_preview_cannot_write_a_resource_the_editor_holds() {
    let mut app = util::editor_test_app();
    app.register_type::<EditorTuning>();
    app.register_type::<CharCreateForm>();
    app.insert_resource(EditorTuning { volume: 9.0 });
    app.insert_resource(CharCreateForm {
        name: "Editor".to_string(),
        class: 9,
        class_name: "Warden".to_string(),
    });

    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let volume = world
        .spawn((
            Name::new("Volume"),
            Node::default(),
            bevy::ui_widgets::SliderValue(9.0),
            ChildOf(root),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(EditorTuning).volume"),
                two_way: true,
            }]),
        ))
        .id();
    let class = world
        .spawn((
            Name::new("Class"),
            Node::default(),
            bevy::ui_widgets::SliderValue(9.0),
            ChildOf(root),
            Bindings(vec![Binding::Value {
                with: BindPath::new("Res(CharCreateForm).class"),
                two_way: true,
            }]),
        ))
        .id();
    for entity in [root, volume, class] {
        jackdaw::scene_io::register_entity_in_ast(world, entity);
    }

    preview_context::set_preview(app.world_mut(), true);
    app.update();

    // The gesture a user makes on the widget itself, not through the panel.
    for source in [volume, class] {
        app.world_mut().trigger(ValueChange::<f32> {
            source,
            value: 2.0,
            is_final: true,
        });
    }
    app.update();
    app.update();

    assert_eq!(
        app.world().resource::<EditorTuning>().volume,
        9.0,
        "a widget edit inside a preview session must not move the editor's own \
         resource: the session has no snapshot of it and never offered the user \
         a control over it",
    );
    assert_eq!(
        app.world().resource::<CharCreateForm>().class,
        9,
        "the integer field is unmoved as well, though the write path's own type \
         support would stop that one either way",
    );
}

/// The reason a section is inert has to reach the user through the same badge
/// the bindings card uses, not only as a line of body text under the rows.
#[test]
fn a_degraded_section_carries_its_reason_as_a_badge() {
    let mut app = previewing_schema(
        ProjectSchema {
            components: vec![schema_type(
                "demo_game::Charge",
                TypeKind::TupleStruct,
                &[("0", "f32")],
            )],
            ..default()
        },
        "demo_game::Charge.0",
    );

    let charge = subject_named(&mut app, "Charge");
    assert_eq!(
        charge
            .fields
            .iter()
            .map(|field| field.path.as_str())
            .collect::<Vec<_>>(),
        vec![".0"],
        "a schema'd tuple struct reports its index as a reflect path",
    );

    app.world_mut()
        .spawn(Node::default())
        .with_children(preview_context::build_preview_context_panel);
    app.update();
    // The pairing is the assertion: `Hovered` is opt-in and the tooltip
    // renderer reads `(Entity, &Tooltip, &Hovered)`, so a badge that carries
    // only the `Tooltip` is one nobody can ever see.
    let badges: Vec<String> = app
        .world_mut()
        .query::<(&jackdaw_feathers::tooltip::Tooltip, &Hovered)>()
        .iter(app.world())
        .map(|(tooltip, _)| tooltip.title.clone())
        .collect();
    assert!(
        badges.iter().any(|title| title.contains("PIE")),
        "the degraded section carries the reason as a badge the renderer picks up; got {badges:?}",
    );
}

/// A section with no rows under it carries a reason as much as a disabled one
/// does.
#[test]
fn a_linked_type_with_no_fields_says_why_it_has_no_rows() {
    let mut app = previewing("Beacon.anything", |app| {
        app.register_type::<Beacon>();
    });
    let subject = subject_named(&mut app, "Beacon");
    assert_eq!(subject.availability, PreviewAvailability::Native);
    assert!(subject.fields.is_empty(), "there is nothing to scrub");
    assert!(
        !subject.note.is_empty(),
        "but the header does not stand there unexplained",
    );

    app.world_mut()
        .spawn(Node::default())
        .with_children(preview_context::build_preview_context_panel);
    app.update();
    let notes: Vec<String> = app
        .world_mut()
        .query::<&Text>()
        .iter(app.world())
        .map(|text| text.0.clone())
        .collect();
    assert!(
        notes.contains(&subject.note),
        "and the reason is drawn under it; got {notes:?}",
    );
}

/// The other way a linked type ends up with no rows: the editor could not build
/// a value at all. `Locked` has a field, so the no-fields reason would be the
/// wrong one to show.
#[test]
fn a_linked_type_the_editor_cannot_build_says_that_instead() {
    let mut app = previewing("Locked.seal", |app| {
        app.register_type::<Sealed>();
        app.register_type::<Locked>();
    });
    let subject = subject_named(&mut app, "Locked");
    assert_eq!(subject.availability, PreviewAvailability::Native);
    assert!(subject.fields.is_empty());
    assert_eq!(
        subject.note, "the editor cannot build a value of this type",
        "the reason names the build, not the shape",
    );

    let mut beacon = previewing("Beacon.anything", |app| {
        app.register_type::<Beacon>();
    });
    assert_ne!(
        subject_named(&mut beacon, "Beacon").note,
        subject.note,
        "and the two empty sections do not tell the same story",
    );
}

/// The schema names a tuple element by its bare index, so the panel has to tell
/// an index from a name: a field called `x2` or `2x` is a name, and treating it
/// as an index would build the path `.2x`, which resolves to nothing.
#[test]
fn only_an_all_digit_schema_field_is_read_as_a_tuple_index() {
    let mut app = previewing_schema(
        ProjectSchema {
            components: vec![schema_type(
                "demo_game::Mixed",
                TypeKind::Struct,
                &[
                    ("0", "f32"),
                    ("1", "u8"),
                    ("x2", "f32"),
                    ("2x", "f32"),
                    ("hp", "f32"),
                ],
            )],
            ..default()
        },
        "demo_game::Mixed.0",
    );
    let subject = subject_named(&mut app, "Mixed");
    assert_eq!(
        subject
            .fields
            .iter()
            .map(|field| field.path.as_str())
            .collect::<Vec<_>>(),
        vec![".0", ".1", "x2", "2x", "hp"],
        "only an all-digit name is an index; everything else keeps its own name",
    );
}

/// The binding picker offers `Res(T)` straight out of the schema, so a
/// schema-only resource read is the ordinary case before Play. It resolves
/// against the project's resources; looked up among its components it would
/// resolve to a type nothing knows and render as an error.
#[test]
fn a_schema_only_resource_previews_as_disabled_rows() {
    let mut app = previewing_schema(
        ProjectSchema {
            resources: vec![schema_type(
                "demo_game::CharCreateForm",
                TypeKind::Struct,
                &[("name", "alloc::string::String"), ("class", "u32")],
            )],
            ..default()
        },
        "Res(demo_game::CharCreateForm).class",
    );

    let subject = subject_named(&mut app, "CharCreateForm");
    assert_eq!(
        subject.availability,
        PreviewAvailability::SchemaOnly,
        "the editor knows the shape, it just cannot build one until the game runs",
    );
    assert_eq!(
        subject
            .fields
            .iter()
            .map(|field| (field.path.as_str(), field.kind))
            .collect::<Vec<_>>(),
        vec![
            ("name", PreviewFieldKind::Text),
            ("class", PreviewFieldKind::Number),
        ],
        "and it shows the schema's own fields",
    );
    assert!(
        subject.note.contains("PIE"),
        "under the reason it cannot drive them; got {:?}",
        subject.note,
    );
}

// ---------------------------------------------------------------------------
// A preview never reaches disk, including through the asset pass
// ---------------------------------------------------------------------------

/// A previewed component that also carries an asset handle.
///
/// The emitter re-derives every handle-bearing patch from the live world so the
/// handle field can be spelled with an asset context, and that re-derivation is
/// the one path a previewed value could ride to disk on. A component with no
/// handle never gets re-derived.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Portrait {
    image: Handle<Image>,
    opacity: f32,
}

/// What the author put in the file.
const AUTHORED_OPACITY: f32 = 0.25;
/// What the preview scrubs it to, which must never be written anywhere.
const PREVIEWED_OPACITY: f64 = 0.75;

struct PortraitScene {
    app: App,
    /// The scene file the tab saves to.
    path: std::path::PathBuf,
    /// Kept so the temp directory outlives the test.
    _tmp: tempfile::TempDir,
}

/// A one-entity UI scene whose `Portrait.opacity` is driven by `Vitals.ratio`,
/// registered in the document and pointed at a real file on disk.
fn portrait_scene() -> PortraitScene {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();
    app.register_type::<Portrait>();

    let tmp = tempfile::tempdir().expect("a temp dir");
    let path = tmp.path().join("portrait.bsn");

    let world = app.world_mut();
    let image: Handle<Image> = world.resource::<AssetServer>().load("portrait.png");
    let root = world
        .spawn((
            Name::new("Portrait Root"),
            UiSceneRoot::default(),
            Node::default(),
            Portrait {
                image,
                opacity: AUTHORED_OPACITY,
            },
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Vitals.ratio")],
                via: None,
                write: BindPath::new("Portrait.opacity"),
                as_percent: false,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);

    let mut tab = jackdaw::scenes::SceneTab::new_untitled(1);
    tab.path = Some(path.clone());
    tab.display_name = "portrait".to_string();
    world
        .resource_mut::<jackdaw::scenes::Scenes>()
        .push_tab(tab);
    world
        .resource_mut::<jackdaw::scene_io::SceneFilePath>()
        .path = Some(path.to_string_lossy().into_owned());

    app.update();
    PortraitScene {
        app,
        path,
        _tmp: tmp,
    }
}

fn opacity(app: &App, entity: Entity) -> f32 {
    app.world()
        .get::<Portrait>(entity)
        .expect("the portrait component")
        .opacity
}

fn portrait_root(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<(Entity, &Name), With<Portrait>>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Portrait Root")
        .map(|(entity, _)| entity)
        .expect("the portrait root")
}

#[test]
fn a_scrubbed_value_on_a_handle_bearing_component_never_reaches_disk() {
    let PortraitScene {
        mut app,
        path,
        _tmp,
    } = portrait_scene();
    let root = portrait_root(&mut app);

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, PREVIEWED_OPACITY);
    app.update();

    // The preview has to actually be driving the property, or the save below
    // proves nothing.
    assert!(
        (f64::from(opacity(&app, root)) - PREVIEWED_OPACITY).abs() < 1e-6,
        "the preview drives the component the emitter re-derives; live value is {}",
        opacity(&app, root),
    );

    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves",
    );

    let on_disk = std::fs::read_to_string(&path).expect("the scene is on disk");
    assert!(
        on_disk.contains("0.25"),
        "the file has to hold what the author wrote; disk holds:\n{on_disk}",
    );
    assert!(
        !on_disk.contains("0.75"),
        "and never what the preview was showing; disk holds:\n{on_disk}",
    );

    // The save is an interruption, not an end: the session carries on showing
    // what it was showing.
    assert!(
        (f64::from(opacity(&app, root)) - PREVIEWED_OPACITY).abs() < 1e-6,
        "the preview is still running after the save; live value is {}",
        opacity(&app, root),
    );
}

#[test]
fn a_scrubbed_value_on_a_handle_bearing_component_survives_a_tab_switch() {
    let PortraitScene { mut app, .. } = portrait_scene();
    let root = portrait_root(&mut app);

    // A second tab to switch to. Nothing needs to be in it; the point is that
    // leaving the first one captures it.
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .push_tab(jackdaw::scenes::SceneTab::new_untitled(2));

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    scrub_ratio(&mut app, PREVIEWED_OPACITY);
    app.update();
    assert!(
        (f64::from(opacity(&app, root)) - PREVIEWED_OPACITY).abs() < 1e-6,
        "the preview drives the property before the switch",
    );

    // Leaving the tab captures it, which is a save in everything but name.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    app.update();

    // Stop previewing before coming back, so what the tab hands over is only
    // what the capture stored, not something the evaluator re-derived.
    preview_context::set_preview(app.world_mut(), false);
    app.update();
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);
    app.update();

    let root = portrait_root(&mut app);
    assert!(
        (opacity(&app, root) - AUTHORED_OPACITY).abs() < 1e-6,
        "a tab switch mid-preview stores the authored value, not the scrubbed \
         one; the tab came back holding {}",
        opacity(&app, root),
    );
}
