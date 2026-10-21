//! A module's tooling extension is on only in projects that use the module.

use crate::util;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{Extension, register_dylib_extension};
use jackdaw_project_build::modules::modules_toml;
use jackdaw_project_build::project_manifest::pins_toml;

const NAVMESH: &str = "jackdaw.navmesh";

#[derive(Default)]
struct NavmeshTooling;

impl JackdawExtension for NavmeshTooling {
    fn id(&self) -> String {
        NAVMESH.to_string()
    }

    fn register(&self, _: &mut ExtensionContext) {}
}

fn is_loaded(app: &mut App) -> bool {
    let world = app.world_mut();
    world
        .query::<&Extension>()
        .iter(world)
        .any(|extension| extension.id == NAVMESH)
}

fn project(name: &str, modules: &[&str]) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("module-extensions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let enabled = modules.iter().map(ToString::to_string).collect();
    std::fs::write(
        dir.join("jackdaw.toml"),
        format!("{}\n{}", pins_toml(), modules_toml(&enabled)),
    )
    .unwrap();
    dir
}

#[test]
fn opening_a_project_turns_module_tooling_on_or_off_to_match_it() {
    util::isolate_config_dir();
    let mut app = util::editor_test_app();
    register_dylib_extension(app.world_mut(), || Box::new(NavmeshTooling));

    let with = project("with", &["navmesh"]);
    jackdaw::launcher::apply_project_overrides(app.world_mut(), &with);
    app.update();
    assert!(is_loaded(&mut app));

    let without = project("without", &[]);
    jackdaw::launcher::apply_project_overrides(app.world_mut(), &without);
    app.update();
    assert!(!is_loaded(&mut app));

    let _ = std::fs::remove_dir_all(with);
    let _ = std::fs::remove_dir_all(without);
}
