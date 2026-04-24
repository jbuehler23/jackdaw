use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::InputFocus,
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

pub const GRID_POWER_MIN: i32 = -5;
pub const GRID_POWER_MAX: i32 = 8;

#[derive(Resource)]
pub struct SnapSettings {
    pub translate_snap: bool,
    pub translate_increment: f32,
    pub rotate_snap: bool,
    pub rotate_increment: f32,
    pub scale_snap: bool,
    pub scale_increment: f32,
    /// Exponential grid power. Actual grid size = `2^grid_power`.
    pub grid_power: i32,
}

impl Default for SnapSettings {
    fn default() -> Self {
        let grid_power = -2;
        Self {
            translate_snap: true,
            translate_increment: 2.0_f32.powi(grid_power),
            rotate_snap: true,
            rotate_increment: 15.0_f32.to_radians(),
            scale_snap: true,
            scale_increment: 0.1,
            grid_power,
        }
    }
}

impl SnapSettings {
    /// Actual grid size derived from `grid_power`: `2^grid_power`.
    pub fn grid_size(&self) -> f32 {
        2.0_f32.powi(self.grid_power)
    }

    /// Snap a translation value to the nearest increment.
    pub fn snap_translate(&self, value: f32) -> f32 {
        if self.translate_snap && self.translate_increment > 0.0 {
            (value / self.translate_increment).round() * self.translate_increment
        } else {
            value
        }
    }

    /// Snap a translation vector.
    pub fn snap_translate_vec3(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.snap_translate(v.x),
            self.snap_translate(v.y),
            self.snap_translate(v.z),
        )
    }

    /// Snap a rotation angle to the nearest increment.
    pub fn snap_rotate(&self, angle: f32) -> f32 {
        if self.rotate_snap && self.rotate_increment > 0.0 {
            (angle / self.rotate_increment).round() * self.rotate_increment
        } else {
            angle
        }
    }

    /// Snap a scale value to the nearest increment.
    pub fn snap_scale(&self, value: f32) -> f32 {
        if self.scale_snap && self.scale_increment > 0.0 {
            (value / self.scale_increment).round() * self.scale_increment
        } else {
            value
        }
    }

    /// Snap a scale vector.
    pub fn snap_scale_vec3(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.snap_scale(v.x),
            self.snap_scale(v.y),
            self.snap_scale(v.z),
        )
    }

    /// Check if translate snapping should be active (Ctrl held = toggle snap).
    pub fn translate_active(&self, ctrl_held: bool) -> bool {
        self.translate_snap ^ ctrl_held
    }

    /// Check if rotate snapping should be active (Ctrl held = toggle snap).
    pub fn rotate_active(&self, ctrl_held: bool) -> bool {
        self.rotate_snap ^ ctrl_held
    }

    /// Check if scale snapping should be active (Ctrl held = toggle snap).
    pub fn scale_active(&self, ctrl_held: bool) -> bool {
        self.scale_snap ^ ctrl_held
    }

    /// Conditionally snap a translation vector based on Ctrl state.
    pub fn snap_translate_vec3_if(&self, v: Vec3, ctrl_held: bool) -> Vec3 {
        if self.translate_active(ctrl_held) && self.translate_increment > 0.0 {
            Vec3::new(
                (v.x / self.translate_increment).round() * self.translate_increment,
                (v.y / self.translate_increment).round() * self.translate_increment,
                (v.z / self.translate_increment).round() * self.translate_increment,
            )
        } else {
            v
        }
    }

    /// Conditionally snap a rotation angle based on Ctrl state.
    pub fn snap_rotate_if(&self, angle: f32, ctrl_held: bool) -> f32 {
        if self.rotate_active(ctrl_held) && self.rotate_increment > 0.0 {
            (angle / self.rotate_increment).round() * self.rotate_increment
        } else {
            angle
        }
    }

    /// Conditionally snap a scale vector based on Ctrl state.
    pub fn snap_scale_vec3_if(&self, v: Vec3, ctrl_held: bool) -> Vec3 {
        if self.scale_active(ctrl_held) && self.scale_increment > 0.0 {
            Vec3::new(
                (v.x / self.scale_increment).round() * self.scale_increment,
                (v.y / self.scale_increment).round() * self.scale_increment,
                (v.z / self.scale_increment).round() * self.scale_increment,
            )
        } else {
            v
        }
    }
}

/// Scroll-wheel grid size control. Continuous-input, so it stays as a
/// system rather than an operator. The actual power bump is delegated
/// to [`crate::grid_ops::GridIncreaseOp`] /
/// [`crate::grid_ops::GridDecreaseOp`] (also bound to the bracket
/// keys) so the clamp + translate-increment refresh live in one place.
fn handle_grid_size_scroll(
    keyboard: Res<ButtonInput<KeyCode>>,
    input_focus: Res<InputFocus>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    terrain_edit_mode: Res<crate::terrain::TerrainEditMode>,
    mut scroll_events: MessageReader<MouseWheel>,
    mut commands: Commands,
) {
    if input_focus.0.is_some() || modal.active.is_some() {
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
