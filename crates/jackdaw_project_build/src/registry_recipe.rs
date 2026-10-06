//! The SDK recipe of a jackdaw built from its published package. Shared by
//! `build.rs`, which embeds it, and this crate's tests.

/// A manifest depending on the SDK crates at exactly `version` from crates.io.
/// The bootstrap builds them with `-p`, as it does the workspace recipe.
pub fn manifest(version: &str) -> String {
    format!(
        "[package]\n\
         name = \"jackdaw-sdk-build\"\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\
         \n\
         [dependencies]\n\
         jackdaw_sdk = \"={version}\"\n\
         jackdaw_rustc_wrapper = \"={version}\"\n\
         \n\
         [workspace]\n"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_registry_recipe_pins_the_sdk_crates_to_this_version() {
        let manifest: toml::Value = toml::from_str(&super::manifest("0.19.3")).unwrap();
        let dependencies = manifest["dependencies"].as_table().unwrap();
        for name in ["jackdaw_sdk", "jackdaw_rustc_wrapper"] {
            assert_eq!(dependencies[name].as_str(), Some("=0.19.3"), "{name}");
        }
        assert!(
            manifest.get("workspace").is_some(),
            "the recipe is its own workspace, whatever directory it is unpacked under"
        );
    }
}
