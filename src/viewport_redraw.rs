//! Redrawing the editor only when something changes, while the viewport's
//! Realtime setting is off.
//!
//! Realtime on, the window updates every frame. Off, it updates on input and
//! otherwise once every [`IDLE_REDRAW`], and asks for frames while anything
//! is still moving on its own: a fly, dolly or other move of the camera, a
//! modal tool, a long task, assets or meshes arriving, or the short settle
//! after the last input.
//! Shaders that animate with time stand still between those frames.

use std::time::Duration;

use bevy::prelude::*;
use bevy::window::RequestRedraw;
use bevy::winit::{UpdateMode, WinitSettings};
use jackdaw_api_internal::lifecycle::ActiveModalOperator;

use crate::viewport::CameraFlyActive;
use crate::viewport_settings::ViewportSettings;

/// The longest the window waits for an event before updating anyway, with
/// Realtime off.
pub const IDLE_REDRAW: Duration = Duration::from_secs(1);

/// How long frames keep coming after the last input, so eased camera moves
/// and UI that settles over a few frames finish drawing.
pub const INPUT_SETTLE: Duration = Duration::from_millis(500);

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<LastActivity>()
        .add_systems(
            PreUpdate,
            follow_realtime_setting.run_if(resource_changed::<ViewportSettings>),
        )
        .add_systems(Last, keep_drawing_while_busy)
        .add_systems(
            Update,
            crate::viewport_settings_menu::show_realtime_off_indicator,
        );
}

/// The update mode the window runs in for a Realtime setting.
pub fn update_mode(realtime: bool) -> UpdateMode {
    if realtime {
        UpdateMode::Continuous
    } else {
        UpdateMode::reactive(IDLE_REDRAW)
    }
}

fn follow_realtime_setting(settings: Res<ViewportSettings>, winit: Option<ResMut<WinitSettings>>) {
    let Some(mut winit) = winit else {
        return;
    };
    let mode = update_mode(settings.realtime);
    if winit.focused_mode != mode || winit.unfocused_mode != mode {
        winit.focused_mode = mode;
        winit.unfocused_mode = mode;
    }
}

/// When the editor last saw input or work arriving.
#[derive(Resource, Default)]
struct LastActivity(Option<Duration>);

/// Whatever is still moving on its own and needs another frame.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Busy<'w, 's> {
    fly: Option<Res<'w, CameraFlyActive>>,
    modal: Query<'w, 's, (), With<ActiveModalOperator>>,
    progress: Option<Res<'w, crate::progress::EditorProgress>>,
    dolly: Option<Res<'w, crate::view_ops::DollyInFlight>>,
    moved: Query<
        'w,
        's,
        (),
        (
            With<crate::viewport::MainViewportCamera>,
            Changed<GlobalTransform>,
        ),
    >,
    spawned: Query<'w, 's, (), Added<Mesh3d>>,
}

impl Busy<'_, '_> {
    fn any(&self) -> bool {
        self.fly.as_ref().is_some_and(|fly| fly.0)
            || !self.modal.is_empty()
            || self
                .progress
                .as_ref()
                .is_some_and(|progress| progress.current().is_some())
            || self.dolly.is_some()
            || !self.moved.is_empty()
            || !self.spawned.is_empty()
    }
}

fn keep_drawing_while_busy(
    settings: Res<ViewportSettings>,
    time: Res<Time<Real>>,
    mut last: ResMut<LastActivity>,
    busy: Busy,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut cursor: MessageReader<CursorMoved>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut images: MessageReader<AssetEvent<Image>>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let input = keys.get_pressed().next().is_some()
        || buttons.get_pressed().next().is_some()
        || cursor.read().count() > 0
        || wheel.read().count() > 0;
    let arriving = images.read().count() > 0 || meshes.read().count() > 0;
    let now = time.elapsed();
    if input || arriving || busy.any() {
        last.0 = Some(now);
    }
    if settings.realtime {
        return;
    }
    if last
        .0
        .is_some_and(|at| now.saturating_sub(at) < INPUT_SETTLE)
    {
        redraw.write(RequestRedraw);
    }
}
