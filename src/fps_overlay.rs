//! Frame-rate readout, off by default.
//!
//! Wraps `bevy_dev_tools`' stock overlay, which owns the diagnostic
//! plumbing and a frame-time graph. Its root node is absolutely
//! positioned at the window's top-left, where the menu bar sits;
//! `place_overlay` moves it to the bottom-right corner, clear of both
//! the viewport toolbar and the tool palette.
//!
//! The text and the graph are toggled together by
//! `view.toggle_fps_overlay`: upstream keeps two independent `enabled`
//! flags, so leaving the graph's on would draw it over a hidden readout.
//!
//! Upstream's readout is a frame *rate*, an average in which a single
//! long frame inside a vsync cap barely registers.
//! `append_frame_time` adds the millisecond figure beside it, and the
//! graph beneath both shows a hitch as a spike.

use core::time::Duration;

use bevy::dev_tools::fps_overlay::{
    FPS_OVERLAY_ZINDEX, FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig,
};
use bevy::diagnostic::{Diagnostic, DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_feathers::tokens;

use crate::core_extension::CoreExtensionInputContext;

/// Gap between the readout and the window edges it sits against.
const MARGIN: f32 = 8.0;

/// How often both halves of the readout are rewritten. Upstream's default,
/// shared so the frame-time figure and the rate beside it describe the same
/// moment.
const REFRESH: Duration = Duration::from_millis(100);

/// Marks the span holding the millisecond figure.
#[derive(Component)]
struct FrameTimeText;

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(FpsOverlayPlugin {
        config: FpsOverlayConfig {
            text_config: TextFont::from_font_size(tokens::TEXT_SIZE_PX),
            text_color: tokens::TEXT_PRIMARY,
            enabled: false,
            refresh_interval: REFRESH,
            frame_time_graph_config: FrameTimeGraphConfig {
                enabled: false,
                ..default()
            },
        },
    })
    .add_systems(PostStartup, (place_overlay, append_frame_time))
    .add_systems(Update, update_frame_time.run_if(on_timer(REFRESH)));
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewToggleFpsOverlayOp>();
    ctx.bind_operator::<CoreExtensionInputContext, ViewToggleFpsOverlayOp>([PresetInput::key(
        "F3",
    )]);
}

/// Move the stock overlay off the menu bar.
///
/// Upstream spawns one root node carrying [`FPS_OVERLAY_ZINDEX`], which
/// identifies it here since its marker components are private. Runs in
/// `PostStartup` because upstream's spawn is a `Startup` system.
fn place_overlay(mut nodes: Query<(&GlobalZIndex, &mut Node)>) {
    for (z_index, mut node) in &mut nodes {
        if z_index.0 != FPS_OVERLAY_ZINDEX {
            continue;
        }
        node.left = Val::Auto;
        node.top = Val::Auto;
        node.right = Val::Px(MARGIN);
        node.bottom = Val::Px(tokens::STATUS_BAR_HEIGHT + MARGIN);
    }
}

/// Add the millisecond figure to upstream's readout.
///
/// A span rather than a node of its own, so it sits on the same line as the
/// rate and inherits the font and colour upstream's `customize` pass writes
/// across the whole text.
fn append_frame_time(
    mut commands: Commands,
    config: Res<FpsOverlayConfig>,
    roots: Query<(&GlobalZIndex, &Children)>,
) {
    for (z_index, children) in &roots {
        if z_index.0 != FPS_OVERLAY_ZINDEX {
            continue;
        }
        let Some(text) = children.first() else {
            continue;
        };
        commands.entity(*text).with_child((
            TextSpan::default(),
            config.text_config.clone(),
            FrameTimeText,
        ));
    }
}

/// Write the last frame's duration into the span [`append_frame_time`] made
/// for it.
fn update_frame_time(
    diagnostics: Res<DiagnosticsStore>,
    mut spans: Query<&mut TextSpan, With<FrameTimeText>>,
) {
    let Some(frame_time) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(Diagnostic::smoothed)
    else {
        return;
    };
    for mut span in &mut spans {
        **span = format!("  {frame_time:.1} ms");
    }
}

/// Show or hide the frame-rate readout.
#[operator(
    id = "view.toggle_fps_overlay",
    label = "Toggle FPS Overlay",
    description = "Show or hide the frame-rate readout.",
    allows_undo = false
)]
pub(crate) fn view_toggle_fps_overlay(
    _: In<OperatorParameters>,
    mut config: ResMut<FpsOverlayConfig>,
) -> OperatorResult {
    let enabled = !config.enabled;
    config.enabled = enabled;
    config.frame_time_graph_config.enabled = enabled;
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use bevy::ui::Display;

    use super::*;

    /// Checked on the rendered `Node` rather than the config, since upstream's
    /// `toggle_display` decides whether anything is on screen.
    #[test]
    fn the_operator_toggles_what_the_overlay_displays() {
        let mut app = App::new();
        // The stock overlay pulls in a UI material for its frame-time graph, so this needs
        // the render plugins; no backend is required to hold the assets they register.
        app.add_plugins(
            DefaultPlugins
                .set(bevy::render::RenderPlugin {
                    render_creation: bevy::render::settings::RenderCreation::Automatic(Box::new(
                        bevy::render::settings::WgpuSettings {
                            backends: None,
                            ..default()
                        },
                    )),
                    ..default()
                })
                .disable::<bevy::audio::AudioPlugin>()
                .disable::<bevy::winit::WinitPlugin>(),
        )
        .add_plugins(plugin);
        app.finish();
        app.update();

        assert_eq!(displayed(&mut app), Some(Display::None), "starts hidden");

        toggle(&mut app);
        assert_eq!(
            displayed(&mut app),
            Some(Display::DEFAULT),
            "the operator shows the readout"
        );

        toggle(&mut app);
        assert_eq!(
            displayed(&mut app),
            Some(Display::None),
            "a second call hides it again"
        );
    }

    /// A frame rate is an average that a hitch barely moves, so the readout carries the
    /// millisecond figure too.
    #[test]
    fn the_readout_carries_a_frame_time_beside_the_rate() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::diagnostic::DiagnosticsPlugin,
            FrameTimeDiagnosticsPlugin::default(),
        ))
        .init_resource::<FpsOverlayConfig>()
        .add_systems(Update, update_frame_time.run_if(on_timer(REFRESH)));

        let span = app
            .world_mut()
            .spawn((TextSpan::default(), FrameTimeText))
            .id();
        // The diagnostic must have measured a frame before it reads back, and the readout
        // rewrites only on its own timer.
        for _ in 0..4 {
            app.update();
            std::thread::sleep(REFRESH);
        }
        app.update();

        let written = app.world().get::<TextSpan>(span).expect("the span lives");
        assert!(
            written.ends_with(" ms"),
            "the readout must name a frame time in milliseconds, got {written:?}"
        );
    }

    fn toggle(app: &mut App) {
        let outcome = app
            .world_mut()
            .run_system_cached_with(view_toggle_fps_overlay, OperatorParameters::default())
            .expect("the operator runs");
        assert_eq!(outcome, OperatorResult::Finished);
        app.update();
    }

    /// `Display` of the readout's text node, found through the overlay root's z-index the
    /// same way [`place_overlay`] finds it.
    fn displayed(app: &mut App) -> Option<Display> {
        let world = app.world_mut();
        let mut roots = world.query::<(Entity, &GlobalZIndex)>();
        let root = roots
            .iter(world)
            .find(|(_, z)| z.0 == FPS_OVERLAY_ZINDEX)
            .map(|(entity, _)| entity)?;
        let child = *world.get::<Children>(root)?.first()?;
        Some(world.get::<Node>(child)?.display)
    }
}
