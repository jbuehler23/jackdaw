//! Archetypes view: every archetype in the running game as a sortable table.
//!
//! The generic poll helper writes each `jackdaw/archetypes` reply (a fixed
//! no-params method) into `ArchetypesReply`. `ArchetypeSort` holds the active
//! column and direction the header buttons edit; the panel rebuilds its rows
//! reactively when either resource changes, sorting a clone of the reply with
//! the pure `sort_rows` before spawning the table.

use bevy::prelude::*;
use serde::Deserialize;

use jackdaw_feathers::button::{ButtonClickEvent, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::list_view::list_view;
use jackdaw_feathers::tokens;

/// One archetype from a `jackdaw/archetypes` reply: the reflect type paths of
/// its component set, how many entities share it, and the summed component size
/// of a single entity in it.
#[derive(Deserialize, Clone)]
pub struct ArchetypeRow {
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
pub(crate) struct ArchHeader;

#[derive(Component)]
pub(crate) struct ArchRows;

/// Marks a clickable column header and the sort it selects.
#[derive(Component)]
pub(crate) struct SortHeader(SortKey);

/// Fixed pixel width of each numeric column, shared by the header buttons and
/// the row cells so the table reads as aligned columns.
const NUMERIC_COL_WIDTH: f32 = 96.0;

/// Short component name: the final `::` segment of a reflect type path.
fn short_name(type_path: &str) -> &str {
    type_path.rsplit("::").next().unwrap_or(type_path)
}

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
                ArchHeader,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    width: Val::Percent(100.0),
                    column_gap: Val::Px(tokens::SPACING_SM),
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
                children![(ArchRows, list_view())],
            ),
        ],
    )
}

/// A clickable column header. The active column shows a plain direction caret
/// and the `Active` variant so the current sort is visible at a glance.
fn sort_header(label: &str, key: SortKey, sort: &ArchetypeSort, grow: bool) -> impl Bundle {
    let active = sort.key == key;
    let content = if active {
        let caret = if sort.ascending { "^" } else { "v" };
        format!("{label} {caret}")
    } else {
        label.to_string()
    };
    let variant = if active {
        ButtonVariant::Active
    } else {
        ButtonVariant::Default
    };
    (
        Node {
            width: if grow {
                Val::Auto
            } else {
                Val::Px(NUMERIC_COL_WIDTH)
            },
            flex_grow: if grow { 1.0 } else { 0.0 },
            flex_shrink: 0.0,
            ..default()
        },
        children![(
            button(ButtonProps::new(content).with_variant(variant).align_left(),),
            SortHeader(key),
        )],
    )
}

/// One table row: the component set as chips followed by the entity count and
/// per-entity byte size in fixed-width, right-aligned numeric cells.
fn arch_row(row: &ArchetypeRow) -> impl Bundle {
    let chips: Vec<_> = row.components.iter().map(|c| comp_chip(c)).collect();
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            width: Val::Percent(100.0),
            column_gap: Val::Px(tokens::SPACING_SM),
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
            ..default()
        },
        children![
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
            numeric_cell(row.entity_count.to_string(), tokens::TEXT_PRIMARY),
            numeric_cell(
                format!("{} B", row.bytes_per_entity),
                tokens::TEXT_SECONDARY
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

/// A read-only chip for one component on the archetype.
fn comp_chip(type_path: &str) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
            ..default()
        },
        BackgroundColor(tokens::COMPONENT_CARD_BG),
        children![(
            Text::new(short_name(type_path)),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Rebuild the column headers and sorted rows when a new reply arrives, the
/// sort changes, or the panel opens.
pub(crate) fn rebuild_archetypes(
    reply: Option<Res<ArchetypesReply>>,
    sort: Res<ArchetypeSort>,
    mut commands: Commands,
    headers: Query<Entity, With<ArchHeader>>,
    rows_containers: Query<Entity, With<ArchRows>>,
    new_ui: Query<(), Or<(Added<ArchHeader>, Added<ArchRows>)>>,
) {
    let reply_changed = matches!(reply.as_ref(), Some(r) if r.is_changed());
    if !reply_changed && !sort.is_changed() && new_ui.is_empty() {
        return;
    }

    for container in &headers {
        commands.entity(container).despawn_children();
        commands.spawn((
            sort_header("Components", SortKey::Components, &sort, true),
            ChildOf(container),
        ));
        commands.spawn((
            sort_header("Entities", SortKey::Entities, &sort, false),
            ChildOf(container),
        ));
        commands.spawn((
            sort_header("Size", SortKey::Size, &sort, false),
            ChildOf(container),
        ));
    }

    let mut rows = reply
        .as_ref()
        .map(|r| r.archetypes.clone())
        .unwrap_or_default();
    sort_rows(&mut rows, sort.key, sort.ascending);

    for container in &rows_containers {
        commands.entity(container).despawn_children();
        for row in &rows {
            commands.spawn((arch_row(row), ChildOf(container)));
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

    fn row(components: usize, entity_count: u64, bytes_per_entity: u64) -> ArchetypeRow {
        ArchetypeRow {
            components: vec!["c".to_string(); components],
            entity_count,
            bytes_per_entity,
        }
    }

    #[test]
    fn sort_by_entities_descending_puts_largest_first() {
        let mut rows = vec![row(1, 10, 8), row(1, 512, 8), row(1, 64, 8)];
        sort_rows(&mut rows, SortKey::Entities, false);
        let counts: Vec<u64> = rows.iter().map(|r| r.entity_count).collect();
        assert_eq!(counts, vec![512, 64, 10]);
    }

    #[test]
    fn sort_by_entities_ascending_reverses() {
        let mut rows = vec![row(1, 10, 8), row(1, 512, 8), row(1, 64, 8)];
        sort_rows(&mut rows, SortKey::Entities, true);
        let counts: Vec<u64> = rows.iter().map(|r| r.entity_count).collect();
        assert_eq!(counts, vec![10, 64, 512]);
    }

    #[test]
    fn sort_by_size_orders_by_bytes_per_entity() {
        let mut rows = vec![row(1, 1, 96), row(1, 1, 16), row(1, 1, 48)];
        sort_rows(&mut rows, SortKey::Size, false);
        let bytes: Vec<u64> = rows.iter().map(|r| r.bytes_per_entity).collect();
        assert_eq!(bytes, vec![96, 48, 16]);
    }

    #[test]
    fn sort_by_components_orders_by_set_length() {
        let mut rows = vec![row(2, 1, 8), row(5, 1, 8), row(1, 1, 8)];
        sort_rows(&mut rows, SortKey::Components, false);
        let lengths: Vec<usize> = rows.iter().map(|r| r.components.len()).collect();
        assert_eq!(lengths, vec![5, 2, 1]);
    }

    #[test]
    fn short_name_takes_final_segment() {
        assert_eq!(
            short_name("bevy_transform::components::Transform"),
            "Transform"
        );
        assert_eq!(short_name("Bare"), "Bare");
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
        assert_eq!(
            reply.archetypes[0].components,
            vec!["skybound::Enemy".to_string()]
        );
    }
}
