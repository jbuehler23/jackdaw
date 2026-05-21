use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_picking::hover::Hovered;
use bevy_ui::prelude::*;
use bevy_window::prelude::*;
use bevy_window::{CursorIcon, SystemCursorIcon};

pub fn plugin(app: &mut App) {
    app.init_resource::<ManagedCursor>()
        .add_systems(Update, update_cursors);
}

#[derive(Component)]
pub struct HoverCursor(pub SystemCursorIcon);

#[derive(Component)]
pub struct ActiveCursor(pub SystemCursorIcon);

#[derive(Resource, Default)]
struct ManagedCursor(Option<SystemCursorIcon>);

fn update_cursors(
    active_cursors: Query<&ActiveCursor>,
    hover_cursors: Query<(&HoverCursor, &Hovered, Option<&ZIndex>)>,
    window: Single<Entity, With<Window>>,
    mut commands: Commands,
    mut managed: ResMut<ManagedCursor>,
) {
    let desired = if let Some(active) = active_cursors.iter().next() {
        Some(active.0)
    } else {
        hover_cursors
            .iter()
            .filter(|(_, hovered, _)| hovered.get())
            .max_by_key(|(_, _, z)| z.map(|z| z.0).unwrap_or(0))
            .map(|(hover, _, _)| hover.0)
    };

    if managed.0 == desired {
        return;
    }

    match desired {
        Some(icon) => {
            commands.entity(*window).insert(CursorIcon::from(icon));
        }
        None => {
            commands.entity(*window).remove::<CursorIcon>();
        }
    }

    managed.0 = desired;
}
