//! Levels of detail by screen size: each [`LodGroup`] level's meshes get a
//! [`VisibilityRange`] converted from its screen height at the field of view
//! of the camera the scene is seen through.

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{VisibilityRange, VisibilitySystems};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use jackdaw_scene_types::model_parts::{ModelParts, add_model_parts, source_path};
use jackdaw_scene_types::{GltfSource, LodGroup};

/// Keeps every [`LodGroup`]'s levels showing at the distances their screen
/// heights stand for.
pub struct LodPlugin;

impl Plugin for LodPlugin {
    fn build(&self, app: &mut App) {
        add_model_parts(app);
        app.init_resource::<LodView>()
            .add_systems(Update, request_first_levels)
            .add_systems(
                PostUpdate,
                (follow_lod_view, place_lod_ranges)
                    .chain()
                    .after(VisibilitySystems::CalculateBounds)
                    .before(VisibilitySystems::CheckVisibility),
            )
            .add_systems(Update, report_levels_that_never_show);
    }
}

/// The view level-of-detail distances are measured for: the tangent of half
/// the vertical field of view of the active 3D camera drawn last. Zero while
/// there is no such camera.
#[derive(Resource, Default, Debug, PartialEq)]
pub struct LodView {
    pub half_fov_tan: f32,
}

fn follow_lod_view(
    cameras: Query<(&Camera, &Projection), With<Camera3d>>,
    mut view: ResMut<LodView>,
) {
    let half_fov_tan = cameras
        .iter()
        .filter(|(camera, _)| camera.is_active)
        .filter_map(|(camera, projection)| match projection {
            Projection::Perspective(perspective) => Some((camera.order, perspective.fov)),
            _ => None,
        })
        .max_by_key(|(order, _)| *order)
        .map_or(0.0, |(_, fov)| (fov / 2.0).tan());
    if (view.half_fov_tan - half_fov_tan).abs() > 1e-5 {
        view.half_fov_tan = half_fov_tan;
    }
}

/// The distance at which an object `size` across covers `screen_height` of
/// the screen; the object is never that small when the height is not
/// positive.
pub fn lod_distance(size: f32, screen_height: f32, half_fov_tan: f32) -> f32 {
    if screen_height <= 0.0 || half_fov_tan <= 0.0 {
        return f32::INFINITY;
    }
    size / (2.0 * screen_height * half_fov_tan)
}

/// The ranges each level of `group` shows over, for an object `size` across.
/// A level with nothing after it to hand over to shows out to any distance.
pub fn lod_ranges(group: &LodGroup, size: f32, half_fov_tan: f32) -> Vec<VisibilityRange> {
    let margin = |distance: f32| {
        if distance.is_finite() {
            let half = distance * group.fade.max(0.0) / 2.0;
            (distance - half).max(0.0)..distance + half
        } else {
            f32::MAX..f32::MAX
        }
    };
    let mut near = 0.0..0.0;
    group
        .levels
        .iter()
        .map(|level| {
            let far = margin(lod_distance(size, level.screen_height, half_fov_tan));
            let range = VisibilityRange {
                start_margin: near.clone(),
                end_margin: far.clone(),
                use_aabb: false,
            };
            near = far;
            range
        })
        .collect()
}

/// Ask for the first level's model of every group that gained or changed
/// levels, which is what a group is measured, baked and collided by without
/// waiting for it to be spawned.
fn request_first_levels(
    groups: Query<&Children, (With<LodGroup>, Or<(Changed<LodGroup>, Changed<Children>)>)>,
    sources: Query<&GltfSource>,
    mut parts: ResMut<ModelParts>,
) {
    for children in &groups {
        if let Some(source) = children.first().and_then(|first| sources.get(*first).ok()) {
            parts.request(&source_path(source));
        }
    }
}

/// Whether level `index` of `group` shows at any distance. A level whose
/// screen height is no smaller than the one before it has nothing left to
/// show over, so it never draws. An index past the last level is not a level
/// and counts as shown.
pub fn level_shows(group: &LodGroup, index: usize) -> bool {
    let (Some(level), Some(before)) = (
        group.levels.get(index),
        index
            .checked_sub(1)
            .and_then(|before| group.levels.get(before)),
    ) else {
        return true;
    };
    lod_distance(1.0, level.screen_height, 1.0) > lod_distance(1.0, before.screen_height, 1.0)
}

/// Whether `entity` is a level of a [`LodGroup`] that never shows at any
/// distance, and so is not worth bringing a model on for.
pub fn is_level_never_shown(world: &World, entity: Entity) -> bool {
    let Some(group) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
        return false;
    };
    let (Some(lod), Some(children)) = (world.get::<LodGroup>(group), world.get::<Children>(group))
    else {
        return false;
    };
    is_child_never_shown(lod, children, entity)
}

/// Whether `child`, one of a [`LodGroup`]'s `children`, is a level that never
/// shows at any distance.
pub fn is_child_never_shown(group: &LodGroup, children: &Children, child: Entity) -> bool {
    children
        .iter()
        .position(|level| level == child)
        .is_some_and(|index| !level_shows(group, index))
}

fn report_levels_that_never_show(
    groups: Query<(Entity, &LodGroup, Option<&Name>), Changed<LodGroup>>,
) {
    let mut hidden = groups
        .iter()
        .filter(|(_, group, _)| (0..group.levels.len()).any(|index| !level_shows(group, index)));
    let Some((entity, _, name)) = hidden.next() else {
        return;
    };
    let others = hidden.count();
    let name = name.map_or_else(|| entity.to_string(), ToString::to_string);
    if others == 0 {
        warn!(
            "LOD group {name} has a level with a screen height no smaller than the level before it; that level never shows and is not placed"
        );
    } else {
        warn!(
            "LOD group {name} and {others} more have a level with a screen height no smaller than the level before it; those levels never show and are not placed"
        );
    }
}

type Groups<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Ref<'static, LodGroup>,
        &'static GlobalTransform,
        Option<&'static Children>,
    ),
>;

type Meshes<'w, 's> = Query<
    'w,
    's,
    (
        &'static GlobalTransform,
        Option<&'static Aabb>,
        Option<&'static mut VisibilityRange>,
    ),
    With<Mesh3d>,
>;

fn place_lod_ranges(
    view: Res<LodView>,
    groups: Groups,
    added: Query<Entity, Added<Mesh3d>>,
    parents: Query<&ChildOf>,
    descendants: Query<&Children>,
    sources: Query<(&GltfSource, &GlobalTransform)>,
    parts: Res<ModelParts>,
    mut unmeasured: Local<HashMap<String, Vec<Entity>>>,
    mut meshes: Meshes,
    mut commands: Commands,
) {
    if view.half_fov_tan <= 0.0 {
        return;
    }
    let mut due: HashSet<Entity> = if view.is_changed() {
        groups.iter().map(|(entity, ..)| entity).collect()
    } else {
        groups
            .iter()
            .filter(|(_, group, ..)| group.is_changed())
            .map(|(entity, ..)| entity)
            .collect()
    };
    for mesh in &added {
        due.extend(
            parents
                .iter_ancestors(mesh)
                .find(|ancestor| groups.contains(*ancestor)),
        );
    }
    for path in parts.settled() {
        if let Some(waiting) = unmeasured.remove(path) {
            due.extend(waiting);
        }
    }

    for entity in due {
        let Ok((_, group, transform, children)) = groups.get(entity) else {
            continue;
        };
        let Some(children) = children else {
            continue;
        };
        let size = if group.size > 0.0 {
            group.size * transform.compute_transform().scale.abs().max_element()
        } else {
            let Some(first) = children.first().copied() else {
                continue;
            };
            match sources.get(first) {
                Ok((source, placed)) => {
                    let path = source_path(source);
                    match parts.get(&path) {
                        Some(model) => placed_extent(&model.bounds, placed),
                        None => {
                            if !parts.failed(&path) {
                                unmeasured.entry(path).or_default().push(entity);
                            }
                            continue;
                        }
                    }
                }
                Err(_) => world_extent(first, &descendants, &meshes).unwrap_or(0.0),
            }
        };
        if size <= 0.0 {
            continue;
        }
        for (level, range) in children
            .iter()
            .zip(lod_ranges(&group, size, view.half_fov_tan))
        {
            for part in std::iter::once(level).chain(descendants.iter_descendants(level)) {
                let Ok((_, _, held)) = meshes.get_mut(part) else {
                    continue;
                };
                match held {
                    Some(mut held) if *held != range => *held = range.clone(),
                    Some(_) => {}
                    None => {
                        commands.entity(part).try_insert(range.clone());
                    }
                }
            }
        }
    }
}

/// The largest side of the world bounds of the meshes under `root`.
fn world_extent(root: Entity, descendants: &Query<&Children>, meshes: &Meshes) -> Option<f32> {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for part in std::iter::once(root).chain(descendants.iter_descendants(root)) {
        let Ok((transform, Some(aabb), _)) = meshes.get(part) else {
            continue;
        };
        for point in corners(aabb).map(|corner| transform.transform_point(corner)) {
            low = low.min(point);
            high = high.max(point);
        }
    }
    low.cmple(high).all().then(|| (high - low).max_element())
}

/// The largest side of `bounds` once placed by `transform`, in world space.
fn placed_extent(bounds: &Aabb, transform: &GlobalTransform) -> f32 {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for point in corners(bounds).map(|corner| transform.transform_point(corner)) {
        low = low.min(point);
        high = high.max(point);
    }
    (high - low).max_element()
}

fn corners(aabb: &Aabb) -> [Vec3; 8] {
    let (center, half) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
    std::array::from_fn(|corner| {
        center
            + half
                * Vec3::new(
                    if corner & 1 == 0 { -1.0 } else { 1.0 },
                    if corner & 2 == 0 { -1.0 } else { 1.0 },
                    if corner & 4 == 0 { -1.0 } else { 1.0 },
                )
    })
}
