//! The sky, fog, ambient light and grading a scene's cameras render with.

use bevy::prelude::*;

/// The sky, fog, ambient light and post-processing of a scene; the first one it holds applies.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, @crate::EditorCategory::new("Environment"))]
pub struct Environment {
    pub sky: Sky,
    pub fog: Fog,
    pub ambient: Ambient,
    pub post: PostProcess,
}

/// A gradient sky with a sun disc and an optional cloud layer, drawn behind everything.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct Sky {
    /// Whether the sky is drawn.
    pub enabled: bool,
    /// The colour straight overhead.
    pub zenith: Color,
    /// The colour along the horizon.
    pub horizon: Color,
    /// The colour straight below.
    pub ground: Color,
    /// How far the horizon colour reaches up and down, 0..1.
    pub horizon_softness: f32,
    /// Sky luminance in candela per square metre; 1000 shows the colours as authored.
    pub brightness: f32,
    /// Angular diameter of the disc drawn at the first directional light, in degrees.
    pub sun_size: f32,
    /// Disc brightness as a multiple of that light's illuminance.
    pub sun_intensity: f32,
    /// How much of the sky the cloud covers, 0..1.
    pub cloud_coverage: f32,
    /// Cloud colour; its alpha is how much of the sky it hides.
    pub cloud_color: Color,
    /// How many clouds span the sky.
    pub cloud_scale: f32,
    /// How fast the cloud drifts with the scene's wind.
    pub cloud_speed: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            enabled: false,
            zenith: Color::srgb(0.2, 0.42, 0.78),
            horizon: Color::srgb(0.72, 0.84, 0.95),
            ground: Color::srgb(0.35, 0.36, 0.38),
            horizon_softness: 0.35,
            brightness: 1000.0,
            sun_size: 1.0,
            sun_intensity: 1.0,
            cloud_coverage: 0.0,
            cloud_color: Color::WHITE,
            cloud_scale: 1.0,
            cloud_speed: 0.02,
        }
    }
}

/// How fog thickens with distance from the camera.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum FogMode {
    /// No fog.
    #[default]
    Off,
    /// Rises from [`Fog::start`] to [`Fog::end`].
    Linear,
    /// Thickens by [`Fog::density`] per metre.
    Exponential,
    /// Thickens with the square of [`Fog::density`] times distance.
    ExponentialSquared,
}

/// Distance fog.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct Fog {
    pub mode: FogMode,
    pub color: Color,
    /// Per-metre density of the exponential modes.
    pub density: f32,
    /// Where linear fog begins, in metres.
    pub start: f32,
    /// Where linear fog is complete, in metres.
    pub end: f32,
    /// Fog colour toward the sun; alpha 0 leaves the fog one colour.
    pub sun_color: Color,
    /// How tightly that colour gathers around the sun.
    pub sun_exponent: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            mode: FogMode::Off,
            color: Color::srgb(0.6, 0.7, 0.8),
            density: 0.01,
            start: 50.0,
            end: 500.0,
            sun_color: Color::NONE,
            sun_exponent: 8.0,
        }
    }
}

/// Where a scene's ambient light comes from.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum AmbientMode {
    /// The cameras keep the ambient light they have.
    #[default]
    Off,
    /// One colour from every direction.
    Flat,
    /// Sky colour from above, equator from the sides, ground from below.
    Trilight,
}

/// Light every surface receives from all around, by which way it faces.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct Ambient {
    pub mode: AmbientMode,
    /// The colour from every direction in [`AmbientMode::Flat`].
    pub color: Color,
    /// The colour an upward face receives in [`AmbientMode::Trilight`].
    pub sky: Color,
    /// The colour a sideways face receives in [`AmbientMode::Trilight`].
    pub equator: Color,
    /// The colour a downward face receives in [`AmbientMode::Trilight`].
    pub ground: Color,
    /// Ambient luminance in candela per square metre.
    pub brightness: f32,
}

impl Default for Ambient {
    fn default() -> Self {
        Self {
            mode: AmbientMode::Off,
            color: Color::srgb(0.5, 0.55, 0.6),
            sky: Color::srgb(0.6, 0.65, 0.7),
            equator: Color::srgb(0.3, 0.32, 0.34),
            ground: Color::srgb(0.1, 0.09, 0.08),
            brightness: 500.0,
        }
    }
}

impl Ambient {
    /// Colour reaching a face whose normal's vertical component is `up`, -1..1.
    pub fn color_facing(&self, up: f32) -> LinearRgba {
        let up = up.clamp(-1.0, 1.0);
        match self.mode {
            AmbientMode::Off => LinearRgba::BLACK,
            AmbientMode::Flat => self.color.to_linear(),
            AmbientMode::Trilight => {
                let equator = self.equator.to_linear();
                if up >= 0.0 {
                    equator.mix(&self.sky.to_linear(), up)
                } else {
                    equator.mix(&self.ground.to_linear(), -up)
                }
            }
        }
    }
}

/// The tone curve from rendered light to the screen's range.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum Tonemapper {
    None,
    Reinhard,
    ReinhardLuminance,
    AcesFitted,
    AgX,
    SomewhatBoringDisplayTransform,
    #[default]
    TonyMcMapface,
    BlenderFilmic,
    KhronosPbrNeutral,
}

/// Exposure, tone curve, bloom, grading and vignette.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct PostProcess {
    /// Whether these settings replace the cameras' own.
    pub enabled: bool,
    pub tonemapper: Tonemapper,
    /// The camera exposure, in EV100.
    pub exposure: f32,
    /// Exposure added after lighting, in stops.
    pub post_exposure: f32,
    /// Contrast around the midpoint; 1 leaves it alone.
    pub contrast: f32,
    /// Saturation; 1 leaves it alone and 0 is grey.
    pub saturation: f32,
    /// A turn of every hue, in degrees.
    pub hue_shift: f32,
    /// White balance from cool (negative) to warm (positive).
    pub temperature: f32,
    /// White balance from green (negative) to magenta (positive).
    pub tint: f32,
    /// How strongly bright areas glow. 0 is no bloom.
    pub bloom_intensity: f32,
    /// How bright a pixel has to be before it glows.
    pub bloom_threshold: f32,
    /// How dark the corners fall, 0..1. 0 is no vignette.
    pub vignette_intensity: f32,
    /// How gradually the vignette fades in from the centre.
    pub vignette_smoothness: f32,
}

impl Default for PostProcess {
    fn default() -> Self {
        Self {
            enabled: false,
            tonemapper: Tonemapper::TonyMcMapface,
            exposure: 9.7,
            post_exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            hue_shift: 0.0,
            temperature: 0.0,
            tint: 0.0,
            bloom_intensity: 0.0,
            bloom_threshold: 0.0,
            vignette_intensity: 0.0,
            vignette_smoothness: 5.0,
        }
    }
}

impl Environment {
    /// Whether any group is switched on.
    pub fn dresses_cameras(&self) -> bool {
        self.sky.enabled
            || self.fog.mode != FogMode::Off
            || self.ambient.mode != AmbientMode::Off
            || self.post.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trilight() -> Ambient {
        Ambient {
            mode: AmbientMode::Trilight,
            sky: Color::linear_rgb(0.6, 0.62, 0.64),
            equator: Color::linear_rgb(0.1, 0.12, 0.13),
            ground: Color::linear_rgb(0.05, 0.04, 0.03),
            ..Ambient::default()
        }
    }

    #[test]
    fn an_unedited_environment_dresses_nothing() {
        assert!(!Environment::default().dresses_cameras());
    }

    #[test]
    fn trilight_gives_an_upward_face_the_sky_and_a_downward_face_the_ground() {
        let ambient = trilight();
        assert_eq!(ambient.color_facing(1.0), ambient.sky.to_linear());
        assert_eq!(ambient.color_facing(0.0), ambient.equator.to_linear());
        assert_eq!(ambient.color_facing(-1.0), ambient.ground.to_linear());
    }

    #[test]
    fn trilight_blends_halfway_between_the_horizon_and_the_zenith() {
        let ambient = trilight();
        let half = ambient.color_facing(0.5);
        let expected = ambient
            .equator
            .to_linear()
            .mix(&ambient.sky.to_linear(), 0.5);
        assert!((half.red - expected.red).abs() < 1e-6);
        assert!((half.blue - expected.blue).abs() < 1e-6);
    }

    #[test]
    fn flat_ambient_is_one_colour_all_around() {
        let ambient = Ambient {
            mode: AmbientMode::Flat,
            ..Ambient::default()
        };
        assert_eq!(ambient.color_facing(1.0), ambient.color_facing(-1.0));
    }
}
