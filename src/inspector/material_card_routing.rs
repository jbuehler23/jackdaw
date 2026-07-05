use bevy::feathers::containers::{pane, pane_body, pane_header};
use bevy::feathers::controls::FeathersDisclosureToggle;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::ToggleChecked;
use jackdaw_feathers::tokens;
use jackdaw_widgets::collapsible::{CollapsibleBody, CollapsibleHeader, CollapsibleSection};

use super::{
    ComponentDisplay, ComponentDisplayBody, ComponentDisplayTypePath, ComponentName, Inspector,
    InspectorDirty, InspectorTarget, component_display::DisclosureSection,
};

/// Resolve which `Handle<StandardMaterial>` to display for a brush entity.
///
/// Priority: selected face (face edit mode + at least one face selected) ->
/// first face that has a non-default material handle -> None.
pub(crate) fn resolve_brush_material_handle(
    world: &World,
    brush_entity: Entity,
) -> Option<Handle<StandardMaterial>> {
    let brush = world.get::<crate::brush::Brush>(brush_entity)?;

    // Try to read the selected faces from BrushSelection.
    let selection = world.get_resource::<crate::brush::BrushSelection>();
    let edit_mode = world
        .get_resource::<crate::brush::EditMode>()
        .copied()
        .unwrap_or(crate::brush::EditMode::Object);

    if edit_mode == crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face)
        && let Some(sel) = selection.and_then(|s| s.active_sub())
        && !sel.faces.is_empty()
    {
        let face_idx = sel.faces[0];
        if let Some(face) = brush.faces.get(face_idx) {
            let h = face.material.clone();
            if h != Handle::default() {
                return Some(h);
            }
        }
    }

    // Fall back to the first face that has an explicit (non-default) material.
    for face in &brush.faces {
        if face.material != Handle::default() {
            return Some(face.material.clone());
        }
    }

    None
}

/// Spawn a material card shell (section + header + body) under
/// `inspector_entity` from feathers panel containers, tagged with the
/// `ComponentDisplay*` markers so the category filter routes it to the Material
/// tab. The header carries a `FeathersDisclosureToggle` that drives collapse
/// through `on_disclosure_change`; the section/header/body keep the
/// `Collapsible*` markers so `persist_inspector_collapse` records the state.
/// Returns the body entity for deferred filling.
pub(crate) fn spawn_material_card_shell(
    commands: &mut Commands,
    inspector_entity: Entity,
    title: &str,
    icon: jackdaw_feathers::icons::Icon,
    type_path: &str,
    icon_font: &Handle<Font>,
    collapsed: bool,
) -> Entity {
    let body_display = if collapsed {
        Display::None
    } else {
        Display::Flex
    };

    // Card frame: a feathers pane holding a header and a body.
    let section = commands
        .spawn_scene(pane())
        .insert((
            ComponentDisplay,
            ComponentName(title.to_string()),
            ComponentDisplayTypePath(type_path.to_string()),
            CollapsibleSection { collapsed },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(inspector_entity),
        ))
        .id();

    let header = commands
        .spawn_scene(pane_header())
        .insert((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Start,
                column_gap: Val::Px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(section),
        ))
        .id();

    // Disclosure toggle: its `Checked` state is the expanded state and it draws
    // the rotating chevron. `ValueChange<bool>` routes through
    // `on_disclosure_change`.
    let mut disclosure = commands.spawn_scene(bsn! { @FeathersDisclosureToggle });
    disclosure.insert((ChildOf(header), DisclosureSection(section)));
    if !collapsed {
        disclosure.insert(Checked);
    }
    let disclosure_entity = disclosure.id();

    // Card icon then title.
    commands.spawn((
        Text::new(String::from(icon.unicode())),
        TextFont {
            font: icon_font.clone().into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(header),
    ));
    commands.spawn((
        Text::new(title.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_DISPLAY_COLOR.into()),
        ChildOf(header),
    ));

    // Clicking anywhere on the header toggles the disclosure, which then emits
    // `ValueChange<bool>` through `on_disclosure_change`.
    commands
        .entity(header)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleChecked {
                entity: disclosure_entity,
            });
        });

    commands
        .spawn_scene(pane_body())
        .insert((
            ComponentDisplayBody,
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                display: body_display,
                ..Default::default()
            },
            ChildOf(section),
        ))
        .id()
}

/// True if a card with `card_type_path` should be refreshed by a trigger for
/// `trigger_type_path`. Exact match, OR a prefix trigger (ending in "::") that
/// the card's path starts with (so `"material_card::"` refreshes every material card).
pub(crate) fn refresh_card_matches(card_type_path: &str, trigger_type_path: &str) -> bool {
    card_type_path == trigger_type_path
        || (trigger_type_path.ends_with("::") && card_type_path.starts_with(trigger_type_path))
}

/// Map a card `type_path` (from `ComponentDisplayTypePath`) to its `MaterialCardKind`.
/// Returns `None` when the path does not belong to a material card.
fn material_card_kind_for(
    type_path: &str,
) -> Option<crate::inspector::material_display::MaterialCardKind> {
    use crate::inspector::material_display::MaterialCardKind;
    MaterialCardKind::ALL
        .into_iter()
        .find(|k| k.type_path() == type_path)
}

/// Targeted single-card refresh. Rebuilds only the BODY of the card(s) whose
/// `ComponentDisplayTypePath` equals `type_path`, for the inspector(s) showing
/// `source`. In-place edits that change just one card's content (e.g. a material
/// apply re-resolving the assigned handle) trigger this instead of the
/// full-panel teardown in `on_inspector_dirty`.
#[derive(Event)]
pub(crate) struct RefreshInspectorCardBody {
    pub(crate) source: Entity,
    pub(crate) type_path: String,
}

pub(crate) fn on_refresh_inspector_card_body(
    refresh: On<RefreshInspectorCardBody>,
    mut commands: Commands,
    inspectors: Query<&InspectorTarget, With<Inspector>>,
    cards: Query<
        (
            &ComponentDisplayTypePath,
            &bevy::ecs::hierarchy::ChildOf,
            &Children,
        ),
        With<ComponentDisplay>,
    >,
    bodies: Query<(), With<ComponentDisplayBody>>,
    children_q: Query<&Children>,
) {
    let source = refresh.source;
    let mut matched = false;
    for (type_path, child_of, card_children) in &cards {
        if !refresh_card_matches(&type_path.0, &refresh.type_path) {
            continue;
        }
        // The card section is a child of its inspector; only refresh cards in an
        // inspector that targets `source`.
        let Ok(target) = inspectors.get(child_of.parent()) else {
            continue;
        };
        if target.0 != source {
            continue;
        }
        let Some(body) = card_children.iter().find(|&child| bodies.contains(child)) else {
            continue;
        };
        matched = true;
        let card_tp = type_path.0.clone();
        let body_children: Vec<Entity> = children_q
            .get(body)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        commands.queue(move |world: &mut World| {
            for child in body_children {
                if let Ok(ec) = world.get_entity_mut(child) {
                    ec.despawn();
                }
            }
            if let Some(kind) = material_card_kind_for(&card_tp) {
                crate::inspector::material_display::fill_material_card_body(
                    world, source, body, kind,
                );
            }
        });
    }

    // No card mounted for this type path (e.g. the inspector has not been built
    // for this selection yet): fall back to a full rebuild so the edit shows.
    if !matched {
        commands.entity(source).insert(InspectorDirty);
    }
}

#[cfg(test)]
mod refresh_card_matches_tests {
    use super::refresh_card_matches;

    #[test]
    fn exact_match_is_true() {
        assert!(refresh_card_matches(
            "material_card::surface",
            "material_card::surface"
        ));
    }

    #[test]
    fn prefix_trigger_matches_card_under_it() {
        assert!(refresh_card_matches(
            "material_card::surface",
            "material_card::"
        ));
    }

    #[test]
    fn prefix_trigger_does_not_match_unrelated_card() {
        assert!(!refresh_card_matches("transform", "material_card::"));
    }

    #[test]
    fn exact_match_settings_card() {
        assert!(refresh_card_matches(
            "material_card::settings",
            "material_card::settings"
        ));
    }
}
