//! Documents the editor refuses to load.
//!
//! A retired component does not announce itself on load: the patch names a
//! type the registry does not carry, the apply path warns and moves on, and
//! the user gets back a scene that has quietly lost something. Refusing the
//! document by name says what went missing instead, and every path that
//! installs a document refuses alike: an interactive open, a tab activation,
//! and a game runtime loading the same file.

use crate::SceneBsnAst;

/// Type-path prefix of the removed facade UI vocabulary (`UiCanvas`,
/// `UiButton`, `UiSlot`, ...). Matched as a prefix so every retired type is
/// covered without listing them.
pub const RETIRED_UI_PREFIX: &str = "jackdaw_ui::";

/// Reject a document that still carries facade UI components.
///
/// These types do not exist, so loading would silently drop them and hand the
/// user a scene that has lost its UI. The error names the removal instead.
pub fn reject_retired_ui_components(ast: &SceneBsnAst) -> Result<(), RetiredUiComponents> {
    let mut found: Vec<String> = Vec::new();
    for type_path in ast
        .all_patch_type_paths()
        .filter(|path| path.starts_with(RETIRED_UI_PREFIX))
    {
        if !found.iter().any(|seen| seen == type_path) {
            found.push(type_path.to_string());
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    found.sort();
    Err(RetiredUiComponents(found))
}

/// The facade UI components a rejected scene still carries.
#[derive(Debug)]
pub struct RetiredUiComponents(Vec<String>);

impl core::fmt::Display for RetiredUiComponents {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "the facade UI system was removed, and this scene still uses it ({}). \
             Re-author its UI with real Bevy UI components (Node, Text, and the \
             bevy_ui_widgets widgets) and save it again.",
            self.0.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::reject_retired_ui_components;

    fn rejection(bsn: &str) -> String {
        let ast = crate::parse_bsn_text(bsn).expect("the fixture parses");
        reject_retired_ui_components(&ast)
            .expect_err("a facade UI component must be rejected")
            .to_string()
    }

    #[test]
    fn a_scene_using_a_facade_ui_component_is_rejected_by_name() {
        let message = rejection(
            r#"
#Overlay
jackdaw_ui::UiCanvas
"#,
        );

        assert!(
            message.contains("jackdaw_ui::UiCanvas"),
            "the message must name the offending component: {message}"
        );
        assert!(
            message.contains("facade UI system was removed"),
            "the message must explain the removal rather than read as a generic \
             unknown type: {message}"
        );
        assert!(
            message.contains("Bevy UI components"),
            "the message must point at the replacement: {message}"
        );
    }

    #[test]
    fn every_retired_type_is_caught_by_the_prefix_not_a_list() {
        // Struct, tuple-struct, and bare-type patch forms all carry a type
        // path, and a nested child is rejected as a root is.
        let message = rejection(
            r#"
bevy_ecs::hierarchy::Children [
    #Root
    jackdaw_ui::UiStyleOverride { padding: 4.0 }
    Children [
        #Child
        jackdaw_ui::UiSlot("content")
    ]
]
"#,
        );

        assert!(message.contains("jackdaw_ui::UiSlot"), "{message}");
        assert!(message.contains("jackdaw_ui::UiStyleOverride"), "{message}");
    }

    #[test]
    fn a_scene_with_no_facade_components_loads() {
        let ast = crate::parse_bsn_text(
            r#"
#World
bevy_transform::components::transform::Transform
"#,
        )
        .expect("the fixture parses");

        assert!(reject_retired_ui_components(&ast).is_ok());
    }
}
