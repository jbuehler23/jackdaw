//! System dependency graph panel: one schedule's systems as a layered node
//! graph, drawn via `graph::spawn_graph`, with dependency-ordering edges and
//! scheduler ambiguities. Reads the same polled `SchedulesReply` the
//! Schedules panel reads (`schedules.rs`); no separate poll.

use bevy::prelude::*;

use jackdaw_feathers::button::{ButtonClickEvent, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::tokens;

use super::graph::{self, GraphEdgeMaterial, GraphNodeSpec};
use super::schedules::SchedulesReply;
use super::style;

/// Which schedule's graph is shown. Defaults to `"Update"`; the rebuild
/// system falls back to the reply's first schedule if `"Update"` is absent.
#[derive(Resource)]
pub struct SelectedSchedule(pub String);

impl Default for SelectedSchedule {
    fn default() -> Self {
        Self("Update".to_string())
    }
}

#[derive(Component)]
struct DepgraphPanel;

#[derive(Component)]
pub(crate) struct DepgraphMeta;

#[derive(Component)]
pub(crate) struct DepgraphSelector;

#[derive(Component)]
pub(crate) struct DepgraphBanner;

#[derive(Component)]
pub(crate) struct DepgraphCanvas;

/// Marks a schedule-selector button with the schedule name it selects.
#[derive(Component)]
pub(crate) struct ScheduleButton(String);

/// Build the dependency graph panel content (no header: the dock tab is the
/// title).
pub fn depgraph_panel_content() -> impl Bundle {
    (
        DepgraphPanel,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (
                DepgraphMeta,
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
            ),
            (
                DepgraphSelector,
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(tokens::SPACING_SM),
                    row_gap: Val::Px(tokens::SPACING_SM),
                    padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                    ..default()
                },
            ),
            (
                DepgraphBanner,
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
                    ..default()
                },
                children![(
                    DepgraphCanvas,
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                )],
            ),
        ],
    )
}

/// A warning-tinted banner explaining what "ambiguities" means, shown only
/// when the selected schedule has at least one.
fn ambiguity_banner(count: usize) -> impl Bundle {
    let warning = Color::Srgba(tokens::DESTRUCTIVE_RED);
    (
        Node {
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(warning.with_alpha(0.12)),
        BorderColor::all(warning),
        children![(
            Text::new(format!(
                "{count} ambiguities: unordered systems with conflicting data access"
            )),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(warning),
        )],
    )
}

/// Rebuild the selector row, meta line, banner, and graph canvas when the
/// reply changes, the selected schedule changes, or the panel opens.
pub(crate) fn rebuild_depgraph(
    reply: Option<Res<SchedulesReply>>,
    selected: Res<SelectedSchedule>,
    mut commands: Commands,
    mut materials: ResMut<Assets<GraphEdgeMaterial>>,
    meta_containers: Query<Entity, With<DepgraphMeta>>,
    selector_containers: Query<Entity, With<DepgraphSelector>>,
    banner_containers: Query<Entity, With<DepgraphBanner>>,
    canvas_containers: Query<Entity, With<DepgraphCanvas>>,
    new_ui: Query<
        (),
        Or<(
            Added<DepgraphMeta>,
            Added<DepgraphSelector>,
            Added<DepgraphBanner>,
            Added<DepgraphCanvas>,
        )>,
    >,
) {
    let reply_changed = matches!(reply.as_ref(), Some(r) if r.is_changed());
    if !reply_changed && !selected.is_changed() && new_ui.is_empty() {
        return;
    }

    let schedules = reply
        .as_ref()
        .map(|r| r.schedules.as_slice())
        .unwrap_or(&[]);
    let active = schedules
        .iter()
        .find(|s| s.schedule == selected.0)
        .or_else(|| schedules.first());

    for container in &selector_containers {
        commands.entity(container).despawn_children();
        for info in schedules {
            let is_active = active.is_some_and(|a| a.schedule == info.schedule);
            let variant = if is_active {
                ButtonVariant::Active
            } else {
                ButtonVariant::Default
            };
            commands.spawn((
                button(ButtonProps::new(info.schedule.clone()).with_variant(variant)),
                ScheduleButton(info.schedule.clone()),
                ChildOf(container),
            ));
        }
    }

    let meta_right = match active {
        Some(info) => {
            let edge_total = info.edges.len() + info.ambiguities.len();
            let mut text = format!(
                "{}: {} systems, {} ambiguities",
                info.schedule,
                info.systems.len(),
                info.ambiguities.len()
            );
            if edge_total > graph::MAX_EDGES {
                text.push_str(&format!(
                    ", showing {} of {edge_total} edges",
                    graph::MAX_EDGES
                ));
            }
            text
        }
        None => "no schedules".to_string(),
    };
    for container in &meta_containers {
        commands.entity(container).despawn_children();
        commands.spawn((
            style::panel_meta("System dependency graph", &meta_right),
            ChildOf(container),
        ));
    }

    let ambiguity_count = active.map(|a| a.ambiguities.len()).unwrap_or(0);
    for container in &banner_containers {
        commands.entity(container).despawn_children();
        if ambiguity_count > 0 {
            commands.spawn((ambiguity_banner(ambiguity_count), ChildOf(container)));
        }
    }

    for container in &canvas_containers {
        commands.entity(container).despawn_children();
        let Some(info) = active else { continue };
        let nodes: Vec<GraphNodeSpec> = info
            .systems
            .iter()
            .map(|s| GraphNodeSpec {
                label: style::short_type_name(&s.name),
            })
            .collect();
        let ambiguities: Vec<(usize, usize)> =
            info.ambiguities.iter().map(|a| (a.a, a.b)).collect();
        graph::spawn_graph(
            &mut commands,
            &mut materials,
            container,
            &nodes,
            &info.edges,
            &ambiguities,
        );
    }
}

/// Select a schedule when its selector button is clicked.
pub(crate) fn on_schedule_button_clicked(
    event: On<ButtonClickEvent>,
    buttons: Query<&ScheduleButton>,
    mut selected: ResMut<SelectedSchedule>,
) {
    let Ok(target) = buttons.get(event.entity) else {
        return;
    };
    if selected.0 != target.0 {
        selected.0 = target.0.clone();
    }
}
