//! Colliders for placed models: an [`AvianCollider`] on an entity whose meshes
//! sit on its descendants, as a glTF model's do, is handed to avian's
//! [`ColliderConstructorHierarchy`], which builds one collider per mesh.
//!
//! A LOD group collides as its first level, read from that level's model
//! rather than from whichever level is spawned, so its shape never changes
//! with the camera.

use avian3d::prelude::*;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use jackdaw_scene_types::model_parts::{ModelParts, add_model_parts, source_path};
use jackdaw_scene_types::{Brush, GltfSource, LodGroup};

use crate::AvianCollider;

/// Builds the colliders of every placed model from its [`AvianCollider`], and
/// rebuilds them when the shape changes or the model's parts arrive.
pub struct ModelCollidersPlugin;

impl Plugin for ModelCollidersPlugin {
    fn build(&self, app: &mut App) {
        add_model_parts(app);
        app.add_systems(
            PreUpdate,
            (build_model_colliders, build_lod_group_colliders),
        )
        .add_observer(remove_model_colliders);
    }
}

/// An entity whose meshes are its descendants: neither a brush nor a mesh of
/// its own.
type ModelFilter = (Without<Brush>, Without<Mesh3d>);

/// Colliders built on a model's parts: every descendant collider that no
/// [`AvianCollider`] of its own asked for.
type BuiltColliders<'w, 's> = Query<'w, 's, (), (With<Collider>, Without<AvianCollider>)>;

fn build_model_colliders(
    mut commands: Commands,
    models: Query<
        (Entity, &AvianCollider),
        (
            ModelFilter,
            Without<LodGroup>,
            Or<(Changed<AvianCollider>, Changed<Children>)>,
        ),
    >,
    descendants: Query<&Children>,
    built: BuiltColliders,
) {
    for (model, collider) in &models {
        clear_part_colliders(&mut commands, model, &descendants, &built);
        let mut model = commands.entity(model);
        if collider.0.requires_mesh() {
            model
                .try_remove::<Collider>()
                .insert(ColliderConstructorHierarchy::new(collider.0.clone()));
        } else if let Some(shape) = Collider::try_from_constructor(collider.0.clone(), None) {
            model
                .try_remove::<ColliderConstructorHierarchy>()
                .insert(shape);
        }
    }
}

/// Take a model's part colliders off with its [`AvianCollider`], so turning
/// physics off or undoing it leaves nothing behind.
fn remove_model_colliders(
    removed: On<Remove, AvianCollider>,
    mut commands: Commands,
    models: Query<(), ModelFilter>,
    descendants: Query<&Children>,
    built: BuiltColliders,
) {
    let model = removed.event_target();
    if !models.contains(model) {
        return;
    }
    clear_part_colliders(&mut commands, model, &descendants, &built);
    if let Ok(mut model) = commands.get_entity(model) {
        model.try_remove::<ColliderConstructorHierarchy>();
    }
}

fn clear_part_colliders(
    commands: &mut Commands,
    model: Entity,
    descendants: &Query<&Children>,
    built: &BuiltColliders,
) {
    for part in descendants.iter_descendants(model) {
        if built.contains(part) {
            commands.entity(part).try_remove::<Collider>();
        }
    }
}

type LodGroupModels<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static AvianCollider,
        &'static LodGroup,
        &'static Children,
    ),
    (ModelFilter, With<AvianCollider>),
>;

/// Build a LOD group's collider from its first level's model: one shape per
/// part of the model, placed where the level puts it.
fn build_lod_group_colliders(
    mut commands: Commands,
    groups: LodGroupModels,
    changed: Query<
        Entity,
        (
            With<LodGroup>,
            Or<(Changed<AvianCollider>, Changed<LodGroup>, Changed<Children>)>,
        ),
    >,
    levels: Query<(&GltfSource, &Transform)>,
    mut parts: ResMut<ModelParts>,
    meshes: Option<Res<Assets<Mesh>>>,
    descendants: Query<&Children>,
    built: BuiltColliders,
    mut waiting: Local<HashSet<Entity>>,
) {
    let mut due: HashSet<Entity> = changed.iter().collect();
    if !parts.settled().is_empty() {
        due.extend(waiting.drain());
    }
    for group in due {
        let Ok((group, collider, lod, children)) = groups.get(group) else {
            continue;
        };
        clear_part_colliders(&mut commands, group, &descendants, &built);
        let mut entity = commands.entity(group);
        entity.try_remove::<ColliderConstructorHierarchy>();
        if !collider.0.requires_mesh() {
            if let Some(shape) = Collider::try_from_constructor(collider.0.clone(), None) {
                entity.insert(shape);
            }
            continue;
        }
        let Some((source, placed)) = children
            .first()
            .filter(|_| !lod.levels.is_empty())
            .and_then(|first| levels.get(*first).ok())
        else {
            continue;
        };
        let path = source_path(source);
        parts.request(&path);
        let Some(model) = parts.get(&path) else {
            if !parts.failed(&path) {
                waiting.insert(group);
            }
            continue;
        };
        let Some(meshes) = meshes.as_deref() else {
            continue;
        };
        let mut shapes: Vec<Collider> = model
            .parts
            .iter()
            .filter_map(|part| {
                let mesh = meshes
                    .get(&part.mesh)?
                    .clone()
                    .transformed_by(*placed * part.local);
                Collider::try_from_constructor(collider.0.clone(), Some(&mesh))
            })
            .collect();
        let shape = match shapes.len() {
            0 | 1 => shapes.pop(),
            _ => Some(Collider::compound(
                shapes
                    .into_iter()
                    .map(|shape| (Vec3::ZERO, Quat::IDENTITY, shape))
                    .collect(),
            )),
        };
        if let Some(shape) = shape {
            entity.insert(shape);
        }
    }
}
