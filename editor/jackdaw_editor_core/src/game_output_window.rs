//! Game Output window: a read-only bottom-dock panel listing what a game
//! instance printed, from [`GameOutput`]. Rows are only appended as lines
//! arrive, and at most [`OUTPUT_ROWS`] are on show.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy::ui_widgets::observe;
use jackdaw_api::prelude::*;
use jackdaw_api::{
    DefaultArea,
    prelude::{ExtensionContext, ExtensionKind, JackdawExtension, WindowDescriptor},
};
use jackdaw_feathers::button::{
    ButtonContentText, ButtonOperatorCall, ButtonProps, ButtonVariant, button,
};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::popover::EditorPopover;
use jackdaw_feathers::text_edit::{self, TextEditProps, TextEditValue};
use jackdaw_feathers::tokens;

use crate::game_output::{GameOutput, OutputLine, OutputStream};
use crate::game_panel_header::{MenuRow, menu_trigger, named_instances, toggle_menu};
use crate::pie::{InstanceKey, PieInstances};
use crate::run_config::RunConfigs;

/// Dock-window id for the Game Output window.
pub const GAME_OUTPUT_WINDOW_ID: &str = "jackdaw.game_output";

/// How many lines the window shows at once.
pub const OUTPUT_ROWS: usize = 500;

const EMPTY_NO_GAME: &str = "No game output yet. Press Play to run the game.";
const EMPTY_NO_LINES: &str = "The game has not printed anything.";
const EMPTY_NO_MATCH: &str = "No lines match the filter.";

pub struct GameOutputWindowPlugin;

impl Plugin for GameOutputWindowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameOutputView>()
            .init_resource::<GameOutput>()
            .add_systems(
                Update,
                (
                    follow_focused_instance.run_if(resource_exists_and_changed::<PieInstances>),
                    apply_output_filter,
                    settle_output_scroll,
                    sync_game_output,
                )
                    .chain()
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// Which instance the window shows and the filter typed into it.
#[derive(Resource, Default, Debug)]
pub struct GameOutputView {
    /// The instance picked in the window or focused in the Game panel last.
    pub selected: Option<InstanceKey>,
    pub filter: String,
}

/// The instance whose output the window shows: the one picked, else the one
/// the Game panel shows, else the one launched last.
pub fn shown_instance(world: &World) -> Option<InstanceKey> {
    let output = world.get_resource::<GameOutput>()?;
    let selected = world
        .get_resource::<GameOutputView>()
        .and_then(|view| view.selected.clone());
    let focused = world
        .get_resource::<PieInstances>()
        .and_then(|instances| instances.focused.clone());
    resolve_shown(output, selected.as_ref(), focused.as_ref())
}

fn resolve_shown(
    output: &GameOutput,
    selected: Option<&InstanceKey>,
    focused: Option<&InstanceKey>,
) -> Option<InstanceKey> {
    selected
        .into_iter()
        .chain(focused)
        .find(|key| output.has(key))
        .cloned()
        .or_else(|| output.keys().into_iter().next())
}

/// Every instance with output, in run configuration order, with its name.
pub fn output_instances(world: &World) -> Vec<(InstanceKey, String)> {
    let keys = world
        .get_resource::<GameOutput>()
        .map(GameOutput::keys)
        .unwrap_or_default();
    let runs = world
        .get_resource::<RunConfigs>()
        .map(|configs| configs.manifest.runs.as_slice())
        .unwrap_or_default();
    named_instances(runs, &keys)
}

/// The instance named by `config` and `instance`, or the one the window shows
/// when `config` is empty.
fn instance_param(world: &World, params: &OperatorParameters) -> Option<InstanceKey> {
    match params.as_str("config").filter(|config| !config.is_empty()) {
        Some(config) => Some(InstanceKey {
            config: config.to_string(),
            instance: u32::try_from(params.as_int("instance").unwrap_or(1)).ok()?,
        }),
        None => shown_instance(world),
    }
}

/// Empty a game instance's output.
#[operator(
    id = "game_output.clear",
    label = "Clear Game Output",
    description = "Empty the output kept for a game instance.",
    allows_undo = false,
    params(
        config(
            String,
            default = "",
            doc = "The run configuration's name; empty for the instance the Game Output window shows."
        ),
        instance(i64, default = 1, doc = "Which instance of it, counting from 1."),
    )
)]
pub(crate) fn game_output_clear(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| {
        if let Some(key) = instance_param(world, &params)
            && let Some(output) = world.get_resource::<GameOutput>()
        {
            output.clear(&key);
        }
    });
    OperatorResult::Finished
}

/// Show one game instance's output in the Game Output window.
#[operator(
    id = "game_output.show_instance",
    label = "Show Instance Output",
    description = "Show this game instance's output in the Game Output window.",
    allows_undo = false,
    params(
        config(String, doc = "The run configuration's name."),
        instance(i64, default = 1, doc = "Which instance of it, counting from 1."),
    )
)]
pub(crate) fn game_output_show_instance(
    params: In<OperatorParameters>,
    mut view: ResMut<GameOutputView>,
) -> OperatorResult {
    let Some(config) = params.as_str("config").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    let Ok(instance) = u32::try_from(params.as_int("instance").unwrap_or(1)) else {
        return OperatorResult::Cancelled;
    };
    view.selected = Some(InstanceKey { config, instance });
    OperatorResult::Finished
}

/// The scrolling list of lines, with what it has on show.
#[derive(Component, Default)]
pub struct GameOutputList {
    key: Option<InstanceKey>,
    generation: u64,
    filter: String,
    /// The last sequence number read, matching or not.
    last_seq: Option<u64>,
    /// [`GameOutput::revision`] when the list last read the buffer.
    revision: Option<u64>,
    rows: VecDeque<Entity>,
    /// Whether matching lines older than the rows on show exist.
    more: bool,
}

/// One line on show, with the stream it came from.
#[derive(Component, Debug)]
pub struct GameOutputRow(pub OutputStream);

/// Marker on the text shown in place of rows when there are none.
#[derive(Component)]
pub struct GameOutputEmpty;

/// Marker on the note saying older lines are not on show.
#[derive(Component)]
pub struct GameOutputMore;

/// Marker on the button picking the instance, shown while several have output.
#[derive(Component)]
pub struct GameOutputPicker;

/// Marker on the filter box.
#[derive(Component)]
pub struct GameOutputFilter;

fn small_text(text: &str, color: Color) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(color),
    )
}

fn build_game_output_window(world: &mut World, parent: Entity) {
    let root = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                position_type: PositionType::Absolute,
                top: px(0.0),
                bottom: px(0.0),
                left: px(0.0),
                right: px(0.0),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();
    world.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(tokens::SPACING_SM),
            padding: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(root),
        children![
            (
                GameOutputPicker,
                jackdaw_feathers::tooltip::Tooltip::title("Game instance")
                    .with_description("The running or last run instance whose output shows."),
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
            (
                Node {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_width: px(60.0),
                    max_width: px(240.0),
                    ..Default::default()
                },
                children![(
                    GameOutputFilter,
                    text_edit::text_edit(
                        TextEditProps::default()
                            .with_placeholder("Filter")
                            .allow_empty()
                    ),
                )],
            ),
            Node {
                flex_grow: 1.0,
                ..Default::default()
            },
            (
                Node {
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                children![button(
                    ButtonProps::new("Clear")
                        .with_variant(ButtonVariant::Ghost)
                        .call_operator(GameOutputClearOp::ID),
                )],
            ),
        ],
    ));

    let body = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_grow: 1.0,
                min_width: px(0.0),
                min_height: px(0.0),
                width: percent(100),
                ..Default::default()
            },
            ChildOf(root),
        ))
        .id();
    let list = world
        .spawn((
            GameOutputList::default(),
            jackdaw_feathers::scroll::scroll_area(),
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_width: px(0.0),
                overflow: Overflow {
                    x: OverflowAxis::Clip,
                    y: OverflowAxis::Scroll,
                },
                padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_XS)),
                ..Default::default()
            },
            ChildOf(body),
        ))
        .id();
    world.spawn((jackdaw_feathers::scroll::scrollbar(list), ChildOf(body)));
    world.spawn((
        GameOutputMore,
        small_text("", tokens::TEXT_DISABLED),
        Node {
            display: Display::None,
            flex_shrink: 0.0,
            margin: UiRect::bottom(px(tokens::SPACING_XS)),
            ..Default::default()
        },
        ChildOf(list),
    ));
    world.spawn((
        GameOutputEmpty,
        small_text(EMPTY_NO_GAME, tokens::TEXT_SECONDARY),
        Node {
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(list),
    ));
}

fn instance_menu_rows(world: &World) -> Vec<MenuRow> {
    let shown = shown_instance(world);
    output_instances(world)
        .into_iter()
        .map(|(key, name)| MenuRow::Check {
            label: name,
            checked: shown.as_ref() == Some(&key),
            call: ButtonOperatorCall::new(GameOutputShowInstanceOp::ID)
                .with_param("config", key.config.clone())
                .with_param("instance", i64::from(key.instance)),
        })
        .collect()
}

/// Show the instance the Game panel switches to.
fn follow_focused_instance(instances: Res<PieInstances>, mut view: ResMut<GameOutputView>) {
    if let Some(focused) = &instances.focused
        && view.selected.as_ref() != Some(focused)
    {
        view.selected = Some(focused.clone());
    }
}

fn apply_output_filter(
    inputs: Query<&TextEditValue, (With<GameOutputFilter>, Changed<TextEditValue>)>,
    mut view: ResMut<GameOutputView>,
) {
    for input in &inputs {
        if view.filter != input.0 {
            view.filter.clone_from(&input.0);
        }
    }
}

/// The furthest a list can scroll, in logical pixels.
fn scroll_range(computed: &ComputedNode) -> f32 {
    ((computed.content_size().y - computed.size().y) * computed.inverse_scale_factor()).max(0.0)
}

/// Bring a list pinned past its end back to its end once layout has measured
/// the rows added, so the wheel scrolls up from there.
fn settle_output_scroll(
    mut lists: Query<(&mut ScrollPosition, &ComputedNode), With<GameOutputList>>,
) {
    for (mut scroll, computed) in &mut lists {
        let range = scroll_range(computed);
        if scroll.y > range + 1.0 {
            scroll.y = range;
        }
    }
}

fn row(line: &OutputLine, list: Entity) -> impl Bundle {
    let color = match line.stream {
        OutputStream::Stdout => tokens::TEXT_PRIMARY,
        OutputStream::Stderr => tokens::TEXT_WARNING,
    };
    (
        GameOutputRow(line.stream),
        Text::new(line.text.clone()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(color),
        Node {
            flex_shrink: 0.0,
            max_width: percent(100),
            ..Default::default()
        },
        ChildOf(list),
    )
}

/// Bring each list up to date with the buffer of the instance it shows: new
/// lines are appended, and the rows rebuilt only when the instance, the
/// filter or the buffer's generation changed. A list scrolled to its end
/// stays there.
fn sync_game_output(world: &mut World, mut last: Local<Option<ViewKey>>) {
    let mut lists = world.query::<(Entity, &GameOutputList)>();
    let lists: Vec<(Entity, bool)> = lists
        .iter(world)
        .map(|(entity, list)| (entity, list.revision.is_none()))
        .collect();
    if lists.is_empty() {
        *last = None;
        return;
    }
    let Some(output) = world.get_resource::<GameOutput>().cloned() else {
        return;
    };
    let filter = world
        .get_resource::<GameOutputView>()
        .map(|view| view.filter.clone())
        .unwrap_or_default();
    let now = (output.revision(), shown_instance(world), filter);
    let fresh = lists.iter().any(|(_, fresh)| *fresh);
    if !fresh && last.as_ref() == Some(&now) {
        return;
    }
    let (revision, shown, filter) = &now;
    for (list, _) in lists {
        sync_list(world, list, &output, *revision, shown.as_ref(), filter);
    }
    let instances = output_instances(world);
    sync_header(world, shown.as_ref(), &instances);
    *last = Some(now);
}

/// What the lists were last brought up to date with: the buffer revision, the
/// instance shown and the filter.
type ViewKey = (u64, Option<InstanceKey>, String);

fn sync_list(
    world: &mut World,
    list: Entity,
    output: &GameOutput,
    revision: u64,
    shown: Option<&InstanceKey>,
    filter: &str,
) {
    let Some(state) = world.get::<GameOutputList>(list) else {
        return;
    };
    let same_view = state.key.as_ref() == shown && state.filter == filter;
    if same_view && state.revision == Some(revision) {
        return;
    }
    let (generation, last_seq) = (state.generation, state.last_seq);

    let mut page =
        shown.and_then(|key| output.tail(key, last_seq.filter(|_| same_view), filter, OUTPUT_ROWS));
    let rebuild = !same_view
        || page
            .as_ref()
            .is_some_and(|page| page.generation != generation);
    if rebuild && same_view {
        page = shown.and_then(|key| output.tail(key, None, filter, OUTPUT_ROWS));
    }

    let pinned = match (
        world.get::<ScrollPosition>(list),
        world.get::<ComputedNode>(list),
    ) {
        (Some(scroll), Some(computed)) => scroll.y >= scroll_range(computed) - 4.0,
        _ => true,
    };

    let Some(mut held) = world.get_mut::<GameOutputList>(list) else {
        return;
    };
    let mut state = std::mem::take(&mut *held);
    let mut stale: Vec<Entity> = Vec::new();
    if rebuild {
        stale.extend(state.rows.drain(..));
        state.more = false;
    }
    let mut added = 0;
    if let Some(page) = &page {
        for line in &page.lines {
            state.rows.push_back(world.spawn(row(line, list)).id());
            added += 1;
        }
        state.more |= page.more;
        state.generation = page.generation;
        state.last_seq = page.next_seq.checked_sub(1);
    } else {
        state.last_seq = None;
    }
    while state.rows.len() > OUTPUT_ROWS {
        if let Some(oldest) = state.rows.pop_front() {
            stale.push(oldest);
            state.more = true;
        }
    }
    state.key = shown.cloned();
    state.filter = filter.to_string();
    state.revision = Some(revision);
    let empty = state.rows.is_empty();
    let more = state.more;
    if let Some(mut held) = world.get_mut::<GameOutputList>(list) {
        *held = state;
    }
    for entity in stale {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }

    let empty_message = if shown.is_none() {
        EMPTY_NO_GAME
    } else if filter.is_empty() {
        EMPTY_NO_LINES
    } else {
        EMPTY_NO_MATCH
    };
    let more_message = if filter.is_empty() {
        format!("Showing the last {OUTPUT_ROWS} lines")
    } else {
        format!("Showing the last {OUTPUT_ROWS} matching lines")
    };
    set_text_part::<GameOutputEmpty>(world, list, empty, empty_message);
    set_text_part::<GameOutputMore>(world, list, more, &more_message);
    if (added > 0 || rebuild)
        && pinned
        && let Some(mut scroll) = world.get_mut::<ScrollPosition>(list)
    {
        scroll.y = f32::MAX;
    }
}

/// Show or hide the `M` text under `list`, with `message`.
fn set_text_part<M: Component>(world: &mut World, list: Entity, shown: bool, message: &str) {
    let mut parts = world.query_filtered::<(Entity, &ChildOf), With<M>>();
    let Some(part) = parts
        .iter(world)
        .find(|(_, parent)| parent.parent() == list)
        .map(|(entity, _)| entity)
    else {
        return;
    };
    let display = if shown { Display::Flex } else { Display::None };
    if let Some(mut node) = world.get_mut::<Node>(part)
        && node.display != display
    {
        node.display = display;
    }
    if let Some(mut text) = world.get_mut::<Text>(part)
        && text.0 != message
    {
        text.0 = message.to_string();
    }
}

/// Show the instance picker while several instances have output, captioned
/// with the one on show.
fn sync_header(
    world: &mut World,
    shown: Option<&InstanceKey>,
    instances: &[(InstanceKey, String)],
) {
    let caption = shown
        .and_then(|key| instances.iter().find(|(held, _)| held == key))
        .map(|(_, name)| name.clone())
        .unwrap_or_default();
    let display = if instances.len() > 1 {
        Display::Flex
    } else {
        Display::None
    };
    let mut pickers = world.query_filtered::<(Entity, &mut Node), With<GameOutputPicker>>();
    let mut found = Vec::new();
    for (entity, mut node) in pickers.iter_mut(world) {
        if node.display != display {
            node.display = display;
        }
        found.push(entity);
    }
    for picker in found {
        if let Some(text) = button_caption(world, picker)
            && let Some(mut text) = world.get_mut::<Text>(text)
            && text.0 != caption
        {
            text.0.clone_from(&caption);
        }
    }
}

/// The caption text of `button`, leaving out a menu it holds open.
fn button_caption(world: &World, button: Entity) -> Option<Entity> {
    let mut stack = vec![button];
    while let Some(entity) = stack.pop() {
        if world.get::<ButtonContentText>(entity).is_some() {
            return Some(entity);
        }
        if let Some(kids) = world.get::<Children>(entity) {
            stack.extend(
                kids.iter()
                    .filter(|kid| world.get::<EditorPopover>(*kid).is_none()),
            );
        }
    }
    None
}

/// Registers the Game Output window as a bottom-dock window and its operators.
#[derive(Default)]
pub struct GameOutputExtension;

impl JackdawExtension for GameOutputExtension {
    fn id(&self) -> String {
        "jackdaw.game_output".to_string()
    }

    fn label(&self) -> String {
        "Game Output".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<GameOutputClearOp>()
            .register_operator::<GameOutputShowInstanceOp>();
        ctx.register_window(
            WindowDescriptor::new(GAME_OUTPUT_WINDOW_ID)
                .with_name("Game Output")
                .with_icon(Icon::SquareTerminal.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(4)
                .with_build(|window| {
                    let parent = window.target_entity();
                    build_game_output_window(window.world_mut(), parent);
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(config: &str) -> InstanceKey {
        InstanceKey {
            config: config.to_string(),
            instance: 1,
        }
    }

    #[test]
    fn the_window_shows_the_picked_then_the_focused_then_the_latest_instance() {
        let output = GameOutput::default();
        assert_eq!(resolve_shown(&output, None, None), None);
        let mut child = std::process::Command::new("true").spawn().expect("a child");
        drop(output.capture(&key("Server"), &mut child));
        drop(output.capture(&key("Client"), &mut child));
        child.wait().ok();
        assert_eq!(resolve_shown(&output, None, None), Some(key("Client")));
        assert_eq!(
            resolve_shown(&output, None, Some(&key("Server"))),
            Some(key("Server"))
        );
        assert_eq!(
            resolve_shown(&output, Some(&key("Client")), Some(&key("Server"))),
            Some(key("Client"))
        );
        assert_eq!(
            resolve_shown(&output, Some(&key("Gone")), None),
            Some(key("Client")),
            "an instance without output is passed over"
        );
    }
}
