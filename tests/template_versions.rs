//! Scaffold templates must request the anchored jackdaw version. CI builds the
//! templates with a `[patch.crates-io]` redirect to this checkout, so nothing
//! else checks the version they actually resolve from crates.io.

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

#[test]
fn templates_request_the_anchored_jackdaw_version() {
    let minor = workspace_minor();
    for path in [
        "templates/game/Cargo.toml.template",
        "templates/extension/Cargo.toml.template",
    ] {
        let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mut checked = 0;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("jackdaw_runtime") || trimmed.starts_with("jackdaw_api") {
                assert!(
                    trimmed.contains(&format!("\"{minor}\"")),
                    "{path}: jackdaw dependency must request \"{minor}\", got: {trimmed}"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "{path}: expected at least one jackdaw dependency line to check"
        );
    }
}
