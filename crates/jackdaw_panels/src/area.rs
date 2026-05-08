use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::tree::TabId;

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
    /// Per-instance handle. Two `DockWindow` entities with the same
    /// `descriptor_id` (e.g. two Outliner tabs) carry distinct
    /// `tab_id`s so the reconciler / activate / close paths can tell
    /// them apart.
    pub tab_id: TabId,
}

/// `Some(tab_id)` of the active tab in this leaf, or `None` for an
/// empty leaf. Reconciler reads this to decide which content entity
/// to show. Tracking by `TabId` rather than `window_id` lets two tabs
/// of the same window kind coexist without their content stacking.
#[derive(Component, Clone, Debug, Default)]
pub struct ActiveDockWindow(pub Option<TabId>);

#[derive(Component)]
pub struct DockTabBar;

#[derive(Component)]
pub struct DockTab {
    pub window_id: String,
    pub tab_id: TabId,
}

#[derive(Component)]
pub struct DockTabCloseButton {
    pub window_id: String,
    pub tab_id: TabId,
}

#[derive(Component)]
pub struct DockTabContent {
    pub window_id: String,
    pub tab_id: TabId,
}

/// A possible default area for a window. Use this to specify where on the screen a window should open by default when the user adds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultArea {
    Left,
    Center,
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
            Some(area) => area.anchor_id(),
            None => String::new(),
        }
    }
}

impl ToAnchorId for DefaultArea {
    fn anchor_id(self) -> String {
        match self {
            DefaultArea::Left => "left",
            DefaultArea::Center => "center",
            DefaultArea::BottomDock => "bottom_dock",
            DefaultArea::RightSidebar => "right_sidebar",
        }
        .into()
    }
}
