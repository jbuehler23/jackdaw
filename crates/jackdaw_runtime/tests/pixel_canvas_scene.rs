use std::path::PathBuf;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot, PixelCanvasExpanded};
use jackdaw_scene_types::{CanvasScale, PixelCanvas};

const SCENE: &str = "
#Scene2d
jackdaw_scene_types::Scene2dRoot
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #\"Main Camera\"
    bevy_camera::components::Camera2d
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::pixel_canvas::PixelCanvas {
        resolution: glam::UVec2 { x: 320, y: 180 },
        scale: jackdaw_scene_types::pixel_canvas::CanvasScale::Fit,
        letterbox: bevy_color::color::Color::Srgba(bevy_color::srgba::Srgba { red: 0.25, green: 0.5, blue: 0.75, alpha: 1.0 }),
        pixel_snap: false,
    }
]
";

#[test]
fn a_loaded_scene_camera_keeps_its_canvas_and_draws_through_it() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::world_serialization::WorldSerializationPlugin,
        JackdawPlugin,
    ));
    let handle =
        app.world_mut()
            .resource_mut::<Assets<JackdawScene>>()
            .add(JackdawScene::with_stem(
                SCENE.to_string(),
                PathBuf::new(),
                Some("level".to_string()),
            ));
    app.world_mut().spawn(JackdawSceneRoot(handle));
    for _ in 0..3 {
        app.update();
    }

    let (camera, canvas) = app
        .world_mut()
        .query::<(Entity, &PixelCanvas)>()
        .single(app.world())
        .expect("one canvas camera");
    assert_eq!(
        *canvas,
        PixelCanvas {
            resolution: UVec2::new(320, 180),
            scale: CanvasScale::Fit,
            letterbox: Color::srgb(0.25, 0.5, 0.75),
            pixel_snap: false,
        }
    );
    let expanded = app
        .world()
        .get::<PixelCanvasExpanded>(camera)
        .expect("the runtime expands it");
    assert!(matches!(
        app.world().get::<RenderTarget>(camera),
        Some(RenderTarget::Image(target)) if target.handle == *expanded.image()
    ));
}
