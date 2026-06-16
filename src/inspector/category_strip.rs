use bevy::prelude::*;
use jackdaw_api_internal::inspector::{InspectorRegistry, resolve_active_category};
use jackdaw_feathers::{tokens, tooltip::Tooltip};
use std::borrow::Cow;

/// The inspector category whose cards are currently shown. Defaults to "object".
#[derive(Resource)]
pub(crate) struct ActiveInspectorCategory(pub(crate) Cow<'static, str>);

impl Default for ActiveInspectorCategory {
    fn default() -> Self {
        Self(Cow::Borrowed("object"))
    }
}

/// Category ids (in strip order) that have at least one card present for the
/// current entity. Used to dim absent tabs and to drive sticky/fallback.
pub(crate) fn applicable_categories<'a>(
    registry: &'a InspectorRegistry,
    present_type_paths: impl Iterator<Item = &'a str>,
) -> Vec<&'a str> {
    let present: std::collections::HashSet<&str> = present_type_paths
        .map(|tp| registry.category_for(tp))
        .collect();
    registry
        .categories_sorted()
        .into_iter()
        .map(|c| c.id.as_ref())
        .filter(|id| present.contains(id))
        .collect()
}

/// Category ids (in strip order) that are SHOWN for the current selection.
///
/// "object" is always shown. "mesh" is shown only when a Brush card is present.
/// "modifiers", "material", and "physics" are shown when a Brush is present OR
/// when the category already has a card. "components" and extension ids are
/// shown only when they have a card.
///
/// This is the canonical visibility rule shared by both the strip paint system
/// and the sticky-category resolver so the two cannot drift apart.
pub(super) fn shown_categories<'a>(
    registry: &'a InspectorRegistry,
    applicable: &[&'a str],
) -> Vec<&'a str> {
    let brush_present = applicable.contains(&"mesh");
    registry
        .categories_sorted()
        .into_iter()
        .map(|c| c.id.as_ref())
        .filter(|id| match *id {
            "object" => true,
            "mesh" => brush_present,
            "modifiers" | "material" | "physics" => brush_present || applicable.contains(id),
            _ => applicable.contains(id),
        })
        .collect()
}

/// Marker placed on the panel host entity. An observer fires on `Add` and
/// spawns the real strip rail as a sibling-via-child.
#[derive(Component)]
pub(crate) struct InspectorCategoryStripMount;

/// Marker on the strip rail container (the vertical icon column).
#[derive(Component)]
pub(super) struct InspectorCategoryStrip;

/// Marker on one tab button, carrying its category id.
#[derive(Component, Clone)]
pub(super) struct InspectorCategoryTab(pub(super) Cow<'static, str>);

/// The icon glyph child of an `InspectorCategoryTab`. Kept as a component
/// so the paint system can find it without querying all `Text` nodes.
#[derive(Component)]
pub(super) struct InspectorCategoryTabIcon;

/// Spawn the rail and one icon button per registered category under `parent`.
pub(super) fn spawn_category_strip(
    commands: &mut Commands,
    parent: Entity,
    registry: &InspectorRegistry,
    icon_font: &Handle<Font>,
) {
    // The rail is the mount entity itself. Inserting the layout here (rather
    // than spawning a child rail) keeps it a real flex item in the
    // [strip | content] row: a Node whose parent has no Node detaches from
    // layout, so the mount must carry the rail's Node directly.
    commands.entity(parent).insert((
        InspectorCategoryStrip,
        Node {
            width: Val::Px(tokens::SIDEBAR_WIDTH),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::vertical(Val::Px(tokens::SPACING_SM)),
            row_gap: Val::Px(tokens::SPACING_XS),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(tokens::PANEL_HEADER_BG),
    ));
    let rail = parent;

    for cat in registry.categories_sorted() {
        let cat_id = cat.id.clone();
        let label = cat.label.clone();

        let cell = commands
            .spawn((
                InspectorCategoryTab(cat_id.clone()),
                Tooltip::title(label.as_ref()),
                bevy::picking::hover::Hovered::default(),
                Node {
                    width: Val::Px(tokens::SIDEBAR_WIDTH),
                    height: Val::Px(tokens::SIDEBAR_WIDTH),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::left(Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                BorderColor::all(Color::NONE),
                ChildOf(rail),
            ))
            .id();

        commands.spawn((
            InspectorCategoryTabIcon,
            Text::new(String::from(cat.icon.unicode())),
            TextFont {
                font: icon_font.clone(),
                font_size: tokens::ICON_MD,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(cell),
        ));

        let click_id = cat_id.clone();
        commands.entity(cell).observe(
            move |_: On<Pointer<Click>>, mut active: ResMut<ActiveInspectorCategory>| {
                active.0 = click_id.clone();
            },
        );
    }
}

/// Each frame: for every tab cell, show or hide the cell and set
/// background / left-border / icon color for shown cells.
///
/// Tabs that are not relevant to the current selection are hidden
/// (`Display::None`). No greyed-out state exists.
///
/// Visibility rules:
/// - `object`: always shown.
/// - `mesh`: shown only when a Brush card is present.
/// - `modifiers`, `material`, `physics`: shown when a Brush is present OR
///   when the category already has a card.
/// - `components`: shown only when applicable (has a card).
/// - Extension categories: shown only when applicable.
///
/// Skips the repaint when neither the active category nor the displayed card
/// set has changed since the last run.
pub(super) fn paint_category_tabs(
    registry: Res<InspectorRegistry>,
    active: Res<ActiveInspectorCategory>,
    type_paths: Query<&super::ComponentDisplayTypePath>,
    added_paths: Query<(), Added<super::ComponentDisplayTypePath>>,
    removed_paths: RemovedComponents<super::ComponentDisplayTypePath>,
    tabs: Query<(Entity, &InspectorCategoryTab, &Children)>,
    mut node_query: Query<&mut Node>,
    mut bg_query: Query<&mut BackgroundColor>,
    mut border_query: Query<&mut BorderColor>,
    mut icon_query: Query<&mut TextColor, With<InspectorCategoryTabIcon>>,
) {
    // Skip repaint when neither the active category nor the displayed card
    // set has changed this frame.
    if !active.is_changed() && added_paths.is_empty() && removed_paths.is_empty() {
        return;
    }

    let present_paths: Vec<&str> = type_paths.iter().map(|p| p.0.as_str()).collect();
    let applicable = applicable_categories(&registry, present_paths.into_iter());
    let shown_set = shown_categories(&registry, &applicable);

    for (cell, tab, children) in &tabs {
        let id = tab.0.as_ref();
        let shown = shown_set.contains(&id);

        // Show or hide the cell.
        if let Ok(mut node) = node_query.get_mut(cell) {
            node.display = if shown { Display::Flex } else { Display::None };
        }

        if !shown {
            continue;
        }

        let is_active = id == active.0.as_ref();

        // Cell background and left-border accent.
        if let Ok(mut bg) = bg_query.get_mut(cell) {
            bg.0 = if is_active {
                tokens::PANEL_BG
            } else {
                Color::NONE
            };
        }
        if let Ok(mut border) = border_query.get_mut(cell) {
            *border = BorderColor::all(if is_active {
                tokens::ACCENT_BLUE
            } else {
                Color::NONE
            });
        }

        // Icon color.
        let icon_color = if is_active {
            tokens::TEXT_PRIMARY
        } else {
            tokens::TEXT_SECONDARY
        };

        for child in children.iter() {
            if let Ok(mut tc) = icon_query.get_mut(child) {
                tc.0 = icon_color;
            }
        }
    }
}

/// After the inspector rebuilds, keep the active category sticky (or fall back
/// to the first applicable), then re-trigger the category filter for the new
/// cards by writing the resource unconditionally (marking it changed).
pub(super) fn resolve_active_on_rebuild(
    registry: Res<InspectorRegistry>,
    cards: Query<&super::ComponentDisplayTypePath, With<super::ComponentDisplay>>,
    added: Query<(), Added<super::ComponentDisplay>>,
    mut removed: RemovedComponents<super::ComponentDisplay>,
    mut active: ResMut<ActiveInspectorCategory>,
) {
    let changed = !added.is_empty() || removed.read().next().is_some();
    if !changed {
        return;
    }
    let applicable = applicable_categories(&registry, cards.iter().map(|t| t.0.as_str()));
    let shown = shown_categories(&registry, &applicable);
    let resolved = resolve_active_category(active.0.as_ref(), &shown).to_string();
    active.0 = std::borrow::Cow::Owned(resolved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_api_internal::inspector::{InspectorRegistry, seed_default_categories};

    #[test]
    fn applicable_categories_from_present_paths_in_order() {
        let mut r = InspectorRegistry::default();
        seed_default_categories(&mut r);
        // A brush entity: Brush + Transform + a custom component.
        let present = [
            "jackdaw_jsn::types::Brush",
            "bevy_transform::components::transform::Transform",
            "my_game::Health",
        ];
        let got = applicable_categories(&r, present.iter().copied());
        // Strip order: object before mesh before components; only present ones returned.
        assert_eq!(got, ["object", "mesh", "components"]);
    }
}
