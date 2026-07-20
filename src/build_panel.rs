//! Build Window: a bottom-dock panel that forwards project-build status
//! and logs, so the background pre-build and any rebuild are visible with
//! progress instead of only a footer line. It focuses itself on the first
//! build after opening a project and whenever a build fails, but stays out
//! of the way on routine successful rebuilds.

use bevy::prelude::*;
use jackdaw_api::{
    DefaultArea,
    prelude::{ExtensionContext, ExtensionKind, JackdawExtension, WindowDescriptor},
};
use jackdaw_feathers::icons::{EditorFont, Icon};
use jackdaw_feathers::tokens;
use jackdaw_panels::tree::DockTree;

use crate::build_status::{BuildState, BuildStatus};
use crate::scrolling_log::{self, ScrollingLog, ScrollingLogProps};

/// Dock-window id for the Build panel.
pub const BUILD_WINDOW_ID: &str = "jackdaw.build";

/// Seconds for one back-and-forth of the indeterminate progress segment.
const BAR_CYCLE: f32 = 1.4;
/// Width of the sliding segment, in percent.
const BAR_SEGMENT: f32 = 22.0;

pub struct BuildPanelPlugin;

impl Plugin for BuildPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildPanelFocus>()
            .add_systems(OnEnter(crate::AppState::Editor), reset_build_focus)
            .add_systems(
                Update,
                (sync_build_panel, animate_build_bar, auto_focus_build_panel)
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// Tracks whether the panel has already surfaced itself for the current
/// project's first build, so it does that once per open, not every build.
#[derive(Resource, Default)]
struct BuildPanelFocus {
    first_build_shown: bool,
}

#[derive(Component)]
struct BuildPanelStatusText;
#[derive(Component)]
struct BuildPanelLog;
#[derive(Component)]
struct BuildPanelBarTrack;
#[derive(Component)]
struct BuildPanelBarFill;

/// The panel content: a status header with a thin progress bar, and a
/// scrolling log filling the rest.
fn build_build_panel(world: &mut World, parent: Entity, font: Handle<Font>) {
    let root = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    let header = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(5.0),
                padding: UiRect::all(Val::Px(8.0)),
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    world.spawn((
        BuildPanelStatusText,
        Text::new("No build running"),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(header),
    ));
    let track = world
        .spawn((
            BuildPanelBarTrack,
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(3.0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            Visibility::Hidden,
            ChildOf(header),
        ))
        .id();
    world.spawn((
        BuildPanelBarFill,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(0.0),
            width: Val::Percent(0.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(tokens::TEXT_ACCENT),
        ChildOf(track),
    ));

    let log = scrolling_log::spawn(
        world,
        root,
        ScrollingLogProps {
            max_height: Val::Auto,
            margin: UiRect::all(Val::Px(0.0)),
            font,
            font_size: tokens::TEXT_SIZE_XS,
            text_color: tokens::TEXT_SECONDARY,
            background: Color::NONE,
            auto_hide_when_empty: false,
        },
    );
    world.entity_mut(log).insert(BuildPanelLog);
    // Let the log take the remaining height and scroll within it.
    if let Some(mut node) = world.entity_mut(log).get_mut::<Node>() {
        node.flex_grow = 1.0;
        node.min_height = Val::Px(0.0);
    }
}

/// Mirror the build state into the panel: header text/color, progress-bar
/// visibility, and the scrolling log. The build log lives in the
/// `BuildProgress` sink only while a build runs; once it finishes the last
/// captured output stays on screen.
fn sync_build_panel(
    build_status: Res<BuildStatus>,
    mut status: Query<(&mut Text, &mut TextColor), With<BuildPanelStatusText>>,
    mut logs: Query<&mut ScrollingLog, With<BuildPanelLog>>,
    mut track: Query<&mut Visibility, With<BuildPanelBarTrack>>,
) {
    let (label, color, log_content, building) = match &build_status.state {
        BuildState::Building { progress, .. } => {
            let (current, done, log) = progress
                .lock()
                .map(|g| {
                    (
                        g.current_crate.clone(),
                        g.artifacts_done,
                        g.full_log.clone(),
                    )
                })
                .unwrap_or((None, 0, String::new()));
            let label = match current {
                Some(name) => format!("Compiling {name} ({done})"),
                None => "Building project...".to_string(),
            };
            (label, tokens::TEXT_SECONDARY, Some(log), true)
        }
        BuildState::Ready { components, .. } => (
            format!("Build succeeded - {components} components"),
            tokens::TEXT_SUCCESS,
            None,
            false,
        ),
        BuildState::Failed { .. } => (
            "Build failed - see the log below".to_string(),
            tokens::TEXT_ERROR,
            None,
            false,
        ),
        BuildState::Idle => (
            "No build running".to_string(),
            tokens::TEXT_SECONDARY,
            None,
            false,
        ),
    };

    if let Ok((mut text, mut text_color)) = status.single_mut() {
        if text.0 != label {
            text.0 = label;
        }
        if text_color.0 != color {
            text_color.0 = color;
        }
    }
    if let Some(content) = log_content
        && let Ok(mut log) = logs.single_mut()
        && log.content != content
    {
        log.content = content;
    }
    if let Ok(mut visibility) = track.single_mut() {
        let desired = if building {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *visibility != desired {
            *visibility = desired;
        }
    }
}

/// Slide the indeterminate progress segment while a build runs (the
/// redirected project build has no reliable unit total to fill against).
fn animate_build_bar(
    time: Res<Time>,
    build_status: Res<BuildStatus>,
    mut fill: Query<&mut Node, With<BuildPanelBarFill>>,
) {
    let Ok(mut node) = fill.single_mut() else {
        return;
    };
    if matches!(build_status.state, BuildState::Building { .. }) {
        let phase = (time.elapsed_secs() % BAR_CYCLE) / BAR_CYCLE;
        let sweep = 1.0 - (2.0 * phase - 1.0).abs();
        node.left = Val::Percent(sweep * (100.0 - BAR_SEGMENT));
        node.width = Val::Percent(BAR_SEGMENT);
    } else if node.width != Val::Percent(0.0) {
        node.width = Val::Percent(0.0);
    }
}

/// Reset the once-per-open focus flag when a project opens.
fn reset_build_focus(mut focus: ResMut<BuildPanelFocus>) {
    focus.first_build_shown = false;
}

/// Bring the Build panel to the front on the first build after opening a
/// project and whenever a build fails; leave routine successful rebuilds
/// alone so the panel does not steal focus.
fn auto_focus_build_panel(
    build_status: Res<BuildStatus>,
    mut tree: ResMut<DockTree>,
    mut focus: ResMut<BuildPanelFocus>,
) {
    if !build_status.is_changed() {
        return;
    }
    let surface = match &build_status.state {
        BuildState::Failed { .. } => true,
        BuildState::Building { .. } if !focus.first_build_shown => {
            focus.first_build_shown = true;
            true
        }
        _ => false,
    };
    if surface {
        activate_build_tab(&mut tree);
    }
}

/// Make the Build panel the active tab in whichever dock leaf holds it.
fn activate_build_tab(tree: &mut DockTree) {
    let Some(leaf_id) = tree.find_leaf_with_window(BUILD_WINDOW_ID) else {
        return;
    };
    let tab = tree
        .get(leaf_id)
        .and_then(|node| node.as_leaf())
        .and_then(|leaf| {
            leaf.tabs()
                .find(|(id, _)| *id == BUILD_WINDOW_ID)
                .map(|(_, tab)| tab)
        });
    if let Some(tab) = tab {
        tree.set_active(leaf_id, tab);
    }
}

/// Registers the Build panel as a bottom-dock window, like the other
/// built-in panels.
#[derive(Default)]
pub struct BuildPanelExtension;

impl JackdawExtension for BuildPanelExtension {
    fn id(&self) -> String {
        "jackdaw.build_panel".to_string()
    }

    fn label(&self) -> String {
        "Build".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(BUILD_WINDOW_ID)
                .with_name("Build")
                .with_icon(Icon::Hammer.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(3)
                .with_build(|window| {
                    let font = window
                        .world()
                        .get_resource::<EditorFont>()
                        .map(|f| f.0.clone())
                        .unwrap_or_default();
                    let parent = window.target_entity();
                    build_build_panel(window.world_mut(), parent, font);
                }),
        );
    }
}
