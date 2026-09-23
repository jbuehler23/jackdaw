//! The material component a mesh entity wears.
//!
//! A mesh wears its material on a `MeshMaterial3d<M>`, one component type per
//! material type, so putting a layered surface on a mesh that wore a standard
//! one is a component swap rather than a handle write. Everything that hands a
//! material around - the inspector's asset row, the apply operator, the
//! Materials panel - passes a [`WornMaterial`] so neither side has to know
//! which kind it holds.

use crate::{FoliageMaterial, LayeredSurfaceMaterial, WaterMaterial};
use bevy::asset::UntypedHandle;
use bevy::prelude::*;

/// Reflect type path of the component a mesh wears a standard material on.
pub const STANDARD_MATERIAL_COMPONENT: &str =
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>";

/// The material a mesh entity wears, whichever kind holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum WornMaterial {
    Standard(Handle<StandardMaterial>),
    Layered(Handle<LayeredSurfaceMaterial>),
    Foliage(Handle<FoliageMaterial>),
    Water(Handle<WaterMaterial>),
}

impl WornMaterial {
    /// The material the entity is wearing, if it wears one.
    pub fn of(world: &World, entity: Entity) -> Option<Self> {
        if let Some(standard) = world.get::<MeshMaterial3d<StandardMaterial>>(entity) {
            return Some(Self::Standard(standard.0.clone()));
        }
        if let Some(layered) = world.get::<MeshMaterial3d<LayeredSurfaceMaterial>>(entity) {
            return Some(Self::Layered(layered.0.clone()));
        }
        if let Some(foliage) = world.get::<MeshMaterial3d<FoliageMaterial>>(entity) {
            return Some(Self::Foliage(foliage.0.clone()));
        }
        world
            .get::<MeshMaterial3d<WaterMaterial>>(entity)
            .map(|water| Self::Water(water.0.clone()))
    }

    /// The material a handle holds, if it holds one of a kind a mesh can wear.
    pub fn of_handle(handle: UntypedHandle) -> Option<Self> {
        if let Ok(standard) = handle.clone().try_typed::<StandardMaterial>() {
            return Some(Self::Standard(standard));
        }
        if let Ok(layered) = handle.clone().try_typed::<LayeredSurfaceMaterial>() {
            return Some(Self::Layered(layered));
        }
        if let Ok(foliage) = handle.clone().try_typed::<FoliageMaterial>() {
            return Some(Self::Foliage(foliage));
        }
        handle.try_typed::<WaterMaterial>().ok().map(Self::Water)
    }

    /// Put this material on an entity, taking off the one it wore.
    pub fn wear(&self, world: &mut World, entity: Entity) {
        let Ok(mut node) = world.get_entity_mut(entity) else {
            return;
        };
        node.remove::<MeshMaterial3d<StandardMaterial>>();
        node.remove::<MeshMaterial3d<LayeredSurfaceMaterial>>();
        node.remove::<MeshMaterial3d<FoliageMaterial>>();
        node.remove::<MeshMaterial3d<WaterMaterial>>();
        match self {
            Self::Standard(handle) => node.insert(MeshMaterial3d(handle.clone())),
            Self::Layered(handle) => node.insert(MeshMaterial3d(handle.clone())),
            Self::Foliage(handle) => node.insert(MeshMaterial3d(handle.clone())),
            Self::Water(handle) => node.insert(MeshMaterial3d(handle.clone())),
        };
        mark_for_respecialization(world, entity);
    }

    /// The reflect type path of the component this material is worn on.
    pub fn component_type_path(&self) -> &'static str {
        match self {
            Self::Standard(_) => STANDARD_MATERIAL_COMPONENT,
            Self::Layered(_) => layered_material_component(),
            Self::Foliage(_) => foliage_material_component(),
            Self::Water(_) => water_material_component(),
        }
    }

    pub fn untyped(&self) -> UntypedHandle {
        match self {
            Self::Standard(handle) => handle.clone().untyped(),
            Self::Layered(handle) => handle.clone().untyped(),
            Self::Foliage(handle) => handle.clone().untyped(),
            Self::Water(handle) => handle.clone().untyped(),
        }
    }

    /// The standard material this wears, and nothing when it wears another
    /// kind. What the surfaces that only edit standard materials read.
    pub fn standard(&self) -> Option<&Handle<StandardMaterial>> {
        match self {
            Self::Standard(handle) => Some(handle),
            Self::Layered(_) | Self::Foliage(_) | Self::Water(_) => None,
        }
    }

    /// Whether this is the empty handle every mesh starts with.
    pub fn is_default(&self) -> bool {
        match self {
            Self::Standard(handle) => *handle == Handle::default(),
            Self::Layered(handle) => *handle == Handle::default(),
            Self::Foliage(handle) => *handle == Handle::default(),
            Self::Water(handle) => *handle == Handle::default(),
        }
    }
}

/// Tell the renderer this mesh's material changed, whatever part of the frame
/// the change was made in.
///
/// Bevy collects the meshes whose material changed in `PostUpdate` but reads
/// the material itself when it extracts the frame, after `Last`. A swap made
/// between the two, which is where every operator a remote session dispatches
/// runs, reaches the render world with the mesh still held in the render phases
/// against the pipeline its old material was specialized to. Between two
/// materials of one type that only draws a stale material for a frame; between
/// two material types it binds one material's bind group against the other's
/// pipeline, which fails validation and ends the render. Listing the entity
/// here reaches the same frame's extraction, so the mesh leaves the phases with
/// its old material.
///
/// Every material type's list is drained into one set of changed renderables,
/// which the renderer reads without regard to type, so one list carries a swap
/// in either direction.
fn mark_for_respecialization(world: &mut World, entity: Entity) {
    if let Some(mut changed) =
        world.get_resource_mut::<bevy::pbr::EntitiesNeedingSpecialization<StandardMaterial>>()
    {
        changed.changed.push(entity);
    }
}

/// Reflect type path of the component a mesh wears a layered surface on.
pub fn layered_material_component() -> &'static str {
    use bevy::reflect::TypePath;
    <MeshMaterial3d<LayeredSurfaceMaterial> as TypePath>::type_path()
}

/// Reflect type path of the component a mesh wears a foliage material on.
pub fn foliage_material_component() -> &'static str {
    use bevy::reflect::TypePath;
    <MeshMaterial3d<FoliageMaterial> as TypePath>::type_path()
}

/// Reflect type path of the component a mesh wears a water material on.
pub fn water_material_component() -> &'static str {
    use bevy::reflect::TypePath;
    <MeshMaterial3d<WaterMaterial> as TypePath>::type_path()
}

/// The component type paths a mesh can wear a material on.
pub fn material_component_paths() -> [&'static str; 4] {
    [
        STANDARD_MATERIAL_COMPONENT,
        layered_material_component(),
        foliage_material_component(),
        water_material_component(),
    ]
}
