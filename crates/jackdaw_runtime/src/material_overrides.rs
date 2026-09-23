//! Dressing a placed model's parts in the materials its [`MaterialOverrides`] names.

use std::collections::BTreeMap;

use bevy::gltf::GltfMaterialName;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use jackdaw_bsn::BsnProjectAssets;
use jackdaw_scene_types::{InstanceMaterialOverrides, MaterialOverrides};
use jackdaw_surface::WornMaterial;

use crate::JackdawCatalog;

/// Dresses the parts under every [`MaterialOverrides`] and [`InstanceMaterialOverrides`] by glTF material name.
pub struct MaterialOverridesPlugin;

impl Plugin for MaterialOverridesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, find_models_to_dress);
    }
}

/// The material a part wore before an override dressed it, put back when the override goes.
#[derive(Component, Clone, Debug)]
pub struct ModelMaterial(pub WornMaterial);

type Overridden = Or<(With<MaterialOverrides>, With<InstanceMaterialOverrides>)>;

fn find_models_to_dress(
    mut commands: Commands,
    changed: Query<
        Entity,
        Or<(
            Changed<MaterialOverrides>,
            Changed<InstanceMaterialOverrides>,
        )>,
    >,
    new_parts: Query<Entity, Added<GltfMaterialName>>,
    overridden: Query<Entity, Overridden>,
    mut removed: RemovedComponents<MaterialOverrides>,
    mut removed_instance: RemovedComponents<InstanceMaterialOverrides>,
    project: Option<Res<BsnProjectAssets>>,
    catalog: Option<Res<JackdawCatalog>>,
) {
    let mut subtrees: HashSet<Entity> = changed.iter().collect();
    subtrees.extend(removed.read());
    subtrees.extend(removed_instance.read());
    let references_changed = project.is_some_and(|project| project.is_changed())
        || catalog.is_some_and(|catalog| catalog.is_changed());
    if references_changed {
        subtrees.extend(overridden.iter());
    }
    let parts: Vec<Entity> = new_parts.iter().collect();
    if subtrees.is_empty() && parts.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for root in subtrees {
            dress_model(world, root);
        }
        for part in parts {
            dress_part(world, part);
        }
    });
}

/// The material a reference names, through the editor's project references or the game's catalog.
pub fn material_of_reference(world: &World, reference: &str) -> Option<WornMaterial> {
    let handle = world
        .get_resource::<BsnProjectAssets>()
        .and_then(|project| project.0.get(reference).cloned())
        .or_else(|| {
            world
                .get_resource::<JackdawCatalog>()
                .and_then(|catalog| catalog.get(reference).cloned())
        })?;
    WornMaterial::of_handle(handle)
}

/// The overrides that reach `part`: the nearest model's own, then each enclosing
/// instance's laid on top, the outermost last so it wins.
pub fn overrides_reaching(world: &World, part: Entity) -> BTreeMap<String, String> {
    let mut model = None;
    let mut instances = Vec::new();
    let mut at = Some(part);
    while let Some(entity) = at {
        if model.is_none()
            && let Some(overrides) = world.get::<MaterialOverrides>(entity)
        {
            model = Some(&overrides.materials);
        }
        if let Some(overrides) = world.get::<InstanceMaterialOverrides>(entity) {
            instances.push(&overrides.materials);
        }
        at = world.get::<ChildOf>(entity).map(ChildOf::parent);
    }
    let mut reaching = model.cloned().unwrap_or_default();
    for layer in instances {
        reaching.extend(
            layer
                .iter()
                .map(|(name, path)| (name.clone(), path.clone())),
        );
    }
    reaching
}

/// The root and every entity under it, depth first.
fn model_parts(world: &World, root: Entity) -> Vec<Entity> {
    let mut parts = Vec::new();
    let mut open = vec![root];
    while let Some(at) = open.pop() {
        parts.push(at);
        if let Some(children) = world.get::<Children>(at) {
            open.extend(children.iter());
        }
    }
    parts
}

/// Dress each part under `root` in the material the overrides reaching it name, and give back its own where none does.
pub fn dress_model(world: &mut World, root: Entity) {
    if world.get_entity(root).is_err() {
        return;
    }
    for part in model_parts(world, root) {
        dress_part(world, part);
    }
}

/// Dress one part in the material the overrides reaching it name, or give it back its own.
pub fn dress_part(world: &mut World, part: Entity) {
    let Some(name) = world
        .get::<GltfMaterialName>(part)
        .map(|name| name.0.clone())
    else {
        return;
    };
    let wanted = overrides_reaching(world, part)
        .get(&name)
        .and_then(|reference| material_of_reference(world, reference));
    let Some(wanted) = wanted else {
        give_back(world, part);
        return;
    };
    let wearing = WornMaterial::of(world, part);
    if world.get::<ModelMaterial>(part).is_none()
        && let Some(own) = wearing.clone()
    {
        world.entity_mut(part).insert(ModelMaterial(own));
    }
    if wearing.as_ref() != Some(&wanted) {
        wanted.wear(world, part);
    }
}

fn give_back(world: &mut World, part: Entity) {
    let Some(ModelMaterial(own)) = world.get::<ModelMaterial>(part).cloned() else {
        return;
    };
    world.entity_mut(part).remove::<ModelMaterial>();
    own.wear(world, part);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::platform::collections::HashMap;
    use jackdaw_surface::{LayeredSurfaceMaterial, WaterMaterial};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<StandardMaterial>()
            .init_asset::<LayeredSurfaceMaterial>()
            .init_asset::<WaterMaterial>()
            .add_plugins(MaterialOverridesPlugin);
        app
    }

    fn reference(app: &mut App, path: &str, handle: impl Into<bevy::asset::UntypedHandle>) {
        let mut references = app
            .world_mut()
            .remove_resource::<BsnProjectAssets>()
            .unwrap_or_else(|| BsnProjectAssets(HashMap::default()));
        references.0.insert(path.to_string(), handle.into());
        app.world_mut().insert_resource(references);
    }

    fn model(app: &mut App, own: &Handle<StandardMaterial>) -> (Entity, Entity, Entity) {
        let world = app.world_mut();
        let rock = world
            .spawn((GltfMaterialName("Rock".into()), MeshMaterial3d(own.clone())))
            .id();
        let moss = world
            .spawn((GltfMaterialName("Moss".into()), MeshMaterial3d(own.clone())))
            .id();
        let root = world
            .spawn(Transform::default())
            .add_children(&[rock, moss])
            .id();
        (root, rock, moss)
    }

    fn overrides(pairs: &[(&str, &str)]) -> MaterialOverrides {
        MaterialOverrides {
            materials: pairs
                .iter()
                .map(|(name, path)| (name.to_string(), path.to_string()))
                .collect(),
        }
    }

    #[test]
    fn an_override_dresses_only_the_parts_its_material_name_matches() {
        let mut app = app();
        let own = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let water = app
            .world_mut()
            .resource_mut::<Assets<WaterMaterial>>()
            .add(WaterMaterial::default());
        reference(&mut app, "materials/lake.bsn", water.clone());
        let (root, rock, moss) = model(&mut app, &own);

        app.world_mut()
            .entity_mut(root)
            .insert(overrides(&[("Rock", "materials/lake.bsn")]));
        app.update();

        assert_eq!(
            WornMaterial::of(app.world(), rock),
            Some(WornMaterial::Water(water))
        );
        assert_eq!(
            WornMaterial::of(app.world(), moss),
            Some(WornMaterial::Standard(own))
        );
    }

    #[test]
    fn removing_the_overrides_gives_each_part_its_own_material_back() {
        let mut app = app();
        let own = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let layered = app
            .world_mut()
            .resource_mut::<Assets<LayeredSurfaceMaterial>>()
            .add(LayeredSurfaceMaterial::default());
        reference(&mut app, "materials/cliff.bsn", layered);
        let (root, rock, _) = model(&mut app, &own);
        app.world_mut()
            .entity_mut(root)
            .insert(overrides(&[("Rock", "materials/cliff.bsn")]));
        app.update();

        app.world_mut()
            .entity_mut(root)
            .remove::<MaterialOverrides>();
        app.update();

        assert_eq!(
            WornMaterial::of(app.world(), rock),
            Some(WornMaterial::Standard(own))
        );
        assert!(app.world().get::<ModelMaterial>(rock).is_none());
    }

    #[test]
    fn parts_that_arrive_after_the_overrides_are_dressed_too() {
        let mut app = app();
        let own = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let water = app
            .world_mut()
            .resource_mut::<Assets<WaterMaterial>>()
            .add(WaterMaterial::default());
        reference(&mut app, "materials/lake.bsn", water.clone());
        let root = app
            .world_mut()
            .spawn(overrides(&[("Surface", "materials/lake.bsn")]))
            .id();
        app.update();

        let part = app
            .world_mut()
            .spawn((
                GltfMaterialName("Surface".into()),
                MeshMaterial3d(own),
                ChildOf(root),
            ))
            .id();
        app.update();

        assert_eq!(
            WornMaterial::of(app.world(), part),
            Some(WornMaterial::Water(water))
        );
    }

    #[test]
    fn an_instances_overrides_lay_over_its_models_and_leave_the_rest_to_it() {
        let mut app = app();
        let own = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let lake = app
            .world_mut()
            .resource_mut::<Assets<WaterMaterial>>()
            .add(WaterMaterial::default());
        let cliff = app
            .world_mut()
            .resource_mut::<Assets<LayeredSurfaceMaterial>>()
            .add(LayeredSurfaceMaterial::default());
        reference(&mut app, "materials/lake.bsn", lake.clone());
        reference(&mut app, "materials/cliff.bsn", cliff.clone());
        let (model, rock, moss) = model(&mut app, &own);
        app.world_mut().entity_mut(model).insert(overrides(&[
            ("Rock", "materials/cliff.bsn"),
            ("Moss", "materials/cliff.bsn"),
        ]));
        let instance = app
            .world_mut()
            .spawn(InstanceMaterialOverrides {
                materials: [("Rock".to_string(), "materials/lake.bsn".to_string())]
                    .into_iter()
                    .collect(),
            })
            .add_child(model)
            .id();
        app.update();

        assert_eq!(
            WornMaterial::of(app.world(), rock),
            Some(WornMaterial::Water(lake))
        );
        assert_eq!(
            WornMaterial::of(app.world(), moss),
            Some(WornMaterial::Layered(cliff.clone()))
        );

        app.world_mut()
            .entity_mut(instance)
            .remove::<InstanceMaterialOverrides>();
        app.update();
        assert_eq!(
            WornMaterial::of(app.world(), rock),
            Some(WornMaterial::Layered(cliff)),
            "without its own entry the instance falls back to the model's"
        );
    }

    #[test]
    fn a_reference_that_resolves_later_dresses_the_part_when_it_does() {
        let mut app = app();
        let own = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let (root, rock, _) = model(&mut app, &own);
        app.world_mut()
            .entity_mut(root)
            .insert(overrides(&[("Rock", "materials/late.bsn")]));
        app.update();
        assert_eq!(
            WornMaterial::of(app.world(), rock),
            Some(WornMaterial::Standard(own))
        );

        let late = app
            .world_mut()
            .resource_mut::<Assets<WaterMaterial>>()
            .add(WaterMaterial::default());
        reference(&mut app, "materials/late.bsn", late.clone());
        app.update();

        assert_eq!(
            WornMaterial::of(app.world(), rock),
            Some(WornMaterial::Water(late))
        );
    }
}
