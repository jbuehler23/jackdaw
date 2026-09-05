//! The inspector's `Val` and `UiRect` composite fields, and the structured
//! `Node` card built from them.

use crate::util;

use bevy::camera::RenderTarget;
use bevy::picking::{
    backend::HitData,
    events::{Click, Pointer},
    pointer::{Location, PointerButton, PointerId},
};
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::ValueChange;
use bevy::window::{PrimaryWindow, WindowRef};
use jackdaw::inspector::node_card::NodeCardBody;
use jackdaw::inspector::val_field::{refresh_val_fields, spawn_ui_rect_field, spawn_val_field};
use jackdaw::selection::Selection;
use jackdaw_bsn::{BsnValue, SceneBsnAst};
use jackdaw_commands::CommandHistory;
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, EditorComboBox};
use jackdaw_feathers::number_input::{ScrubNumberInput, ScrubNumberInputValue};
use jackdaw_feathers::panel_card::PanelCardKey;
use jackdaw_feathers::tokens;

const NODE: &str = "bevy_ui::ui_node::Node";

/// Unit order the composite offers, mirrored here so tests select by name.
const UNITS: [&str; 7] = ["px", "%", "auto", "vw", "vh", "vmin", "vmax"];

fn unit_index(label: &str) -> usize {
    UNITS
        .iter()
        .position(|u| *u == label)
        .expect("unit in the dropdown")
}

/// A selected, document-tracked entity carrying a `Node`.
fn app_with_node(node: Node) -> (App, Entity) {
    let mut app = util::editor_test_app();
    let entity = app.world_mut().spawn((Name::new("ui"), node)).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();
    (app, entity)
}

/// Depth-first descendants of `root` in child order, `root` excluded.
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

fn widgets_with<C: Component>(world: &mut World, root: Entity) -> Vec<Entity> {
    descendants(world, root)
        .into_iter()
        .filter(|e| world.get::<C>(*e).is_some())
        .collect()
}

fn number_input(app: &mut App, root: Entity) -> Entity {
    widgets_with::<ScrubNumberInput>(app.world_mut(), root)
        .first()
        .copied()
        .expect("composite has a scrub number input")
}

fn unit_select(app: &mut App, root: Entity) -> Entity {
    widgets_with::<EditorComboBox>(app.world_mut(), root)
        .first()
        .copied()
        .expect("composite has a unit dropdown")
}

fn spawn_val(app: &mut App, entity: Entity, field: &str, value: Val) -> Entity {
    let parent = app.world_mut().spawn(Node::default()).id();
    let world = app.world_mut();
    let root = {
        let mut commands = world.commands();
        spawn_val_field(
            &mut commands,
            parent,
            field,
            value,
            0,
            field.to_string(),
            entity,
            NODE,
        )
    };
    world.flush();
    app.update();
    app.update();
    root
}

fn spawn_rect(app: &mut App, entity: Entity, field: &str, value: UiRect) -> Entity {
    let parent = app.world_mut().spawn(Node::default()).id();
    let world = app.world_mut();
    let root = {
        let mut commands = world.commands();
        spawn_ui_rect_field(
            &mut commands,
            parent,
            field,
            value,
            0,
            field.to_string(),
            entity,
            NODE,
        )
    };
    world.flush();
    app.update();
    app.update();
    root
}

fn scrub(app: &mut App, widget: Entity, value: f64, is_final: bool) {
    app.world_mut().trigger(ValueChange {
        source: widget,
        value,
        is_final,
    });
    app.update();
}

fn pick_unit(app: &mut App, widget: Entity, unit: &str) {
    let index = unit_index(unit);
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: widget,
        selected: index,
        label: unit.to_string(),
        value: None,
    });
    app.update();
}

/// Undo the last edit, then run the composite's refresh pass directly: it is
/// registered behind `AppState::Editor`, which a headless test never enters, and
/// `run_system_cached` keeps its `Local` change tick across calls.
fn undo_and_refresh(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    app.world_mut()
        .run_system_cached(refresh_val_fields)
        .expect("refresh pass runs");
    app.update();
}

fn authored(app: &App, entity: Entity, field_path: &str) -> Option<BsnValue> {
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity)?;
    jackdaw_bsn::get_bsn_field(ast, node, NODE, field_path)
}

/// A committed `Val` reads back as its variant path plus magnitude.
fn assert_authored_val(value: Option<BsnValue>, variant: &str, magnitude: f32) {
    let Some(BsnValue::TupleStruct(data)) = value else {
        panic!("expected a tuple-variant Val in the document, got {value:?}");
    };
    assert!(
        data.type_path.ends_with(variant),
        "document holds {variant}; got {}",
        data.type_path
    );
    let Some(BsnValue::Float(n)) = data.values.first() else {
        panic!("expected a float magnitude, got {:?}", data.values);
    };
    assert!(
        (*n as f32 - magnitude).abs() < 1e-4,
        "document magnitude is {magnitude}; got {n}",
    );
}

/// A field the user only looked at commits nothing. The text stands for the
/// value rather than being it, so a blur that committed it back would author
/// `33.33` over the exact third a percent landing writes.
#[test]
fn a_val_field_blurred_without_an_edit_keeps_the_value_it_was_shown() {
    let exact = 100.0 / 3.0;
    let (mut app, entity) = app_with_node(Node {
        width: Val::Percent(exact),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Percent(exact));
    let number = number_input(&mut app, root);
    let text = editable_text(&mut app, number);
    let entries = app.world().resource::<CommandHistory>().undo_stack.len();

    blur(&mut app, text);

    assert_eq!(
        app.world()
            .get::<Node>(entity)
            .expect("the node stands")
            .width,
        Val::Percent(exact),
        "the value the field was shown survives being looked at",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        entries,
        "and there is nothing to undo",
    );
}

/// Typed text commits even when the field it replaces shows more digits than
/// were typed: `2` over a readout of `12.00` is an edit.
#[test]
fn a_val_field_typed_into_commits_what_was_typed() {
    use bevy::text::TextEdit;

    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(12.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(12.0));
    let number = number_input(&mut app, root);
    let text = editable_text(&mut app, number);
    let entries = app.world().resource::<CommandHistory>().undo_stack.len();

    let mut editable = app
        .world_mut()
        .get_mut::<bevy::text::EditableText>(text)
        .expect("the editable text");
    editable.queue_edit(TextEdit::SelectAll);
    editable.queue_edit(TextEdit::Insert("2".into()));
    app.update();
    assert_eq!(
        app.world()
            .get::<bevy::text::EditableText>(text)
            .expect("the editable text")
            .value()
            .to_string(),
        "2",
        "the typed text replaced the readout",
    );

    blur(&mut app, text);

    assert_eq!(
        app.world()
            .get::<Node>(entity)
            .expect("the node stands")
            .width,
        Val::Px(2.0),
        "what was typed is what the node now holds",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        entries + 1,
        "as one entry to undo",
    );
}

/// The text entry inside a scrub number field.
fn editable_text(app: &mut App, number: Entity) -> Entity {
    widgets_with::<bevy::text::EditableText>(app.world_mut(), number)
        .first()
        .copied()
        .expect("the number input has a text entry")
}

/// Take the focus off a field, the way clicking elsewhere does.
fn blur(app: &mut App, text: Entity) {
    app.world_mut()
        .trigger(bevy::input_focus::FocusLost { entity: text });
    app.update();
    app.update();
}

#[test]
fn scrubbing_a_val_commits_to_ecs_document_and_history() {
    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(100.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(100.0));
    let number = number_input(&mut app, root);

    scrub(&mut app, number, 42.0, true);

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(42.0)),
        "the live component must move",
    );
    assert_authored_val(authored(&app, entity, "width"), "Val::Px", 42.0);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        1,
        "one commit mints exactly one undo entry",
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(100.0)),
        "undo restores the pre-edit value",
    );
}

/// Where a readout that does not fit loses its digits. A `UiRect` cell is at its
/// floor at the shipped panel width, and an unfocused field is scrolled to its
/// caret: a caret after the last digit would turn "12.00" into a plausible
/// "2.00", so the caret is parked at the start and the decimals fall off.
#[test]
fn a_readout_shows_its_leading_digits_when_it_cannot_show_them_all() {
    use bevy::text::EditableText;

    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(12.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(12.0));
    let number = number_input(&mut app, root);
    let text = widgets_with::<EditableText>(app.world_mut(), number)
        .first()
        .copied()
        .expect("the number input has an editable text");

    let editable = app
        .world()
        .get::<EditableText>(text)
        .expect("the editable text");
    assert_eq!(
        editable.value().to_string(),
        "12.00",
        "the readout holds the value it was given",
    );
    assert_eq!(
        editable.editor().raw_selection().text_range(),
        0..0,
        "with the caret at the first digit, so the field is scrolled to it",
    );

    // Every value the field is handed parks the same way, a drag's included.
    scrub(&mut app, number, 123.0, true);
    app.update();
    let editable = app
        .world()
        .get::<EditableText>(text)
        .expect("the editable text");
    assert_eq!(editable.value().to_string(), "123.00");
    assert_eq!(
        editable.editor().raw_selection().text_range(),
        0..0,
        "a scrubbed value parks at its first digit too",
    );
}

#[test]
fn a_unit_pick_after_undo_carries_the_restored_magnitude() {
    // An undo (or a gizmo, or another editor) moves the component behind the
    // row's back, and the next edit must build on the restored value.
    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(100.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(100.0));
    let number = number_input(&mut app, root);

    scrub(&mut app, number, 42.0, true);
    undo_and_refresh(&mut app);

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(100.0)),
        "undo restores the component",
    );
    assert_eq!(
        app.world().get::<ScrubNumberInputValue>(number),
        Some(&ScrubNumberInputValue::F64(100.0)),
        "and the number input follows it",
    );

    let units = unit_select(&mut app, root);
    pick_unit(&mut app, units, "%");

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Percent(100.0)),
        "the unit pick carries the restored magnitude, not the undone one",
    );
    assert_authored_val(authored(&app, entity, "width"), "Val::Percent", 100.0);
}

#[test]
fn an_external_unit_change_refreshes_the_row() {
    // Undoing a unit change has to move the row's unit back too, or the next
    // scrub commits the magnitude under the wrong unit.
    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(20.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(20.0));
    let units = unit_select(&mut app, root);

    pick_unit(&mut app, units, "%");
    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Percent(20.0)),
    );

    undo_and_refresh(&mut app);
    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(20.0)),
    );

    let number = number_input(&mut app, root);
    scrub(&mut app, number, 8.0, true);
    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(8.0)),
        "the row went back to px, so the next scrub commits px",
    );
}

#[test]
fn a_drag_tick_previews_without_minting_history() {
    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(100.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(100.0));
    let number = number_input(&mut app, root);

    scrub(&mut app, number, 55.0, false);

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(55.0)),
        "a drag tick previews on live ECS",
    );
    assert!(
        app.world()
            .resource::<CommandHistory>()
            .undo_stack
            .is_empty(),
        "a drag tick must not mint undo",
    );

    scrub(&mut app, number, 60.0, true);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        1,
        "the whole drag lands as one undo entry",
    );
}

#[test]
fn switching_px_to_percent_preserves_the_magnitude() {
    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(30.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(30.0));
    let units = unit_select(&mut app, root);

    pick_unit(&mut app, units, "%");

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Percent(30.0)),
        "changing the unit keeps the number",
    );
    assert_authored_val(authored(&app, entity, "width"), "Val::Percent", 30.0);
}

#[test]
fn auto_clears_the_number_and_disables_the_input() {
    let (mut app, entity) = app_with_node(Node {
        width: Val::Px(30.0),
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Px(30.0));
    let units = unit_select(&mut app, root);
    let number = number_input(&mut app, root);

    pick_unit(&mut app, units, "auto");

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Auto),
    );
    assert!(
        app.world().get::<InteractionDisabled>(number).is_some(),
        "Auto has no magnitude, so the number input is disabled",
    );
    assert_eq!(
        authored(&app, entity, "width"),
        Some(BsnValue::Type("bevy_ui::geometry::Val::Auto".to_string())),
        "the document records the unit variant",
    );
}

#[test]
fn leaving_auto_starts_the_magnitude_at_zero() {
    let (mut app, entity) = app_with_node(Node {
        width: Val::Auto,
        ..default()
    });
    let root = spawn_val(&mut app, entity, "width", Val::Auto);
    let number = number_input(&mut app, root);
    assert!(
        app.world().get::<InteractionDisabled>(number).is_some(),
        "an Auto field starts disabled",
    );

    let units = unit_select(&mut app, root);
    pick_unit(&mut app, units, "px");

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.width),
        Some(Val::Px(0.0)),
        "leaving Auto starts at zero",
    );
    assert!(
        app.world().get::<InteractionDisabled>(number).is_none(),
        "the number input comes back",
    );
}

#[test]
fn a_ui_rect_edits_its_four_sides_independently() {
    let (mut app, entity) = app_with_node(Node {
        margin: UiRect::all(Val::Px(4.0)),
        ..default()
    });
    let root = spawn_rect(&mut app, entity, "margin", UiRect::all(Val::Px(4.0)));

    let inputs = widgets_with::<ScrubNumberInput>(app.world_mut(), root);
    assert_eq!(inputs.len(), 4, "left, top, right, bottom");

    scrub(&mut app, inputs[0], 12.0, true);

    let margin = app.world().get::<Node>(entity).map(|n| n.margin);
    assert_eq!(
        margin,
        Some(UiRect {
            left: Val::Px(12.0),
            right: Val::Px(4.0),
            top: Val::Px(4.0),
            bottom: Val::Px(4.0),
        }),
        "only the edited side moves",
    );
    assert_authored_val(authored(&app, entity, "margin.left"), "Val::Px", 12.0);
    assert_eq!(app.world().resource::<CommandHistory>().undo_stack.len(), 1,);
}

/// Entities in `root`'s subtree that are field rows. The shared row shape is the
/// only thing here carrying the field row's minimum height.
fn field_rows(app: &mut App, root: Entity) -> usize {
    let mut all = descendants(app.world_mut(), root);
    all.push(root);
    all.into_iter()
        .filter(|entity| {
            app.world()
                .get::<Node>(*entity)
                .is_some_and(|node| node.min_height == Val::Px(tokens::FIELD_ROW_HEIGHT))
        })
        .count()
}

/// The width of a field row's label column: the row's first child.
fn row_label_width(app: &mut App, row: Entity) -> Val {
    let label = app.world().get::<Children>(row).expect("row has children")[0];
    app.world().get::<Node>(label).expect("label node").width
}

#[test]
fn a_ui_rect_is_one_row_not_a_column_of_five() {
    // Twelve stacked length rows for one `Node`'s margin/padding/border pushes
    // the whole spacing story off the bottom of the panel.
    let (mut app, entity) = app_with_node(Node {
        margin: UiRect::all(Val::Px(4.0)),
        ..default()
    });
    let root = spawn_rect(&mut app, entity, "margin", UiRect::all(Val::Px(4.0)));

    assert_eq!(
        field_rows(&mut app, root),
        1,
        "a rect is one row; its four sides are cells inside that row",
    );
    assert_eq!(
        widgets_with::<ScrubNumberInput>(app.world_mut(), root).len(),
        4,
        "all four sides are still editable",
    );
}

#[test]
fn a_long_label_does_not_push_its_control_out_of_the_column() {
    // The label column is the alignment contract for the whole panel: a
    // `flex_basis` row and a `top` row must put their inputs at the same x.
    let (mut app, entity) = app_with_node(Node::default());
    let short = spawn_val(&mut app, entity, "top", Val::Px(1.0));
    let long = spawn_val(&mut app, entity, "flex_basis", Val::Px(1.0));

    let short_width = row_label_width(&mut app, short);
    assert_eq!(
        short_width,
        Val::Px(tokens::FIELD_LABEL_WIDTH),
        "a length row takes the shared label column",
    );
    assert_eq!(
        row_label_width(&mut app, long),
        short_width,
        "a long name narrows nothing and moves no control",
    );
}

#[test]
fn a_selected_nodes_lengths_reach_the_inspector_as_unit_rows() {
    // The dispatch, not the widgets: selecting a UI node has to build its `Node`
    // card out of these composites rather than variant menus.
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Play"),
            Node {
                width: Val::Px(120.0),
                height: Val::Auto,
                ..default()
            },
        ))
        .id();
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    app.update();
    app.update();

    let unit_rows = app
        .world_mut()
        .query_filtered::<Entity, With<EditorComboBox>>()
        .iter(app.world())
        .count();
    assert!(
        unit_rows >= 2,
        "width and height each get a unit dropdown; found {unit_rows}",
    );
    let card_text = component_card_labels(&mut app);
    assert!(
        card_text.iter().any(|t| t == "width"),
        "the length field is labelled in place, not behind a menu: {card_text:?}",
    );
    assert!(
        !card_text.iter().any(|t| t.contains("Val::")),
        "no variant menu text survives: {card_text:?}",
    );
}

/// Every text drawn inside an inspector component card.
fn component_card_labels(app: &mut App) -> Vec<String> {
    let cards: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::inspector::ComponentDisplay>>()
        .iter(app.world())
        .collect();
    let mut labels = Vec::new();
    for card in cards {
        let mut stack = vec![card];
        while let Some(entity) = stack.pop() {
            if let Some(text) = app.world().get::<Text>(entity) {
                labels.push(text.0.clone());
            }
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
    }
    labels
}

// ---------------------------------------------------------------------------
// The structured `Node` card
// ---------------------------------------------------------------------------

/// A selected, document-tracked `Node` entity with an inspector mounted, so
/// the card is built by the real dispatch rather than called directly.
fn app_with_node_card(node: Node) -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app.world_mut().spawn((Name::new("ui"), node)).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
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

/// The card body the dispatch built.
fn node_card_body(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<NodeCardBody>>()
        .iter(app.world())
        .next()
        .expect("a Node selection builds the structured card")
}

/// The card section titled `title`, found by the key it remembers its collapsed
/// state under rather than by a word that might also be a segment label.
fn card_section(app: &mut App, title: &str) -> Entity {
    let body = node_card_body(app);
    let key = format!("node_card::{title}");
    descendants(app.world_mut(), body)
        .into_iter()
        .find(|entity| {
            app.world()
                .get::<PanelCardKey>(*entity)
                .is_some_and(|card| card.0 == key)
        })
        .unwrap_or_else(|| panic!("no `{title}` section on the card"))
}

/// Every dropdown inside a card section, in row order.
fn section_combos(app: &mut App, title: &str) -> Vec<Entity> {
    let section = card_section(app, title);
    widgets_with::<EditorComboBox>(app.world_mut(), section)
}

/// The row labelled `label` inside the `title` section, rather than an index.
fn row_of_label(app: &mut App, title: &str, label: &str) -> Entity {
    let section = card_section(app, title);
    for entity in descendants(app.world_mut(), section) {
        let is_label = app
            .world()
            .get::<Text>(entity)
            .is_some_and(|text| text.0 == label);
        if is_label {
            return app
                .world()
                .get::<ChildOf>(entity)
                .expect("a row holds its label")
                .parent();
        }
    }
    panic!("no `{label}` row in the `{title}` section");
}

/// The dropdown on that row.
fn row_combo(app: &mut App, title: &str, label: &str) -> Entity {
    let row = row_of_label(app, title, label);
    widgets_with::<EditorComboBox>(app.world_mut(), row)
        .first()
        .copied()
        .unwrap_or_else(|| panic!("no `{label}` dropdown in the `{title}` section"))
}

/// The card's refresh passes, driven by hand. They are scheduled behind
/// `AppState::Editor`, which a headless test never enters.
fn refresh_card(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::inspector::node_card::paint_node_segments)
        .expect("the segment paint runs");
    app.world_mut()
        .run_system_cached(jackdaw::inspector::node_card::refresh_node_enum_combos)
        .expect("the dropdown refresh runs");
    app.world_mut()
        .run_system_cached(jackdaw::inspector::node_card::refresh_node_optional_numbers)
        .expect("the optional-row refresh runs");
    app.update();
}

/// The integer inside an authored `Option`, so a test can hold the document
/// and the component to the same number.
fn authored_option_int(app: &App, entity: Entity, path: &str) -> Option<i128> {
    match authored(app, entity, path)? {
        BsnValue::TupleStruct(data) => match data.values.first()? {
            BsnValue::Int(v) => Some(*v),
            _ => None,
        },
        _ => None,
    }
}

/// The segment drawing `label`: a radio button with that word inside it.
fn segment_with_label(app: &mut App, label: &str) -> Entity {
    let body = node_card_body(app);
    for entity in descendants(app.world_mut(), body) {
        if app
            .world()
            .get::<bevy::ui_widgets::RadioButton>(entity)
            .is_none()
        {
            continue;
        }
        let draws_label = app.world().get::<Children>(entity).is_some_and(|children| {
            children.iter().any(|child| {
                app.world()
                    .get::<Text>(child)
                    .is_some_and(|text| text.0 == label)
            })
        });
        if draws_label {
            return entity;
        }
    }
    panic!("no `{label}` segment on the card");
}

/// Click a segment the way a user does: the `Pointer<Click>` its observer
/// is watching for.
fn click_segment(app: &mut App, segment: Entity) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        Click {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: core::time::Duration::ZERO,
            count: 1,
        },
        segment,
    ));
    app.update();
}

fn pick_option(app: &mut App, combo: Entity, index: usize, label: &str) {
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo,
        selected: index,
        label: label.to_string(),
        value: None,
    });
    app.update();
}

/// Nothing about `Node` may reach the generic renderer: `EnumVariantHost` is
/// that renderer's marker, and a `Node` producing one would be showing variant
/// menus where the card promises controls.
#[test]
fn a_node_selection_builds_the_card_and_nothing_generic() {
    let (mut app, _) = app_with_node_card(Node {
        width: Val::Px(120.0),
        ..default()
    });

    let _ = node_card_body(&mut app);
    let generic_rows = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::inspector::EnumVariantHost>>()
        .iter(app.world())
        .count();
    assert_eq!(
        generic_rows, 0,
        "a Node's enums are card controls, not generic variant menus",
    );
}

/// One click on the segmented control changes the layout model, as one undoable
/// document edit.
#[test]
fn a_display_segment_click_commits_and_undoes() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let grid = segment_with_label(&mut app, "Grid");

    click_segment(&mut app, grid);

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.display),
        Some(Display::Grid),
        "the live component must move",
    );
    assert_eq!(
        authored(&app, entity, "display"),
        Some(BsnValue::Type(
            "bevy_ui::ui_node::Display::Grid".to_string()
        )),
        "and the document records the variant",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        1,
        "one click mints exactly one undo entry",
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    app.update();
    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.display),
        Some(Display::Flex),
        "undo restores the pre-click value",
    );
    assert_eq!(
        authored(&app, entity, "display"),
        None,
        "and takes the authored override back out of the document",
    );
}

/// The card's segmented controls are radio groups, and the variant the component
/// holds is the checked one.
#[test]
fn the_card_segments_are_a_radio_group() {
    use bevy::ui::Checked;
    use bevy::ui_widgets::{RadioButton, RadioGroup};

    let (mut app, _entity) = app_with_node_card(Node::default());
    let grid = segment_with_label(&mut app, "Grid");
    let flex = segment_with_label(&mut app, "Flex");

    for segment in [grid, flex] {
        assert!(
            app.world().get::<RadioButton>(segment).is_some(),
            "a segment is a radio button",
        );
        assert!(
            app.world().get::<Interaction>(segment).is_none(),
            "and not a hand-rolled interaction control",
        );
    }
    let bar = app
        .world()
        .get::<ChildOf>(grid)
        .expect("a segment sits in a bar")
        .parent();
    assert!(
        app.world().get::<RadioGroup>(bar).is_some(),
        "the bar the segments share is the radio group",
    );
    assert!(
        app.world().get::<Checked>(flex).is_some(),
        "the variant the component holds is checked",
    );
    assert!(app.world().get::<Checked>(grid).is_none());

    click_segment(&mut app, grid);
    refresh_card(&mut app);
    assert!(
        app.world().get::<Checked>(grid).is_some(),
        "and the check follows the pick",
    );
    assert!(app.world().get::<Checked>(flex).is_none());
}

/// A segment that does not match the component is not the lit one.
#[test]
fn the_lit_segment_follows_the_component() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let grid = segment_with_label(&mut app, "Grid");
    let flex = segment_with_label(&mut app, "Flex");

    click_segment(&mut app, grid);
    refresh_card(&mut app);
    assert_ne!(
        app.world().get::<BackgroundColor>(grid).map(|c| c.0),
        Some(Color::NONE),
        "the picked segment lights up",
    );
    assert_eq!(
        app.world().get::<BackgroundColor>(flex).map(|c| c.0),
        Some(Color::NONE),
        "and the one it replaced goes dark",
    );

    app.world_mut().entity_mut(entity).insert(Node {
        display: Display::Flex,
        ..default()
    });
    refresh_card(&mut app);
    assert_eq!(
        app.world().get::<BackgroundColor>(grid).map(|c| c.0),
        Some(Color::NONE),
        "a change the card did not make moves the highlight too",
    );
}

/// The align group's dropdowns write the enum they name.
#[test]
fn an_align_combobox_commit_updates_align_items() {
    let (mut app, entity) = app_with_node_card(Node::default());
    assert_eq!(
        section_combos(&mut app, "Align").len(),
        3,
        "items, self, content",
    );
    let items = row_combo(&mut app, "Align", "items");

    // Index 5 of AlignItems is `Center`.
    pick_option(&mut app, items, 5, "Center");

    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.align_items),
        Some(AlignItems::Center),
    );
    assert_eq!(
        authored(&app, entity, "align_items"),
        Some(BsnValue::Type(
            "bevy_ui::ui_node::AlignItems::Center".to_string()
        )),
    );
    assert_eq!(app.world().resource::<CommandHistory>().undo_stack.len(), 1);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    app.update();
    assert_eq!(
        app.world().get::<Node>(entity).map(|n| n.align_items),
        Some(AlignItems::Default),
        "undo restores the pre-pick value",
    );
    assert_eq!(
        authored(&app, entity, "align_items"),
        None,
        "and the document forgets the override",
    );
}

/// The justify group shares row labels with the align group, so picking in one
/// must not move the other.
#[test]
fn the_justify_group_writes_its_own_fields() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let content = row_combo(&mut app, "Justify", "content");

    // Index 5 of JustifyContent is `Center`.
    pick_option(&mut app, content, 5, "Center");

    let node = app.world().get::<Node>(entity).cloned().unwrap_or_default();
    assert_eq!(node.justify_content, JustifyContent::Center);
    assert_eq!(
        node.align_content,
        AlignContent::Default,
        "the align group is untouched",
    );
}

/// `aspect_ratio` is an `Option`, which the generic renderer shows as a variant
/// menu. The card asks one question, auto or a number, and authors either.
#[test]
fn the_aspect_ratio_row_switches_between_auto_and_a_number() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let mode = row_combo(&mut app, "Size", "aspect ratio");

    pick_option(&mut app, mode, 1, "set");

    assert_eq!(
        app.world().get::<Node>(entity).and_then(|n| n.aspect_ratio),
        Some(1.0),
        "leaving auto starts at a ratio that exists",
    );

    pick_option(&mut app, mode, 0, "auto");
    assert_eq!(
        app.world().get::<Node>(entity).and_then(|n| n.aspect_ratio),
        None,
        "and going back takes the number away",
    );
}

/// Scrubbing the number half of an optional row authors the value, and the grid
/// lines take it as the `NonZero` they are.
#[test]
fn an_optional_rows_number_commits_and_a_grid_line_takes_it() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let ratio_row = row_of_label(&mut app, "Size", "aspect ratio");
    let mode = row_combo(&mut app, "Size", "aspect ratio");
    pick_option(&mut app, mode, 1, "set");

    let number = number_input(&mut app, ratio_row);
    scrub(&mut app, number, 1.5, true);
    assert_eq!(
        app.world().get::<Node>(entity).and_then(|n| n.aspect_ratio),
        Some(1.5),
    );

    let start = row_combo(&mut app, "Grid", "row start");
    pick_option(&mut app, start, 1, "set");
    assert_eq!(
        app.world()
            .get::<Node>(entity)
            .and_then(|n| n.grid_row.get_start()),
        Some(1),
        "a grid line reaches the component as a NonZero",
    );
}

/// A drag across an optional row is one edit: ticks preview on live ECS and only
/// the release mints history.
#[test]
fn an_optional_number_drag_lands_as_one_history_entry() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let mode = row_combo(&mut app, "Size", "aspect ratio");
    pick_option(&mut app, mode, 1, "set");
    let settled = app.world().resource::<CommandHistory>().undo_stack.len();

    let row = row_of_label(&mut app, "Size", "aspect ratio");
    let number = number_input(&mut app, row);
    scrub(&mut app, number, 1.2, false);
    scrub(&mut app, number, 1.4, false);
    scrub(&mut app, number, 1.6, false);

    assert_eq!(
        app.world().get::<Node>(entity).and_then(|n| n.aspect_ratio),
        Some(1.6),
        "a drag tick previews on live ECS",
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        settled,
        "a drag tick must not mint undo",
    );

    scrub(&mut app, number, 1.8, true);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        settled + 1,
        "the whole drag lands as one undo entry",
    );
    assert_eq!(
        app.world().get::<Node>(entity).and_then(|n| n.aspect_ratio),
        Some(1.8),
    );
}

/// A grid line stops at a number the layout engine survives: truncating would
/// leave the document holding a value the component refused, and the raw
/// `NonZeroU16` ceiling panics taffy.
#[test]
fn a_grid_line_scrub_stops_where_the_layout_engine_does() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let row = row_of_label(&mut app, "Grid", "row span");
    let number = number_input(&mut app, row);

    scrub(&mut app, number, 90_000.0, true);

    assert_eq!(
        app.world()
            .get::<Node>(entity)
            .and_then(|n| n.grid_row.get_span()),
        Some(1000),
        "clamped to a span the grid can lay out",
    );
    assert_eq!(
        authored_option_int(&app, entity, "grid_row.span"),
        Some(1000),
        "and the document says the same number",
    );
}

/// Zero is not a ratio, so the row goes back to `auto` rather than authoring a
/// degenerate box.
#[test]
fn a_ratio_scrubbed_to_zero_goes_back_to_auto() {
    let (mut app, entity) = app_with_node_card(Node::default());
    let mode = row_combo(&mut app, "Size", "aspect ratio");
    pick_option(&mut app, mode, 1, "set");
    let row = row_of_label(&mut app, "Size", "aspect ratio");
    let number = number_input(&mut app, row);

    scrub(&mut app, number, 0.0, true);

    assert_eq!(
        app.world().get::<Node>(entity).and_then(|n| n.aspect_ratio),
        None,
        "zero is not a ratio, so the field goes absent",
    );
    refresh_card(&mut app);
    assert!(
        app.world().get::<InteractionDisabled>(number).is_some(),
        "and the row shows it: no number to edit",
    );
}

/// The widget on the card that writes `field_path`, found by the address it
/// edits rather than by where it sits.
fn field_widget(app: &mut App, field_path: &str) -> Entity {
    let body = node_card_body(app);
    descendants(app.world_mut(), body)
        .into_iter()
        .find(|entity| {
            jackdaw::inspector::field_edited_by(app.world(), *entity) == Some((NODE, field_path))
        })
        .unwrap_or_else(|| panic!("no control on the card writes `{field_path}`"))
}

/// The four grid track lists go through the generic reflect renderer, so a track
/// is a row of controls rather than a count in disabled text.
#[test]
fn a_grid_track_list_is_edited_rather_than_counted() {
    let (mut app, entity) = app_with_node_card(Node {
        display: Display::Grid,
        grid_template_columns: RepeatedGridTrack::px(2, 40.0),
        ..default()
    });

    let repetition = field_widget(&mut app, "grid_template_columns[0].repetition.0");
    app.world_mut().trigger(ValueChange {
        source: repetition,
        value: 5_i64,
        is_final: true,
    });
    app.update();

    assert_eq!(
        app.world()
            .get::<Node>(entity)
            .map(|node| node.grid_template_columns.clone()),
        Some(RepeatedGridTrack::px(5, 40.0)),
        "the track's repeat count is a control the card commits",
    );
}

/// The same for a plain track list, whose items are sizing functions.
#[test]
fn an_auto_track_list_is_edited_rather_than_counted() {
    let (mut app, entity) = app_with_node_card(Node {
        display: Display::Grid,
        grid_auto_columns: GridTrack::px(40.0),
        ..default()
    });

    let size = field_widget(&mut app, "grid_auto_columns[0].max_sizing_function.0");
    app.world_mut().trigger(ValueChange {
        source: size,
        value: 64.0_f64,
        is_final: true,
    });
    app.update();

    let tracks = app
        .world()
        .get::<Node>(entity)
        .map(|node| node.grid_auto_columns.clone())
        .expect("the entity still carries its Node");
    assert_eq!(
        tracks,
        GridTrack::minmax::<Vec<GridTrack>>(
            MinTrackSizingFunction::Px(40.0),
            MaxTrackSizingFunction::Px(64.0),
        ),
        "the track's maximum size is a control the card commits",
    );
}

// ---------------------------------------------------------------------------
// The decoration gutter
// ---------------------------------------------------------------------------

/// Widths the rows are measured at, from the shipped docked inspector down to
/// where a mark's strip costs the control the room it was using. What a control
/// gets is not monotonic in the panel's width: 212 px is the widest panel that
/// still keeps label and control on one line, and 150 px covers the wrap point
/// where the control drops onto its own line with more room.
const PANEL_WIDTHS: [f32; 5] = [260.0, 212.0, 150.0, 120.0, 100.0];

/// The width a unit dropdown stops reading at: "vmin" plus the chevron.
const UNIT_DROPDOWN_FLOOR: f32 = 34.0;

/// The width a standalone magnitude stops reading at: three digits.
const NUMBER_FLOOR: f32 = 28.0;

/// What one of a rect's four cells is left with instead, four-up on a line.
const RECT_NUMBER_FLOOR: f32 = 24.0;

/// The two length rows a `Node` card lays out: one length, and the four-cell
/// rect the margin and padding fields use.
#[derive(Clone, Copy)]
enum LengthRow {
    One,
    Rect,
}

/// One length row laid out inside a fixed-width column, optionally showing a
/// property mark. Returns the row after a real layout pass.
fn laid_out_length_row(
    app: &mut App,
    entity: Entity,
    kind: LengthRow,
    width: f32,
    marked: bool,
) -> Entity {
    let column = app
        .world_mut()
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Px(width),
            flex_direction: FlexDirection::Column,
            ..default()
        })
        .id();
    let world = app.world_mut();
    let row = {
        let mut commands = world.commands();
        match kind {
            LengthRow::One => spawn_val_field(
                &mut commands,
                column,
                "width",
                Val::Px(120.0),
                0,
                "width".to_string(),
                entity,
                NODE,
            ),
            LengthRow::Rect => spawn_ui_rect_field(
                &mut commands,
                column,
                "margin",
                UiRect::all(Val::Px(4.0)),
                0,
                "margin".to_string(),
                entity,
                NODE,
            ),
        }
    };
    world.flush();
    if marked {
        // What the three decorators hang off a row: an absolutely-positioned
        // wrapper at the row's right edge, carrying the shared marker.
        world.spawn((
            jackdaw_feathers::field_row::FieldRowDecoration,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(2.0),
                right: Val::Px(20.0),
                width: Val::Px(8.0),
                height: Val::Px(8.0),
                ..default()
            },
            ChildOf(row),
        ));
    }
    // The editor test app sits in `ProjectSelect`, so the inspector's Update
    // systems are not running; the gutter pass is invoked by hand.
    app.update();
    app.world_mut()
        .run_system_cached(jackdaw_feathers::field_row::reserve_decoration_gutters)
        .expect("the gutter pass runs");
    app.update();
    row
}

/// How far short of the row's right edge its rightmost control stops. A rect
/// row has four cells; the last one is the one a mark would land on.
fn control_clearance(app: &mut App, row: Entity) -> f32 {
    let selects = widgets_with::<EditorComboBox>(app.world_mut(), row);
    assert!(!selects.is_empty(), "the row has at least one dropdown");
    let rightmost = selects
        .into_iter()
        .map(|select| right_edge(app, select))
        .fold(f32::MIN, f32::max);
    right_edge(app, row) - rightmost
}

/// Every dropdown in the row, by laid-out width.
fn dropdown_widths(app: &mut App, row: Entity) -> Vec<f32> {
    widgets_with::<EditorComboBox>(app.world_mut(), row)
        .into_iter()
        .map(|select| {
            app.world()
                .get::<bevy::ui::ComputedNode>(select)
                .unwrap_or_else(|| panic!("{select} is laid out"))
                .size()
                .x
        })
        .collect()
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

/// The gutter is only worth reserving if the control really stops short of it.
/// Measured against a layout pass, not the padding value: a control that refuses
/// to shrink overflows the reduced box and ends where it always did.
#[test]
fn a_marked_row_keeps_its_control_clear_of_the_mark() {
    for kind in [LengthRow::One, LengthRow::Rect] {
        for width in PANEL_WIDTHS {
            let (mut app, entity) = app_with_node(Node::default());
            let row = laid_out_length_row(&mut app, entity, kind, width, true);
            let clearance = control_clearance(&mut app, row);
            assert!(
                clearance >= 40.0,
                "a marked row's control ends clear of the strip the mark sits in; \
                 only {clearance} px of clearance in a {width} px panel",
            );
        }
    }
}

/// Both halves of a length have a width they stop reading at, and neither may be
/// paid for by the other or by hanging off the row.
///
/// A headless app has no font, so digits measure nothing and the row never runs
/// out of room. The stand-in child gives each field the width its glyphs take,
/// at a short value and at the ~87 px a full one measures.
#[test]
fn both_halves_of_a_length_keep_a_width_they_read_at() {
    for kind in [LengthRow::One, LengthRow::Rect] {
        for content in NUMBER_CONTENT_WIDTHS {
            for width in PANEL_WIDTHS {
                let (mut app, entity) = app_with_node(Node::default());
                let row = laid_out_length_row(&mut app, entity, kind, width, false);
                crowd_number_fields(&mut app, row, content);
                let panel = format!("a {width} px panel, {content} px of digits");

                for dropdown in dropdown_widths(&mut app, row) {
                    assert!(
                        dropdown >= UNIT_DROPDOWN_FLOOR,
                        "a unit dropdown keeps {UNIT_DROPDOWN_FLOOR} px to read in; \
                         it got {dropdown} px in {panel}",
                    );
                }
                // A rect's four cells carry no hard floor: on a marked narrow
                // row the unit stops meaning anything first.
                let floor = match kind {
                    LengthRow::One => NUMBER_FLOOR,
                    LengthRow::Rect => RECT_NUMBER_FLOOR,
                };
                for number in number_widths(&mut app, row) {
                    assert!(
                        number >= floor,
                        "a magnitude keeps {floor} px to read in; \
                         it got {number} px in {panel}",
                    );
                }
                // A control that keeps its size by hanging off the row is
                // clipped at the panel's edge, down to a single glyph.
                let clearance = control_clearance(&mut app, row);
                assert!(
                    clearance >= 0.0,
                    "the controls end inside the row rather than past it; \
                     they overhang by {} px in {panel}",
                    -clearance,
                );
            }
        }
    }
}

/// Widths a number field's digits take once a font has measured them.
const NUMBER_CONTENT_WIDTHS: [f32; 2] = [45.0, 80.0];

/// Give every number field in the row the content a real font would put in
/// it, and lay the row out again.
fn crowd_number_fields(app: &mut App, row: Entity, content: f32) {
    for field in widgets_with::<ScrubNumberInput>(app.world_mut(), row) {
        app.world_mut().spawn((
            Node {
                width: Val::Px(content),
                height: Val::Px(1.0),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(field),
        ));
    }
    app.update();
    app.update();
}

/// Every number field in the row, by laid-out width.
fn number_widths(app: &mut App, row: Entity) -> Vec<f32> {
    widgets_with::<ScrubNumberInput>(app.world_mut(), row)
        .into_iter()
        .map(|field| {
            app.world()
                .get::<bevy::ui::ComputedNode>(field)
                .unwrap_or_else(|| panic!("{field} is laid out"))
                .size()
                .x
        })
        .collect()
}

/// A row with no mark keeps the width instead of paying for a strip nothing is
/// using: the diamond is `Transform`-only and the prefab dot needs an instance.
#[test]
fn an_unmarked_row_spends_the_gutter_on_its_control() {
    for kind in [LengthRow::One, LengthRow::Rect] {
        for width in PANEL_WIDTHS {
            let (mut app, entity) = app_with_node(Node::default());
            let marked = laid_out_length_row(&mut app, entity, kind, width, true);
            let plain = laid_out_length_row(&mut app, entity, kind, width, false);

            let marked_clearance = control_clearance(&mut app, marked);
            let plain_clearance = control_clearance(&mut app, plain);
            assert!(
                plain_clearance < marked_clearance - 30.0,
                "an unmarked row gives its control the strip back: {plain_clearance} px clear \
                 against the marked row's {marked_clearance} px, in a {width} px panel",
            );
        }
    }
}
