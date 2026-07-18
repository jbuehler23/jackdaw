//! Lightweight source detection: find a project's `JackdawExtension`
//! impl and its game `Plugin` type by scanning `src/lib.rs`. Pure
//! filesystem and string parsing, no editor or bevy dependency, so the
//! build pipeline can resolve a shim spec from the filesystem alone.

use std::path::Path;

/// Detect a `JackdawExtension` impl in the project's lib source,
/// returning `(lib_name, extension_type_name)`.
pub fn detect_extension(root: &Path) -> Option<(String, String)> {
    let src = std::fs::read_to_string(root.join("src/lib.rs")).ok()?;
    for line in src.lines() {
        let trimmed = line.trim_start();
        let Some(after) = trimmed.strip_prefix("impl ") else {
            continue;
        };
        let Some((_, tail)) = after.split_once("JackdawExtension for ") else {
            continue;
        };
        let name: String = tail
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            let lib_name = root
                .join("src/lib.rs")
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().replace('-', "_"))
                .unwrap_or_default();
            return Some((lib_name, name));
        }
    }
    None
}

/// Find the plugin type in the project's lib source: the first
/// `pub struct ...Plugin`. Returns `lib_name::TypeName`.
pub fn detect_plugin(root: &Path, lib_name: &str) -> Option<String> {
    let src = std::fs::read_to_string(root.join("src/lib.rs")).ok()?;
    for line in src.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("pub struct ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.ends_with("Plugin") && name.len() > "Plugin".len() {
            return Some(format!("{lib_name}::{name}"));
        }
    }
    None
}
