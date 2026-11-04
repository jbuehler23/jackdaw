//! The `RecipeInstance` card: the recipe's inputs as fields that rebuild the
//! root as they change, then its calls under a collapsed Steps section, where
//! a literal parameter can be promoted to a new input.

use bevy::{ecs::system::SystemState, prelude::*, ui::Checked, ui_widgets::ValueChange};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{BatchTransaction, HistoryHold};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonProps, ButtonSize, IconButtonProps, button, icon_button},
    combobox::{ComboBoxChangeEvent, combobox_with_selected},
    field_row::{FieldRowProps, spawn_field_row},
    icons::{Icon, IconFont},
    number_input::{HardLimit, NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue},
    text_edit::{
        TextEditCommitEvent, TextEditProps, TextEditWrapper, set_text_input_value, text_edit,
    },
    tokens,
    tooltip::Tooltip,
    utils::{find_ancestor, set_marker_if_alive},
};
use jackdaw_recipe::{InputKind, InputSpec, Value as RecipeValue};
use jackdaw_scene_types::RecipeInstance;
use serde_json::{Value, json};

use crate::recipes::RecipeView;

/// A field editing input `input` of the recipe `root` was built from; for a
/// vector, `component` is the one it edits.
#[derive(Component, Clone, Debug)]
pub struct RecipeInputField {
    pub root: Entity,
    pub input: String,
    pub kind: InputKind,
    pub component: Option<usize>,
}

/// The collapsed section holding the calls of the recipe `root` was built
/// from.
#[derive(Component)]
pub struct RecipeSteps {
    pub root: Entity,
}

/// The row of parameter `param` of step `step`.
#[derive(Component, Clone, Debug)]
pub struct RecipeStepParam {
    pub step: usize,
    pub param: String,
}

/// The button that shows or hides a parameter's promote form.
#[derive(Component)]
pub struct RecipePromoteToggle {
    pub form: Entity,
}

/// What promoting one parameter will ask for, as the form holds it.
#[derive(Component, Clone, Debug)]
pub struct RecipePromoteForm {
    pub root: Entity,
    pub step: usize,
    pub param: String,
    pub name: String,
    pub kind: String,
    pub default: String,
    pub min: String,
    pub max: String,
}

/// Which text of a promote form a field edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromoteText {
    Name,
    Default,
    Min,
    Max,
}

#[derive(Component)]
pub struct RecipePromoteField {
    pub form: Entity,
    pub text: PromoteText,
}

#[derive(Component)]
pub struct RecipePromoteKind {
    pub form: Entity,
}

#[derive(Component)]
pub struct RecipePromoteButton {
    pub form: Entity,
}

/// A drag on an input, its rebuilds held as one undo step until it ends.
#[derive(Resource)]
struct InputDrag {
    root: Entity,
    transaction: BatchTransaction,
}

const DRAG_HOLD: &str = "a recipe input is being dragged";

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(on_input_number)
        .add_observer(on_input_bool)
        .add_observer(on_text_commit)
        .add_observer(on_click)
        .add_observer(on_kind_pick)
        .add_observer(show_written_inputs);
}

/// Fill the `RecipeInstance` card body for `source`. World-exclusive and
/// deferred by the caller.
pub(crate) fn fill_recipe_card_body(world: &mut World, source: Entity, body: Entity) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(instance) = world.get::<RecipeInstance>(source).cloned() else {
        return;
    };
    let view = RecipeView::read(world, &instance.recipe);
    let icon_font = (world.get_resource::<IconFont>()).map_or_else(default, |f| f.0.clone());
    let specs = view
        .as_ref()
        .map(|view| (view.inputs.clone(), view.calls.clone(), view.is_starter()));
    let mut state: SystemState<Commands> = SystemState::new(world);
    let Ok(mut commands) = state.get_mut(world) else {
        return;
    };
    match specs {
        Err(why) => {
            commands.spawn((
                Text::new(why),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                ChildOf(body),
            ));
        }
        Ok((inputs, calls, starter)) => {
            for spec in &inputs {
                let value = instance
                    .inputs
                    .get(&spec.name)
                    .and_then(|text| spec.read_text(text).ok())
                    .unwrap_or_else(|| spec.default.clone());
                spawn_input_row(&mut commands, body, source, spec, &value);
            }
            spawn_steps(&mut commands, body, source, &calls, starter, &icon_font);
        }
    }
    state.apply(world);
    world.flush();
}

fn spawn_input_row(
    commands: &mut Commands,
    body: Entity,
    root: Entity,
    spec: &InputSpec,
    value: &RecipeValue,
) {
    let row = spawn_field_row(
        commands,
        body,
        FieldRowProps::new(super::sentence(&spec.name)),
    );
    let field = |component| RecipeInputField {
        root,
        input: spec.name.clone(),
        kind: spec.kind,
        component,
    };
    match spec.kind {
        InputKind::Float | InputKind::Int => {
            let number = number_of(value);
            spawn_number(commands, row.control, spec, number, field(None));
        }
        InputKind::Vec2 | InputKind::Vec3 => {
            let parts: Vec<f64> = match value {
                RecipeValue::List(items) => items.iter().map(number_of).collect(),
                _ => Vec::new(),
            };
            for (index, part) in parts.into_iter().enumerate() {
                spawn_number(commands, row.control, spec, part, field(Some(index)));
            }
        }
        InputKind::Bool => {
            let checked = matches!(value, RecipeValue::Bool(true));
            let toggle =
                super::reflect_fields::spawn_checkbox(commands, row.control, None, checked);
            commands.entity(toggle).insert(field(None));
        }
        InputKind::Text => {
            let text = jackdaw_recipe::value_text(value);
            commands.spawn((
                text_edit(
                    TextEditProps::default()
                        .with_default_value(text)
                        .allow_empty(),
                ),
                field(None),
                ChildOf(row.control),
            ));
        }
    }
}

fn number_of(value: &RecipeValue) -> f64 {
    match value {
        RecipeValue::Number(number) => *number,
        _ => 0.0,
    }
}

fn spawn_number(
    commands: &mut Commands,
    parent: Entity,
    spec: &InputSpec,
    value: f64,
    field: RecipeInputField,
) {
    let precision = match spec.kind {
        InputKind::Int => 0,
        _ => 3,
    };
    let input = commands
        .spawn_scene(bsn! { @ScrubNumberInput })
        .insert((
            ScrubNumberInputValue::F64(value),
            NumberInputPrecision(precision),
            field,
            ChildOf(parent),
        ))
        .id();
    if let Some((min, max)) = spec.range {
        commands.entity(input).insert(HardLimit::f64(min..max));
    }
}

fn spawn_steps(
    commands: &mut Commands,
    body: Entity,
    root: Entity,
    calls: &[crate::recipes::RecipeCall],
    starter: bool,
    icon_font: &Handle<Font>,
) {
    let (section, steps) =
        jackdaw_feathers::collapsible::collapsible_section_open(commands, "Steps", body, false);
    commands.entity(section).insert(RecipeSteps { root });
    for (index, call) in calls.iter().enumerate() {
        commands.spawn((
            Text::new(format!("{index}  {}", call.id)),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(steps),
        ));
        for (param, text) in &call.params {
            let row = spawn_field_row(
                commands,
                steps,
                FieldRowProps {
                    indent: 1,
                    ..FieldRowProps::new(param.clone())
                },
            );
            commands.entity(row.row).insert(RecipeStepParam {
                step: index,
                param: param.clone(),
            });
            commands.spawn((
                Text::new(text.clone()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                ChildOf(row.control),
            ));
            if starter || text.contains('$') {
                continue;
            }
            let form = spawn_promote_form(commands, steps, root, index, param, text);
            commands.spawn((
                icon_button(
                    IconButtonProps::new(Icon::SlidersHorizontal).with_size(ButtonSize::IconSM),
                    icon_font,
                ),
                Tooltip::title("Promote to input"),
                RecipePromoteToggle { form },
                ChildOf(row.control),
            ));
        }
    }
}

fn spawn_promote_form(
    commands: &mut Commands,
    parent: Entity,
    root: Entity,
    step: usize,
    param: &str,
    text: &str,
) -> Entity {
    let form = commands
        .spawn((
            RecipePromoteForm {
                root,
                step,
                param: param.to_string(),
                name: param.to_string(),
                kind: String::new(),
                default: text.to_string(),
                min: String::new(),
                max: String::new(),
            },
            Node {
                display: Display::None,
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    for (label, which, value) in [
        ("Name", PromoteText::Name, param),
        ("Default", PromoteText::Default, text),
        ("Min", PromoteText::Min, ""),
        ("Max", PromoteText::Max, ""),
    ] {
        let row = spawn_field_row(
            commands,
            form,
            FieldRowProps {
                indent: 2,
                ..FieldRowProps::new(label)
            },
        );
        commands.spawn((
            text_edit(
                TextEditProps::default()
                    .with_default_value(value.to_string())
                    .allow_empty(),
            ),
            RecipePromoteField { form, text: which },
            ChildOf(row.control),
        ));
    }
    let row = spawn_field_row(
        commands,
        form,
        FieldRowProps {
            indent: 2,
            ..FieldRowProps::new("Kind")
        },
    );
    let mut kinds = vec!["auto".to_string()];
    kinds.extend(InputKind::NAMES.iter().map(ToString::to_string));
    commands.spawn((
        combobox_with_selected(kinds, 0),
        RecipePromoteKind { form },
        ChildOf(row.control),
    ));
    commands.spawn((
        button(ButtonProps::new("Promote")),
        RecipePromoteButton { form },
        ChildOf(form),
    ));
    form
}

fn on_input_number(
    change: On<ValueChange<f64>>,
    fields: Query<&RecipeInputField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(change.source).cloned() else {
        return;
    };
    commands
        .entity(change.source)
        .insert(ScrubNumberInputValue::F64(change.value));
    let (value, is_final) = (change.value, change.is_final);
    commands.queue(move |world: &mut World| edit_input(world, &field, json!(value), is_final));
}

fn on_input_bool(
    change: On<ValueChange<bool>>,
    fields: Query<&RecipeInputField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(change.source).cloned() else {
        return;
    };
    let value = change.value;
    set_marker_if_alive::<Checked>(&mut commands, change.source, value);
    commands.queue(move |world: &mut World| edit_input(world, &field, json!(value), true));
}

fn on_text_commit(
    commit: On<TextEditCommitEvent>,
    fields: Query<&RecipeInputField>,
    promote_fields: Query<&RecipePromoteField>,
    mut forms: Query<&mut RecipePromoteForm>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let text = commit.text.trim().to_string();
    if let Some((_, field)) = find_ancestor(commit.entity, &fields, &parents) {
        let field = field.clone();
        commands.queue(move |world: &mut World| edit_input(world, &field, json!(text), true));
        return;
    }
    if let Some((_, field)) = find_ancestor(commit.entity, &promote_fields, &parents)
        && let Ok(mut form) = forms.get_mut(field.form)
    {
        match field.text {
            PromoteText::Name => form.name = text,
            PromoteText::Default => form.default = text,
            PromoteText::Min => form.min = text,
            PromoteText::Max => form.max = text,
        }
    }
}

fn on_kind_pick(
    pick: On<ComboBoxChangeEvent>,
    combos: Query<&RecipePromoteKind>,
    mut forms: Query<&mut RecipePromoteForm>,
) {
    let Ok(combo) = combos.get(pick.entity) else {
        return;
    };
    if let Ok(mut form) = forms.get_mut(combo.form) {
        form.kind = pick
            .selected
            .checked_sub(1)
            .and_then(|index| InputKind::NAMES.get(index))
            .map_or_else(String::new, ToString::to_string);
    }
}

fn on_click(
    click: On<ButtonClickEvent>,
    toggles: Query<&RecipePromoteToggle>,
    buttons: Query<&RecipePromoteButton>,
    forms: Query<&RecipePromoteForm>,
    mut nodes: Query<&mut Node>,
    mut commands: Commands,
) {
    if let Ok(toggle) = toggles.get(click.entity) {
        if let Ok(mut node) = nodes.get_mut(toggle.form) {
            node.display = match node.display {
                Display::None => Display::Flex,
                _ => Display::None,
            };
        }
        return;
    }
    let Some(form) = buttons
        .get(click.entity)
        .ok()
        .and_then(|button| forms.get(button.form).ok())
    else {
        return;
    };
    let mut call = commands
        .operator(crate::recipes::RecipePromoteOp::ID)
        .param("entity", form.root)
        .param("step", form.step as i64)
        .param("param", form.param.clone());
    if !form.name.is_empty() {
        call = call.param("input", form.name.clone());
    }
    if !form.kind.is_empty() {
        call = call.param("kind", form.kind.clone());
    }
    if !form.default.is_empty() {
        call = call.param("default", form.default.clone());
    }
    if let (Ok(min), Ok(max)) = (form.min.parse::<f64>(), form.max.parse::<f64>()) {
        call = call.param("min", min).param("max", max);
    }
    call.call();
}

/// Rebuild the root with `field` set to `value`. A value mid-drag joins the
/// drag's one undo step; the final one closes it.
fn edit_input(world: &mut World, field: &RecipeInputField, value: Value, is_final: bool) {
    let Some(instance) = world.get::<RecipeInstance>(field.root).cloned() else {
        return;
    };
    let value = match field.component {
        None => value,
        Some(component) => {
            let mut parts: Vec<Value> = instance
                .inputs
                .get(&field.input)
                .map(|text| {
                    text.trim_matches(['[', ']'])
                        .split(',')
                        .map(|part| part.trim().parse::<f64>().map_or(json!(0.0), |n| json!(n)))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(slot) = parts.get_mut(component) {
                *slot = value;
            }
            Value::Array(parts)
        }
    };
    let value = match value.as_f64() {
        Some(number) if field.kind == InputKind::Int => json!(number.round() as i64),
        _ => value,
    };
    let dragging = world
        .get_resource::<InputDrag>()
        .is_some_and(|drag| drag.root == field.root);
    if !is_final && !dragging {
        finish_drag(world);
        let transaction = BatchTransaction::begin(world, "Edit Recipe Input", false);
        world.get_resource_or_init::<HistoryHold>().hold(DRAG_HOLD);
        world.insert_resource(InputDrag {
            root: field.root,
            transaction,
        });
    }
    let inputs = json!({ field.input.clone(): value });
    let result = world
        .operator(crate::recipes::RecipeRerunOp::ID)
        .param("entity", field.root)
        .param("inputs", inputs.to_string())
        .call();
    if let Err(err) = result {
        warn!("recipe input: {err}");
    }
    if is_final {
        finish_drag(world);
    }
}

fn finish_drag(world: &mut World) {
    if let Some(drag) = world.remove_resource::<InputDrag>() {
        world.get_resource_or_init::<HistoryHold>().release();
        drag.transaction.finish(world, true);
    }
}

/// Show the inputs a root was rebuilt with when something other than a drag
/// on its card rebuilt it: an undo, or a caller's rerun. A root whose recipe
/// gained or lost an input gets its card built again.
fn show_written_inputs(written: On<Insert<RecipeInstance>>, mut commands: Commands) {
    let root = written.entity;
    commands.queue(move |world: &mut World| refresh_inputs(world, root));
}

fn refresh_inputs(world: &mut World, root: Entity) {
    if world
        .get_resource::<InputDrag>()
        .is_some_and(|drag| drag.root == root)
    {
        return;
    }
    let Some(instance) = world.get::<RecipeInstance>(root).cloned() else {
        return;
    };
    let fields: Vec<(Entity, RecipeInputField)> = world
        .query::<(Entity, &RecipeInputField)>()
        .iter(world)
        .filter(|(_, field)| field.root == root)
        .map(|(entity, field)| (entity, field.clone()))
        .collect();
    let shown_here = world
        .query::<&RecipeSteps>()
        .iter(world)
        .any(|steps| steps.root == root);
    if !shown_here {
        return;
    }
    let shown: std::collections::BTreeSet<&str> = fields
        .iter()
        .map(|(_, field)| field.input.as_str())
        .collect();
    let held: std::collections::BTreeSet<&str> =
        instance.inputs.keys().map(String::as_str).collect();
    if shown != held {
        if let Ok(mut entity) = world.get_entity_mut(root) {
            entity.insert(super::InspectorDirty);
        }
        return;
    }
    for (entity, field) in fields {
        let Some(text) = instance.inputs.get(&field.input) else {
            continue;
        };
        match field.kind {
            InputKind::Bool => {
                let on = text == "true";
                if let Ok(mut toggle) = world.get_entity_mut(entity)
                    && toggle.contains::<Checked>() != on
                {
                    match on {
                        true => toggle.insert(Checked),
                        false => toggle.remove::<Checked>(),
                    };
                }
            }
            InputKind::Text => show_text(world, entity, text),
            InputKind::Float | InputKind::Int | InputKind::Vec2 | InputKind::Vec3 => {
                let parts: Vec<f64> = text
                    .trim_matches(['[', ']'])
                    .split(',')
                    .filter_map(|part| part.trim().parse().ok())
                    .collect();
                let Some(&number) = parts.get(field.component.unwrap_or(0)) else {
                    continue;
                };
                let shown = ScrubNumberInputValue::F64(number);
                if world.get::<ScrubNumberInputValue>(entity) != Some(&shown)
                    && let Ok(mut input) = world.get_entity_mut(entity)
                {
                    input.insert(shown);
                }
            }
        }
    }
}

/// Put `text` in the text field `field`, unless it already reads that.
fn show_text(world: &mut World, field: Entity, text: &str) {
    let inner: Vec<Entity> = world
        .get::<Children>(field)
        .into_iter()
        .flat_map(Children::iter)
        .filter_map(|child| world.get::<TextEditWrapper>(child).map(|wrapper| wrapper.0))
        .collect();
    for editable in inner {
        if let Some(mut editable) = world.get_mut::<bevy::text::EditableText>(editable)
            && editable.value() != text
        {
            set_text_input_value(&mut editable, text.to_string());
        }
    }
}
