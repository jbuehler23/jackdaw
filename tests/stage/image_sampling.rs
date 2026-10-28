//! The project's 2D sampling and the per-image override: where they are kept,
//! what the editor shows, and the files it writes or leaves alone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::image::{ImageFilterMode, ImageSampler};
use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::image_sampling::{ProjectSampling2d, Sampling};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::project_settings::settings_path;
use jackdaw::texture_import::meta_path;
use jackdaw_api::prelude::*;
use jackdaw_runtime::game_settings_file;
use tempfile::TempDir;

/// A 1x1 PNG.
const PNG: [u8; 70] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 0,
    5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// A project of this test's own, its 2D sampling `sampling` when given, with
/// the images `art/tiles.png`, `art/hero.png` and `art/wall.png`.
fn project(sampling: Option<&str>) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("assets/art")).expect("the asset folder");
    std::fs::create_dir_all(root.join(".jackdaw")).expect("the settings folder");
    std::fs::write(settings_path(root), "{\n  \"auto_build\": true\n}").expect("settings");
    if let Some(sampling) = sampling {
        std::fs::write(
            game_settings_file(root),
            format!("{{ \"sampling_2d\": \"{sampling}\", \"other\": 1 }}"),
        )
        .expect("the game settings");
    }
    for image in ["tiles", "hero", "wall"] {
        std::fs::write(image_file(root, image), PNG).expect("an image");
    }
    dir
}

fn image_file(root: &Path, name: &str) -> PathBuf {
    root.join(format!("assets/art/{name}.png"))
}

fn open(app: &mut App, root: &Path) {
    app.world_mut().insert_resource(ProjectRoot {
        root: root.to_path_buf(),
        config: ProjectConfig::default(),
    });
}

/// An editor reading `root`'s assets, with `root` open.
fn open_project(root: &Path) -> App {
    let mut app = util::editor_test_app_reading(&root.join("assets"));
    open(&mut app, root);
    app.update();
    app
}

fn set_project(app: &mut App, mode: &'static str) {
    app.world_mut()
        .operator("project.set_2d_sampling")
        .param("mode", mode)
        .call()
        .expect("project.set_2d_sampling dispatches")
        .assert_finished();
    app.update();
}

fn set_image(app: &mut App, path: &'static str, mode: &'static str) -> OperatorResult {
    let result = app
        .world_mut()
        .operator("image.set_sampling")
        .param("path", path)
        .param("mode", mode)
        .call()
        .expect("image.set_sampling dispatches");
    app.update();
    result
}

/// A sprite drawing `path`, once its image has loaded.
fn loaded_sprite(app: &mut App, path: &'static str) -> Handle<Image> {
    let handle = app.world().resource::<AssetServer>().load::<Image>(path);
    app.world_mut().spawn(Sprite::from_image(handle.clone()));
    util::update_until(app, "the image to load", Duration::from_secs(10), |app| {
        app.world().resource::<Assets<Image>>().contains(&handle)
    });
    app.update();
    handle
}

/// The filter the loaded image samples with, `None` for the editor's default.
fn filter(app: &App, image: &Handle<Image>) -> Option<ImageFilterMode> {
    match &app.world().resource::<Assets<Image>>().get(image)?.sampler {
        ImageSampler::Descriptor(descriptor) => Some(descriptor.mag_filter),
        ImageSampler::Default => None,
    }
}

fn metas(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join("assets/art"))
        .expect("the art folder")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "meta")
        })
        .collect()
}

#[test]
fn the_setting_round_trips_through_the_game_settings_file() {
    let dir = project(Some("linear"));
    let mut app = open_project(dir.path());
    assert_eq!(
        app.world().resource::<ProjectSampling2d>().0,
        Sampling::Linear
    );

    set_project(&mut app, "nearest");

    // The game finds the file with the same function, from its asset root.
    let file = game_settings_file(dir.path());
    assert_eq!(
        Sampling::read(&file),
        Sampling::Nearest,
        "what the game reads"
    );
    let written = std::fs::read_to_string(&file).expect("written");
    assert!(
        written.contains("\"other\": 1"),
        "the other keys stay: {written}"
    );
    let editor_settings = std::fs::read_to_string(settings_path(dir.path())).expect("settings");
    assert!(!editor_settings.contains("sampling"), "{editor_settings}");

    let reopened = open_project(dir.path());
    assert_eq!(
        reopened.world().resource::<ProjectSampling2d>().0,
        Sampling::Nearest
    );
}

#[test]
fn the_editor_samples_2d_images_as_the_project_says_without_writing_metas() {
    let dir = project(Some("nearest"));
    let mut app = open_project(dir.path());
    let hero = loaded_sprite(&mut app, "art/hero.png");
    assert_eq!(filter(&app, &hero), Some(ImageFilterMode::Nearest));

    set_project(&mut app, "linear");
    assert_eq!(filter(&app, &hero), None, "back to the editor's default");
    set_project(&mut app, "nearest");
    assert_eq!(filter(&app, &hero), Some(ImageFilterMode::Nearest));

    assert!(metas(dir.path()).is_empty(), "{:?}", metas(dir.path()));
}

#[test]
fn an_image_loading_when_its_sprite_appears_is_sampled_once_in() {
    let dir = project(Some("nearest"));
    let mut app = util::editor_test_app_reading(&dir.path().join("assets"));
    open(&mut app, dir.path());
    let hero = loaded_sprite(&mut app, "art/hero.png");

    assert_eq!(filter(&app, &hero), Some(ImageFilterMode::Nearest));
}

#[test]
fn a_sprite_given_another_image_in_place_is_sampled_too() {
    let dir = project(Some("nearest"));
    let mut app = open_project(dir.path());
    let hero = loaded_sprite(&mut app, "art/hero.png");
    let sprite = app
        .world_mut()
        .query::<(Entity, &Sprite)>()
        .iter(app.world())
        .find(|(_, sprite)| sprite.image == hero)
        .map(|(entity, _)| entity)
        .expect("the sprite");

    let wall = app
        .world()
        .resource::<AssetServer>()
        .load::<Image>("art/wall.png");
    app.world_mut().get_mut::<Sprite>(sprite).unwrap().image = wall.clone();
    util::update_until(
        &mut app,
        "the wall to load",
        Duration::from_secs(10),
        |app| app.world().resource::<Assets<Image>>().contains(&wall),
    );
    app.update();

    assert_eq!(filter(&app, &wall), Some(ImageFilterMode::Nearest));
}

#[test]
fn an_override_wins_survives_a_default_change_and_project_clears_it() {
    let dir = project(None);
    let mut app = open_project(dir.path());
    let tiles = loaded_sprite(&mut app, "art/tiles.png");

    set_image(&mut app, "art/tiles.png", "nearest").assert_finished();
    assert_eq!(filter(&app, &tiles), Some(ImageFilterMode::Nearest));
    set_project(&mut app, "nearest");
    set_project(&mut app, "linear");
    assert_eq!(
        filter(&app, &tiles),
        Some(ImageFilterMode::Nearest),
        "the image's own choice outlives the project's"
    );

    set_image(&mut app, "art/tiles.png", "project").assert_finished();
    assert_eq!(filter(&app, &tiles), None);
    assert!(
        !meta_path(&image_file(dir.path(), "tiles")).exists(),
        "a meta that only held the override goes with it"
    );
}

#[test]
fn project_keeps_an_import_meta() {
    let dir = project(None);
    let meta = meta_path(&image_file(dir.path(), "wall"));
    std::fs::write(
        &meta,
        jackdaw_runtime::texture_import::texture_import_meta(true),
    )
    .expect("an import meta");
    let mut app = open_project(dir.path());

    set_image(&mut app, "art/wall.png", "nearest").assert_finished();
    set_image(&mut app, "art/wall.png", "project").assert_finished();

    let held = std::fs::read(&meta).expect("the import stays");
    let loader = jackdaw_runtime::texture_import::read_texture_import_meta(&held)
        .expect("still an import meta");
    assert_eq!(loader.sampler, ImageSampler::Default);
}

#[test]
fn a_hand_written_meta_is_left_alone_and_wins() {
    let dir = project(Some("nearest"));
    let meta = meta_path(&image_file(dir.path(), "tiles"));
    let written = r#"(
    meta_format_version: "1.0",
    asset: Load(
        loader: "bevy_image::image_loader::ImageLoader",
        settings: (
            format: FromExtension,
            is_srgb: true,
            sampler: Descriptor((
                address_mode_u: ClampToEdge,
                address_mode_v: ClampToEdge,
                address_mode_w: ClampToEdge,
                mag_filter: Linear,
                min_filter: Linear,
                mipmap_filter: Linear,
                lod_min_clamp: 0.0,
                lod_max_clamp: 32.0,
                anisotropy_clamp: 4,
            )),
            asset_usage: ("MAIN_WORLD | RENDER_WORLD"),
        ),
    ),
)"#;
    std::fs::write(&meta, written).expect("a hand-written meta");
    let mut app = open_project(dir.path());
    let tiles = loaded_sprite(&mut app, "art/tiles.png");
    set_project(&mut app, "nearest");

    assert_eq!(std::fs::read_to_string(&meta).expect("the meta"), written);
    assert_eq!(filter(&app, &tiles), Some(ImageFilterMode::Linear));
}

#[test]
fn the_sampling_operators_change_project_files_not_history() {
    let dir = project(None);
    let mut app = open_project(dir.path());
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    set_project(&mut app, "nearest");
    set_image(&mut app, "art/hero.png", "linear").assert_finished();

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before,
        "project files are not scene edits an undo would revert"
    );
}

#[test]
fn the_image_operator_refuses_bad_modes_and_paths() {
    let dir = project(None);
    let outside = dir.path().join("outside.png");
    std::fs::write(&outside, PNG).expect("an image outside the assets");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, image_file(dir.path(), "link")).expect("a symlink");
    let mut app = open_project(dir.path());
    let absolute: &'static str = outside.to_string_lossy().into_owned().leak();
    for (path, mode) in [
        ("art/tiles.png", "blurry"),
        ("art/missing.png", "nearest"),
        ("../outside.png", "nearest"),
        ("art/../../outside.png", "nearest"),
        ("art/link.png", "nearest"),
        ("://art/tiles.png", "nearest"),
        ("art/tiles.png#", "nearest"),
        ("art/tiles.png#part", "nearest"),
        (absolute, "nearest"),
    ] {
        set_image(&mut app, path, mode).assert_cancelled();
    }
    assert!(!meta_path(&outside).exists());
    assert!(metas(dir.path()).is_empty(), "{:?}", metas(dir.path()));
}

#[test]
fn an_image_that_failed_and_then_loads_is_sampled() {
    let dir = project(Some("nearest"));
    let file = image_file(dir.path(), "broken");
    std::fs::write(&file, b"not a png").expect("a broken image");
    let mut app = open_project(dir.path());
    let broken = app
        .world()
        .resource::<AssetServer>()
        .load::<Image>("art/broken.png");
    app.world_mut().spawn(Sprite::from_image(broken.clone()));
    util::update_until(
        &mut app,
        "the broken image to fail",
        Duration::from_secs(10),
        |app| {
            app.world()
                .resource::<AssetServer>()
                .load_state(&broken)
                .is_failed()
        },
    );

    std::fs::write(&file, PNG).expect("the image fixed");
    app.world()
        .resource::<AssetServer>()
        .reload("art/broken.png");
    util::update_until(
        &mut app,
        "the fixed image",
        Duration::from_secs(10),
        |app| filter(app, &broken) == Some(ImageFilterMode::Nearest),
    );
}

#[test]
fn a_reloaded_image_takes_the_project_sampling_again() {
    let dir = project(Some("nearest"));
    let mut app = open_project(dir.path());
    let tiles = loaded_sprite(&mut app, "art/tiles.png");
    set_image(&mut app, "art/tiles.png", "linear").assert_finished();
    set_image(&mut app, "art/tiles.png", "project").assert_finished();
    app.update();
    assert_eq!(filter(&app, &tiles), Some(ImageFilterMode::Nearest));

    // What the file watcher's reload hands back once the override is gone.
    let mut images = app.world_mut().resource_mut::<Assets<Image>>();
    images.get_mut(&tiles).expect("loaded").sampler = ImageSampler::Default;
    app.update();
    app.update();

    assert_eq!(filter(&app, &tiles), Some(ImageFilterMode::Nearest));
}

#[test]
fn an_image_held_only_on_the_gpu_is_reloaded_with_the_new_sampling() {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageLoaderSettings;

    let dir = project(None);
    let mut app = open_project(dir.path());
    let hero = app
        .world()
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.asset_usage = RenderAssetUsages::RENDER_WORLD;
        })
        .load::<Image>("art/hero.png");
    app.world_mut().spawn(Sprite::from_image(hero.clone()));
    util::update_until(
        &mut app,
        "the image to load",
        Duration::from_secs(10),
        |app| app.world().resource::<Assets<Image>>().contains(&hero),
    );
    app.update();
    // What the render world leaves behind once it has taken the pixels.
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut_untracked(&hero)
        .expect("loaded")
        .data = None;

    set_project(&mut app, "nearest");
    util::update_until(
        &mut app,
        "the reloaded image",
        Duration::from_secs(10),
        |app| filter(app, &hero) == Some(ImageFilterMode::Nearest),
    );
    let image = app
        .world()
        .resource::<Assets<Image>>()
        .get(&hero)
        .expect("loaded");
    assert!(
        image.data.is_some(),
        "the sampler came in with fresh pixels"
    );
}
