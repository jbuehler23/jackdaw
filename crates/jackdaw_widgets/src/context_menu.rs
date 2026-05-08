use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};

/// System set containing the context menu close systems.
/// Order your context menu openers `.after(ContextMenuCloseSet)` to avoid
/// the close system immediately despawning a just-created menu.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContextMenuCloseSystems;

pub struct ContextMenuPlugin;

impl Plugin for ContextMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_input_context::<ContextMenuInputContext>()
            .add_systems(
                Update,
                close_context_menu_on_click.in_set(ContextMenuCloseSystems),
            )
            .add_observer(close_context_menu_on_escape)
            .add_systems(Startup, spawn_context_menu_input_context);
    }
}

/// BEI input context that owns the Escape-closes-context-menu binding.
/// Lives on its own entity so the binding can be remapped or scoped
/// without colliding with other Escape handlers (e.g., modal cancel).
#[derive(Component, Default)]
pub struct ContextMenuInputContext;

/// BEI action fired when the user wants to dismiss an open context menu.
/// Bound to `Esc` by default; an observer translates the FIRE event
/// into a despawn of the active menu.
#[derive(Default, InputAction)]
#[action_output(bool)]
pub struct ContextMenuDismissAction;

fn spawn_context_menu_input_context(mut commands: Commands) {
    commands.spawn((
        ContextMenuInputContext,
        actions!(
            ContextMenuInputContext[(
                Action::<ContextMenuDismissAction>::new(),
                bindings!((KeyCode::Escape, Press::default()))
            )]
        ),
    ));
}

/// Marker component for the context menu container.
#[derive(Component)]
pub struct ContextMenu;

/// Individual menu item with an action identifier.
#[derive(Component)]
pub struct ContextMenuItem {
    pub action: String,
    /// The entity that the context menu was opened for (stored at spawn time).
    pub target_entity: Option<Entity>,
}

/// Event fired when a context menu item is clicked.
#[derive(Event, Debug, Clone)]
pub struct ContextMenuAction {
    pub action: String,
    /// The entity that the context menu was opened for (e.g., the hierarchy entity).
    pub target_entity: Option<Entity>,
}

/// Resource tracking the context menu's target entity.
#[derive(Resource, Default)]
pub struct ContextMenuState {
    pub target_entity: Option<Entity>,
    pub menu_entity: Option<Entity>,
}

/// Close context menu when clicking outside it.
///
/// Only listens for LMB. Right-click is the open gesture and races
/// the menu-open operator on the same frame; if this system also
/// closed on RMB, the just-spawned menu would be torn down before
/// the user could see it (system ordering between BEI-dispatched
/// operators and `Update` systems is non-deterministic for this
/// kind of cross-input race).
fn close_context_menu_on_click(
    mouse: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
    mut state: Option<ResMut<ContextMenuState>>,
) {
    let Some(ref mut state) = state else {
        return;
    };
    if !should_close_on_pointer_press(&mouse, state) {
        return;
    }
    if let Some(menu) = state.menu_entity.take() {
        commands.entity(menu).despawn();
    }
    state.target_entity = None;
}

/// True when the LMB just-pressed input should drop a context menu
/// (one is currently open). Pulled out so the regression test for
/// the right-click-doesn't-self-close bug runs without spinning up
/// a full `App`.
fn should_close_on_pointer_press(
    mouse: &ButtonInput<MouseButton>,
    state: &ContextMenuState,
) -> bool {
    if state.menu_entity.is_none() {
        return false;
    }
    mouse.just_pressed(MouseButton::Left)
}

/// Close on Escape via the BEI dismiss action. Routing through BEI
/// (instead of raw `keyboard.just_pressed(KeyCode::Escape)`) keeps the
/// keybind remappable and lets BEI's context layering decide whether
/// our action wins for the frame.
fn close_context_menu_on_escape(
    _: On<Fire<ContextMenuDismissAction>>,
    mut commands: Commands,
    mut state: Option<ResMut<ContextMenuState>>,
) {
    let Some(ref mut state) = state else {
        return;
    };
    if let Some(menu) = state.menu_entity.take() {
        commands.entity(menu).despawn();
    }
    state.target_entity = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_menu_state() -> ContextMenuState {
        ContextMenuState {
            target_entity: Some(Entity::from_raw_u32(7).unwrap()),
            menu_entity: Some(Entity::from_raw_u32(8).unwrap()),
        }
    }

    /// Regression: right-click was the open gesture; if the close
    /// system also fired on RMB, the just-spawned context menu would
    /// be torn down on the same frame and the user never saw it.
    /// The close trigger must ignore RMB entirely.
    #[test]
    fn rmb_press_does_not_close_open_menu() {
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Right);
        let state = open_menu_state();
        assert!(
            !should_close_on_pointer_press(&mouse, &state),
            "right-click is the open gesture; close must not fire on RMB",
        );
    }

    /// LMB outside the menu still closes it; that's the expected
    /// dismiss-on-click-outside behaviour.
    #[test]
    fn lmb_press_closes_open_menu() {
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        let state = open_menu_state();
        assert!(
            should_close_on_pointer_press(&mouse, &state),
            "LMB-press should still close the menu",
        );
    }

    /// With no menu open, the close system must do nothing - the
    /// `should_close` predicate gates the despawn so that a stray
    /// LMB during normal scene editing isn't routed through this
    /// path looking for a menu to close.
    #[test]
    fn lmb_press_does_nothing_without_open_menu() {
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        let state = ContextMenuState::default();
        assert!(!should_close_on_pointer_press(&mouse, &state));
    }
}
