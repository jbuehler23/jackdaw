use bevy::prelude::*;
use jackdaw_widgets::collapsible::{
    CollapsibleBody, CollapsibleHeader, CollapsibleSection, ToggleCollapsible,
};
use lucide_icons::Icon;

use crate::tokens;

/// Options for an inspector card.
#[derive(Clone, Default)]
pub struct InspectorCardOpts {
    /// Optional header icon shown before the title.
    pub icon: Option<Icon>,
    /// Show a remove (X) button in the header. The caller observes clicks on
    /// the returned remove-button entity via the `InspectorCardRemoveButton` marker.
    pub removable: bool,
    /// Enable chevron and collapse behavior.
    pub collapsible: bool,
    /// Initial collapsed state. Defaults to `false` (expanded) so existing
    /// callers that do not set this field are unaffected.
    pub collapsed: bool,
}

/// Entities a spawned inspector card exposes to the caller.
pub struct InspectorCardEntities {
    pub section: Entity,
    pub header: Entity,
    pub body: Entity,
    /// Present when `opts.removable`; the caller wires the remove action.
    pub remove_button: Option<Entity>,
}

/// Marker on the remove (X) button so a consumer can observe its clicks.
#[derive(Component)]
pub struct InspectorCardRemoveButton;

/// Spawn a standard inspector card under `parent` and return its entities.
/// The header shows `title` (and `opts.icon` if set) plus an optional remove
/// button; the caller fills `body`. All colors and radii come from `tokens`.
pub fn spawn_inspector_card(
    commands: &mut Commands,
    parent: Entity,
    title: &str,
    icon_font: &Handle<Font>,
    opts: InspectorCardOpts,
) -> InspectorCardEntities {
    let font = icon_font.clone();

    let body_display = if opts.collapsed {
        Display::None
    } else {
        Display::Flex
    };

    let body = commands
        .spawn((
            CollapsibleBody,
            Node {
                padding: UiRect::new(
                    Val::Px(tokens::SPACING_MD),
                    Val::Px(tokens::SPACING_SM),
                    Val::Px(tokens::SPACING_XS),
                    Val::Px(tokens::SPACING_XS),
                ),
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                display: body_display,
                ..Default::default()
            },
        ))
        .id();

    let section = commands
        .spawn((
            CollapsibleSection {
                collapsed: opts.collapsed,
            },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_BG),
            BorderColor::all(tokens::COMPONENT_CARD_BORDER),
            BoxShadow(vec![ShadowStyle {
                x_offset: Val::ZERO,
                y_offset: Val::ZERO,
                blur_radius: Val::Px(1.0),
                spread_radius: Val::ZERO,
                color: tokens::SHADOW_COLOR,
            }]),
            ChildOf(parent),
        ))
        .id();

    let header = commands
        .spawn((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                column_gap: Val::Px(tokens::SPACING_SM),
                border_radius: BorderRadius::top(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG),
            ChildOf(section),
        ))
        .id();

    // Toggle area (chevron + optional icon + title).
    let toggle_area = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                flex_grow: 1.0,
                ..Default::default()
            },
            ChildOf(header),
        ))
        .id();

    if opts.collapsible {
        commands.spawn((
            Text::new(String::from(Icon::ChevronDown.unicode())),
            TextFont {
                font: font.clone(),
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(toggle_area),
        ));
    }

    if let Some(header_icon) = opts.icon {
        commands.spawn((
            Text::new(String::from(header_icon.unicode())),
            TextFont {
                font: font.clone(),
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(toggle_area),
        ));
    }

    commands.spawn((
        Text::new(title.to_string()),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_DISPLAY_COLOR.into()),
        ChildOf(toggle_area),
    ));

    if opts.collapsible {
        let section_entity = section;
        commands.entity(toggle_area).observe(
            move |_: On<Pointer<Click>>, mut commands: Commands| {
                commands.trigger(ToggleCollapsible {
                    entity: section_entity,
                });
            },
        );
    }

    // Hover effects on header.
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

    let remove_button = if opts.removable {
        let btn = commands
            .spawn((
                Text::new(String::from(Icon::X.unicode())),
                TextFont {
                    font: font.clone(),
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                InspectorCardRemoveButton,
                ChildOf(header),
            ))
            .id();
        Some(btn)
    } else {
        None
    };

    commands.entity(body).insert(ChildOf(section));

    InspectorCardEntities {
        section,
        header,
        body,
        remove_button,
    }
}

/// Spawn a labeled field row under `body`. Returns the slot entity for
/// the caller's input widget. The label uses a fixed min-width and
/// `TEXT_SECONDARY` color for cross-category consistency.
pub fn spawn_inspector_field_row(commands: &mut Commands, body: Entity, label: &str) -> Entity {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(body),
        ))
        .id();

    commands.spawn((
        Text::new(label.to_string()),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: Val::Px(64.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(row),
    ));

    commands
        .spawn((
            Node {
                flex_grow: 1.0,
                ..Default::default()
            },
            ChildOf(row),
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captures the spawned entities so the test body can inspect them.
    #[derive(Resource, Default)]
    struct CardStore(Option<InspectorCardResult>);

    struct InspectorCardResult {
        section: Entity,
        header: Entity,
        body: Entity,
        remove_button: Option<Entity>,
    }

    fn spawn_card_system(mut commands: Commands, mut store: ResMut<CardStore>) {
        let parent = commands.spawn(Node::default()).id();
        // A dummy font handle; no assets plugin is loaded in headless tests
        // so the handle is invalid, but entity structure is all we verify.
        let font: Handle<Font> = Handle::default();
        let ents = spawn_inspector_card(
            &mut commands,
            parent,
            "Test",
            &font,
            InspectorCardOpts {
                removable: true,
                collapsible: true,
                icon: Some(Icon::Move3d),
                collapsed: false,
            },
        );
        store.0 = Some(InspectorCardResult {
            section: ents.section,
            header: ents.header,
            body: ents.body,
            remove_button: ents.remove_button,
        });
    }

    #[test]
    fn spawn_card_yields_section_header_body_with_markers() {
        let mut app = App::new();
        app.init_resource::<CardStore>();

        // Register as a one-shot system and run it once, then flush.
        let system_id = app.world_mut().register_system(spawn_card_system);
        app.world_mut().run_system(system_id).unwrap();
        app.world_mut().flush();

        let store = app.world().resource::<CardStore>();
        let result = store.0.as_ref().expect("system must have stored entities");

        // Section must carry CollapsibleSection.
        assert!(
            app.world()
                .get::<CollapsibleSection>(result.section)
                .is_some(),
            "section entity must have CollapsibleSection"
        );

        // Body must carry CollapsibleBody and a Node.
        assert!(
            app.world().get::<CollapsibleBody>(result.body).is_some(),
            "body entity must have CollapsibleBody"
        );
        assert!(
            app.world().get::<Node>(result.body).is_some(),
            "body entity must have Node"
        );

        // Header must carry CollapsibleHeader.
        assert!(
            app.world()
                .get::<CollapsibleHeader>(result.header)
                .is_some(),
            "header entity must have CollapsibleHeader"
        );

        // Remove button must exist and carry the marker.
        let remove = result
            .remove_button
            .expect("remove_button must be Some when removable");
        assert!(
            app.world()
                .get::<InspectorCardRemoveButton>(remove)
                .is_some(),
            "remove button entity must have InspectorCardRemoveButton"
        );
    }
}
