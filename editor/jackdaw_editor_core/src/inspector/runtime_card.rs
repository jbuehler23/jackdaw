//! The Inspector's read-only view of a runtime entity picked in the Outliner:
//! its components as the game last sent them, with nothing to edit, add or
//! remove.

use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use jackdaw_feathers::field_row::{FieldRowProps, spawn_field_row};
use jackdaw_feathers::tokens;
use serde_json::Value;

use super::add_header::InspectorAddHeaderMount;
use super::live_values::format_value;
use super::{Inspector, InspectorTarget};
use crate::game_query::{GameEntity, GameQueries};

/// The read-only card of a runtime entity, under the Inspector, and the lines it
/// shows.
#[derive(Component)]
pub struct RuntimeEntityCard {
    lines: Vec<CardLine>,
}

/// The text of a value line in a [`RuntimeEntityCard`], by its place among them.
#[derive(Component)]
struct RuntimeCardValue(usize);

/// An add header hidden while a runtime entity is shown.
#[derive(Component)]
struct HiddenForRuntimeCard;

/// One line of a runtime entity's card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardLine {
    Title(String),
    Note(String),
    Component(String),
    Field { label: String, value: String },
}

impl CardLine {
    fn shape(&self) -> CardLine {
        match self {
            CardLine::Field { label, .. } => CardLine::Field {
                label: label.clone(),
                value: String::new(),
            },
            other => other.clone(),
        }
    }
}

/// What reflection calls a type whose name is compiled out.
const HIDDEN_NAME: &str = "<Enable the debug feature";

/// The lines a runtime entity's card shows: its name, then each component and
/// its fields. `answered` says the game has answered for the entity, so one it
/// left out is gone.
pub fn card_lines(bits: u64, entity: Option<&GameEntity>, answered: bool) -> Vec<CardLine> {
    let mut lines = vec![CardLine::Title(crate::runtime_outliner::entity_label(
        bits,
        entity.and_then(|entity| entity.name.as_deref()),
    ))];
    lines.push(CardLine::Note(
        "Spawned by the running game. Read-only; nothing here is saved.".to_string(),
    ));
    let Some(entity) = entity else {
        let note = if answered {
            "No longer in the game"
        } else {
            "Waiting for the game"
        };
        lines.push(CardLine::Note(note.to_string()));
        return lines;
    };
    for (type_path, value) in &entity.components {
        if type_path == CHILD_OF {
            continue;
        }
        lines.push(CardLine::Component(short_type_name(type_path)));
        match value {
            Value::Object(fields) => {
                for (name, field) in fields {
                    lines.push(CardLine::Field {
                        label: super::sentence(name),
                        value: card_value(field),
                    });
                }
            }
            other => lines.push(CardLine::Field {
                label: "Value".to_string(),
                value: card_value(other),
            }),
        }
    }
    if let Some(note) = unreadable_note(&entity.opaque) {
        lines.push(CardLine::Note(note));
    }
    lines
}

/// The parent relation, which the Outliner already shows as nesting.
const CHILD_OF: &str = "bevy_ecs::hierarchy::ChildOf";

/// A field's value on the card. Asset handles come through as null.
fn card_value(value: &Value) -> String {
    if value.is_null() {
        "asset".to_string()
    } else {
        format_value(value)
    }
}

/// What the card says of the components the game could not serialize.
fn unreadable_note(opaque: &[String]) -> Option<String> {
    let names: Vec<String> = opaque
        .iter()
        .filter(|name| !name.starts_with(HIDDEN_NAME))
        .map(|name| short_type_name(name))
        .collect();
    let hidden = opaque.len() - names.len();
    let count = |n: usize| {
        if n == 1 {
            "1 component".to_string()
        } else {
            format!("{n} components")
        }
    };
    match (names.is_empty(), hidden) {
        (true, 0) => None,
        (true, hidden) => Some(format!("{} not readable", count(hidden))),
        (false, 0) => Some(format!("Not readable: {}", names.join(", "))),
        (false, hidden) => Some(format!(
            "Not readable: {}, and {hidden} more",
            names.join(", ")
        )),
    }
}

/// A type path with every path in it cut to its last segment:
/// `bevy_pbr::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>` reads
/// `MeshMaterial3d<StandardMaterial>`.
pub fn short_type_name(type_path: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for c in type_path.chars() {
        if matches!(c, '<' | '>' | ',' | ' ' | '(' | ')' | '[' | ']' | ';' | '&') {
            out.push_str(word.rsplit("::").next().unwrap_or(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(word.rsplit("::").next().unwrap_or(&word));
    out
}

/// Show the picked runtime entity in each Inspector holding no selection, and
/// take the card away once the pick ends or something is selected. Works only
/// when the watch or the Inspectors' targets moved.
pub(crate) fn sync_runtime_card(
    world: &mut World,
    mut last: Local<Option<(u64, Vec<(Entity, bool)>)>>,
) {
    let generation = world
        .get_resource::<GameQueries>()
        .map_or(0, GameQueries::generation);
    let mut inspectors = world.query_filtered::<(Entity, Has<InspectorTarget>), With<Inspector>>();
    let inspectors: Vec<(Entity, bool)> = inspectors.iter(world).collect();
    let inputs = Some((generation, inspectors));
    if *last == inputs {
        return;
    }
    let inspectors = inputs
        .as_ref()
        .map(|(_, held)| held.clone())
        .unwrap_or_default();
    *last = inputs;

    let lines = world.get_resource::<GameQueries>().and_then(|queries| {
        let bits = queries.runtime_pick()?;
        let watched = queries
            .watched()
            .iter()
            .find(|entity| entity.entity == bits);
        let answered = queries.watch_answered()
            && queries.watch_target() == Some(&crate::game_query::GameEntityRef::Entity(bits));
        Some(card_lines(bits, watched, answered))
    });
    let mut cards = world.query::<(Entity, &RuntimeEntityCard, &ChildOf)>();
    let cards: Vec<(Entity, Vec<CardLine>, Entity)> = cards
        .iter(world)
        .map(|(card, shown, child_of)| (card, shown.lines.clone(), child_of.parent()))
        .collect();

    let mut showing = false;
    for (inspector, targeted) in inspectors {
        let held = cards.iter().find(|(_, _, at)| *at == inspector);
        let wanted = lines.as_ref().filter(|_| !targeted);
        match (held, wanted) {
            (Some((card, shown, _)), Some(lines)) => {
                showing = true;
                if shown == lines {
                    continue;
                }
                let same_shape = shown.len() == lines.len()
                    && shown.iter().zip(lines).all(|(a, b)| a.shape() == b.shape());
                if same_shape {
                    update_values(world, *card, lines);
                } else {
                    world.entity_mut(*card).despawn();
                    spawn_card(world, inspector, lines);
                }
            }
            (Some((card, _, _)), None) => {
                world.entity_mut(*card).despawn();
            }
            (None, Some(lines)) => {
                showing = true;
                spawn_card(world, inspector, lines);
            }
            (None, None) => {}
        }
    }
    hide_add_headers(world, showing);
}

/// Whether the runtime card has anything to do: a runtime entity is picked or
/// a card is up.
pub(crate) fn runtime_card_may_change(
    queries: Option<Res<GameQueries>>,
    cards: Query<(), With<RuntimeEntityCard>>,
) -> bool {
    !cards.is_empty() || queries.is_some_and(|queries| queries.runtime_pick().is_some())
}

fn update_values(world: &mut World, card: Entity, lines: &[CardLine]) {
    let values: Vec<&String> = lines
        .iter()
        .filter_map(|line| match line {
            CardLine::Field { value, .. } => Some(value),
            _ => None,
        })
        .collect();
    let mut texts = world.query::<(&RuntimeCardValue, &mut Text)>();
    for (place, mut text) in texts.iter_mut(world) {
        if let Some(value) = values.get(place.0)
            && text.0 != **value
        {
            text.0 = (*value).clone();
        }
    }
    if let Some(mut shown) = world.get_mut::<RuntimeEntityCard>(card) {
        shown.lines = lines.to_vec();
    }
}

fn spawn_card(world: &mut World, inspector: Entity, lines: &[CardLine]) {
    let mut commands = world.commands();
    let card = commands
        .spawn((
            RuntimeEntityCard {
                lines: lines.to_vec(),
            },
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(inspector),
        ))
        .id();
    let mut values = 0;
    for line in lines {
        match line {
            CardLine::Title(title) => {
                commands.spawn((
                    Text::new(title.clone()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_LG,
                        ..default()
                    },
                    ThemedText,
                    ChildOf(card),
                ));
            }
            CardLine::Note(note) => {
                commands.spawn((
                    Text::new(note.clone()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    ChildOf(card),
                ));
            }
            CardLine::Component(name) => {
                commands.spawn((
                    Text::new(name.clone()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE,
                        ..default()
                    },
                    ThemedText,
                    Node {
                        margin: UiRect::top(Val::Px(tokens::SPACING_SM)),
                        ..default()
                    },
                    ChildOf(card),
                ));
            }
            CardLine::Field { label, value } => {
                let row = spawn_field_row(&mut commands, card, FieldRowProps::new(label.clone()));
                commands.spawn((
                    RuntimeCardValue(values),
                    Text::new(value.clone()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    ThemedText,
                    ChildOf(row.control),
                ));
                values += 1;
            }
        }
    }
    world.flush();
}

fn hide_add_headers(world: &mut World, hide: bool) {
    let mut mounts = world
        .query_filtered::<(Entity, Has<HiddenForRuntimeCard>), With<InspectorAddHeaderMount>>();
    let mounts: Vec<(Entity, bool)> = mounts.iter(world).collect();
    for (mount, hidden) in mounts {
        if hide == hidden {
            continue;
        }
        let mut entity = world.entity_mut(mount);
        if hide {
            entity.insert(HiddenForRuntimeCard);
        } else {
            entity.remove::<HiddenForRuntimeCard>();
        }
        if let Some(mut node) = entity.get_mut::<Node>() {
            node.display = if hide { Display::None } else { Display::Flex };
        }
    }
}

/// Every line of text the runtime entity's card shows, in order.
pub fn runtime_card_text(world: &mut World) -> Option<Vec<String>> {
    let mut cards = world.query::<&RuntimeEntityCard>();
    let card = cards.iter(world).next()?;
    Some(
        card.lines
            .iter()
            .map(|line| match line {
                CardLine::Title(text) | CardLine::Note(text) | CardLine::Component(text) => {
                    text.clone()
                }
                CardLine::Field { label, value } => format!("{label}: {value}"),
            })
            .collect(),
    )
}

/// Whether any Inspector shows its add header, which offers to add components.
pub fn add_component_offered(world: &mut World) -> bool {
    let mut mounts = world.query_filtered::<&Node, With<InspectorAddHeaderMount>>();
    mounts.iter(world).any(|node| node.display != Display::None)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn drone() -> GameEntity {
        GameEntity {
            entity: 7,
            name: Some("Drone 0".to_string()),
            parent: Some(533),
            node: None,
            components: vec![
                ("bevy_ecs::hierarchy::ChildOf".to_string(), json!(533)),
                ("bevy_mesh::components::Mesh3d".to_string(), json!(null)),
                (
                    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>"
                        .to_string(),
                    json!(null),
                ),
            ],
            opaque: vec![
                "<Enable the debug feature to see the name>".to_string(),
                "<Enable the debug feature to see the name>".to_string(),
            ],
        }
    }

    fn texts(lines: &[CardLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match line {
                CardLine::Title(text) | CardLine::Note(text) | CardLine::Component(text) => {
                    text.clone()
                }
                CardLine::Field { label, value } => format!("{label}: {value}"),
            })
            .collect()
    }

    #[test]
    fn components_without_names_are_counted() {
        let lines = texts(&card_lines(7, Some(&drone()), true));
        assert!(
            lines.contains(&"2 components not readable".to_string()),
            "{lines:?}"
        );
        assert_eq!(
            unreadable_note(&["game::Brain".to_string(), HIDDEN_NAME.to_string()]),
            Some("Not readable: Brain, and 1 more".to_string())
        );
    }

    #[test]
    fn asset_handles_read_as_assets() {
        let lines = texts(&card_lines(7, Some(&drone()), true));
        assert!(lines.contains(&"Value: asset".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains("none")), "{lines:?}");
    }

    #[test]
    fn generic_arguments_are_shortened() {
        let lines = texts(&card_lines(7, Some(&drone()), true));
        assert!(
            lines.contains(&"MeshMaterial3d<StandardMaterial>".to_string()),
            "{lines:?}"
        );
        assert_eq!(
            short_type_name("core::option::Option<alloc::vec::Vec<glam::Vec3>>"),
            "Option<Vec<Vec3>>"
        );
    }

    #[test]
    fn the_parent_relation_is_left_to_the_outliner() {
        let lines = texts(&card_lines(7, Some(&drone()), true));
        assert!(
            !lines.iter().any(|line| line.contains("ChildOf")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|line| line.contains("533")), "{lines:?}");
    }

    #[test]
    fn a_picked_entity_the_game_no_longer_has_says_so() {
        assert!(texts(&card_lines(7, None, false)).contains(&"Waiting for the game".to_string()));
        assert!(texts(&card_lines(7, None, true)).contains(&"No longer in the game".to_string()));
    }
}
