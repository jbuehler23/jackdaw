//! Scaffolded projects must request the anchored jackdaw and Bevy versions. CI
//! builds the templates with a `[patch.crates-io]` redirect to this checkout, so
//! nothing else checks what they resolve from crates.io.
//!
//! The templates state versions as placeholders substituted at scaffold time, so
//! both halves are covered: no hard-coded version that could drift, and the
//! substituted value is the workspace's anchored minor.

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
        (
            "crates/jackdaw_scaffold/templates/game/Cargo.toml.template",
            "jackdaw_runtime",
        ),
        (
            "crates/jackdaw_scaffold/templates/extension/Cargo.toml.template",
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
        // The whole requirement, not just a version: what a scaffolded
        // project can reach the crate through depends on how this jackdaw
        // was built, and a literal version is one of three answers.
        let placeholder = format!("{{{{{dep}_dep}}}}");
        assert!(
            dep_line.contains(&placeholder),
            "{path}: {dep} must use the {placeholder} placeholder, got: {dep_line}"
        );
    }
}

#[test]
fn the_substituted_version_is_the_anchored_minor() {
    assert_eq!(jackdaw_project_build::BEVY_VERSION, workspace_minor());
}

/// The physics module's avian source is a plain literal in the module
/// catalog (it is not jackdaw's to substitute), so it can silently drift
/// from what the Physics module builds against. A scaffolded project
/// resolving a different avian than the module authors scenes with is
/// exactly the kind of mismatch onboarding is supposed to prevent.
#[test]
fn the_physics_module_tracks_the_integrations_avian() {
    let workspace = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/integrations/physics/Cargo.toml"
    ))
    .expect("physics workspace manifest");
    let entry = workspace
        .split_once("[workspace.dependencies]")
        .expect("[workspace.dependencies]")
        .1
        .lines()
        .find_map(|line| line.trim().strip_prefix("avian3d = {"))
        .expect("[workspace.dependencies] avian3d");
    let key = |name: &str| {
        let rest = entry.split(&format!("{name} = \"")).nth(1)?;
        rest.split('"').next().map(str::to_string)
    };
    let source: Vec<(&str, String)> = ["version", "git", "rev"]
        .into_iter()
        .filter_map(|name| Some((name, key(name)?)))
        .collect();
    assert!(!source.is_empty(), "avian3d names no version or git source");
    let dest = std::env::temp_dir()
        .join(format!("jackdaw_template_avian_{}", std::process::id()))
        .join("physical");
    let _ = std::fs::remove_dir_all(&dest);
    let physics = ["physics".to_string()].into_iter().collect();
    jackdaw::scaffold::scaffold_new_project_with(&dest, "physical", TemplateKind::Game, &physics)
        .expect("scaffold");
    let cargo = std::fs::read_to_string(dest.join("Cargo.toml")).expect("scaffolded manifest");
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
    let line = cargo
        .lines()
        .find(|line| line.starts_with("avian3d = "))
        .unwrap_or_else(|| panic!("a game with the physics module must request avian3d:\n{cargo}"));
    let bare = matches!(source.as_slice(), [("version", _)]);
    for (name, value) in &source {
        let wanted = if bare {
            format!("\"{value}\"")
        } else {
            format!("{name} = \"{value}\"")
        };
        assert!(
            line.contains(&wanted),
            "the scaffolded avian3d must carry the workspace's {name} {value}: {line}"
        );
    }
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
    let text = template("crates/jackdaw_scaffold/templates/game/jackdaw.toml.template");
    assert!(text.contains("{{jackdaw_pins}}"));
}
