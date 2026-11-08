//! The Game panel: a dockable surface showing the focused instance's
//! streamed frame. A click on the frame engages input capture. The panel is
//! a pure monitor: no editor camera, no overlay, no compositing.

use std::time::{Duration, Instant};

use bevy::{prelude::*, ui_widgets::observe};
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::tokens;

use jackdaw_panels::tree::{DockTree, NodeId, TabId};

use crate::live_frame::LiveFrameStream;
use crate::pie::{GameExitReason, LastGameExit, PiePlayOp, PieToggleInstanceOp, PieWindowMode};

/// Dock window id, used for registration and presence checks.
pub const GAME_WINDOW_ID: &str = "jackdaw.game";

/// Marker on the panel content root.
#[derive(Component)]
pub struct GamePanel;

/// Marker on the letterbox container the frame image centers inside. Its
/// computed size is the stream request size and the cursor remap space.
#[derive(Component)]
pub struct GamePanelSurface;

/// Marker on the frame `ImageNode`.
#[derive(Component)]
pub struct GamePanelImage;

/// Marker on the column shown while no frame is streaming.
#[derive(Component)]
pub struct GamePanelIdle;

/// Marker on the idle label saying what the game is doing.
#[derive(Component)]
pub struct GamePanelIdleLabel;

/// Marker on the text holding a crashed game's last stderr lines.
#[derive(Component)]
pub struct GamePanelStderrText;

/// One part of the idle column, shown or hidden by what the game is doing.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameIdlePart {
    Label,
    Stderr,
    /// The Play button shown while stopped.
    Play,
    /// The Restart button shown after a game went away, relaunching that game.
    Restart,
    /// The button opening the Build panel after a failed build.
    BuildErrors,
    /// The input hint under the Play button.
    Hint,
}

/// Marker on the "no signal" chip shown while the focused instance's stream
/// has gone quiet.
#[derive(Component)]
pub struct GameNoSignalChip;

/// Marker on the "Playing, Shift+Esc to release" chip shown while input
/// capture is forwarding to the game.
#[derive(Component)]
pub struct GamePlayingChip;

/// Marker on the header button that toggles forwarding editor input to the
/// running game.
#[derive(Component)]
pub struct GamePlayInputButton;

/// Whether Play brought the Game panel forward, and the tab its dock showed
/// before, so the end of the session can put that tab back. `failed` holds
/// the panel after a launch that went wrong, until the next one.
#[derive(Resource, Default, Debug)]
struct GamePanelSurfaced {
    shown: bool,
    failed: bool,
    previous: Option<(NodeId, TabId)>,
}

/// Letterboxed (contain-fit) display size and top-left offset for a stream
/// inside a panel.
pub(crate) fn contain_fit(panel: Vec2, stream: Vec2) -> (Vec2, Vec2) {
    let scale = (panel.x / stream.x).min(panel.y / stream.y);
    let size = stream * scale;
    (size, (panel - size) * 0.5)
}

/// Map a panel-local position to stream pixels through the contain fit,
/// clamped to the stream bounds. `None` on degenerate sizes.
pub fn panel_to_stream(local: Vec2, panel: Vec2, stream: Vec2) -> Option<Vec2> {
    if panel.x < 1.0 || panel.y < 1.0 || stream.x < 1.0 || stream.y < 1.0 {
        return None;
    }
    let scale = (panel.x / stream.x).min(panel.y / stream.y);
    let size = stream * scale;
    let offset = (panel - size) * 0.5;
    Some(((local - offset) / scale).clamp(Vec2::ZERO, stream))
}

/// The surface's top-left corner and logical size in window-logical pixels,
/// from its `ComputedNode` and `UiGlobalTransform`. `UiGlobalTransform`'s
/// translation is the node center in physical pixels, so it is scaled to
/// logical and shifted by half the logical size. Window-logical positions
/// map to surface-local with `window_pos - top_left`.
pub(crate) fn surface_remap(
    computed: &ComputedNode,
    transform: &bevy::ui::UiGlobalTransform,
) -> (Vec2, Vec2) {
    let scale = computed.inverse_scale_factor();
    let panel = computed.size() * scale;
    let top_left = transform.translation * scale - panel * 0.5;
    (top_left, panel)
}

/// Engage input capture on a click on the frame surface. Clicks while the
/// stream is not fresh do nothing, though the last frame stays up.
fn surface_clicked(world: &mut World, surface: Entity) {
    let fresh = world
        .get_resource::<LiveFrameStream>()
        .is_some_and(LiveFrameStream::is_fresh);
    let embedded = crate::pie::effective_window_mode_in(world) == PieWindowMode::Embedded;
    if !fresh || !embedded {
        return;
    }
    if !world
        .resource::<crate::live_input::LiveInputCapture>()
        .active
    {
        crate::live_input::engage_capture(world, surface);
    }
}

/// Build the panel content: a header with the signal and playing chips and
/// a Play-input button; below it a letterbox surface
/// hosting the frame image and an idle label.
pub fn game_panel_content() -> impl Bundle {
    (
        GamePanel,
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            height: percent(100),
            border: UiRect::all(px(1.0)),
            ..Default::default()
        },
        BorderColor::all(Color::NONE),
        children![
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_SM),
                    padding: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                children![
                    game_no_signal_chip(),
                    game_playing_chip(),
                    game_play_input_button(),
                    game_instance_button(),
                ],
            ),
            (
                GamePanelSurface,
                Node {
                    flex_grow: 1.0,
                    overflow: Overflow::clip(),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    position_type: PositionType::Relative,
                    ..Default::default()
                },
                observe(|click: On<PointerClick>, mut commands: Commands| {
                    let surface = click.event_target();
                    commands.queue(move |world: &mut World| {
                        surface_clicked(world, surface);
                    });
                }),
                children![
                    (
                        GamePanelImage,
                        ImageNode::new(Handle::default()),
                        Node {
                            display: Display::None,
                            ..Default::default()
                        },
                    ),
                    game_panel_idle(),
                ],
            ),
        ],
    )
}

/// Build the column shown while no frame is streaming: what the game is doing,
/// a Play or Restart button, a crashed game's last stderr lines, and a hint.
/// The stderr block shrinks to the room left and keeps its newest lines.
fn game_panel_idle() -> impl Bundle {
    (
        GamePanelIdle,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            bottom: px(0.0),
            left: px(0.0),
            right: px(0.0),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            row_gap: px(tokens::SPACING_MD),
            padding: UiRect::all(px(tokens::SPACING_MD)),
            ..Default::default()
        },
        children![
            (
                GamePanelIdleLabel,
                GameIdlePart::Label,
                Text::new(String::new()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                TextLayout::justify(Justify::Center),
                Node {
                    display: Display::None,
                    ..Default::default()
                },
            ),
            (
                GameIdlePart::Play,
                button(
                    ButtonProps::new("Play")
                        .with_variant(ButtonVariant::Primary)
                        .call_operator(PiePlayOp::ID)
                        .hidden(),
                ),
            ),
            (
                GameIdlePart::Restart,
                ButtonOperatorCall::new(PieToggleInstanceOp::ID),
                button(ButtonProps::new("Restart").hidden()),
            ),
            (
                GameIdlePart::BuildErrors,
                ButtonOperatorCall::new(crate::WindowOpenOp::ID)
                    .with_param("window_id", crate::build_panel::BUILD_WINDOW_ID),
                button(ButtonProps::new("Show build errors").hidden()),
            ),
            (
                GameIdlePart::Stderr,
                Node {
                    display: Display::None,
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::FlexEnd,
                    flex_shrink: 1.0,
                    min_height: px(0.0),
                    overflow: Overflow::clip(),
                    padding: UiRect::all(px(tokens::SPACING_MD)),
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
                    max_width: percent(100),
                    ..Default::default()
                },
                BackgroundColor(tokens::ELEVATED_BG),
                children![(
                    GamePanelStderrText,
                    Text::new(String::new()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_XS,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    Node {
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                )],
            ),
            (
                GameIdlePart::Hint,
                Text::new(IDLE_HINT),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    display: Display::None,
                    ..Default::default()
                },
            ),
        ],
    )
}

/// Build the "no signal" chip, shown while the focused instance's stream has
/// gone quiet.
fn game_no_signal_chip() -> impl Bundle {
    (
        GameNoSignalChip,
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            display: Display::None,
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        children![(
            Text::new("no signal"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_DISABLED),
        )],
    )
}

/// Build the "Playing, Shift+Esc to release" chip, shown while input capture
/// forwards editor input to the running game.
fn game_playing_chip() -> impl Bundle {
    (
        GamePlayingChip,
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            display: Display::None,
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        children![(
            Text::new("Playing, Shift+Esc to release"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(crate::default_style::CAPTURE_ACCENT),
        )],
    )
}

/// Build the button that toggles forwarding editor input to the running game.
fn game_play_input_button() -> impl Bundle {
    (
        GamePlayInputButton,
        jackdaw_feathers::tooltip::Tooltip::title(
            "Forward keyboard and mouse to the running game (Shift+Esc releases)",
        ),
        button(ButtonProps::new("Play Input")),
        observe(|_: On<PointerClick>, mut commands: Commands| {
            commands
                .operator(crate::live_input::PiePlayInputToggleOp::ID)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: false,
                })
                .call();
        }),
    )
}

/// Marker on the button naming the shown instance, there with several running.
#[derive(Component)]
pub struct GameInstanceButton;

/// Build the button that steps the panel to the next running instance.
fn game_instance_button() -> impl Bundle {
    (
        GameInstanceButton,
        jackdaw_feathers::tooltip::Tooltip::title("Show the next running instance"),
        button(ButtonProps::new("").hidden()),
        observe(|_: On<PointerClick>, mut commands: Commands| {
            commands
                .operator(crate::pie::PieNextInstanceOp::ID)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: false,
                })
                .call();
        }),
        children![(
            Text::new(String::new()),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Show the instance button with several instances running, naming the focused one.
fn update_game_instance_button(
    session: Option<NonSend<crate::pie::PieSession>>,
    instances: Res<crate::pie::PieInstances>,
    mut buttons: Query<(&mut Node, &Children), With<GameInstanceButton>>,
    mut labels: Query<&mut Text>,
) {
    let several = session.is_some_and(|session| session.live_keys().len() > 1);
    let display = if several {
        Display::Flex
    } else {
        Display::None
    };
    let label = instances
        .focused
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    for (mut node, children) in &mut buttons {
        if node.display != display {
            node.display = display;
        }
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child)
                && text.0 != label
            {
                text.0 = label.clone();
            }
        }
    }
}

/// Point the frame image at the current stream and show it once a frame has
/// arrived, else show the idle column. A slow game's last frame stays up.
fn sync_game_panel_image(
    stream: Res<LiveFrameStream>,
    mut images: Query<(&mut ImageNode, &mut Node), With<GamePanelImage>>,
    mut idle: Query<&mut Node, (With<GamePanelIdle>, Without<GamePanelImage>)>,
) {
    let handle = stream.image.clone();
    let show_image = handle.is_some();
    let image_display = if show_image {
        Display::Flex
    } else {
        Display::None
    };
    let idle_display = if show_image {
        Display::None
    } else {
        Display::Flex
    };
    for (mut image_node, mut node) in &mut images {
        if let Some(ref handle) = handle
            && image_node.image != *handle
        {
            image_node.image = handle.clone();
        }
        if node.display != image_display {
            node.display = image_display;
        }
    }
    for mut node in &mut idle {
        if node.display != idle_display {
            node.display = idle_display;
        }
    }
}

const IDLE_HINT: &str = "Click the game to give it input, Shift+Esc to release";
const IDLE_BUILDING: &str = "Building the game...";
const IDLE_WINDOWED: &str = "Running in its own window";
const IDLE_LOADING: &str = "Loading the game...";
const IDLE_STARTING: &str = "Starting the game...";
const IDLE_OWN_WINDOW: &str = "This game opened its own window. To show it here, pass DefaultPlugins \
     through jackdaw_runtime::maybe_windowless in its main.rs.";
const IDLE_BUILD_FAILED: &str = "The build failed.";
const IDLE_NEVER_CONNECTED: &str = "The game did not connect to the editor. Its app must add \
     JackdawPlugin, with the pie feature of jackdaw_runtime enabled.";

/// How long a launched game may stay unconnected before the panel says why it
/// might never connect. It is stopped soon after, at the session's own timeout.
const SLOW_CONNECT: Duration = Duration::from_secs(10);

/// How long a game may take to its first frame before the panel remarks on it.
const SLOW_FIRST_FRAME: Duration = Duration::from_secs(60);

/// What the game is doing, for the label shown while the panel has no frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GameWait<'a> {
    Stopped,
    Building,
    Windowed,
    OwnWindow,
    /// Launched and not yet connected, for this long. A game connects when
    /// `JackdawPlugin` builds, so one without it never does.
    Connecting(Duration),
    /// Connected and not yet showing a frame, for this long.
    Loading(Duration),
    /// The last launch's build failed.
    BuildFailed,
    /// The last game went away on its own.
    Exited(&'a GameExitReason),
}

/// The button the idle column offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdleAction {
    None,
    Play,
    Restart,
    ShowBuildErrors,
}

fn idle_message(wait: GameWait) -> String {
    match wait {
        GameWait::Stopped => String::new(),
        GameWait::Building => IDLE_BUILDING.to_string(),
        GameWait::Windowed => IDLE_WINDOWED.to_string(),
        GameWait::OwnWindow => IDLE_OWN_WINDOW.to_string(),
        GameWait::Connecting(waited) if waited >= SLOW_CONNECT => {
            "The game has not connected to the editor yet. It connects when its app adds \
             JackdawPlugin, and is stopped if it does not."
                .to_string()
        }
        GameWait::Connecting(_) => IDLE_STARTING.to_string(),
        GameWait::Loading(waited) if waited >= SLOW_FIRST_FRAME => format!(
            "Still loading the game ({} s). A large scene takes a while, and a game with its \
             own window on an older runtime will not show here.",
            waited.as_secs()
        ),
        GameWait::Loading(_) => IDLE_LOADING.to_string(),
        GameWait::BuildFailed => IDLE_BUILD_FAILED.to_string(),
        GameWait::Exited(GameExitReason::Crashed(status)) => format!("Game exited ({status})"),
        GameWait::Exited(GameExitReason::ExitedBeforeConnecting(status)) => {
            format!("Game exited before connecting ({status})")
        }
        GameWait::Exited(GameExitReason::NeverConnected) => IDLE_NEVER_CONNECTED.to_string(),
        GameWait::Exited(GameExitReason::ConnectFailed(error)) => {
            format!("The game could not connect to the editor: {error}")
        }
    }
}

fn idle_action(wait: GameWait) -> IdleAction {
    match wait {
        GameWait::Stopped => IdleAction::Play,
        GameWait::Exited(_) => IdleAction::Restart,
        GameWait::BuildFailed => IdleAction::ShowBuildErrors,
        _ => IdleAction::None,
    }
}

/// Whether `part` of the idle column shows for `wait`, given whether the game
/// left any stderr lines and whether a launch opens its own window.
fn idle_part_shown(part: GameIdlePart, wait: GameWait, has_stderr: bool, windowed: bool) -> bool {
    match part {
        GameIdlePart::Label => wait != GameWait::Stopped,
        GameIdlePart::Stderr => matches!(wait, GameWait::Exited(_)) && has_stderr,
        GameIdlePart::Play => idle_action(wait) == IdleAction::Play,
        GameIdlePart::Restart => idle_action(wait) == IdleAction::Restart,
        GameIdlePart::BuildErrors => idle_action(wait) == IdleAction::ShowBuildErrors,
        GameIdlePart::Hint => wait == GameWait::Stopped && !windowed,
    }
}

/// What the game is doing while it is not running, from the session and the
/// last recorded exit.
fn stopped_wait<'a>(
    session: Option<&crate::pie::PieSession>,
    last: Option<&'a GameExitReason>,
) -> GameWait<'a> {
    let Some(session) = session else {
        return last.map_or(GameWait::Stopped, GameWait::Exited);
    };
    if session.is_launching() {
        GameWait::Building
    } else if let Some(waited) = session.connecting_for() {
        GameWait::Connecting(waited)
    } else if session.launch_failed() {
        GameWait::BuildFailed
    } else {
        last.map_or(GameWait::Stopped, GameWait::Exited)
    }
}

/// Say where the game is while the panel has no frame to show, offer Play or
/// Restart, and report a first frame that took unusually long.
fn update_game_panel_idle_label(
    play_state: Option<Res<State<PlayState>>>,
    launched: crate::pie::LaunchedWindowMode,
    stream: Res<LiveFrameStream>,
    request: Option<Res<crate::live_frame_view::StreamRequest>>,
    session: Option<NonSend<crate::pie::PieSession>>,
    last_exit: Option<Res<LastGameExit>>,
    mut loading_since: Local<Option<Instant>>,
    mut labels: Query<&mut Text, With<GamePanelIdleLabel>>,
    mut stderr_texts: Query<&mut Text, (With<GamePanelStderrText>, Without<GamePanelIdleLabel>)>,
    mut parts: Query<(&GameIdlePart, &mut Node)>,
    mut restart_calls: Query<(&GameIdlePart, &mut ButtonOperatorCall)>,
    added: Query<(), Added<GameIdlePart>>,
    mut commands: Commands,
) {
    let running = play_state.is_some_and(|state| *state.get() != PlayState::Stopped);
    let windowed = launched.get() == PieWindowMode::Windowed;
    if !running || windowed || stream.game_has_own_window {
        *loading_since = None;
    } else if stream.has_frame() {
        if let Some(waited) = loading_since.take().map(|since| since.elapsed())
            && waited >= SLOW_FIRST_FRAME
        {
            commands.queue(move |world: &mut World| {
                crate::status_bar::notify_info(
                    world,
                    format!(
                        "The game took {} s to show its first frame",
                        waited.as_secs()
                    ),
                );
            });
        }
    } else if request.is_some_and(|request| request.is_requested()) {
        loading_since.get_or_insert_with(Instant::now);
    }

    let exit = last_exit.as_ref().and_then(|last| last.0.as_ref());
    let wait = if running {
        if windowed {
            GameWait::Windowed
        } else if stream.game_has_own_window {
            GameWait::OwnWindow
        } else {
            GameWait::Loading(loading_since.map_or(Duration::ZERO, |since| since.elapsed()))
        }
    } else {
        stopped_wait(session.as_deref(), exit.map(|exit| &exit.reason))
    };
    let message = idle_message(wait);
    for mut text in &mut labels {
        if text.0 != message {
            text.0.clone_from(&message);
        }
    }
    if last_exit.as_ref().is_some_and(Res::is_changed) || !added.is_empty() {
        let stderr = exit.map(|exit| exit.stderr.join("\n")).unwrap_or_default();
        for mut text in &mut stderr_texts {
            if text.0 != stderr {
                text.0.clone_from(&stderr);
            }
        }
        if let Some(exit) = exit {
            for (part, mut call) in &mut restart_calls {
                if *part == GameIdlePart::Restart {
                    *call = ButtonOperatorCall::new(PieToggleInstanceOp::ID)
                        .with_param("config", exit.key.config.clone())
                        .with_param("instance", i64::from(exit.key.instance));
                }
            }
        }
    }
    let has_stderr = exit.is_some_and(|exit| !exit.stderr.is_empty());
    for (part, mut node) in &mut parts {
        let display = if idle_part_shown(*part, wait, has_stderr, windowed) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

/// Bring the Game panel to the front when an embedded launch starts, from its
/// build on, and put back the tab its dock showed before once play stops, if
/// the Game panel is still the one in front. A launch that ends in a crash, a
/// failed connection or a failed build leaves the panel in front with its
/// message. The game only streams frames while the panel is laid out, so a
/// hidden panel would leave the game unseen.
fn surface_game_panel_on_play(world: &mut World) {
    let state = world
        .get_resource::<State<PlayState>>()
        .map_or(PlayState::Stopped, |state| state.get().clone());
    let session = world.get_non_send::<crate::pie::PieSession>();
    let running = crate::pie::play_phase(&state, session) != crate::pie::PlayPhase::Stopped;
    let failed = session.is_some_and(crate::pie::PieSession::launch_failed)
        || world
            .get_resource::<LastGameExit>()
            .is_some_and(|last| last.0.is_some());
    let embedded = crate::pie::effective_window_mode_in(world) == PieWindowMode::Embedded;
    let Some((shown, held)) = world
        .get_resource::<GamePanelSurfaced>()
        .map(|surfaced| (surfaced.shown, surfaced.failed))
    else {
        return;
    };
    if !world.contains_resource::<DockTree>() {
        return;
    }
    if running && !embedded && shown {
        *world.resource_mut::<GamePanelSurfaced>() = GamePanelSurfaced::default();
    } else if running && embedded && (!shown || held) {
        let previous = front_tab_beside_game(world.resource::<DockTree>());
        crate::open_window_in_default_area_if_absent(world, GAME_WINDOW_ID);
        let mut surfaced = world.resource_mut::<GamePanelSurfaced>();
        if !shown {
            surfaced.previous = previous;
        }
        surfaced.shown = true;
        surfaced.failed = false;
    } else if !running && shown && failed {
        world.resource_mut::<GamePanelSurfaced>().failed = true;
    } else if !running && shown {
        let previous = std::mem::take(&mut *world.resource_mut::<GamePanelSurfaced>()).previous;
        let mut tree = world.resource_mut::<DockTree>();
        if let Some((leaf, tab)) = previous
            && front_window(&tree, leaf).as_deref() == Some(GAME_WINDOW_ID)
        {
            tree.set_active(leaf, tab);
        }
    }
}

/// The dock leaf holding the Game panel and the tab in front of it, when
/// that tab is another window's.
fn front_tab_beside_game(tree: &DockTree) -> Option<(NodeId, TabId)> {
    let leaf_id = tree.find_leaf_with_window(GAME_WINDOW_ID)?;
    let leaf = tree.get(leaf_id)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .any(|(window, tab)| tab == active && window != GAME_WINDOW_ID)
        .then_some((leaf_id, active))
}

/// The window whose tab is in front of `leaf`.
fn front_window(tree: &DockTree, leaf: NodeId) -> Option<String> {
    let leaf = tree.get(leaf)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
}

/// Letterbox the frame image inside the surface.
fn layout_game_panel_image(
    stream: Res<LiveFrameStream>,
    surfaces: Query<&ComputedNode, With<GamePanelSurface>>,
    mut images: Query<&mut Node, With<GamePanelImage>>,
) {
    let stream_size = stream.size.as_vec2();
    if stream_size.x < 1.0 || stream_size.y < 1.0 {
        return;
    }
    let Ok(computed) = surfaces.single() else {
        return;
    };
    let panel = computed.size() * computed.inverse_scale_factor();
    if panel.x < 1.0 || panel.y < 1.0 {
        return;
    }
    let (size, _offset) = contain_fit(panel, stream_size);
    let width = Val::Px(size.x);
    let height = Val::Px(size.y);
    for mut node in &mut images {
        if node.width != width {
            node.width = width;
        }
        if node.height != height {
            node.height = height;
        }
    }
}

/// A stream that showed frames and has stopped. A game still on its way to its
/// first frame is loading, which the idle label says.
fn went_quiet(stream: &LiveFrameStream) -> bool {
    stream.has_frame() && !stream.is_fresh()
}

/// Drive the no-signal chip, the playing chip, and the capture accent border.
fn update_game_panel_chips(
    capture: Res<crate::live_input::LiveInputCapture>,
    stream: Res<LiveFrameStream>,
    instances: Res<crate::pie::PieInstances>,
    mut no_signal: Query<&mut Node, With<GameNoSignalChip>>,
    mut playing: Query<&mut Node, (With<GamePlayingChip>, Without<GameNoSignalChip>)>,
    mut roots: Query<&mut BorderColor, With<GamePanel>>,
) {
    let no_signal_display = if instances.focused.is_some() && went_quiet(&stream) {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut no_signal {
        if node.display != no_signal_display {
            node.display = no_signal_display;
        }
    }
    let playing_display = if capture.active {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut playing {
        if node.display != playing_display {
            node.display = playing_display;
        }
    }
    let border = if capture.active {
        BorderColor::all(crate::default_style::CAPTURE_ACCENT)
    } else {
        BorderColor::all(Color::NONE)
    };
    for mut root in &mut roots {
        if *root != border {
            *root = border;
        }
    }
}

pub struct GamePanelPlugin;

impl Plugin for GamePanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GamePanelSurfaced>()
            .add_systems(Update, surface_game_panel_on_play)
            .add_systems(
                Update,
                (
                    sync_game_panel_image,
                    update_game_panel_idle_label,
                    layout_game_panel_image,
                    update_game_panel_chips,
                    update_game_instance_button,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_fit_letterboxes_and_centers() {
        let (size, offset) = contain_fit(Vec2::new(800.0, 600.0), Vec2::new(1600.0, 1200.0));
        assert_eq!(size, Vec2::new(800.0, 600.0));
        assert_eq!(offset, Vec2::ZERO);
        let (size, offset) = contain_fit(Vec2::new(600.0, 600.0), Vec2::new(1200.0, 600.0));
        assert_eq!(size, Vec2::new(600.0, 300.0));
        assert_eq!(offset, Vec2::new(0.0, 150.0));
        let (size, offset) = contain_fit(Vec2::new(400.0, 800.0), Vec2::new(800.0, 800.0));
        assert_eq!(size, Vec2::new(400.0, 400.0));
        assert_eq!(offset, Vec2::new(0.0, 200.0));
    }

    #[test]
    fn surface_click_without_fresh_stream_is_a_noop() {
        let mut world = World::new();
        world.init_resource::<crate::live_input::LiveInputCapture>();
        world.init_resource::<crate::live_input::PendingForwards>();
        let surface = world.spawn_empty().id();
        // No LiveFrameStream resource: not fresh, so the click does nothing.
        surface_clicked(&mut world, surface);
        assert!(
            !world
                .resource::<crate::live_input::LiveInputCapture>()
                .active
        );
    }

    /// Only a game that said it has its own window is told to change main.rs.
    #[test]
    fn a_busy_game_reads_as_loading_and_is_not_blamed() {
        let loading = idle_message(GameWait::Loading(Duration::from_secs(5)));
        assert_eq!(loading, IDLE_LOADING);
        assert!(!loading.contains("maybe_windowless"));

        let long = idle_message(GameWait::Loading(Duration::from_secs(200)));
        assert!(long.contains("200 s"), "{long}");
        assert!(!long.contains("maybe_windowless"));

        assert!(idle_message(GameWait::OwnWindow).contains("maybe_windowless"));
        assert_eq!(
            idle_message(GameWait::Connecting(Duration::from_secs(1))),
            IDLE_STARTING
        );
        let unconnected = idle_message(GameWait::Connecting(SLOW_CONNECT));
        assert!(unconnected.contains("JackdawPlugin"), "{unconnected}");
        assert!(!unconnected.contains("oading"), "{unconnected}");
        assert_eq!(idle_message(GameWait::Building), IDLE_BUILDING);
        assert_eq!(idle_message(GameWait::Windowed), IDLE_WINDOWED);
    }

    const PARTS: [GameIdlePart; 6] = [
        GameIdlePart::Label,
        GameIdlePart::Stderr,
        GameIdlePart::Play,
        GameIdlePart::Restart,
        GameIdlePart::BuildErrors,
        GameIdlePart::Hint,
    ];

    fn shown(wait: GameWait, has_stderr: bool) -> Vec<GameIdlePart> {
        PARTS
            .into_iter()
            .filter(|part| idle_part_shown(*part, wait, has_stderr, false))
            .collect()
    }

    #[test]
    fn a_game_in_its_own_window_gets_play_without_the_input_hint() {
        let parts: Vec<GameIdlePart> = PARTS
            .into_iter()
            .filter(|part| idle_part_shown(*part, GameWait::Stopped, false, true))
            .collect();
        assert_eq!(parts, [GameIdlePart::Play]);
    }

    #[test]
    fn stopped_offers_play_and_the_input_hint() {
        assert_eq!(
            shown(GameWait::Stopped, false),
            [GameIdlePart::Play, GameIdlePart::Hint]
        );
        assert_eq!(stopped_wait(None, None), GameWait::Stopped);
    }

    #[test]
    fn a_crashed_game_shows_its_code_and_stderr_with_restart() {
        let crashed = GameExitReason::Crashed("code 101".to_string());
        assert_eq!(
            idle_message(GameWait::Exited(&crashed)),
            "Game exited (code 101)"
        );
        assert_eq!(
            shown(GameWait::Exited(&crashed), true),
            [
                GameIdlePart::Label,
                GameIdlePart::Stderr,
                GameIdlePart::Restart
            ]
        );
        assert_eq!(
            shown(GameWait::Exited(&crashed), false),
            [GameIdlePart::Label, GameIdlePart::Restart]
        );
        let early = GameExitReason::ExitedBeforeConnecting("signal 9".to_string());
        assert_eq!(
            idle_message(GameWait::Exited(&early)),
            "Game exited before connecting (signal 9)"
        );
        assert_eq!(
            stopped_wait(None, Some(&crashed)),
            GameWait::Exited(&crashed)
        );
    }

    #[test]
    fn a_game_that_never_connected_is_told_to_add_jackdaw_plugin() {
        let message = idle_message(GameWait::Exited(&GameExitReason::NeverConnected));
        assert!(message.contains("JackdawPlugin"), "{message}");
        assert!(message.contains("pie feature"), "{message}");
        assert_eq!(
            idle_action(GameWait::Exited(&GameExitReason::NeverConnected)),
            IdleAction::Restart
        );
    }

    #[test]
    fn a_failed_build_offers_its_errors() {
        assert_eq!(idle_message(GameWait::BuildFailed), IDLE_BUILD_FAILED);
        assert_eq!(
            shown(GameWait::BuildFailed, false),
            [GameIdlePart::Label, GameIdlePart::BuildErrors]
        );
    }

    #[test]
    fn a_launch_under_way_wins_over_the_last_exit() {
        let crashed = GameExitReason::Crashed("code 1".to_string());
        let mut session = crate::pie::PieSession::default();
        session.insert_test_pending_build(crate::pie::InstanceKey {
            config: "Game".to_string(),
            instance: 1,
        });
        assert_eq!(
            stopped_wait(Some(&session), Some(&crashed)),
            GameWait::Building
        );
        session.fail_test_builds();
        assert_eq!(
            stopped_wait(Some(&session), Some(&crashed)),
            GameWait::BuildFailed
        );
        assert_eq!(
            shown(GameWait::Building, false),
            [GameIdlePart::Label],
            "building keeps its message and offers nothing"
        );
    }

    #[test]
    fn only_a_stream_that_showed_frames_can_go_quiet() {
        let mut stream = LiveFrameStream::default();
        assert!(!went_quiet(&stream), "still loading");

        let mut images = Assets::<Image>::default();
        stream.image = Some(images.add(Image::default()));
        assert!(went_quiet(&stream));

        stream.received_at = Some(Instant::now());
        assert!(!went_quiet(&stream));
    }

    #[test]
    fn panel_to_stream_inverts_the_fit() {
        let panel = Vec2::new(600.0, 600.0);
        let stream = Vec2::new(1200.0, 600.0);
        assert_eq!(
            panel_to_stream(Vec2::new(300.0, 300.0), panel, stream),
            Some(Vec2::new(600.0, 300.0))
        );
        assert_eq!(
            panel_to_stream(Vec2::new(300.0, 10.0), panel, stream),
            Some(Vec2::new(600.0, 0.0))
        );
        assert_eq!(panel_to_stream(Vec2::ZERO, Vec2::ZERO, stream), None);
    }
}
