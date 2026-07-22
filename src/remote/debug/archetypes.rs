//! Archetypes view: every archetype in the running game as a sortable table.
//!
//! The generic poll helper writes each `jackdaw/archetypes` reply (a fixed
//! no-params method) into `ArchetypesReply`. `ArchetypeSort` holds the active
//! column and direction the header cells edit; the panel rebuilds its rows
//! reactively when either resource changes. Each row is stamped with its
//! original position in the reply (`ArchetypeRow::index`) before the pure
//! `sort_rows` reorders a clone of it, so the `#<index>` label a row shows
//! stays put no matter which column the table is sorted by.

use bevy::prelude::*;
use bevy::ui_widgets::observe;
use serde::Deserialize;

use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::list_view::list_view;
use jackdaw_feathers::tokens;

use super::style;

/// One archetype from a `jackdaw/archetypes` reply: the reflect type paths of
/// its component set, how many entities share it, and the summed component
/// size of a single entity in it. `index` is not part of the wire shape; it
/// is stamped in by `rebuild_archetypes` from the row's position in the reply
/// before sorting, so the table's `#<index>` label is stable across re-sorts.
#[derive(Deserialize, Clone)]
pub struct ArchetypeRow {
    #[serde(skip)]
    pub index: usize,
    pub components: Vec<String>,
    pub entity_count: u64,
    pub bytes_per_entity: u64,
}

/// The last parsed `jackdaw/archetypes` reply, written by the poll helper.
#[derive(Resource, Deserialize, Default)]
pub struct ArchetypesReply {
    pub archetypes: Vec<ArchetypeRow>,
}

/// Which column the archetype table is ordered by.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Entities,
    Size,
    Components,
}

/// The active sort column and direction. `ascending` false is the natural
/// order (largest first), matching the server's default `entity_count` sort.
#[derive(Resource)]
pub struct ArchetypeSort {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for ArchetypeSort {
    fn default() -> Self {
        Self {
            key: SortKey::Entities,
            ascending: false,
        }
    }
}

/// Order `rows` in place by `key`. Descending by default (largest first);
/// `ascending` reverses it. `Components` orders by component-set length.
#[expect(
    clippy::ptr_arg,
    reason = "the panel sorts an owned Vec clone and this is the panel's public sort entry point"
)]
pub fn sort_rows(rows: &mut Vec<ArchetypeRow>, key: SortKey, ascending: bool) {
    rows.sort_by(|a, b| {
        let ordering = match key {
            SortKey::Entities => a.entity_count.cmp(&b.entity_count),
            SortKey::Size => a.bytes_per_entity.cmp(&b.bytes_per_entity),
            SortKey::Components => a.components.len().cmp(&b.components.len()),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

#[derive(Component)]
struct ArchetypesPanel;

#[derive(Component)]
pub(crate) struct ArchMeta;

#[derive(Component)]
pub(crate) struct ArchHeader;

#[derive(Component)]
pub(crate) struct ArchRows;

/// Marks a clickable column header and the sort it selects.
#[derive(Component)]
pub(crate) struct SortHeader(SortKey);

/// Marks a table row so its hover observer can find its own `BackgroundColor`
/// even when the pointer event bubbles up from a child (a chip or a text
/// label) rather than landing on the row directly.
#[derive(Component)]
struct ArchRowMarker;

/// Fixed pixel width of each numeric column, shared by the header cells and
/// the row cells so the table reads as aligned columns.
const NUMERIC_COL_WIDTH: f32 = 96.0;
/// Fixed pixel width of the leading `#<index>` column.
const ID_COL_WIDTH: f32 = 52.0;

/// Build the archetypes panel content (no header: the dock tab is the title).
pub fn archetypes_panel_content() -> impl Bundle {
    (
        ArchetypesPanel,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (
                ArchMeta,
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
            ),
            (
                ArchHeader,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    width: Val::Percent(100.0),
                    column_gap: Val::Px(tokens::SPACING_SM),
                    padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
                    border: UiRect::bottom(Val::Px(1.0)),
                    ..default()
                },
                BorderColor::all(tokens::BORDER_SUBTLE),
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
                children![(ArchRows, list_view())],
            ),
        ],
    )
}

/// A plain, non-sortable column-header label at a fixed width.
fn plain_header(label: &str, width: f32) -> impl Bundle {
    (
        Node {
            width: Val::Px(width),
            flex_shrink: 0.0,
            padding: UiRect::axes(Val::Px(tokens::SPACING_XS), Val::Px(tokens::SPACING_XS)),
            ..default()
        },
        children![(
            Text::new(label.to_uppercase()),
            TextFont {
                font_size: tokens::TEXT_SIZE_XS,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// A clickable column header: uppercase label, `TEXT_SIZE_XS`/`TEXT_SECONDARY`
/// like the rest of the row, and a plain ASCII caret on the active column.
/// Built from raw pickable primitives rather than the `button` widget so the
/// header text can share the row's own small, muted styling; a click still
/// fires the same `ButtonClickEvent` `on_sort_header_clicked` already
/// listens for.
fn sort_header(label: &str, key: SortKey, sort: &ArchetypeSort, grow: bool) -> impl Bundle {
    let active = sort.key == key;
    let content = if active {
        let caret = if sort.ascending { " ^" } else { " v" };
        format!("{}{caret}", label.to_uppercase())
    } else {
        label.to_uppercase()
    };
    (
        SortHeader(key),
        Node {
            width: if grow {
                Val::Auto
            } else {
                Val::Px(NUMERIC_COL_WIDTH)
            },
            flex_grow: if grow { 1.0 } else { 0.0 },
            flex_shrink: 0.0,
            justify_content: if grow {
                JustifyContent::Start
            } else {
                JustifyContent::FlexEnd
            },
            padding: UiRect::axes(Val::Px(tokens::SPACING_XS), Val::Px(tokens::SPACING_XS)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        children![(
            Text::new(content),
            TextFont {
                font_size: tokens::TEXT_SIZE_XS,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
        observe(on_header_over),
        observe(on_header_out),
        observe(on_header_pressed),
    )
}

fn on_header_over(hover: On<Pointer<Over>>, mut q: Query<&mut BackgroundColor, With<SortHeader>>) {
    if let Ok(mut bg) = q.get_mut(hover.event_target()) {
        bg.0 = tokens::HOVER_BG;
    }
}

fn on_header_out(out: On<Pointer<Out>>, mut q: Query<&mut BackgroundColor, With<SortHeader>>) {
    if let Ok(mut bg) = q.get_mut(out.event_target()) {
        bg.0 = Color::NONE;
    }
}

fn on_header_pressed(click: On<Pointer<Click>>, mut commands: Commands) {
    commands.trigger(ButtonClickEvent {
        entity: click.event_target(),
    });
}

/// One table row: `#<index>`, the entities bar cell, the row-size cell, then
/// the component set as chips. A subtle bottom border separates rows; hover
/// tints the whole row.
fn arch_row(row: &ArchetypeRow, max_count: u64) -> impl Bundle {
    let chips: Vec<_> = row
        .components
        .iter()
        .map(|c| style::component_chip(c))
        .collect();
    (
        ArchRowMarker,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            width: Val::Percent(100.0),
            column_gap: Val::Px(tokens::SPACING_SM),
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            id_cell(row.index),
            entities_cell(row.entity_count, max_count),
            numeric_cell(
                format!("{} B", row.bytes_per_entity),
                tokens::TEXT_SECONDARY
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    flex_grow: 1.0,
                    min_width: Val::Px(0.0),
                    column_gap: Val::Px(tokens::SPACING_SM),
                    row_gap: Val::Px(tokens::SPACING_XS),
                    ..default()
                },
                Children::spawn(SpawnIter(chips.into_iter())),
            ),
        ],
        observe(on_row_over),
        observe(on_row_out),
    )
}

fn on_row_over(hover: On<Pointer<Over>>, mut q: Query<&mut BackgroundColor, With<ArchRowMarker>>) {
    if let Ok(mut bg) = q.get_mut(hover.event_target()) {
        bg.0 = tokens::HOVER_BG;
    }
}

fn on_row_out(out: On<Pointer<Out>>, mut q: Query<&mut BackgroundColor, With<ArchRowMarker>>) {
    if let Ok(mut bg) = q.get_mut(out.event_target()) {
        bg.0 = Color::NONE;
    }
}

/// The leading `#<index>` cell.
fn id_cell(index: usize) -> impl Bundle {
    (
        Node {
            width: Val::Px(ID_COL_WIDTH),
            flex_shrink: 0.0,
            ..default()
        },
        children![(
            Text::new(format!("#{index}")),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// A fixed-width cell showing `count` right-aligned in front of a
/// `style::count_bar` fill sized by `count`'s share of `max_count`.
fn entities_cell(count: u64, max_count: u64) -> impl Bundle {
    let fraction = if max_count == 0 {
        0.0
    } else {
        count as f32 / max_count as f32
    };
    (
        Node {
            width: Val::Px(NUMERIC_COL_WIDTH),
            flex_shrink: 0.0,
            justify_content: JustifyContent::FlexEnd,
            align_items: AlignItems::Center,
            ..default()
        },
        children![
            style::count_bar(fraction),
            (
                Text::new(count.to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
        ],
    )
}

/// A fixed-width, right-aligned numeric cell.
fn numeric_cell(value: String, color: Color) -> impl Bundle {
    (
        Node {
            width: Val::Px(NUMERIC_COL_WIDTH),
            flex_shrink: 0.0,
            justify_content: JustifyContent::FlexEnd,
            ..default()
        },
        children![(
            Text::new(value),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(color),
        )],
    )
}

/// Rebuild the meta line, column headers, and sorted rows when a new reply
/// arrives, the sort changes, or the panel opens.
pub(crate) fn rebuild_archetypes(
    reply: Option<Res<ArchetypesReply>>,
    sort: Res<ArchetypeSort>,
    mut commands: Commands,
    meta_containers: Query<Entity, With<ArchMeta>>,
    headers: Query<Entity, With<ArchHeader>>,
    rows_containers: Query<Entity, With<ArchRows>>,
    new_ui: Query<(), Or<(Added<ArchMeta>, Added<ArchHeader>, Added<ArchRows>)>>,
) {
    let reply_changed = matches!(reply.as_ref(), Some(r) if r.is_changed());
    if !reply_changed && !sort.is_changed() && new_ui.is_empty() {
        return;
    }

    let archetypes: &[ArchetypeRow] = reply
        .as_ref()
        .map(|r| r.archetypes.as_slice())
        .unwrap_or(&[]);
    let total_entities: u64 = archetypes.iter().map(|a| a.entity_count).sum();
    let meta_right = format!(
        "{} archetypes,  {} entities",
        archetypes.len(),
        total_entities
    );

    for container in &meta_containers {
        commands.entity(container).despawn_children();
        commands.spawn((
            style::panel_meta("Every unique component set in the world", &meta_right),
            ChildOf(container),
        ));
    }

    for container in &headers {
        commands.entity(container).despawn_children();
        commands.spawn((plain_header("Archetype", ID_COL_WIDTH), ChildOf(container)));
        commands.spawn((
            sort_header("Entities", SortKey::Entities, &sort, false),
            ChildOf(container),
        ));
        commands.spawn((
            sort_header("Row Size", SortKey::Size, &sort, false),
            ChildOf(container),
        ));
        commands.spawn((
            sort_header("Components", SortKey::Components, &sort, true),
            ChildOf(container),
        ));
    }

    let mut rows: Vec<ArchetypeRow> = archetypes
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, mut row)| {
            row.index = index;
            row
        })
        .collect();
    sort_rows(&mut rows, sort.key, sort.ascending);
    let max_count = rows.iter().map(|r| r.entity_count).max().unwrap_or(0);

    for container in &rows_containers {
        commands.entity(container).despawn_children();
        for row in &rows {
            commands.spawn((arch_row(row, max_count), ChildOf(container)));
        }
    }
}

/// Select a column when its header is clicked; a click on the active column
/// toggles the direction, a click on another switches to it (descending).
pub(crate) fn on_sort_header_clicked(
    event: On<ButtonClickEvent>,
    headers: Query<&SortHeader>,
    mut sort: ResMut<ArchetypeSort>,
) {
    let Ok(header) = headers.get(event.entity) else {
        return;
    };
    if sort.key == header.0 {
        sort.ascending = !sort.ascending;
    } else {
        sort.key = header.0;
        sort.ascending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        index: usize,
        components: usize,
        entity_count: u64,
        bytes_per_entity: u64,
    ) -> ArchetypeRow {
        ArchetypeRow {
            index,
            components: vec!["c".to_string(); components],
            entity_count,
            bytes_per_entity,
        }
    }

    #[test]
    fn sort_by_entities_descending_puts_largest_first() {
        let mut rows = vec![row(0, 1, 10, 8), row(1, 1, 512, 8), row(2, 1, 64, 8)];
        sort_rows(&mut rows, SortKey::Entities, false);
        let counts: Vec<u64> = rows.iter().map(|r| r.entity_count).collect();
        assert_eq!(counts, vec![512, 64, 10]);
    }

    #[test]
    fn sort_by_entities_ascending_reverses() {
        let mut rows = vec![row(0, 1, 10, 8), row(1, 1, 512, 8), row(2, 1, 64, 8)];
        sort_rows(&mut rows, SortKey::Entities, true);
        let counts: Vec<u64> = rows.iter().map(|r| r.entity_count).collect();
        assert_eq!(counts, vec![10, 64, 512]);
    }

    #[test]
    fn sort_by_size_orders_by_bytes_per_entity() {
        let mut rows = vec![row(0, 1, 1, 96), row(1, 1, 1, 16), row(2, 1, 1, 48)];
        sort_rows(&mut rows, SortKey::Size, false);
        let bytes: Vec<u64> = rows.iter().map(|r| r.bytes_per_entity).collect();
        assert_eq!(bytes, vec![96, 48, 16]);
    }

    #[test]
    fn sort_by_components_orders_by_set_length() {
        let mut rows = vec![row(0, 2, 1, 8), row(1, 5, 1, 8), row(2, 1, 1, 8)];
        sort_rows(&mut rows, SortKey::Components, false);
        let lengths: Vec<usize> = rows.iter().map(|r| r.components.len()).collect();
        assert_eq!(lengths, vec![5, 2, 1]);
    }

    #[test]
    fn sort_preserves_each_rows_original_index() {
        let mut rows = vec![row(0, 1, 10, 8), row(1, 1, 512, 8), row(2, 1, 64, 8)];
        sort_rows(&mut rows, SortKey::Entities, false);
        let indices: Vec<usize> = rows.iter().map(|r| r.index).collect();
        assert_eq!(indices, vec![1, 2, 0]);
    }

    #[test]
    fn reply_deserializes_from_brp_shape() {
        let value = serde_json::json!({
            "archetypes": [
                { "components": ["skybound::Enemy"], "entity_count": 512, "bytes_per_entity": 96 }
            ]
        });
        let reply: ArchetypesReply = serde_json::from_value(value).unwrap();
        assert_eq!(reply.archetypes.len(), 1);
        assert_eq!(reply.archetypes[0].entity_count, 512);
        assert_eq!(reply.archetypes[0].bytes_per_entity, 96);
        assert_eq!(reply.archetypes[0].index, 0);
        assert_eq!(
            reply.archetypes[0].components,
            vec!["skybound::Enemy".to_string()]
        );
    }
}
