//! Authored-clip data model. Every type here is a reflected component
//! stored in the scene AST and round-tripped through JSN/BSN.
//!
//! These are the **authoring** representation. [`compile_clips`]
//! converts them into real Bevy `AnimationClip` + `AnimationGraph`
//! assets; from that point Bevy's own `AnimationPlayer` handles
//! playback. Jackdaw never interprets keyframes or samples curves.
//!
//! Authoring data lives under the entity it animates:
//!
//! ```text
//! (Door: Transform + Mesh + Name("Door"))
//!   +-- Clip "Door Open" (duration: 2.0)
//!   |     +-- AnimationTrack (translation, Linear)
//!   |     |     +-- Vec3Keyframe(0.0, [0,0,0])
//!   |     |     +-- Vec3Keyframe(2.0, [2,0,0])
//!   |     +-- AnimationTrack (rotation, Linear)
//!   |           +-- QuatKeyframe(1.0, ...)
//!   +-- Clip "Door Close" (...)
//! ```
//!
//! All mutations go through `SpawnEntity` / `SetBsnField` /
//! `DespawnEntity`. The animation crate exports no custom commands.
//!
//! [`compile_clips`]: crate::compile_clips

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Top-level component on a clip entity.
///
/// `duration` is the authored length in seconds, used for both the
/// timeline visual range and the compiled `AnimationClip` duration.
/// Stored rather than derived from keyframes so the range stays
/// stable during editing. Display name lives on Bevy's `Name`
/// component; tracks are `AnimationTrack` children; keyframes are
/// children of their track.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy)]
#[reflect(Component, Default, Serialize, Deserialize, @jackdaw_scene_types::EditorHidden)]
pub struct Clip {
    pub duration: f32,
    /// What playback does at the end of the clip.
    pub loop_mode: LoopMode,
    /// Multiplier the preview transport plays this clip at.
    pub speed: f32,
}

impl Default for Clip {
    fn default() -> Self {
        Self {
            duration: 2.0,
            loop_mode: LoopMode::Clamp,
            speed: 1.0,
        }
    }
}

/// What playback does once a clip has run out.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[reflect(Default)]
pub enum LoopMode {
    /// Hold the last frame.
    #[default]
    Clamp,
    /// Start again from the top.
    Wrap,
}

impl LoopMode {
    /// The name an operator and the toolbar both call this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clamp => "clamp",
            Self::Wrap => "wrap",
        }
    }

    /// The mode a name asks for, or `None` when it names neither.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "clamp" => Some(Self::Clamp),
            "wrap" => Some(Self::Wrap),
            _ => None,
        }
    }
}

/// Interpolation mode for an [`AnimationTrack`].
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[reflect(Default)]
pub enum Interpolation {
    /// Blend between adjacent keyframes via `Animatable::interpolate`.
    #[default]
    Linear,
    /// Ease between keyframes along a cubic spline whose tangents are read
    /// off the neighbouring keys. Tangents are not authored.
    Cubic,
    /// Hold the previous keyframe's value until the next.
    Step,
}

impl Interpolation {
    /// The name an operator and the track badge both call this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Cubic => "cubic",
            Self::Step => "step",
        }
    }

    /// The short label the track badge shows.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Linear => "LIN",
            Self::Cubic => "CUB",
            Self::Step => "STP",
        }
    }

    /// The mode a name asks for, or `None` when it names none of them.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "linear" => Some(Self::Linear),
            "cubic" => Some(Self::Cubic),
            "step" => Some(Self::Step),
            _ => None,
        }
    }

    /// The next mode a click on the badge moves to.
    pub fn next(self) -> Self {
        match self {
            Self::Linear => Self::Cubic,
            Self::Cubic => Self::Step,
            Self::Step => Self::Linear,
        }
    }
}

/// A single track on a clip. Addresses the animated property via
/// `(component_type_path, field_path)`, the same convention the
/// inspector and `SetBsnField` use. Target entity is implicit: the
/// clip's parent via `ChildOf`.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone)]
#[reflect(Component, Default, Serialize, Deserialize, @jackdaw_scene_types::EditorHidden)]
pub struct AnimationTrack {
    pub component_type_path: String,
    pub field_path: String,
    pub interpolation: Interpolation,
    /// Whether the compile step reads this track. A track switched off keeps
    /// its keys and stops driving the property.
    pub enabled: bool,
}

impl Default for AnimationTrack {
    fn default() -> Self {
        Self {
            component_type_path: String::new(),
            field_path: String::new(),
            interpolation: Interpolation::Linear,
            enabled: true,
        }
    }
}

impl AnimationTrack {
    /// Convenience constructor, defaults to `Linear` interpolation.
    pub fn new(component_type_path: impl Into<String>, field_path: impl Into<String>) -> Self {
        Self {
            component_type_path: component_type_path.into(),
            field_path: field_path.into(),
            ..Self::default()
        }
    }

    /// Path pair used to dispatch in the compile step.
    pub fn property_path(&self) -> (&str, &str) {
        (&self.component_type_path, &self.field_path)
    }
}

// Keyframe components, one per value type. Named after the Bevy type
// they hold, not the field they target. Adding a new value type is a
// new component here plus a dispatch arm in compile.rs.
// `compile.rs`.

/// A keyframe that stores a [`Vec3`] value. Used for translation,
/// scale, and future Vec3-valued animated fields.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_scene_types::EditorHidden)]
pub struct Vec3Keyframe {
    pub time: f32,
    pub value: Vec3,
}

/// A keyframe that stores a [`Quat`] value. Used for rotation.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_scene_types::EditorHidden)]
pub struct QuatKeyframe {
    pub time: f32,
    pub value: Quat,
}

impl Default for QuatKeyframe {
    fn default() -> Self {
        Self {
            time: 0.0,
            value: Quat::IDENTITY,
        }
    }
}

/// A keyframe that stores an [`f32`] value. Used for light intensity,
/// weights, camera FOV, or any scalar animated field.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_scene_types::EditorHidden)]
pub struct F32Keyframe {
    pub time: f32,
    pub value: f32,
}

/// A clip an earlier version of the editor imported from a glTF file into
/// the document.
///
/// glTF clips are now indexed into the editor's animation library instead of
/// spawned as document entities, and a load drops the children that carry
/// this. The type stays registered so a document written before that reads
/// without complaint.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_scene_types::EditorHidden)]
pub struct GltfClipRef {
    pub gltf_path: String,
    pub clip_name: String,
}

/// Which clip the timeline panel is currently editing. `None` shows
/// the create-clip placeholder. Not persisted.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct SelectedClip(pub Option<Entity>);

/// Which keyframes are currently selected in the timeline. Not persisted.
#[derive(Resource, Default, Debug, Clone)]
pub struct SelectedKeyframes {
    pub entities: std::collections::HashSet<Entity>,
}

impl SelectedKeyframes {
    pub fn clear(&mut self) {
        self.entities.clear();
    }
    pub fn is_selected(&self, entity: Entity) -> bool {
        self.entities.contains(&entity)
    }
    pub fn toggle(&mut self, entity: Entity) {
        if !self.entities.insert(entity) {
            self.entities.remove(&entity);
        }
    }
    pub fn select_only(&mut self, entity: Entity) {
        self.entities.clear();
        self.entities.insert(entity);
    }
}

/// Snap behavior for the timeline scrubber. Shift disables snapping
/// temporarily. `threshold_ratio` is a fraction of the visible range.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TimelineSnap {
    pub enabled: bool,
    pub snap_to_ticks: bool,
    pub snap_to_keyframes: bool,
    pub threshold_ratio: f32,
    /// Frames per second the sheet rounds a time to, and the rate its minor
    /// ticks are drawn at.
    pub rate: f32,
}

impl Default for TimelineSnap {
    fn default() -> Self {
        Self {
            enabled: true,
            snap_to_ticks: true,
            snap_to_keyframes: true,
            threshold_ratio: 0.015,
            rate: 30.0,
        }
    }
}

impl TimelineSnap {
    /// `time` rounded to the nearest frame at [`Self::rate`], or unchanged
    /// when snapping is off or the rate is not a rate.
    pub fn round(&self, time: f32) -> f32 {
        if !self.enabled || self.rate <= 0.0 {
            return time;
        }
        (time * self.rate).round() / self.rate
    }

    /// The frame `time` falls on at [`Self::rate`].
    pub fn frame_of(&self, time: f32) -> i32 {
        if self.rate <= 0.0 {
            return 0;
        }
        (time * self.rate).round() as i32
    }
}

/// Whether an inspector edit of a tracked property writes a key at the
/// playhead. Not persisted.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipRecording(pub bool);

/// Which half of the sheet the Timeline tab draws. Not persisted.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineView {
    /// Keys as diamonds, one row per track.
    #[default]
    Dopesheet,
    /// The selected track's value over time, one polyline per component.
    Curves,
}

impl TimelineView {
    /// The name an operator calls this view.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dopesheet => "dopesheet",
            Self::Curves => "curves",
        }
    }

    /// The view a name asks for, or `None` when it names neither.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dopesheet" => Some(Self::Dopesheet),
            "curves" => Some(Self::Curves),
            _ => None,
        }
    }
}

/// How much of the clip the sheet spans, as a multiple of its length. Not
/// persisted.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TimelineZoom(pub f32);

impl Default for TimelineZoom {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Which track the Curves view draws, and whose keys the footer reads. Not
/// persisted.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct SelectedTrack(pub Option<Entity>);

/// Whether the sheet draws the pose either side of the playhead. Reserved:
/// the toggle is in the toolbar and nothing reads it yet.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnionSkin(pub bool);

/// A clip the timeline shows but cannot edit: the glTF clip the library is
/// previewing. Not persisted.
///
/// The editor fills this in, because the file and the player it came from are
/// the editor's business; the widget only needs to know what to draw.
#[derive(Resource, Default, Debug, Clone)]
pub struct ImportedClipView {
    /// The clip being previewed, as `<file>#<clip name>`. `None` when the
    /// timeline has an authored clip up instead.
    pub clip: Option<String>,
    /// What the summary row is called.
    pub name: String,
    /// The clip's length, as its file gave it.
    pub duration: f32,
    /// Bone track names, where the skeleton is bound and they can be read.
    pub bones: Vec<String>,
    /// How many tracks the clip holds, which is what the group heading counts
    /// when the names cannot be read.
    pub curve_count: usize,
}

/// Which keyframe the scrubber is snapped onto during a drag.
/// `None` when not dragging or snapped to a tick. Cleared on drag end.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TimelineSnapHint {
    pub hovered_keyframe: Option<Entity>,
}

/// Typed keyframe value for the copy/paste clipboard.
#[derive(Debug, Clone, Copy)]
pub enum KeyframeValue {
    Vec3(Vec3),
    Quat(Quat),
    F32(f32),
}

/// One entry in the keyframe clipboard. Time is relative to the
/// earliest copied keyframe so paste preserves spacing.
#[derive(Debug, Clone)]
pub struct KeyframeClipboardEntry {
    pub component_type_path: String,
    pub field_path: String,
    pub relative_time: f32,
    pub value: KeyframeValue,
}

/// Keyframes copied with Ctrl+C. Ctrl+V pastes them at the playhead.
#[derive(Resource, Default, Debug, Clone)]
pub struct KeyframeClipboard {
    pub entries: Vec<KeyframeClipboardEntry>,
}
