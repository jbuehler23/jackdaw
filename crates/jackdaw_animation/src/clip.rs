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
//! All mutations go through `SpawnEntity` / `SetJsnField` /
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
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct Clip {
    pub duration: f32,
}

impl Default for Clip {
    fn default() -> Self {
        Self { duration: 2.0 }
    }
}

/// Interpolation mode for an [`AnimationTrack`].
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Interpolation {
    /// Blend between adjacent keyframes via `Animatable::interpolate`.
    #[default]
    Linear,
    /// Hold the previous keyframe's value until the next. Not yet
    /// implemented in the compile step (warns and skips).
    Step,
}

/// A single track on a clip. Addresses the animated property via
/// `(component_type_path, field_path)`, the same convention the
/// inspector and `SetJsnField` use. Target entity is implicit: the
/// clip's parent via `ChildOf`.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct AnimationTrack {
    pub component_type_path: String,
    pub field_path: String,
    pub interpolation: Interpolation,
}

impl AnimationTrack {
    /// Convenience constructor, defaults to `Linear` interpolation.
    pub fn new(component_type_path: impl Into<String>, field_path: impl Into<String>) -> Self {
        Self {
            component_type_path: component_type_path.into(),
            field_path: field_path.into(),
            interpolation: Interpolation::Linear,
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
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct Vec3Keyframe {
    pub time: f32,
    pub value: Vec3,
}

/// A keyframe that stores a [`Quat`] value. Used for rotation.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
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
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct F32Keyframe {
    pub time: f32,
    pub value: f32,
}

/// Marker on a [`Clip`] whose source is a glTF-imported animation.
/// The compile step loads `Gltf::named_animations[clip_name]` directly
/// instead of building from keyframe children. Read-only in the
/// timeline; persisted as two strings and re-resolved on reload.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
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
}

impl Default for TimelineSnap {
    fn default() -> Self {
        Self {
            enabled: true,
            snap_to_ticks: true,
            snap_to_keyframes: true,
            threshold_ratio: 0.015,
        }
    }
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
