use bevy_app::prelude::*;
use bevy_color::palettes::tailwind;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;

use crate::icons::EditorFont;
use crate::tokens::{CORNER_RADIUS, TEXT_SIZE};

#[derive(Component)]
pub struct EditorAlert;

#[derive(Default, Clone, Copy)]
pub enum AlertVariant {
    #[default]
    Info,
    Warning,
    Important,
}

impl AlertVariant {
    fn border_color(&self) -> Srgba {
        match self {
            Self::Info => tailwind::BLUE_500,
            Self::Warning => tailwind::YELLOW_500,
            Self::Important => tailwind::VIOLET_500,
        }
    }

    fn bg_color(&self) -> Color {
        match self {
            Self::Info => tailwind::BLUE_500.with_alpha(0.1).into(),
            Self::Warning => tailwind::YELLOW_500.with_alpha(0.1).into(),
            Self::Important => tailwind::VIOLET_500.with_alpha(0.1).into(),
        }
    }

    fn text_color(&self) -> Srgba {
        match self {
            Self::Info => tailwind::BLUE_400,
            Self::Warning => tailwind::YELLOW_400,
            Self::Important => tailwind::VIOLET_400,
        }
    }
}

const TEXT_ALPHA: f32 = 0.8;
const BOLD_ALPHA: f32 = 1.0;

#[derive(Clone)]
pub enum AlertSpan {
    Text(String),
    Bold(String),
}

#[derive(Component)]
struct AlertConfig {
    variant: AlertVariant,
    spans: Vec<AlertSpan>,
}

pub fn plugin(app: &mut App) {
    app.add_systems(Update, setup_alert);
}

pub fn alert(variant: AlertVariant, spans: Vec<AlertSpan>) -> impl Bundle {
    (
        EditorAlert,
        AlertConfig {
            variant,
            spans: spans.clone(),
        },
        Node {
            width: percent(100),
            padding: UiRect::all(px(12.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(CORNER_RADIUS),
            ..default()
        },
        BackgroundColor(variant.bg_color()),
        BorderColor::all(variant.border_color()),
    )
}

fn setup_alert(
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    alerts: Query<(Entity, &AlertConfig), Added<AlertConfig>>,
) {
    let font = editor_font.0.clone();

    for (entity, config) in &alerts {
        let text_color = config.variant.text_color();

        let Some(first) = config.spans.first() else {
            continue;
        };

        let (first_text, first_weight, first_alpha) = span_props(first);
        let text_id = commands
            .spawn((
                Text::new(first_text),
                TextFont {
                    font: font.clone(),
                    font_size: TEXT_SIZE,
                    weight: first_weight,
                    ..default()
                },
                TextColor(text_color.with_alpha(first_alpha).into()),
            ))
            .id();

        for span in config.spans.iter().skip(1) {
            let (text, weight, alpha) = span_props(span);
            let span_id = commands
                .spawn((
                    TextSpan::new(text),
                    TextFont {
                        font: font.clone(),
                        font_size: TEXT_SIZE,
                        weight,
                        ..default()
                    },
                    TextColor(text_color.with_alpha(alpha).into()),
                ))
                .id();
            commands.entity(text_id).add_child(span_id);
        }

        crate::utils::attach_or_despawn(&mut commands, entity, text_id);
    }
}

fn span_props(span: &AlertSpan) -> (&str, FontWeight, f32) {
    match span {
        AlertSpan::Text(t) => (t.as_str(), FontWeight::NORMAL, TEXT_ALPHA),
        AlertSpan::Bold(t) => (t.as_str(), FontWeight::MEDIUM, BOLD_ALPHA),
    }
}
