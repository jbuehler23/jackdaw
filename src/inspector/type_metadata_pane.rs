//! Type-settings sub-pane toggled from an inspector card header.

use bevy::feathers::controls::{ButtonVariant, FeathersToolButton};
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ToggleChecked};
use jackdaw_feathers::icons::{Icon, icon_scene};
use jackdaw_feathers::text_edit::{self, TextEditCommitEvent, TextEditProps};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::utils::find_ancestor;
use jackdaw_widgets::collapsible::CollapsibleSection;

use crate::project::ProjectRoot;
use crate::type_metadata::set_category;

use super::component_display::{ComponentDisplayCard, DisclosureSection};

#[derive(Component)]
struct TypeMetadataToggle;

#[derive(Component)]
struct TypeMetadataPane;

#[derive(Component)]
struct TypeMetadataCategoryInput {
    type_path: String,
}

pub(super) fn plugin(app: &mut App) {
    app.add_observer(on_category_commit);
}

pub(crate) fn spawn_type_metadata_ui(
    commands: &mut Commands,
    card: &ComponentDisplayCard,
    type_path: &str,
    category: &str,
) {
    let display = Display::None;
    commands
        .spawn_scene(bsn! {
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                display: {display},
                flex_shrink: 0.0,
                padding: UiRect::all(px(tokens::SPACING_MD)),
                row_gap: px(tokens::SPACING_SM),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(tokens::COMPONENT_CARD_RADIUS)),
                margin: UiRect::bottom(px(tokens::SPACING_SM)),
            }
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG)
            BorderColor::all(tokens::COMPONENT_CARD_BORDER)
        })
        .insert((TypeMetadataPane, ChildOf(card.body)))
        .with_child((
            TypeMetadataCategoryInput {
                type_path: type_path.to_string(),
            },
            text_edit::text_edit(
                TextEditProps::default()
                    .with_label("Category")
                    .with_placeholder("Actor, Gameplay, ...")
                    .with_default_value(category.to_string())
                    .allow_empty(),
            ),
        ));

    commands
        .spawn_scene(type_metadata_toggle_button())
        .insert((ChildOf(card.header), TypeMetadataToggle))
        .observe(on_toggle_type_metadata);
}

fn type_metadata_toggle_button() -> impl Scene {
    let glyph = String::from(Icon::Ellipsis.unicode());
    bsn! {
        @FeathersToolButton {
            @caption: bsn! { icon_scene(glyph, tokens::TEXT_SIZE_SM_PX) },
            @variant: {ButtonVariant::Plain}
        }
        Tooltip::title("Type settings")
    }
}

fn on_toggle_type_metadata(
    activate: On<Activate>,
    mut commands: Commands,
    toggles: Query<&TypeMetadataToggle>,
    child_of: Query<&ChildOf>,
    children: Query<&Children>,
    sections: Query<&CollapsibleSection>,
    disclosures: Query<(), With<DisclosureSection>>,
    mut panes: Query<&mut Node, With<TypeMetadataPane>>,
) {
    if find_ancestor(activate.event_target(), &toggles, &child_of).is_none() {
        return;
    }
    let Some((section_entity, section)) =
        find_ancestor(activate.event_target(), &sections, &child_of)
    else {
        return;
    };
    let collapsed = section.collapsed;
    let Some(pane_entity) = children
        .iter_descendants(section_entity)
        .find(|&e| panes.contains(e))
    else {
        return;
    };
    let Ok(mut node) = panes.get_mut(pane_entity) else {
        return;
    };
    let pane_open = node.display != Display::None;

    if !collapsed && pane_open {
        node.display = Display::None;
        return;
    }
    if collapsed
        && let Some(disclosure) = children
            .iter_descendants(section_entity)
            .find(|&e| disclosures.contains(e))
    {
        commands.trigger(ToggleChecked { entity: disclosure });
    }
    node.display = Display::Flex;
}

fn on_category_commit(
    event: On<TextEditCommitEvent>,
    inputs: Query<&TypeMetadataCategoryInput>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some((_, input)) = find_ancestor(event.entity, &inputs, &child_of) else {
        return;
    };
    let type_path = input.type_path.clone();
    let category = event.text.trim().to_string();
    commands.queue(move |world: &mut World| {
        let Some(root) = world.get_resource::<ProjectRoot>().map(|p| p.root.clone()) else {
            return;
        };
        if let Err(err) = set_category(world, &root, &type_path, &category) {
            warn!("failed to write type category to jackdaw_metadata.bsn: {err}");
        }
    });
}
