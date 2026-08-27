//! The `Bindings` inspector card.
//!
//! A binding names two places in the game: read this, write that. These tests
//! drive the card the way the inspector builds it, through a real selection
//! on a real scene document, and pin what each control has to do: render one
//! row per binding, commit a whole `Bindings` value per edit as one undoable
//! entry, write FULL type paths into the document, badge a path that names
//! nothing, and keep the authored order.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use jackdaw::inspector::bindings_card::{
    AddBindingMenu, BindControl, BindKind, BindingControl, BindingRowLabel, BindingsCardBody,
    PathSlot,
};
use jackdaw::project_types::ProjectTypes;
use jackdaw::selection::Selection;
use jackdaw_bind::{BindPath, Binding, Bindings};
use jackdaw_bsn::{BsnValue, SceneBsnAst};
use jackdaw_commands::CommandHistory;
use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, EditorComboBox};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::variant_edit::VariantEditConfig;
use jackdaw_schema::{
    ArgOwnership, FieldSchema, FunctionSchema, ProjectSchema, TypeKind, TypeSchema,
};

mod util;

const BINDINGS: &str = "jackdaw_bind::types::Bindings";

// ---------------------------------------------------------------------------
// A project schema to pick from
// ---------------------------------------------------------------------------

fn field(name: &str, type_path: &str) -> FieldSchema {
    FieldSchema {
        name: name.to_string(),
        type_path: type_path.to_string(),
    }
}

fn schema_type(type_path: &str, kind: TypeKind, fields: Vec<FieldSchema>) -> TypeSchema {
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
        fields,
        kind,
        default: None,
        variants: Vec::new(),
        entity_fields: Vec::new(),
        fills_gaps: true,
    }
}

/// The fixture project: one component, one resource, three events (one of
/// them an enum the dispatcher cannot fill, one of them unable to fill its
/// own gaps), and two functions, only one of which a binding can call.
fn fixture_schema() -> ProjectSchema {
    let mut fired = schema_type(
        "demo_game::Fired",
        TypeKind::Struct,
        vec![
            field("amount", "f32"),
            field("entity", "bevy_ecs::entity::Entity"),
        ],
    );
    fired.entity_fields = vec!["entity".to_string()];

    let mut strict = schema_type(
        "demo_game::Strict",
        TypeKind::Struct,
        vec![field("amount", "f32")],
    );
    strict.fills_gaps = false;

    // Two of the game's own markers: one reflection can build, one it cannot.
    // Only the first is a thing a binding can put on a widget.
    let flagged = schema_type("demo_game::Flagged", TypeKind::Struct, Vec::new());
    let mut unbuildable = schema_type("demo_game::Unbuildable", TypeKind::Struct, Vec::new());
    unbuildable.fills_gaps = false;

    // In both buckets, which is what an extraction reports: bevy's
    // `Resource: Component` supertrait puts a `ReflectComponent` on every
    // reflected resource, so the two lists overlap by construction. What the
    // suite proves about that is that the card tolerates the overlap; no test
    // here counts either picker's entries.
    let audio = schema_type(
        "demo_game::AudioSettings",
        TypeKind::Struct,
        vec![field("master", "f32")],
    );

    ProjectSchema {
        components: vec![
            schema_type(
                "demo_game::Health",
                TypeKind::Struct,
                vec![field("current", "f32"), field("max", "f32")],
            ),
            flagged,
            unbuildable,
            audio.clone(),
        ],
        resources: vec![audio],
        events: vec![
            fired,
            strict,
            schema_type("demo_game::Mode", TypeKind::Enum, Vec::new()),
        ],
        functions: vec![
            FunctionSchema {
                name: "demo_game::ratio".to_string(),
                arg_type_paths: vec!["f32".to_string(), "f32".to_string()],
                arg_ownerships: vec![ArgOwnership::Owned, ArgOwnership::Owned],
                return_type_path: "f32".to_string(),
                return_ownership: ArgOwnership::Owned,
                docs: None,
            },
            FunctionSchema {
                name: "demo_game::half".to_string(),
                arg_type_paths: vec!["f32".to_string()],
                arg_ownerships: vec![ArgOwnership::Owned],
                return_type_path: "f32".to_string(),
                return_ownership: ArgOwnership::Owned,
                docs: None,
            },
            // Borrows its argument: however it is spelled in a picker the
            // evaluator cannot call it, so it must never be offered.
            FunctionSchema {
                name: "demo_game::peek".to_string(),
                arg_type_paths: vec!["f32".to_string()],
                arg_ownerships: vec![ArgOwnership::Ref],
                return_type_path: "f32".to_string(),
                return_ownership: ArgOwnership::Owned,
                docs: None,
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A selected, document-tracked entity carrying `bindings`, with an
/// inspector mounted and the fixture schema loaded, so the card is built by
/// the real dispatch rather than called directly.
fn app_with_bindings(bindings: Bindings) -> (App, Entity) {
    app_with_bindings_and_project(bindings, &[])
}

/// The same, with `project_components` authored on the node: schema types the
/// document carries but the editor has no ECS registration for, which is what
/// every project component is.
fn app_with_bindings_and_project(bindings: Bindings, project_components: &[&str]) -> (App, Entity) {
    let mut app = util::editor_test_app();
    {
        let native = jackdaw::project_types::native_type_paths(
            &app.world().resource::<AppTypeRegistry>().read(),
        );
        app.world_mut()
            .resource_mut::<ProjectTypes>()
            .update(&fixture_schema(), &native);
    }
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((Name::new("hud"), Node::default(), bindings))
        .id();
    // The document is the point: an authored entity is what the card has to
    // work against, and it is also what puts `Bindings` in the inspector's
    // authored-type filter.
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    for type_path in project_components {
        author_project_component(&mut app, entity, type_path);
    }
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    app.update();
    app.update();
    (app, entity)
}

/// Put a project component's patch on the entity's document node. The
/// editor never registers a project type as a real ECS component (its code
/// lives in the game binary) so the document is the only place it exists.
fn author_project_component(app: &mut App, entity: Entity, type_path: &str) {
    let mut ast = app.world_mut().resource_mut::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the entity is in the document");
    let patch = ast
        .world
        .spawn(jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
            type_path: type_path.to_string(),
            fields: jackdaw_bsn::BsnStructFields(Vec::new()),
        }))
        .id();
    if let Some(patches) = ast.get_patches_mut(node) {
        patches.0.push(patch);
    }
}

fn card_body(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<BindingsCardBody>>()
        .iter(app.world())
        .next()
        .expect("a Bindings selection builds the bindings card")
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
            let mut kids: Vec<Entity> = children.iter().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    out
}

/// Every control the card built, as (entity, binding index, control).
fn controls(app: &mut App) -> Vec<(Entity, usize, BindControl)> {
    app.world_mut()
        .query::<(Entity, &BindingControl)>()
        .iter(app.world())
        .map(|(entity, control)| (entity, control.binding(), control.control()))
        .collect()
}

fn control(app: &mut App, binding: usize, wanted: BindControl) -> Entity {
    controls(app)
        .into_iter()
        .find(|(_, index, control)| *index == binding && *control == wanted)
        .map(|(entity, _, _)| entity)
        .unwrap_or_else(|| panic!("no {wanted:?} control on binding {binding}"))
}

fn pick(app: &mut App, combo: Entity, selected: usize, label: &str, value: Option<&str>) {
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo,
        selected,
        label: label.to_string(),
        value: value.map(str::to_string),
    });
    app.update();
    app.update();
}

fn click(app: &mut App, button: Entity) {
    app.world_mut().trigger(ButtonClickEvent { entity: button });
    app.update();
    app.update();
}

fn live(app: &App, entity: Entity) -> Bindings {
    app.world()
        .get::<Bindings>(entity)
        .cloned()
        .expect("the entity still carries its bindings")
}

/// The `Bindings` value the scene document holds, as its list of authored
/// binding values.
fn authored_bindings(app: &App, entity: Entity) -> Vec<BsnValue> {
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the entity is in the document");
    let patch = ast
        .find_patch_by_type_path(node, BINDINGS)
        .and_then(|pe| ast.get_patch(pe))
        .expect("the document holds a Bindings patch");
    let jackdaw_bsn::BsnPatch::TupleStruct(data) = patch else {
        panic!("Bindings authors as a newtype patch, got {patch:?}");
    };
    match data.values.first() {
        Some(BsnValue::List(items)) => items.clone(),
        other => panic!("Bindings holds a list of bindings, got {other:?}"),
    }
}

/// Undo the last edit, then run the card's change-detection pass. That pass
/// is scheduled behind `AppState::Editor`, which a headless test never
/// enters, so it is driven directly; `run_system_cached` keeps its change
/// tick across calls, exactly as the scheduled system would.
/// One named field of a binding as the document holds it.
fn authored_field(binding: &BsnValue, name: &str) -> BsnValue {
    let BsnValue::Struct(data) = binding else {
        panic!("a data-carrying binding authors as a struct, got {binding:?}");
    };
    data.fields
        .0
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value.clone())
        .unwrap_or_else(|| panic!("no `{name}` in the authored binding: {data:?}"))
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    app.update();
    refresh_card(app);
}

fn refresh_card(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::inspector::bindings_card::refresh_bindings_card_on_change)
        .expect("the card's change-detection pass runs");
    app.update();
    app.update();
}

/// Every binding row's summary line, in card order.
fn row_labels(app: &mut App) -> Vec<String> {
    let body = card_body(app);
    descendants(app.world_mut(), body)
        .into_iter()
        .filter(|entity| app.world().get::<BindingRowLabel>(*entity).is_some())
        .filter_map(|entity| app.world().get::<Text>(entity).map(|text| text.0.clone()))
        .collect()
}

fn read_path(binding: &Binding, index: usize) -> String {
    match binding {
        Binding::Field { read, .. } => read[index].raw.clone(),
        Binding::Text { args, .. } => args[index].raw.clone(),
        Binding::Visible { read, .. } => read.raw.clone(),
        Binding::Value { with, .. } => with.raw.clone(),
        Binding::Action { fields, .. } => fields[index].1.raw.clone(),
    }
}

fn write_path(binding: &Binding) -> String {
    match binding {
        Binding::Field { write, .. } => write.raw.clone(),
        other => panic!("not a Field binding: {other:?}"),
    }
}

fn field_binding(read: &str, write: &str) -> Binding {
    Binding::Field {
        read: vec![BindPath::new(read)],
        via: None,
        write: BindPath::new(write),
        as_percent: false,
    }
}

/// The card takes the component's name from `Bindings` itself rather than
/// from a copy of its path, so moving the type inside the crate moves the
/// card and the document it writes along with it.
#[test]
fn the_card_names_the_component_by_the_path_the_type_declares() {
    let declared = <Bindings as bevy::reflect::TypePath>::type_path();
    assert_eq!(
        jackdaw::inspector::bindings_card::bindings_type_path(),
        declared,
        "the card asks the type for its path",
    );

    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "",
        "bevy_ui::ui_node::Node.width",
    )]));
    let combo = control(&mut app, 0, BindControl::PathType(PathSlot::Read(0)));
    pick(&mut app, combo, 0, "Health", Some("demo_game::Health"));

    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the entity is in the document");
    assert!(
        ast.find_patch_by_type_path(node, declared).is_some(),
        "the committed patch names `{declared}`",
    );
}

// ---------------------------------------------------------------------------
// One row per binding, showing its kind
// ---------------------------------------------------------------------------

#[test]
fn the_card_renders_one_row_per_binding_showing_its_kind() {
    let (mut app, _) = app_with_bindings(Bindings(vec![
        field_binding("demo_game::Health.current", "bevy_ui::ui_node::Node.width"),
        Binding::Text {
            format: "{}".to_string(),
            args: vec![BindPath::new("demo_game::Health.current")],
        },
        Binding::Visible {
            read: BindPath::new("demo_game::Health.current"),
            via: None,
        },
    ]));

    let _ = card_body(&mut app);
    let kinds: Vec<Entity> = controls(&mut app)
        .into_iter()
        .filter(|(_, _, control)| *control == BindControl::Kind)
        .map(|(entity, _, _)| entity)
        .collect();
    assert_eq!(kinds.len(), 3, "one kind picker per authored binding");

    let shown: Vec<String> = kinds
        .iter()
        .map(|entity| {
            let config = app
                .world()
                .get::<VariantEditConfig>(*entity)
                .expect("the kind picker is a variant edit");
            config.variants[config.selected_index].name.clone()
        })
        .collect();
    assert_eq!(
        shown,
        vec![
            "Field".to_string(),
            "Text".to_string(),
            "Visible".to_string()
        ],
        "each row names its own kind, in authored order",
    );
}

/// Every kind renders. A kind whose per-kind rows were never written would
/// show a bare header and no way to author anything.
#[test]
fn every_binding_kind_renders_its_own_controls() {
    let (mut app, _) = app_with_bindings(Bindings(vec![
        field_binding("demo_game::Health.current", "bevy_ui::ui_node::Node.width"),
        Binding::Text {
            format: "hp {}".to_string(),
            args: vec![BindPath::new("demo_game::Health.current")],
        },
        Binding::Visible {
            read: BindPath::new("demo_game::Health.current"),
            via: None,
        },
        Binding::Value {
            with: BindPath::new("Res(demo_game::AudioSettings).master"),
            two_way: true,
        },
        Binding::Action {
            event: "demo_game::Fired".to_string(),
            fields: Vec::new(),
        },
    ]));

    let _ = card_body(&mut app);
    let all = controls(&mut app);
    let has = |binding: usize, control: BindControl| {
        all.iter()
            .any(|(_, index, found)| *index == binding && *found == control)
    };

    assert!(
        has(0, BindControl::PathType(PathSlot::Read(0))),
        "Field read"
    );
    assert!(
        has(0, BindControl::PathType(PathSlot::Write)),
        "Field write"
    );
    assert!(has(0, BindControl::AsPercent), "Field as_percent");
    assert!(has(1, BindControl::Format), "Text format");
    assert!(has(1, BindControl::PathType(PathSlot::Read(0))), "Text arg");
    assert!(
        has(2, BindControl::PathType(PathSlot::Read(0))),
        "Visible read"
    );
    assert!(has(3, BindControl::TwoWay), "Value two_way");
    assert!(has(4, BindControl::Event), "Action event");
    assert!(
        has(4, BindControl::PathType(PathSlot::EventField)),
        "Action maps the event's own fields",
    );
}

/// A marker write puts a component on and takes it off again; there is no
/// number on the other end to scale, so the row that offers `as percent`
/// offers a control that cannot do anything. The summary already leaves the
/// suffix off such a row.
#[test]
fn a_marker_write_row_offers_no_percent_checkbox() {
    let marker = "bevy_ui::interaction_states::InteractionDisabled";
    let (mut app, _) = app_with_bindings(Bindings(vec![
        field_binding("demo_game::Health.current", marker),
        field_binding("demo_game::Health.current", "bevy_ui::ui_node::Node.width"),
    ]));

    let _ = card_body(&mut app);
    let all = controls(&mut app);
    let has_percent = |binding: usize| {
        all.iter()
            .any(|(_, index, found)| *index == binding && *found == BindControl::AsPercent)
    };

    assert!(
        !has_percent(0),
        "a write with no field means the whole component, which has no percentage",
    );
    assert!(
        has_percent(1),
        "a field write still scales, so the row that can use the control keeps it",
    );
}

// ---------------------------------------------------------------------------
// Add a binding
// ---------------------------------------------------------------------------

/// A gesture that arrives after the component it edits has gone. The card is
/// built from a snapshot, so its controls outlive a `Bindings` an undo or
/// another editor took off. The edit is refused whole, rather than leaving one
/// binding half written, and the refusal reaches the log rather than looking
/// like a control that does nothing.
#[test]
fn an_edit_aimed_at_a_component_that_is_gone_is_refused_whole() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "bevy_ui::ui_node::Node.width",
    )]));
    let history = app.world().resource::<CommandHistory>().undo_stack.len();
    let document = authored_bindings(&app, entity);

    let menu = app
        .world_mut()
        .query_filtered::<Entity, With<AddBindingMenu>>()
        .iter(app.world())
        .next()
        .expect("the card footer offers Add Binding");
    app.world_mut().entity_mut(entity).remove::<Bindings>();
    app.update();

    let visible = BindKind::ALL
        .iter()
        .position(|kind| *kind == BindKind::Visible)
        .expect("Visible is one of the five kinds");
    pick(&mut app, menu, visible, "Visible", None);

    assert!(
        app.world().get::<Bindings>(entity).is_none(),
        "the edit does not put the component back by halves",
    );
    assert_eq!(
        authored_bindings(&app, entity),
        document,
        "and the document is untouched",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        history,
        "a refused edit mints no history entry",
    );
}

#[test]
fn adding_a_binding_grows_the_document_in_one_undoable_entry() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "bevy_ui::ui_node::Node.width",
    )]));
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    let menu = app
        .world_mut()
        .query_filtered::<Entity, With<AddBindingMenu>>()
        .iter(app.world())
        .next()
        .expect("the card footer offers Add Binding");
    let visible = BindKind::ALL
        .iter()
        .position(|kind| *kind == BindKind::Visible)
        .expect("Visible is one of the five kinds");
    pick(&mut app, menu, visible, "Visible", None);

    assert_eq!(live(&app, entity).0.len(), 2, "the live component grew");
    assert!(
        matches!(live(&app, entity).0[1], Binding::Visible { .. }),
        "the added binding is the kind that was picked",
    );
    assert_eq!(
        authored_bindings(&app, entity).len(),
        2,
        "the document grew with it",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "one add is one history entry",
    );

    undo(&mut app);
    assert_eq!(
        live(&app, entity).0.len(),
        1,
        "undo takes the added binding back off",
    );
    assert_eq!(
        authored_bindings(&app, entity).len(),
        1,
        "and off the document too",
    );
}

/// Every kind has to be constructible from the footer: `Binding` has no
/// `Default`, so the card owns the five defaults and a missing one would be
/// a menu entry that adds nothing.
#[test]
fn the_footer_can_add_every_kind() {
    let (mut app, entity) = app_with_bindings(Bindings(Vec::new()));

    for (index, kind) in BindKind::ALL.iter().enumerate() {
        let menu = app
            .world_mut()
            .query_filtered::<Entity, With<AddBindingMenu>>()
            .iter(app.world())
            .next()
            .expect("the card footer offers Add Binding");
        pick(&mut app, menu, index, kind.label(), None);
        assert_eq!(
            live(&app, entity).0.len(),
            index + 1,
            "adding {kind:?} appended a binding",
        );
        assert_eq!(
            BindKind::of(&live(&app, entity).0[index]),
            *kind,
            "the appended binding is a {kind:?}",
        );
    }
}

// ---------------------------------------------------------------------------
// A picker writes the full type path
// ---------------------------------------------------------------------------

#[test]
fn a_component_pick_writes_the_full_type_path() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "",
        "bevy_ui::ui_node::Node.width",
    )]));

    let combo = control(&mut app, 0, BindControl::PathType(PathSlot::Read(0)));
    pick(&mut app, combo, 0, "Health", Some("demo_game::Health"));

    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "demo_game::Health.current",
        "the picker writes the full type path and lands on the type's first field",
    );

    let field_combo = control(&mut app, 0, BindControl::PathField(PathSlot::Read(0)));
    pick(&mut app, field_combo, 1, "max", Some("max"));
    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "demo_game::Health.max",
        "the field half of the picker keeps the full type path",
    );

    let authored = authored_bindings(&app, entity);
    let text = format!("{:?}", authored[0]);
    assert!(
        text.contains("demo_game::Health.max"),
        "the document holds the full path too: {text}",
    );
}

/// A resource read spells itself `Res(Type).field`, which is the only form
/// the resolver takes for one.
///
/// This pins the SPELLING, not the list. A duplicate in the resource dropdown
/// would leave the pick at this index answering the same way, so nothing here
/// counts what the picker offers.
#[test]
fn a_resource_pick_writes_the_res_form() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![Binding::Value {
        with: BindPath::new(""),
        two_way: false,
    }]));

    let source = control(&mut app, 0, BindControl::PathSource(PathSlot::Read(0)));
    pick(&mut app, source, 1, "Resource", None);

    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "Res(demo_game::AudioSettings).master",
        "a resource path carries the full type path inside Res()",
    );
}

// ---------------------------------------------------------------------------
// The error state
// ---------------------------------------------------------------------------

#[test]
fn a_path_naming_no_known_type_badges_the_row() {
    let (mut app, _) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Ghost.current",
        "bevy_ui::ui_node::Node.width",
    )]));

    let body = card_body(&mut app);
    let label = descendants(app.world_mut(), body)
        .into_iter()
        .find(|entity| app.world().get::<BindingRowLabel>(*entity).is_some())
        .expect("each binding row has a label");

    assert_eq!(
        app.world().get::<TextColor>(label).map(|color| color.0),
        Some(tokens::TEXT_ERROR),
        "a path that resolves against nothing paints its row label as an error",
    );
    assert_eq!(
        badge(&mut app, label).as_deref(),
        Some("unknown type 'demo_game::Ghost'"),
        "the tooltip is the binding error the runtime would raise",
    );
}

/// What a badge on `entity` says, read the way the renderer reads it.
///
/// The pairing is the assertion: `Hovered` is opt-in and the tooltip system
/// queries `(Entity, &Tooltip, &Hovered)`, so a `Tooltip` sitting on its own
/// is a badge nobody can ever see.
fn badge(app: &mut App, entity: Entity) -> Option<String> {
    let mut query = app.world_mut().query::<(&Tooltip, &Hovered)>();
    query
        .get(app.world(), entity)
        .ok()
        .map(|(tooltip, _)| tooltip.title.clone())
}

/// The card's own "add read" button is one click from a `Field` the
/// evaluator refuses, so the row says so on the spot rather than at Play
/// time, and it needs no schema to know it.
#[test]
fn a_second_read_with_no_via_badges_the_row() {
    let (mut app, _) = app_with_bindings(Bindings(vec![Binding::Field {
        read: vec![
            BindPath::new("demo_game::Health.current"),
            BindPath::new("demo_game::Health.max"),
        ],
        via: None,
        write: BindPath::new("bevy_ui::ui_node::Node.width"),
        as_percent: false,
    }]));

    let body = card_body(&mut app);
    let label = descendants(app.world_mut(), body)
        .into_iter()
        .find(|entity| app.world().get::<BindingRowLabel>(*entity).is_some())
        .expect("each binding row has a label");
    assert_eq!(
        badge(&mut app, label).as_deref(),
        Some("Field binding has 2 reads but no via function to combine them"),
        "two reads with nothing to combine them is the runtime's own complaint",
    );
}

/// The badge is a badge. A broken path still commits, because refusing to
/// save half-finished work is how an author loses it.
#[test]
fn a_badged_row_still_commits_its_edits() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Ghost.current",
        "bevy_ui::ui_node::Node.width",
    )]));

    let percent = control(&mut app, 0, BindControl::AsPercent);
    app.world_mut().trigger(bevy::ui_widgets::ValueChange {
        source: percent,
        value: true,
        is_final: true,
    });
    app.update();
    app.update();

    assert!(
        matches!(
            live(&app, entity).0[0],
            Binding::Field {
                as_percent: true,
                ..
            }
        ),
        "the edit landed even though the row is badged",
    );
}

/// A path nobody has picked yet is not an error; it is an empty field. Only
/// a path that names something and misses gets the red.
#[test]
fn an_unpicked_path_is_not_an_error() {
    let (mut app, _) = app_with_bindings(Bindings(vec![field_binding("", "")]));

    let body = card_body(&mut app);
    let label = descendants(app.world_mut(), body)
        .into_iter()
        .find(|entity| app.world().get::<BindingRowLabel>(*entity).is_some())
        .expect("each binding row has a label");
    assert_ne!(
        app.world().get::<TextColor>(label).map(|color| color.0),
        Some(tokens::TEXT_ERROR),
        "a freshly added binding is unfinished, not broken",
    );
}

// ---------------------------------------------------------------------------
// Reorder
// ---------------------------------------------------------------------------

#[test]
fn reordering_swaps_the_committed_order() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![
        field_binding("demo_game::Health.current", "bevy_ui::ui_node::Node.width"),
        field_binding("demo_game::Health.max", "bevy_ui::ui_node::Node.height"),
    ]));

    let down = control(&mut app, 0, BindControl::MoveDown);
    click(&mut app, down);

    let after = live(&app, entity);
    assert_eq!(read_path(&after.0[0], 0), "demo_game::Health.max");
    assert_eq!(read_path(&after.0[1], 0), "demo_game::Health.current");

    let authored = authored_bindings(&app, entity);
    assert!(
        format!("{:?}", authored[0]).contains("demo_game::Health.max"),
        "the document carries the new order, not just the live component",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        1,
        "one reorder is one history entry",
    );
}

#[test]
fn removing_a_binding_drops_it_from_the_document() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![
        field_binding("demo_game::Health.current", "bevy_ui::ui_node::Node.width"),
        field_binding("demo_game::Health.max", "bevy_ui::ui_node::Node.height"),
    ]));

    let remove = control(&mut app, 0, BindControl::Remove);
    click(&mut app, remove);

    assert_eq!(live(&app, entity).0.len(), 1);
    assert_eq!(authored_bindings(&app, entity).len(), 1);
    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "demo_game::Health.max",
        "the surviving binding is the one that was not removed",
    );
}

// ---------------------------------------------------------------------------
// Changes from outside the card
// ---------------------------------------------------------------------------

/// The card addresses every widget by index, so a value that moves behind
/// its back has to rebuild it. Undo is the everyday way that happens.
#[test]
fn an_undone_add_rebuilds_the_card() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "bevy_ui::ui_node::Node.width",
    )]));

    let menu = app
        .world_mut()
        .query_filtered::<Entity, With<AddBindingMenu>>()
        .iter(app.world())
        .next()
        .expect("the card footer offers Add Binding");
    pick(&mut app, menu, 0, "Field", None);
    assert_eq!(
        row_labels(&mut app).len(),
        2,
        "the card grew with the value"
    );

    undo(&mut app);

    assert_eq!(live(&app, entity).0.len(), 1);
    assert_eq!(
        row_labels(&mut app).len(),
        1,
        "an undone add takes its row off the card, not just its binding",
    );
}

/// A row's remove button remembers which read it stands for, and the
/// binding can lose reads before the click lands, through undo, a second
/// inspector, or the running game. A slot that is not there names no read, so
/// the click has to go nowhere rather than take the process down.
#[test]
fn removing_a_read_that_is_already_gone_does_nothing() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![Binding::Field {
        read: vec![
            BindPath::new("demo_game::Health.current"),
            BindPath::new("demo_game::Health.max"),
            BindPath::new("demo_game::Health.regen"),
        ],
        via: None,
        write: BindPath::new("bevy_ui::ui_node::Node.width"),
        as_percent: false,
    }]));
    let remove = control(&mut app, 0, BindControl::RemoveRead(2));

    // Shrink the binding behind the card's back, without an update, so the
    // button is still there when the click reaches it.
    app.world_mut()
        .get_mut::<Bindings>(entity)
        .expect("the entity carries its bindings")
        .0[0] = Binding::Field {
        read: vec![BindPath::new("demo_game::Health.current")],
        via: None,
        write: BindPath::new("bevy_ui::ui_node::Node.width"),
        as_percent: false,
    };

    click(&mut app, remove);

    let after = live(&app, entity);
    let Binding::Field { read, .. } = &after.0[0] else {
        panic!("the binding is still a Field");
    };
    assert_eq!(
        read.len(),
        1,
        "the stale click left the surviving read alone",
    );
}

/// The other half of the same system: the card's own commits already
/// rebuilt, so the change-detection pass must not rebuild them again. A
/// second rebuild would throw away every widget mid-gesture, the dropdown the
/// user is looking at included.
#[test]
fn the_card_does_not_rebuild_itself_for_its_own_edit() {
    let (mut app, _) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "bevy_ui::ui_node::Node.width",
    )]));

    let combo = control(&mut app, 0, BindControl::PathField(PathSlot::Read(0)));
    pick(&mut app, combo, 1, "max", Some("max"));

    let body = card_body(&mut app);
    let before: Vec<Entity> = descendants(app.world_mut(), body);
    refresh_card(&mut app);
    let after: Vec<Entity> = descendants(app.world_mut(), body);

    assert_eq!(
        before, after,
        "the pass left the card the edit already rebuilt exactly as it was",
    );
}

/// The sharp end of the same rule. After an undone reorder the rows and the
/// bindings would disagree about who is first, and the row drawn first would
/// quietly edit the other one.
#[test]
fn an_undone_reorder_re_indexes_the_rows() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![
        field_binding("demo_game::Health.current", "bevy_ui::ui_node::Node.width"),
        field_binding("demo_game::Health.max", "bevy_ui::ui_node::Node.height"),
    ]));
    let first = row_labels(&mut app);

    let down = control(&mut app, 0, BindControl::MoveDown);
    click(&mut app, down);
    assert_ne!(row_labels(&mut app), first, "the reorder redrew the rows");

    undo(&mut app);
    assert_eq!(
        row_labels(&mut app),
        first,
        "undo puts the rows back in the order the bindings are in",
    );

    // And the row drawn first now edits the binding that is first. The
    // writes differ, so which one moved is not a guess.
    let percent = control(&mut app, 0, BindControl::AsPercent);
    app.world_mut().trigger(bevy::ui_widgets::ValueChange {
        source: percent,
        value: true,
        is_final: true,
    });
    app.update();
    app.update();

    let after = live(&app, entity);
    let percent_of = |binding: &Binding| match binding {
        Binding::Field { as_percent, .. } => *as_percent,
        _ => panic!("both bindings are Fields"),
    };
    assert!(
        percent_of(&after.0[0]) && !percent_of(&after.0[1]),
        "the first row edited the first binding, not the one that used to be there",
    );
}

// ---------------------------------------------------------------------------
// The schema feeds
// ---------------------------------------------------------------------------

/// `callable_by_value` is the filter, not a suggestion: a function that
/// borrows its argument cannot be called however it is spelled, and arity
/// has to match the reads it would combine.
#[test]
fn the_via_picker_offers_only_callable_functions_of_the_right_arity() {
    let (mut app, _) = app_with_bindings(Bindings(vec![Binding::Field {
        read: vec![
            BindPath::new("demo_game::Health.current"),
            BindPath::new("demo_game::Health.max"),
        ],
        via: None,
        write: BindPath::new("bevy_ui::ui_node::Node.width"),
        as_percent: false,
    }]));

    let via = control(&mut app, 0, BindControl::Via);
    let offered = combo_values(&mut app, via);
    assert!(
        offered.iter().any(|value| value == "demo_game::ratio"),
        "the two-argument function is offered for two reads: {offered:?}",
    );
    assert!(
        !offered.iter().any(|value| value == "demo_game::half"),
        "a one-argument function cannot combine two reads: {offered:?}",
    );
    assert!(
        !offered.iter().any(|value| value == "demo_game::peek"),
        "a function that borrows its argument is never callable: {offered:?}",
    );
}

/// Enum events are in the schema but the dispatcher cannot fill one, so the
/// picker must not offer it as if it could.
#[test]
fn the_event_picker_offers_only_struct_events() {
    let (mut app, _) = app_with_bindings(Bindings(vec![Binding::Action {
        event: String::new(),
        fields: Vec::new(),
    }]));

    let event = control(&mut app, 0, BindControl::Event);
    let offered = combo_values(&mut app, event);
    assert!(
        offered.iter().any(|value| value == "demo_game::Fired"),
        "a struct event is pickable: {offered:?}",
    );
    assert!(
        !offered.iter().any(|value| value == "demo_game::Mode"),
        "an enum event cannot be dispatched from a binding: {offered:?}",
    );
}

/// An event that cannot fill its own gaps has to map every field it
/// declares, and the card says so before the game does.
#[test]
fn an_event_that_cannot_fill_gaps_warns_while_fields_are_unmapped() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![Binding::Action {
        event: "demo_game::Strict".to_string(),
        fields: Vec::new(),
    }]));

    let body = card_body(&mut app);
    let warning = descendants(app.world_mut(), body)
        .into_iter()
        .find(|entity| {
            badge(&mut app, *entity)
                .is_some_and(|title| title.contains("leaves field 'amount' unmapped"))
        });
    assert!(
        warning.is_some(),
        "an unfillable event with an unmapped field warns on the card",
    );

    // Map it, and the warning goes.
    let combo = control(&mut app, 0, BindControl::PathType(PathSlot::EventField));
    pick(&mut app, combo, 0, "Health", Some("demo_game::Health"));
    let Binding::Action { event, fields } = live(&app, entity).0[0].clone() else {
        panic!("the binding is still an Action");
    };
    assert_eq!(event, "demo_game::Strict");
    assert_eq!(
        fields,
        vec![(
            "amount".to_string(),
            BindPath::new("demo_game::Health.current"),
        )],
        "picking a source maps the event field by name",
    );
    // The mapping has to reach the document as DATA. Its `Debug` text
    // contains the path either way, so only the shape can tell a real pair
    // from a stringified tuple.
    let authored = authored_field(&authored_bindings(&app, entity)[0], "fields");
    let BsnValue::List(items) = &authored else {
        panic!("the mappings author as a list, got {authored:?}");
    };
    let Some(BsnValue::List(pair)) = items.first() else {
        panic!("a mapping authors as a pair of values, not as text: {items:?}");
    };
    assert!(
        matches!(pair.first(), Some(BsnValue::String(name)) if name == "amount"),
        "the pair's first half is the field name: {pair:?}",
    );
    assert!(
        matches!(pair.get(1), Some(BsnValue::Struct(path)) if path.type_path.ends_with("BindPath")),
        "the pair's second half is a BindPath, not its Debug text: {pair:?}",
    );

    let body = card_body(&mut app);
    let still_warning = descendants(app.world_mut(), body)
        .into_iter()
        .any(|entity| {
            app.world()
                .get::<Tooltip>(entity)
                .is_some_and(|tip| tip.title.contains("leaves field 'amount' unmapped"))
        });
    assert!(
        !still_warning,
        "once every field is mapped there is nothing to warn about",
    );
}

/// The widget's own components are what a Field binding writes fields of, so
/// the write picker is the selected entity's archetype rather than the whole
/// registry. Markers are the one exception; the test below covers them.
#[test]
fn the_write_picker_offers_the_selected_entitys_own_components() {
    let (mut app, _) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "",
    )]));

    let write = control(&mut app, 0, BindControl::PathType(PathSlot::Write));
    let offered = combo_values(&mut app, write);
    assert!(
        offered
            .iter()
            .any(|value| value == "bevy_ui::ui_node::Node"),
        "the entity's own Node is a write target: {offered:?}",
    );
    assert!(
        !offered.iter().any(|value| value == "demo_game::Health"),
        "a component the entity does not carry cannot be written: {offered:?}",
    );
}

/// A marker is the one write target that does not have to be on the widget
/// already: putting it on is what the binding does. So the picker offers
/// every registered marker, and picking one leaves a path with no field: the
/// whole component is the value.
#[test]
fn the_write_picker_offers_markers_the_widget_does_not_carry_yet() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "",
    )]));

    let write = control(&mut app, 0, BindControl::PathType(PathSlot::Write));
    let offered = combo_values(&mut app, write);
    let marker = "bevy_ui::interaction_states::InteractionDisabled";
    assert!(
        offered.iter().any(|value| value == marker),
        "a marker the widget could be given is a write target: {offered:?}",
    );

    let index = offered
        .iter()
        .position(|value| value == marker)
        .expect("the marker is offered");
    pick(&mut app, write, index, "InteractionDisabled", Some(marker));
    assert_eq!(
        write_path(&live(&app, entity).0[0]),
        marker,
        "picking a marker leaves the type on its own, with no field after it",
    );
}

/// A project component authored on this entity is a write target like any
/// other. It is never a real ECS component in the editor (its code is in the
/// game binary) so the archetype cannot see it, and judging a write by the
/// archetype alone would badge a good path red and refuse to offer it in the
/// first place.
#[test]
fn a_write_to_an_authored_project_component_is_offered_and_not_badged() {
    // Three bindings. The first has picked no write yet, so what the picker
    // offers is the picker's own answer rather than the current value being
    // kept in the list. The second writes to the authored project component.
    // The third writes to a native component this entity does not carry, as
    // game code adds `Transform` to a UI node all the time, which the
    // archetype cannot vouch for either.
    let (mut app, _) = app_with_bindings_and_project(
        Bindings(vec![
            field_binding("demo_game::Health.current", ""),
            field_binding("demo_game::Health.current", "demo_game::Health.max"),
            field_binding(
                "demo_game::Health.current",
                "bevy_transform::components::transform::Transform.translation",
            ),
        ]),
        &["demo_game::Health"],
    );

    let write = control(&mut app, 0, BindControl::PathType(PathSlot::Write));
    let offered = combo_values(&mut app, write);
    assert!(
        offered.iter().any(|value| value == "demo_game::Health"),
        "the game component the document authored here is a write target: {offered:?}",
    );

    let body = card_body(&mut app);
    let labels: Vec<Entity> = descendants(app.world_mut(), body)
        .into_iter()
        .filter(|entity| app.world().get::<BindingRowLabel>(*entity).is_some())
        .collect();
    for (row, what) in [
        (1, "a component the document authored here"),
        (2, "a known component the entity does not carry yet"),
    ] {
        let label = *labels
            .get(row)
            .unwrap_or_else(|| panic!("row {row} exists"));
        assert_eq!(
            app.world()
                .get::<Tooltip>(label)
                .map(|tip| tip.title.clone()),
            None,
            "a write to {what} resolves, so it carries no badge",
        );
        assert_ne!(
            app.world().get::<TextColor>(label).map(|color| color.0),
            Some(tokens::TEXT_ERROR),
            "a write to {what} is not an error",
        );
    }
}

/// A game's own marker is a write target like a native one, but only when
/// reflection can build it. Without a default the game cannot put the
/// component on either, so offering it would author a binding that reads
/// right and dies in the log at run time.
#[test]
fn a_project_marker_is_writable_only_when_the_game_can_build_it() {
    let (mut app, entity) = app_with_bindings_and_project(
        Bindings(vec![field_binding("demo_game::Health.current", "")]),
        &["demo_game::Flagged", "demo_game::Unbuildable"],
    );

    let write = control(&mut app, 0, BindControl::PathType(PathSlot::Write));
    let offered = combo_values(&mut app, write);
    for marker in ["demo_game::Flagged", "demo_game::Unbuildable"] {
        assert!(
            offered.iter().any(|value| value == marker),
            "{marker} is authored here, so it stays in the list: {offered:?}",
        );
    }

    let index = |offered: &[String], wanted: &str| {
        offered
            .iter()
            .position(|value| value == wanted)
            .unwrap_or_else(|| panic!("{wanted} is offered"))
    };

    pick(
        &mut app,
        write,
        index(&offered, "demo_game::Flagged"),
        "Flagged",
        Some("demo_game::Flagged"),
    );
    assert_eq!(
        write_path(&live(&app, entity).0[0]),
        "demo_game::Flagged",
        "a marker the game can build authors as the type on its own",
    );

    let write = control(&mut app, 0, BindControl::PathType(PathSlot::Write));
    let offered = combo_values(&mut app, write);
    pick(
        &mut app,
        write,
        index(&offered, "demo_game::Unbuildable"),
        "Unbuildable",
        Some("demo_game::Unbuildable"),
    );
    assert_eq!(
        write_path(&live(&app, entity).0[0]),
        "",
        "one the game cannot build is not a marker write, so no path is composed",
    );
}

/// An event the schema does not know still has whatever mappings were
/// authored against it. Drawing no rows would leave those values in the
/// document with no way to see or take them back.
#[test]
fn an_action_keeps_its_mappings_when_the_event_is_unknown() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![Binding::Action {
        event: "demo_game::Vanished".to_string(),
        fields: vec![(
            "amount".to_string(),
            BindPath::new("demo_game::Health.current"),
        )],
    }]));

    let combo = control(&mut app, 0, BindControl::PathField(PathSlot::EventField));
    pick(&mut app, combo, 1, "max", Some("max"));

    let Binding::Action { fields, .. } = live(&app, entity).0[0].clone() else {
        panic!("the binding is still an Action");
    };
    assert_eq!(
        fields,
        vec![("amount".to_string(), BindPath::new("demo_game::Health.max"))],
        "the mapping on an unknown event is still shown, and still editable",
    );
}

/// A type the editor has no schema for is still a type the game may know.
/// The picker carries it as an entry of its own, and picking it keeps the
/// field the path already named: an empty field list means "no schema
/// here", not "this type has no fields", and treating the two the same
/// erases what the user authored.
#[test]
fn a_pick_with_no_schema_to_check_it_against_keeps_the_authored_path() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Ghost.amount",
        "bevy_ui::ui_node::Node.width",
    )]));

    let combo = control(&mut app, 0, BindControl::PathType(PathSlot::Read(0)));
    assert!(
        combo_values(&mut app, combo).contains(&"demo_game::Ghost".to_string()),
        "the picker offers the authored type rather than hiding it",
    );

    pick(&mut app, combo, 0, "Ghost", Some("demo_game::Ghost"));

    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "demo_game::Ghost.amount",
        "the pick left the authored path whole",
    );
}

/// Every option a combobox would open, by value (the full path the pick
/// writes), reading the widget's own config through the card's helper.
fn combo_values(app: &mut App, combo: Entity) -> Vec<String> {
    jackdaw::inspector::bindings_card::combo_option_values(app.world(), combo)
}

// ---------------------------------------------------------------------------
// The authored case
// ---------------------------------------------------------------------------

/// The card has to work on a binding that came out of a scene file, not
/// only on one poked into a live world. An entity in the document is
/// filtered by its authored type paths before it reaches a card at all, and
/// `Bindings` sits in the namespace that filter culls by default.
#[test]
fn an_authored_bindings_component_still_gets_its_card() {
    let (mut app, entity) = app_with_bindings(Bindings(vec![field_binding(
        "demo_game::Health.current",
        "bevy_ui::ui_node::Node.width",
    )]));

    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(entity)
            .is_some(),
        "the fixture entity is a document entity",
    );
    assert_eq!(
        authored_bindings(&app, entity).len(),
        1,
        "and its Bindings is authored, not derived",
    );
    let _ = card_body(&mut app);

    // And an edit on the authored component round-trips: the picker writes,
    // the document takes it, undo puts the authored value back.
    let combo = control(&mut app, 0, BindControl::PathField(PathSlot::Read(0)));
    pick(&mut app, combo, 1, "max", Some("max"));
    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "demo_game::Health.max",
    );
    undo(&mut app);
    assert_eq!(
        read_path(&live(&app, entity).0[0], 0),
        "demo_game::Health.current",
        "undo restores the authored path",
    );
}

// ---------------------------------------------------------------------------
// The document round trip
// ---------------------------------------------------------------------------

/// The card's own tests stop at the AST: they prove what an edit writes, not
/// what a reopened file gives back. This is the other half: the editor saves a
/// scene holding bindings, loads that file into a fresh editor, and has to find
/// the same value it wrote.
#[test]
fn a_saved_binding_comes_back_off_disk_unchanged() {
    let authored = Bindings(vec![
        Binding::Field {
            read: vec![
                BindPath::new("game::hud::Health.current"),
                BindPath::new("game::hud::Health.max"),
            ],
            via: Some("ratio".to_string()),
            write: BindPath::new("bevy_ui::ui_node::Node.width"),
            as_percent: true,
        },
        Binding::Action {
            event: "game::hud::RetryPressed".to_string(),
            fields: vec![("slot".to_string(), BindPath::new("game::hud::Save.slot"))],
        },
    ]);
    let (mut app, _) = app_with_bindings(authored.clone());

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("hud.bsn");
    let saved = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), dir.path());
    std::fs::write(&path, &saved).expect("write the scene");

    let mut reloaded = util::editor_test_app();
    jackdaw::scene_io::load_scene_from_file(reloaded.world_mut(), &path);
    reloaded.update();

    let world = reloaded.world_mut();
    let loaded = world
        .query::<(&Name, &Bindings)>()
        .iter(world)
        .find(|(name, _)| name.as_str() == "hud")
        .map(|(_, bindings)| bindings.clone())
        .unwrap_or_else(|| panic!("the reloaded document carries the bound node:\n{saved}"));
    assert_eq!(
        loaded, authored,
        "every path, option and pair survives the file:\n{saved}",
    );

    let resaved =
        jackdaw::scene_io::emit_bsn_scene_with_inline_assets(reloaded.world_mut(), dir.path());
    assert_eq!(resaved, saved, "and saving what loaded is a fixpoint");
}

// ---------------------------------------------------------------------------
// The row at the width the panel ships at
// ---------------------------------------------------------------------------

/// The shipped right sidebar, and the same panel dragged narrower.
const PANEL_WIDTHS: [f32; 3] = [260.0, 212.0, 120.0];

/// The width a picker stops naming a type at.
const PICKER_FLOOR: f32 = 72.0;

/// Reparent the card under a fixed-width root and lay it out. The card the
/// dispatch builds hangs in a panel with no width of its own in a headless
/// app, so the panel's width is applied here instead.
fn laid_out_card(app: &mut App, width: f32) -> Entity {
    let body = card_body(app);
    // The panel sizes itself to its content in a headless app, and a card
    // opens on a pointer gesture. Both are set here, so what the row is
    // measured in is the width the editor docks the panel at.
    let mut root = body;
    while let Some(parent) = app.world().get::<ChildOf>(root).map(ChildOf::parent) {
        root = parent;
    }
    if let Some(mut node) = app.world_mut().get_mut::<Node>(root) {
        node.width = Val::Px(width);
        node.position_type = PositionType::Absolute;
    }
    if let Some(mut node) = app.world_mut().get_mut::<Node>(body) {
        node.display = Display::Flex;
    }
    app.update();
    app.update();
    body
}

/// Right edge of a laid-out node, in the window's pixels.
fn right_edge(app: &App, entity: Entity) -> f32 {
    let computed = app
        .world()
        .get::<bevy::ui::ComputedNode>(entity)
        .unwrap_or_else(|| panic!("{entity} is laid out"));
    let centre = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .unwrap_or_else(|| panic!("{entity} is laid out"))
        .translation;
    centre.x + computed.size().x / 2.0
}

/// A read row carries three pickers and a write row two, all on a line the
/// label already spent half of. They wrap rather than clip: a picker showing
/// half a type name names nothing, and one hanging off the panel's edge is
/// cut off by it.
///
/// The two halves are not equals where they meet. A panel dragged narrower
/// than the floor itself (the dock clamps a split at 5%, not at a width a
/// card would like) cannot give a picker [`PICKER_FLOOR`] and keep it
/// inside the panel both. There the edge wins: a picker that took the whole
/// narrow line is still something the user can open, and its popover names
/// the types in full, while one hung past the edge is clipped to nothing.
/// So the floor is asserted against the room the row actually has.
#[test]
fn a_path_rows_pickers_stay_readable_at_the_shipped_width() {
    for width in PANEL_WIDTHS {
        let (mut app, _) = app_with_bindings(Bindings(vec![field_binding(
            "demo_game::Health.current",
            "bevy_ui::ui_node::Node.width",
        )]));
        let body = laid_out_card(&mut app, width);

        let pickers: Vec<Entity> = descendants(app.world_mut(), body)
            .into_iter()
            .filter(|entity| app.world().get::<EditorComboBox>(*entity).is_some())
            .filter(|entity| {
                matches!(
                    app.world()
                        .get::<BindingControl>(*entity)
                        .map(BindingControl::control),
                    Some(
                        BindControl::PathSource(_)
                            | BindControl::PathType(_)
                            | BindControl::PathField(_)
                    )
                )
            })
            .collect();
        assert!(!pickers.is_empty(), "the card has pickers to measure");

        for picker in pickers {
            let size = app
                .world()
                .get::<bevy::ui::ComputedNode>(picker)
                .unwrap_or_else(|| panic!("{picker} is laid out"))
                .size()
                .x;
            // The line the picker wrapped onto, which is what the row had
            // to give it.
            let line = app
                .world()
                .get::<ChildOf>(picker)
                .map(ChildOf::parent)
                .and_then(|control| app.world().get::<bevy::ui::ComputedNode>(control))
                .unwrap_or_else(|| panic!("{picker} sits in a laid-out control"))
                .size()
                .x;
            let floor = PICKER_FLOOR.min(line);
            assert!(
                size >= floor,
                "a picker keeps {floor} px to name a type in; \
                 it got {size} px in a {width} px panel",
            );
            assert!(
                right_edge(&app, picker) <= right_edge(&app, body) + 0.5,
                "and ends inside the panel rather than past its edge, \
                 in a {width} px panel",
            );
        }
    }
}
