use bevy::feathers::controls::FeathersDisclosureToggle;
use bevy::ui::Checked;
use bevy::ui_widgets::ToggleChecked;
use bevy::{feathers::theme::ThemedText, prelude::*};
use jackdaw_widgets::collapsible::{CollapsibleBody, CollapsibleHeader, CollapsibleSection};

use crate::panel_card::DisclosureSection;
use crate::tokens;

/// Spawn a styled collapsible section. Returns `(section_entity, body_entity)`.
pub fn collapsible_section(
    commands: &mut Commands,
    title: &str,
    parent: Entity,
) -> (Entity, Entity) {
    let body = commands
        .spawn((
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::left(Val::Px(tokens::SPACING_MD)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
        ))
        .id();

    let section = commands
        .spawn((
            CollapsibleSection { collapsed: false },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    // Header
    let title_owned = title.to_string();
    let header = commands
        .spawn((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
                column_gap: Val::Px(tokens::SPACING_SM),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG),
            ChildOf(section),
        ))
        .id();

    let disclosure = commands
        .spawn_scene(bsn! { @FeathersDisclosureToggle })
        .insert((ChildOf(header), DisclosureSection(section), Checked))
        .id();

    // Title text
    commands.spawn((
        Text::new(title_owned),
        TextFont {
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        ThemedText,
        ChildOf(header),
    ));

    commands
        .entity(header)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleChecked { entity: disclosure });
        });

    // Hover effect on header
    commands.entity(header).observe(
        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor, With<CollapsibleHeader>>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        },
    );
    commands.entity(header).observe(
        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor, With<CollapsibleHeader>>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = tokens::COMPONENT_CARD_HEADER_BG;
            }
        },
    );

    // Attach body to section
    commands.entity(body).insert(ChildOf(section));

    (section, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct SectionStore(Option<(Entity, Entity)>);

    fn spawn_section(mut commands: Commands, mut store: ResMut<SectionStore>) {
        let parent = commands.spawn(Node::default()).id();
        store.0 = Some(collapsible_section(&mut commands, "Terrain", parent));
    }

    /// The header's chevron is the feathers disclosure toggle, and the
    /// section's open state is that toggle's `Checked`.
    #[test]
    fn the_section_opens_on_a_feathers_disclosure_toggle() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_asset::<Image>()
        .add_observer(crate::panel_card::on_disclosure_change)
        .init_resource::<SectionStore>();

        let system_id = app.world_mut().register_system(spawn_section);
        app.world_mut().run_system(system_id).unwrap();
        app.world_mut().flush();

        let (section, body) = app.world().resource::<SectionStore>().0.expect("spawned");
        let mut disclosures = app
            .world_mut()
            .query_filtered::<(Entity, &DisclosureSection), With<FeathersDisclosureToggle>>();
        let (disclosure, link) = disclosures
            .iter(app.world())
            .next()
            .expect("the header carries a feathers disclosure toggle");
        assert_eq!(link.0, section, "the toggle opens its own section");
        assert!(
            app.world().get::<Checked>(disclosure).is_some(),
            "a section that opens expanded starts checked"
        );

        app.world_mut().trigger(bevy::ui_widgets::ValueChange {
            source: disclosure,
            value: false,
            is_final: true,
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<Node>(body).map(|node| node.display),
            Some(Display::None),
            "collapsing hides the body"
        );
    }
}
