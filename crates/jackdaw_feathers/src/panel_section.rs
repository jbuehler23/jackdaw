use bevy::prelude::*;
use lucide_icons::Icon;

use crate::button::{ButtonClickEvent, ButtonVariant, IconButtonProps, icon_button};
use crate::icons::IconFont;
use crate::tokens::{BORDER_COLOR, TEXT_DISPLAY_COLOR, TEXT_SIZE};

pub fn plugin(app: &mut App) {
    app.add_observer(setup_panel_section_buttons);
}

#[derive(Component)]
pub struct EditorPanelSection;

#[derive(Component)]
struct PanelSectionHeader;

#[derive(Component)]
struct PanelSectionButtonsContainer;

#[derive(Component)]
pub struct PanelSectionAddButton(pub Entity);

#[derive(Component)]
struct PanelSectionCollapseButton(Entity);

#[derive(Component, Default)]
struct Collapsed(bool);

#[derive(Component)]
struct PanelSectionState {
    has_add_button: bool,
    collapsible: bool,
}

#[derive(Default, Clone, Copy)]
pub enum PanelSectionSize {
    #[default]
    MD,
    XL,
}

impl PanelSectionSize {
    fn padding(&self) -> UiRect {
        match self {
            Self::MD => UiRect::all(px(12)),
            Self::XL => UiRect::axes(px(24), px(14)),
        }
    }
}

#[derive(Default)]
pub struct PanelSectionProps {
    pub title: String,
    pub size: PanelSectionSize,
    pub has_add_button: bool,
    pub collapsible: bool,
}

impl PanelSectionProps {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..default()
        }
    }

    pub fn with_size(mut self, size: PanelSectionSize) -> Self {
        self.size = size;
        self
    }

    pub fn with_add_button(mut self) -> Self {
        self.has_add_button = true;
        self
    }

    pub fn collapsible(mut self) -> Self {
        self.collapsible = true;
        self
    }
}

pub fn panel_section(props: PanelSectionProps, editor_font: &Handle<Font>) -> impl Bundle {
    let PanelSectionProps {
        title,
        size,
        has_add_button,
        collapsible,
    } = props;

    (
        EditorPanelSection,
        Collapsed::default(),
        Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
            padding: size.padding(),
            border: UiRect::bottom(px(1)),
            ..default()
        },
        BorderColor::all(BORDER_COLOR),
        PanelSectionState {
            has_add_button,
            collapsible,
        },
        children![(
            PanelSectionHeader,
            Node {
                width: percent(100),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            },
            children![
                (
                    Text::new(title),
                    TextFont {
                        font: editor_font.clone().into(),
                        font_size: TEXT_SIZE,
                        weight: FontWeight::SEMIBOLD,
                        ..default()
                    },
                    TextColor(TEXT_DISPLAY_COLOR.into()),
                ),
                (
                    PanelSectionButtonsContainer,
                    Node {
                        align_items: AlignItems::Center,
                        ..default()
                    },
                ),
            ],
        )],
    )
}

/// The header's button row is the last part of a section to spawn, so the
/// buttons are put in it as it arrives, two levels under the section.
fn setup_panel_section_buttons(
    add: On<Add<PanelSectionButtonsContainer>>,
    mut commands: Commands,
    icon_font: Res<IconFont>,
    parents: Query<&ChildOf>,
    sections: Query<&PanelSectionState, With<EditorPanelSection>>,
) {
    let container_entity = add.entity;
    let Ok(section_entity) = parents
        .get(container_entity)
        .and_then(|header| parents.get(header.parent()))
        .map(ChildOf::parent)
    else {
        return;
    };
    let Ok(state) = sections.get(section_entity) else {
        return;
    };

    if state.has_add_button {
        let add_entity = commands
            .spawn((
                PanelSectionAddButton(section_entity),
                icon_button(
                    IconButtonProps::new(Icon::Plus).variant(ButtonVariant::Ghost),
                    &icon_font.0,
                ),
            ))
            .observe(on_add_click)
            .id();
        crate::utils::attach_or_despawn(&mut commands, container_entity, add_entity);
    }

    if state.collapsible {
        let collapse_entity = commands
            .spawn((
                PanelSectionCollapseButton(section_entity),
                UiTransform {
                    rotation: Rot2::degrees(180.0),
                    ..default()
                },
                icon_button(
                    IconButtonProps::new(Icon::ChevronDown).variant(ButtonVariant::Ghost),
                    &icon_font.0,
                ),
            ))
            .observe(on_collapse_click)
            .id();
        crate::utils::attach_or_despawn(&mut commands, container_entity, collapse_entity);
    }
}

fn on_add_click(
    event: On<ButtonClickEvent>,
    add_buttons: Query<&PanelSectionAddButton>,
    mut commands: Commands,
) {
    let Ok(add_button) = add_buttons.get(event.entity) else {
        return;
    };
    commands.trigger(ButtonClickEvent {
        entity: add_button.0,
    });
}

fn on_collapse_click(
    event: On<ButtonClickEvent>,
    collapse_buttons: Query<&PanelSectionCollapseButton>,
    mut sections: Query<(&mut Collapsed, &Children), With<EditorPanelSection>>,
    mut nodes: Query<&mut Node, Without<PanelSectionHeader>>,
    headers: Query<Entity, With<PanelSectionHeader>>,
    mut button_transforms: Query<&mut UiTransform>,
) {
    let button_entity = event.entity;
    let Ok(collapse_button) = collapse_buttons.get(button_entity) else {
        return;
    };

    let Ok((mut collapsed, section_children)) = sections.get_mut(collapse_button.0) else {
        return;
    };

    collapsed.0 = !collapsed.0;

    for child in section_children.iter() {
        if headers.get(child).is_ok() {
            continue;
        }
        if let Ok(mut node) = nodes.get_mut(child) {
            node.display = if collapsed.0 {
                Display::None
            } else {
                Display::Flex
            };
        }
    }

    if let Ok(mut transform) = button_transforms.get_mut(button_entity) {
        transform.rotation = if collapsed.0 {
            Rot2::degrees(0.0)
        } else {
            Rot2::degrees(180.0)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::EditorFont;

    /// A section's header buttons are there on the frame it is spawned.
    #[test]
    fn a_section_has_its_header_buttons_on_the_frame_it_spawns() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            crate::button::plugin,
            plugin,
        ));
        app.init_asset::<Font>();
        app.insert_resource(IconFont(Handle::default()));
        app.insert_resource(EditorFont(Handle::default()));

        let section = app
            .world_mut()
            .spawn(panel_section(
                PanelSectionProps::new("Layers")
                    .with_add_button()
                    .collapsible(),
                &Handle::default(),
            ))
            .id();
        app.world_mut().flush();

        let world = app.world_mut();
        let mut adds = world.query::<(&PanelSectionAddButton, &ChildOf)>();
        let (add, row) = adds
            .single(world)
            .map(|(add, parent)| (add.0, parent.parent()))
            .expect("the add button is there");
        assert_eq!(add, section);
        let mut collapses = world.query::<(&PanelSectionCollapseButton, &ChildOf)>();
        let (collapse, collapse_row) = collapses
            .single(world)
            .map(|(collapse, parent)| (collapse.0, parent.parent()))
            .expect("and so is the collapse button");
        assert_eq!(collapse, section);
        assert_eq!(collapse_row, row);
        assert!(
            world.get::<PanelSectionButtonsContainer>(row).is_some(),
            "both in the header's button row",
        );
    }
}
