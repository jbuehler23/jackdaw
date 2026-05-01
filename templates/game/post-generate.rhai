// Rename the template files to their real names after liquid
// substitution has run. Mirrors TheBevyFlock/bevy_new_2d.
file::rename("Cargo.toml.template", "Cargo.toml");
file::rename("README.md.template", "README.md");
file::rename("src/lib.rs.template", "src/lib.rs");
file::rename("build.rs.template", "build.rs");
