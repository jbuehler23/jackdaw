//! Schedules view: every app schedule and the systems it runs, in run order.
//!
//! The generic poll helper writes each `jackdaw/schedules` reply (a fixed
//! no-params method) into `SchedulesReply`, shared with the System Graph
//! panel (`depgraph.rs`). `systems` arrives already in run order and each
//! carries the system sets it belongs to; `edges` (dependency ordering) and
//! `ambiguities` (scheduler conflicts) are carried through but only rendered
//! by the graph panel. This panel rebuilds its sections reactively when the
//! reply changes: one bordered card per schedule, each listing its systems
//! as boxes wrapped in run order.

use bevy::prelude::*;
use serde::Deserialize;

use jackdaw_feathers::list_view::list_view;
use jackdaw_feathers::tokens;

use super::style;

/// One system in a schedule: its full reflect type path and the names of the
/// system sets it belongs to.
#[derive(Deserialize, Clone)]
pub struct SystemInfo {
    pub name: String,
    pub sets: Vec<String>,
}

/// One scheduler-detected ambiguity: two systems the scheduler found no
/// ordering between despite conflicting data access. `a`/`b` index into the
/// same schedule's `systems` array as `edges`. An empty `components` list
/// means the conflict is on `World` access in general (e.g. an exclusive
/// system) rather than a specific component.
#[derive(Deserialize, Clone)]
pub struct Ambiguity {
    pub a: usize,
    pub b: usize,
    pub components: Vec<String>,
}

/// One schedule from a `jackdaw/schedules` reply: its label, the systems it
/// runs (already ordered as they execute), the dependency-ordering edges
/// between them, and the scheduler ambiguities found among them. `edges` and
/// `ambiguities` share the same index space as `systems`.
#[derive(Deserialize, Clone)]
pub struct ScheduleInfo {
    pub schedule: String,
    pub systems: Vec<SystemInfo>,
    #[serde(default)]
    pub edges: Vec<(usize, usize)>,
    #[serde(default)]
    pub ambiguities: Vec<Ambiguity>,
}

/// The last parsed `jackdaw/schedules` reply, written by the poll helper. The
/// server's `initialized` field is not declared, so serde drops it.
#[derive(Resource, Deserialize, Default)]
pub struct SchedulesReply {
    pub schedules: Vec<ScheduleInfo>,
}

/// Short system name: the final `::` segment of a reflect type path, with any
/// generic argument list stripped first. A bare name with no separator
/// returns itself.
pub fn short_system_name(name: &str) -> String {
    style::short_type_name(name)
}

#[derive(Component)]
struct SchedulesPanel;

#[derive(Component)]
pub(crate) struct SchedMeta;

#[derive(Component)]
pub(crate) struct SchedRows;

/// Build the schedules panel content (no header: the dock tab is the title).
pub fn schedules_panel_content() -> impl Bundle {
    (
        SchedulesPanel,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (
                SchedMeta,
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
            ),
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    overflow: Overflow::scroll_y(),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    ..default()
                },
                children![(SchedRows, list_view())],
            ),
        ],
    )
}

/// One schedule card: a bordered panel with a muted name/hint header row
/// above its systems, each rendered in run order as a wrapping row of boxes.
fn schedule_card(info: &ScheduleInfo) -> impl Bundle {
    let boxes: Vec<_> = info.systems.iter().map(system_box).collect();
    (
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            row_gap: Val::Px(tokens::SPACING_SM),
            padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
            margin: UiRect::bottom(Val::Px(tokens::SPACING_MD)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
            ..default()
        },
        BackgroundColor(tokens::COMPONENT_CARD_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            style::panel_meta(&info.schedule.to_uppercase(), "left to right = run order"),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    width: Val::Percent(100.0),
                    column_gap: Val::Px(tokens::SPACING_SM),
                    row_gap: Val::Px(tokens::SPACING_SM),
                    ..default()
                },
                Children::spawn(SpawnIter(boxes.into_iter())),
            ),
        ],
    )
}

/// One system box: the short system name followed by its set names as chips.
fn system_box(system: &SystemInfo) -> impl Bundle {
    let chips: Vec<_> = system
        .sets
        .iter()
        .map(|s| style::component_chip(s))
        .collect();
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            column_gap: Val::Px(tokens::SPACING_SM),
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
            ..default()
        },
        BackgroundColor(tokens::INPUT_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            (
                Text::new(short_system_name(&system.name)),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(tokens::SPACING_XS),
                    row_gap: Val::Px(tokens::SPACING_XS),
                    ..default()
                },
                Children::spawn(SpawnIter(chips.into_iter())),
            ),
        ],
    )
}

/// Rebuild the meta line and schedule cards when a new reply arrives or the
/// panel opens.
pub(crate) fn rebuild_schedules(
    reply: Option<Res<SchedulesReply>>,
    mut commands: Commands,
    meta_containers: Query<Entity, With<SchedMeta>>,
    rows_containers: Query<Entity, With<SchedRows>>,
    new_ui: Query<(), Or<(Added<SchedMeta>, Added<SchedRows>)>>,
) {
    let reply_changed = matches!(reply.as_ref(), Some(r) if r.is_changed());
    if !reply_changed && new_ui.is_empty() {
        return;
    }

    let schedules = reply
        .as_ref()
        .map(|r| r.schedules.clone())
        .unwrap_or_default();
    let total_systems: usize = schedules.iter().map(|s| s.systems.len()).sum();
    let meta_right = format!("{} schedules,  {} systems", schedules.len(), total_systems);

    for container in &meta_containers {
        commands.entity(container).despawn_children();
        commands.spawn((
            style::panel_meta("System run order per schedule", &meta_right),
            ChildOf(container),
        ));
    }

    for container in &rows_containers {
        commands.entity(container).despawn_children();
        for info in &schedules {
            commands.spawn((schedule_card(info), ChildOf(container)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_system_name_takes_final_segment() {
        assert_eq!(
            short_system_name("bevy_transform::systems::sync_simple_transforms"),
            "sync_simple_transforms"
        );
    }

    #[test]
    fn short_system_name_passes_bare_name_through() {
        assert_eq!(short_system_name("bare_system"), "bare_system");
    }

    #[test]
    fn reply_deserializes_edges_and_ambiguities() {
        let value = serde_json::json!({
            "schedules": [
                {
                    "schedule": "Update",
                    "initialized": true,
                    "systems": [
                        { "name": "skybound::movement::step", "sets": ["Movement"] }
                    ],
                    "edges": [[0, 1]],
                    "ambiguities": [
                        { "a": 0, "b": 1, "components": ["skybound::Health"] }
                    ]
                }
            ]
        });
        let reply: SchedulesReply = serde_json::from_value(value).unwrap();
        assert_eq!(reply.schedules.len(), 1);
        assert_eq!(reply.schedules[0].schedule, "Update");
        assert_eq!(reply.schedules[0].systems.len(), 1);
        assert_eq!(
            reply.schedules[0].systems[0].name,
            "skybound::movement::step"
        );
        assert_eq!(
            reply.schedules[0].systems[0].sets,
            vec!["Movement".to_string()]
        );
        assert_eq!(reply.schedules[0].edges, vec![(0, 1)]);
        assert_eq!(reply.schedules[0].ambiguities.len(), 1);
        assert_eq!(reply.schedules[0].ambiguities[0].a, 0);
        assert_eq!(reply.schedules[0].ambiguities[0].b, 1);
        assert_eq!(
            reply.schedules[0].ambiguities[0].components,
            vec!["skybound::Health".to_string()]
        );
    }

    #[test]
    fn reply_defaults_edges_and_ambiguities_when_absent() {
        let value = serde_json::json!({
            "schedules": [
                {
                    "schedule": "Update",
                    "initialized": true,
                    "systems": []
                }
            ]
        });
        let reply: SchedulesReply = serde_json::from_value(value).unwrap();
        assert!(reply.schedules[0].edges.is_empty());
        assert!(reply.schedules[0].ambiguities.is_empty());
    }
}
