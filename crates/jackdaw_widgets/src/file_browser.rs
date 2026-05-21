use bevy_ecs::prelude::*;
use bevy_app::prelude::*;

pub struct FileBrowserPlugin;

impl Plugin for FileBrowserPlugin {
    fn build(&self, _app: &mut App) {}
}

/// Marker for the file browser container.
#[derive(Component)]
pub struct FileBrowser;

/// Represents a single file/directory item in the browser.
#[derive(Component, Clone)]
pub struct FileBrowserItem {
    pub path: String,
    pub is_directory: bool,
    pub file_name: String,
}

/// Event fired when a file item is double-clicked.
#[derive(Event, Debug, Clone)]
pub struct FileItemDoubleClicked {
    pub path: String,
    pub is_directory: bool,
}

/// Event fired when a file item drag starts.
#[derive(Event, Debug, Clone)]
pub struct FileItemDragStarted {
    pub path: String,
}
