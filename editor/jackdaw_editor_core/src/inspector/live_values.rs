//! Live-value chips: on the selected entity's field rows, the running game's
//! value of the field, where it differs from the document's.
//!
//! Both sides are compared as reflection serializes them. A row gets a chip
//! when its field reads as a number, a bool, a string or a list of numbers
//! (vectors and quaternions) on both sides; enum, map and nested struct fields
//! get none.

use bevy::prelude::*;
use jackdaw_feathers::tokens;
use serde_json::Value;

use jackdaw_pie_protocol::HighlightPlace;
use jackdaw_widgets::collapsible::{CollapsibleHeader, CollapsibleSection};

use super::InspectorFieldRow;
use crate::game_query::{GameEntity, GameEntityRef, GameQueries};
use crate::selection::Selection;

/// A chip under a field row showing the game's value of the field.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct LiveValueChip {
    pub type_path: String,
    pub field_path: String,
    pub text: String,
}

/// A mark on a component card's header saying a field inside it is drifted,
/// shown while the card is collapsed.
#[derive(Component)]
pub struct LiveCardMarker;

/// The game entity watched for the primary selection, while a game is live:
/// only when the watch names the selection's scene node, and the game sent one
/// entity spawned from that node.
fn live_for_selection(world: &World, primary: Entity) -> Option<&GameEntity> {
    crate::pie::focused_live_instance(world)?;
    let queries = world.get_resource::<GameQueries>()?;
    if queries.runtime_pick().is_some() {
        return None;
    }
    let Some(GameEntityRef::Places(places)) = queries.watch_target() else {
        return None;
    };
    let ast = world.get_resource::<jackdaw_bsn::SceneBsnAst>()?;
    let node = ast.stable_id_of(ast.ast_for(primary)?)?;
    let names_node = |place: &HighlightPlace| matches!(place, HighlightPlace::Node { node: at, inner, .. } if *at == node && inner.is_empty());
    if places.is_empty() || !places.iter().all(names_node) {
        return None;
    }
    match queries.watched() {
        [only] if only.node == Some(node) => Some(only),
        _ => None,
    }
}

/// The document's value of the component `type_path` on the scene node `node`:
/// the type's default with what the document writes applied over it. `None`
/// when the document does not hold the component, so a component the editor
/// adds on its own is never compared.
fn document_component(
    ast: &jackdaw_bsn::SceneBsnAst,
    node: Entity,
    type_path: &str,
    registry: &bevy::reflect::TypeRegistry,
) -> Option<Value> {
    let written = jackdaw_bsn::apply::get_bsn_field(ast, node, type_path, "")?;
    let registration = registry.get_with_type_path(type_path)?;
    let mut value = registration
        .data::<bevy::prelude::ReflectDefault>()?
        .default();
    let patch =
        jackdaw_bsn::apply::bsn_value_to_reflect(&written, registration.type_id(), registry, None)?;
    value.try_apply(patch.as_ref()).ok()?;
    jackdaw_pie_protocol::snapshot::reflect_value_json(value.as_partial_reflect(), registry)
}

/// The type path of the field at `field_path` inside the type `type_path`.
fn field_type_path<'a>(
    registry: &'a bevy::reflect::TypeRegistry,
    type_path: &str,
    field_path: &str,
) -> Option<&'a str> {
    let mut at = registry.get_with_type_path(type_path)?.type_info();
    for segment in field_path.split('.') {
        let name = segment.split_once('[').map_or(segment, |(name, _)| name);
        let bevy::reflect::TypeInfo::Struct(info) = at else {
            return None;
        };
        let field = info.field(name)?;
        at = registry.get(field.type_id())?.type_info();
        if segment.contains('[') {
            return None;
        }
    }
    Some(at.type_path())
}

/// Put a chip on each of the primary selection's field rows whose field the game
/// holds at another value than the document, and take it off once they agree or
/// the game stops. Works only when the watch, the selection, the document or
/// the rows changed.
pub(crate) fn refresh_live_value_chips(world: &mut World, mut last: Local<Option<u64>>) {
    let generation = world
        .get_resource::<GameQueries>()
        .map_or(0, GameQueries::generation);
    let mut new_rows = world.query_filtered::<(), Added<InspectorFieldRow>>();
    let rows_added = new_rows.iter(world).next().is_some();
    let selection_moved = world
        .get_resource_ref::<Selection>()
        .is_some_and(|selection| selection.is_changed());
    let document_moved = world
        .get_resource_ref::<jackdaw_bsn::SceneBsnAst>()
        .is_some_and(|document| document.is_changed());
    let mut toggled =
        world.query_filtered::<(), (With<super::ComponentDisplay>, Changed<CollapsibleSection>)>();
    let toggled = toggled.iter(world).next().is_some();
    let changed = *last != Some(generation) || rows_added || selection_moved || document_moved;
    if !changed {
        if toggled {
            refresh_card_markers(world);
        }
        return;
    }
    *last = Some(generation);

    let wanted = wanted_chips(world);
    let mut standing = world.query::<(Entity, &LiveValueChip, &ChildOf)>();
    let standing: Vec<(Entity, LiveValueChip, Entity)> = standing
        .iter(world)
        .map(|(entity, chip, child_of)| (entity, chip.clone(), child_of.parent()))
        .collect();
    for (chip, held, row) in &standing {
        if !wanted.iter().any(|(at, want)| at == row && want == held) {
            world.entity_mut(*chip).despawn();
        }
    }
    for (row, chip) in wanted {
        if standing
            .iter()
            .any(|(_, held, at)| *at == row && *held == chip)
        {
            continue;
        }
        spawn_chip(world, row, chip);
    }
    refresh_card_markers(world);
}

fn wanted_chips(world: &mut World) -> Vec<(Entity, LiveValueChip)> {
    let Some(primary) = world
        .get_resource::<Selection>()
        .and_then(Selection::primary)
    else {
        return Vec::new();
    };
    let mut rows = world.query::<(Entity, &InspectorFieldRow)>();
    let rows: Vec<(Entity, &InspectorFieldRow)> = rows
        .iter(world)
        .filter(|(_, row)| row.source_entity == primary)
        .collect();
    let Some(live) = live_for_selection(world, primary) else {
        return Vec::new();
    };
    let Some(ast) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return Vec::new();
    };
    let Some(node) = ast.ast_for(primary) else {
        return Vec::new();
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    let mut documented: std::collections::HashMap<&str, Option<Value>> = default();
    let mut wanted = Vec::new();
    for (row, field) in rows {
        let type_path = field.type_path.as_str();
        let field_path = field.field_path.as_str();
        let Some((_, live_value)) = live.components.iter().find(|(path, _)| path == type_path)
        else {
            continue;
        };
        let document = documented
            .entry(type_path)
            .or_insert_with(|| document_component(ast, node, type_path, &registry));
        let (Some(live_field), Some(document_field)) = (
            field_at(live_value, field_path),
            document
                .as_ref()
                .and_then(|value| field_at(value, field_path)),
        ) else {
            continue;
        };
        if !is_leaf(live_field) || !is_leaf(document_field) {
            continue;
        }
        let rotation = field_type_path(&registry, type_path, field_path) == Some("glam::Quat");
        let agree = if rotation {
            rotations_agree(live_field, document_field)
        } else {
            values_agree(live_field, document_field)
        };
        if !agree {
            wanted.push((
                row,
                LiveValueChip {
                    type_path: type_path.to_string(),
                    field_path: field_path.to_string(),
                    text: format_value(live_field),
                },
            ));
        }
    }
    wanted
}

/// Mark each collapsed component card that holds a chip, and unmark the rest.
fn refresh_card_markers(world: &mut World) {
    let mut chips = world.query_filtered::<Entity, With<LiveValueChip>>();
    let chips: Vec<Entity> = chips.iter(world).collect();
    let mut drifted = std::collections::HashSet::new();
    for chip in chips {
        let mut at = chip;
        while let Some(child_of) = world.get::<ChildOf>(at) {
            at = child_of.parent();
            if world.get::<super::ComponentDisplay>(at).is_some() {
                drifted.insert(at);
                break;
            }
        }
    }
    let mut cards = world
        .query_filtered::<(Entity, &CollapsibleSection, &Children), With<super::ComponentDisplay>>(
        );
    let cards: Vec<(Entity, bool, Option<Entity>)> = cards
        .iter(world)
        .map(|(card, section, children)| {
            let header = children
                .iter()
                .find(|child| world.get::<CollapsibleHeader>(*child).is_some());
            (card, section.collapsed, header)
        })
        .collect();
    for (card, collapsed, header) in cards {
        let Some(header) = header else {
            continue;
        };
        let marker = world.get::<Children>(header).and_then(|children| {
            children
                .iter()
                .find(|child| world.get::<LiveCardMarker>(*child).is_some())
        });
        let wanted = collapsed && drifted.contains(&card);
        match (marker, wanted) {
            (Some(marker), false) => world.entity_mut(marker).despawn(),
            (None, true) => {
                world.spawn((
                    LiveCardMarker,
                    live_badge("Live"),
                    jackdaw_feathers::tooltip::Tooltip::title("Live value")
                        .with_description("A field in this component differs in the running game."),
                    ChildOf(header),
                ));
            }
            _ => {}
        }
    }
}

/// Whether the chips have anything to do: the game watches something or a chip
/// is up.
pub(crate) fn chips_may_change(
    queries: Option<Res<GameQueries>>,
    chips: Query<(), Or<(With<LiveValueChip>, With<LiveCardMarker>)>>,
) -> bool {
    !chips.is_empty() || queries.is_some_and(|queries| queries.watch_target().is_some())
}

/// The accent badge live values are drawn with.
fn live_badge(text: &str) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..default()
        },
        BackgroundColor(tokens::CHIP_BG),
        children![(
            Text::new(text.to_string()),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(crate::default_style::PLAYING_ACCENT),
        )],
    )
}

fn spawn_chip(world: &mut World, row: Entity, chip: LiveValueChip) {
    let text = format!("Live: {}", chip.text);
    world.spawn((
        chip,
        Node {
            width: Val::Percent(100.0),
            ..default()
        },
        ChildOf(row),
        children![(
            live_badge(&text),
            jackdaw_feathers::tooltip::Tooltip::title("Live value")
                .with_description("The running game holds this value. It is not saved."),
        )],
    ));
}

/// The value at a dotted field path inside a reflect-serialized component.
/// Vector axes `x`, `y`, `z` and `w` read list items.
pub fn field_at<'a>(value: &'a Value, field_path: &str) -> Option<&'a Value> {
    if field_path.is_empty() {
        return Some(value);
    }
    field_path.split('.').try_fold(value, |at, segment| {
        let (name, index) = match segment.split_once('[') {
            Some((name, rest)) => (name, rest.strip_suffix(']')?.parse::<usize>().ok()),
            None => (segment, None),
        };
        let at = match at {
            Value::Object(fields) => fields.get(name)?,
            Value::Array(items) => {
                let item = match name {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    "w" => 3,
                    other => other.parse().ok()?,
                };
                items.get(item)?
            }
            _ => return None,
        };
        match index {
            Some(index) => at.as_array()?.get(index),
            None => Some(at),
        }
    })
}

fn is_leaf(value: &Value) -> bool {
    match value {
        Value::Number(_) | Value::Bool(_) | Value::String(_) => true,
        Value::Array(items) => !items.is_empty() && items.iter().all(Value::is_number),
        _ => false,
    }
}

/// Whether two serialized values agree, numbers to a small tolerance.
pub fn values_agree(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-4 * x.abs().max(y.abs()).max(1.0),
            _ => x == y,
        },
        (Value::Array(xs), Value::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| values_agree(x, y))
        }
        _ => a == b,
    }
}

/// Whether two serialized quaternions name the same rotation: `q` and `-q` do.
pub fn rotations_agree(a: &Value, b: &Value) -> bool {
    let as_quat = |value: &Value| -> Option<[f64; 4]> {
        let items = value.as_array()?;
        let mut quat = [0.0; 4];
        if items.len() != 4 {
            return None;
        }
        for (slot, item) in quat.iter_mut().zip(items) {
            *slot = item.as_f64()?;
        }
        Some(quat)
    };
    match (as_quat(a), as_quat(b)) {
        (Some(a), Some(b)) => {
            let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            dot.abs() >= 1.0 - 1e-4
        }
        _ => values_agree(a, b),
    }
}

/// A serialized value as a short line of text.
pub fn format_value(value: &Value) -> String {
    const LONGEST: usize = 120;
    let text = match value {
        Value::Null => "none".to_string(),
        Value::Bool(on) => on.to_string(),
        Value::Number(number) => format_number(number),
        Value::String(text) => text.clone(),
        Value::Array(items) if items.iter().all(Value::is_number) => items
            .iter()
            .filter_map(Value::as_number)
            .map(format_number)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    if text.chars().count() > LONGEST {
        let cut: String = text.chars().take(LONGEST).collect();
        format!("{cut}...")
    } else {
        text
    }
}

fn format_number(number: &serde_json::Number) -> String {
    if number.is_i64() || number.is_u64() {
        return number.to_string();
    }
    let Some(float) = number.as_f64() else {
        return number.to_string();
    };
    let text = format!("{float:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// The chips the inspector shows, as (type path, field path, value).
pub fn live_value_chips(world: &mut World) -> Vec<(String, String, String)> {
    let mut chips = world.query::<&LiveValueChip>();
    chips
        .iter(world)
        .map(|chip| {
            (
                chip.type_path.clone(),
                chip.field_path.clone(),
                chip.text.clone(),
            )
        })
        .collect()
}

/// How many component cards carry the live marker on their header.
pub fn live_card_markers(world: &mut World) -> usize {
    let mut markers = world.query_filtered::<(), With<LiveCardMarker>>();
    markers.iter(world).count()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_field_path_reads_struct_fields_and_vector_axes() {
        let transform = json!({
            "translation": [1.0, 2.0, 3.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        });
        assert_eq!(
            field_at(&transform, "translation"),
            Some(&json!([1.0, 2.0, 3.0]))
        );
        assert_eq!(field_at(&transform, "translation.y"), Some(&json!(2.0)));
        assert_eq!(field_at(&transform, "rotation.w"), Some(&json!(1.0)));
        assert_eq!(field_at(&transform, "missing"), None);
        let nested = json!({ "faces": [{ "uv": [0.5, 0.25] }] });
        assert_eq!(field_at(&nested, "faces[0].uv.x"), Some(&json!(0.5)));
    }

    #[test]
    fn numbers_agree_to_a_small_tolerance() {
        assert!(values_agree(&json!([1.0, 2.0]), &json!([1.000_000_1, 2.0])));
        assert!(!values_agree(&json!([1.0, 2.0]), &json!([1.5, 2.0])));
        assert!(!values_agree(&json!(true), &json!(false)));
        assert!(values_agree(&json!("a"), &json!("a")));
    }

    #[test]
    fn a_rotation_and_its_negation_agree() {
        let q = json!([0.0, -0.983, 0.0, -0.1837]);
        let minus = json!([0.0, 0.983, 0.0, 0.1837]);
        assert!(rotations_agree(&q, &minus));
        assert!(!rotations_agree(&q, &json!([0.0, 0.0, 0.0, 1.0])));
        assert!(!values_agree(&q, &minus), "plain vectors keep their sign");
    }

    #[test]
    fn values_read_as_short_text() {
        assert_eq!(format_value(&json!([5.0, 0.25, -0.0])), "5, 0.25, 0");
        assert_eq!(format_value(&json!(3)), "3");
        assert_eq!(format_value(&json!(null)), "none");
        assert_eq!(format_value(&json!({ "a": 1 })), "{\"a\":1}");
    }
}
