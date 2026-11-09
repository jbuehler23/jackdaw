//! A colour drag in the Components panel during Play previews in the game, and a
//! gesture that ends with no release from the widget still settles.

use std::time::{Duration, Instant};

use bevy::feathers::controls::{ColorChannel, ColorSlider, SliderBaseColor};
use bevy::picking::pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput};
use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::pie::InstanceKey;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::event::from_bytes;
use jackdaw_pie_protocol::{ControlEvent, IpcChannelTransport, PieTransport as _};

use crate::util::{self, OperatorResultExt as _};

const GUARD: Duration = Duration::from_secs(30);

#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default)]
struct Tint {
    color: Color,
}

impl Default for Tint {
    fn default() -> Self {
        Self {
            color: Color::srgba(0.2, 0.2, 0.2, 1.0),
        }
    }
}

struct Playing {
    app: App,
    game: IpcChannelTransport,
    zone: Entity,
    _dir: tempfile::TempDir,
}

impl Drop for Playing {
    fn drop(&mut self) {
        jackdaw::pie::handle_stop(self.app.world_mut());
    }
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

/// An editor with `zone.bsn` open, its Zone holding a [`Tint`] and selected in
/// the Components panel, and a game connected and playing it.
fn playing() -> Playing {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("assets folder");
    std::fs::write(
        dir.path().join("assets/zone.bsn"),
        "#Zone\nbevy_transform::components::transform::Transform\n",
    )
    .expect("scene written");
    let mut app = util::editor_test_app();
    app.register_type::<Tint>();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    run_op_clause(app.world_mut(), "scene.open path=assets/zone.bsn")
        .expect("the scene opens")
        .assert_finished();
    settle(&mut app);
    let zone = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Zone")
        .map(|(entity, _)| entity)
        .expect("the scene's Zone");
    app.world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, zone);
        });
    app.update();
    app.world_mut()
        .operator("component.add")
        .param("entity", zone)
        .param("type_path", Tint::type_path().to_string())
        .call()
        .expect("the operator dispatches")
        .assert_finished();
    settle(&mut app);

    let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
    let accept = std::thread::spawn(move || server.accept());
    let game = jackdaw_pie_protocol::connect(&name).expect("the game connects");
    let editor_end = accept
        .join()
        .expect("accept ran")
        .expect("the editor accepts");
    let child = std::process::Command::new("sleep")
        .arg("600")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("a child process");
    let key = InstanceKey {
        config: "Game".to_string(),
        instance: 1,
    };
    jackdaw::pie::connect_test_game(app.world_mut(), key.clone(), child, editor_end)
        .expect("a link");
    jackdaw::play_scene::note_test_asked_scene(app.world_mut(), &key, "zone.bsn");
    settle(&mut app);
    let mut playing = Playing {
        app,
        game,
        zone,
        _dir: dir,
    };
    playing.game.drain_received();
    playing
}

impl Playing {
    /// The red slider of the [`Tint`] card's colour field, the only one showing
    /// the tint's colour.
    fn red_slider(&mut self) -> Entity {
        let tint = Tint::default().color;
        let world = self.app.world_mut();
        let sliders: Vec<Entity> = world
            .query::<(Entity, &ColorSlider, &SliderBaseColor)>()
            .iter(world)
            .filter(|(_, slider, base)| {
                matches!(slider.channel, ColorChannel::Red) && base.0 == tint
            })
            .map(|(entity, _, _)| entity)
            .collect();
        assert_eq!(sliders.len(), 1, "one red slider for the tint");
        sliders[0]
    }

    fn red(&self) -> f32 {
        self.app
            .world()
            .get::<Tint>(self.zone)
            .expect("the Zone's tint")
            .color
            .to_srgba()
            .red
    }

    fn undo_entries(&self) -> usize {
        self.app
            .world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len()
    }

    /// Run frames until the game receives a [`Tint`], and return its red.
    fn red_in_game(&mut self) -> f64 {
        let until = Instant::now() + GUARD;
        loop {
            assert!(Instant::now() < until, "the tint never reached the game");
            self.app.update();
            for (_, bytes) in self.game.drain_received() {
                let event = from_bytes::<ControlEvent>(&bytes).expect("a control event");
                if let ControlEvent::NodeComponents { set, .. } = event
                    && let Some((_, value)) =
                        set.into_iter().find(|(path, _)| path == Tint::type_path())
                {
                    return value["color"]["Srgba"]["red"]
                        .as_f64()
                        .expect("an sRGBA colour");
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Press or let go of the primary mouse button, as picking sees it.
fn mouse(app: &mut App, pressed: bool) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    let target = bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    let action = if pressed {
        PointerAction::Press(PointerButton::Primary)
    } else {
        PointerAction::Release(PointerButton::Primary)
    };
    app.world_mut().write_message(PointerInput::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        action,
    ));
    app.update();
}

fn tick(playing: &mut Playing, slider: Entity, red: f32) {
    playing.app.world_mut().trigger(ValueChange {
        source: slider,
        value: red,
        is_final: false,
    });
}

/// A drag previews in the game while the pointer is held and authors nothing; a
/// pointer let go with no release from the widget authors the colour once.
#[test]
fn a_colour_drag_during_play_previews_in_the_game_until_the_pointer_lets_go() {
    let mut playing = playing();
    let slider = playing.red_slider();
    let undo_before = playing.undo_entries();

    mouse(&mut playing.app, true);
    tick(&mut playing, slider, 0.7);
    assert!(
        (playing.red_in_game() - 0.7).abs() < 1e-4,
        "the drag previews"
    );
    assert!((playing.red() - 0.7).abs() < 1e-4);
    assert_eq!(
        playing.undo_entries(),
        undo_before,
        "a held drag authors nothing"
    );

    mouse(&mut playing.app, false);
    playing.app.update();
    assert_eq!(
        playing.undo_entries(),
        undo_before + 1,
        "the let-go authors once"
    );
    assert!((playing.red_in_game() - 0.7).abs() < 1e-4);
    assert!((playing.red() - 0.7).abs() < 1e-4);
}

/// A click on the track moves the colour and authors it, though the widget sends
/// no release for it.
#[test]
fn a_click_on_a_colour_track_authors_the_colour() {
    let mut playing = playing();
    let slider = playing.red_slider();
    let undo_before = playing.undo_entries();

    tick(&mut playing, slider, 0.5);
    playing.app.update();
    playing.app.update();
    assert_eq!(playing.undo_entries(), undo_before + 1);
    assert!((playing.red_in_game() - 0.5).abs() < 1e-4);
    assert!((playing.red() - 0.5).abs() < 1e-4);
}

/// A drag the widget releases itself authors once, with nothing left for the
/// let-go to author again.
#[test]
fn a_released_colour_drag_authors_once() {
    let mut playing = playing();
    let slider = playing.red_slider();
    let undo_before = playing.undo_entries();

    mouse(&mut playing.app, true);
    tick(&mut playing, slider, 0.6);
    playing.app.update();
    playing.app.world_mut().trigger(ValueChange {
        source: slider,
        value: 0.8f32,
        is_final: true,
    });
    mouse(&mut playing.app, false);
    playing.app.update();
    assert_eq!(playing.undo_entries(), undo_before + 1);
    assert!((playing.red() - 0.8).abs() < 1e-4);
}
