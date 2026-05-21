use bevy_app::prelude::*;
use bevy_asset::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_picking::hover::Hovered;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;
use lucide_icons::Icon;

use crate::tokens::{BORDER_COLOR, TEXT_BODY_COLOR, TEXT_SIZE};

#[derive(Event)]
pub struct CheckboxCommitEvent {
    pub entity: Entity,
    pub checked: bool,
}

pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            handle_checkbox_hover,
            handle_checkbox_click,
            sync_checkbox_icon,
        ),
    );
}

#[derive(Component)]
pub struct EditorCheckbox;

#[derive(Component, Default)]
pub struct CheckboxState {
    pub checked: bool,
}

#[derive(Component)]
struct CheckboxIconMarker;

#[derive(Component)]
struct CheckboxBox;

#[derive(Default)]
pub struct CheckboxProps {
    pub label: String,
    pub checked: bool,
}

impl CheckboxProps {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..default()
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

pub fn checkbox(
    props: CheckboxProps,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) -> impl Bundle {
    let CheckboxProps { label, checked } = props;
    let icon_display = if checked {
        Display::Flex
    } else {
        Display::None
    };

    (
        EditorCheckbox,
        CheckboxState { checked },
        Button,
        Hovered::default(),
        Node {
            align_items: AlignItems::Center,
            column_gap: px(6),
            ..default()
        },
        children![
            (
                CheckboxBox,
                Node {
                    width: px(16),
                    height: px(16),
                    border: UiRect::all(px(1.0)),
                    border_radius: BorderRadius::all(px(2.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BorderColor::all(BORDER_COLOR),
                children![(
                    CheckboxIconMarker,
                    Text::new(Icon::Check.unicode()),
                    TextFont {
                        font: icon_font.clone(),
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(TEXT_BODY_COLOR.into()),
                    Node {
                        display: icon_display,
                        ..default()
                    },
                )],
            ),
            (
                Text::new(label),
                TextFont {
                    font: editor_font.clone(),
                    font_size: TEXT_SIZE,
                    ..default()
                },
                TextColor(TEXT_BODY_COLOR.into()),
            ),
        ],
    )
}

fn handle_checkbox_hover(
    checkboxes: Query<(&Hovered, &Children), (Changed<Hovered>, With<EditorCheckbox>)>,
    mut boxes: Query<&mut BorderColor, With<CheckboxBox>>,
) {
    for (hovered, children) in &checkboxes {
        let Some(box_entity) = children.iter().next() else {
            continue;
        };

        let Ok(mut border_color) = boxes.get_mut(box_entity) else {
            continue;
        };

        let border = if hovered.get() {
            BORDER_COLOR.lighter(0.05)
        } else {
            BORDER_COLOR
        };
        *border_color = BorderColor::all(border);
    }
}

fn handle_checkbox_click(
    mut commands: Commands,
    mut checkboxes: Query<
        (Entity, &Interaction, &mut CheckboxState, &Children),
        (Changed<Interaction>, With<EditorCheckbox>),
    >,
    boxes: Query<&Children, With<CheckboxBox>>,
    mut icons: Query<&mut Node, With<CheckboxIconMarker>>,
) {
    for (checkbox_entity, interaction, mut state, checkbox_children) in &mut checkboxes {
        if *interaction != Interaction::Pressed {
            continue;
        }

        state.checked = !state.checked;

        commands.trigger(CheckboxCommitEvent {
            entity: checkbox_entity,
            checked: state.checked,
        });

        let Some(box_entity) = checkbox_children.iter().next() else {
            continue;
        };

        let Ok(box_children) = boxes.get(box_entity) else {
            continue;
        };

        let Some(icon_entity) = box_children.iter().next() else {
            continue;
        };

        let Ok(mut icon_node) = icons.get_mut(icon_entity) else {
            continue;
        };

        icon_node.display = if state.checked {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn sync_checkbox_icon(
    checkboxes: Query<(&CheckboxState, &Children), Changed<CheckboxState>>,
    boxes: Query<&Children, With<CheckboxBox>>,
    mut icons: Query<&mut Node, With<CheckboxIconMarker>>,
) {
    for (state, checkbox_children) in &checkboxes {
        let Some(box_entity) = checkbox_children.iter().next() else {
            continue;
        };

        let Ok(box_children) = boxes.get(box_entity) else {
            continue;
        };

        let Some(icon_entity) = box_children.iter().next() else {
            continue;
        };

        let Ok(mut icon_node) = icons.get_mut(icon_entity) else {
            continue;
        };

        icon_node.display = if state.checked {
            Display::Flex
        } else {
            Display::None
        };
    }
}
