use bevy::math::Vec2;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use lucide_icons::Icon;

/// Center angle (radians, screen space, y-down) of wedge `i` of `n`: item 0
/// points up, the rest clockwise.
pub fn wedge_angle(i: usize, n: usize) -> f32 {
    let step = std::f32::consts::TAU / n.max(1) as f32;
    // Screen y is down, so "up" is -y == angle -PI/2 in atan2(y, x). Clockwise
    // on screen = increasing this angle.
    -std::f32::consts::FRAC_PI_2 + i as f32 * step
}

/// Which wedge the cursor points toward, or `None` inside the center deadzone
/// (or when there are no items).
pub fn highlighted_index(anchor: Vec2, cursor: Vec2, n: usize, deadzone: f32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let delta = cursor - anchor;
    if delta.length() < deadzone {
        return None;
    }
    let angle = delta.y.atan2(delta.x); // y-down screen space
    let step = std::f32::consts::TAU / n as f32;
    // Offset so wedge 0 is centered on `wedge_angle(0, n)`.
    let rel = (angle - wedge_angle(0, n)).rem_euclid(std::f32::consts::TAU);
    Some(((rel + step / 2.0) / step) as usize % n)
}

// --- Layout constants (visual tuning; not tested headlessly) ---
const RADIAL_RADIUS: f32 = 120.0;
const RADIAL_DEADZONE: f32 = 30.0;
const WEDGE_W: f32 = 116.0;
const WEDGE_H: f32 = 46.0;
const LABEL_FONT_SIZE: f32 = 12.0;
const ICON_FONT_SIZE: f32 = 15.0;

/// One radial item. `action` is opaque: the consumer maps it to behavior.
#[derive(Clone)]
pub struct RadialMenuItem {
    pub label: String,
    pub icon: Option<Icon>,
    pub action: String,
}

/// State of the currently-open menu, if any.
pub struct RadialMenuOpen {
    pub root: Entity,
    pub anchor: Vec2,
    pub items: Vec<RadialMenuItem>,
    pub highlighted: Option<usize>,
}

/// Resource tracking whether a radial menu is currently open.
#[derive(Resource, Default)]
pub struct RadialMenuState {
    pub open: Option<RadialMenuOpen>,
}

/// Fired when the user confirms a highlighted wedge. Observer event.
#[derive(Event, Debug, Clone)]
pub struct RadialMenuSelect {
    pub action: String,
}

/// Marker on the menu root node.
#[derive(Component)]
pub struct RadialMenu;

/// Marker on each wedge child, carrying its item index for highlight accenting.
#[derive(Component)]
pub struct RadialWedge {
    pub index: usize,
}

/// Loaded Lucide icon font handle, held as a resource so wedge children can
/// reference it when the menu is spawned inside a world closure.
#[derive(Resource, Clone)]
pub struct RadialIconFont(pub Handle<Font>);

/// BEI input context that owns the Escape-closes-radial-menu binding.
#[derive(Component, Default)]
pub struct RadialMenuInputContext;

/// BEI action fired when the user wants to dismiss an open radial menu.
#[derive(Default, InputAction)]
#[action_output(bool)]
pub struct RadialMenuDismissAction;

fn spawn_radial_menu_input_context(mut commands: Commands) {
    commands.spawn((
        RadialMenuInputContext,
        actions!(
            RadialMenuInputContext[(
                Action::<RadialMenuDismissAction>::new(),
                bindings!((KeyCode::Escape, Press::default()))
            )]
        ),
    ));
}

fn close_radial_menu_on_escape(
    _: On<Fire<RadialMenuDismissAction>>,
    mut commands: Commands,
    mut state: Option<ResMut<RadialMenuState>>,
) {
    let Some(ref mut state) = state else {
        return;
    };
    let Some(open) = state.open.take() else {
        return;
    };
    commands.entity(open.root).try_despawn();
}

/// Spawn the full radial menu tree and register it in `RadialMenuState`.
/// All spawning happens inside a deferred world closure so the font resource
/// and state resource are both accessible at the same point.
pub fn open_radial_menu(commands: &mut Commands, anchor: Vec2, items: Vec<RadialMenuItem>) {
    commands.queue(move |world: &mut World| {
        let font_handle = world.get_resource::<RadialIconFont>().map(|r| r.0.clone());

        let n = items.len();
        let root = world
            .spawn((
                RadialMenu,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(anchor.x),
                    top: Val::Px(anchor.y),
                    ..Default::default()
                },
                Pickable::IGNORE,
            ))
            .id();

        for (i, item) in items.iter().enumerate() {
            let a = wedge_angle(i, n);
            let offset = Vec2::new(a.cos(), a.sin()) * RADIAL_RADIUS;

            let wedge = world
                .spawn((
                    RadialWedge { index: i },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(offset.x - WEDGE_W / 2.0),
                        top: Val::Px(offset.y - WEDGE_H / 2.0),
                        width: Val::Px(WEDGE_W),
                        height: Val::Px(WEDGE_H),
                        padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                        row_gap: Val::Px(2.0),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        flex_direction: FlexDirection::Column,
                        ..Default::default()
                    },
                    BackgroundColor(Color::srgb(0.16, 0.16, 0.18)),
                    ChildOf(root),
                ))
                .id();

            // Label text.
            world.spawn((
                Text::new(item.label.clone()),
                TextFont {
                    font_size: LABEL_FONT_SIZE,
                    ..Default::default()
                },
                TextLayout::new_with_justify(Justify::Center),
                ChildOf(wedge),
            ));

            // Optional icon glyph.
            if let (Some(icon), Some(handle)) = (item.icon, &font_handle) {
                world.spawn((
                    Text::new(String::from(icon.unicode())),
                    TextFont {
                        font: handle.clone(),
                        font_size: ICON_FONT_SIZE,
                        ..Default::default()
                    },
                    ChildOf(wedge),
                ));
            }
        }

        let prev = world
            .resource_mut::<RadialMenuState>()
            .open
            .replace(RadialMenuOpen {
                root,
                anchor,
                items,
                highlighted: None,
            });

        if let Some(prev) = prev {
            despawn_root(world, prev.root);
        }
    });
}

/// Despawn a radial-menu root entity if it still exists.
fn despawn_root(world: &mut World, root: Entity) {
    if let Ok(e) = world.get_entity_mut(root) {
        e.despawn();
    }
}

/// Confirm the currently-highlighted wedge. Fires `RadialMenuSelect` if a
/// wedge is highlighted, then closes the menu regardless.
pub fn confirm_radial_menu(world: &mut World) {
    let Some(open) = world.resource_mut::<RadialMenuState>().open.take() else {
        return;
    };
    if let Some(i) = open.highlighted {
        let action = open.items[i].action.clone();
        world.trigger(RadialMenuSelect { action });
    }
    despawn_root(world, open.root);
}

/// Dismiss the currently-open menu without firing any selection event.
pub fn cancel_radial_menu(world: &mut World) {
    let Some(open) = world.resource_mut::<RadialMenuState>().open.take() else {
        return;
    };
    despawn_root(world, open.root);
}

/// Per-frame system: update `RadialMenuOpen::highlighted` from cursor position
/// and repaint wedge backgrounds to reflect the current highlight.
fn update_radial_highlight(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut state: ResMut<RadialMenuState>,
    mut wedges: Query<(&RadialWedge, &mut BackgroundColor)>,
) {
    let Some(open) = state.open.as_mut() else {
        return;
    };
    let cursor = windows.iter().next().and_then(Window::cursor_position);
    let n = open.items.len();
    open.highlighted = cursor.and_then(|c| highlighted_index(open.anchor, c, n, RADIAL_DEADZONE));
    for (wedge, mut bg) in &mut wedges {
        *bg = if Some(wedge.index) == open.highlighted {
            BackgroundColor(Color::srgb(0.30, 0.55, 0.95))
        } else {
            BackgroundColor(Color::srgb(0.20, 0.20, 0.24))
        };
    }
}

pub struct RadialMenuPlugin;

impl Plugin for RadialMenuPlugin {
    fn build(&self, app: &mut App) {
        // Load the Lucide icon font eagerly so wedge spawns never race the asset loader.
        let handle = {
            let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
            let font = Font::try_from_bytes(lucide_icons::LUCIDE_FONT_BYTES.to_vec())
                .expect("load lucide icon font");
            fonts.add(font)
        };
        app.insert_resource(RadialIconFont(handle));

        app.init_resource::<RadialMenuState>()
            .add_input_context::<RadialMenuInputContext>()
            .add_systems(Update, update_radial_highlight)
            .add_observer(close_radial_menu_on_escape)
            .add_systems(Startup, spawn_radial_menu_input_context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec2;

    #[test]
    fn highlight_resolves_cardinal_directions_for_four_items() {
        let anchor = Vec2::new(100.0, 100.0);
        let dead = 8.0;
        // Screen space is y-down; item 0 is up (-y), then clockwise.
        let up = anchor + Vec2::new(0.0, -40.0);
        let right = anchor + Vec2::new(40.0, 0.0);
        let down = anchor + Vec2::new(0.0, 40.0);
        let left = anchor + Vec2::new(-40.0, 0.0);
        assert_eq!(highlighted_index(anchor, up, 4, dead), Some(0));
        assert_eq!(highlighted_index(anchor, right, 4, dead), Some(1));
        assert_eq!(highlighted_index(anchor, down, 4, dead), Some(2));
        assert_eq!(highlighted_index(anchor, left, 4, dead), Some(3));
    }

    #[test]
    fn center_deadzone_is_no_selection() {
        let anchor = Vec2::new(100.0, 100.0);
        assert_eq!(
            highlighted_index(anchor, anchor + Vec2::new(2.0, 1.0), 4, 8.0),
            None
        );
        assert_eq!(highlighted_index(anchor, anchor, 6, 8.0), None);
    }

    #[test]
    fn empty_menu_never_highlights() {
        let anchor = Vec2::ZERO;
        assert_eq!(
            highlighted_index(anchor, Vec2::new(0.0, -40.0), 0, 8.0),
            None
        );
    }

    #[test]
    fn confirm_fires_select_for_highlighted_item() {
        use bevy::prelude::*;

        #[derive(Resource, Default)]
        struct Captured(Vec<String>);

        let mut app = App::new();
        app.init_resource::<RadialMenuState>();
        app.init_resource::<Captured>();
        app.add_observer(|trigger: On<RadialMenuSelect>, mut cap: ResMut<Captured>| {
            cap.0.push(trigger.event().action.clone());
        });

        let root = app.world_mut().spawn_empty().id();
        let item = |a: &str| RadialMenuItem {
            label: a.to_string(),
            icon: None,
            action: a.to_string(),
        };
        app.world_mut().resource_mut::<RadialMenuState>().open = Some(RadialMenuOpen {
            root,
            anchor: Vec2::ZERO,
            items: vec![item("a"), item("b")],
            highlighted: Some(1),
        });

        confirm_radial_menu(app.world_mut());

        assert_eq!(app.world().resource::<Captured>().0, vec!["b".to_string()]);
        assert!(app.world().resource::<RadialMenuState>().open.is_none());
    }

    #[test]
    fn confirm_with_no_highlight_fires_nothing_and_closes() {
        use bevy::prelude::*;

        #[derive(Resource, Default)]
        struct Captured(u32);

        let mut app = App::new();
        app.init_resource::<RadialMenuState>();
        app.init_resource::<Captured>();
        app.add_observer(|_t: On<RadialMenuSelect>, mut cap: ResMut<Captured>| {
            cap.0 += 1;
        });

        let root = app.world_mut().spawn_empty().id();
        let item = |a: &str| RadialMenuItem {
            label: a.to_string(),
            icon: None,
            action: a.to_string(),
        };
        app.world_mut().resource_mut::<RadialMenuState>().open = Some(RadialMenuOpen {
            root,
            anchor: Vec2::ZERO,
            items: vec![item("a")],
            highlighted: None,
        });

        confirm_radial_menu(app.world_mut());

        assert_eq!(app.world().resource::<Captured>().0, 0);
        assert!(app.world().resource::<RadialMenuState>().open.is_none());
    }
}
