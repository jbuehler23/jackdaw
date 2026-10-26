//! The 2D world adapter on the 2D panel's stage: picks and outlines the
//! [`Sprite`]s of a scene rooted at a [`Scene2dRoot`]. It sits beside
//! [`crate::ui_stage`], which does the same for authored UI, and owns a stage
//! only while `shows_2d_world` holds for it.

use std::{cmp::Ordering, collections::HashMap};

use bevy::{
    asset::AssetEvent,
    ecs::system::SystemParam,
    picking::{
        events::{PointerDrag, PointerPress, PointerRelease},
        prelude::Pickable,
    },
    prelude::*,
    sprite::{Anchor, SpritePickingMode, SpritePickingSettings},
    transform::TransformSystems,
    ui::{ComputedNode, UiGlobalTransform, UiSystems, UiTransform},
};
use jackdaw_feathers::tokens;
use jackdaw_scene_types::{Locked, Scene2dRoot};

use crate::{
    EditorEntity,
    prefab::AuthoredUiSceneRoot,
    selection::Selection,
    ui_stage::{stage_pixels_per_target_pixel, stage_to_authored},
    viewport_2d::{
        Scene2dViewport, Viewport2dMode, Viewport2dPanelHost, stage_offset_clamped,
        target_pixel_to_world, target_pixels_per_stage_pixel, world_to_target_pixel,
    },
};

/// Thickness of a sprite outline's edges, in the stage's logical pixels.
const EDGE_WIDTH: f32 = 2.0;

/// Draw order of the outlines inside the stage, level with the UI outlines.
const OUTLINE_Z: i32 = 50;

pub struct SpriteStagePlugin;

impl Plugin for SpriteStagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpritePress>()
            .add_observer(on_sprite_press)
            .add_observer(on_sprite_drag)
            .add_observer(on_sprite_release)
            .add_systems(
                PostUpdate,
                sync_sprite_outlines
                    .after(TransformSystems::Propagate)
                    .before(UiSystems::Prepare),
            );
    }
}

/// The filter naming the root of an open 2D world scene.
pub(crate) type OpenScene2dRoot = (With<Scene2dRoot>, Without<ChildOf>);

/// Roots of the open 2D world scenes.
pub(crate) type Scene2dRoots<'w, 's> = Query<'w, 's, Entity, OpenScene2dRoot>;

/// Whether `host`'s stage shows a 2D world scene: one is open and no authored
/// UI is routed to the panel's camera. The sprite adapter owns such a stage,
/// and the UI marquee stays off it.
pub(crate) fn shows_2d_world<'a>(
    host: &Viewport2dPanelHost,
    routed: impl IntoIterator<Item = &'a UiTargetCamera>,
    scene_roots: &Scene2dRoots,
) -> bool {
    world_on_panel(!scene_roots.is_empty(), routed, host)
}

/// Whether a panel shows a 2D world scene, given whether one is open and the
/// cameras authored UI is routed to.
pub(crate) fn world_on_panel<'a>(
    open: bool,
    routed: impl IntoIterator<Item = &'a UiTargetCamera>,
    host: &Viewport2dPanelHost,
) -> bool {
    open && !routed
        .into_iter()
        .any(|routed| routed.entity() == host.camera)
}

/// [`shows_2d_world`] read straight off the world, for a menu building its
/// rows.
pub(crate) fn world_shows_2d_world(world: &World, host: &Viewport2dPanelHost) -> bool {
    let open = world
        .try_query_filtered::<(), OpenScene2dRoot>()
        .is_some_and(|mut roots| roots.iter(world).next().is_some());
    let Some(mut routed) = world.try_query_filtered::<&UiTargetCamera, AuthoredUiSceneRoot>()
    else {
        return open;
    };
    world_on_panel(open, routed.iter(world), host)
}

/// Which world the transform tools act on from the keyboard: the 3D world,
/// or the 2D panel in front when it is editing a 2D world scene.
#[derive(SystemParam)]
pub(crate) struct ToolViewport<'w, 's> {
    fronted: crate::viewport_2d::FrontedViewport<'w, 's>,
    ui_roots: Query<'w, 's, &'static UiTargetCamera, AuthoredUiSceneRoot>,
    scene_roots: Scene2dRoots<'w, 's>,
}

impl ToolViewport<'_, '_> {
    /// Whether the tools apply: not on a UI canvas, nor on a 2D panel that is
    /// being played rather than edited.
    pub(crate) fn takes_tools(&self) -> bool {
        if self.fronted.is_three_d() {
            return true;
        }
        self.fronted.two_d_host().is_some_and(|host| {
            host.mode == Viewport2dMode::Edit
                && shows_2d_world(host, self.ui_roots.iter(), &self.scene_roots)
        })
    }
}

/// The sprite the last press on a 2D world stage picked, if any. A drag that
/// follows the press moves the selection when this is selected.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub(crate) struct SpritePress {
    pub(crate) stage: Option<Entity>,
    pub(crate) sprite: Option<Entity>,
    /// A sprite of a larger selection the press kept the selection for, so
    /// a drag could take all of it. Released without a drag, the click
    /// narrows the selection to it.
    narrow: Option<Entity>,
}

/// The world point under `cursor` (ui-logical) on a 2D panel's stage, and
/// whether the cursor is on the stage at all; off it, the point is clamped
/// to the stage's edge.
pub(crate) fn stage_world_point(
    cursor: Vec2,
    host: &Viewport2dPanelHost,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
) -> (Vec2, bool) {
    let (offset, inside) = stage_offset_clamped(
        cursor,
        transform.translation,
        computed.size(),
        computed.inverse_scale_factor(),
        target_pixels_per_stage_pixel(computed.size(), host.target_size),
    );
    let point = target_pixel_to_world(
        host.view,
        host.target_size,
        stage_to_authored(offset, host.target_size),
    );
    (point, inside)
}

/// The outline drawn around one selected sprite on one panel's stage.
#[derive(Component, Clone, Copy, Debug)]
pub struct SpriteSelectionOutline {
    /// The panel content entity carrying the stage's [`Viewport2dPanelHost`].
    pub host: Entity,
    pub sprite: Entity,
    /// The sprite's rect corners, counter-clockwise in the world from the
    /// local bottom-left, in the stage's logical pixels.
    pub corners: [Vec2; 4],
    edges: [Entity; 4],
}

type PickParts = (
    Entity,
    &'static Sprite,
    Option<&'static Anchor>,
    &'static GlobalTransform,
    &'static InheritedVisibility,
    Option<&'static SpritePickingMode>,
);

/// What a sprite's size and pixels are read from.
#[derive(SystemParam)]
pub struct SpritePixels<'w> {
    images: Res<'w, Assets<Image>>,
    atlases: Res<'w, Assets<TextureAtlasLayout>>,
    server: Res<'w, AssetServer>,
    settings: Option<Res<'w, SpritePickingSettings>>,
}

impl SpritePixels<'_> {
    /// Whether the press at `point` lands on `sprite` under the sprite's own
    /// picking mode, or the app's [`SpritePickingSettings`] without one.
    fn contains(
        &self,
        sprite: &Sprite,
        anchor: Anchor,
        transform: &GlobalTransform,
        mode: Option<&SpritePickingMode>,
        point: Vec2,
    ) -> bool {
        let mode = mode.copied().unwrap_or_else(|| {
            self.settings.as_deref().map_or_else(
                || SpritePickingSettings::default().picking_mode,
                |s| s.picking_mode,
            )
        });
        let image = sprite_image(sprite, &self.images, &self.server);
        sprite_contains(
            sprite,
            anchor,
            transform,
            point,
            mode,
            image,
            &self.atlases,
            &self.images,
        )
    }

    pub(crate) fn world_corners(
        &self,
        sprite: &Sprite,
        anchor: Anchor,
        transform: &GlobalTransform,
    ) -> Option<[Vec2; 4]> {
        sprite_world_corners(sprite, anchor, transform, &self.images, &self.atlases)
    }
}

/// What a sprite draws from, as far as picking is concerned.
#[derive(Clone, Copy)]
pub enum SpriteImage<'a> {
    /// An image in main-world memory, read texel by texel.
    Pixels(&'a Image),
    /// Drawn, but with no pixels to read: a plain colour, or an image kept
    /// only on the GPU. The whole rect picks.
    Opaque,
    /// Not drawn: the image is still loading or failed to.
    Missing,
}

/// Classify what `sprite` draws from.
pub fn sprite_image<'a>(
    sprite: &Sprite,
    images: &'a Assets<Image>,
    server: &AssetServer,
) -> SpriteImage<'a> {
    if let Some(image) = images.get(&sprite.image) {
        return SpriteImage::Pixels(image);
    }
    if sprite.image.id() == AssetId::default() || server.is_loaded(&sprite.image) {
        return SpriteImage::Opaque;
    }
    SpriteImage::Missing
}

/// The sprite's drawn rect in its own local frame: its size from
/// `custom_size`, `rect`, the atlas cell or the image, shifted by the anchor.
/// `None` while the image it is sized by has not loaded.
pub fn sprite_local_rect(
    sprite: &Sprite,
    anchor: Anchor,
    images: &Assets<Image>,
    atlases: &Assets<TextureAtlasLayout>,
) -> Option<Rect> {
    let size = sprite
        .custom_size
        .or_else(|| sprite.rect.map(|rect| rect.size()))
        .or_else(|| {
            sprite
                .texture_atlas
                .as_ref()
                .and_then(|atlas| atlas.texture_rect(atlases))
                .map(|rect| rect.size().as_vec2())
        })
        .or_else(|| {
            images
                .get(&sprite.image)
                .map(|image| image.size().as_vec2())
        })?;
    Some(Rect::from_center_size(-anchor.as_vec() * size, size))
}

/// The four corners of [`sprite_local_rect`] carried into the world (y up),
/// counter-clockwise from the local bottom-left.
pub fn sprite_world_corners(
    sprite: &Sprite,
    anchor: Anchor,
    transform: &GlobalTransform,
    images: &Assets<Image>,
    atlases: &Assets<TextureAtlasLayout>,
) -> Option<[Vec2; 4]> {
    let rect = sprite_local_rect(sprite, anchor, images, atlases)?;
    let local = [
        rect.min,
        Vec2::new(rect.max.x, rect.min.y),
        rect.max,
        Vec2::new(rect.min.x, rect.max.y),
    ];
    Some(local.map(|corner| transform.transform_point(corner.extend(0.0)).xy()))
}

/// Whether the world point `point`, seen down the 2D camera's axis, picks the
/// sprite. The rect test is bevy's own (`Sprite::compute_pixel_space_point`).
/// Under [`SpritePickingMode::AlphaThreshold`] the texel there must be more
/// opaque than the cutoff; a texel that cannot be read (an unsupported
/// format) falls back to the rect.
pub fn sprite_contains(
    sprite: &Sprite,
    anchor: Anchor,
    transform: &GlobalTransform,
    point: Vec2,
    mode: SpritePickingMode,
    image: SpriteImage,
    atlases: &Assets<TextureAtlasLayout>,
    images: &Assets<Image>,
) -> bool {
    if matches!(image, SpriteImage::Missing) {
        return false;
    }
    let to_local = transform.affine().inverse();
    let near = to_local.transform_point3(point.extend(0.0));
    let far = to_local.transform_point3(point.extend(1.0));
    if near.z == far.z {
        return false;
    }
    let local = near.lerp(far, near.z / (near.z - far.z)).xy();
    let Ok(texel) = sprite.compute_pixel_space_point(local, anchor, images, atlases) else {
        return false;
    };
    let SpritePickingMode::AlphaThreshold(cutoff) = mode else {
        return true;
    };
    let SpriteImage::Pixels(image) = image else {
        return true;
    };
    let texel = texel.as_uvec2();
    match image.get_color_at(texel.x, texel.y) {
        Ok(color) => color.alpha() > cutoff,
        Err(_) => true,
    }
}

/// Whether `entity` sits under an open [`Scene2dRoot`].
pub(crate) fn in_2d_scene(entity: Entity, parents: &Query<&ChildOf>, roots: &Scene2dRoots) -> bool {
    parents
        .iter_ancestors(entity)
        .any(|ancestor| roots.contains(ancestor))
}

/// `entity`'s place in the scene document: its sibling index at each level
/// from the root down. Comparing two of these orders entities the way the
/// outliner lists them.
fn document_path(
    entity: Entity,
    parents: &Query<&ChildOf>,
    children: &Query<&Children>,
) -> Vec<usize> {
    let mut path = Vec::new();
    let mut current = entity;
    while let Ok(parent) = parents.get(current) {
        let index = children
            .get(parent.parent())
            .ok()
            .and_then(|siblings| siblings.iter().position(|sibling| sibling == current))
            .unwrap_or(0);
        path.push(index);
        current = parent.parent();
    }
    path.reverse();
    path
}

/// Select the topmost sprite under a press on a 2D world stage, in
/// [`Viewport2dMode::Edit`]. Only visible, unlocked sprites of the open 2D
/// scene take part.
///
/// Topmost is the highest world z. Bevy draws sprites of equal z in the order
/// it happened to queue them, which an author cannot see or set, so a tie
/// goes to the sprite later in the scene document instead, as the outliner
/// lists them.
///
/// Shift extends, Ctrl toggles and a plain press on no sprite clears, through
/// [`Selection`] the way [`crate::ui_stage`] does.
fn on_sprite_press(
    event: On<PointerPress>,
    ui_scale: Res<UiScale>,
    keys: Res<ButtonInput<KeyCode>>,
    hosts: Query<&Viewport2dPanelHost>,
    stages: Query<(&ComputedNode, &UiGlobalTransform), With<Scene2dViewport>>,
    ui_roots: Query<&UiTargetCamera, AuthoredUiSceneRoot>,
    scene_roots: Scene2dRoots,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    sprites: Query<PickParts, (Without<Locked>, Without<EditorEntity>)>,
    pixels: SpritePixels,
    mut selection: ResMut<Selection>,
    mut press: ResMut<SpritePress>,
    mut commands: Commands,
) -> Result {
    if event.button != PointerButton::Primary {
        return Ok(());
    }
    let target = event.event_target();
    let Some(host) = hosts.iter().find(|host| host.stage == target) else {
        return Ok(());
    };
    if host.mode != Viewport2dMode::Edit || !shows_2d_world(host, ui_roots.iter(), &scene_roots) {
        return Ok(());
    }
    let (computed, transform) = stages.get(host.stage)?;
    let (point, inside) = stage_world_point(
        event.pointer.position / ui_scale.0,
        host,
        computed,
        transform,
    );
    if !inside {
        return Ok(());
    }

    let hits: Vec<(Entity, f32)> = sprites
        .iter()
        .filter(|(_, sprite, anchor, transform, visible, mode)| {
            let anchor = anchor.copied().unwrap_or_default();
            visible.get() && pixels.contains(sprite, anchor, transform, *mode, point)
        })
        .filter(|(entity, ..)| in_2d_scene(*entity, &parents, &scene_roots))
        .map(|(entity, _, _, transform, ..)| (entity, transform.translation().z))
        .collect();
    let top = hits
        .iter()
        .map(|(_, z)| *z)
        .max_by(f32::total_cmp)
        .unwrap_or(f32::NEG_INFINITY);
    let picked = hits
        .into_iter()
        .filter(|(_, z)| z.total_cmp(&top) == Ordering::Equal)
        .map(|(entity, _)| (document_path(entity, &parents, &children), entity))
        .max()
        .map(|(_, entity)| entity);

    let extend = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let toggle = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mut narrow = None;
    match picked {
        Some(entity) if !extend && !toggle && selection.is_selected(entity) => {
            if selection.entities.len() > 1 {
                narrow = Some(entity);
            }
        }
        Some(entity) if extend => selection.extend(&mut commands, entity),
        Some(entity) if toggle => selection.toggle(&mut commands, entity),
        Some(entity) => selection.select_single(&mut commands, entity),
        None if !extend && !toggle => selection.clear(&mut commands),
        None => {}
    }
    *press = SpritePress {
        stage: Some(host.stage),
        sprite: picked,
        narrow,
    };
    Ok(())
}

/// A drag from the press, once past the drag threshold, keeps the selection
/// it started with.
fn on_sprite_drag(event: On<PointerDrag>, ui_scale: Res<UiScale>, mut press: ResMut<SpritePress>) {
    if press.stage == Some(event.event_target())
        && event.distance.length() / ui_scale.0 > crate::viewport_select::BOX_SELECT_DRAG_THRESHOLD
    {
        press.narrow = None;
    }
}

/// A click on one sprite of a larger selection, released without a drag,
/// selects that sprite alone.
fn on_sprite_release(
    event: On<PointerRelease>,
    mut press: ResMut<SpritePress>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Primary || press.stage != Some(event.event_target()) {
        return;
    }
    if let Some(entity) = press.narrow.take() {
        selection.select_single(&mut commands, entity);
    }
}

/// Keep one outline per selected sprite on each 2D panel in
/// [`Viewport2dMode::Edit`], following the sprite's transform and the view.
/// Runs after transforms propagate and before UI layout, so an outline lands
/// on the frame its sprite moves; it does nothing on a frame where none of
/// its inputs changed.
///
/// Each outline is four edge nodes rotated into place, so a rotated, scaled
/// or sheared sprite is traced exactly while the line stays a constant
/// thickness at any zoom.
fn sync_sprite_outlines(
    mut commands: Commands,
    selection: Res<Selection>,
    hosts: Query<(Entity, Ref<Viewport2dPanelHost>)>,
    stages: Query<Ref<ComputedNode>, With<Scene2dViewport>>,
    scene_roots: Scene2dRoots,
    parents: Query<&ChildOf>,
    sprites: Query<(&Sprite, Option<&Anchor>, &GlobalTransform)>,
    moved: Query<
        (),
        (
            With<Sprite>,
            Or<(Changed<GlobalTransform>, Changed<Sprite>, Changed<Anchor>)>,
        ),
    >,
    mut image_events: MessageReader<AssetEvent<Image>>,
    pixels: SpritePixels,
    mut outlines: Query<(Entity, &mut SpriteSelectionOutline)>,
    mut edges: Query<(&mut Node, &mut UiTransform)>,
) {
    let images_changed = !image_events.is_empty();
    image_events.clear();
    let changed = selection.is_changed()
        || images_changed
        || hosts.iter().any(|(_, host)| host.is_changed())
        || stages.iter().any(|stage| stage.is_changed())
        || selection
            .entities
            .iter()
            .any(|&entity| moved.contains(entity));
    if !changed {
        return;
    }

    let mut wanted: HashMap<(Entity, Entity), [Vec2; 4]> = HashMap::new();
    for (host_entity, host) in &hosts {
        if host.mode != Viewport2dMode::Edit {
            continue;
        }
        let Ok(stage) = stages.get(host.stage) else {
            continue;
        };
        let scale = stage_pixels_per_target_pixel(
            target_pixels_per_stage_pixel(stage.size(), host.target_size),
            stage.inverse_scale_factor(),
        );
        for &entity in &selection.entities {
            let Ok((sprite, anchor, transform)) = sprites.get(entity) else {
                continue;
            };
            if !in_2d_scene(entity, &parents, &scene_roots) {
                continue;
            }
            let anchor = anchor.copied().unwrap_or_default();
            let Some(world) = pixels.world_corners(sprite, anchor, transform) else {
                continue;
            };
            let corners = world
                .map(|corner| world_to_target_pixel(host.view, host.target_size, corner) * scale);
            wanted.insert((host_entity, entity), corners);
        }
    }

    for (entity, mut outline) in &mut outlines {
        let Some(corners) = wanted.remove(&(outline.host, outline.sprite)) else {
            commands.entity(entity).despawn();
            continue;
        };
        if outline.corners == corners {
            continue;
        }
        outline.corners = corners;
        for (index, edge) in outline.edges.iter().enumerate() {
            if let Ok((mut node, mut transform)) = edges.get_mut(*edge) {
                place_edge(
                    &mut node,
                    &mut transform,
                    corners[index],
                    corners[(index + 1) % 4],
                );
            }
        }
    }

    for ((host_entity, sprite), corners) in wanted {
        let Ok((_, host)) = hosts.get(host_entity) else {
            continue;
        };
        spawn_outline(&mut commands, host_entity, host.stage, sprite, corners);
    }
}

fn spawn_outline(
    commands: &mut Commands,
    host: Entity,
    stage: Entity,
    sprite: Entity,
    corners: [Vec2; 4],
) {
    let outline = commands
        .spawn((
            EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            ZIndex(OUTLINE_Z),
            Pickable::IGNORE,
            ChildOf(stage),
        ))
        .id();
    let edges = [0, 1, 2, 3].map(|index| {
        let mut node = Node {
            position_type: PositionType::Absolute,
            ..default()
        };
        let mut transform = UiTransform::default();
        place_edge(
            &mut node,
            &mut transform,
            corners[index],
            corners[(index + 1) % 4],
        );
        commands
            .spawn((
                EditorEntity,
                node,
                transform,
                BackgroundColor(tokens::ACCENT_BLUE),
                Pickable::IGNORE,
                ChildOf(outline),
            ))
            .id()
    });
    commands.entity(outline).insert(SpriteSelectionOutline {
        host,
        sprite,
        corners,
        edges,
    });
}

/// Lay an edge node from `from` to `to` (stage logical pixels, y down): a
/// thin bar centred on the segment's midpoint, turned clockwise onto it.
fn place_edge(node: &mut Node, transform: &mut UiTransform, from: Vec2, to: Vec2) {
    let delta = to - from;
    let length = delta.length();
    let middle = (from + to) / 2.0;
    node.left = px(middle.x - length / 2.0);
    node.top = px(middle.y - EDGE_WIDTH / 2.0);
    node.width = px(length);
    node.height = px(EDGE_WIDTH);
    transform.rotation = Rot2::radians(delta.y.atan2(delta.x));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    };

    /// A 2x1 image: the left texel opaque, the right one transparent.
    fn half_clear(images: &mut Assets<Image>) -> Handle<Image> {
        images.add(Image::new(
            Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![255, 255, 255, 255, 0, 0, 0, 0],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ))
    }

    fn hit(
        sprite: &Sprite,
        image: SpriteImage,
        mode: SpritePickingMode,
        images: &Assets<Image>,
        x: f32,
    ) -> bool {
        sprite_contains(
            sprite,
            Anchor::CENTER,
            &GlobalTransform::IDENTITY,
            Vec2::new(x, 0.0),
            mode,
            image,
            &Assets::<TextureAtlasLayout>::default(),
            images,
        )
    }

    #[test]
    fn a_transparent_texel_does_not_take_the_press() {
        let mut images = Assets::<Image>::default();
        let sprite = Sprite {
            image: half_clear(&mut images),
            custom_size: Some(Vec2::new(20.0, 10.0)),
            ..default()
        };
        let pixels = SpriteImage::Pixels(images.get(&sprite.image).expect("added"));
        let mode = SpritePickingSettings::default().picking_mode;
        assert!(
            hit(&sprite, pixels, mode, &images, -5.0),
            "the opaque half picks"
        );
        assert!(
            !hit(&sprite, pixels, mode, &images, 5.0),
            "the transparent half does not"
        );
        assert!(
            !hit(&sprite, pixels, mode, &images, 15.0),
            "outside the rect does not"
        );
        assert!(
            hit(
                &sprite,
                pixels,
                SpritePickingMode::BoundingBox,
                &images,
                5.0
            ),
            "a bounding-box mode ignores alpha"
        );
        assert!(
            hit(
                &sprite,
                pixels,
                SpritePickingMode::AlphaThreshold(-1.0),
                &images,
                5.0
            ),
            "the cutoff comes from the mode"
        );
    }

    #[test]
    fn a_sprite_that_does_not_draw_does_not_pick() {
        let images = Assets::<Image>::default();
        let sprite = Sprite {
            custom_size: Some(Vec2::new(20.0, 10.0)),
            ..default()
        };
        let mode = SpritePickingSettings::default().picking_mode;
        assert!(!hit(&sprite, SpriteImage::Missing, mode, &images, 0.0));
        assert!(hit(&sprite, SpriteImage::Opaque, mode, &images, 0.0));
    }

    #[test]
    fn the_rect_follows_the_anchor() {
        let images = Assets::<Image>::default();
        let atlases = Assets::<TextureAtlasLayout>::default();
        let sprite = Sprite {
            custom_size: Some(Vec2::new(20.0, 10.0)),
            ..default()
        };
        let rect = sprite_local_rect(&sprite, Anchor::BOTTOM_LEFT, &images, &atlases);
        assert_eq!(rect, Some(Rect::new(0.0, 0.0, 20.0, 10.0)));
    }
}
