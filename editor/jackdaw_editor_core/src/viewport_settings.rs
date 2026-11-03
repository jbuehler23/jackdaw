//! How the editor's 3D viewports draw: quality, show flags, redraw and stats.
//!
//! Kept per user in `viewport.json` in the config directory, never in a
//! scene: these trade the editor's picture for speed without touching what
//! the scene holds, so Play and the saved file are unaffected. Kept out of
//! the undo snapshot, and set through operators that make no history entry.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use serde::{Deserialize, Serialize};

use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn plugin(app: &mut App) {
    let path = jackdaw_env::paths::viewport_settings_path();
    let settings = path.as_deref().map(load).unwrap_or_default();
    app.insert_resource(ViewportSettingsFile {
        path,
        saved: settings.clone(),
    })
    .insert_resource(settings)
    .add_systems(
        Update,
        store_viewport_settings.run_if(resource_changed::<ViewportSettings>),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportQualityPresetOp>()
        .register_operator::<ViewportQualitySetOp>()
        .register_operator::<ViewportShowToggleOp>()
        .register_operator::<ViewportRealtimeToggleOp>()
        .register_operator::<ViewportStatsToggleOp>()
        .register_operator::<ViewportFrameGraphToggleOp>();
    ctx.bind_operator::<CoreExtensionInputContext, ViewportStatsToggleOp>([PresetInput::key("F3")]);
}

/// A named set of quality choices, from fastest to best looking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    Low,
    Medium,
    High,
    Full,
}

impl QualityPreset {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Full];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Full => "Full",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.label().eq_ignore_ascii_case(name))
    }

    /// The quality choices this preset makes.
    pub fn quality(self) -> ViewportQuality {
        let (render_scale, anti_aliasing, shadow_quality, detail_distance) = match self {
            Self::Low => (
                50,
                AntiAliasing::Off,
                ShadowQuality::Low,
                DetailDistance::Near,
            ),
            Self::Medium => (
                75,
                AntiAliasing::Fxaa,
                ShadowQuality::Medium,
                DetailDistance::Medium,
            ),
            Self::High => (
                100,
                AntiAliasing::Fxaa,
                ShadowQuality::High,
                DetailDistance::Far,
            ),
            Self::Full => (
                100,
                AntiAliasing::Taa,
                ShadowQuality::High,
                DetailDistance::Far,
            ),
        };
        let at_least = |preset: Self| self as u8 >= preset as u8;
        ViewportQuality {
            render_scale,
            anti_aliasing,
            shadow_quality,
            detail_distance,
            sun_shadows: true,
            point_shadows: at_least(Self::High),
            fog: at_least(Self::Medium),
            post_processing: at_least(Self::Medium),
            bloom: at_least(Self::High),
            ambient_occlusion: at_least(Self::Full),
        }
    }
}

/// Edge smoothing the viewport draws with, whatever the scene asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AntiAliasing {
    Off,
    #[default]
    Fxaa,
    Taa,
}

impl AntiAliasing {
    pub const ALL: [Self; 3] = [Self::Off, Self::Fxaa, Self::Taa];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Fxaa => "FXAA",
            Self::Taa => "TAA",
        }
    }
}

/// How sharp and how far the sun's shadows reach in the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowQuality {
    Low,
    Medium,
    #[default]
    High,
}

impl ShadowQuality {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

/// How far terrain detail and finer levels of detail reach from the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailDistance {
    Near,
    Medium,
    #[default]
    Far,
}

impl DetailDistance {
    pub const ALL: [Self; 3] = [Self::Near, Self::Medium, Self::Far];

    pub fn label(self) -> &'static str {
        match self {
            Self::Near => "Near",
            Self::Medium => "Medium",
            Self::Far => "Far",
        }
    }
}

/// The choices a [`QualityPreset`] sets together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewportQuality {
    /// Percent of the viewport's size the scene is drawn at before scaling up.
    pub render_scale: u32,
    pub anti_aliasing: AntiAliasing,
    pub shadow_quality: ShadowQuality,
    pub detail_distance: DetailDistance,
    pub sun_shadows: bool,
    /// Shadows cast by point and spot lights.
    pub point_shadows: bool,
    pub fog: bool,
    /// The scene's tonemapping, colour grading, vignette and exposure.
    pub post_processing: bool,
    pub bloom: bool,
    pub ambient_occlusion: bool,
}

impl Default for ViewportQuality {
    fn default() -> Self {
        QualityPreset::High.quality()
    }
}

/// The smallest render scale, in percent, the viewport accepts.
pub const MIN_RENDER_SCALE: u32 = 25;

/// Everything the editor's 3D viewports draw with that is not in the scene.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewportSettings {
    pub quality: ViewportQuality,
    /// Ambient light and reflections from the scene's sky or environment map.
    pub sky_reflection: bool,
    /// Grass and other terrain detail layers.
    pub terrain_detail: bool,
    /// Redraw every frame, rather than only when something changes.
    pub realtime: bool,
    pub stats: bool,
    /// The frame time graph under the stats readout.
    pub frame_graph: bool,
}

impl Default for ViewportSettings {
    fn default() -> Self {
        Self {
            quality: ViewportQuality::default(),
            sky_reflection: true,
            terrain_detail: true,
            realtime: true,
            stats: false,
            frame_graph: false,
        }
    }
}

impl ViewportSettings {
    /// The preset the quality choices match, or `None` once any differs.
    pub fn preset(&self) -> Option<QualityPreset> {
        QualityPreset::ALL
            .into_iter()
            .find(|preset| preset.quality() == self.quality)
    }

    /// The flag a [`ViewportShowToggleOp`] names, if it names one.
    fn show_flag(&mut self, name: &str) -> Option<&mut bool> {
        let quality = &mut self.quality;
        Some(match name {
            "sun_shadows" => &mut quality.sun_shadows,
            "point_shadows" => &mut quality.point_shadows,
            "fog" => &mut quality.fog,
            "post_processing" => &mut quality.post_processing,
            "bloom" => &mut quality.bloom,
            "ambient_occlusion" => &mut quality.ambient_occlusion,
            "sky_reflection" => &mut self.sky_reflection,
            "terrain_detail" => &mut self.terrain_detail,
            _ => return None,
        })
    }
}

/// Where the viewport settings are kept, and what that file last held.
#[derive(Resource, Debug)]
pub struct ViewportSettingsFile {
    /// `None` keeps the settings in memory only, for this run.
    pub path: Option<PathBuf>,
    saved: ViewportSettings,
}

/// Read the settings file; a missing or unreadable one gives the defaults.
pub fn load(path: &Path) -> ViewportSettings {
    let Ok(bytes) = std::fs::read(path) else {
        return ViewportSettings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        warn!(
            "could not read the viewport settings at {}: {error}",
            path.display()
        );
        ViewportSettings::default()
    })
}

/// Write the settings file, creating its directory.
pub fn store(path: &Path, settings: &ViewportSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}

fn store_viewport_settings(
    settings: Res<ViewportSettings>,
    mut file: ResMut<ViewportSettingsFile>,
) {
    if file.saved == *settings {
        return;
    }
    file.saved = settings.clone();
    let Some(path) = &file.path else {
        return;
    };
    if let Err(error) = store(path, &settings) {
        warn!(
            "could not keep the viewport settings at {}: {error}",
            path.display()
        );
    }
}

/// Make every quality choice a preset makes.
#[operator(
    id = "viewport.quality.preset",
    label = "Viewport Quality Preset",
    description = "Set the viewport's render scale, anti-aliasing, shadows, effects and detail \
                   distance from a preset. Editor only: the scene is unchanged.",
    allows_undo = false,
    params(preset(String, doc = "`low`, `medium`, `high` or `full`."))
)]
pub(crate) fn viewport_quality_preset(
    params: In<OperatorParameters>,
    mut settings: ResMut<ViewportSettings>,
) -> OperatorResult {
    let Some(preset) = params.as_str("preset").and_then(QualityPreset::parse) else {
        warn!("viewport.quality.preset: 'preset' must be low, medium, high or full");
        return OperatorResult::Cancelled;
    };
    let quality = preset.quality();
    if settings.quality != quality {
        settings.quality = quality;
    }
    OperatorResult::Finished
}

/// Change single quality choices. Every field is optional and the ones left
/// out keep what they were.
#[operator(
    id = "viewport.quality.set",
    label = "Set Viewport Quality",
    description = "Change the viewport's render scale, anti-aliasing, shadow quality or detail \
                   distance. Editor only: the scene is unchanged.",
    allows_undo = false,
    params(
        render_scale(i64, doc = "Percent of the viewport's size to draw at, 25 to 100."),
        anti_aliasing(String, doc = "`off`, `fxaa` or `taa`."),
        shadow_quality(String, doc = "`low`, `medium` or `high`."),
        detail_distance(String, doc = "`near`, `medium` or `far`."),
    )
)]
pub(crate) fn viewport_quality_set(
    params: In<OperatorParameters>,
    mut settings: ResMut<ViewportSettings>,
) -> OperatorResult {
    let id = "viewport.quality.set";
    let mut quality = settings.quality;
    if let Some(percent) = params.as_int("render_scale") {
        let Some(percent) = u32::try_from(percent)
            .ok()
            .filter(|percent| (MIN_RENDER_SCALE..=100).contains(percent))
        else {
            warn!("{id}: 'render_scale' must be a percent from {MIN_RENDER_SCALE} to 100");
            return OperatorResult::Cancelled;
        };
        quality.render_scale = percent;
    }
    if let Some(name) = params.as_str("anti_aliasing") {
        let Some(choice) = find_by_label(AntiAliasing::ALL, AntiAliasing::label, name) else {
            warn!("{id}: 'anti_aliasing' must be off, fxaa or taa");
            return OperatorResult::Cancelled;
        };
        quality.anti_aliasing = choice;
    }
    if let Some(name) = params.as_str("shadow_quality") {
        let Some(choice) = find_by_label(ShadowQuality::ALL, ShadowQuality::label, name) else {
            warn!("{id}: 'shadow_quality' must be low, medium or high");
            return OperatorResult::Cancelled;
        };
        quality.shadow_quality = choice;
    }
    if let Some(name) = params.as_str("detail_distance") {
        let Some(choice) = find_by_label(DetailDistance::ALL, DetailDistance::label, name) else {
            warn!("{id}: 'detail_distance' must be near, medium or far");
            return OperatorResult::Cancelled;
        };
        quality.detail_distance = choice;
    }
    if settings.quality != quality {
        settings.quality = quality;
    }
    OperatorResult::Finished
}

fn find_by_label<T: Copy>(
    all: impl IntoIterator<Item = T>,
    label: fn(T) -> &'static str,
    name: &str,
) -> Option<T> {
    all.into_iter()
        .find(|choice| label(*choice).eq_ignore_ascii_case(name))
}

/// Show or hide one kind of thing the viewport draws.
#[operator(
    id = "viewport.show.toggle",
    label = "Show in Viewport",
    description = "Show or hide one kind of thing the viewport draws. Editor only: the scene is \
                   unchanged.",
    allows_undo = false,
    params(
        flag(
            String,
            doc = "`sun_shadows`, `point_shadows`, `sky_reflection`, `fog`, `post_processing`, \
                   `bloom`, `ambient_occlusion` or `terrain_detail`."
        ),
        on(bool, doc = "On or off. Omit to flip whichever way it currently is."),
    )
)]
pub(crate) fn viewport_show_toggle(
    params: In<OperatorParameters>,
    mut settings: ResMut<ViewportSettings>,
) -> OperatorResult {
    let Some(name) = params.as_str("flag") else {
        warn!("viewport.show.toggle: name a 'flag'");
        return OperatorResult::Cancelled;
    };
    let current = settings.bypass_change_detection().show_flag(name).copied();
    let Some(current) = current else {
        warn!("viewport.show.toggle: '{name}' is not something the viewport shows");
        return OperatorResult::Cancelled;
    };
    let on = params.as_bool("on").unwrap_or(!current);
    if on != current
        && let Some(flag) = settings.show_flag(name)
    {
        *flag = on;
    }
    OperatorResult::Finished
}

/// Redraw the viewport every frame, or only when something changes.
#[operator(
    id = "viewport.realtime.toggle",
    label = "Realtime",
    description = "Redraw the viewport every frame, or only when something changes.",
    allows_undo = false,
    params(on(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn viewport_realtime_toggle(
    params: In<OperatorParameters>,
    mut settings: ResMut<ViewportSettings>,
) -> OperatorResult {
    let on = params.as_bool("on").unwrap_or(!settings.realtime);
    if settings.realtime != on {
        settings.realtime = on;
    }
    OperatorResult::Finished
}

/// Show or hide the viewport's frame rate and frame time readout.
#[operator(
    id = "viewport.stats.toggle",
    label = "Stats",
    description = "Show or hide the frame rate and frame time readout.",
    allows_undo = false,
    params(on(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn viewport_stats_toggle(
    params: In<OperatorParameters>,
    mut settings: ResMut<ViewportSettings>,
) -> OperatorResult {
    let on = params.as_bool("on").unwrap_or(!settings.stats);
    if settings.stats != on {
        settings.stats = on;
    }
    OperatorResult::Finished
}

/// Show or hide the frame time graph under the stats readout.
#[operator(
    id = "viewport.stats.graph.toggle",
    label = "Frame Time Graph",
    description = "Show or hide the frame time graph under the stats readout.",
    allows_undo = false,
    params(on(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn viewport_frame_graph_toggle(
    params: In<OperatorParameters>,
    mut settings: ResMut<ViewportSettings>,
) -> OperatorResult {
    let on = params.as_bool("on").unwrap_or(!settings.frame_graph);
    if settings.frame_graph != on {
        settings.frame_graph = on;
    }
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_preset_is_recognised_from_its_own_choices() {
        for preset in QualityPreset::ALL {
            let settings = ViewportSettings {
                quality: preset.quality(),
                ..default()
            };
            assert_eq!(settings.preset(), Some(preset));
        }
    }

    #[test]
    fn the_defaults_are_the_high_preset_drawn_every_frame() {
        let settings = ViewportSettings::default();
        assert_eq!(settings.preset(), Some(QualityPreset::High));
        assert!(settings.realtime && !settings.stats);
    }

    #[test]
    fn changing_one_choice_leaves_every_preset() {
        let mut settings = ViewportSettings::default();
        settings.quality.render_scale = 60;
        assert_eq!(settings.preset(), None);
    }

    #[test]
    fn stored_settings_load_back_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("viewport.json");
        let mut settings = ViewportSettings {
            quality: QualityPreset::Low.quality(),
            terrain_detail: false,
            realtime: false,
            ..default()
        };
        settings.quality.point_shadows = true;
        store(&path, &settings).unwrap();
        assert_eq!(load(&path), settings);
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("viewport.json")),
            ViewportSettings::default()
        );
    }

    #[test]
    fn fields_a_file_leaves_out_take_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("viewport.json");
        std::fs::write(&path, r#"{"stats": true, "quality": {"bloom": false}}"#).unwrap();
        let loaded = load(&path);
        assert!(loaded.stats && !loaded.quality.bloom);
        assert_eq!(
            loaded.quality.render_scale,
            ViewportQuality::default().render_scale
        );
        assert!(loaded.realtime);
    }

    #[test]
    fn an_unreadable_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("viewport.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), ViewportSettings::default());
    }
}
