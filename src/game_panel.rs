//! The Game panel: a dockable surface showing the focused instance's
//! streamed frame. A Play/Select mode bar decides what a click means: Play
//! engages input capture (the frame is the running game), Select asks the
//! game what is under the cursor and selects it in the Live tree. The panel
//! is a pure monitor: no editor camera, no overlay, no compositing.

use bevy::{prelude::*, ui_widgets::observe};
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens;

use crate::live_frame::LiveFrameStream;

/// Dock window id, used for registration and presence checks.
pub const GAME_WINDOW_ID: &str = "jackdaw.game";

/// What a click in the Game panel does.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePanelMode {
    /// Clicks engage input capture; the game is playable.
    #[default]
    Play,
    /// Game input is off; clicks pick the entity under the cursor.
    Select,
}

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

/// Marker on the idle label shown while no frame is streaming.
#[derive(Component)]
pub struct GamePanelIdleLabel;

/// Marker on one Play/Select segment.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameModeSegment {
    Play,
    Select,
}

/// Marker on the "no signal" chip shown while an instance is focused but no
/// fresh frame has arrived.
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

/// Route a click on the frame surface by mode. Play engages capture; Select
/// forwards the cursor and asks the game to pick. Clicks with no focused
/// fresh stream do nothing (the idle label is showing).
fn surface_clicked(world: &mut World, surface: Entity, window_pos: Vec2) {
    let fresh = world
        .get_resource::<LiveFrameStream>()
        .is_some_and(LiveFrameStream::is_fresh);
    if !fresh {
        return;
    }
    match *world.resource::<GamePanelMode>() {
        GamePanelMode::Play => {
            if !world
                .resource::<crate::live_input::LiveInputCapture>()
                .active
            {
                crate::live_input::engage_capture(world, surface);
            }
        }
        GamePanelMode::Select => {
            if let Some(position) = surface_to_stream(world, surface, window_pos) {
                crate::pie::send_control_to_focused(
                    world,
                    jackdaw_pie_protocol::ControlEvent::Input(
                        jackdaw_pie_protocol::PieInputEvent::CursorMoved { position },
                    ),
                );
                crate::pie::send_control_to_focused(
                    world,
                    jackdaw_pie_protocol::ControlEvent::Pick,
                );
            }
        }
    }
}

/// Window-logical position to stream pixels via the surface's letterbox.
fn surface_to_stream(world: &mut World, surface: Entity, window_pos: Vec2) -> Option<Vec2> {
    let stream = world.get_resource::<LiveFrameStream>()?.size.as_vec2();
    let computed = world.get::<ComputedNode>(surface)?;
    let transform = world.get::<bevy::ui::UiGlobalTransform>(surface)?;
    let (top_left, panel) = surface_remap(computed, transform);
    panel_to_stream(window_pos - top_left, panel, stream)
}

/// Build the panel content: a header with the Play/Select mode bar, signal
/// and playing chips, and a Play-input button; below it a letterbox surface
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
                    game_mode_bar(),
                    game_no_signal_chip(),
                    game_playing_chip(),
                    game_play_input_button(),
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
                observe(|click: On<Pointer<Click>>, mut commands: Commands| {
                    let surface = click.event_target();
                    let window_pos = click.pointer_location.position;
                    commands.queue(move |world: &mut World| {
                        surface_clicked(world, surface, window_pos);
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
                    (
                        GamePanelIdleLabel,
                        Text::new("not playing"),
                        TextFont {
                            font_size: tokens::FONT_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_DISABLED),
                    ),
                ],
            ),
        ],
    )
}

/// Build the two-segment Play/Select mode bar.
fn game_mode_bar() -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            overflow: Overflow::clip(),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            game_mode_segment(
                GameModeSegment::Play,
                "Play",
                "Play: clicks and keys go to the game (Shift+Esc releases)",
            ),
            game_mode_segment(
                GameModeSegment::Select,
                "Select",
                "Select: click the frame to inspect the entity under the cursor",
            ),
        ],
    )
}

/// One clickable segment inside the Play/Select mode bar.
fn game_mode_segment(
    segment: GameModeSegment,
    label: &'static str,
    tooltip: &'static str,
) -> impl Bundle {
    (
        segment,
        Interaction::default(),
        jackdaw_feathers::tooltip::Tooltip::title(tooltip),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(move |world: &mut World| {
                let next = match segment {
                    GameModeSegment::Play => GamePanelMode::Play,
                    GameModeSegment::Select => GamePanelMode::Select,
                };
                if next == GamePanelMode::Select
                    && world
                        .resource::<crate::live_input::LiveInputCapture>()
                        .active
                {
                    world
                        .resource_mut::<crate::live_input::LiveInputCapture>()
                        .release_requested = true;
                    crate::live_input::apply_release_requests(world);
                    crate::live_input::flush_forwards(world);
                }
                *world.resource_mut::<GamePanelMode>() = next;
            });
        }),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Build the "no signal" chip, shown while an instance is focused but no
/// fresh frame has arrived.
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
                font_size: tokens::FONT_SM,
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
                font_size: tokens::FONT_SM,
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
        Interaction::default(),
        jackdaw_feathers::tooltip::Tooltip::title(
            "Forward keyboard and mouse to the running game (Shift+Esc releases)",
        ),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(crate::live_input::PiePlayInputToggleOp::ID)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: false,
                })
                .call();
        }),
        children![(
            Text::new("Play Input"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Point the frame image at the current stream and show it when fresh, else
/// show the idle label.
fn sync_game_panel_image(
    stream: Res<LiveFrameStream>,
    mut images: Query<(&mut ImageNode, &mut Node), With<GamePanelImage>>,
    mut idle: Query<&mut Node, (With<GamePanelIdleLabel>, Without<GamePanelImage>)>,
) {
    let fresh = stream.is_fresh();
    let handle = stream.image.clone();
    let show_image = fresh && handle.is_some();
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

/// Highlight the active mode segment.
fn update_game_mode_bar(
    mode: Res<GamePanelMode>,
    mut segments: Query<(&GameModeSegment, &mut BackgroundColor)>,
) {
    for (segment, mut bg) in &mut segments {
        let is_active = match segment {
            GameModeSegment::Play => *mode == GamePanelMode::Play,
            GameModeSegment::Select => *mode == GamePanelMode::Select,
        };
        let color = if is_active {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            Color::NONE
        };
        if bg.0 != color {
            bg.0 = color;
        }
    }
}

/// Drive the no-signal chip, the playing chip, and the capture accent border.
fn update_game_panel_chips(
    capture: Res<crate::live_input::LiveInputCapture>,
    stream: Res<LiveFrameStream>,
    instances: Res<crate::pie_mirror::PieInstances>,
    mut no_signal: Query<&mut Node, With<GameNoSignalChip>>,
    mut playing: Query<&mut Node, (With<GamePlayingChip>, Without<GameNoSignalChip>)>,
    mut roots: Query<&mut BorderColor, With<GamePanel>>,
) {
    let no_signal_display = if instances.focused.is_some() && !stream.is_fresh() {
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

/// While Select mode is active, forward the cursor position over the surface
/// so the game's picking backend tracks hover before a pick click.
fn forward_select_hover(
    mode: Res<GamePanelMode>,
    stream: Option<Res<LiveFrameStream>>,
    mut cursor: MessageReader<bevy::window::CursorMoved>,
    surfaces: Query<(&ComputedNode, &bevy::ui::UiGlobalTransform), With<GamePanelSurface>>,
    mut pending: ResMut<crate::live_input::PendingForwards>,
) {
    if *mode != GamePanelMode::Select {
        cursor.clear();
        return;
    }
    let Some(stream) = stream.filter(|s| s.is_fresh()) else {
        cursor.clear();
        return;
    };
    let Ok((computed, transform)) = surfaces.single() else {
        cursor.clear();
        return;
    };
    let (top_left, panel) = surface_remap(computed, transform);
    for moved in cursor.read() {
        let local = moved.position - top_left;
        if local.x < 0.0 || local.y < 0.0 || local.x > panel.x || local.y > panel.y {
            continue;
        }
        if let Some(position) = panel_to_stream(local, panel, stream.size.as_vec2()) {
            pending
                .0
                .push(jackdaw_pie_protocol::PieInputEvent::CursorMoved { position });
        }
    }
}

pub struct GamePanelPlugin;

impl Plugin for GamePanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GamePanelMode>().add_systems(
            Update,
            (
                sync_game_panel_image,
                layout_game_panel_image,
                update_game_mode_bar,
                update_game_panel_chips,
                forward_select_hover,
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
        world.init_resource::<GamePanelMode>();
        world.init_resource::<crate::live_input::LiveInputCapture>();
        world.init_resource::<crate::live_input::PendingForwards>();
        let surface = world.spawn_empty().id();
        // No LiveFrameStream resource: not fresh, so the click does nothing.
        surface_clicked(&mut world, surface, Vec2::new(10.0, 10.0));
        assert!(
            !world
                .resource::<crate::live_input::LiveInputCapture>()
                .active
        );
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
