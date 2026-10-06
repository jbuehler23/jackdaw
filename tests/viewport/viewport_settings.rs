//! The viewport's quality, show and redraw settings: kept per user, set by
//! operators, and never written into the scene.

use crate::util;

use bevy::prelude::*;
use jackdaw::viewport_settings::{
    AntiAliasing, QualityPreset, ViewportSettings, ViewportSettingsFile,
};
use jackdaw_api::prelude::*;

/// An editor whose viewport settings are kept in `dir` rather than the
/// process's shared config directory.
fn app_keeping_settings_in(dir: &std::path::Path) -> App {
    let mut app = util::editor_test_app();
    app.world_mut().resource_mut::<ViewportSettingsFile>().path = Some(dir.join("viewport.json"));
    app
}

fn call(app: &mut App, id: &'static str, params: &[(&'static str, &str)]) -> OperatorResult {
    let mut call = app.world_mut().operator(id);
    for (name, value) in params {
        call = call.param(*name, value.to_string());
    }
    let result = call
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"));
    app.update();
    result
}

#[track_caller]
fn run(app: &mut App, id: &'static str, params: &[(&'static str, &str)]) {
    assert_eq!(
        call(app, id, params),
        OperatorResult::Finished,
        "{id} {params:?}"
    );
}

fn settings(app: &App) -> &ViewportSettings {
    app.world().resource::<ViewportSettings>()
}

#[test]
fn a_preset_sets_every_quality_choice_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_keeping_settings_in(dir.path());
    run(&mut app, "viewport.quality.preset", &[("preset", "low")]);
    assert_eq!(settings(&app).quality, QualityPreset::Low.quality());
    assert_eq!(settings(&app).preset(), Some(QualityPreset::Low));
}

#[test]
fn setting_one_choice_keeps_the_others_and_leaves_the_preset() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_keeping_settings_in(dir.path());
    run(
        &mut app,
        "viewport.quality.set",
        &[("anti_aliasing", "taa"), ("render_scale", "75")],
    );
    let quality = settings(&app).quality;
    assert_eq!(quality.anti_aliasing, AntiAliasing::Taa);
    assert_eq!(quality.render_scale, 75);
    assert_eq!(
        quality.shadow_quality,
        QualityPreset::High.quality().shadow_quality
    );
    assert_eq!(settings(&app).preset(), None);
}

#[test]
fn a_show_flag_flips_or_takes_the_state_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_keeping_settings_in(dir.path());
    run(
        &mut app,
        "viewport.show.toggle",
        &[("flag", "point_shadows")],
    );
    assert!(!settings(&app).quality.point_shadows);
    run(
        &mut app,
        "viewport.show.toggle",
        &[("flag", "terrain_detail"), ("on", "false")],
    );
    run(
        &mut app,
        "viewport.show.toggle",
        &[("flag", "terrain_detail"), ("on", "false")],
    );
    assert!(!settings(&app).terrain_detail);
}

#[test]
fn names_the_settings_do_not_have_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_keeping_settings_in(dir.path());
    let before = settings(&app).clone();
    for (id, params) in [
        ("viewport.quality.preset", [("preset", "ultra")]),
        ("viewport.quality.set", [("render_scale", "10")]),
        ("viewport.quality.set", [("anti_aliasing", "msaa")]),
        ("viewport.show.toggle", [("flag", "everything")]),
    ] {
        assert_eq!(
            call(&mut app, id, &params),
            OperatorResult::Cancelled,
            "{id} {params:?}"
        );
    }
    assert_eq!(*settings(&app), before);
}

#[test]
fn a_changed_setting_is_kept_in_the_users_settings_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_keeping_settings_in(dir.path());
    let file = dir.path().join("viewport.json");
    assert!(!file.exists(), "nothing is written before a change");

    run(&mut app, "viewport.quality.preset", &[("preset", "medium")]);
    run(&mut app, "viewport.realtime.toggle", &[]);
    run(&mut app, "viewport.stats.toggle", &[]);

    let kept = jackdaw::viewport_settings::load(&file);
    assert_eq!(kept, *settings(&app));
    assert_eq!(kept.preset(), Some(QualityPreset::Medium));
    assert!(!kept.realtime && kept.stats);
}

#[test]
fn changing_viewport_settings_leaves_the_saved_scene_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_keeping_settings_in(dir.path());
    for id in [
        "entity.add.directional_light",
        "entity.add.point_light",
        "entity.add.spot_light",
        "entity.add.camera",
        "entity.add.cube",
    ] {
        run(&mut app, id, &[]);
    }
    for clause in [
        "entity.add.group name=Sky",
        "component.add name=Sky type_path=jackdaw_scene_types::environment::Environment",
        "environment.set fog.mode=Linear post.bloom_intensity=0.3",
    ] {
        let result = jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
            .unwrap_or_else(|err| panic!("{clause}: {err}"));
        assert_eq!(result, OperatorResult::Finished, "{clause}");
        app.update();
    }
    let scene = dir.path().join("scene.bsn");
    let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
    if scenes.tabs.is_empty() {
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.active = 0;
    }
    let active = scenes.active;
    scenes.tabs[active].path = Some(scene.clone());
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let before = std::fs::read(&scene).expect("the scene is on disk");

    run(&mut app, "viewport.quality.preset", &[("preset", "low")]);
    for flag in [
        "sun_shadows",
        "point_shadows",
        "sky_reflection",
        "fog",
        "post_processing",
        "bloom",
        "ambient_occlusion",
        "terrain_detail",
    ] {
        run(
            &mut app,
            "viewport.show.toggle",
            &[("flag", flag), ("on", "false")],
        );
    }
    run(&mut app, "viewport.realtime.toggle", &[]);
    run(&mut app, "viewport.stats.toggle", &[]);
    for mode in ["lighting_only", "wireframe", "lod_colors", "unlit"] {
        run(&mut app, "view.mode", &[("mode", mode)]);
        app.update();
    }
    for _ in 0..3 {
        app.update();
    }

    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let after = std::fs::read(&scene).expect("the scene is on disk");
    assert!(
        before == after,
        "the saved scene changed:\n{}\n---\n{}",
        String::from_utf8_lossy(&before),
        String::from_utf8_lossy(&after)
    );
}
