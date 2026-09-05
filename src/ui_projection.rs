//! Editor-only projections of authored UI canvases.
//!
//! A Bevy UI root can target only one camera. Jackdaw therefore keeps the
//! authored ECS subtree as the source of truth and creates one derived
//! projection per editor view.

use std::{collections::HashMap, error::Error, fmt};

use bevy::{
    ecs::{
        component::Component,
        entity::EntityCloner,
        hierarchy::{ChildOf, Children},
        reflect::ReflectComponent,
        resource::Resource,
        world::World,
    },
    prelude::{App, Entity, GlobalTransform, InheritedVisibility, Plugin, Reflect, ViewVisibility},
    ui::{CalculatedClip, ComputedNode, ComputedUiTargetCamera, UiGlobalTransform, UiTargetCamera},
};
use jackdaw_bsn::AstNodeRef;
use jackdaw_scene_types::{EditorHidden, SceneNodeId};
use jackdaw_ui::UiCanvas;

use crate::{EditorEntity, NonSerializable};

/// Opaque identity for one view-local projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UiProjectionHandle(u64);

/// Inputs required to create a UI projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiProjectionSpec {
    /// Authored [`UiCanvas`] ECS root.
    pub canvas: Entity,
    /// Camera that should render the projected root.
    pub target_camera: Entity,
}

/// Links a projected entity back to stable authored scene identity.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct ProjectedFrom(pub SceneNodeId);

/// Marks the root entity of a view-local projection.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiProjectionRoot {
    /// Projection that owns this subtree.
    pub handle: UiProjectionHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiProjectionError {
    /// The requested source entity no longer exists.
    MissingCanvas(Entity),
    /// The requested source entity is not a UI canvas.
    NotCanvas(Entity),
    /// The projection handle has already been closed.
    MissingProjection(UiProjectionHandle),
}

impl fmt::Display for UiProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCanvas(entity) => write!(formatter, "UI canvas {entity} does not exist"),
            Self::NotCanvas(entity) => write!(formatter, "entity {entity} is not a UI canvas"),
            Self::MissingProjection(handle) => {
                write!(formatter, "UI projection {handle:?} does not exist")
            }
        }
    }
}

impl Error for UiProjectionError {}

struct ProjectionEntry {
    spec: UiProjectionSpec,
    root: Entity,
    by_node: HashMap<SceneNodeId, Entity>,
}

#[derive(Resource, Default)]
struct UiProjectionStore {
    next_handle: u64,
    entries: HashMap<UiProjectionHandle, ProjectionEntry>,
}

/// Shared projection service used by the UI Canvas and 3D Viewport adapters.
pub struct UiProjection;

impl UiProjection {
    /// Create a view-local clone of an authored canvas.
    pub fn open(
        world: &mut World,
        spec: UiProjectionSpec,
    ) -> Result<UiProjectionHandle, UiProjectionError> {
        if world.get_entity(spec.canvas).is_err() {
            return Err(UiProjectionError::MissingCanvas(spec.canvas));
        }
        if world.get::<UiCanvas>(spec.canvas).is_none() {
            return Err(UiProjectionError::NotCanvas(spec.canvas));
        }

        world.init_resource::<UiProjectionStore>();
        let handle = {
            let mut store = world.resource_mut::<UiProjectionStore>();
            let handle = UiProjectionHandle(store.next_handle);
            store.next_handle += 1;
            handle
        };
        let entry = build_projection(world, handle, spec);
        world
            .resource_mut::<UiProjectionStore>()
            .entries
            .insert(handle, entry);
        Ok(handle)
    }

    /// Rebuild a projection from its latest authored ECS state.
    pub fn refresh(world: &mut World, handle: UiProjectionHandle) -> Result<(), UiProjectionError> {
        let old = world
            .resource_mut::<UiProjectionStore>()
            .entries
            .remove(&handle)
            .ok_or(UiProjectionError::MissingProjection(handle))?;
        if world.get_entity(old.root).is_ok() {
            world.entity_mut(old.root).despawn();
        }
        if world.get_entity(old.spec.canvas).is_err() {
            return Err(UiProjectionError::MissingCanvas(old.spec.canvas));
        }
        if world.get::<UiCanvas>(old.spec.canvas).is_none() {
            return Err(UiProjectionError::NotCanvas(old.spec.canvas));
        }
        let replacement = build_projection(world, handle, old.spec);
        world
            .resource_mut::<UiProjectionStore>()
            .entries
            .insert(handle, replacement);
        Ok(())
    }

    /// Destroy a projection. Other projections of the same canvas are
    /// unaffected.
    pub fn close(world: &mut World, handle: UiProjectionHandle) -> bool {
        let Some(entry) = world
            .get_resource_mut::<UiProjectionStore>()
            .and_then(|mut store| store.entries.remove(&handle))
        else {
            return false;
        };
        if world.get_entity(entry.root).is_ok() {
            world.entity_mut(entry.root).despawn();
        }
        true
    }

    /// Return the projected root entity.
    pub fn root(world: &World, handle: UiProjectionHandle) -> Option<Entity> {
        world
            .get_resource::<UiProjectionStore>()?
            .entries
            .get(&handle)
            .map(|entry| entry.root)
    }

    /// Resolve one stable authored node into this projection.
    pub fn projected_entity(
        world: &World,
        handle: UiProjectionHandle,
        node: SceneNodeId,
    ) -> Option<Entity> {
        world
            .get_resource::<UiProjectionStore>()?
            .entries
            .get(&handle)?
            .by_node
            .get(&node)
            .copied()
    }

    /// Resolve a projected entity, including generated widget internals, to
    /// the nearest authored node.
    pub fn authored_node(world: &World, projected: Entity) -> Option<SceneNodeId> {
        let mut current = Some(projected);
        while let Some(entity) = current {
            if let Some(source) = world.get::<ProjectedFrom>(entity) {
                return Some(source.0);
            }
            current = world.get::<ChildOf>(entity).map(ChildOf::parent);
        }
        None
    }
}

/// Registers projection types and storage for editor applications.
pub struct UiProjectionPlugin;

impl Plugin for UiProjectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiProjectionStore>()
            .register_type::<ProjectedFrom>();
    }
}

fn build_projection(
    world: &mut World,
    handle: UiProjectionHandle,
    spec: UiProjectionSpec,
) -> ProjectionEntry {
    let mut by_node = HashMap::new();
    let root = clone_authored_subtree(world, spec.canvas, None, &mut by_node);
    world.entity_mut(root).insert((
        UiTargetCamera(spec.target_camera),
        UiProjectionRoot { handle },
    ));
    ProjectionEntry {
        spec,
        root,
        by_node,
    }
}

fn clone_authored_subtree(
    world: &mut World,
    source: Entity,
    projected_parent: Option<Entity>,
    by_node: &mut HashMap<SceneNodeId, Entity>,
) -> Entity {
    let children = world
        .get::<Children>(source)
        .map(|children| children.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let node_id = world.get::<SceneNodeId>(source).copied();
    // Spawn already marked and already parented. Editor surfaces that react to
    // `Add<Name>` or `Add<Transform>` see the first component the cloner writes,
    // so a projected entity that becomes editor-visible for even one frame
    // leaves a stale Outliner row behind.
    let mut target = world.spawn((
        EditorEntity,
        EditorHidden,
        NonSerializable,
        jackdaw_ui::UiMaterialize,
    ));
    if let Some(parent) = projected_parent {
        target.insert(ChildOf(parent));
    }
    let target = target.id();

    let mut cloner = {
        let mut builder = EntityCloner::build_opt_out(world);
        builder.without_required_by_components(|builder| {
            builder
                .deny::<ChildOf>()
                .deny::<Children>()
                .deny::<SceneNodeId>()
                .deny::<AstNodeRef>()
                .deny::<UiTargetCamera>()
                .deny::<ComputedNode>()
                .deny::<ComputedUiTargetCamera>()
                .deny::<UiGlobalTransform>()
                .deny::<CalculatedClip>()
                .deny::<GlobalTransform>()
                .deny::<InheritedVisibility>()
                .deny::<ViewVisibility>()
                .deny::<jackdaw_runtime::JackdawSceneMember>();
        });
        builder.finish()
    };
    cloner.clone_entity(world, source, target);
    // EntityCloner intentionally bypasses required-component insertion. Seed
    // fresh hierarchy-derived visibility instead of copying computed source
    // state into a projection.
    world
        .entity_mut(target)
        .insert((InheritedVisibility::default(), ViewVisibility::default()));

    if let Some(node_id) = node_id {
        world.entity_mut(target).insert(ProjectedFrom(node_id));
        by_node.insert(node_id, target);
    }

    for child in children {
        clone_authored_subtree(world, child, Some(target), by_node);
    }
    target
}
