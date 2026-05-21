use std::time::Duration;

use bevy_app::prelude::*;
use bevy_asset::prelude::*;
use bevy_color::palettes::tailwind;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_text::prelude::*;
use bevy_time::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;
use lucide_icons::Icon;

use crate::button::{ButtonClickEvent, ButtonVariant, IconButtonProps, icon_button};
use crate::separator::{SeparatorProps, separator};
use crate::tokens::{CORNER_RADIUS, TEXT_BODY_COLOR, TEXT_SIZE};

pub const TOAST_BOTTOM_OFFSET: f32 = 12.0;
pub const DEFAULT_TOAST_DURATION: Duration = Duration::from_millis(3000);

pub fn plugin(app: &mut App) {
    app.add_observer(handle_toast_close)
        .add_systems(Update, tick_toast_duration);
}

#[derive(Component)]
pub struct EditorToast;

#[derive(Component, Default, Clone, Copy)]
pub enum ToastVariant {
    #[default]
    Info,
    Success,
    Error,
}

impl ToastVariant {
    pub fn bg_color(&self) -> Srgba {
        match self {
            Self::Info => tailwind::ZINC_700,
            Self::Success => tailwind::GREEN_800,
            Self::Error => tailwind::RED_800,
        }
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::Info => Icon::Info,
            Self::Success => Icon::CircleCheck,
            Self::Error => Icon::CircleX,
        }
    }
}

#[derive(Component)]
pub struct ToastDuration(pub Timer);

pub fn toast(
    variant: ToastVariant,
    content: impl Into<String>,
    duration: Duration,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) -> impl Bundle {
    (
        EditorToast,
        variant,
        Interaction::None,
        ToastDuration(Timer::new(duration, TimerMode::Once)),
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            bottom: px(TOAST_BOTTOM_OFFSET),
            column_gap: px(12),
            padding: UiRect::axes(px(12), px(6)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(CORNER_RADIUS),
            box_sizing: BoxSizing::BorderBox,
            align_items: AlignItems::Center,
            ..default()
        },
        UiTransform {
            translation: Val2 {
                x: percent(-50),
                y: px(0),
            },
            ..default()
        },
        BackgroundColor(variant.bg_color().into()),
        BorderColor::all(TEXT_BODY_COLOR.with_alpha(0.15)),
        children![
            (
                Text::new(variant.icon().unicode()),
                TextFont {
                    font: icon_font.clone(),
                    font_size: 18.0,
                    ..default()
                },
                TextColor(TEXT_BODY_COLOR.into()),
            ),
            (
                Text::new(content),
                TextFont {
                    font: editor_font.clone(),
                    font_size: TEXT_SIZE,
                    ..default()
                },
                TextColor(TEXT_BODY_COLOR.into()),
            ),
            (
                Node {
                    column_gap: px(6),
                    align_items: AlignItems::Center,
                    ..default()
                },
                children![
                    separator(SeparatorProps::vertical()),
                    icon_button(
                        IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                        icon_font,
                    ),
                ],
            ),
        ],
    )
}

fn handle_toast_close(
    trigger: On<ButtonClickEvent>,
    parents: Query<&ChildOf>,
    toasts: Query<Entity, With<EditorToast>>,
    mut commands: Commands,
) {
    // Check if the clicked button is inside a toast
    let mut current = trigger.entity;
    loop {
        if toasts.get(current).is_ok() {
            commands.entity(current).try_despawn();
            return;
        }
        let Ok(child_of) = parents.get(current) else {
            return;
        };
        current = child_of.parent();
    }
}

fn tick_toast_duration(
    mut commands: Commands,
    time: Res<Time>,
    mut toasts: Query<(Entity, &mut ToastDuration), With<EditorToast>>,
) {
    for (entity, mut duration) in &mut toasts {
        duration.0.tick(time.delta());
        if duration.0.is_finished() {
            commands.entity(entity).try_despawn();
        }
    }
}
