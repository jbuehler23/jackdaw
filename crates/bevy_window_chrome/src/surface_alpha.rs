//! Opaque fallback for adapters whose window surface cannot composite alpha.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::render::renderer::{RenderAdapter, RenderInstance};
use bevy::render::view::window::{ExtractedWindows, create_surfaces};
use bevy::render::{Render, RenderApp};
use bevy::window::CompositeAlphaMode;

use crate::{WindowShellRoot, WindowTitleBarRoot};

/// Whether the primary window surface composites with alpha, shared with the render world.
#[derive(Resource, Clone)]
struct SurfaceAlpha(Arc<AtomicBool>);

pub(crate) fn build(app: &mut App) {
    let alpha = SurfaceAlpha(Arc::new(AtomicBool::new(
        crate::window::surface_supports_alpha(),
    )));
    app.insert_resource(alpha.clone()).add_systems(
        PostUpdate,
        square_corners_on_opaque_surface.before(bevy::ui::UiSystems::Layout),
    );
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(alpha).add_systems(
            Render,
            fall_back_to_supported_alpha_mode.before(create_surfaces),
        );
    }
}

/// Keeps `requested` when the surface supports it, otherwise lets wgpu pick an opaque mode.
fn resolve_alpha_mode(
    requested: CompositeAlphaMode,
    supported: &[wgpu::CompositeAlphaMode],
) -> CompositeAlphaMode {
    let wanted = match requested {
        CompositeAlphaMode::PreMultiplied => wgpu::CompositeAlphaMode::PreMultiplied,
        CompositeAlphaMode::PostMultiplied => wgpu::CompositeAlphaMode::PostMultiplied,
        other => return other,
    };
    if supported.contains(&wanted) {
        requested
    } else {
        CompositeAlphaMode::Auto
    }
}

/// Probes each new window's surface before Bevy configures it, so an adapter
/// without alpha compositing gets an opaque surface instead of a validation error.
fn fall_back_to_supported_alpha_mode(
    mut windows: ResMut<ExtractedWindows>,
    instance: Res<RenderInstance>,
    adapter: Res<RenderAdapter>,
    alpha: Res<SurfaceAlpha>,
    mut probed: Local<EntityHashSet>,
) {
    let primary = windows.primary;
    for window in windows.windows.values_mut() {
        if !probed.insert(window.entity)
            || !matches!(
                window.alpha_mode,
                CompositeAlphaMode::PreMultiplied | CompositeAlphaMode::PostMultiplied
            )
        {
            continue;
        }
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(window.handle.get_display_handle()),
            raw_window_handle: window.handle.get_window_handle(),
        };
        // SAFETY: extracted window handles stay valid while the window exists,
        // and the probe surface is dropped before this system returns.
        let Ok(surface) = (unsafe { instance.create_surface_unsafe(target) }) else {
            continue;
        };
        let supported = surface.get_capabilities(&adapter).alpha_modes;
        let resolved = resolve_alpha_mode(window.alpha_mode, &supported);
        if resolved == window.alpha_mode {
            continue;
        }
        warn!(
            "Window surface does not support {:?} alpha (supports {supported:?}), using an opaque window",
            window.alpha_mode
        );
        window.alpha_mode = resolved;
        if primary == Some(window.entity) {
            alpha.0.store(false, Ordering::Relaxed);
        }
    }
}

fn square_corners_on_opaque_surface(
    alpha: Res<SurfaceAlpha>,
    mut roots: Query<&mut Node, Or<(With<WindowShellRoot>, With<WindowTitleBarRoot>)>>,
) {
    if alpha.0.load(Ordering::Relaxed) {
        return;
    }
    for mut node in &mut roots {
        if node.border_radius != BorderRadius::ZERO {
            node.border_radius = BorderRadius::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_premultiplied_when_supported() {
        let supported = [
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ];
        assert_eq!(
            resolve_alpha_mode(CompositeAlphaMode::PreMultiplied, &supported),
            CompositeAlphaMode::PreMultiplied
        );
    }

    #[test]
    fn falls_back_to_auto_when_only_opaque_is_supported() {
        let supported = [wgpu::CompositeAlphaMode::Opaque];
        assert_eq!(
            resolve_alpha_mode(CompositeAlphaMode::PreMultiplied, &supported),
            CompositeAlphaMode::Auto
        );
        assert_eq!(
            resolve_alpha_mode(CompositeAlphaMode::PostMultiplied, &supported),
            CompositeAlphaMode::Auto
        );
    }

    #[test]
    fn leaves_non_alpha_requests_alone() {
        assert_eq!(
            resolve_alpha_mode(CompositeAlphaMode::Opaque, &[]),
            CompositeAlphaMode::Opaque
        );
        assert_eq!(
            resolve_alpha_mode(CompositeAlphaMode::Auto, &[]),
            CompositeAlphaMode::Auto
        );
    }
}
