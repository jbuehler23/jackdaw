// Rename the template files to their real names after Liquid
// substitution has run. Mirrors TheBevyFlock/bevy_new_2d.
file::rename("Cargo.toml.template", "Cargo.toml");
file::rename("README.md.template", "README.md");
file::rename("src/lib.rs.template", "src/lib.rs");
file::rename("src/main.rs.template", "src/main.rs");
file::rename("src/bin/editor.rs.template", "src/bin/editor.rs");
file::rename("tests/template_components.rs.template", "tests/template_components.rs");
