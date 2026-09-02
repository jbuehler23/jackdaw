use bevy::prelude::*;
use jackdaw_commands::KeymapCapture;

pub struct MenuBarPlugin;

impl Plugin for MenuBarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuBarState>()
            .add_observer(close_menu_on_action)
            .add_observer(note_a_press_inside_the_menu)
            .add_systems(Update, close_menu_on_click_outside.in_set(MenuBarClose));
    }
}

/// The pass that takes an open menu down on a press outside it.
///
/// A row is activated on the *release*, which lands a frame or more after
/// the press that started it, so a pass that closed on every press would
/// despawn the row before it ever fired. Only a press that landed outside
/// the bar and its dropdowns closes the menu -- see
/// [`MenuBarState::press_inside`] for where that is decided. What closes
/// a menu from the inside is the row itself, once it has run.
///
/// Public so a click handler that wants the menu to stay open can be
/// ordered before it and write [`MenuBarState::hold_open`] first.
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
    /// Whether the press being handled this frame landed on the bar or on
    /// one of its dropdowns.
    ///
    /// Written by an observer on the press itself: the entity the
    /// press was delivered to *is*
    /// the hit test, already resolved by the picking backend against
    /// whatever is stacked over the editor. Spent by the close pass on
    /// the frame it was written.
    pub press_inside: bool,
}

/// Fired when a menu item is clicked.
#[derive(Event, Debug, Clone)]
pub struct MenuAction {
    pub action: String,
}

/// Take the open menu down: despawn its dropdown and forget it.
///
/// Public because a row that has run its command closes the menu it was
/// in, and that row lives in the crate that draws it.
pub fn close_open_menu(commands: &mut Commands, state: &mut MenuBarState) {
    if let Some(dropdown) = state.dropdown_entity.take()
        && let Ok(mut entity) = commands.get_entity(dropdown)
    {
        entity.try_despawn();
    }
    state.open_menu = None;
    state.hold_open = false;
    state.press_inside = false;
}

fn close_menu_on_action(
    _: On<MenuAction>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
) {
    close_open_menu(&mut commands, &mut state);
}

/// Remember a press that landed on the bar, on a dropdown, or on anything
/// inside one, so the close pass leaves the menu up for it.
fn note_a_press_inside_the_menu(
    press: On<Pointer<Press>>,
    parts: Query<(), Or<(With<MenuBar>, With<MenuBarDropdown>, With<MenuBarItem>)>>,
    parents: Query<&ChildOf>,
    mut state: ResMut<MenuBarState>,
) {
    if state.open_menu.is_none() || press.button != PointerButton::Primary {
        return;
    }
    let inside = core::iter::successors(Some(press.original_event_target()), |entity| {
        parents.get(*entity).ok().map(ChildOf::parent)
    })
    .any(|ancestor| parts.contains(ancestor));
    if inside {
        state.press_inside = true;
    }
}

fn close_menu_on_click_outside(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    capture: Option<Res<KeymapCapture>>,
    mut commands: Commands,
    mut state: ResMut<MenuBarState>,
) {
    if state.open_menu.is_none() || KeymapCapture::is_recording(capture.as_deref()) {
        return;
    }

    // Escape closes wherever the pointer is; a left press closes only when
    // it landed outside the menu, because a press inside one is the first
    // half of a row's own click and the row has not fired yet.
    if !keyboard.just_pressed(KeyCode::Escape) {
        if !mouse.just_pressed(MouseButton::Left) {
            state.press_inside = false;
            return;
        }
        if state.hold_open {
            state.hold_open = false;
            state.press_inside = false;
            return;
        }
        if state.press_inside {
            state.press_inside = false;
            return;
        }
    }
    close_open_menu(&mut commands, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escape closes the open menu, and Escape is also a chord somebody may
    /// be recording. Naming a key must not close the menu the recorder is
    /// sitting in front of.
    #[test]
    fn a_recorded_escape_leaves_the_open_menu_open() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<MenuBarState>();
        app.insert_resource(KeymapCapture { recording: true });
        let item = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<MenuBarState>().open_menu = Some(item);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);

        app.world_mut()
            .run_system_cached(close_menu_on_click_outside)
            .expect("the system runs");
        assert_eq!(
            app.world().resource::<MenuBarState>().open_menu,
            Some(item),
            "the menu survived the press that was naming a chord",
        );

        app.world_mut().resource_mut::<KeymapCapture>().recording = false;
        app.world_mut()
            .run_system_cached(close_menu_on_click_outside)
            .expect("the system runs");
        assert_eq!(
            app.world().resource::<MenuBarState>().open_menu,
            None,
            "and with nobody recording the same press closes it",
        );
    }

    /// A press the observer placed inside the menu leaves it open, so the
    /// row it started on lives long enough to fire on the release.
    #[test]
    fn a_press_inside_the_menu_leaves_it_open_and_one_outside_closes_it() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<MenuBarState>();
        let item = app.world_mut().spawn_empty().id();
        {
            let mut state = app.world_mut().resource_mut::<MenuBarState>();
            state.open_menu = Some(item);
            state.press_inside = true;
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);

        app.world_mut()
            .run_system_cached(close_menu_on_click_outside)
            .expect("the system runs");
        assert_eq!(
            app.world().resource::<MenuBarState>().open_menu,
            Some(item),
            "a press on a row does not take the row away before it fires",
        );
        assert!(
            !app.world().resource::<MenuBarState>().press_inside,
            "and the note is spent, so the next press is judged on its own",
        );

        app.world_mut()
            .run_system_cached(close_menu_on_click_outside)
            .expect("the system runs");
        assert_eq!(
            app.world().resource::<MenuBarState>().open_menu,
            None,
            "a press nobody placed inside the menu closes it",
        );
    }
}
