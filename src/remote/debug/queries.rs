//! Live queries view: build a `With`/`Without` component filter and watch the
//! matching entities in the running game refresh over BRP.
//!
//! `QuerySpec` is the pure filter the panel edits and turns into `world.query`
//! parameters. A custom poll (the generic `BrpPollPlugin` cannot pass params)
//! sends `world.query` with the current spec on a timer and again whenever the
//! spec changes, then parses the reply into `QueryResults`. The panel rebuilds
//! its filter chips and result rows reactively when either resource changes.

use bevy::{prelude::*, tasks::Task, tasks::futures_lite::future};
use serde_json::{Value, json};

use jackdaw_feathers::button::{ButtonClickEvent, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::list_view::list_view;
use jackdaw_feathers::picker::{
    PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text, picker_item,
};
use jackdaw_feathers::tokens;

use super::super::{ConnectionManager, brp};
use super::style;

/// A `With`/`Without` component filter over the running world's entities.
/// Component names are full reflect type paths (for example
/// `skybound::Enemy`).
#[derive(Resource, Debug, Clone)]
pub struct QuerySpec {
    pub with: Vec<String>,
    pub without: Vec<String>,
}

impl Default for QuerySpec {
    fn default() -> Self {
        Self {
            with: vec!["bevy_transform::components::transform::Transform".to_string()],
            without: Vec::new(),
        }
    }
}

impl QuerySpec {
    /// True when a component set satisfies the filter: it holds every `with`
    /// component and none of the `without` ones.
    pub fn matches(&self, comps: &[String]) -> bool {
        self.with.iter().all(|c| comps.contains(c))
            && self.without.iter().all(|c| !comps.contains(c))
    }

    /// The parameters for a BRP `world.query` request built from this filter.
    pub fn to_brp_params(&self) -> serde_json::Value {
        json!({
            "data": { "components": self.with, "has": [], "option": [] },
            "filter": { "with": self.with, "without": self.without }
        })
    }
}

/// One matched entity from a `world.query` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRow {
    pub entity: u64,
    pub name: Option<String>,
    pub components: Vec<String>,
}

/// The last parsed `world.query` reply.
#[derive(Resource, Default)]
pub struct QueryResults {
    pub rows: Vec<QueryRow>,
}

/// In-flight `world.query` request task.
#[derive(Resource)]
pub struct QueryTask(pub Task<Result<serde_json::Value, anyhow::Error>>);

/// Timer controlling the query poll cadence.
#[derive(Resource)]
pub struct QueryPollTimer {
    pub timer: Timer,
}

impl Default for QueryPollTimer {
    fn default() -> Self {
        Self {
            timer: Timer::from_seconds(0.5, TimerMode::Repeating),
        }
    }
}

/// Which side of the filter a builder control targets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterSlot {
    With,
    Without,
}

#[derive(Component)]
struct QueriesPanel;

#[derive(Component)]
pub(crate) struct WithChips;

#[derive(Component)]
pub(crate) struct WithoutChips;

#[derive(Component)]
pub(crate) struct MatchCountText;

#[derive(Component)]
pub(crate) struct ResultsList;

/// Marks a builder "Add" button and which filter side it adds to.
#[derive(Component)]
pub(crate) struct AddFilterButton(FilterSlot);

/// Marks a removable filter chip: click removes `type_path` from `slot`.
#[derive(Component)]
pub(crate) struct FilterChip {
    slot: FilterSlot,
    type_path: String,
}

/// Marks the component picker opened for a filter side.
#[derive(Component)]
pub(crate) struct FilterPicker(FilterSlot);

// --------------------------- Reply parsing ---------------------------

/// Parse a `world.query` reply (a JSON array of matched rows) into [`QueryRow`]s.
pub fn parse_query_rows(value: &Value) -> Vec<QueryRow> {
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    array.iter().filter_map(parse_query_row).collect()
}

fn parse_query_row(value: &Value) -> Option<QueryRow> {
    let entity = value.get("entity")?.as_u64()?;
    let components = value.get("components").and_then(Value::as_object);

    let mut paths: Vec<String> = components
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    paths.sort();

    let name = components
        .and_then(|map| map.get("bevy_ecs::name::Name"))
        .and_then(extract_name_value);

    Some(QueryRow {
        entity,
        name,
        components: paths,
    })
}

/// A reflected `Name` serializes either as a bare string or as a struct with a
/// `name` field, depending on the game's serializer. Accept both.
fn extract_name_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    value.get("name").and_then(Value::as_str).map(String::from)
}

/// Component type paths offered by the picker, gathered from the connected
/// game's registry. Prefers types the registry flags as components; if the
/// registry shape hides that flag, falls back to every registered type so the
/// picker is never empty.
fn registry_component_options(manager: &ConnectionManager) -> Vec<String> {
    let Some(registry) = &manager.registry else {
        return Vec::new();
    };

    let mut set: std::collections::BTreeSet<String> = registry.components.keys().cloned().collect();

    let components: Vec<String> = registry
        .types
        .iter()
        .filter(|(_, schema)| schema_is_component(schema))
        .map(|(path, _)| path.clone())
        .collect();

    if components.is_empty() {
        set.extend(registry.types.keys().cloned());
    } else {
        set.extend(components);
    }

    set.into_iter().collect()
}

/// True when a `registry.schema` type entry declares itself a `Component`.
fn schema_is_component(schema: &Value) -> bool {
    schema
        .get("reflectTypes")
        .and_then(Value::as_array)
        .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("Component")))
}

// --------------------------- Polling ---------------------------

/// Request `world.query` with the current spec: immediately when the spec
/// changes, otherwise once per timer tick. At most one request is in flight and
/// the whole system is a no-op while disconnected.
pub(crate) fn query_poll(
    mut commands: Commands,
    manager: Res<ConnectionManager>,
    time: Res<Time>,
    mut poll_timer: ResMut<QueryPollTimer>,
    spec: Res<QuerySpec>,
    existing_task: Option<Res<QueryTask>>,
) {
    if !manager.is_connected() {
        return;
    }

    // A spec edit re-issues at once, replacing any in-flight request; otherwise
    // wait for the next tick and skip while a request is already pending.
    if !spec.is_changed() {
        if existing_task.is_some() {
            return;
        }
        poll_timer.timer.tick(time.delta());
        if !poll_timer.timer.just_finished() {
            return;
        }
    }

    let task = brp::brp_request(&manager.endpoint, "world.query", Some(spec.to_brp_params()));
    commands.insert_resource(QueryTask(task));
}

/// Drain the in-flight query task and write the parsed rows into [`QueryResults`].
pub(crate) fn poll_query_task(
    mut commands: Commands,
    task: Option<ResMut<QueryTask>>,
    mut results: ResMut<QueryResults>,
) {
    let Some(mut task) = task else { return };

    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<QueryTask>();

    match result {
        Ok(value) => {
            results.rows = parse_query_rows(&value);
        }
        Err(e) => {
            warn!("world.query request failed: {e}");
        }
    }
}

// --------------------------- Panel ---------------------------

/// Build the live queries panel content (no header: the dock tab is the title).
pub fn queries_panel_content() -> impl Bundle {
    (
        QueriesPanel,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    align_items: AlignItems::Center,
                    width: Val::Percent(100.0),
                    column_gap: Val::Px(tokens::SPACING_MD),
                    row_gap: Val::Px(tokens::SPACING_SM),
                    padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
                    ..default()
                },
                children![
                    filter_group("With", WithChips, FilterSlot::With),
                    filter_group("Without", WithoutChips, FilterSlot::Without),
                ],
            ),
            (
                MatchCountText,
                Text::new("Not connected"),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
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
                children![(ResultsList, list_view())],
            ),
        ],
    )
}

/// One filter side: a compact bordered group sized to its own content, not
/// the panel width, holding a muted label, a chip container (filled
/// reactively), and the "Add" button that opens the component picker.
fn filter_group<M: Component>(title: &str, chips_marker: M, slot: FilterSlot) -> impl Bundle {
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
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            (
                Text::new(title),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                chips_marker,
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(tokens::SPACING_SM),
                    row_gap: Val::Px(tokens::SPACING_SM),
                    ..default()
                },
            ),
            (
                button(ButtonProps::new("Add").with_variant(ButtonVariant::Default)),
                AddFilterButton(slot),
            ),
        ],
    )
}

/// A removable builder chip for one filtered component.
fn filter_chip(slot: FilterSlot, type_path: &str) -> impl Bundle {
    (
        button(
            ButtonProps::new(format!("{}  x", style::short_type_name(type_path)))
                .with_variant(ButtonVariant::Default),
        ),
        FilterChip {
            slot,
            type_path: type_path.to_string(),
        },
    )
}

/// A result row: the entity's name / id followed by its component chips.
fn result_row(row: &QueryRow) -> impl Bundle {
    let title = match &row.name {
        Some(name) => format!("{name}  (0x{:X})", row.entity),
        None => format!("Entity 0x{:X}", row.entity),
    };
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(tokens::SPACING_SM),
            row_gap: Val::Px(tokens::SPACING_XS),
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(tokens::SPACING_XS), Val::Px(1.0)),
            ..default()
        },
        children![(
            Text::new(title),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    )
}

/// Rebuild the builder chip rows when the filter changes or the panel opens.
pub(crate) fn rebuild_query_chips(
    spec: Res<QuerySpec>,
    mut commands: Commands,
    with_containers: Query<Entity, With<WithChips>>,
    without_containers: Query<Entity, With<WithoutChips>>,
    new_containers: Query<(), Or<(Added<WithChips>, Added<WithoutChips>)>>,
) {
    if !spec.is_changed() && new_containers.is_empty() {
        return;
    }

    for container in &with_containers {
        commands.entity(container).despawn_children();
        for type_path in &spec.with {
            commands.spawn((filter_chip(FilterSlot::With, type_path), ChildOf(container)));
        }
    }
    for container in &without_containers {
        commands.entity(container).despawn_children();
        for type_path in &spec.without {
            commands.spawn((
                filter_chip(FilterSlot::Without, type_path),
                ChildOf(container),
            ));
        }
    }
}

/// Rebuild the result rows and refresh the match count when results change or
/// the panel opens.
pub(crate) fn rebuild_query_results(
    results: Res<QueryResults>,
    manager: Res<ConnectionManager>,
    mut commands: Commands,
    list_containers: Query<Entity, With<ResultsList>>,
    mut count_texts: Query<&mut Text, With<MatchCountText>>,
    new_ui: Query<(), Or<(Added<ResultsList>, Added<MatchCountText>)>>,
) {
    if !results.is_changed() && new_ui.is_empty() {
        return;
    }

    let label = if manager.is_connected() {
        format!("{} matches", results.rows.len())
    } else {
        "Not connected".to_string()
    };
    for mut text in &mut count_texts {
        text.0 = label.clone();
    }

    for container in &list_containers {
        commands.entity(container).despawn_children();
        for row in &results.rows {
            let row_entity = commands.spawn((result_row(row), ChildOf(container))).id();
            for type_path in &row.components {
                commands.spawn((style::component_chip(type_path), ChildOf(row_entity)));
            }
        }
    }
}

// --------------------------- Builder interaction ---------------------------

/// Open the component picker for a filter side when its "Add" button is clicked.
pub(crate) fn on_add_filter_clicked(
    event: On<ButtonClickEvent>,
    add_buttons: Query<&AddFilterButton>,
    open_pickers: Query<Entity, With<FilterPicker>>,
    manager: Res<ConnectionManager>,
    mut commands: Commands,
) {
    let Ok(add) = add_buttons.get(event.entity) else {
        return;
    };

    // Toggle: a second click while a picker is open closes it.
    if let Some(picker) = open_pickers.iter().next() {
        commands.entity(picker).despawn();
        return;
    }

    let options = registry_component_options(&manager);
    if options.is_empty() {
        return;
    }

    let props = PickerProps::new(picker_spawn_item, picker_select)
        .items(options)
        .title("Add Component")
        .placeholder(Some("Search Components.."));

    commands.spawn((
        props,
        FilterPicker(add.0),
        crate::EditorEntity,
        crate::BlocksCameraInput,
    ));
}

/// Remove a component from the filter when its chip is clicked.
pub(crate) fn on_filter_chip_clicked(
    event: On<ButtonClickEvent>,
    chips: Query<&FilterChip>,
    mut spec: ResMut<QuerySpec>,
) {
    let Ok(chip) = chips.get(event.entity) else {
        return;
    };
    match chip.slot {
        FilterSlot::With => spec.with.retain(|c| c != &chip.type_path),
        FilterSlot::Without => spec.without.retain(|c| c != &chip.type_path),
    }
}

fn picker_spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    mut commands: Commands,
) -> Result {
    commands.spawn((
        picker_item(matched.index),
        ChildOf(entities.list),
        children![match_text(matched.segments)],
    ));
    Ok(())
}

fn picker_select(
    input: In<SelectInput>,
    items: Query<&PickerItems<String>>,
    pickers: Query<&FilterPicker>,
    mut spec: ResMut<QuerySpec>,
    mut commands: Commands,
) -> Result {
    let picker = input.entities.picker;
    let value = items.get(picker)?.at(input.index)?.clone();

    if let Ok(target) = pickers.get(picker) {
        let present = match target.0 {
            FilterSlot::With => spec.with.contains(&value),
            FilterSlot::Without => spec.without.contains(&value),
        };
        if !present {
            match target.0 {
                FilterSlot::With => spec.with.push(value),
                FilterSlot::Without => spec.without.push(value),
            }
        }
    }

    commands.entity(picker).try_despawn();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_requires_all_with_and_no_without() {
        let q = QuerySpec {
            with: vec!["Enemy".into(), "Health".into()],
            without: vec!["Dead".into()],
        };
        assert!(q.matches(&["Enemy".into(), "Health".into(), "Collider".into()]));
        assert!(!q.matches(&["Enemy".into()]));
        assert!(!q.matches(&["Enemy".into(), "Health".into(), "Dead".into()]));
    }

    #[test]
    fn brp_params_lists_components_by_full_path() {
        let q = QuerySpec {
            with: vec!["skybound::Enemy".into()],
            without: vec![],
        };
        let p = q.to_brp_params();
        assert_eq!(p["data"]["components"][0], "skybound::Enemy");
    }

    #[test]
    fn parse_rows_extracts_id_name_and_sorted_components() {
        let reply = json!([
            {
                "entity": 42,
                "components": {
                    "skybound::Health": 100,
                    "bevy_ecs::name::Name": "Player",
                    "skybound::Enemy": null
                },
                "has": {}
            },
            {
                "entity": 7,
                "components": {},
                "has": {}
            }
        ]);
        let rows = parse_query_rows(&reply);
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0].entity, 42);
        assert_eq!(rows[0].name.as_deref(), Some("Player"));
        assert_eq!(
            rows[0].components,
            vec![
                "bevy_ecs::name::Name".to_string(),
                "skybound::Enemy".to_string(),
                "skybound::Health".to_string(),
            ]
        );

        assert_eq!(rows[1].entity, 7);
        assert_eq!(rows[1].name, None);
        assert!(rows[1].components.is_empty());
    }

    #[test]
    fn parse_rows_reads_struct_form_name() {
        let reply = json!([
            { "entity": 1, "components": { "bevy_ecs::name::Name": { "name": "Boss" } } }
        ]);
        let rows = parse_query_rows(&reply);
        assert_eq!(rows[0].name.as_deref(), Some("Boss"));
    }

    #[test]
    fn parse_rows_handles_non_array() {
        assert!(parse_query_rows(&json!({ "not": "an array" })).is_empty());
    }

    #[test]
    fn schema_is_component_reads_reflect_types() {
        assert!(schema_is_component(&json!({
            "reflectTypes": ["Component", "Serialize"]
        })));
        assert!(!schema_is_component(&json!({
            "reflectTypes": ["Resource"]
        })));
        assert!(!schema_is_component(&json!({})));
    }
}
