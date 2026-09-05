//! Scaffolded projects must request the anchored jackdaw and Bevy versions.
//! CI builds the templates with a `[patch.crates-io]` redirect to this
//! checkout, so nothing else checks the versions they actually resolve from
//! crates.io.
//!
//! The templates state versions as placeholders substituted at scaffold
//! time, so this covers both halves: the templates must not hard-code a
//! version that could drift, and the value substituted in must be the
//! workspace's anchored minor. (Rendering a project here would not prove
//! the second half: inside a source checkout the jackdaw dependencies are
//! rewritten to path deps.)

use jackdaw::scaffold::TemplateKind;

fn workspace_minor() -> String {
    let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let version = toml
        .lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("[workspace.package] version");
    let mut parts = version.split('.');
    let major = parts.next().expect("major");
    let minor = parts.next().expect("minor");
    format!("{major}.{minor}")
}

fn template(path: &str) -> String {
    let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn templates_state_versions_as_placeholders() {
    for (path, dep) in [
        ("templates/game/Cargo.toml.template", "jackdaw_runtime"),
        (
            "templates/extension/Cargo.toml.template",
            "jackdaw_extension",
        ),
    ] {
        let text = template(path);
        assert!(
            text.contains("bevy = \"{{bevy_version}}\""),
            "{path}: bevy must use the {{{{bevy_version}}}} placeholder"
        );
        let dep_line = text
            .lines()
            .find(|line| line.trim_start().starts_with(dep))
            .unwrap_or_else(|| panic!("{path}: no {dep} dependency"));
        assert!(
            dep_line.contains("{{jackdaw_version}}"),
            "{path}: {dep} must use the {{{{jackdaw_version}}}} placeholder, got: {dep_line}"
        );
    }
}

#[test]
fn the_substituted_version_is_the_anchored_minor() {
    assert_eq!(jackdaw_project_build::BEVY_VERSION, workspace_minor());
}

/// The game template's third-party requirements are plain literals (they
/// are not jackdaw's to substitute), so they can silently drift from what
/// the editor itself builds against. A scaffolded project resolving a
/// different avian than the editor authors scenes with is exactly the
/// kind of mismatch onboarding is supposed to prevent.
#[test]
fn the_game_template_tracks_the_workspace_avian() {
    let workspace = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("workspace manifest");
    let req = workspace
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("avian3d = {")?;
            let rest = rest.split("version = \"").nth(1)?;
            rest.split('"').next()
        })
        .expect("[workspace.dependencies] avian3d version");
    let template = template("templates/game/Cargo.toml.template");
    assert!(
        template.contains(&format!("avian3d = \"{req}\"")),
        "template must request avian3d \"{req}\":\n{template}"
    );
}

/// The game template's own doc comment mentions `assets/*.bsn`, whose
/// `/*` once made detection discard the rest of the file, so a freshly
/// scaffolded project had no findable plugin and its Play button did
/// nothing. Scaffold for real and check the shipped templates against
/// the detector that has to read them.
#[test]
fn scaffolded_projects_expose_their_plugin_to_detection() {
    let dest = std::env::temp_dir()
        .join(format!("jackdaw_template_detect_{}", std::process::id()))
        .join("detectable");
    let _ = std::fs::remove_dir_all(&dest);
    jackdaw::scaffold::scaffold_new_project(&dest, "detectable", TemplateKind::Game)
        .expect("scaffold");

    let candidates = jackdaw_project_build::detect::plugin_candidates(&dest);
    assert!(
        candidates.contains(&"GamePlugin".to_string()),
        "the game template's plugin must be detectable, got: {candidates:?}"
    );
    assert_eq!(
        jackdaw_project_build::detect::detect_plugin(&dest, "detectable").as_deref(),
        Some("detectable::GamePlugin"),
        "and must resolve to a path the shim can name"
    );
    let _ = std::fs::remove_dir_all(&dest);
}

/// Same for the extension template and its own trait.
#[test]
fn scaffolded_extensions_expose_their_type_to_detection() {
    let dest = std::env::temp_dir()
        .join(format!("jackdaw_template_ext_{}", std::process::id()))
        .join("detectable-ext");
    let _ = std::fs::remove_dir_all(&dest);
    jackdaw::scaffold::scaffold_new_project(&dest, "detectable-ext", TemplateKind::Extension)
        .expect("scaffold");

    let found = jackdaw_project_build::detect::detect_extension(&dest);
    assert!(
        found.is_some(),
        "the extension template's type must be detectable"
    );
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn the_settings_template_records_version_pins() {
    let text = template("templates/game/jackdaw.toml.template");
    assert!(text.contains("{{jackdaw_pins}}"));
}
