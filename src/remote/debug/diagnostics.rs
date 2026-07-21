//! Diagnostics view: live fps, frame time, and entity count read from the
//! running game over BRP (`jackdaw/diagnostics`). Each metric is shown as a
//! value readout with a sparkline of its recent history. The poll helper writes
//! replies into `DiagnosticsSample`; the update system pushes the newest value
//! into a `RingBuffer` per metric and refreshes the cards.

use bevy::prelude::*;
use serde::Deserialize;

use jackdaw_feathers::tokens;

use super::super::ConnectionManager;
use super::sparkline::{RingBuffer, SPARKLINE_WINDOW, SparklineMaterial};

/// One reply from the `jackdaw/diagnostics` BRP method. `fps` and
/// `frame_time_ms` are null until the game has smoothed enough frames.
#[derive(Resource, Deserialize, Default, Clone)]
pub struct DiagnosticsSample {
    pub fps: Option<f64>,
    pub frame_time_ms: Option<f64>,
    pub entity_count: u64,
}

/// Which stat a card tracks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Fps,
    FrameTime,
    EntityCount,
}

/// Recent history for each metric, plotted by the cards' sparklines.
#[derive(Resource)]
pub struct DiagBuffers {
    fps: RingBuffer,
    frame_time_ms: RingBuffer,
    entity_count: RingBuffer,
}

impl Default for DiagBuffers {
    fn default() -> Self {
        Self {
            fps: RingBuffer::new(SPARKLINE_WINDOW),
            frame_time_ms: RingBuffer::new(SPARKLINE_WINDOW),
            entity_count: RingBuffer::new(SPARKLINE_WINDOW),
        }
    }
}

/// Marker on a card's value `Text`, carrying the metric it shows and the
/// handle of its sparkline material so the update system can plot into it.
#[derive(Component)]
pub(crate) struct DiagCard {
    metric: Metric,
    material: Handle<SparklineMaterial>,
}

const FPS_COLOR: Color = tokens::TEXT_SUCCESS;
const FRAME_TIME_COLOR: Color = tokens::TYPE_STRING;
const ENTITY_COLOR: Color = tokens::TEXT_ACCENT;

/// Height of a card's sparkline strip.
const SPARKLINE_HEIGHT: f32 = 40.0;

/// Mint one sparkline material per metric and spawn the panel as a child of the
/// dock window. Called from the window's build closure, which owns the world.
pub fn build_diagnostics_window(window: &mut ChildSpawner<'_>) {
    let (fps, frame_time, entities) = {
        let mut materials = window
            .world_mut()
            .resource_mut::<Assets<SparklineMaterial>>();
        (
            materials.add(SparklineMaterial::new(FPS_COLOR)),
            materials.add(SparklineMaterial::new(FRAME_TIME_COLOR)),
            materials.add(SparklineMaterial::new(ENTITY_COLOR)),
        )
    };
    window.spawn(diagnostics_panel(fps, frame_time, entities));
}

/// Panel column: a header and one stat card per metric.
fn diagnostics_panel(
    fps: Handle<SparklineMaterial>,
    frame_time: Handle<SparklineMaterial>,
    entities: Handle<SparklineMaterial>,
) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![(
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: Val::Px(tokens::SPACING_MD),
                padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
                ..default()
            },
            children![
                stat_card("FPS", Metric::Fps, fps),
                stat_card("Frame Time", Metric::FrameTime, frame_time),
                stat_card("Entities", Metric::EntityCount, entities),
            ],
        ),],
    )
}

/// A single stat card: label, value readout, and a sparkline strip. The value
/// `Text` carries the `DiagCard` marker; the sparkline fill node renders the
/// same material handle the marker stores.
fn stat_card(label: &str, metric: Metric, material: Handle<SparklineMaterial>) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            row_gap: Val::Px(tokens::SPACING_XS),
            padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
            ..default()
        },
        BackgroundColor(tokens::COMPONENT_CARD_BG),
        BorderColor::all(tokens::COMPONENT_CARD_BORDER),
        children![
            (
                Text::new(label),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                Text::new("-"),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XL,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
                DiagCard {
                    metric,
                    material: material.clone(),
                },
            ),
            (
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(SPARKLINE_HEIGHT),
                    ..default()
                },
                children![(
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    MaterialNode(material),
                )],
            ),
        ],
    )
}

/// True while the debugger is connected to a running game.
pub fn connected(manager: Option<Res<ConnectionManager>>) -> bool {
    manager.is_some_and(|m| m.is_connected())
}

/// On each new diagnostics reply, extend every metric's history and refresh the
/// cards' value text and sparklines.
pub(crate) fn update_diagnostics_panel(
    sample: Option<Res<DiagnosticsSample>>,
    mut buffers: ResMut<DiagBuffers>,
    mut materials: ResMut<Assets<SparklineMaterial>>,
    mut cards: Query<(&mut Text, &DiagCard)>,
) {
    let Some(sample) = sample else { return };
    if !sample.is_changed() {
        return;
    }

    if let Some(fps) = sample.fps {
        buffers.fps.push(fps as f32);
    }
    if let Some(frame_time) = sample.frame_time_ms {
        buffers.frame_time_ms.push(frame_time as f32);
    }
    buffers.entity_count.push(sample.entity_count as f32);

    for (mut text, card) in &mut cards {
        let (value, buffer) = match card.metric {
            Metric::Fps => (
                sample
                    .fps
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "-".to_string()),
                &buffers.fps,
            ),
            Metric::FrameTime => (
                sample
                    .frame_time_ms
                    .map(|v| format!("{v:.1} ms"))
                    .unwrap_or_else(|| "-".to_string()),
                &buffers.frame_time_ms,
            ),
            Metric::EntityCount => (with_thousands(sample.entity_count), &buffers.entity_count),
        };
        text.0 = value;
        if let Some(mut material) = materials.get_mut(&card.material) {
            material.set_samples(buffer);
        }
    }
}

/// Group digits into thousands with commas: `12345` -> `12,345`.
fn with_thousands(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::with_thousands;

    #[test]
    fn thousands_grouping() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(42), "42");
        assert_eq!(with_thousands(1000), "1,000");
        assert_eq!(with_thousands(12345), "12,345");
        assert_eq!(with_thousands(1234567), "1,234,567");
    }
}
