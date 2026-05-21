use bevy_ecs::prelude::*;
use bevy_app::prelude::*;

/// Marker for a list view container
#[derive(Component)]
pub struct ListView;

/// Marker for an individual list item. Stores its index.
#[derive(Component)]
pub struct ListItem {
    pub index: usize,
}

/// Content area within a list item (caller populates this)
#[derive(Component)]
pub struct ListItemContent;

pub struct ListViewPlugin;

impl Plugin for ListViewPlugin {
    fn build(&self, _app: &mut App) {}
}
