use bevy::prelude::*;

/// Draws the camera it sits on at a fixed low resolution, then scales that
/// image up to fill the camera's window, for pixel art.
///
/// Only a running game expands it: the camera draws into an image of
/// `resolution` pixels sampled nearest, and a second camera shows that image
/// scaled by `scale` and centred, clearing the rest of the window to
/// `letterbox`. A zero resolution leaves the camera as it is.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default, @crate::EditorCategory::new("2D"))]
pub struct PixelCanvas {
    /// The canvas size in pixels.
    pub resolution: UVec2,
    /// How the canvas grows to fill its window.
    pub scale: CanvasScale,
    /// The colour of the window area the canvas leaves uncovered.
    pub letterbox: Color,
    /// Draw the camera at whole canvas pixels, so slow motion does not
    /// shimmer. The authored transform keeps its fractional position.
    pub pixel_snap: bool,
}

impl Default for PixelCanvas {
    fn default() -> Self {
        Self {
            resolution: UVec2::ZERO,
            scale: CanvasScale::Integer,
            letterbox: Color::BLACK,
            pixel_snap: true,
        }
    }
}

/// How a [`PixelCanvas`] grows to fill its window.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum CanvasScale {
    /// The largest whole multiple that fits, at least 1, so every canvas pixel
    /// covers the same number of window pixels.
    #[default]
    Integer,
    /// The largest uniform scale that fits.
    Fit,
    /// The whole window, whatever its aspect.
    Stretch,
}

/// Reflect type path for [`PixelCanvas`].
pub const PIXEL_CANVAS_TYPE_PATH: &str = "jackdaw_scene_types::pixel_canvas::PixelCanvas";

impl PixelCanvas {
    /// A canvas of `width` by `height` pixels with the default settings.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            resolution: UVec2::new(width, height),
            ..default()
        }
    }

    /// Whether the resolution is set, so the canvas takes effect.
    pub fn is_set(&self) -> bool {
        self.resolution.x > 0 && self.resolution.y > 0
    }

    /// The rectangle the canvas covers in a target of `target` pixels, from
    /// its top-left corner. Centred on whole pixels; in integer mode it
    /// overhangs a target smaller than the canvas.
    pub fn display_rect(&self, target: Vec2) -> Rect {
        if !self.is_set() {
            return Rect::from_corners(Vec2::ZERO, target);
        }
        let resolution = self.resolution.as_vec2();
        let fit = (target / resolution).min_element();
        let size = match self.scale {
            CanvasScale::Integer => resolution * fit.floor().max(1.0),
            CanvasScale::Fit => (resolution * fit).min(target),
            CanvasScale::Stretch => target,
        };
        let min = ((target - size) / 2.0).floor();
        Rect::from_corners(min, min + size)
    }

    /// Where `position` in a target of `target` pixels lands on the canvas,
    /// in canvas pixels from its top-left corner, whether or not it is on it.
    pub fn canvas_position(&self, target: Vec2, position: Vec2) -> Vec2 {
        let rect = self.display_rect(target);
        (position - rect.min) / rect.size() * self.resolution.as_vec2()
    }

    /// [`Self::canvas_position`] when it lands on the canvas, `None` in the
    /// letterbox.
    pub fn canvas_pixel(&self, target: Vec2, position: Vec2) -> Option<Vec2> {
        let pixel = self.canvas_position(target, position);
        let on = pixel.cmpge(Vec2::ZERO).all() && pixel.cmplt(self.resolution.as_vec2()).all();
        on.then_some(pixel)
    }

    /// `position`, a camera centre, moved so the canvas edges fall on whole
    /// canvas pixels of `units_per_pixel` world units each.
    pub fn snap(&self, position: Vec2, units_per_pixel: f32) -> Vec2 {
        if !(units_per_pixel.is_finite() && units_per_pixel > 0.0) {
            return position;
        }
        let half = self.resolution.as_vec2() / 2.0 * units_per_pixel;
        ((position - half) / units_per_pixel).round() * units_per_pixel + half
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    fn canvas(scale: CanvasScale) -> PixelCanvas {
        PixelCanvas {
            scale,
            ..PixelCanvas::new(320, 180)
        }
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    fn rect(min: (f32, f32), size: (f32, f32)) -> Rect {
        let min = Vec2::new(min.0, min.1);
        Rect::from_corners(min, min + Vec2::new(size.0, size.1))
    }

    #[test]
    fn the_type_path_constant_names_the_component() {
        assert_eq!(PixelCanvas::type_path(), PIXEL_CANVAS_TYPE_PATH);
    }

    #[test]
    fn the_defaults_are_integer_black_and_snapped_with_no_resolution() {
        let canvas = PixelCanvas::default();
        assert_eq!(canvas.scale, CanvasScale::Integer);
        assert_eq!(canvas.letterbox, Color::BLACK);
        assert!(canvas.pixel_snap);
        assert!(!canvas.is_set());
        assert!(PixelCanvas::new(320, 180).is_set());
    }

    #[test]
    fn integer_takes_the_largest_whole_multiple_and_centres_it() {
        let integer = canvas(CanvasScale::Integer);
        assert_eq!(
            integer.display_rect(Vec2::new(1366.0, 768.0)),
            rect((43.0, 24.0), (1280.0, 720.0))
        );
        assert_eq!(
            integer.display_rect(Vec2::new(1920.0, 1080.0)),
            rect((0.0, 0.0), (1920.0, 1080.0))
        );
        assert_eq!(
            integer.display_rect(Vec2::new(1000.0, 1000.0)),
            rect((20.0, 230.0), (960.0, 540.0)),
            "the narrow side picks the multiple"
        );
    }

    #[test]
    fn integer_puts_an_odd_margin_on_a_whole_pixel() {
        let integer = canvas(CanvasScale::Integer);
        assert_eq!(
            integer.display_rect(Vec2::new(1367.0, 769.0)),
            rect((43.0, 24.0), (1280.0, 720.0))
        );
    }

    #[test]
    fn integer_never_scales_below_one_and_overhangs_a_small_window() {
        let integer = canvas(CanvasScale::Integer);
        assert_eq!(
            integer.display_rect(Vec2::new(200.0, 100.0)),
            rect((-60.0, -40.0), (320.0, 180.0))
        );
    }

    #[test]
    fn fit_scales_uniformly_to_the_narrow_side() {
        let fit = canvas(CanvasScale::Fit);
        let shown = fit.display_rect(Vec2::new(1366.0, 768.0));
        assert_eq!(shown.min.y, 0.0);
        assert_eq!(shown.height(), 768.0);
        assert!((shown.width() - 768.0 * 16.0 / 9.0).abs() < 1e-3);
        assert_eq!(shown.min.x, 0.0);

        let tall = fit.display_rect(Vec2::new(200.0, 400.0));
        assert_eq!(tall.width(), 200.0);
        assert!((tall.height() - 112.5).abs() < 1e-4);
        assert_eq!(tall.min, Vec2::new(0.0, 143.0));
    }

    #[test]
    fn stretch_fills_the_target() {
        let stretch = canvas(CanvasScale::Stretch);
        assert_eq!(
            stretch.display_rect(Vec2::new(1366.0, 768.0)),
            rect((0.0, 0.0), (1366.0, 768.0))
        );
    }

    #[test]
    fn an_unset_canvas_covers_the_target() {
        assert_eq!(
            PixelCanvas::default().display_rect(Vec2::new(640.0, 480.0)),
            rect((0.0, 0.0), (640.0, 480.0))
        );
    }

    #[test]
    fn a_target_position_maps_to_canvas_pixels_through_the_letterbox() {
        let integer = canvas(CanvasScale::Integer);
        let target = Vec2::new(1366.0, 768.0);
        assert_eq!(
            integer.canvas_pixel(target, Vec2::new(683.0, 384.0)),
            Some(Vec2::new(160.0, 90.0))
        );
        assert_eq!(
            integer.canvas_pixel(target, Vec2::new(43.0, 24.0)),
            Some(Vec2::ZERO),
            "the top-left corner is canvas pixel zero"
        );
        let last = integer
            .canvas_pixel(target, Vec2::new(1322.9, 743.9))
            .expect("the last window pixel inside is on the canvas");
        assert!(near(last, Vec2::new(319.975, 179.975)), "{last}");
        assert_eq!(integer.canvas_pixel(target, Vec2::new(42.9, 384.0)), None);
        assert_eq!(integer.canvas_pixel(target, Vec2::new(1323.0, 384.0)), None);
        assert_eq!(integer.canvas_pixel(target, Vec2::new(683.0, 10.0)), None);
        assert_eq!(
            integer.canvas_position(target, Vec2::new(39.0, 24.0)),
            Vec2::new(-1.0, 0.0),
            "the unbounded map carries on into the bars"
        );
    }

    #[test]
    fn fit_and_stretch_map_their_own_rects() {
        let target = Vec2::new(1000.0, 1000.0);
        let fit = canvas(CanvasScale::Fit);
        let centre = fit.canvas_pixel(target, Vec2::splat(500.0)).expect("on");
        assert!((centre - Vec2::new(160.0, 90.0)).length() < 0.3, "{centre}");
        assert_eq!(fit.canvas_pixel(target, Vec2::new(500.0, 10.0)), None);

        let stretch = canvas(CanvasScale::Stretch);
        let top = stretch
            .canvas_pixel(target, Vec2::new(500.0, 10.0))
            .expect("stretch covers the whole target");
        assert!(near(top, Vec2::new(160.0, 1.8)), "{top}");
    }

    #[test]
    fn snapping_lands_the_canvas_edge_on_a_whole_pixel() {
        let even = PixelCanvas::new(320, 180);
        assert_eq!(even.snap(Vec2::new(10.4, -3.6), 1.0), Vec2::new(10.0, -4.0));
        assert_eq!(even.snap(Vec2::new(10.6, 0.2), 0.5), Vec2::new(10.5, 0.0));
        assert_eq!(even.snap(Vec2::new(7.2, 7.2), 2.0), Vec2::new(8.0, 8.0));

        let odd = PixelCanvas::new(321, 181);
        assert_eq!(
            odd.snap(Vec2::new(10.2, 0.1), 1.0),
            Vec2::new(10.5, 0.5),
            "an odd canvas centres on a half pixel"
        );
    }

    #[test]
    fn snapping_ignores_a_degenerate_scale() {
        let canvas = PixelCanvas::new(320, 180);
        let at = Vec2::new(1.3, 2.7);
        assert_eq!(canvas.snap(at, 0.0), at);
        assert_eq!(canvas.snap(at, f32::NAN), at);
    }
}
