//! Data-driven keymap presets. A preset is a serializable document of
//! operator-id bindings; applying one replaces the BEI binding entities
//! of every operator action it names. Extensions record their defaults
//! through `ExtensionContext::bind_operator`, and the generated
//! "classic" preset reproduces those defaults exactly.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{
    Binding, BindingOf, InputModKeys, ModKeys, Press, Release, Tap,
};
use serde::{Deserialize, Serialize};

use crate::keymap_conditions::{DoubleClick, ScrollTick};

use crate::lifecycle::OperatorAction;

/// One input trigger: a keyboard key chord, a mouse button chord, or a
/// scroll tick. Key code names follow the Bevy `KeyCode` variant spelling,
/// e.g. `"Digit1"`, `"KeyK"`, `"Escape"`, `"F9"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PresetInput {
    Key {
        /// Bevy `KeyCode` name, e.g. `"Digit1"`, `"KeyK"`, `"Escape"`.
        /// Stored as a string so preset files stay readable and stable
        /// across enum reordering.
        key: String,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        alt: bool,
    },
    /// Mouse button, optionally combined with modifier keys.
    /// `button` is one of `"Left"`, `"Right"`, `"Middle"`, `"Back"`, `"Forward"`.
    MouseButton {
        button: String,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        alt: bool,
    },
    /// One wheel tick; `up: false` is a downward tick.
    Scroll {
        up: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        alt: bool,
    },
}

impl PresetInput {
    pub fn key(name: &str) -> Self {
        Self::Key {
            key: name.to_string(),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// Construct a mouse-button input. `button` must be one of
    /// `"Left"`, `"Right"`, `"Middle"`, `"Back"`, `"Forward"`.
    pub fn mouse(button: &str) -> Self {
        Self::MouseButton {
            button: button.to_string(),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// Construct a scroll-wheel input.
    pub fn scroll(up: bool) -> Self {
        Self::Scroll {
            up,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// Set the Ctrl modifier.
    pub fn ctrl(mut self) -> Self {
        match &mut self {
            Self::Key { ctrl, .. } | Self::MouseButton { ctrl, .. } | Self::Scroll { ctrl, .. } => {
                *ctrl = true;
            }
        }
        self
    }

    /// Set the Shift modifier.
    pub fn shift(mut self) -> Self {
        match &mut self {
            Self::Key { shift, .. }
            | Self::MouseButton { shift, .. }
            | Self::Scroll { shift, .. } => *shift = true,
        }
        self
    }

    /// Set the Alt modifier.
    pub fn alt(mut self) -> Self {
        match &mut self {
            Self::Key { alt, .. } | Self::MouseButton { alt, .. } | Self::Scroll { alt, .. } => {
                *alt = true;
            }
        }
        self
    }
}

/// Parse a mouse button from the preset name string. Returns `None` for
/// `"Other"` or any unrecognised name.
pub fn mouse_button_from_name(name: &str) -> Option<MouseButton> {
    match name {
        "Left" => Some(MouseButton::Left),
        "Right" => Some(MouseButton::Right),
        "Middle" => Some(MouseButton::Middle),
        "Back" => Some(MouseButton::Back),
        "Forward" => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Display-stable name for a `MouseButton`. Returns `None` for
/// `MouseButton::Other(_)`, which the preset format does not support.
pub fn mouse_button_name(button: MouseButton) -> Option<String> {
    match button {
        MouseButton::Left => Some("Left".to_string()),
        MouseButton::Right => Some("Right".to_string()),
        MouseButton::Middle => Some("Middle".to_string()),
        MouseButton::Back => Some("Back".to_string()),
        MouseButton::Forward => Some("Forward".to_string()),
        MouseButton::Other(_) => None,
    }
}

/// When the binding fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetPhase {
    #[default]
    Press,
    Release,
    DoubleClick,
    Tap,
}

impl PresetPhase {
    /// Used by serde to omit the default phase from generated files.
    pub fn is_press(&self) -> bool {
        matches!(self, Self::Press)
    }
}

/// Which action set an entry binds into. `Operators` resolves through
/// the `OperatorAction` id tag; `Modal` and `Navigation` resolve through
/// the `BuiltinActions` registry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetContext {
    #[default]
    Operators,
    Modal,
    Navigation,
}

impl PresetContext {
    /// Used by serde to omit the default context from generated files.
    pub fn is_operators(&self) -> bool {
        matches!(self, Self::Operators)
    }
}

/// One preset entry: an input chord bound to an operator id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetBinding {
    pub operator: String,
    pub input: PresetInput,
    #[serde(default, skip_serializing_if = "PresetPhase::is_press")]
    pub phase: PresetPhase,
    #[serde(default, skip_serializing_if = "PresetContext::is_operators")]
    pub context: PresetContext,
}

/// A complete keymap preset document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapPreset {
    pub name: String,
    pub bindings: Vec<PresetBinding>,
}

/// Defaults recorded by `ExtensionContext::bind_operator` during
/// extension registration. The classic preset is generated from this,
/// so it can never drift from what extensions declare.
#[derive(Resource, Default)]
pub struct DefaultKeymap {
    pub bindings: Vec<PresetBinding>,
}

/// Registry mapping builtin action names (e.g. `"modal.confirm"`) to the
/// action entities that the keymap applier binds into.
///
/// Populated by `input_contexts::spawn_contexts` at startup, before
/// `apply_active_keymap` runs. The Modal and Navigation arms in
/// `apply_keymap_preset` resolve entries here just like Operators entries
/// resolve through `OperatorAction`. Unknown names land in
/// `skipped_unknown_operator` (same slot; the semantics are identical: a
/// preset entry naming something that does not exist).
#[derive(Resource, Default)]
pub struct BuiltinActions {
    map: std::collections::HashMap<String, Vec<Entity>>,
}

impl BuiltinActions {
    /// Register `name` as owning `entity`. May be called multiple times with
    /// the same name to accumulate multiple entities (analogous to multiple
    /// action entities per operator).
    pub fn register(&mut self, name: impl Into<String>, entity: Entity) {
        self.map.entry(name.into()).or_default().push(entity);
    }

    /// Look up the entities registered under `name`.
    pub fn get(&self, name: &str) -> Option<&[Entity]> {
        self.map.get(name).map(Vec::as_slice)
    }
}

impl DefaultKeymap {
    /// Snapshot the recorded defaults as the "classic" preset.
    pub fn to_classic_preset(&self) -> KeymapPreset {
        KeymapPreset {
            name: "classic".into(),
            bindings: self.bindings.clone(),
        }
    }
}

/// Which preset is active. Persisted as plain JSON in the user config
/// directory next to the keybinds file. Only "classic" exists today;
/// the file format is the contract future presets and the settings UI
/// build on.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveKeymapPreset {
    pub name: String,
}

impl Default for ActiveKeymapPreset {
    fn default() -> Self {
        Self {
            name: "classic".into(),
        }
    }
}

fn keymap_preset_path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("keymap_preset.json"))
}

/// Load the active keymap preset from disk. Returns the default ("classic")
/// silently if the file is absent, or with a `warn!` if the file is present
/// but cannot be parsed.
pub fn load_active_keymap_preset() -> ActiveKeymapPreset {
    let Some(path) = keymap_preset_path() else {
        return ActiveKeymapPreset::default();
    };
    if !path.is_file() {
        return ActiveKeymapPreset::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read keymap preset file {}: {e}", path.display());
            return ActiveKeymapPreset::default();
        }
    };
    match serde_json::from_str::<ActiveKeymapPreset>(&data) {
        Ok(preset) => preset,
        Err(e) => {
            warn!(
                "Corrupt keymap preset file {}; falling back to default: {e}",
                path.display()
            );
            ActiveKeymapPreset::default()
        }
    }
}

/// Persist the active keymap preset to disk.
pub fn save_active_keymap_preset(preset: &ActiveKeymapPreset) {
    let Some(path) = keymap_preset_path() else {
        warn!("Could not determine config directory for keymap preset");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(preset) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                warn!("Failed to write keymap preset file: {e}");
            }
        }
        Err(e) => {
            warn!("Failed to serialize keymap preset: {e}");
        }
    }
}

/// Outcome of one preset application, for conformance checks and logs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeymapApplyReport {
    /// Entries that bound to at least one action entity.
    pub applied_entries: usize,
    /// Binding entities spawned (>= `applied_entries` when an operator
    /// has multiple action entities).
    pub spawned_bindings: usize,
    pub skipped_unknown_operator: Vec<String>,
    pub skipped_unparseable_key: Vec<String>,
    /// Entries skipped because their input type, phase, or context is not
    /// yet handled by the applier. Each entry is `"operator-or-name: reason"`.
    /// Currently always empty; kept for forward compatibility when new input
    /// types or phases are added.
    pub skipped_unsupported: Vec<String>,
}

/// Marker on binding entities spawned by [`apply_keymap_preset`].
///
/// Re-application first despawns every entity carrying this marker,
/// so it replaces exactly its own previous spawns and never touches
/// bindings that were attached by other means (raw spawn sites, tests).
#[derive(Component)]
pub struct PresetSpawnedBinding;

/// Fully resolved binding, ready to spawn with the correct condition component.
enum ResolvedBinding {
    KeyPress {
        binding: Binding,
        phase: PresetPhase,
    },
    MousePress {
        binding: Binding,
        phase: PresetPhase,
    },
    Scroll {
        binding: Binding,
        positive: bool,
    },
}

/// Replace the preset-managed bindings of every operator action with
/// the entries in `preset`.
///
/// Entries that name an unknown operator id or carry an unparseable key
/// name are collected into the returned [`KeymapApplyReport`] rather than
/// warned inline. After the loop, one aggregated `warn!` is emitted per
/// non-empty skip vec.
///
/// Re-application is idempotent: all entities carrying [`PresetSpawnedBinding`]
/// are despawned before any new spawning begins.
pub fn apply_keymap_preset(world: &mut World, preset: &KeymapPreset) -> KeymapApplyReport {
    // Despawn every binding entity previously owned by this applier.
    let stale: Vec<Entity> = world
        .query_filtered::<Entity, With<PresetSpawnedBinding>>()
        .iter(world)
        .collect();
    for entity in stale {
        world.entity_mut(entity).despawn();
    }

    // Build a map from operator id -> list of action entity ids.
    let mut by_operator: HashMap<&'static str, Vec<Entity>> = HashMap::new();
    let mut action_query = world.query::<(Entity, &OperatorAction)>();
    for (entity, tag) in action_query.iter(world) {
        by_operator.entry(tag.0).or_default().push(entity);
    }

    // Snapshot the builtin actions map so we can look up Modal/Navigation
    // entries without holding a reference to the world.
    let builtin_snapshot: std::collections::HashMap<String, Vec<Entity>> = world
        .get_resource::<BuiltinActions>()
        .map(|b| b.map.clone())
        .unwrap_or_default();

    let mut report = KeymapApplyReport::default();
    for entry in &preset.bindings {
        // Resolve the input to a binding + phase shape. Scroll entries always
        // use ScrollTick regardless of the phase field; the phase is ignored.
        let resolved = match &entry.input {
            PresetInput::Key {
                key,
                ctrl,
                shift,
                alt,
            } => {
                let Some(key_code) = key_code_from_name(key) else {
                    report.skipped_unparseable_key.push(key.clone());
                    continue;
                };
                let mod_keys = mod_keys_from_bools(*ctrl, *shift, *alt);
                ResolvedBinding::KeyPress {
                    binding: key_code.with_mod_keys(mod_keys),
                    phase: entry.phase,
                }
            }
            PresetInput::MouseButton {
                button,
                ctrl,
                shift,
                alt,
            } => {
                let Some(mb) = mouse_button_from_name(button) else {
                    report
                        .skipped_unparseable_key
                        .push(format!("{}: {button}", entry.operator));
                    continue;
                };
                let mod_keys = mod_keys_from_bools(*ctrl, *shift, *alt);
                ResolvedBinding::MousePress {
                    binding: mb.with_mod_keys(mod_keys),
                    phase: entry.phase,
                }
            }
            PresetInput::Scroll {
                up,
                ctrl,
                shift,
                alt,
            } => {
                let mod_keys = mod_keys_from_bools(*ctrl, *shift, *alt);
                ResolvedBinding::Scroll {
                    // Scroll phase is irrelevant; ScrollTick replaces it.
                    binding: Binding::MouseWheel { mod_keys },
                    positive: *up,
                }
            }
        };

        // Resolve operator id / builtin name to its action entities.
        // Operators context: look up via OperatorAction tag.
        // Modal / Navigation contexts: look up via BuiltinActions registry.
        let action_entities: Vec<Entity> = match entry.context {
            PresetContext::Operators => match by_operator.get(entry.operator.as_str()) {
                Some(v) => v.clone(),
                None => {
                    report.skipped_unknown_operator.push(entry.operator.clone());
                    continue;
                }
            },
            PresetContext::Modal | PresetContext::Navigation => {
                match builtin_snapshot.get(entry.operator.as_str()) {
                    Some(v) => v.clone(),
                    None => {
                        report.skipped_unknown_operator.push(entry.operator.clone());
                        continue;
                    }
                }
            }
        };

        // Spawn one binding entity per action entity, with the correct condition.
        let spawned = spawn_resolved(world, &resolved, &action_entities);
        report.spawned_bindings += spawned;
        report.applied_entries += 1;
    }

    // Emit one aggregated warning per non-empty skip vec.
    if !report.skipped_unknown_operator.is_empty() {
        warn!(
            "preset '{}' skipped {} unknown operators: {:?}",
            preset.name,
            report.skipped_unknown_operator.len(),
            report.skipped_unknown_operator,
        );
    }
    if !report.skipped_unparseable_key.is_empty() {
        warn!(
            "preset '{}' skipped {} unparseable keys: {:?}",
            preset.name,
            report.skipped_unparseable_key.len(),
            report.skipped_unparseable_key,
        );
    }
    if !report.skipped_unsupported.is_empty() {
        warn!(
            "preset '{}' skipped {} unsupported bindings: {:?}",
            preset.name,
            report.skipped_unsupported.len(),
            report.skipped_unsupported,
        );
    }

    report
}

/// Build a `ModKeys` bitmask from the three preset boolean fields.
fn mod_keys_from_bools(ctrl: bool, shift: bool, alt: bool) -> ModKeys {
    let mut mk = ModKeys::empty();
    if ctrl {
        mk |= ModKeys::CONTROL;
    }
    if shift {
        mk |= ModKeys::SHIFT;
    }
    if alt {
        mk |= ModKeys::ALT;
    }
    mk
}

/// Spawn one binding entity per action entity and return the spawn count.
fn spawn_resolved(
    world: &mut World,
    resolved: &ResolvedBinding,
    action_entities: &[Entity],
) -> usize {
    let mut count = 0;
    for &action_entity in action_entities {
        match resolved {
            ResolvedBinding::KeyPress { binding, phase }
            | ResolvedBinding::MousePress { binding, phase } => match phase {
                PresetPhase::Press => {
                    world.spawn((
                        *binding,
                        Press::default(),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
                PresetPhase::Release => {
                    world.spawn((
                        *binding,
                        Release::default(),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
                PresetPhase::DoubleClick => {
                    world.spawn((
                        *binding,
                        DoubleClick::default(),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
                PresetPhase::Tap => {
                    world.spawn((
                        *binding,
                        Tap::new(0.2),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
            },
            ResolvedBinding::Scroll { binding, positive } => {
                world.spawn((
                    *binding,
                    ScrollTick::new(*positive),
                    BindingOf(action_entity),
                    PresetSpawnedBinding,
                    ChildOf(action_entity),
                ));
            }
        }
        count += 1;
    }
    count
}

/// Parse the `KeyCode` named by a preset entry. Returns `None` for
/// unknown names so a typo in a preset file degrades to an unbound
/// operator plus a warning instead of a panic.
pub fn key_code_from_name(name: &str) -> Option<KeyCode> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

/// Display-stable name for a `KeyCode`. Inverse of `key_code_from_name`
/// for all named keys; the `Unidentified` platform variant falls back to
/// its debug form, which does not parse back and degrades to warn-and-skip.
pub fn key_code_name(key: KeyCode) -> String {
    match serde_json::to_value(key) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{key:?}"),
    }
}

#[cfg(test)]
mod tests {
    use bevy_enhanced_input::prelude::{Binding, TriggerState};

    use super::*;
    use crate::lifecycle::OperatorAction;

    #[test]
    fn preset_round_trips_through_json() {
        let preset = KeymapPreset {
            name: "classic".into(),
            bindings: vec![
                PresetBinding {
                    operator: "edit_mode.vertex".into(),
                    input: PresetInput::key("Digit1"),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "history.undo".into(),
                    input: PresetInput::key("KeyZ").ctrl(),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "view.orbit".into(),
                    input: PresetInput::mouse("Middle"),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "view.zoom".into(),
                    input: PresetInput::scroll(true).ctrl(),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "select.deselect".into(),
                    input: PresetInput::key("Escape"),
                    phase: PresetPhase::Release,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "modal.confirm".into(),
                    input: PresetInput::key("Enter"),
                    phase: PresetPhase::Press,
                    context: PresetContext::Modal,
                },
            ],
        };
        let json = serde_json::to_string_pretty(&preset).expect("serialize");
        let back: KeymapPreset = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(preset, back);
    }

    #[test]
    fn key_code_names_round_trip() {
        for key in [KeyCode::Digit1, KeyCode::KeyK, KeyCode::Escape, KeyCode::F9] {
            let name = key_code_name(key);
            assert_eq!(
                key_code_from_name(&name),
                Some(key),
                "round trip failed for {name}"
            );
        }
    }

    #[test]
    fn unknown_key_name_is_none_not_panic() {
        assert_eq!(key_code_from_name("NotAKey"), None);
    }

    #[test]
    fn default_keymap_snapshot_is_classic() {
        let mut defaults = DefaultKeymap::default();
        defaults.bindings.push(PresetBinding {
            operator: "tool.select".into(),
            input: PresetInput::key("KeyQ"),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        });
        let preset = defaults.to_classic_preset();
        assert_eq!(preset.name, "classic");
        assert_eq!(preset.bindings, defaults.bindings);
    }

    #[test]
    fn serialized_shape_is_the_documented_contract() {
        let binding = PresetBinding {
            operator: "history.undo".into(),
            input: PresetInput::key("KeyZ").ctrl(),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        };
        let json = serde_json::to_string(&binding).expect("serialize");
        assert_eq!(
            json,
            r#"{"operator":"history.undo","input":{"type":"Key","key":"KeyZ","ctrl":true}}"#
        );
    }

    #[test]
    fn mouse_button_golden_shape() {
        let binding = PresetBinding {
            operator: "x".into(),
            input: PresetInput::mouse("Right"),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        };
        let json = serde_json::to_string(&binding).expect("serialize");
        assert_eq!(
            json,
            r#"{"operator":"x","input":{"type":"MouseButton","button":"Right"}}"#
        );
    }

    #[test]
    fn minimal_handwritten_json_parses_with_defaults() {
        let json = r#"{"operator":"tool.select","input":{"type":"Key","key":"KeyQ"}}"#;
        let binding: PresetBinding = serde_json::from_str(json).expect("minimal JSON must parse");
        assert_eq!(binding.phase, PresetPhase::Press);
        assert_eq!(binding.context, PresetContext::Operators);
        assert_eq!(
            binding.input,
            PresetInput::key("KeyQ"),
            "omitted modifiers must default to false"
        );
    }

    #[test]
    fn mouse_button_name_round_trips() {
        for (name, button) in [
            ("Left", MouseButton::Left),
            ("Right", MouseButton::Right),
            ("Middle", MouseButton::Middle),
            ("Back", MouseButton::Back),
            ("Forward", MouseButton::Forward),
        ] {
            assert_eq!(
                mouse_button_from_name(name),
                Some(button),
                "from_name failed for {name}"
            );
            assert_eq!(
                mouse_button_name(button).as_deref(),
                Some(name),
                "to_name failed for {name}"
            );
        }
        assert_eq!(
            mouse_button_from_name("Other"),
            None,
            "Other must return None"
        );
        assert_eq!(
            mouse_button_name(MouseButton::Other(42)),
            None,
            "Other(_) must return None"
        );
    }

    // Helpers shared by the applier tests.

    fn preset_one(operator: &str, key: &str) -> KeymapPreset {
        KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: operator.to_string(),
                input: PresetInput::key(key),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        }
    }

    fn spawn_action(world: &mut World, operator_id: &'static str) -> Entity {
        world
            .spawn((OperatorAction(operator_id), TriggerState::default()))
            .id()
    }

    #[test]
    fn applier_spawns_and_replaces_bindings() {
        let mut world = World::new();

        // Spawn an action entity tagged with the operator id.
        let _action = spawn_action(&mut world, "tool.select");

        // Apply a preset with one binding; expect exactly 1 spawn.
        let preset_a = preset_one("tool.select", "KeyQ");
        let report_a = apply_keymap_preset(&mut world, &preset_a);
        assert_eq!(
            report_a.spawned_bindings, 1,
            "first application should spawn 1 binding"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            1
        );

        // Re-apply with a different key; old binding is gone, still exactly 1.
        let preset_b = preset_one("tool.select", "KeyW");
        let report_b = apply_keymap_preset(&mut world, &preset_b);
        assert_eq!(
            report_b.spawned_bindings, 1,
            "re-application should spawn 1 binding"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            1,
            "re-application must not accumulate; old binding must be removed"
        );

        // Apply with an unknown operator id; zero spawns and one skip recorded.
        let preset_unknown = preset_one("unknown.op", "KeyQ");
        let report_unknown = apply_keymap_preset(&mut world, &preset_unknown);
        assert_eq!(
            report_unknown.spawned_bindings, 0,
            "unknown operator should yield 0 spawns"
        );
        assert_eq!(report_unknown.skipped_unknown_operator.len(), 1);
    }

    #[test]
    fn applier_binds_every_action_entity_of_an_operator() {
        let mut world = World::new();

        // Two action entities share the same operator id.
        let _a1 = spawn_action(&mut world, "tool.select");
        let _a2 = spawn_action(&mut world, "tool.select");

        let preset = preset_one("tool.select", "KeyQ");
        let report = apply_keymap_preset(&mut world, &preset);

        // One binding should be spawned per action entity.
        assert_eq!(
            report.spawned_bindings, 2,
            "each action entity must receive its own binding"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            2
        );
    }

    #[test]
    fn applier_never_touches_foreign_bindings() {
        let mut world = World::new();

        let action = spawn_action(&mut world, "tool.select");

        // Spawn a binding entity WITHOUT PresetSpawnedBinding to simulate
        // a raw/manual binding (e.g. from a test or a deferred raw site).
        let foreign = world
            .spawn(Binding::Keyboard {
                key: KeyCode::KeyF,
                mod_keys: ModKeys::empty(),
            })
            .id();

        // Apply and then re-apply so the despawn pass runs.
        let preset = preset_one("tool.select", "KeyQ");
        apply_keymap_preset(&mut world, &preset);
        apply_keymap_preset(&mut world, &preset);

        // The foreign binding entity must still exist.
        assert!(
            world.get_entity(foreign).is_ok(),
            "foreign binding entity must not be despawned by the applier"
        );

        // Only the one preset-owned binding should remain, not the foreign one.
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            1
        );

        // Suppress unused variable warning.
        let _ = action;
    }

    #[test]
    fn applier_applies_scroll_input_with_scroll_tick() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "view.zoom");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "view.zoom".into(),
                input: PresetInput::scroll(true),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(report.spawned_bindings, 1, "scroll must be applied");
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
        assert!(report.skipped_unknown_operator.is_empty());
        assert!(report.skipped_unparseable_key.is_empty());

        // The spawned binding must be a MouseWheel variant.
        let binding = world
            .query_filtered::<&Binding, With<PresetSpawnedBinding>>()
            .single(&world)
            .expect("exactly one preset binding must exist");
        assert!(
            matches!(binding, Binding::MouseWheel { .. }),
            "scroll entry must produce a MouseWheel binding, got {binding:?}"
        );
    }

    #[test]
    fn applier_applies_mouse_button_with_press_condition() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "view.orbit");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "view.orbit".into(),
                input: PresetInput::mouse("Middle"),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(report.spawned_bindings, 1, "mouse button must be applied");
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
        assert!(report.skipped_unparseable_key.is_empty());

        let binding = world
            .query_filtered::<&Binding, With<PresetSpawnedBinding>>()
            .single(&world)
            .expect("exactly one preset binding must exist");
        assert!(
            matches!(
                binding,
                Binding::MouseButton {
                    button: MouseButton::Middle,
                    ..
                }
            ),
            "mouse button entry must produce a MouseButton binding, got {binding:?}"
        );
    }

    #[test]
    fn applier_applies_double_click_phase() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "select.add");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "select.add".into(),
                input: PresetInput::mouse("Left"),
                phase: PresetPhase::DoubleClick,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(
            report.spawned_bindings, 1,
            "double-click phase must be applied"
        );
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
    }

    #[test]
    fn applier_rejects_unknown_mouse_button_name() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "some.op");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "some.op".into(),
                input: PresetInput::MouseButton {
                    button: "MiddleThumbnail".to_string(),
                    ctrl: false,
                    shift: false,
                    alt: false,
                },
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(report.spawned_bindings, 0, "unknown button must not spawn");
        assert_eq!(
            report.skipped_unparseable_key.len(),
            1,
            "unknown button name must land in skipped_unparseable_key"
        );
    }

    #[test]
    fn applier_applies_modal_context_when_builtin_registered() {
        let mut world = World::new();

        // Register the builtin action entity so the applier can find it.
        let action = world.spawn(TriggerState::default()).id();
        world
            .get_resource_or_init::<BuiltinActions>()
            .register("modal.confirm", action);

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "modal.confirm".into(),
                input: PresetInput::key("Enter"),
                phase: PresetPhase::Press,
                context: PresetContext::Modal,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(
            report.spawned_bindings, 1,
            "modal-context entry must be applied when builtin is registered"
        );
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
        assert!(report.skipped_unknown_operator.is_empty());
        assert!(report.skipped_unparseable_key.is_empty());
    }

    #[test]
    fn applier_skips_modal_context_when_builtin_absent() {
        // With no BuiltinActions resource, modal entries land in
        // skipped_unknown_operator (not skipped_unsupported).
        let mut world = World::new();

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "modal.confirm".into(),
                input: PresetInput::key("Enter"),
                phase: PresetPhase::Press,
                context: PresetContext::Modal,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(
            report.spawned_bindings, 0,
            "modal entry must not spawn when unregistered"
        );
        assert_eq!(report.applied_entries, 0);
        assert!(
            report.skipped_unsupported.is_empty(),
            "skipped_unsupported must be empty; unknown builtins go to skipped_unknown_operator"
        );
        assert_eq!(
            report.skipped_unknown_operator.len(),
            1,
            "unregistered builtin must land in skipped_unknown_operator"
        );
    }
}
