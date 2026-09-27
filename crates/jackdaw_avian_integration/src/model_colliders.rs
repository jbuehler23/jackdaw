//! Colliders for placed models: an [`AvianCollider`] on an entity whose meshes
//! sit on its descendants, as a glTF model's do, is handed to avian's
//! [`ColliderConstructorHierarchy`], which builds one collider per mesh.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_scene_types::Brush;

use crate::AvianCollider;

/// Builds the colliders of every placed model from its [`AvianCollider`], and
/// rebuilds them when the shape changes or the model's parts arrive.
pub struct ModelCollidersPlugin;

impl Plugin for ModelCollidersPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, build_model_colliders)
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
        (ModelFilter, Or<(Changed<AvianCollider>, Changed<Children>)>),
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
