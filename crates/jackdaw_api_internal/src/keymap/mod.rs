//! Data-driven keymap presets. A preset is a serializable document of
//! operator-id bindings; applying one replaces the BEI binding entities
//! of every operator action it names. Extensions record their defaults
//! through `ExtensionContext::bind_operator`, and the generated
//! "classic" preset reproduces those defaults exactly.

mod apply;
mod persist;
mod types;

pub use apply::{KeymapApplyReport, PresetSpawnedBinding, apply_keymap_preset};
pub use persist::{load_active_keymap_preset, save_active_keymap_preset};
pub use types::{
    ActiveKeymapPreset, BuiltinActions, DefaultKeymap, KeymapPreset, PresetBinding, PresetContext,
    PresetInput, PresetPhase, key_code_from_name, key_code_name, mouse_button_from_name,
    mouse_button_name,
};
