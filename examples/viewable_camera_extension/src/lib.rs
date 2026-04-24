//! Viewable Camera extension.
//!
//! - **F6**: place a viewable camera at the editor camera position.
//!   The dispatcher captures the scene diff automatically, so one
//!   Ctrl+Z despawns the camera.
//! - **F7**: toggle between the editor view and looking through the
//!   viewable camera. Preview mutates Bevy resources and components
//!   that aren't in the scene AST, so the dispatcher's diff is empty
//!   and no history entry is pushed.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use jackdaw_api::prelude::*;

#[derive(Default)]
pub struct ViewableCameraExtension;

impl JackdawExtension for ViewableCameraExtension {
    fn id(&self) -> String {
        "viewable_camera".to_string()
    }

    fn label(&self) -> String {
        "Viewable Camera".to_string()
    }

    fn register_input_context(&self, app: &mut App) {
        app.add_input_context::<ViewableCameraContext>();
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.init_resource::<CameraPreviewState>();

        ctx.register_operator::<PlaceViewableCamera>();
        ctx.register_operator::<ToggleCameraPreview>();

        ctx.register_menu_entry(MenuEntryDescriptor {
            menu: "Add".into(),
            label: "Viewable Camera".into(),
            operator_id: PlaceViewableCamera::ID,
        });

        ctx.spawn((
            ViewableCameraContext,
            actions!(ViewableCameraContext[
                (Action::<PlaceViewableCamera>::new(), bindings![KeyCode::F6]),
                (Action::<ToggleCameraPreview>::new(), bindings![KeyCode::F7]),
            ]),
        ));

        // Exit preview if the previewed camera gets despawned (e.g. the
        // user undoes the placement while preview is active), so the
        // viewport falls back to the editor camera.
        ctx.add_observer(
            move |trigger: On<Remove, ViewableCamera>,
                  mut state: ResMut<CameraPreviewState>,
                  mut commands: Commands| {
                if state.active == Some(trigger.event_target()) {
                    state.active = None;
                    commands.queue(|world: &mut World| {
                        restore_editor_camera(world);
                    });
                }
            },
        );
    }
}

/// BEI context for this extension; gives key-binding isolation.
#[derive(Component, Default)]
pub struct ViewableCameraContext;

/// Marker on camera entities created by this extension. Scene data,
/// not editor-local.
#[derive(Component, Default, Reflect)]
pub struct ViewableCamera;

/// Tracks the currently-previewed camera. Not saved to the scene.
#[derive(Resource, Default)]
struct CameraPreviewState {
    active: Option<Entity>,
    /// `(editor_camera, its_render_target)` captured on enter so it can
    /// be restored on exit.
    saved: Option<(Entity, RenderTarget)>,
}

/// Place a viewable camera at the editor camera's current position.
/// Undoable via the dispatcher's automatic snapshot-diff.
#[operator(
    id = "viewable_camera.place",
    label = "Place Viewable Camera",
    description = "Place a camera at the viewport position",
    name = "PlaceViewableCamera"
)]
fn place_viewable_camera(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    // Match the editor camera's transform so "look through" feels
    // natural on the first toggle.
    let spawn_transform = world
        .run_system_cached(find_editor_camera)
        .ok()
        .flatten()
        .and_then(|e| world.get::<Transform>(e).copied())
        .unwrap_or_default();

    world.spawn((
        Name::new("Viewable Camera"),
        ViewableCamera,
        Camera3d::default(),
        Camera {
            // Stays off until `enter_preview` hands over the viewport
            // image target.
            is_active: false,
            order: -1,
            ..default()
        },
        // `Camera`'s required components default `RenderTarget` to the
        // primary window, which would render over the editor UI if
        // `is_active` ever flipped true. Keep the camera inert until
        // `enter_preview` swaps in the viewport image target.
        RenderTarget::None {
            size: UVec2::splat(1),
        },
        spawn_transform,
        Visibility::default(),
    ));

    OperatorResult::Finished
}

/// Toggle "look through the viewable camera" against the editor view.
/// Preview only mutates Bevy resources and components that aren't in
/// the scene AST, so the dispatcher's snapshot-diff is empty and no
/// history entry is pushed.
#[operator(
    id = "viewable_camera.toggle_preview",
    label = "Toggle Camera Preview",
    description = "Look through the selected viewable camera",
    name = "ToggleCameraPreview"
)]
fn toggle_preview(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let currently_active = world.resource::<CameraPreviewState>().active;
    if currently_active.is_some() {
        restore_editor_camera(world);
        info!("Exited viewable-camera preview");
    } else {
        let Ok(Some(target)) = world.run_system_cached(pick_preview_target) else {
            warn!("No viewable camera to preview; press F6 to place one first");
            return OperatorResult::Cancelled;
        };
        match enter_preview(world, target) {
            Ok(()) => info!("Entered preview through viewable camera {target:?}"),
            Err(reason) => warn!("Preview failed: {reason}"),
        }
    }
    OperatorResult::Finished
}

/// Find the active editor-viewport camera: an active `Camera3d` with an
/// `Image` render target that isn't one of ours.
fn find_editor_camera(
    world: &mut World,
    cameras: &mut QueryState<
        (Entity, &Camera, &RenderTarget),
        (With<Camera3d>, Without<ViewableCamera>),
    >,
) -> Option<Entity> {
    for (entity, camera, target) in cameras.iter(world) {
        if camera.is_active && matches!(target, RenderTarget::Image(_)) {
            return Some(entity);
        }
    }
    None
}

/// Pick which viewable camera to preview: the only one if there is
/// exactly one, otherwise the first one found. A richer rule honouring
/// the editor's `Selection` resource would require depending on the
/// main jackdaw crate.
fn pick_preview_target(
    world: &mut World,
    cameras: &mut QueryState<Entity, With<ViewableCamera>>,
) -> Option<Entity> {
    let cams: Vec<Entity> = cameras.iter(world).collect();
    if cams.len() == 1 {
        return Some(cams[0]);
    }
    cams.first().copied()
}

fn enter_preview(world: &mut World, target: Entity) -> Result<(), &'static str> {
    let Ok(Some(editor_cam)) = world.run_system_cached(find_editor_camera) else {
        return Err("couldn't find the editor viewport camera");
    };
    let render_target = world
        .get::<RenderTarget>(editor_cam)
        .cloned()
        .ok_or("editor camera has no RenderTarget")?;

    // Disable the editor camera.
    if let Some(mut c) = world.get_mut::<Camera>(editor_cam) {
        c.is_active = false;
    }

    // Hand its render target to the viewable camera and activate it.
    if let Ok(mut ec) = world.get_entity_mut(target) {
        ec.insert(render_target.clone());
    }
    if let Some(mut c) = world.get_mut::<Camera>(target) {
        c.is_active = true;
    }
    // Bevy's `camera_system` recomputes `target_info` only when
    // Projection is marked changed, viewport size differs, or the
    // underlying image/window changes. Replacing `RenderTarget` via
    // `insert` triggers none of those, so without this the camera
    // renders into the stale 1x1 target from spawn.
    if let Some(mut proj) = world.get_mut::<Projection>(target) {
        proj.set_changed();
    }

    let mut state = world.resource_mut::<CameraPreviewState>();
    state.active = Some(target);
    state.saved = Some((editor_cam, render_target));
    Ok(())
}

/// Hand the render target back to the editor camera. Safe to call when
/// preview is already off.
fn restore_editor_camera(world: &mut World) {
    let Some((editor_cam, _target)) = world.resource_mut::<CameraPreviewState>().saved.take()
    else {
        return;
    };
    let active = world.resource_mut::<CameraPreviewState>().active.take();

    if let Some(preview) = active {
        if let Ok(mut ec) = world.get_entity_mut(preview) {
            // `Camera` requires a `RenderTarget`, so swap to `None`
            // rather than removing the component outright. Leaving the
            // Image target on an inactive camera triggers ambiguity
            // warnings when preview is re-entered.
            ec.insert(RenderTarget::None {
                size: UVec2::splat(1),
            });
        }
        if let Some(mut c) = world.get_mut::<Camera>(preview) {
            c.is_active = false;
        }
        // Mirrors the Projection poke in `enter_preview`. See its
        // comment for why.
        if let Some(mut proj) = world.get_mut::<Projection>(preview) {
            proj.set_changed();
        }
    }

    if let Some(mut c) = world.get_mut::<Camera>(editor_cam) {
        c.is_active = true;
    }
}
