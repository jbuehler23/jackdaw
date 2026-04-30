use bevy::prelude::*;
use jackdaw_api_internal::paths::keybinds_path;
use serde_json::{Map, Value};

pub use jackdaw_commands::keybinds::{EditorAction, Keybind, KeybindRegistry};

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
