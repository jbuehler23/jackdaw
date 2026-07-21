//! Schedules view: every app schedule and the systems it runs, in run order.
//!
//! The generic poll helper writes each `jackdaw/schedules` reply (a fixed
//! no-params method) into `SchedulesReply`. `systems` arrives already in run
//! order and each carries the system sets it belongs to; `edges` (dependency
//! ordering, for the later system graph) is ignored here. The panel rebuilds
//! its sections reactively when the reply changes: one section per schedule,
//! each listing its systems as boxes with their set names shown as chips.

use bevy::prelude::*;
use serde::Deserialize;

use jackdaw_feathers::list_view::list_view;
use jackdaw_feathers::tokens;

/// One system in a schedule: its full reflect type path and the names of the
/// system sets it belongs to.
#[derive(Deserialize, Clone)]
pub struct SystemInfo {
    pub name: String,
    pub sets: Vec<String>,
}

/// One schedule from a `jackdaw/schedules` reply: its label and the systems it
/// runs, already ordered as they execute.
#[derive(Deserialize, Clone)]
pub struct ScheduleInfo {
    pub schedule: String,
    pub systems: Vec<SystemInfo>,
}

/// The last parsed `jackdaw/schedules` reply, written by the poll helper. The
/// server's `initialized` and `edges` fields are not declared, so serde drops
/// them.
#[derive(Resource, Deserialize, Default)]
pub struct SchedulesReply {
    pub schedules: Vec<ScheduleInfo>,
}

/// Short system name: the final `::` segment of a reflect type path. A bare
/// name with no separator returns itself.
pub fn short_system_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

#[derive(Component)]
struct SchedulesPanel;

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
        children![(
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
        )],
    )
}

/// One schedule section: a plain name label above its systems, each rendered in
/// run order as a box.
fn schedule_section(info: &ScheduleInfo) -> impl Bundle {
    let boxes: Vec<_> = info.systems.iter().map(system_box).collect();
    (
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            row_gap: Val::Px(tokens::SPACING_SM),
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_MD)),
            ..default()
        },
        children![
            (
                Text::new(format!("{} ({})", info.schedule, info.systems.len())),
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    row_gap: Val::Px(tokens::SPACING_XS),
                    ..default()
                },
                Children::spawn(SpawnIter(boxes.into_iter())),
            ),
        ],
    )
}

/// One system box: the short system name followed by its set names as chips.
fn system_box(system: &SystemInfo) -> impl Bundle {
    let chips: Vec<_> = system.sets.iter().map(|s| set_chip(s)).collect();
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            width: Val::Percent(100.0),
            column_gap: Val::Px(tokens::SPACING_SM),
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
            ..default()
        },
        BackgroundColor(tokens::COMPONENT_CARD_BG),
        children![
            (
                Node {
                    flex_grow: 1.0,
                    min_width: Val::Px(0.0),
                    ..default()
                },
                children![(
                    Text::new(short_system_name(&system.name).to_string()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                )],
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    flex_shrink: 0.0,
                    column_gap: Val::Px(tokens::SPACING_SM),
                    row_gap: Val::Px(tokens::SPACING_XS),
                    ..default()
                },
                Children::spawn(SpawnIter(chips.into_iter())),
            ),
        ],
    )
}

/// A read-only chip for one system set the box belongs to.
fn set_chip(name: &str) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
            ..default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        children![(
            Text::new(short_system_name(name).to_string()),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Rebuild the schedule sections when a new reply arrives or the panel opens.
pub(crate) fn rebuild_schedules(
    reply: Option<Res<SchedulesReply>>,
    mut commands: Commands,
    rows_containers: Query<Entity, With<SchedRows>>,
    new_ui: Query<(), Added<SchedRows>>,
) {
    let reply_changed = matches!(reply.as_ref(), Some(r) if r.is_changed());
    if !reply_changed && new_ui.is_empty() {
        return;
    }

    let schedules = reply
        .as_ref()
        .map(|r| r.schedules.clone())
        .unwrap_or_default();

    for container in &rows_containers {
        commands.entity(container).despawn_children();
        for info in &schedules {
            commands.spawn((schedule_section(info), ChildOf(container)));
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
    fn reply_deserializes_from_brp_shape_ignoring_edges() {
        let value = serde_json::json!({
            "schedules": [
                {
                    "schedule": "Update",
                    "initialized": true,
                    "systems": [
                        { "name": "skybound::movement::step", "sets": ["Movement"] }
                    ],
                    "edges": [[0, 1]]
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
    }
}
