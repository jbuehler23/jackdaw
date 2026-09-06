use bevy::prelude::*;
use jackdaw_env::paths::keybinds_path;
use serde_json::{Map, Value};

pub use jackdaw_commands::keybinds::{EditorAction, Keybind, KeybindRegistry};

/// The modifiers a chord names for itself.
///
/// A binding says nothing about the modifiers it leaves out, so anything
/// not named here holds the chord back; see [`unwanted_modifier`].
#[derive(Clone, Copy, Default)]
pub(crate) struct ChordModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Whether a modifier the chord did not name is being held.
///
/// `bevy_enhanced_input` matches a binding on the modifiers it *names* and
/// says nothing about the ones it does not, so a binding on a bare key
/// answers a chord built on that key as well: Ctrl+C started a cut brush,
/// and Ctrl+Shift+Z ran Undo alongside Redo. Every chord that shares a key
/// with a longer one asks this before acting.
pub(crate) fn unwanted_modifier(keyboard: &ButtonInput<KeyCode>, named: ChordModifiers) -> bool {
    let held = |wanted: bool, keys: [KeyCode; 2]| !wanted && keyboard.any_pressed(keys);
    held(named.ctrl, [KeyCode::ControlLeft, KeyCode::ControlRight])
        || held(named.ctrl, [KeyCode::SuperLeft, KeyCode::SuperRight])
        || held(named.alt, [KeyCode::AltLeft, KeyCode::AltRight])
        || held(named.shift, [KeyCode::ShiftLeft, KeyCode::ShiftRight])
}

pub struct KeybindsPlugin;

impl Plugin for KeybindsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeybindRegistry>()
            .add_systems(OnEnter(crate::AppState::Editor), load_keybinds);
    }
}

fn load_keybinds(mut registry: ResMut<KeybindRegistry>) {
    let Some(path) = keybinds_path() else {
        return;
    };
    if !path.is_file() {
        return;
    }
    let Ok(data) = std::fs::read_to_string(&path) else {
        warn!("Failed to read keybinds file: {}", path.display());
        return;
    };
    let Ok(map) = serde_json::from_str::<Map<String, Value>>(&data) else {
        warn!("Failed to parse keybinds file as JSON object");
        return;
    };

    for (key, value) in map {
        let Some(action) = EditorAction::from_display_name(&key) else {
            warn!("Unknown keybind action: {key}");
            continue;
        };
        let bindings = match value {
            Value::String(s) => match Keybind::parse(&s) {
                Some(b) => vec![b],
                None => {
                    warn!("Failed to parse keybind \"{s}\" for {key}");
                    continue;
                }
            },
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| {
                    let s = v.as_str()?;
                    let b = Keybind::parse(s);
                    if b.is_none() {
                        warn!("Failed to parse keybind \"{s}\" for {key}");
                    }
                    b
                })
                .collect(),
            _ => {
                warn!("Invalid keybind value for {key}");
                continue;
            }
        };
        registry.bindings.insert(action, bindings);
    }

    info!("Loaded custom keybinds from {}", path.display());
}

pub fn save_keybinds(registry: &KeybindRegistry) {
    let Some(path) = keybinds_path() else {
        warn!("Could not determine config directory for keybinds");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut map = Map::new();
    // Sort by action display name for stable output
    let mut entries: Vec<_> = registry.bindings.iter().collect();
    entries.sort_by_key(|(action, _)| action.to_string());

    for (action, bindings) in entries {
        let key = action.to_string();
        let value = if bindings.len() == 1 {
            Value::String(bindings[0].to_string())
        } else {
            Value::Array(
                bindings
                    .iter()
                    .map(|b| Value::String(b.to_string()))
                    .collect(),
            )
        };
        map.insert(key, value);
    }

    match serde_json::to_string_pretty(&map) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                warn!("Failed to write keybinds file: {e}");
            } else {
                info!("Saved keybinds to {}", path.display());
            }
        }
        Err(e) => {
            warn!("Failed to serialize keybinds: {e}");
        }
    }
}
