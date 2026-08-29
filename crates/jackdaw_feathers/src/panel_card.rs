//! A collapsible titled card: native feathers pane, a disclosure toggle that
//! owns the collapsed state, an icon and a title.
//!
//! The grouping shape for stacked fields. A surface that needs its own markers
//! on the card inserts them on the returned entities.

use std::borrow::Cow;
use std::collections::HashMap;

use bevy::feathers::containers::{pane, pane_body, pane_header};
use bevy::feathers::controls::FeathersDisclosureToggle;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{ToggleChecked, ValueChange};
use jackdaw_widgets::collapsible::{CollapsibleBody, CollapsibleHeader, CollapsibleSection};
use lucide_icons::Icon;

use crate::tokens;

pub fn plugin(app: &mut App) {
    app.init_resource::<PanelCardCollapseState>()
        .add_observer(on_disclosure_change);
}

/// Which keyed cards the user has closed, so a panel that rebuilds does
/// not reopen what was shut.
///
/// Keyed by a caller-supplied string rather than by entity, because the
/// entity is exactly what a rebuild destroys. A card with no key is not
/// recorded and always opens at its default.
///
/// The inspector keeps its own equivalent, `InspectorCollapseState`, keyed by
/// card title and scoped to the inspector's rebuild pass.
#[derive(Resource, Default)]
pub struct PanelCardCollapseState(pub HashMap<String, bool>);

impl PanelCardCollapseState {
    /// What `key` was last left at, or `default` if it has never been
    /// toggled.
    pub fn collapsed(&self, key: &str, default: bool) -> bool {
        self.0.get(key).copied().unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, collapsed: bool) {
        self.0.insert(key.to_string(), collapsed);
    }
}

/// The key a card records its collapsed state under. Absent on cards that
/// do not want to be remembered.
#[derive(Component)]
pub struct PanelCardKey(pub Cow<'static, str>);

/// Links a disclosure toggle to the section it expands. Spawned on the
/// toggle by [`spawn_panel_card`] and by any other card that wants the
/// same collapse behaviour.
#[derive(Component)]
pub struct DisclosureSection(pub Entity);

/// Entities a spawned card exposes.
pub struct PanelCard {
    pub section: Entity,
    pub header: Entity,
    /// Where the caller stacks its rows.
    pub body: Entity,
    pub disclosure: Entity,
}

#[derive(Clone)]
pub struct PanelCardProps {
    pub title: String,
    pub icon: Option<Icon>,
    /// How this card opens the first time. A keyed card that has been
    /// toggled since uses what the user left it at instead.
    pub default_collapsed: bool,
    /// Key to remember the collapsed state under, across rebuilds.
    pub key: Option<Cow<'static, str>>,
}

impl PanelCardProps {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            icon: None,
            default_collapsed: false,
            key: None,
        }
    }

    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// State how this card opens. Not derived from the title: two surfaces
    /// may open the same section differently.
    pub fn default_collapsed(mut self, collapsed: bool) -> Self {
        self.default_collapsed = collapsed;
        self
    }

    pub fn remembered_as(mut self, key: impl Into<Cow<'static, str>>) -> Self {
        self.key = Some(key.into());
        self
    }
}

pub fn spawn_panel_card(
    commands: &mut Commands,
    parent: Entity,
    props: PanelCardProps,
    icon_font: &Handle<Font>,
    collapse: &PanelCardCollapseState,
) -> PanelCard {
    let collapsed = match &props.key {
        Some(key) => collapse.collapsed(key, props.default_collapsed),
        None => props.default_collapsed,
    };
    let body_display = if collapsed {
        Display::None
    } else {
        Display::Flex
    };

    let section = commands
        .spawn_scene(pane())
        .insert((
            CollapsibleSection { collapsed },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
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

    if let Some(key) = props.key {
        commands.entity(section).insert(PanelCardKey(key));
    }

    let mut disclosure = commands.spawn_scene(bsn! { @FeathersDisclosureToggle });
    disclosure.insert((ChildOf(header), DisclosureSection(section)));
    if !collapsed {
        disclosure.insert(Checked);
    }
    let disclosure = disclosure.id();

    if let Some(icon) = props.icon {
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
    }
    commands.spawn((
        Text::new(props.title),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_DISPLAY_COLOR.into()),
        ChildOf(header),
    ));

    commands
        .entity(header)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleChecked { entity: disclosure });
        });

    let body = commands
        .spawn_scene(pane_body())
        .insert((
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: Val::Px(tokens::SPACING_XS),
                display: body_display,
                ..Default::default()
            },
            ChildOf(section),
        ))
        .id();

    PanelCard {
        section,
        header,
        body,
        disclosure,
    }
}

/// Drive the section's collapsed flag and body visibility from the
/// disclosure toggle's checked state. `value` is the expanded state, so
/// `collapsed = !value`. The toggle does not self-manage `Checked`; set it
/// here so the chevron rotates.
pub fn on_disclosure_change(
    change: On<ValueChange<bool>>,
    toggles: Query<&DisclosureSection>,
    mut sections: Query<(&mut CollapsibleSection, &Children, Option<&PanelCardKey>)>,
    mut bodies: Query<&mut Node, With<CollapsibleBody>>,
    collapse: Option<ResMut<PanelCardCollapseState>>,
    mut commands: Commands,
) {
    let toggle = change.source;
    let Ok(link) = toggles.get(toggle) else {
        return;
    };
    let expanded = change.value;

    if expanded {
        commands.entity(toggle).insert(Checked);
    } else {
        commands.entity(toggle).remove::<Checked>();
    }

    let Ok((mut section, children, key)) = sections.get_mut(link.0) else {
        return;
    };
    section.collapsed = !expanded;
    if let (Some(key), Some(mut collapse)) = (key, collapse) {
        collapse.set(&key.0, !expanded);
    }

    for child in children.iter() {
        if let Ok(mut node) = bodies.get_mut(child) {
            node.display = if expanded {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card the user has never touched opens the way its surface asked
    /// for; one they have closed stays closed through the next rebuild,
    /// which the string key rather than an entity key makes possible.
    #[test]
    fn a_toggled_card_outlives_the_entity_it_was_toggled_on() {
        let mut state = PanelCardCollapseState::default();
        assert!(!state.collapsed("materials.window.surface", false));
        assert!(state.collapsed("materials.window.textures", true));

        state.set("materials.window.textures", false);
        assert!(
            !state.collapsed("materials.window.textures", true),
            "a card opened by hand must not snap shut on the next rebuild",
        );

        state.set("materials.window.surface", true);
        assert!(state.collapsed("materials.window.surface", false));
    }

    /// Two surfaces may show the same section and open it differently, so
    /// nothing keyed off the title decides this.
    #[test]
    fn surfaces_keep_their_own_default_for_the_same_section_name() {
        let state = PanelCardCollapseState::default();
        assert!(!state.collapsed("materials.window.textures", false));
        assert!(state.collapsed("terrain.textures.textures", true));
    }
}
