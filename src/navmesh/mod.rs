mod brp_client;
mod build;
pub(crate) mod ops;
pub mod save_load;
pub mod toolbar;
mod visualization;

use bevy::prelude::*;
use bevy_rerecast::{TriMeshFromBevyMesh as _, prelude::*, rerecast::TriMesh};

use crate::{EditorEntity, EditorMeta};

pub use toolbar::NavmeshToolbar;
pub use visualization::NavmeshVizConfig;

pub struct NavmeshPlugin;

impl Plugin for NavmeshPlugin {
    fn build(&self, app: &mut App) {
        app.register_type_data::<jackdaw_jsn::NavmeshRegion, crate::ReflectEditorMeta>();
        app.add_plugins(
            NavmeshPlugins::default()
                .build()
                .disable::<bevy_rerecast::debug::NavmeshDebugPlugin>(),
        );
        app.set_navmesh_backend(scene_mesh_backend);
        app.init_resource::<NavmeshObstacles>()
            .init_resource::<NavmeshHandleRes>()
            .init_resource::<NavmeshState>();
        app.add_plugins((
            brp_client::plugin,
            save_load::plugin,
            toolbar::plugin,
            visualization::plugin,
        ));
    }
}

fn scene_mesh_backend(
    input: In<NavmeshSettings>,
    meshes: Res<Assets<Mesh>>,
    mesh_entities: Query<(Entity, &GlobalTransform, &Mesh3d), Without<EditorEntity>>,
    brp_obstacles: Res<NavmeshObstacles>,
) -> TriMesh {
    let mut result = brp_obstacles.0.clone();
    for (entity, global_tf, mesh_handle) in mesh_entities.iter() {
        if input.filter.as_ref().is_some_and(|f| !f.contains(&entity)) {
            continue;
        }
        let Some(mesh) = meshes.get(mesh_handle) else {
            continue;
        };
        let transform = global_tf.compute_transform();
        let transformed = mesh.clone().transformed_by(transform);
        if let Some(tri) = TriMesh::from_mesh(&transformed) {
            result.extend(tri);
        }
    }
    result
}

#[derive(Resource, Deref, DerefMut, Default)]
pub struct NavmeshObstacles(pub TriMesh);

#[derive(Resource, Default, Deref, DerefMut)]
pub struct NavmeshHandleRes(pub Handle<Navmesh>);

#[derive(Resource, Default)]
pub struct NavmeshState {
    pub status: NavmeshStatus,
}

#[derive(Default, Clone, Debug)]
pub enum NavmeshStatus {
    #[default]
    Idle,
    FetchingScene,
    Building,
    Ready,
    Error(String),
}

impl std::fmt::Display for NavmeshStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Navmesh: Idle"),
            Self::FetchingScene => write!(f, "Navmesh: Fetching scene..."),
            Self::Building => write!(f, "Navmesh: Building..."),
            Self::Ready => write!(f, "Navmesh: Ready"),
            Self::Error(e) => write!(f, "Navmesh: Error - {e}"),
        }
    }
}

impl EditorMeta for jackdaw_jsn::NavmeshRegion {
    fn category() -> &'static str {
        "Navmesh"
    }
}

pub fn spawn_navmesh_entity(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Name::new("Navmesh"),
            Transform::from_scale(Vec3::splat(10.0)),
            jackdaw_jsn::NavmeshRegion::default(),
        ))
        .id()
}
