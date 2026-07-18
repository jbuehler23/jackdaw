//! Run-configuration manifest (`jackdaw.toml`) types.
//!
//! The editor reads these from the project root to build the Play
//! dropdown. The game never parses them; it only learns its ipc
//! rendezvous name from the bootstrap env var.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::event::PieMode;

/// One launchable run configuration. Every run launches the same
/// project dylib through the game runner; configs differ only in
/// launch environment, not in what gets built.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct RunConfig {
    /// Dropdown label; defaults to `Play`.
    #[serde(default)]
    pub name: Option<String>,
    /// Number of individually launchable copies (`Label #1..#N`).
    #[serde(default = "one")]
    pub instances: u32,
    /// Process env vars set on the child; the game's own input surface.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Extra argv appended after the game's own.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory; defaults to the project root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Engine-execution axis; `editor-preview` reserved.
    #[serde(default)]
    pub mode: PieMode,
}

fn one() -> u32 {
    1
}

impl RunConfig {
    /// The label shown in the dropdown.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("Play")
    }
}

/// The parsed `jackdaw.toml`.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Manifest {
    /// The game plugin type inside the project's lib crate. Defaults
    /// to source detection, then to `GamePlugin`.
    #[serde(default)]
    pub plugin: Option<String>,
    #[serde(default, rename = "run")]
    pub runs: Vec<RunConfig>,
}

impl Manifest {
    /// Find a run config by its dropdown label.
    pub fn run_by_name(&self, label: &str) -> Option<&RunConfig> {
        self.runs.iter().find(|r| r.label() == label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_and_defaults() {
        let m: Manifest = toml::from_str(
            r#"[[run]]
name = "My Game""#,
        )
        .unwrap();
        assert_eq!(m.runs[0].label(), "My Game");
        assert_eq!(m.runs[0].instances, 1);
        assert_eq!(m.runs[0].mode, PieMode::Play);
        assert!(m.plugin.is_none());
    }

    #[test]
    fn full_entry_parses() {
        let m: Manifest = toml::from_str(
            r#"
plugin = "MyGamePlugin"

[[run]]
name = "Client"
instances = 4
env = { ADDR = "127.0.0.1:5000" }
"#,
        )
        .unwrap();
        assert_eq!(m.plugin.as_deref(), Some("MyGamePlugin"));
        assert_eq!(m.runs[0].label(), "Client");
        assert_eq!(m.runs[0].instances, 4);
        assert_eq!(m.runs[0].env.get("ADDR").unwrap(), "127.0.0.1:5000");
        assert_eq!(m.run_by_name("Client").unwrap().instances, 4);
    }

    #[test]
    fn editor_preview_mode_parses() {
        let m: Manifest = toml::from_str(
            r#"[[run]]
mode = "editor-preview""#,
        )
        .unwrap();
        assert_eq!(m.runs[0].mode, PieMode::EditorPreview);
    }
}
