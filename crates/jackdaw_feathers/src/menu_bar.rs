use bevy::{feathers::theme::ThemedText, prelude::*, ui::ui_transform::UiGlobalTransform};
use jackdaw_widgets::menu_bar::{
    MenuAction, MenuBar, MenuBarDropdown, MenuBarDropdownItem, MenuBarItem, MenuBarState,
};

use crate::button::{ButtonClickEvent, ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use crate::icons::Icon;
use crate::tokens;

/// Action strings in menu entries that start with this prefix are
/// interpreted as operator ids; the suffix is parsed by
/// [`ButtonOperatorCall`]'s `TryFrom<&str>` impl into the call attached
/// to the dropdown button so the editor dispatches through the operator
/// API instead of firing a generic [`MenuAction`].
pub const OP_ACTION_PREFIX: &str = "op:";

/// Action strings in menu entries that start with this prefix render as a
/// non-interactive section label; the suffix is the label text.
pub const SECTION_ACTION_PREFIX: &str = "##";

/// Action string for a horizontal divider row.
pub const SEPARATOR_ACTION: &str = "---";

/// Action strings in menu entries that start with this prefix open a
/// submenu: the suffix is the group's label, and every row between it and
/// its matching [`SUBMENU_END_ACTION`] belongs to the child dropdown that
/// the row expands on hover. Groups nest.
pub const SUBMENU_ACTION_PREFIX: &str = ">>";

/// Action string closing the innermost [`SUBMENU_ACTION_PREFIX`] group.
pub const SUBMENU_END_ACTION: &str = "<<";

/// One menu group as dropdown rows: an opener carrying `label`, the
/// group's own `rows`, and the closer. Feathers renders the opener as a
/// row with an arrow that expands `rows` in a child dropdown while the
/// pointer rests on it.
pub fn submenu_row(
    label: &str,
    rows: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    let mut encoded = vec![(format!("{SUBMENU_ACTION_PREFIX}{label}"), String::new())];
    encoded.extend(rows);
    encoded.push((String::from(SUBMENU_END_ACTION), String::new()));
    encoded
}

pub fn plugin(app: &mut App) {
    app.init_resource::<SubmenuState>()
        .add_observer(on_dropdown_item_click)
        .add_observer(on_menu_bar_item_click)
        .add_observer(on_menu_bar_item_over)
        .add_observer(on_menu_bar_item_out)
        .add_observer(on_menu_pointer_over)
        .add_observer(on_menu_pointer_out)
        .add_systems(
            Update,
            (sync_menu_bar_item_backgrounds, advance_submenu_hover),
        );
}

/// When a dropdown item is clicked, fire the [`MenuAction`]; unless the
/// item carries a [`ButtonOperatorCall`] component, in which case the editor's
/// operator observer will handle dispatch and a `MenuAction` would
/// double-fire.
fn on_dropdown_item_click(
    event: On<ButtonClickEvent>,
    items: Query<(&MenuBarDropdownItem, Option<&ButtonOperatorCall>)>,
    mut commands: Commands,
) {
    let Ok((item, button_op)) = items.get(event.entity) else {
        return;
    };
    if button_op.is_some() {
        return;
    }
    commands.trigger(MenuAction {
        action: item.action.clone(),
    });
}

/// Handle click on a [`MenuBarItem`]: find the item by walking up from the event target.
fn on_menu_bar_item_click(
    mut click: On<Pointer<Click>>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
    items: Query<(&MenuBarItem, &ComputedNode, &UiGlobalTransform)>,
    item_check: Query<Entity, With<MenuBarItem>>,
    parents: Query<&ChildOf>,
    windows: Query<&Window>,
    mut bg_query: Query<(Entity, &mut BackgroundColor), With<MenuBarItem>>,
) {
    let Some(entity) = find_ancestor(click.event_target(), &item_check, &parents) else {
        return;
    };
    let Ok((item, computed, global_tf)) = items.get(entity) else {
        return;
    };

    click.propagate(false);

    open_menu_dropdown(
        &mut commands,
        &mut state,
        entity,
        item,
        computed,
        global_tf,
        window_height(&windows),
    );
    apply_menu_bar_item_backgrounds(state.open_menu, &mut bg_query);
}

fn on_menu_bar_item_over(
    hover: On<Pointer<Over>>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
    items: Query<(&MenuBarItem, &ComputedNode, &UiGlobalTransform)>,
    item_check: Query<Entity, With<MenuBarItem>>,
    parents: Query<&ChildOf>,
    windows: Query<&Window>,
    mut bg_query: Query<(Entity, &mut BackgroundColor), With<MenuBarItem>>,
) {
    let Some(entity) = find_ancestor(hover.event_target(), &item_check, &parents) else {
        return;
    };
    // Hover only switches menus after the user has opened one with a click.
    if state.open_menu.is_none() {
        return;
    }
    if state.open_menu == Some(entity) {
        return;
    }
    let Ok((item, computed, global_tf)) = items.get(entity) else {
        return;
    };

    open_menu_dropdown(
        &mut commands,
        &mut state,
        entity,
        item,
        computed,
        global_tf,
        window_height(&windows),
    );
    apply_menu_bar_item_backgrounds(state.open_menu, &mut bg_query);
}

fn open_menu_dropdown(
    commands: &mut Commands,
    state: &mut MenuBarState,
    entity: Entity,
    item: &MenuBarItem,
    computed: &ComputedNode,
    global_tf: &UiGlobalTransform,
    window_height: f32,
) {
    if let Some(dropdown) = state.dropdown_entity.take() {
        commands.entity(dropdown).despawn();
    }

    state.open_menu = Some(entity);

    let (_, _, mut pos) = global_tf.to_scale_angle_translation();
    pos *= computed.inverse_scale_factor();
    let size = computed.size() * computed.inverse_scale_factor();
    let x = pos.x - size.x / 2.0;
    let y = pos.y + size.y / 2.0;

    let dropdown = spawn_dropdown(
        commands,
        x,
        y,
        available_height(window_height, y),
        &item.actions,
    );
    state.dropdown_entity = Some(dropdown);
}

/// How tall a dropdown opened at `top` may grow before it runs off the
/// bottom of the window. A menu longer than this scrolls rather than
/// putting its last entries out of reach.
fn available_height(window_height: f32, top: f32) -> f32 {
    (window_height - top - DROPDOWN_MARGIN).max(DROPDOWN_MIN_HEIGHT)
}

/// Gap kept between a dropdown and the bottom of the window.
const DROPDOWN_MARGIN: f32 = 8.0;

/// A dropdown never clamps below this, so a menu opened in a very short
/// window is still usable by scrolling.
const DROPDOWN_MIN_HEIGHT: f32 = 120.0;

/// The window height dropdowns clamp against when no window is present
/// (headless runs).
const FALLBACK_WINDOW_HEIGHT: f32 = 1080.0;

fn window_height(windows: &Query<&Window>) -> f32 {
    windows
        .iter()
        .next()
        .map(Window::height)
        .filter(|height| *height > 0.0)
        .unwrap_or(FALLBACK_WINDOW_HEIGHT)
}

/// Open the dropdown of the menu-bar item whose label is `label`, taking
/// the same path a click on it takes. A scripted run has no pointer, so a
/// screenshot of an open menu is otherwise unreachable; nothing here
/// touches document state, only the menu's own open/highlight state.
///
/// Returns whether an item with that label was found.
pub fn open_menu_named(
    label: &str,
    commands: &mut Commands,
    state: &mut MenuBarState,
    items: &Query<(Entity, &MenuBarItem, &ComputedNode, &UiGlobalTransform)>,
    windows: &Query<&Window>,
    backgrounds: &mut Query<(Entity, &mut BackgroundColor), With<MenuBarItem>>,
) -> bool {
    let Some((entity, item, computed, global_tf)) =
        items.iter().find(|(_, item, _, _)| item.label == label)
    else {
        return false;
    };
    open_menu_dropdown(
        commands,
        state,
        entity,
        item,
        computed,
        global_tf,
        window_height(windows),
    );
    apply_menu_bar_item_backgrounds(state.open_menu, backgrounds);
    true
}

fn on_menu_bar_item_out(
    out: On<Pointer<Out>>,
    state: Res<MenuBarState>,
    items: Query<Entity, With<MenuBarItem>>,
    parents: Query<&ChildOf>,
    mut bg_query: Query<&mut BackgroundColor>,
) {
    let Some(entity) = find_ancestor(out.event_target(), &items, &parents) else {
        return;
    };
    if state.open_menu == Some(entity) {
        return;
    }
    let Ok(mut bg) = bg_query.get_mut(entity) else {
        return;
    };
    bg.0 = Color::NONE;
}

// Keep the top-level menu highlight tied to the open menu state. This covers
// closures that happen outside pointer-out events, such as action dispatch or Escape.
fn sync_menu_bar_item_backgrounds(
    state: Res<MenuBarState>,
    mut items: Query<(Entity, &mut BackgroundColor), With<MenuBarItem>>,
) {
    if state.is_changed() {
        apply_menu_bar_item_backgrounds(state.open_menu, &mut items);
    }
}

fn apply_menu_bar_item_backgrounds(
    open_menu: Option<Entity>,
    items: &mut Query<(Entity, &mut BackgroundColor), With<MenuBarItem>>,
) {
    for (entity, mut bg) in items.iter_mut() {
        bg.0 = if open_menu == Some(entity) {
            tokens::HOVER_BG
        } else {
            Color::NONE
        };
    }
}

/// Walk up from `start` through [`ChildOf`] to find an entity with `MenuBarItem`.
fn find_ancestor(
    start: Entity,
    items: &Query<Entity, With<MenuBarItem>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    find_ancestor_matching(start, items, parents)
}

/// Walk up from `start` through [`ChildOf`] to the nearest entity `matching`
/// answers for.
fn find_ancestor_matching<F: bevy::ecs::query::QueryFilter>(
    start: Entity,
    matching: &Query<Entity, F>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut entity = start;
    for _ in 0..10 {
        if matching.contains(entity) {
            return Some(entity);
        }
        if let Ok(child_of) = parents.get(entity) {
            entity = child_of.parent();
        } else {
            return None;
        }
    }
    None
}

/// A dropdown row that expands a group of rows in a child dropdown while
/// the pointer rests on it. Spawned for every [`SUBMENU_ACTION_PREFIX`]
/// row; `actions` are the group's own rows, still encoded, so a child
/// dropdown parses nested groups the same way its parent did.
#[derive(Component)]
pub struct SubmenuRow {
    /// The group's label, as the row shows it.
    pub label: String,
    /// The rows the child dropdown is built from.
    pub actions: Vec<(String, String)>,
}

/// Marker on a child dropdown, naming the [`SubmenuRow`] it hangs off.
#[derive(Component)]
pub struct SubmenuDropdown {
    /// The row that expanded this dropdown.
    pub row: Entity,
}

/// How long the pointer must rest on a [`SubmenuRow`] before its child
/// dropdown opens. Long enough that crossing a group on the way to a
/// lower row opens nothing, short enough that resting on one answers
/// promptly: a few tens of milliseconds flashes dropdowns the pointer
/// only swept across, and around 300ms reads as sluggish.
const SUBMENU_OPEN_DELAY: f32 = 0.18;

/// How long an open submenu survives the pointer being on neither its row
/// nor its own dropdown. The two do not touch along the whole edge, and
/// the pointer crosses that gap diagonally on its way to the child rows.
/// Closing on the first frame outside would leave the child reachable only
/// by a right-angled path, so the menu is held open across that gap.
const SUBMENU_CLOSE_GRACE: f32 = 0.25;

/// Narrowest a dropdown gets, and the room a child dropdown needs on the
/// right of its parent before it opens on the left instead.
const DROPDOWN_MIN_WIDTH: f32 = 180.0;

/// The window width child dropdowns flip against when no window is present
/// (headless runs).
const FALLBACK_WINDOW_WIDTH: f32 = 1920.0;

/// The open submenu chain, and the hover timers that grow and cut it.
///
/// Pointer traversal only; opening a group from the keyboard is not part
/// of this.
#[derive(Resource, Default)]
pub struct SubmenuState {
    /// Open child dropdowns, outermost first. Each one's dropdown owns the
    /// next one's row.
    open: Vec<OpenSubmenu>,
    /// The row the pointer is resting on, and for how long.
    pending: Option<PendingSubmenu>,
    /// How long the pointer has been away, and how much of the chain
    /// survives if it stays away.
    closing: Option<ClosingSubmenu>,
}

struct OpenSubmenu {
    row: Entity,
    dropdown: Entity,
    owner: Entity,
}

struct PendingSubmenu {
    row: Entity,
    waited: f32,
}

struct ClosingSubmenu {
    keep: usize,
    waited: f32,
}

impl SubmenuState {
    /// How deep in the open chain `dropdown` sits: 0 for the menu-bar
    /// dropdown every chain hangs off, 1 for the first child, and so on.
    /// `None` when the dropdown is not part of the open chain.
    fn depth_of(&self, dropdown: Entity) -> Option<usize> {
        if self.open.first().map(|open| open.owner) == Some(dropdown) {
            return Some(0);
        }
        self.open
            .iter()
            .position(|open| open.dropdown == dropdown)
            .map(|index| index + 1)
    }

    /// Whether `row` is the row the submenu at `depth` hangs off.
    fn row_at(&self, depth: usize, row: Entity) -> bool {
        self.open.get(depth).map(|open| open.row) == Some(row)
    }
}

/// Close every open submenu deeper than `keep`.
fn truncate_submenus(state: &mut SubmenuState, keep: usize, commands: &mut Commands) {
    while state.open.len() > keep {
        let Some(open) = state.open.pop() else {
            return;
        };
        commands.entity(open.dropdown).try_despawn();
    }
}

/// The pointer entered a submenu row: wait out the dwell, and drop
/// anything open below the row's own dropdown in the meantime.
fn on_menu_pointer_over(
    over: On<Pointer<Over>>,
    mut commands: Commands,
    mut state: ResMut<SubmenuState>,
    rows: Query<Entity, With<SubmenuRow>>,
    dropdowns: Query<Entity, With<MenuBarDropdown>>,
    parents: Query<&ChildOf>,
) {
    // The event bubbles through the row's ancestors; hover follows what
    // the pointer is actually on, so only the first visit counts.
    if over.event_target() != over.original_event_target() {
        return;
    }
    let target = over.original_event_target();
    let row = find_ancestor_matching(target, &rows, &parents);
    if row.is_none() && state.open.is_empty() && state.pending.is_none() {
        return;
    }
    // Only a pointer back inside the menu calls off a scheduled close.
    // The editor is pickable wall to wall, so clearing it before this
    // guard would let any node the pointer crossed cancel the close and
    // leave the child dropdown floating over the viewport.
    let Some(dropdown) = find_ancestor_matching(target, &dropdowns, &parents) else {
        return;
    };
    state.closing = None;
    // Only one menu is ever open, so a dropdown outside the chain is the
    // menu-bar dropdown the chain hangs off.
    let depth = state.depth_of(dropdown).unwrap_or(0);

    match row {
        Some(row) if state.row_at(depth, row) => {
            // Its submenu is the one already open: keep it and stop any
            // dwell aimed at a sibling.
            state.pending = None;
            truncate_submenus(&mut state, depth + 1, &mut commands);
        }
        Some(row) => {
            // A different group: its siblings close first, so two child
            // dropdowns are never up at once, and this one waits out the
            // dwell.
            truncate_submenus(&mut state, depth, &mut commands);
            state.pending = Some(PendingSubmenu { row, waited: 0.0 });
        }
        None => {
            // A row that opens nothing, or the menu's own background. The
            // pointer crosses these on its way from a group to the child
            // it opened, which is why this waits the grace out instead of
            // closing where it stands.
            state.pending = None;
            state.closing = Some(ClosingSubmenu {
                keep: depth,
                waited: 0.0,
            });
        }
    }
}

/// The pointer left a row or a child dropdown: start the grace period.
/// Whatever it entered instead cancels or shortens this in the same frame.
fn on_menu_pointer_out(
    out: On<Pointer<Out>>,
    mut state: ResMut<SubmenuState>,
    rows: Query<Entity, With<SubmenuRow>>,
    dropdowns: Query<Entity, With<MenuBarDropdown>>,
    parents: Query<&ChildOf>,
) {
    if out.event_target() != out.original_event_target() {
        return;
    }
    if state.open.is_empty() && state.pending.is_none() {
        return;
    }
    let target = out.original_event_target();
    if let Some(row) = find_ancestor_matching(target, &rows, &parents)
        && state.pending.as_ref().map(|pending| pending.row) == Some(row)
    {
        state.pending = None;
    }
    if find_ancestor_matching(target, &dropdowns, &parents).is_none() {
        return;
    }
    // The whole chain is scheduled, not the part below what the pointer
    // left: a pointer that goes nowhere else, out of the window or onto
    // the viewport, sends no further event to close the rest. An Over
    // landing back in the menu shortens this to the depth it landed at.
    state.closing = Some(ClosingSubmenu {
        keep: 0,
        waited: 0.0,
    });
}

/// Run the hover timers: open what the pointer has rested on long enough,
/// close what it has been away from long enough, and drop any child
/// dropdown whose row went away with the menu that held it.
fn advance_submenu_hover(
    time: Res<Time>,
    mut commands: Commands,
    mut state: ResMut<SubmenuState>,
    rows: Query<(&SubmenuRow, &ComputedNode, &UiGlobalTransform, &ChildOf)>,
    dropdowns: Query<(&ComputedNode, &UiGlobalTransform), With<MenuBarDropdown>>,
    windows: Query<&Window>,
) {
    let orphaned = state
        .open
        .iter()
        .position(|open| rows.get(open.row).is_err());
    if let Some(depth) = orphaned {
        truncate_submenus(&mut state, depth, &mut commands);
        state.pending = None;
        state.closing = None;
    }

    let delta = time.delta_secs();
    if let Some(closing) = state.closing.as_mut() {
        closing.waited += delta;
        if closing.waited >= SUBMENU_CLOSE_GRACE {
            let keep = closing.keep;
            state.closing = None;
            truncate_submenus(&mut state, keep, &mut commands);
        }
    }

    let Some(pending) = state.pending.as_mut() else {
        return;
    };
    pending.waited += delta;
    if pending.waited < SUBMENU_OPEN_DELAY {
        return;
    }
    let row = pending.row;
    state.pending = None;
    open_submenu(&mut commands, &mut state, row, &rows, &dropdowns, &windows);
}

/// Spawn the child dropdown `row` expands, beside the dropdown that holds
/// the row. Returns whether it opened.
fn open_submenu(
    commands: &mut Commands,
    state: &mut SubmenuState,
    row: Entity,
    rows: &Query<(&SubmenuRow, &ComputedNode, &UiGlobalTransform, &ChildOf)>,
    dropdowns: &Query<(&ComputedNode, &UiGlobalTransform), With<MenuBarDropdown>>,
    windows: &Query<&Window>,
) -> bool {
    let Ok((submenu, row_node, row_tf, child_of)) = rows.get(row) else {
        return false;
    };
    let owner = child_of.parent();
    let Ok((owner_node, owner_tf)) = dropdowns.get(owner) else {
        return false;
    };
    // Nothing open yet means the row sits in the menu-bar dropdown the
    // whole chain hangs off, which is depth zero.
    let depth = state.depth_of(owner).unwrap_or(0);
    truncate_submenus(state, depth, commands);

    let scale = row_node.inverse_scale_factor();
    let (_, _, row_pos) = row_tf.to_scale_angle_translation();
    let row_pos = row_pos * scale;
    let row_size = row_node.size() * scale;
    let (_, _, owner_pos) = owner_tf.to_scale_angle_translation();
    let owner_pos = owner_pos * owner_node.inverse_scale_factor();
    let owner_size = owner_node.size() * owner_node.inverse_scale_factor();

    let origin = submenu_origin(
        row_pos.y - row_size.y / 2.0,
        owner_pos.x - owner_size.x / 2.0,
        owner_pos.x + owner_size.x / 2.0,
        window_width(windows),
    );
    let (left, top) = (origin.x, origin.y);

    let dropdown = spawn_dropdown(
        commands,
        left,
        top,
        available_height(window_height(windows), top),
        &submenu.actions,
    );
    commands.entity(dropdown).insert(SubmenuDropdown { row });
    state.open.push(OpenSubmenu {
        row,
        dropdown,
        owner,
    });
    true
}

/// Where a child dropdown opens: off the right edge of the dropdown that
/// holds the row, its first row level with that row, flipping to the left
/// of it when the window has no room on the right. A window too narrow for
/// either side keeps the child on screen at the left edge.
fn submenu_origin(row_top: f32, owner_left: f32, owner_right: f32, window_width: f32) -> Vec2 {
    let left = if owner_right + DROPDOWN_MIN_WIDTH > window_width {
        (owner_left - DROPDOWN_MIN_WIDTH).max(0.0)
    } else {
        owner_right
    };
    Vec2::new(left, row_top)
}

/// Expand the submenu whose group label is `label`, as resting the
/// pointer on its row does, but without the dwell: a scripted run has no
/// pointer and no wall clock, so a screenshot of an expanded group is
/// otherwise unreachable. Only menu state is touched.
///
/// Returns whether a group with that label was open to expand.
pub fn open_submenu_named(
    label: &str,
    commands: &mut Commands,
    state: &mut SubmenuState,
    rows: &Query<(&SubmenuRow, &ComputedNode, &UiGlobalTransform, &ChildOf)>,
    row_ids: &Query<(Entity, &SubmenuRow)>,
    dropdowns: &Query<(&ComputedNode, &UiGlobalTransform), With<MenuBarDropdown>>,
    windows: &Query<&Window>,
) -> bool {
    let Some((row, _)) = row_ids.iter().find(|(_, row)| row.label == label) else {
        return false;
    };
    state.pending = None;
    state.closing = None;
    open_submenu(commands, state, row, rows, dropdowns, windows)
}

fn window_width(windows: &Query<&Window>) -> f32 {
    windows
        .iter()
        .next()
        .map(Window::width)
        .filter(|width| *width > 0.0)
        .unwrap_or(FALLBACK_WINDOW_WIDTH)
}

/// Marker for the menu bar root so we can find and populate it.
#[derive(Component)]
pub struct MenuBarRoot;

/// Build the styled menu bar shell. Items are spawned by
/// `populate_menu_bar` system.
///
/// The shell sizes to its content width (menu items + padding) so it
/// composes cleanly inside a horizontal flex row alongside siblings like
/// a document tab strip or transport controls.
pub fn menu_bar_shell() -> impl Bundle {
    (
        MenuBarRoot,
        MenuBar,
        Pickable::IGNORE,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            // Auto width so siblings (tab strips, transport pills) get
            // their share of the row; `flex_shrink: 0` keeps our menu
            // items from being squeezed if the window is narrow.
            width: Val::Auto,
            height: Val::Px(tokens::HEADER_CONTROL_HEIGHT),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::WINDOW_BG),
    )
}

/// Populate a menu bar entity with items. Call from the app layer after spawning the shell.
///
/// Actions are `(action_id, label)` pairs. `action_id` can be an
/// operator id wrapped in the [`OP_ACTION_PREFIX`] or any free-form
/// identifier the host matches in a `MenuAction` observer. Action
/// strings are owned so callers can pass `format!("op:{}", Op::ID)`
/// without leaking operator-id string literals into UI code.
pub fn populate_menu_bar(
    world: &mut World,
    menu_bar_entity: Entity,
    menus: impl IntoIterator<Item = (String, Vec<(String, String)>)>,
) {
    for (label, actions) in menus {
        spawn_menu_bar_item(world, menu_bar_entity, &label, actions);
    }
}

fn spawn_menu_bar_item(
    world: &mut World,
    parent: Entity,
    label: &str,
    actions: Vec<(String, String)>,
) {
    world.spawn((
        MenuBarItem {
            label: label.to_string(),
            actions,
        },
        Node {
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            ThemedText,
        )],
        ChildOf(parent),
    ));
}

fn spawn_dropdown(
    commands: &mut Commands,
    x: f32,
    y: f32,
    max_height: f32,
    actions: &[(String, String)],
) -> Entity {
    let dropdown = commands
        .spawn((
            MenuBarDropdown,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x),
                top: Val::Px(y),
                flex_direction: FlexDirection::Column,
                min_width: Val::Px(DROPDOWN_MIN_WIDTH),
                // A menu the extensions have grown past the bottom of the
                // window scrolls; without the clamp its tail is unreachable.
                max_height: Val::Px(max_height),
                overflow: Overflow::scroll_y(),
                padding: UiRect::axes(Val::Px(tokens::SPACING_XS), Val::Px(tokens::SPACING_SM)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            ScrollPosition::default(),
            BackgroundColor(tokens::MENU_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ZIndex(1000),
        ))
        .id();

    let mut index = 0;
    while index < actions.len() {
        let (action, label) = &actions[index];
        index += 1;
        if action == SEPARATOR_ACTION {
            // Separator
            commands.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(1.0),
                    margin: UiRect::axes(Val::Px(0.0), Val::Px(tokens::SPACING_XS)),
                    ..Default::default()
                },
                BackgroundColor(tokens::BORDER_SUBTLE),
                ChildOf(dropdown),
            ));
            continue;
        }

        if let Some(section) = action.strip_prefix(SECTION_ACTION_PREFIX) {
            commands.spawn((
                Node {
                    padding: UiRect::new(
                        Val::Px(tokens::SPACING_MD),
                        Val::Px(tokens::SPACING_MD),
                        Val::Px(tokens::SPACING_SM),
                        Val::Px(tokens::SPACING_XS),
                    ),
                    ..Default::default()
                },
                Pickable::IGNORE,
                children![(
                    Text::new(section.to_string()),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                )],
                ChildOf(dropdown),
            ));
            continue;
        }

        if let Some(group) = action.strip_prefix(SUBMENU_ACTION_PREFIX) {
            let (rows, past_end) = submenu_group(group, actions, index);
            index = past_end;
            let row = SubmenuRow {
                label: group.to_string(),
                actions: rows,
            };
            let arrow = button(
                ButtonProps::new(group.to_string())
                    .with_variant(ButtonVariant::Ghost)
                    .align_left()
                    .with_right_icon(Icon::ChevronRight),
            );
            commands.entity(dropdown).with_child((row, arrow));
            continue;
        }

        // A closer with no opener: the row carries no content of its own,
        // and the menu was built with rows in the wrong place.
        if action == SUBMENU_END_ACTION {
            warn!(
                "menu row closes a group that was never opened; the rows meant for it are in the menu itself"
            );
            continue;
        }

        let item = MenuBarDropdownItem {
            action: action.clone(),
        };
        let btn = button(
            ButtonProps::new(label.clone())
                .with_variant(ButtonVariant::Ghost)
                // TODO: add keybind as subtitle
                .align_left(),
        );

        if let Ok(call) = ButtonOperatorCall::try_from(action.as_str()) {
            // Operator-bound menu entries dispatch through the editor's
            // `ButtonOperatorCall` observer; the editor's tooltip
            // renderer reads the call's id + params for the rich
            // hover popover.
            commands.entity(dropdown).with_child((item, btn, call));
        } else {
            // Non-operator actions (legacy free-form action ids)
            // dispatch via the `MenuAction` event. They get no hover
            // tooltip; the operator-only tooltip pipeline has no
            // place for them, and these will go away once every menu
            // entry is operator-backed.
            commands.entity(dropdown).with_child((item, btn));
        }
    }

    dropdown
}

/// The rows of the group opened just before `start`, and the index past
/// its closer. Nested groups keep their encoding, so the child dropdown
/// parses them the same way this one was parsed.
fn submenu_group(
    label: &str,
    actions: &[(String, String)],
    start: usize,
) -> (Vec<(String, String)>, usize) {
    let mut depth = 1usize;
    let mut rows = Vec::new();
    let mut index = start;
    while index < actions.len() {
        let (action, _) = &actions[index];
        if action.starts_with(SUBMENU_ACTION_PREFIX) {
            depth += 1;
        } else if action == SUBMENU_END_ACTION {
            depth -= 1;
            if depth == 0 {
                return (rows, index + 1);
            }
        }
        rows.push(actions[index].clone());
        index += 1;
    }
    // An unterminated group takes the rest of the menu rather than
    // dropping rows, and warns: every row after it has moved into it.
    warn!("menu group '{label}' is never closed; the rest of the menu expands with it");
    (rows, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{Out, Over, Pointer};
    use bevy::picking::pointer::PointerId;

    /// Build one dropdown and hand back it and its rows, in order.
    fn dropdown(
        actions: Vec<(String, String)>,
        top: f32,
        window: f32,
    ) -> (World, Entity, Vec<Entity>) {
        let mut world = World::new();
        let entity = world
            .run_system_once(move |mut commands: Commands| {
                spawn_dropdown(
                    &mut commands,
                    0.0,
                    top,
                    available_height(window, top),
                    &actions,
                )
            })
            .expect("the dropdown spawns");
        let rows = world
            .get::<Children>(entity)
            .map(|children| children.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        (world, entity, rows)
    }

    #[test]
    fn a_section_row_is_a_label_and_not_a_menu_entry() {
        let (world, _, rows) = dropdown(
            vec![
                (format!("{SECTION_ACTION_PREFIX}Scene"), String::new()),
                ("op:scene.new".to_string(), "New".to_string()),
            ],
            0.0,
            1080.0,
        );
        assert_eq!(rows.len(), 2, "one row per entry");

        let section = rows[0];
        assert!(
            world.get::<MenuBarDropdownItem>(section).is_none(),
            "a section heading dispatches nothing",
        );
        let pickable = world
            .get::<Pickable>(section)
            .expect("a section heading declares its pointer behaviour");
        assert!(
            !pickable.is_hoverable && !pickable.should_block_lower,
            "a section heading does not take the pointer",
        );
        let text = world
            .get::<Children>(section)
            .and_then(|children| children.iter().next())
            .and_then(|child| world.get::<Text>(child))
            .expect("a section heading carries its label");
        assert_eq!(text.0, "Scene", "the prefix is stripped from the label");

        assert!(
            world.get::<MenuBarDropdownItem>(rows[1]).is_some(),
            "an action row is still a menu entry",
        );
    }

    #[test]
    fn a_long_menu_clamps_and_scrolls_instead_of_running_off_screen() {
        let actions = (0..80)
            .map(|index| (format!("op:test.{index}"), format!("Entry {index}")))
            .collect::<Vec<_>>();
        let (world, dropdown_entity, _) = dropdown(actions, 30.0, 1080.0);

        let node = world
            .get::<Node>(dropdown_entity)
            .expect("the dropdown has a node");
        assert_eq!(
            node.max_height,
            Val::Px(1042.0),
            "the dropdown stops short of the bottom of the window",
        );
        assert_eq!(
            node.overflow.y,
            OverflowAxis::Scroll,
            "what the clamp cuts off stays reachable by scrolling",
        );
        assert!(
            world.get::<ScrollPosition>(dropdown_entity).is_some(),
            "the scroll handler needs somewhere to write",
        );
    }

    #[test]
    fn a_short_window_still_leaves_a_usable_menu() {
        assert_eq!(
            available_height(100.0, 90.0),
            DROPDOWN_MIN_HEIGHT,
            "the clamp never collapses the menu to nothing",
        );
    }

    /// Two entries under one group, plus a plain sibling entry.
    fn grouped_actions() -> Vec<(String, String)> {
        let mut actions = submenu_row(
            "3D Objects",
            [
                ("op:entity.add.cube".to_string(), "Cube".to_string()),
                ("op:entity.add.plane".to_string(), "Plane".to_string()),
            ],
        );
        actions.push(("op:entity.add.empty".to_string(), "Empty".to_string()));
        actions
    }

    #[test]
    fn a_group_is_one_row_that_holds_its_entries() {
        let (world, _, rows) = dropdown(grouped_actions(), 0.0, 1080.0);
        assert_eq!(rows.len(), 2, "the group is one row, not four");

        let group = world
            .get::<SubmenuRow>(rows[0])
            .expect("the group row carries what it expands");
        assert_eq!(group.label, "3D Objects");
        assert_eq!(
            group.actions,
            vec![
                ("op:entity.add.cube".to_string(), "Cube".to_string()),
                ("op:entity.add.plane".to_string(), "Plane".to_string()),
            ],
            "the group's entries wait in the row, not in the parent menu",
        );
        assert!(
            world.get::<MenuBarDropdownItem>(rows[0]).is_none(),
            "the group itself dispatches nothing",
        );
        assert!(
            world.get::<MenuBarDropdownItem>(rows[1]).is_some(),
            "an entry beside a group is still an entry",
        );
    }

    #[test]
    fn a_group_keeps_its_entries_when_the_menu_holds_nested_groups() {
        let inner = submenu_row("Controls", [("widget:ui.button".into(), "Button".into())]);
        let outer = submenu_row("UI", inner);
        let (world, _, rows) = dropdown(outer, 0.0, 1080.0);

        assert_eq!(rows.len(), 1, "one top-level row");
        let group = world.get::<SubmenuRow>(rows[0]).expect("the UI group");
        assert_eq!(
            group.actions.len(),
            3,
            "the inner group keeps its encoding for the child dropdown to parse: {:?}",
            group.actions,
        );
    }

    /// An app holding one open dropdown, with the hover machinery running.
    fn hover_app(actions: Vec<(String, String)>) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.init_resource::<MenuBarState>()
            .insert_resource(Time::<()>::default())
            .add_plugins(plugin);
        let dropdown_entity = app
            .world_mut()
            .run_system_once(move |mut commands: Commands| {
                spawn_dropdown(&mut commands, 0.0, 30.0, 1000.0, &actions)
            })
            .expect("the dropdown spawns");
        app.update();
        let world = app.world_mut();
        let row = world
            .query_filtered::<Entity, With<SubmenuRow>>()
            .iter(world)
            .next()
            .expect("the menu has a group row");
        (app, dropdown_entity, row)
    }

    /// A pointer report the observers can be handed directly: the events
    /// carry a location and a hit they never read, so both are stand-ins.
    fn pointer_report(app: &mut App) -> (bevy::picking::pointer::Location, HitData) {
        let camera = app.world_mut().spawn_empty().id();
        // A real window, because pointer events propagate to the pointer's
        // window and stop there: an entity that only stands in for one
        // sends the traversal round again forever.
        let world = app.world_mut();
        let window = world
            .query_filtered::<Entity, With<Window>>()
            .iter(world)
            .next()
            .unwrap_or_else(|| world.spawn(Window::default()).id());
        let location = bevy::picking::pointer::Location {
            target: bevy::camera::NormalizedRenderTarget::Window(
                bevy::window::WindowRef::Entity(window)
                    .normalize(None)
                    .expect("a window reference normalizes"),
            ),
            position: Vec2::ZERO,
        };
        (location, HitData::new(camera, 0.0, None, None))
    }

    fn hover(app: &mut App, target: Entity) {
        let (location, hit) = pointer_report(app);
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location,
            Over { hit },
            target,
        ));
    }

    fn unhover(app: &mut App, target: Entity) {
        let (location, hit) = pointer_report(app);
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location,
            Out { hit },
            target,
        ));
    }

    fn advance(app: &mut App, seconds: f32) {
        app.world_mut()
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs_f32(seconds));
        app.update();
    }

    fn open_children(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<SubmenuDropdown>>()
            .iter(world)
            .collect()
    }

    /// Two groups side by side, for the sweep across them.
    fn two_groups() -> Vec<(String, String)> {
        let mut actions = submenu_row(
            "3D Objects",
            [("op:entity.add.cube".to_string(), "Cube".to_string())],
        );
        actions.extend(submenu_row(
            "Lights",
            [(
                "op:entity.add.point_light".to_string(),
                "Point Light".to_string(),
            )],
        ));
        actions
    }

    fn group_row(app: &mut App, label: &str) -> Entity {
        let world = app.world_mut();
        world
            .query::<(Entity, &SubmenuRow)>()
            .iter(world)
            .find(|(_, row)| row.label == label)
            .map(|(entity, _)| entity)
            .unwrap_or_else(|| panic!("the menu has a `{label}` group"))
    }

    /// A pointer that walks out of the menu into the scene reports only to
    /// what it landed on, and the child dropdown still folds.
    #[test]
    fn leaving_the_menu_for_the_rest_of_the_editor_folds_the_group() {
        let (mut app, _, row) = hover_app(grouped_actions());
        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY);
        assert_eq!(open_children(&mut app).len(), 1);

        // A node with no menu above it: the viewport, a panel, anything.
        let elsewhere = app.world_mut().spawn(Node::default()).id();
        unhover(&mut app, row);
        hover(&mut app, elsewhere);
        advance(&mut app, SUBMENU_CLOSE_GRACE * 2.0);

        assert!(
            open_children(&mut app).is_empty(),
            "the group stayed open over the rest of the editor",
        );
    }

    /// A pointer that leaves the window sends nothing after the leaving,
    /// so what it left has to schedule the whole close itself.
    #[test]
    fn leaving_the_window_from_inside_a_child_folds_the_group() {
        let (mut app, _, row) = hover_app(grouped_actions());
        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY);
        let child = open_children(&mut app)[0];

        hover(&mut app, child);
        unhover(&mut app, child);
        advance(&mut app, SUBMENU_CLOSE_GRACE * 2.0);

        assert!(
            open_children(&mut app).is_empty(),
            "the child outlived the pointer that left the window",
        );
    }

    /// Sweeping down a column of groups must not stack their dropdowns.
    #[test]
    fn a_sweep_across_groups_leaves_one_open() {
        let (mut app, _, _) = hover_app(two_groups());
        let first = group_row(&mut app, "3D Objects");
        let second = group_row(&mut app, "Lights");

        hover(&mut app, first);
        advance(&mut app, SUBMENU_OPEN_DELAY);
        hover(&mut app, second);
        advance(&mut app, SUBMENU_OPEN_DELAY / 4.0);
        assert!(
            open_children(&mut app).is_empty(),
            "the first group folded the moment the pointer reached the second",
        );

        advance(&mut app, SUBMENU_OPEN_DELAY);
        let open = open_children(&mut app);
        assert_eq!(open.len(), 1, "one group open at a time");
        assert_eq!(
            app.world().get::<SubmenuDropdown>(open[0]).map(|it| it.row),
            Some(second),
            "and it is the one the pointer came to rest on",
        );
    }

    #[test]
    fn a_child_opens_on_the_side_the_window_has_room_for() {
        assert_eq!(
            submenu_origin(30.0, 100.0, 280.0, 1920.0),
            Vec2::new(280.0, 30.0),
            "room on the right: the child opens off the parent's right edge",
        );
        assert_eq!(
            submenu_origin(30.0, 1600.0, 1800.0, 1920.0),
            Vec2::new(1600.0 - DROPDOWN_MIN_WIDTH, 30.0),
            "no room on the right: the child flips to the left of the parent",
        );
        assert_eq!(
            submenu_origin(30.0, 40.0, 200.0, 220.0),
            Vec2::new(0.0, 30.0),
            "no room either side: the child stays on screen",
        );
    }

    #[test]
    fn resting_on_a_group_expands_it_and_leaving_it_folds_it_back() {
        let (mut app, _, row) = hover_app(grouped_actions());

        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY / 2.0);
        assert!(
            open_children(&mut app).is_empty(),
            "a pointer sweeping across a group opens nothing",
        );

        advance(&mut app, SUBMENU_OPEN_DELAY);
        let children = open_children(&mut app);
        assert_eq!(children.len(), 1, "the group expands where it was hovered");
        let child = children[0];
        assert_eq!(
            app.world().get::<SubmenuDropdown>(child).map(|it| it.row),
            Some(row),
            "the child dropdown names the row it hangs off",
        );
        assert_eq!(
            app.world().get::<Children>(child).map(Children::len),
            Some(2),
            "the child dropdown holds the group's entries",
        );

        unhover(&mut app, row);
        advance(&mut app, SUBMENU_CLOSE_GRACE / 2.0);
        assert_eq!(
            open_children(&mut app).len(),
            1,
            "the child survives the gap between the row and itself",
        );

        advance(&mut app, SUBMENU_CLOSE_GRACE);
        assert!(
            open_children(&mut app).is_empty(),
            "a pointer that stays away folds the group back",
        );
    }

    #[test]
    fn reaching_the_child_dropdown_keeps_it_open() {
        let (mut app, _, row) = hover_app(grouped_actions());
        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY);
        let child = open_children(&mut app)[0];

        unhover(&mut app, row);
        hover(&mut app, child);
        advance(&mut app, SUBMENU_CLOSE_GRACE * 2.0);
        assert_eq!(
            open_children(&mut app),
            vec![child],
            "the pointer is on the child, so nothing closes",
        );
    }

    /// The entry beside the group, the one the pointer crosses on a
    /// diagonal to the child dropdown.
    fn plain_entry(app: &App, dropdown: Entity, row: Entity) -> Entity {
        app.world()
            .get::<Children>(dropdown)
            .expect("the menu has rows")
            .iter()
            .find(|entity| *entity != row)
            .expect("the plain entry beside the group")
    }

    /// The child opens off the side of the menu, so the way to its rows
    /// runs over the entries under the group. Crossing them is not
    /// leaving.
    #[test]
    fn crossing_an_entry_on_the_way_to_the_child_is_not_leaving() {
        let (mut app, dropdown_entity, row) = hover_app(grouped_actions());
        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY);
        let child = open_children(&mut app)[0];

        let sibling = plain_entry(&app, dropdown_entity, row);
        unhover(&mut app, row);
        hover(&mut app, sibling);
        advance(&mut app, SUBMENU_CLOSE_GRACE / 2.0);
        unhover(&mut app, sibling);
        hover(&mut app, child);
        advance(&mut app, SUBMENU_CLOSE_GRACE * 2.0);

        assert_eq!(
            open_children(&mut app),
            vec![child],
            "the group folded while the pointer was still on its way",
        );
    }

    #[test]
    fn moving_to_a_plain_entry_folds_the_open_group() {
        let (mut app, dropdown_entity, row) = hover_app(grouped_actions());
        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY);
        assert_eq!(open_children(&mut app).len(), 1);

        let sibling = plain_entry(&app, dropdown_entity, row);
        unhover(&mut app, row);
        hover(&mut app, sibling);
        advance(&mut app, SUBMENU_CLOSE_GRACE * 2.0);
        assert!(
            open_children(&mut app).is_empty(),
            "walking on down the menu leaves no dropdown behind",
        );
    }

    #[test]
    fn a_child_dropdown_clamps_and_scrolls_like_any_other() {
        let long = (0..80)
            .map(|index| (format!("op:test.{index}"), format!("Entry {index}")))
            .collect::<Vec<_>>();
        let (mut app, _, row) = hover_app(submenu_row("Long", long));
        hover(&mut app, row);
        advance(&mut app, SUBMENU_OPEN_DELAY);

        let child = open_children(&mut app)[0];
        let node = app
            .world()
            .get::<Node>(child)
            .expect("the child has a node");
        assert_eq!(
            node.overflow.y,
            OverflowAxis::Scroll,
            "a group longer than the window scrolls, like the menu it hangs off",
        );
        assert!(
            matches!(node.max_height, Val::Px(height) if height > 0.0),
            "the child is clamped to the window: {:?}",
            node.max_height,
        );
    }

    #[test]
    fn a_scripted_run_expands_a_group_without_waiting() {
        let (mut app, _, row) = hover_app(grouped_actions());
        let opened = app
            .world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut state: ResMut<SubmenuState>,
                      rows: Query<(&SubmenuRow, &ComputedNode, &UiGlobalTransform, &ChildOf)>,
                      row_ids: Query<(Entity, &SubmenuRow)>,
                      dropdowns: Query<
                    (&ComputedNode, &UiGlobalTransform),
                    With<MenuBarDropdown>,
                >,
                      windows: Query<&Window>| {
                    open_submenu_named(
                        "3D Objects",
                        &mut commands,
                        &mut state,
                        &rows,
                        &row_ids,
                        &dropdowns,
                        &windows,
                    )
                },
            )
            .expect("the system runs");
        assert!(opened, "the group is there to expand");
        app.update();

        let children = open_children(&mut app);
        assert_eq!(children.len(), 1, "the capture path opens the same child");
        assert_eq!(
            app.world()
                .get::<SubmenuDropdown>(children[0])
                .map(|it| it.row),
            Some(row),
        );
    }
}
