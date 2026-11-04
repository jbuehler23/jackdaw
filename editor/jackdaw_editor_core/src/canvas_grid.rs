//! The 2D world view's grid lines: one `InfiniteGrid` per 2D panel, on that
//! panel camera's own render layer, its minor lines on the panel's snap grid.
//! Shown while the panel shows a 2D world scene and the grid overlay is on.

use bevy::{
    camera::visibility::RenderLayers,
    dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridSettings},
    picking::Pickable,
    prelude::*,
    ui::UiTargetCamera,
};

use crate::default_style;
use crate::snapping::GridSettings;
use crate::sprite_stage::{Scene2dRoots, shows_2d_world};
use crate::viewport_2d::{Viewport2dCamera, Viewport2dPanelHost};
use crate::viewport_overlays::OverlaySettings;

/// How far in front of the panel camera's far plane the grid sits. More than
/// the depth patch's `GRID_DEPTH_YIELD_WORLD`, which pushes the grid's depth
/// that far back, and less than the gap to a y-sorted sprite at z 0.
pub const CANVAS_GRID_MARGIN: f32 = 0.1;

/// Minor lines between two major lines. A power of two, so a major line falls
/// on every step of the power-of-two grid ladder.
pub const CANVAS_GRID_MAJOR_INTERVAL: u32 = 8;

/// Closest minor lines are drawn, in physical pixels. Closer than this they
/// step up to the major spacing, as the shader would otherwise fill the view.
pub const CANVAS_GRID_MIN_SPACING: f32 = 4.0;

/// The grid drawn into one 2D panel, named by the panel's content entity.
#[derive(Component)]
pub struct CanvasGrid {
    pub host: Entity,
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        sync_canvas_grids
            .after(crate::viewport_2d::route_ui_roots_to_cameras)
            .before(bevy::transform::TransformSystems::Propagate),
    );
}

/// Spawn the hidden grid for the 2D panel `host`, drawn on `layer` only.
pub(crate) fn spawn_canvas_grid(world: &mut World, host: Entity, layer: usize) -> Entity {
    let base = world
        .get_resource::<GridSettings>()
        .map(|grid| grid.0)
        .unwrap_or_default();
    world
        .spawn((
            CanvasGrid { host },
            crate::EditorEntity,
            InfiniteGrid,
            canvas_grid_settings(base, 1.0, 1.0),
            Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            Visibility::Hidden,
            Pickable::IGNORE,
            RenderLayers::layer(layer),
        ))
        .id()
}

/// Grid density, in lines per world unit, for a `grid` world units apart seen
/// at `pixels_per_unit` physical pixels per world unit.
pub fn canvas_grid_scale(grid: f32, pixels_per_unit: f32) -> f32 {
    let mut spacing = if grid.is_finite() && grid > 0.0 {
        grid
    } else {
        1.0
    };
    if pixels_per_unit.is_finite() && pixels_per_unit > 0.0 {
        for _ in 0..8 {
            if spacing * pixels_per_unit >= CANVAS_GRID_MIN_SPACING {
                break;
            }
            spacing *= CANVAS_GRID_MAJOR_INTERVAL as f32;
        }
    }
    1.0 / spacing
}

/// The editor grid's appearance laid flat for a 2D panel whose camera sees
/// `far` world units deep. The shader fades by `distance / fadeout_distance`
/// and by `angle / dot_fadeout_strength`; under a 2D camera the plane is a
/// constant `far` away and faces the view, so a fadeout distance far past it
/// and a tiny but finite angle strength leave every line at full alpha.
pub fn canvas_grid_settings(
    base: InfiniteGridSettings,
    scale: f32,
    far: f32,
) -> InfiniteGridSettings {
    InfiniteGridSettings {
        x_axis_color: default_style::AXIS_X,
        z_axis_color: default_style::AXIS_Y,
        scale,
        major_line_interval: CANVAS_GRID_MAJOR_INTERVAL,
        fadeout_distance: far.abs().max(1.0) * 1000.0,
        dot_fadeout_strength: 1e-3,
        ..base
    }
}

/// Show each panel's grid while it shows a 2D world and the grid overlay is
/// on, just inside its camera's far plane and spaced by its snap grid.
fn sync_canvas_grids(
    overlays: Res<OverlaySettings>,
    appearance: Res<GridSettings>,
    hosts: Query<&Viewport2dPanelHost>,
    cameras: Query<(&Transform, &Projection), (With<Viewport2dCamera>, Without<CanvasGrid>)>,
    stages: Query<&ComputedNode>,
    ui_roots: Query<&UiTargetCamera, crate::prefab::AuthoredUiSceneRoot>,
    scene_roots: Scene2dRoots,
    mut grids: Query<
        (&mut InfiniteGridSettings, &mut Transform, &mut Visibility),
        With<CanvasGrid>,
    >,
) {
    for host in &hosts {
        let Ok((mut settings, mut transform, mut visibility)) = grids.get_mut(host.grid) else {
            continue;
        };
        let shown = overlays.show_grid && shows_2d_world(host, ui_roots.iter(), &scene_roots);
        let wanted = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
        let Ok((camera, Projection::Orthographic(ortho))) = cameras.get(host.camera) else {
            continue;
        };
        if !shown {
            continue;
        }
        let z = camera.translation.z - ortho.far + CANVAS_GRID_MARGIN;
        if transform.translation.z != z {
            transform.translation.z = z;
        }
        let scale_factor = stages
            .get(host.stage)
            .map(|node| node.inverse_scale_factor().recip())
            .ok()
            .filter(|factor| factor.is_finite() && *factor > 0.0)
            .unwrap_or(1.0);
        let scale = canvas_grid_scale(host.view.grid, host.view.zoom * scale_factor);
        let next = canvas_grid_settings(appearance.0, scale, ortho.far);
        if appearance.is_changed()
            || settings.scale != next.scale
            || settings.fadeout_distance != next.fadeout_distance
        {
            *settings = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_lines_follow_the_grid_until_they_crowd() {
        assert_eq!(canvas_grid_scale(8.0, 2.0), 1.0 / 8.0);
        assert_eq!(canvas_grid_scale(8.0, 0.5), 1.0 / 8.0);
        assert_eq!(canvas_grid_scale(8.0, 0.25), 1.0 / 64.0);
        assert_eq!(canvas_grid_scale(1.0, 0.01), 1.0 / 512.0);
    }

    #[test]
    fn a_degenerate_view_still_draws_a_grid() {
        assert_eq!(canvas_grid_scale(0.0, 8.0), 1.0);
        assert_eq!(canvas_grid_scale(8.0, f32::NAN), 1.0 / 8.0);
    }

    #[test]
    fn the_margin_clears_the_depth_patch_yield() {
        let source = include_str!("editor_grid_depth_patch.wesl");
        let yield_world: f32 = source
            .lines()
            .find_map(|line| line.strip_prefix("const GRID_DEPTH_YIELD_WORLD: f32 = "))
            .and_then(|rest| rest.trim_end_matches(';').trim().parse().ok())
            .expect("the patched shader declares its depth yield");
        assert!(CANVAS_GRID_MARGIN > yield_world * 2.0);
        const { assert!(CANVAS_GRID_MARGIN < 1.0 - jackdaw_scene_types::Y_SORT_SPAN) };
    }

    /// The shader's fade factors, as `infinite_grid.wesl` computes them for a
    /// plane `far` away that faces the camera.
    #[test]
    fn the_fade_leaves_a_facing_plane_at_full_alpha() {
        let far = 1000.0;
        let settings = canvas_grid_settings(InfiniteGridSettings::default(), 0.125, far);
        let one_over_dot = 1.0 / settings.dot_fadeout_strength;
        assert!(one_over_dot.is_finite());
        let dist_fadeout = (1.0 - far / settings.fadeout_distance).min(1.0);
        assert!(dist_fadeout > 0.99, "no vignette: {dist_fadeout}");
        assert_eq!((one_over_dot * 1.0_f32).min(1.0), 1.0);
        assert_eq!(settings.major_line_interval, CANVAS_GRID_MAJOR_INTERVAL);
        assert_eq!(
            settings.minor_line_color,
            InfiniteGridSettings::default().minor_line_color
        );
    }
}
