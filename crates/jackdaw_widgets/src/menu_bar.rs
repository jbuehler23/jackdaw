use bevy::prelude::*;

pub struct MenuBarPlugin;

impl Plugin for MenuBarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuBarState>()
            .add_observer(close_menu_on_action)
            .add_systems(Update, close_menu_on_click_outside.in_set(MenuBarClose));
    }
}

/// The pass that takes an open menu down on a click outside it.
///
/// Public so a click handler that wants the menu to stay open can be
/// ordered before it: the press and the click land on the same frame,
/// so [`MenuBarState::hold_open`] has to be written first to be read.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MenuBarClose;

/// Marker on the root menu bar node.
#[derive(Component)]
pub struct MenuBar;

/// A top-level menu bar item (e.g., "File", "Edit").
#[derive(Component)]
pub struct MenuBarItem {
    pub label: String,
    /// (`action_id`, `display_label`) pairs for the dropdown.
    pub actions: Vec<(String, String)>,
}

/// Marker on the dropdown container spawned when a menu is opened.
#[derive(Component)]
pub struct MenuBarDropdown;

/// Marker on individual items inside a menu dropdown.
#[derive(Component)]
pub struct MenuBarDropdownItem {
    pub action: String,
}

/// Tracks which menu is currently open.
#[derive(Resource, Default)]
pub struct MenuBarState {
    /// The `MenuBarItem` entity whose dropdown is open, if any.
    pub open_menu: Option<Entity>,
    /// The dropdown entity, if spawned.
    pub dropdown_entity: Option<Entity>,
    /// Set by a click that belongs to the open menu and must not close
    /// it, such as a row that only flips a box. Spent on the next close
    /// pass and cleared by every close, so it holds for exactly the one
    /// click and never survives to swallow a later one.
    pub hold_open: bool,
}

/// Fired when a menu item is clicked.
#[derive(Event, Debug, Clone)]
pub struct MenuAction {
    pub action: String,
}

fn close_menu_on_action(
    _: On<MenuAction>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
) {
    if let Some(dropdown) = state.dropdown_entity.take() {
        commands.entity(dropdown).despawn();
    }
    state.open_menu = None;
    state.hold_open = false;
}

fn close_menu_on_click_outside(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
) {
    if state.open_menu.is_none() {
        return;
    }

    // Escape closes whatever was clicked; a left press closes unless the
    // click that made it belongs to the menu.
    if !keyboard.just_pressed(KeyCode::Escape) {
        if !mouse.just_pressed(MouseButton::Left) {
            return;
        }
        if state.hold_open {
            state.hold_open = false;
            return;
        }
    }
    if let Some(dropdown) = state.dropdown_entity.take() {
        commands.entity(dropdown).despawn();
    }
    state.open_menu = None;
    state.hold_open = false;
}
