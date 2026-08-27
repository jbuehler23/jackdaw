//! Compact composite editors for `Val` and `UiRect`.
//!
//! A `Val` is a unit-tagged length, and rendering it as a plain enum buries a
//! single number under a variant menu. These fields collapse it back to one
//! row: a scrub number plus a unit dropdown. `UiRect` stacks four of those.
//!
//! The shape follows the color composite: the field root holds the edit state
//! and a commit closure, and one pair of global observers serves every field,
//! self-filtering on the sub-widget markers. The row around it is the shared
//! [`spawn_field_row`] one.

use bevy::{
    ecs::reflect::AppTypeRegistry, prelude::*, reflect::GetPath, ui::InteractionDisabled,
    ui_widgets::ValueChange,
};
use jackdaw_feathers::{
    combobox::{
        ComboBoxChangeEvent, ComboBoxOptionData, ComboBoxSelectedIndex, combobox_with_selected,
    },
    field_row::{FieldRowProps, spawn_field_row},
    number_input::{NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue},
    tokens,
};

use super::InspectorFieldRow;

/// Decimal places the magnitude input shows and rounds a drag to.
const VAL_PRECISION: i32 = 2;
/// Width of the unit dropdown; wide enough for `vmin` and its chevron.
const VAL_UNIT_WIDTH: f32 = 70.0;
/// What the same dropdown takes inside a `UiRect` cell, where two cells share
/// a line and the number beside it is already at its floor.
const RECT_UNIT_WIDTH: f32 = 62.0;
/// How far the unit dropdown gives way in a narrow panel: `vmin` plus the
/// chevron still reads at this width, one glyph does not.
const VAL_UNIT_MIN_WIDTH: f32 = 34.0;
/// How much of a narrow row's shortfall the magnitude field absorbs before
/// the dropdown beside it starts paying.
const VAL_NUMBER_SHRINK: f32 = 3.0;
/// How narrow a standalone magnitude field may get: three digits still read
/// here. Set explicitly rather than left to the field's own contents, which
/// would make the dropdown beside it absorb the whole row's shortfall.
const VAL_NUMBER_MIN_WIDTH: f32 = 28.0;
/// Room a length cell asks for: a number and a unit beside each other.
const VAL_CELL_WIDTH: f32 = 96.0;
/// Width of the side letter leading a cell inside a `UiRect` row.
const RECT_SIDE_WIDTH: f32 = 10.0;
/// Room the four sides of a `UiRect` ask for before the row wraps the label
/// away instead. Two cells wide: a narrow panel stacks them two-by-two rather
/// than one per line.
const RECT_CONTROL_WIDTH: f32 = 190.0;

/// The unit half of a [`Val`], split out so the magnitude survives a unit
/// change. Ordered as the dropdown lists them: the two common units first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValUnit {
    Px,
    Percent,
    Auto,
    Vw,
    Vh,
    VMin,
    VMax,
}

const VAL_UNITS: [ValUnit; 7] = [
    ValUnit::Px,
    ValUnit::Percent,
    ValUnit::Auto,
    ValUnit::Vw,
    ValUnit::Vh,
    ValUnit::VMin,
    ValUnit::VMax,
];

impl ValUnit {
    fn label(self) -> &'static str {
        match self {
            ValUnit::Px => "px",
            ValUnit::Percent => "%",
            ValUnit::Auto => "auto",
            ValUnit::Vw => "vw",
            ValUnit::Vh => "vh",
            ValUnit::VMin => "vmin",
            ValUnit::VMax => "vmax",
        }
    }

    fn index(self) -> usize {
        VAL_UNITS.iter().position(|u| *u == self).unwrap_or(0)
    }

    fn of(value: Val) -> Self {
        match value {
            Val::Auto => ValUnit::Auto,
            Val::Px(_) => ValUnit::Px,
            Val::Percent(_) => ValUnit::Percent,
            Val::Vw(_) => ValUnit::Vw,
            Val::Vh(_) => ValUnit::Vh,
            Val::VMin(_) => ValUnit::VMin,
            Val::VMax(_) => ValUnit::VMax,
        }
    }

    fn build(self, magnitude: f32) -> Val {
        match self {
            ValUnit::Auto => Val::Auto,
            ValUnit::Px => Val::Px(magnitude),
            ValUnit::Percent => Val::Percent(magnitude),
            ValUnit::Vw => Val::Vw(magnitude),
            ValUnit::Vh => Val::Vh(magnitude),
            ValUnit::VMin => Val::VMin(magnitude),
            ValUnit::VMax => Val::VMax(magnitude),
        }
    }
}

fn val_magnitude(value: Val) -> f32 {
    match value {
        Val::Auto => 0.0,
        Val::Px(v) | Val::Percent(v) | Val::Vw(v) | Val::Vh(v) | Val::VMin(v) | Val::VMax(v) => v,
    }
}

/// The `Val` one field is editing, held on the field root. The number input
/// does not self-update and the unit lives in a separate widget, so this is
/// the app-owned source of truth both observers read and write.
#[derive(Component)]
struct ValFieldState {
    unit: ValUnit,
    magnitude: f32,
}

/// Where a `Val` field writes its settled value. Invoked with
/// `(world, val, is_final)`: a drag tick passes `false`, a released drag or a
/// unit pick passes `true`.
#[derive(Component)]
struct ValCommit(Box<dyn Fn(&mut World, Val, bool) + Send + Sync>);

/// What a field is bound to. The commit closure captures the same three, but
/// the observers and the refresh pass need them out in the open: to drop edits
/// aimed at a read-only remote proxy, and to read the live `Val` back.
#[derive(Component)]
pub(crate) struct ValFieldBinding {
    source_entity: Entity,
    type_path: String,
    field_path: String,
}

/// Links the magnitude input back to its field root.
#[derive(Component)]
pub(crate) struct ValNumberInput {
    root: Entity,
}

/// Links the unit dropdown back to its field root.
#[derive(Component)]
pub(crate) struct ValUnitSelect {
    root: Entity,
}

/// Spawn a one-row `Val` editor under `parent`, seeded to `value` and bound to
/// `field_path` on `source_entity`'s `type_path` component. Returns the row,
/// which carries the property marker; the edit state and the commit closure
/// live on the cell inside it.
pub fn spawn_val_field(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    value: Val,
    depth: usize,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
) -> Entity {
    let row = spawn_field_row(
        commands,
        parent,
        FieldRowProps::new(name).indented(u8::try_from(depth).unwrap_or(u8::MAX)),
    );
    commands.entity(row.row).insert(InspectorFieldRow {
        source_entity,
        type_path: type_path.to_string(),
        field_path: field_path.clone(),
    });
    spawn_val_cell(
        commands,
        row.control,
        None,
        value,
        field_path,
        source_entity,
        type_path,
    );
    row.row
}

/// Spawn a four-side `UiRect` editor: one labelled row whose control holds a
/// compact cell per side, each bound to `{field_path}.{side}`. The row carries
/// one property marker for the whole rect, the way a `Vec3`'s row does.
pub fn spawn_ui_rect_field(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    value: UiRect,
    depth: usize,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
) -> Entity {
    let row = spawn_field_row(
        commands,
        parent,
        FieldRowProps::new(name).indented(u8::try_from(depth).unwrap_or(u8::MAX)),
    );
    commands.entity(row.row).insert(InspectorFieldRow {
        source_entity,
        type_path: type_path.to_string(),
        field_path: field_path.clone(),
    });
    // The cells wrap: a wide panel shows all four side by side, a narrow one
    // lays them out two by two rather than shrinking a length past reading.
    // The kit's node is extended rather than replaced, keeping the shared
    // control floor; a four-cell floor would overflow a narrow panel instead
    // of wrapping.
    commands
        .entity(row.control)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.flex_wrap = FlexWrap::Wrap;
            node.row_gap = px(tokens::SPACING_XS);
            node.flex_basis = px(RECT_CONTROL_WIDTH);
        });

    for (letter, side, side_value) in [
        ("L", "left", value.left),
        ("T", "top", value.top),
        ("R", "right", value.right),
        ("B", "bottom", value.bottom),
    ] {
        spawn_val_cell(
            commands,
            row.control,
            Some(letter),
            side_value,
            format!("{field_path}.{side}"),
            source_entity,
            type_path,
        );
    }

    row.row
}

/// Spawn the editing half of a length: an optional side letter, the magnitude
/// input and the unit dropdown. The returned cell is what the observers and
/// the refresh pass address: it holds the state, the commit closure and the
/// binding.
fn spawn_val_cell(
    commands: &mut Commands,
    parent: Entity,
    letter: Option<&str>,
    value: Val,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
) -> Entity {
    let tp = type_path.to_string();
    let path = field_path.clone();
    let commit = move |world: &mut World, val: Val, is_final: bool| {
        commit_val_to_field(world, source_entity, &tp, &path, val, is_final);
    };
    let unit = ValUnit::of(value);
    let magnitude = val_magnitude(value);

    let root = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                // A standalone cell keeps a floor under both halves, which a
                // narrow marked panel cannot always seat side by side, so it
                // stacks them rather than reaching over the mark. A rect's
                // cells have no number floor and never need this.
                flex_wrap: if letter.is_some() {
                    FlexWrap::NoWrap
                } else {
                    FlexWrap::Wrap
                },
                row_gap: px(tokens::SPACING_XS),
                // The cell asks for room for a number and a unit side by side,
                // so a rect row seats two per line rather than four squeezed
                // ones. Where even one cell cannot have that, the pair wraps
                // inside the cell instead of shrinking past reading.
                flex_grow: 1.0,
                flex_shrink: 1.0,
                flex_basis: px(VAL_CELL_WIDTH),
                min_width: px(tokens::FIELD_CONTROL_MIN_WIDTH),
                ..Default::default()
            },
            ValFieldState { unit, magnitude },
            ValCommit(Box::new(commit)),
            ValFieldBinding {
                source_entity,
                type_path: type_path.to_string(),
                field_path,
            },
            ChildOf(parent),
        ))
        .id();

    if let Some(letter) = letter {
        commands.spawn((
            Text::new(letter),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_TERTIARY),
            Node {
                width: px(RECT_SIDE_WIDTH),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(root),
        ));
    }

    let number = commands
        .spawn_scene(bsn! { @ScrubNumberInput })
        .insert((
            ScrubNumberInputValue::F64(magnitude as f64),
            NumberInputPrecision(VAL_PRECISION),
            ValNumberInput { root },
            ChildOf(root),
        ))
        .id();
    // The magnitude is the half of the pair that gives way: an explicit floor
    // and a faster shrink rate spend the row's shortfall here first. Left to
    // its contents, the field would hold the width of its digits and the
    // dropdown beside it would absorb the whole difference.
    //
    // A standalone cell keeps a number's floor; one of a rect's four gives it
    // up, since four cells on a marked narrow panel cannot seat both floors.
    let number_floor = if letter.is_some() {
        0.0
    } else {
        VAL_NUMBER_MIN_WIDTH
    };
    commands
        .entity(number)
        .entry::<Node>()
        .and_modify(move |mut node| {
            node.min_width = px(number_floor);
            node.flex_shrink = VAL_NUMBER_SHRINK;
        });
    if unit == ValUnit::Auto {
        commands.entity(number).insert(InteractionDisabled);
    }

    let options: Vec<ComboBoxOptionData> = VAL_UNITS
        .iter()
        .map(|u| ComboBoxOptionData::new(u.label()))
        .collect();
    commands
        .spawn((combobox_with_selected(options, unit.index()), ChildOf(root)))
        .insert((
            ValUnitSelect { root },
            // A fixed, unshrinkable width would push the pair past the row's
            // right edge, over the strip a property mark sits in, as soon as
            // the panel narrows. It gives way down to the width its labels
            // still read at, and the number input gives way first.
            Node {
                width: px(if letter.is_some() {
                    RECT_UNIT_WIDTH
                } else {
                    VAL_UNIT_WIDTH
                }),
                min_width: px(VAL_UNIT_MIN_WIDTH),
                flex_shrink: 1.0,
                ..Default::default()
            },
        ));

    root
}

/// The reflect-field commit: a drag tick previews on live ECS, a settled value
/// mints one undo-backed document write. PIE Live mode routes to the running
/// game instead, like every other inspector write path.
fn commit_val_to_field(
    world: &mut World,
    source_entity: Entity,
    type_path: &str,
    field_path: &str,
    value: Val,
    is_final: bool,
) {
    let json = val_to_json(value);
    if crate::inspector::reflect_fields::try_route_pie_live_field_edit(
        world,
        source_entity,
        type_path,
        field_path,
        json.clone(),
    ) {
        return;
    }
    if is_final {
        crate::commands::field_edit_commit(
            world,
            type_path,
            field_path,
            &json,
            "Set length on multiple entities",
        );
    } else {
        crate::commands::field_edit_preview(world, type_path, field_path, &json);
    }
}

/// A `Val` in the reflect JSON the field deserializer expects: a unit variant
/// is its bare name, a magnitude variant a single-key object.
fn val_to_json(value: Val) -> serde_json::Value {
    match value {
        Val::Auto => serde_json::json!("Auto"),
        Val::Px(v) => serde_json::json!({ "Px": v }),
        Val::Percent(v) => serde_json::json!({ "Percent": v }),
        Val::Vw(v) => serde_json::json!({ "Vw": v }),
        Val::Vh(v) => serde_json::json!({ "Vh": v }),
        Val::VMin(v) => serde_json::json!({ "VMin": v }),
        Val::VMax(v) => serde_json::json!({ "VMax": v }),
    }
}

/// True when a field targets a read-only remote proxy, so the observers drop
/// the edit before the state or the widgets move.
fn val_edit_skipped(
    root: Entity,
    bindings: &Query<&ValFieldBinding>,
    remote_proxies: &Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) -> bool {
    bindings
        .get(root)
        .is_ok_and(|binding| remote_proxies.contains(binding.source_entity))
}

/// Push the field's magnitude, enabled state and unit onto its widgets, so
/// they follow a change they did not originate.
fn sync_val_widgets(world: &mut World, root: Entity) {
    let Some(state) = world.get::<ValFieldState>(root) else {
        return;
    };
    let magnitude = state.magnitude as f64;
    let is_auto = state.unit == ValUnit::Auto;
    let unit_index = state.unit.index();

    let inputs: Vec<Entity> = world
        .query::<(Entity, &ValNumberInput)>()
        .iter(world)
        .filter(|(_, input)| input.root == root)
        .map(|(entity, _)| entity)
        .collect();
    for input in inputs {
        let Ok(mut entity) = world.get_entity_mut(input) else {
            continue;
        };
        entity.insert(ScrubNumberInputValue::F64(magnitude));
        if is_auto {
            entity.insert(InteractionDisabled);
        } else {
            entity.remove::<InteractionDisabled>();
        }
    }

    let selects: Vec<Entity> = world
        .query::<(Entity, &ValUnitSelect)>()
        .iter(world)
        .filter(|(_, select)| select.root == root)
        .map(|(entity, _)| entity)
        .collect();
    for select in selects {
        if let Ok(mut entity) = world.get_entity_mut(select) {
            entity.insert(ComboBoxSelectedIndex(unit_index));
        }
    }
}

/// Build the current `Val` from the field's state and hand it to the field's
/// commit closure. The closure is taken out for the call and put back after,
/// so a commit that rebuilds the inspector only loses the closure's home.
fn commit_val_field(world: &mut World, root: Entity, is_final: bool) {
    let Some(state) = world.get::<ValFieldState>(root) else {
        return;
    };
    let value = state.unit.build(state.magnitude);
    let Some(commit) = world
        .get_entity_mut(root)
        .ok()
        .and_then(|mut entity| entity.take::<ValCommit>())
    else {
        return;
    };
    commit.0(world, value, is_final);
    if let Ok(mut entity) = world.get_entity_mut(root) {
        entity.insert(commit);
    }
}

/// Write path for a `Val` field's magnitude. The number input emits every drag
/// tick with `is_final == false` and once on release, but never self-updates,
/// so this re-inserts its value and previews; only the final tick commits.
pub(crate) fn on_val_number_change(
    event: On<ValueChange<f64>>,
    inputs: Query<&ValNumberInput>,
    bindings: Query<&ValFieldBinding>,
    remote_proxies: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
    mut commands: Commands,
) {
    let source = event.source;
    let Ok(input) = inputs.get(source) else {
        return;
    };
    let root = input.root;
    if val_edit_skipped(root, &bindings, &remote_proxies) {
        return;
    }
    let value = event.value;
    let is_final = event.is_final;

    commands
        .entity(source)
        .insert(ScrubNumberInputValue::F64(value));
    commands.queue(move |world: &mut World| {
        let Some(mut state) = world.get_mut::<ValFieldState>(root) else {
            return;
        };
        if state.unit == ValUnit::Auto {
            return;
        }
        state.magnitude = value as f32;
        commit_val_field(world, root, is_final);
    });
}

/// Write path for a `Val` field's unit. The magnitude carries across units so
/// `10px` becomes `10%`; `Auto` has none, so entering it zeroes the magnitude
/// and leaving it starts from zero.
pub(crate) fn on_val_unit_change(
    event: On<ComboBoxChangeEvent>,
    selects: Query<&ValUnitSelect>,
    bindings: Query<&ValFieldBinding>,
    remote_proxies: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
    mut commands: Commands,
) {
    let Ok(select) = selects.get(event.entity) else {
        return;
    };
    let root = select.root;
    if val_edit_skipped(root, &bindings, &remote_proxies) {
        return;
    }
    let Some(unit) = VAL_UNITS.get(event.selected).copied() else {
        return;
    };

    commands.queue(move |world: &mut World| {
        let Some(mut state) = world.get_mut::<ValFieldState>(root) else {
            return;
        };
        if state.unit == unit {
            return;
        }
        if unit == ValUnit::Auto {
            state.magnitude = 0.0;
        }
        state.unit = unit;
        sync_val_widgets(world, root);
        commit_val_field(world, root, true);
    });
}

/// Pull every `Val` row on the primary selection back in line with the live
/// component.
///
/// The row's `ValFieldState` is what the next edit builds on, so it must not
/// drift from the component: undo, a gizmo, or another editor all move the
/// component behind the row's back. Same change-detection idiom as
/// `crate::inspector::reflect_fields::refresh_inspector_fields`, but these
/// rows carry no `FieldBinding` (a bare number at a `Val`'s path would be the
/// wrong write), so they are found by their own `ValFieldBinding`.
pub fn refresh_val_fields(
    world: &mut World,
    mut last_run: Local<Option<bevy::ecs::change_detection::Tick>>,
    mut unreadable: Local<bevy::platform::collections::HashSet<Entity>>,
) {
    use bevy::ecs::reflect::ReflectComponent;

    let this_run = world.read_change_tick();
    let prev_run = last_run.replace(this_run);

    let Some(primary) = world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary)
    else {
        return;
    };
    if let Some(prev_run) = prev_run {
        let Ok(entity_ref) = world.get_entity(primary) else {
            return;
        };
        if !crate::inspector::reflect_fields::entity_components_changed(
            entity_ref, prev_run, this_run,
        ) {
            return;
        }
    }

    let fields: Vec<(Entity, String, String)> = world
        .query::<(Entity, &ValFieldBinding)>()
        .iter(world)
        .filter(|(_, binding)| binding.source_entity == primary)
        .map(|(root, binding)| (root, binding.type_path.clone(), binding.field_path.clone()))
        .collect();
    if fields.is_empty() {
        return;
    }

    let Some(type_registry) = world.get_resource::<AppTypeRegistry>().cloned() else {
        return;
    };
    let registry = type_registry.read();
    let Ok(entity_ref) = world.get_entity(primary) else {
        return;
    };

    let mut stale: Vec<(Entity, Val)> = Vec::new();
    for (root, type_path, field_path) in &fields {
        let Some(live) = registry
            .get_with_type_path(type_path)
            .and_then(|registration| registration.data::<ReflectComponent>())
            .and_then(|reflect_component| reflect_component.reflect(entity_ref))
            .and_then(|reflected| reflected.reflect_path(field_path.as_str()).ok())
            .and_then(|field| field.try_downcast_ref::<Val>().copied())
        else {
            // The row is editing a value it cannot read back, so its state may
            // author a stale magnitude over a live one. Warned once per row:
            // this pass runs on every component change, and a drag would
            // otherwise repeat the same line every frame.
            if unreadable.insert(*root) {
                warn!(
                    "a length row cannot read '{type_path}.{field_path}' back, so it may be showing a stale value"
                );
            }
            continue;
        };
        unreadable.remove(root);
        let Some(state) = world.get::<ValFieldState>(*root) else {
            continue;
        };
        if state.unit != ValUnit::of(live) || state.magnitude != val_magnitude(live) {
            stale.push((*root, live));
        }
    }
    drop(registry);

    for (root, live) in stale {
        if let Some(mut state) = world.get_mut::<ValFieldState>(root) {
            state.unit = ValUnit::of(live);
            state.magnitude = val_magnitude(live);
        }
        sync_val_widgets(world, root);
    }
}
