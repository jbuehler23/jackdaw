use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_input::prelude::*;

pub struct MenuBarPlugin;

impl Plugin for MenuBarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuBarState>()
            .add_observer(close_menu_on_action)
            .add_systems(Update, close_menu_on_click_outside);
    }
}

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

    // Close on Escape or left-click outside
    if mouse.just_pressed(MouseButton::Left) || keyboard.just_pressed(KeyCode::Escape) {
        if let Some(dropdown) = state.dropdown_entity.take() {
            commands.entity(dropdown).despawn();
        }
        state.open_menu = None;
    }
}
