//! Screen-space regions consumed by platform-native window hit testing.

use bevy::prelude::*;

#[cfg(target_os = "windows")]
use bevy::ui::{ComputedNode, UiGlobalTransform};

#[cfg(target_os = "windows")]
use crate::chrome::WindowChromeStyle;
#[cfg(target_os = "windows")]
use crate::controls::{WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize};
#[cfg(target_os = "windows")]
use crate::header::WindowHeaderRoot;

/// Client-area rectangle in physical pixels (origin: top-left of the window).
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClientRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

#[cfg(target_os = "windows")]
impl ClientRect {
    /// Axis-aligned bounds in physical pixels. `UiGlobalTransform` translation is the node center.
    pub fn from_node(node: &ComputedNode, transform: &UiGlobalTransform) -> Self {
        let size = node.size();
        let (_scale, _angle, center) = transform.to_scale_angle_translation();
        let half = size * 0.5;
        return Self {
            min_x: center.x - half.x,
            min_y: center.y - half.y,
            max_x: center.x + half.x,
            max_y: center.y + half.y,
        };
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        return x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y;
    }

    pub fn is_valid(&self) -> bool {
        return self.max_x > self.min_x && self.max_y > self.min_y;
    }
}

/// Caption button under the cursor during non-client hover (Windows).
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub enum NativeCaptionButton {
    Minimize,
    Maximize,
    Close,
}

/// Hover / pressed state for Win32-drawn caption buttons.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Default, Resource)]
pub struct NativeCaptionHover {
    pub hovered: Option<NativeCaptionButton>,
    pub pressed: Option<NativeCaptionButton>,
}

/// Latest layout-derived regions for native non-client hit testing.
#[cfg(target_os = "windows")]
#[derive(Clone, Debug, Default, Resource)]
pub struct NativeHitTestRegions {
    pub header_drag: Option<ClientRect>,
    pub minimize: Option<ClientRect>,
    pub maximize: Option<ClientRect>,
    pub close: Option<ClientRect>,
    pub client_blocks: Vec<ClientRect>,
}

/// Interactive header widgets that must remain in the client area (menus, tabs, links).
///
/// On Windows, tag any interactive widget placed inside the title-bar drag region with this so
/// Win32 `WM_NCHITTEST` treats it as client area instead of a window-drag surface.
#[derive(Component)]
pub struct NativeHitTestClient;

#[cfg(target_os = "windows")]
mod windows;

pub(crate) fn build(_app: &mut App) {
    #[cfg(target_os = "windows")]
    {
        _app.init_resource::<NativeHitTestRegions>()
            .init_resource::<NativeCaptionHover>()
            .add_systems(
                PostUpdate,
                (
                    sync_native_hit_test_regions,
                    sync_caption_hover_from_cursor,
                    crate::controls::windows_caption::sync_windows_caption_chrome,
                )
                    .chain(),
            )
            .add_systems(PostUpdate, windows::install_primary_window_subclass);
    }
}

#[cfg(target_os = "windows")]
fn sync_native_hit_test_regions(
    style: Res<WindowChromeStyle>,
    mut regions: ResMut<NativeHitTestRegions>,
    primary_window: Query<(Entity, &Window), With<bevy::window::PrimaryWindow>>,
    header_roots: Query<(&ComputedNode, &UiGlobalTransform), With<WindowHeaderRoot>>,
    minimize_buttons: Query<(&ComputedNode, &UiGlobalTransform), With<WindowControlsMinimize>>,
    maximize_buttons: Query<(&ComputedNode, &UiGlobalTransform), With<WindowControlsMaximize>>,
    close_buttons: Query<(&ComputedNode, &UiGlobalTransform), With<WindowControlsClose>>,
    client_blocks: Query<(&ComputedNode, &UiGlobalTransform), With<NativeHitTestClient>>,
) {
    if !style.uses_native_hit_testing() {
        *regions = NativeHitTestRegions::default();
        windows::publish_regions(&regions);
        return;
    }

    if primary_window.single().is_err() {
        return;
    }

    let header_drag = header_roots
        .single()
        .ok()
        .map(|(node, transform)| ClientRect::from_node(node, transform));

    let minimize = minimize_buttons
        .iter()
        .next()
        .map(|(node, transform)| ClientRect::from_node(node, transform));
    let maximize = maximize_buttons
        .iter()
        .next()
        .map(|(node, transform)| ClientRect::from_node(node, transform));
    let close = close_buttons
        .iter()
        .next()
        .map(|(node, transform)| ClientRect::from_node(node, transform));

    let client_blocks = client_blocks
        .iter()
        .map(|(node, transform)| ClientRect::from_node(node, transform))
        .filter(|rect| rect.is_valid())
        .collect();

    *regions = NativeHitTestRegions {
        header_drag,
        minimize,
        maximize,
        close,
        client_blocks,
    };
    windows::publish_regions(&regions);
}

#[cfg(target_os = "windows")]
fn sync_caption_hover_from_cursor(
    _main_thread: bevy::ecs::system::NonSendMarker,
    style: Res<WindowChromeStyle>,
    regions: Res<NativeHitTestRegions>,
    primary_window: Query<(Entity, &Window), With<bevy::window::PrimaryWindow>>,
    mut hover: ResMut<NativeCaptionHover>,
) {
    if !style.uses_native_hit_testing() {
        hover.hovered = None;
        return;
    }

    let Ok((window_entity, window)) = primary_window.single() else {
        return;
    };

    let cursor = windows::primary_window_client_cursor(window_entity)
        .or_else(|| window.physical_cursor_position());

    let Some(cursor) = cursor else {
        hover.hovered = None;
        hover.pressed = windows::caption_button_pressed();
        return;
    };

    hover.hovered = caption_button_at_cursor(&regions, cursor.x, cursor.y);
    hover.pressed = windows::caption_button_pressed();
}

#[cfg(target_os = "windows")]
fn caption_button_at_cursor(
    regions: &NativeHitTestRegions,
    x: f32,
    y: f32,
) -> Option<NativeCaptionButton> {
    if let Some(rect) = regions.close {
        if rect.is_valid() && rect.contains(x, y) {
            return Some(NativeCaptionButton::Close);
        }
    }
    if let Some(rect) = regions.maximize {
        if rect.is_valid() && rect.contains(x, y) {
            return Some(NativeCaptionButton::Maximize);
        }
    }
    if let Some(rect) = regions.minimize {
        if rect.is_valid() && rect.contains(x, y) {
            return Some(NativeCaptionButton::Minimize);
        }
    }
    return None;
}
