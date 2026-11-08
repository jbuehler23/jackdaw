//! The Outliner's Runtime group: the entities the running game spawned that no
//! scene node did, read-only. While the group is open and a game is live the
//! game is asked for them about once a second; closed or stopped, nothing is
//! asked.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::ui_widgets::ToggleChecked;
use jackdaw_api::prelude::*;
use jackdaw_feathers::panel_card::DisclosureSection;
use jackdaw_feathers::tokens;
use jackdaw_widgets::collapsible::CollapsibleSection;

use crate::EditorEntity;
use crate::game_query::{GameEntities, GameQueries, GameQuery, ask_game};
use crate::pie::{InstanceKey, focused_game_is_live, focused_live_instance};

/// How often an open Runtime group asks the game again.
pub const RUNTIME_REFRESH: Duration = Duration::from_secs(1);

/// Where an Outliner panel puts its Runtime group.
#[derive(Component, Default)]
#[require(EditorEntity, Node)]
pub struct RuntimeGroupMount;

/// The Runtime group's collapsible section, and the rows its body shows.
#[derive(Component)]
pub struct RuntimeGroupSection {
    body: Entity,
    shown: Option<u64>,
    picked: Option<u64>,
}

/// A Runtime group row, naming its entity as `Entity::to_bits` in the game.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeEntityRow(pub u64);

/// One line of the Runtime group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeRow {
    Entity {
        entity: u64,
        label: String,
        /// The authored entity it sits under, for a top-level row.
        under: Option<String>,
        depth: usize,
    },
    Note(String),
}

/// The question in flight for the Runtime group and the rows of the last answer.
#[derive(Resource, Default)]
pub struct RuntimeGroup {
    key: Option<InstanceKey>,
    request: Option<u64>,
    asked: Option<Instant>,
    rows: Vec<RuntimeRow>,
    version: u64,
}

impl RuntimeGroup {
    /// The rows of the last answer.
    pub fn rows(&self) -> &[RuntimeRow] {
        &self.rows
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<RuntimeGroup>().add_systems(
        Update,
        sync_runtime_group
            .after(crate::pie::drain_game_events)
            .run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<HierarchyRuntimeGroupOp>()
        .register_operator::<HierarchyPickRuntimeOp>();
}

/// Open or close the Outliner's Runtime group.
#[operator(
    id = "hierarchy.runtime_group",
    label = "Show Runtime Entities",
    description = "Open or close the Outliner's read-only group of entities the running game spawned.",
    allows_undo = false,
    is_available = focused_game_is_live,
    params(on(bool, doc = "Open or closed. Omit to flip it."))
)]
pub(crate) fn hierarchy_runtime_group(
    params: In<OperatorParameters>,
    sections: Query<(Entity, &CollapsibleSection), With<RuntimeGroupSection>>,
    toggles: Query<(Entity, &DisclosureSection)>,
    mut commands: Commands,
) -> OperatorResult {
    if sections.is_empty() {
        return OperatorResult::Cancelled;
    }
    for (section, state) in &sections {
        let open = params.as_bool("on").unwrap_or(state.collapsed);
        if open != state.collapsed {
            continue;
        }
        if let Some((toggle, _)) = toggles.iter().find(|(_, link)| link.0 == section) {
            commands.trigger(ToggleChecked { entity: toggle });
        }
    }
    OperatorResult::Finished
}

/// Show a game-spawned entity in the Inspector, read-only.
#[operator(
    id = "hierarchy.pick_runtime",
    label = "Inspect Runtime Entity",
    description = "Show an entity the running game spawned in the Inspector, read-only. Clears the selection.",
    allows_undo = false,
    is_available = focused_game_is_live,
    params(entity(i64, doc = "The entity in the game, as its Entity::to_bits."))
)]
pub(crate) fn hierarchy_pick_runtime(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(bits) = params.as_int("entity") else {
        return OperatorResult::Cancelled;
    };
    let bits = bits as u64;
    commands.queue(move |world: &mut World| {
        crate::selection::clear_selection_in_world(world);
        crate::game_query::pick_runtime_entity(world, bits);
    });
    OperatorResult::Finished
}

/// Keep each Runtime group present while a game is live, ask the game for its
/// runtime entities while a group is open, and show the answer.
pub(crate) fn sync_runtime_group(world: &mut World) -> Result {
    let mut sections = world.query_filtered::<Entity, With<RuntimeGroupSection>>();
    let sections: Vec<Entity> = sections.iter(world).collect();
    let Some(key) = focused_live_instance(world) else {
        for section in sections {
            world.entity_mut(section).despawn();
        }
        if world.resource::<RuntimeGroup>().key.is_some() {
            *world.resource_mut::<RuntimeGroup>() = RuntimeGroup::default();
        }
        return Ok(());
    };
    if world.resource::<RuntimeGroup>().key.as_ref() != Some(&key) {
        let mut group = world.resource_mut::<RuntimeGroup>();
        let version = group.version + 1;
        *group = RuntimeGroup {
            key: Some(key),
            version,
            ..default()
        };
    }

    spawn_missing_sections(world);

    let mut open = world.query::<(&CollapsibleSection, &RuntimeGroupSection)>();
    let expanded = open.iter(world).any(|(state, _)| !state.collapsed);
    if !expanded {
        let mut group = world.resource_mut::<RuntimeGroup>();
        group.request = None;
        group.asked = None;
        return Ok(());
    }

    take_answer(world);
    let due = world
        .resource::<RuntimeGroup>()
        .asked
        .is_none_or(|asked| asked.elapsed() >= RUNTIME_REFRESH);
    if world.resource::<RuntimeGroup>().request.is_none()
        && due
        && let Ok(request) = ask_game(world, GameQuery::Runtime)
    {
        let mut group = world.resource_mut::<RuntimeGroup>();
        group.request = Some(request);
        group.asked = Some(Instant::now());
    }

    show_rows(world);
    Ok(())
}

fn spawn_missing_sections(world: &mut World) {
    let mut held = world.query_filtered::<&ChildOf, With<RuntimeGroupSection>>();
    let held: HashSet<Entity> = held.iter(world).map(ChildOf::parent).collect();
    let mut mounts = world.query_filtered::<Entity, With<RuntimeGroupMount>>();
    let bare: Vec<Entity> = mounts
        .iter(world)
        .filter(|mount| !held.contains(mount))
        .collect();
    for mount in bare {
        let mut commands = world.commands();
        let (section, body) = jackdaw_feathers::collapsible::collapsible_section_open(
            &mut commands,
            "Runtime",
            mount,
            false,
        );
        commands.entity(section).insert((
            RuntimeGroupSection {
                body,
                shown: None,
                picked: None,
            },
            jackdaw_feathers::tooltip::Tooltip::title("Runtime").with_description(
                "Entities the running game spawned. Read-only; nothing here is saved.",
            ),
        ));
        world.flush();
    }
}

fn take_answer(world: &mut World) {
    let Some(request) = world.resource::<RuntimeGroup>().request else {
        return;
    };
    let Some(answer) = world.resource_mut::<GameQueries>().take(request) else {
        return;
    };
    let rows = match answer {
        Ok(found) => runtime_rows(&found),
        Err(reason) => vec![RuntimeRow::Note(format!(
            "The game did not answer: {reason}"
        ))],
    };
    let mut group = world.resource_mut::<RuntimeGroup>();
    group.request = None;
    if group.rows != rows {
        group.rows = rows;
        group.version += 1;
    }
}

/// The rows an answer shows: each runtime entity under its runtime parent, the
/// top ones naming the authored entity they sit under.
pub fn runtime_rows(found: &GameEntities) -> Vec<RuntimeRow> {
    let runtime: HashMap<u64, &jackdaw_pie_protocol::GameEntity> = found
        .entities
        .iter()
        .filter(|entity| entity.node.is_none())
        .map(|entity| (entity.entity, entity))
        .collect();
    let authored: HashMap<u64, String> = found
        .entities
        .iter()
        .filter(|entity| entity.node.is_some())
        .map(|entity| {
            (
                entity.entity,
                entity_label(entity.entity, entity.name.as_deref()),
            )
        })
        .collect();
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut roots = Vec::new();
    for entity in runtime.values() {
        match entity.parent.filter(|parent| runtime.contains_key(parent)) {
            Some(parent) => children.entry(parent).or_default().push(entity.entity),
            None => roots.push(entity.entity),
        }
    }
    let label_of = |bits: u64| entity_label(bits, runtime[&bits].name.as_deref());
    let order =
        |list: &mut Vec<u64>| list.sort_by_key(|&bits| (label_of(bits).to_lowercase(), bits));
    order(&mut roots);
    for list in children.values_mut() {
        order(list);
    }

    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(u64, usize)> = roots.iter().rev().map(|&bits| (bits, 0)).collect();
    while let Some((bits, depth)) = stack.pop() {
        if !seen.insert(bits) {
            continue;
        }
        let under = (depth == 0)
            .then(|| {
                runtime[&bits]
                    .parent
                    .and_then(|parent| authored.get(&parent).cloned())
            })
            .flatten();
        rows.push(RuntimeRow::Entity {
            entity: bits,
            label: label_of(bits),
            under,
            depth,
        });
        if let Some(below) = children.get(&bits) {
            stack.extend(below.iter().rev().map(|&child| (child, depth + 1)));
        }
    }
    if rows.is_empty() {
        rows.push(RuntimeRow::Note("Nothing spawned at runtime".to_string()));
    }
    if found.truncated {
        rows.push(RuntimeRow::Note("More entities than shown".to_string()));
    }
    rows
}

/// A game entity's name, or `Entity <index>` without one.
pub fn entity_label(bits: u64, name: Option<&str>) -> String {
    match name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("Entity {}", bits as u32),
    }
}

fn show_rows(world: &mut World) {
    let version = world.resource::<RuntimeGroup>().version;
    let picked = world
        .get_resource::<GameQueries>()
        .and_then(GameQueries::runtime_pick);
    let mut sections = world.query::<(Entity, &RuntimeGroupSection)>();
    let stale: Vec<(Entity, Entity, bool)> = sections
        .iter(world)
        .filter(|(_, section)| section.shown != Some(version) || section.picked != picked)
        .map(|(entity, section)| (entity, section.body, section.shown != Some(version)))
        .collect();
    for (section, body, rebuild) in stale {
        if rebuild {
            world.entity_mut(body).despawn_related::<Children>();
            let rows = world.resource::<RuntimeGroup>().rows.clone();
            let mut commands = world.commands();
            for row in &rows {
                spawn_row(&mut commands, body, row);
            }
            world.flush();
        }
        mark_picked_row(world, body, picked);
        if let Some(mut held) = world.get_mut::<RuntimeGroupSection>(section) {
            held.shown = Some(version);
            held.picked = picked;
        }
    }
}

/// Paint the row of the picked runtime entity selected, and no other.
fn mark_picked_row(world: &mut World, body: Entity, picked: Option<u64>) {
    let rows: Vec<(Entity, u64)> = world
        .get::<Children>(body)
        .map(|children| {
            children
                .iter()
                .filter_map(|row| world.get::<RuntimeEntityRow>(row).map(|bits| (row, bits.0)))
                .collect()
        })
        .unwrap_or_default();
    for (row, bits) in rows {
        let mut row = world.entity_mut(row);
        if Some(bits) == picked {
            row.insert(bevy::ui::Selected);
        } else {
            row.remove::<bevy::ui::Selected>();
        }
    }
}

/// The game entity whose row is drawn selected in a Runtime group.
pub fn runtime_row_picked(world: &mut World) -> Option<u64> {
    let mut rows = world.query_filtered::<&RuntimeEntityRow, With<bevy::ui::Selected>>();
    rows.iter(world).next().map(|row| row.0)
}

fn spawn_row(commands: &mut Commands, body: Entity, row: &RuntimeRow) {
    match row {
        RuntimeRow::Entity {
            entity,
            label,
            under,
            depth,
        } => {
            let row = commands
                .spawn((
                    RuntimeEntityRow(*entity),
                    jackdaw_feathers::list_view::list_row(),
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(tokens::SPACING_SM),
                        width: Val::Percent(100.0),
                        padding: UiRect {
                            left: Val::Px(tokens::SPACING_SM + *depth as f32 * tokens::SPACING_LG),
                            right: Val::Px(tokens::SPACING_SM),
                            top: Val::Px(tokens::SPACING_XS),
                            bottom: Val::Px(tokens::SPACING_XS),
                        },
                        ..default()
                    },
                    ChildOf(body),
                    children![(
                        Text::new(label.clone()),
                        TextFont {
                            font_size: tokens::TEXT_SIZE_SM,
                            ..default()
                        },
                        ThemedText,
                    )],
                ))
                .observe(on_runtime_row_click)
                .id();
            if let Some(under) = under {
                commands.spawn((
                    Text::new(format!("in {under}")),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    ChildOf(row),
                ));
            }
        }
        RuntimeRow::Note(note) => {
            commands.spawn((
                Text::new(note.clone()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
                    ..default()
                },
                ChildOf(body),
            ));
        }
    }
}

fn on_runtime_row_click(
    click: On<PointerClick>,
    rows: Query<&RuntimeEntityRow>,
    mut commands: Commands,
) {
    if click.event().button != PointerButton::Primary {
        return;
    }
    let Ok(row) = rows.get(click.event_target()) else {
        return;
    };
    commands
        .operator(HierarchyPickRuntimeOp::ID)
        .param("entity", row.0 as i64)
        .call();
}

/// The labels of the rows each Runtime group shows, in order, notes included.
pub fn runtime_group_labels(world: &mut World) -> Option<Vec<String>> {
    let mut sections = world.query::<&RuntimeGroupSection>();
    let section = sections.iter(world).next()?;
    section.shown?;
    Some(
        world
            .resource::<RuntimeGroup>()
            .rows
            .iter()
            .map(|row| match row {
                RuntimeRow::Entity { label, .. } => label.clone(),
                RuntimeRow::Note(note) => note.clone(),
            })
            .collect(),
    )
}

/// Whether a Runtime group is up and open.
pub fn runtime_group_open(world: &mut World) -> Option<bool> {
    let mut sections = world.query_filtered::<&CollapsibleSection, With<RuntimeGroupSection>>();
    sections.iter(world).next().map(|state| !state.collapsed)
}

#[cfg(test)]
mod tests {
    use jackdaw_pie_protocol::GameEntity;

    use super::*;

    fn spawned(
        entity: u64,
        name: Option<&str>,
        parent: Option<u64>,
        node: Option<u64>,
    ) -> GameEntity {
        GameEntity {
            entity,
            name: name.map(str::to_string),
            parent,
            node,
            components: Vec::new(),
            opaque: Vec::new(),
        }
    }

    #[test]
    fn runtime_entities_nest_under_their_runtime_parent_and_name_the_authored_one() {
        let found = GameEntities {
            entities: vec![
                spawned(1, Some("Zone"), None, Some(7)),
                spawned(43, Some("Spark"), Some(42), None),
                spawned(42, Some("Bullet"), Some(1), None),
                spawned(44, None, None, None),
            ],
            truncated: false,
        };
        assert_eq!(
            runtime_rows(&found),
            vec![
                RuntimeRow::Entity {
                    entity: 42,
                    label: "Bullet".to_string(),
                    under: Some("Zone".to_string()),
                    depth: 0,
                },
                RuntimeRow::Entity {
                    entity: 43,
                    label: "Spark".to_string(),
                    under: None,
                    depth: 1,
                },
                RuntimeRow::Entity {
                    entity: 44,
                    label: "Entity 44".to_string(),
                    under: None,
                    depth: 0,
                },
            ]
        );
    }

    #[test]
    fn an_empty_or_cut_short_answer_says_so() {
        let found = GameEntities {
            entities: Vec::new(),
            truncated: true,
        };
        assert_eq!(
            runtime_rows(&found),
            vec![
                RuntimeRow::Note("Nothing spawned at runtime".to_string()),
                RuntimeRow::Note("More entities than shown".to_string()),
            ]
        );
    }
}
