//! Frame-rate readout, off by default.
//!
//! Wraps `bevy_dev_tools`' stock overlay, which owns the diagnostic
//! plumbing and a frame-time graph. Its root node is absolutely
//! positioned at the window's top-left, where the menu bar sits;
//! `place_overlay` moves it to the bottom-right corner, clear of both
//! the viewport toolbar and the tool palette.
//!
//! The text and the graph follow the viewport's Stats setting together
//! (`viewport.stats.toggle`, F3): upstream keeps two independent `enabled`
//! flags, so leaving the graph's on would draw it over a hidden readout.
//!
//! Upstream's readout is a frame *rate*, an average in which a single
//! long frame inside a vsync cap barely registers.
//! `append_frame_time` adds the millisecond figure beside it, and the
//! graph beneath both shows a hitch as a spike.
//!
//! Beneath them a scene readout counts what the frame draws: meshes drawn
//! against meshes held, lights and the shadow views they cost, and the parts
//! drawn at each level of detail. While shown, the overlay sits in the
//! top-right corner of the first 3D viewport.

use core::time::Duration;

use bevy::dev_tools::fps_overlay::{
    FPS_OVERLAY_ZINDEX, FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig,
};
use bevy::diagnostic::{Diagnostic, DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use jackdaw_feathers::tokens;

use crate::viewport_settings::ViewportSettings;

/// Gap between the readout and the window edges it sits against.
const MARGIN: f32 = 8.0;

/// How often both halves of the readout are rewritten. Upstream's default,
/// shared so the frame-time figure and the rate beside it describe the same
/// moment.
const REFRESH: Duration = Duration::from_millis(100);

/// Marks the span holding the millisecond figure.
#[derive(Component)]
struct FrameTimeText;

/// Marks the text holding the scene readout under the frame rate.
#[derive(Component)]
pub struct SceneReadout;

/// How often the scene readout is counted again.
const READOUT_REFRESH: Duration = Duration::from_millis(500);

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
    .add_systems(
        PostStartup,
        (append_frame_time, append_scene_readout, place_overlay).chain(),
    )
    .add_systems(
        PreUpdate,
        follow_stats_setting.run_if(resource_changed::<ViewportSettings>),
    )
    .add_systems(Update, update_frame_time.run_if(on_timer(REFRESH)))
    .add_systems(
        Update,
        (
            show_scene_readout,
            count_the_scene
                .run_if(stats_shown)
                .run_if(on_timer(READOUT_REFRESH)),
        ),
    )
    .add_systems(PostUpdate, keep_overlay_on_the_viewport.run_if(stats_shown));
}

fn stats_shown(settings: Option<Res<ViewportSettings>>) -> bool {
    settings.is_some_and(|settings| settings.stats)
}

/// On the overlay's root node.
#[derive(Component)]
pub struct StatsOverlay;

/// Where the overlay stacks among the window's root nodes: over the panels,
/// under menus and their dropdowns.
const OVERLAY_Z: i32 = 500;

/// Move the stock overlay off the menu bar, behind open menus, and out of the
/// pointer's way, on a dark backing that keeps it legible over a bright sky.
///
/// Upstream spawns one root node carrying [`FPS_OVERLAY_ZINDEX`], which
/// identifies it here since its marker components are private; it is marked
/// [`StatsOverlay`] and restacked once the other `PostStartup` passes have
/// found it. Runs in `PostStartup` because upstream's spawn is a `Startup`
/// system.
fn place_overlay(
    mut commands: Commands,
    mut nodes: Query<(Entity, &GlobalZIndex, &mut Node)>,
    children: Query<&Children>,
) {
    for (root, z_index, mut node) in &mut nodes {
        if z_index.0 != FPS_OVERLAY_ZINDEX {
            continue;
        }
        node.left = Val::Auto;
        node.top = Val::Auto;
        node.right = Val::Px(MARGIN);
        node.bottom = Val::Px(tokens::STATUS_BAR_HEIGHT + MARGIN);
        node.padding = UiRect::all(Val::Px(tokens::SPACING_SM));
        node.border_radius = BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD));
        commands.entity(root).remove::<GlobalZIndex>().insert((
            StatsOverlay,
            ZIndex(OVERLAY_Z),
            BackgroundColor(OVERLAY_BACKING),
            Pickable::IGNORE,
        ));
        for part in children.iter_descendants(root) {
            commands.entity(part).insert(Pickable::IGNORE);
        }
    }
}

/// The translucent dark the overlay's text sits on.
const OVERLAY_BACKING: Color = Color::srgba(0.05, 0.055, 0.07, 0.8);

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

/// Add the scene readout as a text of its own under upstream's rows.
fn append_scene_readout(
    mut commands: Commands,
    config: Res<FpsOverlayConfig>,
    roots: Query<(Entity, &GlobalZIndex)>,
) {
    for (root, z_index) in &roots {
        if z_index.0 != FPS_OVERLAY_ZINDEX {
            continue;
        }
        commands.entity(root).with_child((
            SceneReadout,
            Text::default(),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..config.text_config.clone()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                display: Display::None,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
}

/// Show the readout, and the overlay's backing with it, while Stats is on.
fn show_scene_readout(
    settings: Option<Res<ViewportSettings>>,
    mut readouts: Query<&mut Node, Or<(With<SceneReadout>, With<StatsOverlay>)>>,
) {
    let wanted = if settings.is_some_and(|settings| settings.stats) {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut readouts {
        if node.display != wanted {
            node.display = wanted;
        }
    }
}

/// What one frame of the scene draws, for the readout.
#[derive(Debug, Default, PartialEq)]
pub struct SceneCounts {
    pub meshes: usize,
    pub meshes_drawn: usize,
    pub lights: usize,
    /// Lights drawn with shadow maps in the viewport.
    pub shadowed_lights: usize,
    /// Views rendered for those shadow maps: six for a point light, one for a
    /// spot light, one per cascade for the sun.
    pub shadow_views: usize,
    /// Parts drawn at each level of detail, most detailed first.
    pub lod_parts: Vec<usize>,
}

impl SceneCounts {
    pub fn lines(&self) -> String {
        let mut text = format!(
            "meshes {} drawn of {}\nlights {}, {} with shadows ({} shadow views)",
            self.meshes_drawn, self.meshes, self.lights, self.shadowed_lights, self.shadow_views
        );
        if !self.lod_parts.is_empty() {
            let levels: Vec<String> = self
                .lod_parts
                .iter()
                .enumerate()
                .map(|(level, parts)| format!("{level}: {parts}"))
                .collect();
            text.push_str(&format!("\nLOD parts {}", levels.join("  ")));
        }
        text
    }
}

type SceneOnly = Without<crate::EditorEntity>;

fn count_the_scene(
    casters: Option<Res<crate::viewport_quality::ShadowCasters>>,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
    points: Query<(&PointLight, &ViewVisibility), SceneOnly>,
    spots: Query<(&SpotLight, &ViewVisibility), SceneOnly>,
    suns: Query<(&DirectionalLight, Option<&bevy::light::CascadeShadowConfig>), SceneOnly>,
    lod_parts: Query<(&jackdaw_runtime::LodPartLevel, &ViewVisibility)>,
    mut readouts: Query<&mut Text, With<SceneReadout>>,
) {
    let casters = casters.map(|casters| *casters).unwrap_or_default();
    let mut counts = SceneCounts {
        meshes: meshes.iter().count(),
        meshes_drawn: meshes.iter().filter(|visible| visible.get()).count(),
        lights: points.iter().count() + spots.iter().count() + suns.iter().count(),
        ..default()
    };
    for (light, visible) in &points {
        if casters.point && light.shadow_maps_enabled && visible.get() {
            counts.shadowed_lights += 1;
            counts.shadow_views += 6;
        }
    }
    for (light, visible) in &spots {
        if casters.point && light.shadow_maps_enabled && visible.get() {
            counts.shadowed_lights += 1;
            counts.shadow_views += 1;
        }
    }
    for (light, cascades) in &suns {
        if casters.sun && light.shadow_maps_enabled {
            counts.shadowed_lights += 1;
            counts.shadow_views += cascades.map_or(1, |config| config.bounds.len());
        }
    }
    for (level, visible) in &lod_parts {
        if !visible.get() {
            continue;
        }
        if counts.lod_parts.len() <= level.0 {
            counts.lod_parts.resize(level.0 + 1, 0);
        }
        counts.lod_parts[level.0] += 1;
    }
    let text = counts.lines();
    for mut readout in &mut readouts {
        if readout.0 != text {
            readout.0.clone_from(&text);
        }
    }
}

/// Hold the overlay in the top-right corner of the first laid-out 3D
/// viewport.
fn keep_overlay_on_the_viewport(
    viewports: Query<
        (&ComputedNode, &bevy::ui::UiGlobalTransform),
        With<crate::viewport::SceneViewport>,
    >,
    mut roots: Query<
        (&ComputedNode, &mut Node),
        (With<StatsOverlay>, Without<crate::viewport::SceneViewport>),
    >,
) {
    let Some((viewport, at)) = viewports
        .iter()
        .find(|(node, _)| node.size().x > 0.0 && node.size().y > 0.0)
    else {
        return;
    };
    let scale = viewport.inverse_scale_factor();
    let centre = at.translation * scale;
    let half = viewport.size() * scale / 2.0;
    for (overlay, mut node) in &mut roots {
        let width = overlay.size().x * overlay.inverse_scale_factor();
        let left = Val::Px((centre.x + half.x - width - MARGIN).max(centre.x - half.x));
        let top = Val::Px(centre.y - half.y + MARGIN);
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
            node.right = Val::Auto;
            node.bottom = Val::Auto;
        }
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

/// Show the readout and its graph while the viewport's Stats setting is on.
fn follow_stats_setting(settings: Res<ViewportSettings>, mut config: ResMut<FpsOverlayConfig>) {
    if config.enabled == settings.stats && config.frame_time_graph_config.enabled == settings.stats
    {
        return;
    }
    config.enabled = settings.stats;
    config.frame_time_graph_config.enabled = settings.stats;
}

#[cfg(test)]
mod tests {
    use bevy::ui::Display;

    use super::*;

    /// Checked on the rendered `Node` rather than the config, since upstream's
    /// `toggle_display` decides whether anything is on screen.
    #[test]
    fn the_stats_setting_toggles_what_the_overlay_displays() {
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
        .init_resource::<ViewportSettings>()
        .add_plugins(plugin);
        app.finish();
        app.update();

        assert_eq!(displayed(&mut app), Some(Display::None), "starts hidden");

        toggle(&mut app);
        assert_eq!(
            displayed(&mut app),
            Some(Display::DEFAULT),
            "turning Stats on shows the readout"
        );

        toggle(&mut app);
        assert_eq!(
            displayed(&mut app),
            Some(Display::None),
            "turning it off hides it again"
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

    #[test]
    fn the_scene_readout_names_what_the_frame_draws() {
        let counts = SceneCounts {
            meshes: 120,
            meshes_drawn: 40,
            lights: 5,
            shadowed_lights: 2,
            shadow_views: 10,
            lod_parts: vec![30, 8, 2],
        };
        assert_eq!(
            counts.lines(),
            "meshes 40 drawn of 120\nlights 5, 2 with shadows (10 shadow views)\nLOD parts 0: 30  1: 8  2: 2"
        );
        let no_lod = SceneCounts {
            lod_parts: Vec::new(),
            ..counts
        };
        assert!(!no_lod.lines().contains("LOD"));
    }

    fn toggle(app: &mut App) {
        let mut settings = app.world_mut().resource_mut::<ViewportSettings>();
        settings.stats = !settings.stats;
        app.update();
    }

    /// `Display` of the readout's text node, under the root [`place_overlay`] marks.
    fn displayed(app: &mut App) -> Option<Display> {
        let world = app.world_mut();
        let mut roots = world.query_filtered::<Entity, With<StatsOverlay>>();
        let root = roots.iter(world).next()?;
        let child = *world.get::<Children>(root)?.first()?;
        Some(world.get::<Node>(child)?.display)
    }
}
