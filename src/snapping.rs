use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
};
use bevy_infinite_grid::{InfiniteGrid, InfiniteGridSettings};
use jackdaw_api::op::{Operator, OperatorCommandsExt as _};

use crate::default_style;

pub struct SnappingPlugin;

impl Plugin for SnappingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SnapSettings>()
            .init_resource::<GridSettings>()
            .add_systems(
                Update,
                handle_grid_size_scroll.in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                Update,
                sync_grid_settings
                    .after(handle_grid_size_scroll)
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

#[derive(Resource)]
pub struct GridSettings {
    pub visible: bool,
    pub scale: f32,
    pub major_line_color: Color,
    pub minor_line_color: Color,
    pub x_axis_color: Color,
    pub z_axis_color: Color,
    pub fadeout_distance: f32,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            visible: true,
            scale: 4.0,
            major_line_color: default_style::GRID_MAJOR_LINE,
            minor_line_color: default_style::GRID_MINOR_LINE,
            x_axis_color: default_style::AXIS_X,
            z_axis_color: default_style::AXIS_Z,
            fadeout_distance: 100.0,
        }
    }
}

fn sync_grid_settings(
    snap: Res<SnapSettings>,
    mut grid: ResMut<GridSettings>,
    mut grids: Query<(&mut InfiniteGridSettings, &mut Visibility), With<InfiniteGrid>>,
) {
    // Sync grid scale from snap settings whenever snap changes.
    // InfiniteGrid scale is lines-per-unit (density), so use the reciprocal of cell size.
    if snap.is_changed() {
        grid.scale = 1.0 / snap.grid_size();
    }
    if !grid.is_changed() {
        return;
    }
    for (mut settings, mut visibility) in &mut grids {
        settings.scale = grid.scale;
        settings.major_line_color = grid.major_line_color;
        settings.minor_line_color = grid.minor_line_color;
        settings.x_axis_color = grid.x_axis_color;
        settings.z_axis_color = grid.z_axis_color;
        settings.fadeout_distance = grid.fadeout_distance;
        *visibility = if grid.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

pub use jackdaw_snap::{GRID_POWER_MAX, GRID_POWER_MIN};

/// Bevy resource wrapper over the engine-agnostic [`jackdaw_snap::SnapSettings`].
/// Derefs to the inner value, so reads, the `snap_*` methods, and field access
/// resolve through `Deref`/`DerefMut` unchanged. The snapping math lives in
/// `jackdaw_snap`; this newtype is the editor's adapter.
#[derive(Resource, Clone, PartialEq, Default, Deref, DerefMut)]
pub struct SnapSettings(pub jackdaw_snap::SnapSettings);

/// Scroll-wheel grid size control. Continuous-input, so it stays as a
/// system rather than an operator. The actual power bump is delegated
/// to [`crate::grid_ops::GridIncreaseOp`] /
/// [`crate::grid_ops::GridDecreaseOp`] (also bound to the bracket
/// keys) so the clamp + translate-increment refresh live in one place.
///
/// Raw wheel read kept: grid-size stepping is gated behind a held modifier
/// chord and predates the keymap engine; migrates with the binding-layer
/// follow-up.
fn handle_grid_size_scroll(
    keyboard: Res<ButtonInput<KeyCode>>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    terrain_edit_mode: Res<crate::terrain::TerrainEditMode>,
    mut scroll_events: MessageReader<MouseWheel>,
    mut commands: Commands,
) {
    if keybind_focus.is_typing() || modal.active.is_some() {
        return;
    }

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    // Shift+Scroll is used for brush resize when terrain sculpt is active;
    // only allow grid resize via Shift+Scroll when NOT sculpting.
    let shift_grid = shift
        && !matches!(
            *terrain_edit_mode,
            crate::terrain::TerrainEditMode::Sculpt(_)
        );

    if !((ctrl && alt) || shift_grid) {
        return;
    }

    for event in scroll_events.read() {
        let delta = match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y * 0.01,
        };
        if delta > 0.0 {
            commands
                .operator(crate::grid_ops::GridIncreaseOp::ID)
                .call();
        } else if delta < 0.0 {
            commands
                .operator(crate::grid_ops::GridDecreaseOp::ID)
                .call();
        }
    }
}
