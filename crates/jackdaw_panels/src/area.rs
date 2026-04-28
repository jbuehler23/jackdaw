use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Resource, Clone, Default)]
pub struct IconFontHandle(pub Handle<Font>);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockAreaStyle {
    #[default]
    TabBar,
    IconSidebar,
    /// No tab bar; the panel content provides its own header.
    /// Used for single-window areas or panels with internal tabs.
    Headless,
}

#[derive(Component, Clone, Debug)]
pub struct DockArea {
    pub id: String,
    pub style: DockAreaStyle,
}

#[derive(Component, Clone, Debug)]
pub struct DockWindow {
    pub descriptor_id: String,
}

#[derive(Component, Clone, Debug, Default)]
pub struct ActiveDockWindow(pub Option<String>);

#[derive(Component)]
pub struct DockTabBar;

#[derive(Component)]
pub struct DockTab {
    pub window_id: String,
}

#[derive(Component)]
pub struct DockTabCloseButton {
    pub window_id: String,
}

#[derive(Component)]
pub struct DockTabContent {
    pub window_id: String,
}

/// A possible default area for a window. Use this to specify where on the screen a window should open by default when the user adds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultArea {
    Left,
    BottomDock,
    RightSidebar,
}

/// Trait used to convert a [`DefaultArea`] or [`Option<DefaultArea>`] into a string anchor ID.
pub trait ToAnchorId: Copy {
    fn anchor_id(self) -> String;
}
impl ToAnchorId for Option<DefaultArea> {
    fn anchor_id(self) -> String {
        match self {
            Some(area @ DefaultArea::Left)
            | Some(area @ DefaultArea::BottomDock)
            | Some(area @ DefaultArea::RightSidebar) => area.anchor_id(),
            None => String::new(),
        }
    }
}

impl ToAnchorId for DefaultArea {
    fn anchor_id(self) -> String {
        match self {
            DefaultArea::Left => "left",
            DefaultArea::BottomDock => "bottom_dock",
            DefaultArea::RightSidebar => "right_sidebar",
        }
        .into()
    }
}
