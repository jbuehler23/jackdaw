use bevy::feathers::cursor::CursorIconPlugin;
use bevy::prelude::*;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use crate::caption_controls::load_caption_font;
#[cfg(target_os = "macos")]
use crate::macos_titlebar;

/// Styling values used in the window chrome.
#[derive(Resource, Clone, Debug)]
pub struct WindowChromeTheme {
    /// Height of the title bar row, in logical pixels.
    pub title_bar_height: f32,
    /// Background color of the shell and title bar.
    pub window_background: Color,
    /// Left inset reserved for macOS traffic lights.
    pub macos_traffic_light_inset: f32,
    /// Horizontal origin of the macOS traffic lights.
    pub macos_traffic_light_position_x: f32,
    /// Corner radius for the window shell and title bar on Linux / FreeBSD, in logical pixels.
    pub linux_corner_radius: f32,
    /// Styling for client-side caption buttons.
    pub caption: CaptionTheme,
}

/// Styling for client-side caption buttons.
#[derive(Clone, Debug)]
pub struct CaptionTheme {
    /// Icon color for minimize / maximize / close.
    pub icon_color: Color,
    /// Hover background for the minimize and maximize buttons.
    pub button_hover_background: Color,
    /// Hover background for the close button.
    pub close_hover_background: Color,
    /// Width of each caption button, in logical pixels.
    pub button_width: f32,
    /// Glyph font size, in logical pixels.
    pub glyph_size: f32,
}

impl Default for CaptionTheme {
    fn default() -> Self {
        Self {
            icon_color: Color::srgb(0.925, 0.925, 0.925),
            button_hover_background: Color::srgb(0.165, 0.165, 0.180),
            close_hover_background: Color::srgb(220.0 / 255.0, 38.0 / 255.0, 38.0 / 255.0),
            button_width: 46.0,
            glyph_size: 10.0,
        }
    }
}

impl Default for WindowChromeTheme {
    fn default() -> Self {
        Self {
            title_bar_height: 36.0,
            window_background: Color::srgb(0.122, 0.122, 0.141),
            macos_traffic_light_inset: 78.0,
            macos_traffic_light_position_x: 12.0,
            linux_corner_radius: 8.0,
            caption: CaptionTheme::default(),
        }
    }
}

/// Plugin which handles the custom window chrome.
///
/// The window itself must be created with `primary_window`: [`crate::primary_window_attributes`]
/// into Bevy's `WindowPlugin`.
pub struct WindowChromePlugin {
    pub theme: WindowChromeTheme,
}

impl WindowChromePlugin {
    pub fn new(theme: WindowChromeTheme) -> Self {
        Self { theme }
    }
}

impl Plugin for WindowChromePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.theme.clone());
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        {
            let caption_font = {
                let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
                load_caption_font(&mut fonts)
            };
            app.insert_resource(caption_font);
        }

        #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
        {
            if !app.is_plugin_added::<CursorIconPlugin>() {
                app.add_plugins(CursorIconPlugin);
            }
            crate::title_bar::register_drag_region_handlers(app);
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
            {
                crate::caption_controls::build(app);
                app.add_observer(crate::resize::on_resize_edge_press);
                app.add_systems(Last, crate::resize::sync_resize_overlay_pickability);
            }
            #[cfg(target_os = "windows")]
            {
                app.add_systems(PostUpdate, crate::window::apply_windows_corner_round);
            }
            #[cfg(target_os = "macos")]
            {
                macos_titlebar::set_theme(self.theme.clone());
                app.add_systems(
                    PostUpdate,
                    (
                        macos_titlebar::on_macos_window_created,
                        macos_titlebar::sync_macos_window_shell_state,
                    ),
                );
            }
        }
    }
}
