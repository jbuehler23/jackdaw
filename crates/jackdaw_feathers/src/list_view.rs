//! Lists.
//!
//! A list is [`ListBox`] and its rows are `bevy_feathers`'
//! [`FeathersListRow`], which is a [`ListItem`](bevy::ui_widgets::ListItem)
//! painted from the theme's list-row tokens: hover, selection and the
//! disabled state come from the widget rather than from a pair of
//! hand-written pointer observers.

use bevy::feathers::controls::FeathersListRow;
use bevy::prelude::*;
use bevy::ui_widgets::ListBox;

use crate::tokens;

pub fn plugin(app: &mut App) {
    app.add_systems(Update, setup_list_rows);
}

/// A list container: a column of rows the list widget owns.
pub fn list_view() -> impl Bundle {
    (
        ListBox,
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::left(px(tokens::SPACING_LG)),
            ..default()
        },
    )
}

/// A row of a list.
///
/// The returned bundle is the request, not the row: `FeathersListRow` is
/// a scene component, so the crate's setup pass applies its scene to the
/// entity and puts the layout the caller spawned it with back
/// afterwards.
pub fn list_row() -> impl Bundle {
    EditorListRow { applied: false }
}

/// Marks an entity that is to become a `FeathersListRow`.
#[derive(Component)]
pub struct EditorListRow {
    applied: bool,
}

fn setup_list_rows(
    mut commands: Commands,
    mut rows: Query<(Entity, &mut EditorListRow), Added<EditorListRow>>,
) {
    for (entity, mut row) in &mut rows {
        if row.applied {
            continue;
        }
        row.applied = true;
        commands.queue(move |world: &mut World| apply_list_row(world, entity));
    }
}

/// Put the row's scene on `entity`, keeping the layout it was spawned
/// with: the scene writes its own row `Node` over the entity, and a
/// caller that sized or padded its row means that.
fn apply_list_row(world: &mut World, entity: Entity) {
    let node = world.get::<Node>(entity).cloned();
    let applied = {
        let Ok(mut row) = world.get_entity_mut(entity) else {
            return;
        };
        row.apply_scene(bsn! { @FeathersListRow })
    };
    if let Err(error) = applied {
        error!("a list row did not spawn: {error}");
        return;
    }
    let Ok(mut row) = world.get_entity_mut(entity) else {
        return;
    };
    if let Some(node) = node {
        row.insert(node);
    }
}
