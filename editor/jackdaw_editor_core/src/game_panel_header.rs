//! The Game panel's header: the view menu (size, mute and output), the instance
//! picker shown while several instances run, the "no signal" and "muted"
//! chips and the pop-out button.

use std::time::Duration;

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, observe};
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::{
    ButtonContentText, ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button,
};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::popover::{EditorPopover, PopoverPlacement, PopoverProps, popover};
use jackdaw_feathers::separator::{SeparatorProps, separator};
use jackdaw_feathers::tokens;
use jackdaw_pie_protocol::manifest::RunConfig;

use crate::live_input::LiveInputCapture;
use crate::pie::{
    InstanceKey, PieFocusInstanceOp, PieInstances, PiePopOutOp, PieSession, PieWindowMode,
};
use crate::play_settings::{GameView, PieMuteOp, PieViewAspectOp, PlaySettings};
use crate::run_config::RunConfigs;

/// How long the release hint shows over the panel once capture engages.
pub const CAPTURE_FLASH: Duration = Duration::from_secs(2);

/// The release hint flashed over the panel when capture engages.
pub const CAPTURE_FLASH_TEXT: &str = "Shift+Esc to release";

/// Marker on the button opening the view menu, captioned with the view size.
#[derive(Component)]
pub struct GameViewButton;

/// Marker on the button opening the instance list, captioned with the
/// instance on show. Shown while more than one instance runs.
#[derive(Component)]
pub struct GameInstancePicker;

/// Marker on the button moving the game between the panel and its own window.
#[derive(Component)]
pub struct GamePopOutButton;

/// Marker on the "muted" chip.
#[derive(Component)]
pub struct GameMutedChip;

/// Marker on the release hint flashed over the panel when capture engages.
#[derive(Component)]
pub struct GameCaptureFlash;

/// Marker on a row of an open header menu; activating one closes the menu.
#[derive(Component)]
pub struct GameHeaderMenuItem;

/// The header menu that is open, if any, and the button it opened under.
#[derive(Resource, Default)]
struct GameHeaderMenu {
    popover: Option<Entity>,
    anchor: Option<Entity>,
}

/// What an instance is called in the picker: its run configuration's label,
/// with its number when that configuration runs more than one.
pub fn instance_name(key: &InstanceKey, instances_of_config: u32) -> String {
    if instances_of_config > 1 {
        format!("{} {}", key.config, key.instance)
    } else {
        key.config.clone()
    }
}

/// The running instances in run configuration order, each with its name. An
/// instance no configuration names comes last, by its key.
pub fn named_instances(runs: &[RunConfig], live: &[InstanceKey]) -> Vec<(InstanceKey, String)> {
    let mut named = Vec::new();
    for run in runs {
        for instance in 1..=run.instances {
            let key = InstanceKey {
                config: run.label().to_string(),
                instance,
            };
            if live.contains(&key) {
                let name = instance_name(&key, run.instances);
                named.push((key, name));
            }
        }
    }
    let mut rest: Vec<&InstanceKey> = live
        .iter()
        .filter(|key| !named.iter().any(|(held, _)| held == *key))
        .collect();
    rest.sort_by(|a, b| a.config.cmp(&b.config).then(a.instance.cmp(&b.instance)));
    named.extend(rest.into_iter().map(|key| (key.clone(), key.to_string())));
    named
}

/// Build the header row.
pub(crate) fn game_panel_header() -> impl Bundle {
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
            (
                GameViewButton,
                jackdaw_feathers::tooltip::Tooltip::title("Game view")
                    .with_description("The size the game renders at, and its sound."),
                button(
                    ButtonProps::new(GameView::Free.label())
                        .with_variant(ButtonVariant::Ghost)
                        .with_right_icon(Icon::ChevronDown),
                ),
                observe(
                    |click: On<PointerClick>,
                     popovers: Query<(), With<EditorPopover>>,
                     parents: Query<&ChildOf>,
                     mut commands: Commands| {
                        let Some(anchor) = menu_trigger(&click, &popovers, &parents) else {
                            return;
                        };
                        commands.queue(move |world: &mut World| {
                            let rows = view_menu_rows(world);
                            toggle_menu(world, anchor, rows);
                        });
                    }
                ),
            ),
            (
                GameInstancePicker,
                jackdaw_feathers::tooltip::Tooltip::title("Running instance")
                    .with_description("The instance the Game panel shows."),
                button(
                    ButtonProps::new(" ")
                        .with_variant(ButtonVariant::Ghost)
                        .with_right_icon(Icon::ChevronDown)
                        .hidden(),
                ),
                observe(
                    |click: On<PointerClick>,
                     popovers: Query<(), With<EditorPopover>>,
                     parents: Query<&ChildOf>,
                     mut commands: Commands| {
                        let Some(anchor) = menu_trigger(&click, &popovers, &parents) else {
                            return;
                        };
                        commands.queue(move |world: &mut World| {
                            let rows = instance_menu_rows(world);
                            toggle_menu(world, anchor, rows);
                        });
                    }
                ),
            ),
            crate::game_panel::game_no_signal_chip(),
            (
                GameMutedChip,
                chip_node(),
                BackgroundColor(tokens::ELEVATED_BG),
                children![(
                    Text::new("muted"),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                )],
            ),
            Node {
                flex_grow: 1.0,
                ..Default::default()
            },
            (
                GamePopOutButton,
                button(
                    ButtonProps::new("Pop out")
                        .with_size(ButtonSize::Icon)
                        .with_variant(ButtonVariant::Ghost)
                        .with_left_icon(Icon::SquareArrowOutUpRight)
                        .call_operator(PiePopOutOp::ID),
                ),
            ),
        ],
    )
}

/// The node of a small header chip, hidden until its state is reached.
pub(crate) fn chip_node() -> Node {
    Node {
        align_items: AlignItems::Center,
        padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
        border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
        display: Display::None,
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// The release hint, centred over the frame.
pub(crate) fn game_capture_flash() -> impl Bundle {
    (
        GameCaptureFlash,
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
            ..Default::default()
        },
        BackgroundColor(tokens::MENU_BG),
        Pickable::IGNORE,
        children![(
            Text::new(CAPTURE_FLASH_TEXT),
            TextFont {
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            TextColor(crate::default_style::CAPTURE_ACCENT),
            Pickable::IGNORE,
        )],
    )
}

/// The trigger a click on a header menu button opens a menu under, or `None`
/// for a click inside the open menu, which hangs off the trigger.
pub(crate) fn menu_trigger(
    click: &On<PointerClick>,
    popovers: &Query<(), With<EditorPopover>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    if click.event().button != PointerButton::Primary {
        return None;
    }
    let trigger = click.event_target();
    let mut entity = click.original_event_target();
    while entity != trigger {
        if popovers.contains(entity) {
            return None;
        }
        entity = parents.get(entity).ok()?.parent();
    }
    Some(trigger)
}

/// One row of a header menu.
pub(crate) enum MenuRow {
    Check {
        label: String,
        checked: bool,
        call: ButtonOperatorCall,
    },
    Action {
        label: String,
        call: ButtonOperatorCall,
    },
    Separator,
}

fn view_menu_rows(world: &World) -> Vec<MenuRow> {
    let settings = world.get_resource::<PlaySettings>();
    let current = settings.map(|settings| settings.view).unwrap_or_default();
    let muted = settings.is_some_and(|settings| settings.muted);
    let mut rows: Vec<MenuRow> = GameView::ALL
        .into_iter()
        .map(|view| MenuRow::Check {
            label: view.label().to_string(),
            checked: view == current,
            call: ButtonOperatorCall::new(PieViewAspectOp::ID).with_param("value", view.name()),
        })
        .collect();
    rows.push(MenuRow::Separator);
    rows.push(MenuRow::Check {
        label: "Mute".to_string(),
        checked: muted,
        call: ButtonOperatorCall::new(PieMuteOp::ID),
    });
    rows.push(MenuRow::Separator);
    rows.push(MenuRow::Action {
        label: "Show output".to_string(),
        call: ButtonOperatorCall::new(crate::WindowOpenOp::ID).with_param(
            "window_id",
            crate::game_output_window::GAME_OUTPUT_WINDOW_ID,
        ),
    });
    rows
}

fn instance_menu_rows(world: &World) -> Vec<MenuRow> {
    let focused = world
        .get_resource::<PieInstances>()
        .and_then(|instances| instances.focused.clone());
    let live = world
        .get_non_send::<PieSession>()
        .map(PieSession::live_keys)
        .unwrap_or_default();
    let runs = world
        .get_resource::<RunConfigs>()
        .map(|configs| configs.manifest.runs.as_slice())
        .unwrap_or_default();
    named_instances(runs, &live)
        .into_iter()
        .map(|(key, name)| MenuRow::Check {
            label: name,
            checked: focused.as_ref() == Some(&key),
            call: ButtonOperatorCall::new(PieFocusInstanceOp::ID)
                .with_param("config", key.config.clone())
                .with_param("instance", i64::from(key.instance)),
        })
        .collect()
}

/// Open a menu of `rows` under `anchor`, closing any open menu. A click on
/// the button whose menu is open only closes it.
pub(crate) fn toggle_menu(world: &mut World, anchor: Entity, rows: Vec<MenuRow>) {
    let (open, open_anchor) = {
        let mut menu = world.get_resource_or_init::<GameHeaderMenu>();
        (menu.popover.take(), menu.anchor.take())
    };
    if let Some(open) = open
        && world.get_entity(open).is_ok()
    {
        world.despawn(open);
        if open_anchor == Some(anchor) {
            return;
        }
    }
    let menu = world
        .spawn(popover(
            PopoverProps::new(anchor)
                .with_placement(PopoverPlacement::BottomStart)
                .with_padding(tokens::SPACING_XS)
                .with_gap(2.0)
                .with_z_index(1000)
                .with_node(Node {
                    min_width: px(180.0),
                    ..Default::default()
                }),
        ))
        .id();
    for row in rows {
        match row {
            MenuRow::Check {
                label,
                checked,
                call,
            } => {
                world.spawn((
                    GameHeaderMenuItem,
                    call,
                    button(
                        ButtonProps::new(label)
                            .with_variant(ButtonVariant::Ghost)
                            .align_left()
                            .with_left_checkbox(checked),
                    ),
                    ChildOf(menu),
                ));
            }
            MenuRow::Action { label, call } => {
                world.spawn((
                    GameHeaderMenuItem,
                    call,
                    button(
                        ButtonProps::new(label)
                            .with_variant(ButtonVariant::Ghost)
                            .align_left(),
                    ),
                    ChildOf(menu),
                ));
            }
            MenuRow::Separator => {
                world.spawn((separator(SeparatorProps::horizontal()), ChildOf(menu)));
            }
        }
    }
    let mut held = world.resource_mut::<GameHeaderMenu>();
    held.popover = Some(menu);
    held.anchor = Some(anchor);
}

/// Close the header menu once one of its rows has run its operator.
fn close_menu_on_item_activate(
    activate: On<Activate>,
    items: Query<(), With<GameHeaderMenuItem>>,
    mut menu: ResMut<GameHeaderMenu>,
    popovers: Query<(), With<EditorPopover>>,
    mut commands: Commands,
) {
    if !items.contains(activate.entity) {
        return;
    }
    menu.anchor = None;
    if let Some(open) = menu.popover.take()
        && popovers.contains(open)
    {
        commands.entity(open).try_despawn();
    }
}

/// The caption of a header button, leaving out the menu it holds open.
fn own_caption(
    button: Entity,
    children: &Query<&Children>,
    captions: &Query<(), With<ButtonContentText>>,
    popovers: &Query<(), With<EditorPopover>>,
) -> Option<Entity> {
    let mut stack = vec![button];
    while let Some(entity) = stack.pop() {
        if captions.contains(entity) {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter().filter(|kid| !popovers.contains(*kid)));
        }
    }
    None
}

/// Caption the view button with the view size and show the muted chip.
fn sync_view_controls(
    settings: Res<PlaySettings>,
    buttons: Query<Entity, With<GameViewButton>>,
    mut chips: Query<&mut Node, With<GameMutedChip>>,
    children: Query<&Children>,
    captions: Query<(), With<ButtonContentText>>,
    popovers: Query<(), With<EditorPopover>>,
    mut texts: Query<&mut Text>,
    added: Query<(), Added<ButtonContentText>>,
) {
    if !settings.is_changed() && added.is_empty() {
        return;
    }
    let label = settings.view.label();
    for button in &buttons {
        if let Some(caption) = own_caption(button, &children, &captions, &popovers)
            && let Ok(mut text) = texts.get_mut(caption)
            && text.0 != label
        {
            label.clone_into(&mut text.0);
        }
    }
    let display = if settings.muted {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut chips {
        if node.display != display {
            node.display = display;
        }
    }
}

/// The running instances and the focused one, as the picker last showed them.
type PickerShown = (Vec<InstanceKey>, Option<InstanceKey>);

/// Show the instance picker with several instances running, captioned with
/// the one on show. Runs its update only when the running instances, the
/// focus or the run configurations change.
fn sync_instance_picker(
    session: Option<NonSend<PieSession>>,
    configs: Option<Res<RunConfigs>>,
    instances: Option<Res<PieInstances>>,
    mut pickers: Query<(Entity, &mut Node), With<GameInstancePicker>>,
    children: Query<&Children>,
    captions: Query<(), With<ButtonContentText>>,
    popovers: Query<(), With<EditorPopover>>,
    mut texts: Query<&mut Text>,
    added: Query<(), Added<ButtonContentText>>,
    mut shown: Local<Option<PickerShown>>,
) {
    let mut live = session
        .as_deref()
        .map(PieSession::live_keys)
        .unwrap_or_default();
    live.sort_by(|a, b| a.config.cmp(&b.config).then(a.instance.cmp(&b.instance)));
    let focused = instances
        .as_deref()
        .and_then(|instances| instances.focused.clone());
    let configs_changed = configs.as_ref().is_some_and(Res::is_changed);
    let now = (live, focused);
    if shown.as_ref() == Some(&now) && !configs_changed && added.is_empty() {
        return;
    }
    let (live, focused) = shown.insert(now);
    let several = live.len() > 1;
    let display = if several {
        Display::Flex
    } else {
        Display::None
    };
    let label = several
        .then(|| {
            let runs = configs
                .as_deref()
                .map(|configs| configs.manifest.runs.as_slice())
                .unwrap_or_default();
            let focused = focused.as_ref()?;
            named_instances(runs, live)
                .into_iter()
                .find(|(key, _)| key == focused)
                .map(|(_, name)| name)
        })
        .flatten()
        .unwrap_or_default();
    for (picker, mut node) in &mut pickers {
        if node.display != display {
            node.display = display;
        }
        if let Some(caption) = own_caption(picker, &children, &captions, &popovers)
            && let Ok(mut text) = texts.get_mut(caption)
            && text.0 != label
            && several
        {
            text.0.clone_from(&label);
        }
    }
}

/// Show the pop-out button pressed while launches open their own window.
fn sync_pop_out_button(
    mode: Res<PieWindowMode>,
    mut buttons: Query<&mut ButtonVariant, With<GamePopOutButton>>,
    added: Query<(), Added<GamePopOutButton>>,
) {
    if !mode.is_changed() && added.is_empty() {
        return;
    }
    let variant = match *mode {
        PieWindowMode::Windowed => ButtonVariant::ActiveAlt,
        PieWindowMode::Embedded => ButtonVariant::Ghost,
    };
    for mut current in &mut buttons {
        if *current != variant {
            *current = variant;
        }
    }
}

/// Whether the release hint shows: for [`CAPTURE_FLASH`] after capture
/// engages, while it holds.
pub fn capture_flash_shown(capture: &LiveInputCapture) -> bool {
    capture.active
        && capture
            .engaged_at
            .is_some_and(|at| at.elapsed() < CAPTURE_FLASH)
}

fn sync_capture_flash(
    capture: Res<LiveInputCapture>,
    mut flashes: Query<&mut Node, With<GameCaptureFlash>>,
    added: Query<(), Added<GameCaptureFlash>>,
    mut was_shown: Local<bool>,
) {
    let shown = capture_flash_shown(&capture);
    if shown == *was_shown && added.is_empty() {
        return;
    }
    *was_shown = shown;
    let display = if shown { Display::Flex } else { Display::None };
    for mut node in &mut flashes {
        if node.display != display {
            node.display = display;
        }
    }
}

pub struct GamePanelHeaderPlugin;

impl Plugin for GamePanelHeaderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameHeaderMenu>()
            .add_observer(close_menu_on_item_activate)
            .add_systems(
                Update,
                (
                    sync_view_controls.run_if(resource_exists::<PlaySettings>),
                    sync_instance_picker,
                    sync_pop_out_button.run_if(resource_exists::<PieWindowMode>),
                    sync_capture_flash,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(config: &str, instance: u32) -> InstanceKey {
        InstanceKey {
            config: config.to_string(),
            instance,
        }
    }

    fn run(name: &str, instances: u32) -> RunConfig {
        RunConfig {
            name: Some(name.to_string()),
            instances,
            ..Default::default()
        }
    }

    #[test]
    fn an_instance_is_numbered_only_when_its_config_runs_several() {
        assert_eq!(instance_name(&key("Server", 1), 1), "Server");
        assert_eq!(instance_name(&key("Client", 2), 2), "Client 2");
    }

    #[test]
    fn running_instances_list_in_config_order_with_their_names() {
        let runs = [run("Server", 1), run("Client", 2)];
        let live = [key("Client", 2), key("Server", 1), key("Client", 1)];
        let names: Vec<String> = named_instances(&runs, &live)
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        assert_eq!(names, ["Server", "Client 1", "Client 2"]);
    }

    #[test]
    fn an_instance_no_config_names_still_lists() {
        let names: Vec<String> = named_instances(&[run("Server", 1)], &[key("Old", 1)])
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        assert_eq!(names, ["Old #1"]);
    }

    #[test]
    fn the_release_hint_shows_briefly_after_capture_engages() {
        let mut capture = LiveInputCapture {
            active: true,
            engaged_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        assert!(capture_flash_shown(&capture));
        capture.engaged_at = std::time::Instant::now().checked_sub(CAPTURE_FLASH);
        assert!(!capture_flash_shown(&capture));
        capture.engaged_at = Some(std::time::Instant::now());
        capture.active = false;
        assert!(!capture_flash_shown(&capture));
    }
}
