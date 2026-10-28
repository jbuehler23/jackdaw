use std::hash::{DefaultHasher, Hash, Hasher};

use bevy::asset::uuid::Uuid;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers, VisibilitySystems};
use bevy::camera::{
    CameraUpdateSystems, NormalizedRenderTarget, RenderTarget, ScalingMode, Viewport,
};
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::ecs::system::SystemParam;
use bevy::image::ImageSampler;
use bevy::picking::pointer::{Location, PointerAction, PointerId, PointerInput, PointerMap};
use bevy::picking::{Pickable, PickingSystems};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::transform::TransformSystems;
use bevy::window::PrimaryWindow;
use jackdaw_scene_types::PixelCanvas;

use crate::{CanvasExpansionSystems, RuntimeOnly};

/// The order an expanded canvas camera draws its canvas at, ahead of every
/// camera that shows one. Its view camera takes over its authored order.
pub const CANVAS_ORDER: isize = isize::MIN / 2;

const FIRST_LAYER: usize = 31;
const LAYER_POOL: usize = 8;
const POINTER_TAG: u64 = 0x6a64_7069_7863_616e;

/// Marks what the runtime adds to show a [`PixelCanvas`]: the view camera,
/// the sprite showing the canvas and the pointers forwarded into it.
#[derive(Component, Clone, Copy, Debug)]
#[require(RuntimeOnly)]
pub struct PixelCanvasHelper {
    /// The camera whose canvas this helps show.
    pub camera: Entity,
}

/// On a camera whose [`PixelCanvas`] the runtime has expanded: what it had
/// before, and the helpers showing its canvas.
#[derive(Component, Debug)]
pub struct PixelCanvasExpanded {
    target: RenderTarget,
    order: isize,
    viewport: Option<Viewport>,
    image: Handle<Image>,
    layer: usize,
    view: Entity,
    display: Entity,
}

impl PixelCanvasExpanded {
    /// The image the camera draws into.
    pub fn image(&self) -> &Handle<Image> {
        &self.image
    }
}

#[derive(Resource, Default)]
struct CanvasLayers(u32);

#[derive(Component)]
struct CapturesView(Entity);

/// Expands every [`PixelCanvas`] on a camera into its low-resolution image,
/// view camera and display, keeps the display fitted to its viewport, snaps
/// the camera to canvas pixels and forwards pointers into the canvas.
pub struct PixelCanvasPlugin;

impl Plugin for PixelCanvasPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CanvasLayers>()
            .add_observer(collapse_on_remove)
            .add_systems(
                PostUpdate,
                (
                    (expand_canvases, sync_views)
                        .chain()
                        .in_set(CanvasExpansionSystems)
                        .before(CameraUpdateSystems),
                    fit_displays
                        .after(CameraUpdateSystems)
                        .before(TransformSystems::Propagate),
                    snap_cameras
                        .after(TransformSystems::Propagate)
                        .before(VisibilitySystems::UpdateFrusta),
                ),
            )
            .add_systems(
                PreUpdate,
                forward_pointers
                    .before(PickingSystems::ProcessInput)
                    .run_if(resource_exists::<PointerMap>),
            );
    }
}

fn view_area(view: &Camera) -> Option<(Rect, f32)> {
    let rect = view.physical_viewport_rect()?;
    let rect = Rect::from_corners(rect.min.as_vec2(), rect.max.as_vec2());
    Some((rect, view.target_scaling_factor()?))
}

fn canvas_image(resolution: UVec2) -> Image {
    let mut image = Image::new_target_texture(
        resolution.x,
        resolution.y,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.sampler = ImageSampler::nearest();
    image
}

fn expand_canvases(
    canvases: Query<
        (Entity, &PixelCanvas, Option<&PixelCanvasExpanded>),
        (With<Camera>, Or<(Changed<PixelCanvas>, Added<Camera>)>),
    >,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) -> Result {
    for (entity, canvas, expanded) in &canvases {
        match expanded {
            None if canvas.is_set() => {
                commands.queue(move |world: &mut World| expand(world, entity));
            }
            Some(expanded) if !canvas.is_set() => collapse(&mut commands, entity, expanded),
            Some(expanded)
                if images
                    .get(&expanded.image)
                    .is_some_and(|image| image.size() != canvas.resolution) =>
            {
                images.insert(expanded.image.id(), canvas_image(canvas.resolution))?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn expand(world: &mut World, entity: Entity) {
    let Ok(camera) = world.get_entity(entity) else {
        return;
    };
    let (Some(canvas), Some(target), Some(order)) = (
        camera.get::<PixelCanvas>().copied(),
        camera.get::<RenderTarget>().cloned(),
        camera.get::<Camera>().map(|camera| camera.order),
    ) else {
        return;
    };
    if !canvas.is_set() || camera.contains::<PixelCanvasExpanded>() {
        return;
    }
    let mut layers = world.resource_mut::<CanvasLayers>();
    let Some(slot) = (0..LAYER_POOL).find(|slot| layers.0 & (1 << slot) == 0) else {
        warn!("pixel canvas: more than {LAYER_POOL} canvases, {entity} draws unscaled");
        return;
    };
    layers.0 |= 1 << slot;
    let layer = FIRST_LAYER - slot;
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(canvas_image(canvas.resolution));
    let helper = PixelCanvasHelper { camera: entity };
    let view = (Name::new("Pixel Canvas View"), Camera2d, target.clone());
    let view = world.spawn((view, RenderLayers::layer(layer), helper)).id();
    let sprite = Sprite {
        image: image.clone(),
        custom_size: Some(canvas.resolution.as_vec2()),
        ..default()
    };
    let display = (Name::new("Pixel Canvas Display"), sprite, NoFrustumCulling);
    let display = world
        .spawn((
            display,
            Pickable::IGNORE,
            RenderLayers::layer(layer),
            helper,
        ))
        .id();
    let expanded = PixelCanvasExpanded {
        target,
        order,
        viewport: None,
        image: image.clone(),
        layer,
        view,
        display,
    };
    world
        .entity_mut(entity)
        .insert((RenderTarget::Image(image.into()), expanded));
}

fn collapse(commands: &mut Commands, entity: Entity, expanded: &PixelCanvasExpanded) {
    let (target, order) = (expanded.target.clone(), expanded.order);
    let (viewport, slot) = (expanded.viewport.clone(), FIRST_LAYER - expanded.layer);
    commands.queue(move |world: &mut World| {
        world.resource_mut::<CanvasLayers>().0 &= !(1 << slot);
        let helpers: Vec<Entity> = world
            .query::<(Entity, &PixelCanvasHelper)>()
            .iter(world)
            .filter(|(_, helper)| helper.camera == entity)
            .map(|(helper, _)| helper)
            .collect();
        for helper in helpers {
            world.despawn(helper);
        }
        let Ok(mut camera) = world.get_entity_mut(entity) else {
            return;
        };
        camera.remove::<PixelCanvasExpanded>().insert(target);
        if let Some(mut camera) = camera.get_mut::<Camera>() {
            camera.order = order;
            camera.viewport = viewport;
        }
        if let Some(mut transform) = camera.get_mut::<Transform>() {
            transform.set_changed();
        }
    });
}

fn collapse_on_remove(
    removed: On<Remove<PixelCanvas>>,
    expansions: Query<&PixelCanvasExpanded>,
    mut commands: Commands,
) -> Result {
    if let Ok(expanded) = expansions.get(removed.entity) {
        collapse(&mut commands, removed.entity, expanded);
    }
    Ok(())
}

fn sync_views(
    mut removed: RemovedComponents<PixelCanvasExpanded>,
    mut canvases: Query<
        (Ref<PixelCanvas>, &mut Camera, &mut PixelCanvasExpanded),
        Without<PixelCanvasHelper>,
    >,
    mut views: Query<(Entity, &mut Camera, &RenderTarget, &PixelCanvasHelper)>,
    primary: Query<Entity, With<PrimaryWindow>>,
) -> Result {
    let removed = removed.read().count() > 0;
    let changed = canvases.iter_mut().any(|(canvas, camera, expanded)| {
        canvas.is_changed() || camera.is_changed() || expanded.is_added()
    });
    if !removed && !changed {
        return Ok(());
    }
    for (_, mut camera, mut expanded) in &mut canvases {
        if camera.order != CANVAS_ORDER {
            expanded.order = camera.order;
            camera.order = CANVAS_ORDER;
        }
        if camera.viewport.is_some() {
            expanded.viewport = camera.viewport.take();
        }
        if let Ok((_, mut view, _, _)) = views.get_mut(expanded.view) {
            view.is_active = camera.is_active;
            view.order = expanded.order;
            view.viewport = expanded.viewport.clone();
        }
    }
    let primary = primary.iter().next();
    let shown: Vec<_> = views
        .iter()
        .filter(|(_, view, ..)| view.is_active)
        .map(|(entity, view, target, _)| (view.order, entity, target.normalize(primary)))
        .collect();
    for (entity, mut view, target, helper) in &mut views {
        let (order, target) = (view.order, target.normalize(primary));
        let clears = !shown.iter().any(|(other_order, other, on)| {
            *on == target && (*other_order, *other) < (order, entity)
        });
        let letterbox = canvases
            .get(helper.camera)
            .map(|(canvas, ..)| canvas.letterbox);
        view.clear_color = match letterbox {
            Ok(letterbox) if clears => ClearColorConfig::Custom(letterbox),
            _ => ClearColorConfig::None,
        };
    }
    Ok(())
}

fn fit_displays(
    views: Query<(&Camera, &PixelCanvasHelper), Changed<Camera>>,
    canvases: Query<(&PixelCanvas, &PixelCanvasExpanded)>,
    mut displays: Query<(&mut Sprite, &mut Transform), With<PixelCanvasHelper>>,
    mut captures: Query<(&CapturesView, &mut Projection)>,
) -> Result {
    for (view, helper) in &views {
        let (Some((area, scale)), Ok((canvas, expanded))) =
            (view_area(view), canvases.get(helper.camera))
        else {
            continue;
        };
        let Ok((mut sprite, mut transform)) = displays.get_mut(expanded.display) else {
            continue;
        };
        let rect = canvas.display_rect(area.size());
        let offset = (rect.center() - area.size() / 2.0) / scale;
        sprite.custom_size = Some(rect.size() / scale);
        transform.translation = Vec3::new(offset.x, -offset.y, 0.0);
        for (capture, mut projection) in &mut captures {
            if capture.0 == expanded.view {
                *projection = capture_projection(area.size() / scale);
            }
        }
    }
    Ok(())
}

fn capture_projection(shown: Vec2) -> Projection {
    Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::AutoMin {
            min_width: shown.x,
            min_height: shown.y,
        },
        ..OrthographicProjection::default_2d()
    })
}

/// Points the frame-capture `camera` at what the view of `rig`'s canvas
/// shows, so captured frames match the game window. False when `rig` has no
/// expanded canvas.
#[cfg_attr(
    not(any(feature = "pie", test)),
    expect(dead_code, reason = "only the windowed frame capture calls it")
)]
pub(crate) fn capture_canvas(world: &mut World, camera: Entity, rig: Entity) -> bool {
    let Some(expanded) = world.get::<PixelCanvasExpanded>(rig) else {
        return false;
    };
    let (layer, view) = (RenderLayers::layer(expanded.layer), expanded.view);
    let shown = world
        .get::<Camera>(view)
        .and_then(view_area)
        .map_or(Vec2::ONE, |(area, scale)| area.size() / scale);
    let clear_color = world
        .get::<PixelCanvas>(rig)
        .map_or(ClearColorConfig::None, |canvas| {
            ClearColorConfig::Custom(canvas.letterbox)
        });
    let mut capture = world.entity_mut(camera);
    capture.insert((
        Camera2d,
        layer,
        CapturesView(view),
        capture_projection(shown),
    ));
    if let Some(mut camera) = capture.get_mut::<Camera>() {
        camera.clear_color = clear_color;
    }
    true
}

fn snap_cameras(
    mut access: ParamSet<(
        Query<
            Entity,
            Or<(
                Changed<GlobalTransform>,
                Changed<PixelCanvas>,
                Changed<Projection>,
            )>,
        >,
        Query<&mut GlobalTransform>,
    )>,
    canvases: Query<(Ref<PixelCanvas>, &Projection), With<Camera>>,
    mut transforms: Query<&mut Transform>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    mut stack: Local<Vec<(Entity, GlobalTransform)>>,
) -> Result {
    let mut cameras: Vec<Entity> = Vec::new();
    for entity in &access.p0() {
        for at in std::iter::once(entity).chain(parents.iter_ancestors(entity)) {
            if canvases.contains(at) && !cameras.contains(&at) {
                cameras.push(at);
            }
        }
    }
    let mut globals = access.p1();
    for camera in cameras {
        let Ok((canvas, projection)) = canvases.get(camera) else {
            continue;
        };
        if !canvas.pixel_snap || !canvas.is_set() {
            if canvas.is_changed()
                && let Ok(mut transform) = transforms.get_mut(camera)
            {
                transform.set_changed();
            }
            continue;
        }
        let Projection::Orthographic(ortho) = projection else {
            continue;
        };
        let chain: Vec<Entity> = std::iter::once(camera)
            .chain(parents.iter_ancestors(camera))
            .collect();
        let authored = chain
            .iter()
            .rev()
            .filter_map(|&at| transforms.get(at).ok())
            .fold(GlobalTransform::IDENTITY, |global, transform| {
                global.mul_transform(*transform)
            });
        let at = authored.translation().truncate();
        let delta = canvas.snap(at, ortho.area.width() / canvas.resolution.x as f32) - at;
        stack.push((camera, authored));
        while let Some((entity, unsnapped)) = stack.pop() {
            if let Ok(mut global) = globals.get_mut(entity) {
                let mut affine = unsnapped.affine();
                let moved = unsnapped.translation().truncate() + delta;
                affine.translation = moved.extend(global.translation().z).into();
                if global.affine() != affine {
                    *global = affine.into();
                }
            }
            for &child in children.get(entity).into_iter().flatten() {
                if let Ok(transform) = transforms.get(child) {
                    stack.push((child, unsnapped.mul_transform(*transform)));
                }
            }
        }
    }
    Ok(())
}

fn is_forwarded(pointer: PointerId) -> bool {
    matches!(pointer, PointerId::Custom(uuid) if uuid.as_u64_pair().0 == POINTER_TAG)
}

fn canvas_pointer(camera: Entity, source: PointerId) -> PointerId {
    let mut hasher = DefaultHasher::new();
    (camera.to_bits(), source).hash(&mut hasher);
    PointerId::Custom(Uuid::from_u64_pair(POINTER_TAG, hasher.finish()))
}

fn forward_pointers(
    mut inputs: ResMut<Messages<PointerInput>>,
    mut cursor: Local<MessageCursor<PointerInput>>,
    views: Query<(&Camera, &RenderTarget, &PixelCanvasHelper)>,
    canvases: Query<(&PixelCanvas, &PixelCanvasExpanded)>,
    pointers: Res<PointerMap>,
    primary: Query<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) -> Result {
    let primary = primary.iter().next();
    let incoming: Vec<PointerInput> = cursor
        .read(&inputs)
        .filter(|input| !is_forwarded(input.pointer_id))
        .cloned()
        .collect();
    let mut spawned = Vec::new();
    for input in incoming {
        for (view, shown_on, helper) in &views {
            if !view.is_active
                || shown_on.normalize(primary).as_ref() != Some(&input.location.target)
            {
                continue;
            }
            let (Some((area, scale)), Ok((canvas, expanded))) =
                (view_area(view), canvases.get(helper.camera))
            else {
                continue;
            };
            let pointer = canvas_pointer(helper.camera, input.pointer_id);
            if pointers.get_entity(pointer).is_none() && !spawned.contains(&pointer) {
                commands.spawn((pointer, *helper));
                spawned.push(pointer);
            }
            let to_canvas = |at: Vec2| canvas.canvas_position(area.size(), at * scale - area.min);
            let position = to_canvas(input.location.position);
            let action = match input.action {
                PointerAction::Move { delta } => PointerAction::Move {
                    delta: position - to_canvas(input.location.position - delta),
                },
                other => other,
            };
            let target = NormalizedRenderTarget::Image(expanded.image.clone().into());
            inputs.write(PointerInput::new(
                pointer,
                Location { target, position },
                action,
            ));
        }
    }
    Ok(())
}

/// Maps a cursor through a [`PixelCanvas`] into the world its camera sees.
#[derive(SystemParam)]
pub struct CanvasCursor<'w, 's> {
    canvases: Query<
        'w,
        's,
        (
            &'static PixelCanvas,
            &'static Camera,
            &'static GlobalTransform,
            &'static PixelCanvasExpanded,
        ),
    >,
    views: Query<'w, 's, &'static Camera, With<PixelCanvasHelper>>,
}

impl CanvasCursor<'_, '_> {
    /// The world point under `cursor` for the canvas on `camera`, `cursor`
    /// in logical pixels of the window the canvas shows in, as
    /// `Window::cursor_position` gives it. `None` in the letterbox, or when
    /// `camera` has no expanded canvas.
    pub fn world_position(&self, camera: Entity, cursor: Vec2) -> Option<Vec2> {
        let (canvas, camera, global, expanded) = self.canvases.get(camera).ok()?;
        let (area, scale) = view_area(self.views.get(expanded.view).ok()?)?;
        let pixel = canvas.canvas_pixel(area.size(), cursor * scale - area.min)?;
        camera.viewport_to_world_2d(global, pixel).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::RenderTargetInfo;
    use bevy::window::WindowRef;
    use jackdaw_scene_types::CanvasScale;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            PixelCanvasPlugin,
        ));
        app.init_asset::<Image>();
        app
    }

    fn picking_app() -> (App, Entity) {
        let mut app = app();
        app.init_resource::<PointerMap>()
            .add_message::<PointerInput>();
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        (app, window)
    }

    fn spawn_canvas(app: &mut App, canvas: PixelCanvas) -> Entity {
        let camera = app
            .world_mut()
            .spawn((Camera2d, canvas, Transform::default()))
            .id();
        app.update();
        camera
    }

    fn expanded(app: &App, camera: Entity) -> &PixelCanvasExpanded {
        app.world()
            .get::<PixelCanvasExpanded>(camera)
            .expect("the canvas expanded")
    }

    fn view_of(app: &App, camera: Entity) -> &Camera {
        let view = expanded(app, camera).view;
        app.world().get::<Camera>(view).expect("view camera")
    }

    fn helpers(app: &mut App) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<PixelCanvasHelper>>()
            .iter(app.world())
            .collect()
    }

    /// Stands in for the render camera system: gives `camera` a target of
    /// `physical` pixels at `scale` and the matching projection.
    fn render_into(app: &mut App, camera: Entity, physical: UVec2, scale: f32) {
        let mut entity = app.world_mut().entity_mut(camera);
        let mut projection = entity.get::<Projection>().expect("projection").clone();
        let logical = physical.as_vec2() / scale;
        projection.update(logical.x, logical.y);
        let clip_from_view = projection.get_clip_from_view();
        *entity.get_mut::<Projection>().expect("projection") = projection;
        let mut camera = entity.get_mut::<Camera>().expect("camera");
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: physical,
            scale_factor: scale,
        });
        camera.computed.clip_from_view = clip_from_view;
    }

    fn display(app: &App, camera: Entity) -> (Vec2, Vec3) {
        let display = expanded(app, camera).display;
        let sprite = app.world().get::<Sprite>(display).expect("sprite");
        let transform = app.world().get::<Transform>(display).expect("transform");
        (sprite.custom_size.expect("sized"), transform.translation)
    }

    fn letterboxes(camera: &Camera, color: Color) -> bool {
        matches!(camera.clear_color, ClearColorConfig::Custom(c) if c == color)
    }

    #[test]
    fn a_canvas_camera_draws_into_a_nearest_image_shown_by_a_view_camera() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));

        let expanded = expanded(&app, camera);
        let image = app
            .world()
            .resource::<Assets<Image>>()
            .get(&expanded.image)
            .expect("the canvas image exists");
        assert_eq!(image.size(), UVec2::new(320, 180));
        assert!(matches!(
            image.sampler,
            ImageSampler::Descriptor(ref descriptor)
                if *descriptor == bevy::image::ImageSamplerDescriptor::nearest()
        ));
        match app.world().get::<RenderTarget>(camera) {
            Some(RenderTarget::Image(target)) => assert_eq!(target.handle, expanded.image),
            other => panic!("the camera draws into the canvas, not {other:?}"),
        }

        let (view, display) = (expanded.view, expanded.display);
        let world = app.world();
        assert!(world.get::<Camera2d>(view).is_some());
        assert!(matches!(
            world.get::<RenderTarget>(view),
            Some(RenderTarget::Window(WindowRef::Primary))
        ));
        let view_camera = world.get::<Camera>(view).expect("view camera");
        assert_eq!(view_camera.order, 0, "the view draws at the authored order");
        assert!(letterboxes(view_camera, Color::BLACK));
        assert_eq!(
            world.get::<Camera>(camera).map(|c| c.order),
            Some(CANVAS_ORDER)
        );
        let layer = RenderLayers::layer(FIRST_LAYER);
        assert_eq!(world.get::<RenderLayers>(view), Some(&layer));
        assert_eq!(world.get::<RenderLayers>(display), Some(&layer));
        assert_eq!(
            world
                .get::<Sprite>(display)
                .map(|sprite| sprite.image.clone()),
            Some(expanded.image.clone())
        );
        assert!(world.get::<RuntimeOnly>(view).is_some());
        assert!(world.get::<RuntimeOnly>(display).is_some());
        let mut marked = helpers(&mut app);
        marked.sort();
        let mut expected = vec![view, display];
        expected.sort();
        assert_eq!(marked, expected);
    }

    #[test]
    fn an_unset_canvas_or_a_plain_entity_is_left_alone() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::default());
        app.world_mut().spawn(PixelCanvas::new(320, 180));
        app.update();
        assert!(app.world().get::<PixelCanvasExpanded>(camera).is_none());
        assert!(helpers(&mut app).is_empty());
    }

    #[test]
    fn a_hud_above_the_canvas_stays_above_its_view() {
        let mut app = app();
        let hud = app
            .world_mut()
            .spawn((
                Camera2d,
                Camera {
                    order: 1,
                    ..default()
                },
            ))
            .id();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let hud_order = app.world().get::<Camera>(hud).map(|c| c.order);
        assert_eq!(hud_order, Some(1));
        assert_eq!(view_of(&app, camera).order, 0);

        app.world_mut().get_mut::<Camera>(camera).expect("c").order = 2;
        app.update();
        assert_eq!(
            view_of(&app, camera).order,
            2,
            "an order edit moves the view"
        );
        assert_eq!(
            app.world().get::<Camera>(camera).map(|c| c.order),
            Some(CANVAS_ORDER)
        );

        app.world_mut().entity_mut(camera).remove::<PixelCanvas>();
        assert_eq!(app.world().get::<Camera>(camera).map(|c| c.order), Some(2));
    }

    #[test]
    fn an_inactive_canvas_camera_hides_its_view() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        assert!(view_of(&app, camera).is_active);
        app.world_mut()
            .get_mut::<Camera>(camera)
            .expect("c")
            .is_active = false;
        app.update();
        assert!(!view_of(&app, camera).is_active);
    }

    #[test]
    fn two_canvases_keep_their_own_layers_and_only_the_lowest_view_clears() {
        let mut app = app();
        let red = Color::srgb(1.0, 0.0, 0.0);
        let first = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let second = app
            .world_mut()
            .spawn((
                Camera2d,
                Camera {
                    order: 1,
                    ..default()
                },
                PixelCanvas {
                    letterbox: red,
                    ..PixelCanvas::new(160, 90)
                },
            ))
            .id();
        app.update();
        assert_eq!(expanded(&app, first).layer, 31);
        assert_eq!(expanded(&app, second).layer, 30);
        let second_display = expanded(&app, second).display;
        assert_eq!(
            app.world().get::<RenderLayers>(second_display),
            Some(&RenderLayers::layer(30))
        );
        assert!(letterboxes(view_of(&app, first), Color::BLACK));
        assert!(matches!(
            view_of(&app, second).clear_color,
            ClearColorConfig::None
        ));

        app.world_mut().entity_mut(first).remove::<PixelCanvas>();
        app.update();
        assert!(letterboxes(view_of(&app, second), red), "it now clears");
        let third = spawn_canvas(&mut app, PixelCanvas::new(64, 64));
        assert_eq!(expanded(&app, third).layer, 31, "the freed layer is reused");
    }

    #[test]
    fn expansion_skips_a_camera_gone_before_it_applies() {
        let mut app = app();
        let camera = app
            .world_mut()
            .spawn((Camera2d, PixelCanvas::new(320, 180)))
            .id();
        app.world_mut().despawn(camera);
        expand(app.world_mut(), camera);
        let kept = app
            .world_mut()
            .spawn((Camera2d, PixelCanvas::new(320, 180)))
            .id();
        app.world_mut().entity_mut(kept).remove::<PixelCanvas>();
        expand(app.world_mut(), kept);
        assert!(helpers(&mut app).is_empty());
        assert_eq!(app.world().resource::<CanvasLayers>().0, 0);

        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        app.world_mut().despawn(camera);
        app.update();
        assert!(helpers(&mut app).is_empty(), "despawning takes the helpers");
        assert_eq!(app.world().resource::<CanvasLayers>().0, 0);
    }

    #[test]
    fn the_display_follows_the_view_target_through_resizes() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let view = expanded(&app, camera).view;

        render_into(&mut app, view, UVec2::new(1366, 768), 1.0);
        app.update();
        assert_eq!(
            display(&app, camera),
            (Vec2::new(1280.0, 720.0), Vec3::ZERO)
        );

        render_into(&mut app, view, UVec2::new(1367, 769), 1.0);
        app.update();
        assert_eq!(
            display(&app, camera),
            (Vec2::new(1280.0, 720.0), Vec3::new(-0.5, 0.5, 0.0)),
            "the odd margin falls on a whole pixel"
        );

        render_into(&mut app, view, UVec2::new(2732, 1536), 2.0);
        app.update();
        assert_eq!(
            display(&app, camera),
            (Vec2::new(1280.0, 720.0), Vec3::ZERO)
        );

        render_into(&mut app, view, UVec2::new(200, 100), 1.0);
        app.update();
        assert_eq!(display(&app, camera).0, Vec2::new(320.0, 180.0));
    }

    #[test]
    fn a_half_window_viewport_moves_to_the_view_and_back() {
        let mut app = app();
        let half = Viewport {
            physical_size: UVec2::new(683, 768),
            ..default()
        };
        let camera = app
            .world_mut()
            .spawn((
                Camera2d,
                Camera {
                    viewport: Some(half.clone()),
                    ..default()
                },
                PixelCanvas::new(320, 180),
            ))
            .id();
        app.update();
        assert!(
            app.world()
                .get::<Camera>(camera)
                .is_some_and(|c| c.viewport.is_none())
        );
        let view = expanded(&app, camera).view;
        render_into(&mut app, view, UVec2::new(1366, 768), 1.0);
        app.update();
        assert_eq!(
            view_of(&app, camera)
                .viewport
                .as_ref()
                .map(|v| v.physical_size),
            Some(half.physical_size)
        );
        assert_eq!(
            display(&app, camera),
            (Vec2::new(640.0, 360.0), Vec3::new(-0.5, 0.0, 0.0))
        );

        app.world_mut().entity_mut(camera).remove::<PixelCanvas>();
        let restored = app
            .world()
            .get::<Camera>(camera)
            .and_then(|c| c.viewport.clone());
        assert_eq!(restored.map(|v| v.physical_size), Some(half.physical_size));
    }

    #[test]
    fn editing_the_canvas_resizes_the_image_and_recolours_the_bars() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let view = expanded(&app, camera).view;
        render_into(&mut app, view, UVec2::new(1366, 768), 1.0);
        app.update();

        let red = Color::srgb(1.0, 0.0, 0.0);
        {
            let mut canvas = app
                .world_mut()
                .get_mut::<PixelCanvas>(camera)
                .expect("canvas");
            canvas.resolution = UVec2::new(640, 360);
            canvas.letterbox = red;
            canvas.scale = CanvasScale::Stretch;
        }
        app.update();

        let image = expanded(&app, camera).image.clone();
        let size = app
            .world()
            .resource::<Assets<Image>>()
            .get(&image)
            .map(Image::size);
        assert_eq!(size, Some(UVec2::new(640, 360)));
        assert!(letterboxes(view_of(&app, camera), red));
        assert_eq!(display(&app, camera).0, Vec2::new(1366.0, 768.0));
    }

    #[test]
    fn removing_the_canvas_restores_the_camera_and_clears_the_helpers() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        app.world_mut().entity_mut(camera).remove::<PixelCanvas>();
        app.update();
        assert!(helpers(&mut app).is_empty());
        assert!(app.world().get::<PixelCanvasExpanded>(camera).is_none());
        assert!(matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Window(WindowRef::Primary))
        ));
        assert_eq!(app.world().get::<Camera>(camera).map(|c| c.order), Some(0));

        app.world_mut()
            .entity_mut(camera)
            .insert(PixelCanvas::new(320, 180));
        app.update();
        assert_eq!(helpers(&mut app).len(), 2, "a new canvas expands again");
    }

    fn snap_scene(app: &mut App) -> (Entity, Entity) {
        let camera = app
            .world_mut()
            .spawn((
                Camera2d,
                PixelCanvas::new(320, 180),
                Transform::from_xyz(10.4, -3.6, 5.0),
            ))
            .id();
        let reticle = app
            .world_mut()
            .spawn((Transform::from_xyz(1.0, 2.0, 0.0), ChildOf(camera)))
            .id();
        app.update();
        set_scale(app, camera, 1.0);
        app.update();
        (camera, reticle)
    }

    fn set_scale(app: &mut App, camera: Entity, scale: f32) {
        let mut projection = app
            .world_mut()
            .get_mut::<Projection>(camera)
            .expect("projection");
        if let Projection::Orthographic(ortho) = projection.as_mut() {
            ortho.scale = scale;
            ortho.area = Rect::from_center_half_size(Vec2::ZERO, Vec2::new(160.0, 90.0) * scale);
        }
    }

    fn drawn(app: &App, entity: Entity) -> Vec3 {
        app.world()
            .get::<GlobalTransform>(entity)
            .expect("global")
            .translation()
    }

    #[test]
    fn the_camera_and_its_children_draw_on_whole_canvas_pixels() {
        let mut app = app();
        let (camera, reticle) = snap_scene(&mut app);
        assert_eq!(drawn(&app, camera), Vec3::new(10.0, -4.0, 5.0));
        assert!(drawn(&app, reticle).abs_diff_eq(Vec3::new(11.0, -2.0, 5.0), 1e-5));
        assert_eq!(
            app.world().get::<Transform>(camera).map(|t| t.translation),
            Some(Vec3::new(10.4, -3.6, 5.0))
        );

        app.world_mut()
            .get_mut::<PixelCanvas>(camera)
            .expect("c")
            .pixel_snap = false;
        app.update();
        app.update();
        assert_eq!(drawn(&app, camera), Vec3::new(10.4, -3.6, 5.0));
        assert!(drawn(&app, reticle).abs_diff_eq(Vec3::new(11.4, -1.6, 5.0), 1e-5));
    }

    #[test]
    fn zooming_a_still_camera_snaps_from_the_authored_position_without_drift() {
        let mut app = app();
        let (camera, _) = snap_scene(&mut app);
        let canvas = PixelCanvas::new(320, 180);
        for scale in [0.9, 0.7, 1.3, 0.9, 0.55, 1.0, 0.9] {
            set_scale(&mut app, camera, scale);
            app.update();
            let expected = canvas.snap(Vec2::new(10.4, -3.6), scale);
            let at = drawn(&app, camera).truncate();
            assert!(
                (at - expected).length() < 1e-4,
                "{scale}: {at} vs {expected}"
            );
        }
    }

    fn mapped_canvas(physical: UVec2, scale: f32) -> (App, Entity) {
        let (mut app, _) = picking_app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let view = expanded(&app, camera).view;
        render_into(&mut app, camera, UVec2::new(320, 180), 1.0);
        render_into(&mut app, view, physical, scale);
        app.update();
        (app, camera)
    }

    fn assert_near(at: Option<Vec2>, expected: Vec2) {
        let at = at.expect("on the canvas");
        assert!((at - expected).length() < 1e-3, "{at} vs {expected}");
    }

    fn world_at(app: &mut App, camera: Entity, cursor: Vec2) -> Option<Vec2> {
        app.world_mut()
            .run_system_cached_with(
                |In((camera, cursor)): In<(Entity, Vec2)>, canvas: CanvasCursor| {
                    canvas.world_position(camera, cursor)
                },
                (camera, cursor),
            )
            .expect("runs")
    }

    #[test]
    fn a_cursor_maps_through_the_letterbox_into_the_world() {
        let (mut app, camera) = mapped_canvas(UVec2::new(1366, 768), 1.0);
        assert_near(
            world_at(&mut app, camera, Vec2::new(683.0, 384.0)),
            Vec2::ZERO,
        );
        assert_near(
            world_at(&mut app, camera, Vec2::new(43.0, 24.0)),
            Vec2::new(-160.0, 90.0),
        );
        assert_near(
            world_at(&mut app, camera, Vec2::new(1322.5, 743.5)),
            Vec2::new(159.875, -89.875),
        );
        assert_eq!(world_at(&mut app, camera, Vec2::new(20.0, 384.0)), None);
        assert_eq!(world_at(&mut app, camera, Vec2::new(683.0, 760.0)), None);
    }

    #[test]
    fn a_cursor_maps_at_a_fractional_scale_factor() {
        let (mut app, camera) = mapped_canvas(UVec2::new(1600, 900), 1.25);
        assert_near(
            world_at(&mut app, camera, Vec2::new(640.0, 360.0)),
            Vec2::ZERO,
        );
        assert_near(
            world_at(&mut app, camera, Vec2::ZERO),
            Vec2::new(-160.0, 90.0),
        );
    }

    fn forwarded(app: &App) -> Vec<PointerInput> {
        app.world()
            .resource::<Messages<PointerInput>>()
            .iter_current_update_messages()
            .filter(|input| is_forwarded(input.pointer_id))
            .cloned()
            .collect()
    }

    #[test]
    fn each_source_pointer_gets_its_own_canvas_pointer() {
        let (mut app, window) = picking_app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let view = expanded(&app, camera).view;
        render_into(&mut app, view, UVec2::new(1366, 768), 1.0);
        app.update();

        let target = RenderTarget::Window(WindowRef::Primary)
            .normalize(Some(window))
            .expect("window target");
        let foreign = PointerId::Custom(Uuid::from_u128(7));
        for (pointer, at) in [
            (PointerId::Touch(1), Vec2::new(683.0, 384.0)),
            (PointerId::Touch(2), Vec2::new(43.0, 24.0)),
            (foreign, Vec2::new(47.0, 24.0)),
        ] {
            let location = Location {
                target: target.clone(),
                position: at,
            };
            let action = PointerAction::Move {
                delta: Vec2::new(4.0, 0.0),
            };
            app.world_mut()
                .write_message(PointerInput::new(pointer, location, action));
        }
        app.update();

        let sent = forwarded(&app);
        assert_eq!(sent.len(), 3, "{sent:?}");
        let image = expanded(&app, camera).image.clone();
        let canvas_target = NormalizedRenderTarget::Image(image.into());
        assert!(
            sent.iter()
                .all(|input| input.location.target == canvas_target)
        );
        let positions: Vec<Vec2> = sent.iter().map(|i| i.location.position).collect();
        assert_eq!(
            positions,
            [Vec2::new(160.0, 90.0), Vec2::ZERO, Vec2::new(1.0, 0.0)]
        );
        let ids: Vec<PointerId> = sent.iter().map(|i| i.pointer_id).collect();
        assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
        assert!(matches!(
            sent[0].action,
            PointerAction::Move { delta } if delta == Vec2::new(1.0, 0.0)
        ));
        assert_eq!(
            helpers(&mut app).len(),
            5,
            "view, display and three pointers"
        );

        let mut seen = app
            .world()
            .resource::<Messages<PointerInput>>()
            .get_cursor_current();
        app.update();
        let messages = app.world().resource::<Messages<PointerInput>>();
        assert_eq!(seen.read(messages).count(), 0, "nothing is forwarded twice");
    }

    #[test]
    fn a_capture_camera_shows_what_the_view_shows() {
        let mut app = app();
        let camera = spawn_canvas(&mut app, PixelCanvas::new(320, 180));
        let view = expanded(&app, camera).view;
        render_into(&mut app, view, UVec2::new(1366, 768), 1.0);
        app.update();
        let capture = app.world_mut().spawn(Camera::default()).id();
        assert!(capture_canvas(app.world_mut(), capture, camera));
        let plain = app.world_mut().spawn(Camera2d).id();
        assert!(!capture_canvas(app.world_mut(), capture, plain));

        let world = app.world();
        assert_eq!(
            world.get::<RenderLayers>(capture),
            Some(&RenderLayers::layer(31))
        );
        assert!(matches!(
            world.get::<Projection>(capture),
            Some(Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::AutoMin { min_width, min_height },
                ..
            })) if *min_width == 1366.0 && *min_height == 768.0
        ));
        assert!(
            world
                .get::<Camera>(capture)
                .is_some_and(|c| letterboxes(c, Color::BLACK))
        );
    }
}
