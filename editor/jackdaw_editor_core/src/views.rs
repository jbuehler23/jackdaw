//! Offscreen views of a part: cameras of their own, framed on the part's
//! bounds, rendered to small tiles and laid out on one sheet. The user's
//! viewport camera is never touched.

use bevy::camera::ScalingMode;
use bevy::camera::visibility::RenderLayers;
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::camera_capture::{CaptureSink, OffscreenCamera, spawn_offscreen_camera};
use crate::viewport::{MainViewportCamera, ViewportLayerCounter};

/// Most tiles one request may render.
pub const MAX_TILES: usize = 16;
/// Largest sheet edge a request may ask for, in pixels.
pub const MAX_EDGE: u32 = 4096;
/// Smallest tile edge worth rendering, in pixels.
pub const MIN_TILE: u32 = 16;
/// Room left between the framed bounds and each edge of a tile, as a share
/// of the tile.
pub const PADDING: f32 = 0.08;
/// Smallest radius framed, so a point-sized target still gets a view.
const MIN_RADIUS: f32 = 0.05;
/// Width of the line drawn between tiles on a sheet, in pixels.
const DIVIDER: u32 = 1;
const DIVIDER_COLOR: [u8; 3] = [24, 24, 24];

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ViewCaptures>()
        .add_systems(Update, follow_isolated_meshes);
}

/// Where a view looks from. Axis views stand where `view.set_axis` puts
/// the viewport and are orthographic; `Iso` and `Eye` are perspective.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewAngle {
    Front,
    Back,
    Left,
    Right,
    Top,
    Iso,
    /// An explicit eye; `target` defaults to the centre of the bounds.
    Eye {
        eye: Vec3,
        target: Option<Vec3>,
    },
}

impl ViewAngle {
    pub const DEFAULT: [ViewAngle; 4] = [Self::Front, Self::Right, Self::Top, Self::Iso];
    pub const NAMES: [&'static str; 6] = ["front", "back", "left", "right", "top", "iso"];

    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "front" => Self::Front,
            "back" => Self::Back,
            "left" => Self::Left,
            "right" => Self::Right,
            "top" => Self::Top,
            "iso" => Self::Iso,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Iso => "iso",
            Self::Eye { .. } => "eye",
        }
    }

    /// The direction from the target to the eye, and the up hint, for the
    /// views that are not an explicit eye.
    fn axis(&self) -> Option<(Vec3, Vec3)> {
        Some(match self {
            Self::Front => (Vec3::Z, Vec3::Y),
            Self::Back => (Vec3::NEG_Z, Vec3::Y),
            Self::Right => (Vec3::X, Vec3::Y),
            Self::Left => (Vec3::NEG_X, Vec3::Y),
            Self::Top => (Vec3::Y, Vec3::NEG_Z),
            Self::Iso => (Vec3::ONE.normalize(), Vec3::Y),
            Self::Eye { .. } => return None,
        })
    }
}

/// The camera pose and projection that frame the world box `min..max` from
/// `angle`, for a square tile.
pub fn frame(angle: ViewAngle, (min, max): (Vec3, Vec3)) -> (Transform, Projection) {
    let center = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(MIN_RADIUS);
    let perspective = PerspectiveProjection::default();
    match angle {
        ViewAngle::Eye { eye, target } => {
            let target = target.unwrap_or(center);
            let up = up_for(target - eye);
            let reach = eye.distance(center) + radius * 2.0;
            (
                Transform::from_translation(eye).looking_at(target, up),
                Projection::Perspective(PerspectiveProjection {
                    far: perspective.far.max(reach),
                    ..perspective
                }),
            )
        }
        ViewAngle::Iso => {
            let (dir, up) = angle.axis().unwrap_or((Vec3::Z, Vec3::Y));
            let distance = radius / padded_half_angle(perspective.fov).sin();
            (
                Transform::from_translation(center + dir * distance).looking_at(center, up),
                Projection::Perspective(PerspectiveProjection {
                    far: perspective.far.max(distance + radius * 2.0),
                    ..perspective
                }),
            )
        }
        _ => {
            let (dir, up) = angle.axis().unwrap_or((Vec3::Z, Vec3::Y));
            let distance = radius * 2.0 + 1.0;
            let transform =
                Transform::from_translation(center + dir * distance).looking_at(center, up);
            let (right, up) = (transform.right(), transform.up());
            let mut half = MIN_RADIUS;
            for corner in corners(min, max) {
                let offset = corner - center;
                half = half
                    .max(offset.dot(*right).abs())
                    .max(offset.dot(*up).abs());
            }
            (
                transform,
                Projection::Orthographic(OrthographicProjection {
                    scaling_mode: ScalingMode::FixedVertical {
                        viewport_height: half * 2.0 / (1.0 - 2.0 * PADDING),
                    },
                    near: 0.0,
                    far: distance + radius * 2.0,
                    ..OrthographicProjection::default_3d()
                }),
            )
        }
    }
}

/// Half the angle a perspective tile of vertical `fov` leaves for the
/// bounds once [`PADDING`] is kept clear on each side.
pub fn padded_half_angle(fov: f32) -> f32 {
    ((fov * 0.5).tan() * (1.0 - 2.0 * PADDING)).atan()
}

fn up_for(forward: Vec3) -> Vec3 {
    if forward.normalize_or_zero().dot(Vec3::Y).abs() > 0.999 {
        Vec3::NEG_Z
    } else {
        Vec3::Y
    }
}

fn corners(min: Vec3, max: Vec3) -> impl Iterator<Item = Vec3> {
    (0..8u32).map(move |corner| {
        Vec3::new(
            if corner & 1 == 0 { min.x } else { max.x },
            if corner & 2 == 0 { min.y } else { max.y },
            if corner & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// How square tiles sit on a sheet: `columns` by `rows`, each `tile` pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetLayout {
    pub columns: u32,
    pub rows: u32,
    pub tile: u32,
}

impl SheetLayout {
    /// The layout for `tiles` tiles whose sheet fits inside `max_px`, with
    /// `columns` when given and a near-square grid otherwise. `None` when
    /// there are no tiles or a tile would be smaller than [`MIN_TILE`].
    pub fn new(tiles: usize, columns: Option<usize>, max_px: u32) -> Option<Self> {
        let tiles = u32::try_from(tiles).ok().filter(|tiles| *tiles > 0)?;
        let columns = columns
            .and_then(|columns| u32::try_from(columns).ok())
            .filter(|columns| *columns > 0)
            .unwrap_or_else(|| (tiles as f32).sqrt().ceil() as u32)
            .min(tiles);
        let rows = tiles.div_ceil(columns);
        let tile = max_px / columns.max(rows);
        (tile >= MIN_TILE).then_some(Self {
            columns,
            rows,
            tile,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.columns * self.tile, self.rows * self.tile)
    }

    /// The top-left pixel of tile `index`.
    pub fn origin(&self, index: usize) -> (u32, u32) {
        let index = index as u32;
        (
            index % self.columns * self.tile,
            index / self.columns * self.tile,
        )
    }
}

/// The tiles laid out on one sheet, with a thin line between neighbours.
/// A tile of the wrong size is scaled to fit its cell.
pub fn compose(layout: SheetLayout, tiles: &[::image::RgbImage]) -> ::image::RgbImage {
    let (width, height) = layout.size();
    let mut sheet = ::image::RgbImage::new(width, height);
    for (index, tile) in tiles.iter().enumerate() {
        let (x, y) = layout.origin(index);
        let fitted;
        let tile = if tile.dimensions() == (layout.tile, layout.tile) {
            tile
        } else {
            fitted = ::image::imageops::resize(
                tile,
                layout.tile,
                layout.tile,
                ::image::imageops::FilterType::Triangle,
            );
            &fitted
        };
        ::image::imageops::replace(&mut sheet, tile, i64::from(x), i64::from(y));
    }
    let divider = ::image::Rgb(DIVIDER_COLOR);
    for column in 1..layout.columns {
        let x0 = column * layout.tile;
        for x in x0..(x0 + DIVIDER).min(width) {
            for y in 0..height {
                sheet.put_pixel(x, y, divider);
            }
        }
    }
    for row in 1..layout.rows {
        let y0 = row * layout.tile;
        for y in y0..(y0 + DIVIDER).min(height) {
            for x in 0..width {
                sheet.put_pixel(x, y, divider);
            }
        }
    }
    sheet
}

/// One tile to render: what it is called, the target it frames when the
/// sheet has one tile per target, and the camera that sees it.
#[derive(Clone, Debug)]
pub struct TilePlan {
    pub label: String,
    pub target: Option<Entity>,
    pub transform: Transform,
    pub projection: Projection,
}

/// A camera rendering one tile of a views request.
#[derive(Component)]
pub struct ViewTile {
    pub request: u64,
    pub index: usize,
}

/// Tiles read back, by request and index, until the request collects them.
#[derive(Resource, Default)]
pub struct ViewCaptures {
    next: u64,
    landed: HashMap<(u64, usize), ::image::RgbImage>,
    /// Requests with a tile read back before the renderer was ready.
    late: bevy::platform::collections::HashSet<u64>,
}

impl ViewCaptures {
    /// Hold a read-back tile until its request collects it.
    /// `late` marks a tile read back before the renderer was ready.
    pub(crate) fn land(&mut self, request: u64, index: usize, image: &Image, late: bool) {
        if late {
            self.late.insert(request);
        }
        let tile = match image.clone().try_into_dynamic() {
            Ok(dynamic) => dynamic.to_rgb8(),
            Err(err) => {
                error!("views: cannot convert a captured tile: {err}");
                ::image::RgbImage::new(1, 1)
            }
        };
        self.landed.insert((request, index), tile);
    }
}

/// The render layer an isolated request draws on, the targets whose meshes
/// carry it, and what each entity it was added to carried before. Despawning
/// the entity holding it frees the layer.
#[derive(Component)]
struct IsolationLayer {
    layer: usize,
    targets: Vec<Entity>,
    before: EntityHashMap<Option<RenderLayers>>,
}

/// The isolation a request set up, to be taken down by [`finish`].
pub struct Isolation {
    holder: Entity,
}

/// Spawn one offscreen camera per plan, each rendering an `edge` pixel square.
/// With `isolate`, the cameras see only those entities' meshes and the
/// scene's lights. Returns the request id the tiles land under.
pub fn start(
    world: &mut World,
    plans: &[TilePlan],
    edge: u32,
    isolate: Option<&[Entity]>,
) -> (u64, Option<Isolation>) {
    let request = {
        let mut captures = world.get_resource_or_init::<ViewCaptures>();
        captures.next += 1;
        captures.next
    };
    let isolation = isolate.map(|targets| isolate_targets(world, targets));
    let layers = match &isolation {
        Some(isolation) => world
            .get::<IsolationLayer>(isolation.holder)
            .map_or_else(RenderLayers::default, |held| {
                RenderLayers::layer(held.layer)
            }),
        None => RenderLayers::layer(0),
    };
    let look_of = {
        let mut cameras = world.query_filtered::<Entity, With<MainViewportCamera>>();
        crate::screenshot::viewport_camera(world).or_else(|| cameras.iter(world).next())
    };
    for (index, plan) in plans.iter().enumerate() {
        let camera = spawn_offscreen_camera(
            world,
            OffscreenCamera {
                transform: plan.transform,
                projection: plan.projection.clone(),
                layers: layers.clone(),
                width: edge,
                height: edge,
                look_of,
            },
            CaptureSink::ViewTile { request, index },
        );
        world
            .entity_mut(camera)
            .insert((Name::new("Views Camera"), ViewTile { request, index }));
    }
    (request, isolation)
}

/// Reserve a render layer and put it on the targets' meshes and the scene's
/// lights.
fn isolate_targets(world: &mut World, targets: &[Entity]) -> Isolation {
    let reserved = world.get_resource_or_init::<ViewportLayerCounter>().next();
    let layer = reserved.get_layer();
    let holder = world
        .spawn((
            crate::EditorEntity,
            IsolationLayer {
                layer,
                targets: targets.to_vec(),
                before: EntityHashMap::default(),
            },
            reserved,
        ))
        .id();
    let mut lights = world.query_filtered::<(Entity, Option<&RenderLayers>), (
        Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>,
        Without<crate::EditorEntity>,
    )>();
    let scene = RenderLayers::layer(0);
    let lit: Vec<Entity> = lights
        .iter(world)
        .filter(|(_, layers)| layers.is_none_or(|layers| layers.intersects(&scene)))
        .map(|(entity, _)| entity)
        .collect();
    let mut drawn = target_meshes(world, targets);
    drawn.extend(lit);
    add_layer(world, holder, drawn);
    Isolation { holder }
}

/// Every mesh at or under `targets`, each once.
fn target_meshes(world: &World, targets: &[Entity]) -> EntityHashSet {
    let mut meshes = EntityHashSet::default();
    let mut stack: Vec<Entity> = targets.to_vec();
    while let Some(at) = stack.pop() {
        if world.get::<Mesh3d>(at).is_some() {
            meshes.insert(at);
        }
        if let Some(children) = world.get::<Children>(at) {
            stack.extend(children.iter());
        }
    }
    meshes
}

/// Add `holder`'s layer to each of `entities` that lacks it, remembering what
/// an entity carried the first time it is touched.
fn add_layer(world: &mut World, holder: Entity, entities: impl IntoIterator<Item = Entity>) {
    let Some(layer) = world.get::<IsolationLayer>(holder).map(|held| held.layer) else {
        return;
    };
    let mut first_seen = Vec::new();
    for entity in entities {
        let before = world.get::<RenderLayers>(entity).cloned();
        if before
            .as_ref()
            .is_some_and(|layers| layers.intersects(&RenderLayers::layer(layer)))
        {
            continue;
        }
        let joined = before.clone().unwrap_or_default().with(layer);
        if let Ok(mut touched) = world.get_entity_mut(entity) {
            touched.insert(joined);
            first_seen.push((entity, before));
        }
    }
    if let Some(mut held) = world.get_mut::<IsolationLayer>(holder) {
        for (entity, before) in first_seen {
            held.before.entry(entity).or_insert(before);
        }
    }
}

/// Keep the isolation layer on the targets' meshes while the tiles settle:
/// a brush rebuild replaces its chunk meshes with new entities.
fn follow_isolated_meshes(world: &mut World) {
    let mut holders = world.query::<(Entity, &IsolationLayer)>();
    let held: Vec<(Entity, Vec<Entity>)> = holders
        .iter(world)
        .map(|(holder, held)| (holder, held.targets.clone()))
        .collect();
    for (holder, targets) in held {
        let meshes = target_meshes(world, &targets);
        add_layer(world, holder, meshes);
    }
}

/// The tiles of `request`, once all `count` have landed; they leave the log.
pub fn take_landed(
    world: &mut World,
    request: u64,
    count: usize,
) -> Option<Vec<::image::RgbImage>> {
    let mut captures = world.get_resource_or_init::<ViewCaptures>();
    if !(0..count).all(|index| captures.landed.contains_key(&(request, index))) {
        return None;
    }
    (0..count)
        .map(|index| captures.landed.remove(&(request, index)))
        .collect()
}

/// Whether any tile of `request` was read back before the renderer finished
/// compiling, and so may be blank.
pub fn was_late(world: &mut World, request: u64) -> bool {
    world
        .get_resource_or_init::<ViewCaptures>()
        .late
        .contains(&request)
}

/// End `request`: despawn any tile cameras still waiting, drop tiles that
/// landed, and take the isolation layer off every entity carrying it.
pub fn finish(world: &mut World, request: u64, isolation: Option<Isolation>) {
    let mut tiles = world.query::<(Entity, &ViewTile)>();
    let waiting: Vec<Entity> = tiles
        .iter(world)
        .filter(|(_, tile)| tile.request == request)
        .map(|(entity, _)| entity)
        .collect();
    for camera in waiting {
        world.despawn(camera);
    }
    world
        .get_resource_or_init::<ViewCaptures>()
        .landed
        .retain(|(held, _), _| *held != request);
    world
        .get_resource_or_init::<ViewCaptures>()
        .late
        .remove(&request);
    let Some(isolation) = isolation else {
        return;
    };
    let Some(held) = world.get::<IsolationLayer>(isolation.holder) else {
        return;
    };
    let layer = RenderLayers::layer(held.layer);
    let mut before = held.before.clone();
    let mut carriers = world.query::<(Entity, &RenderLayers)>();
    let carrying: Vec<(Entity, RenderLayers)> = carriers
        .iter(world)
        .filter(|(entity, layers)| *entity != isolation.holder && layers.intersects(&layer))
        .map(|(entity, layers)| (entity, layers.clone()))
        .collect();
    for (entity, layers) in carrying {
        let mut entity_mut = world.entity_mut(entity);
        match before.remove(&entity) {
            Some(Some(was)) => {
                entity_mut.insert(was);
            }
            Some(None) => {
                entity_mut.remove::<RenderLayers>();
            }
            None => {
                entity_mut.insert(layers.symmetric_difference(&layer));
            }
        }
    }
    world.despawn(isolation.holder);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_tiles_fill_a_square_sheet() {
        let layout = SheetLayout::new(4, None, 512).expect("fits");
        assert_eq!(
            layout,
            SheetLayout {
                columns: 2,
                rows: 2,
                tile: 256
            }
        );
        assert_eq!(layout.size(), (512, 512));
        assert_eq!(layout.origin(3), (256, 256));
    }

    #[test]
    fn rows_of_angles_keep_their_columns() {
        let layout = SheetLayout::new(6, Some(2), 512).expect("fits");
        assert_eq!((layout.columns, layout.rows, layout.tile), (2, 3, 170));
        assert_eq!(
            SheetLayout::new(5, None, 512).map(|l| (l.columns, l.rows)),
            Some((3, 2))
        );
        assert!(SheetLayout::new(16, None, 32).is_none());
        assert!(SheetLayout::new(0, None, 512).is_none());
    }

    #[test]
    fn a_sheet_places_each_tile_in_its_cell() {
        let layout = SheetLayout::new(2, None, 64).expect("fits");
        let red = ::image::RgbImage::from_pixel(32, 32, ::image::Rgb([255, 0, 0]));
        let blue = ::image::RgbImage::from_pixel(10, 10, ::image::Rgb([0, 0, 255]));
        let sheet = compose(layout, &[red, blue]);
        assert_eq!(sheet.dimensions(), (64, 32));
        assert_eq!(sheet.get_pixel(5, 5).0, [255, 0, 0]);
        assert_eq!(sheet.get_pixel(50, 20).0, [0, 0, 255]);
        assert_eq!(sheet.get_pixel(32, 5).0, DIVIDER_COLOR);
    }

    /// Every corner of the box is inside the view with the padding kept
    /// clear, the widest corner sits right at the padding, and the far side
    /// stays inside the clip range.
    #[test]
    fn axis_views_frame_the_whole_box() {
        let bounds = (Vec3::new(-1.0, 0.0, -2.0), Vec3::new(3.0, 2.0, 1.0));
        for angle in [
            ViewAngle::Front,
            ViewAngle::Right,
            ViewAngle::Top,
            ViewAngle::Back,
        ] {
            let (transform, projection) = frame(angle, bounds);
            let Projection::Orthographic(ortho) = projection else {
                panic!("{angle:?} is not orthographic");
            };
            let ScalingMode::FixedVertical { viewport_height } = ortho.scaling_mode else {
                panic!("unexpected scaling");
            };
            let inner = viewport_height * 0.5 * (1.0 - 2.0 * PADDING) + 1e-4;
            let to_view = transform.compute_affine().inverse();
            let mut widest = 0.0f32;
            for corner in corners(bounds.0, bounds.1) {
                let local = to_view.transform_point3(corner);
                widest = widest.max(local.x.abs()).max(local.y.abs());
                assert!(local.x.abs() <= inner, "{angle:?} {local}");
                assert!(local.y.abs() <= inner, "{angle:?} {local}");
                assert!(
                    -local.z > ortho.near && -local.z < ortho.far,
                    "{angle:?} {local}"
                );
            }
        }
    }

    #[test]
    fn the_iso_view_sees_the_bounding_sphere() {
        let bounds = (Vec3::new(10.0, 0.0, 10.0), Vec3::new(14.0, 6.0, 12.0));
        let (transform, projection) = frame(ViewAngle::Iso, bounds);
        let Projection::Perspective(perspective) = projection else {
            panic!("iso is perspective");
        };
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = (bounds.1 - bounds.0).length() * 0.5;
        let distance = transform.translation.distance(center);
        let fitted = radius / distance;
        assert!((fitted - padded_half_angle(perspective.fov).sin()).abs() < 1e-4);
        assert!(padded_half_angle(perspective.fov) < perspective.fov * 0.5);
        assert!(
            (transform
                .forward()
                .dot((center - transform.translation).normalize())
                - 1.0)
                .abs()
                < 1e-4
        );
    }

    #[test]
    fn an_eye_view_looks_at_the_centre_unless_told() {
        let bounds = (Vec3::ZERO, Vec3::splat(2.0));
        let (transform, _) = frame(
            ViewAngle::Eye {
                eye: Vec3::new(0.0, 10.0, 0.0),
                target: None,
            },
            bounds,
        );
        assert_eq!(transform.translation, Vec3::new(0.0, 10.0, 0.0));
        let toward = (Vec3::ONE - transform.translation).normalize();
        assert!((transform.forward().dot(toward) - 1.0).abs() < 1e-4);
    }
}
