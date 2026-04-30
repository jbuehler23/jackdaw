use anyhow::anyhow;
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    platform::collections::HashMap,
    prelude::*,
    remote::BrpRequest,
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, futures_lite::future},
};
use bevy_rerecast::editor_integration::{
    brp::{
        BRP_GENERATE_EDITOR_INPUT, BRP_POLL_EDITOR_INPUT, GenerateEditorInputParams,
        GenerateEditorInputResponse, PollEditorInputParams, PollEditorInputResponse,
    },
    transmission::deserialize,
};
use jackdaw_api::prelude::*;

use super::{NavmeshHandleRes, NavmeshObstacles, NavmeshState, NavmeshStatus};
use crate::EditorEntity;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            poll_remote_navmesh_input.run_if(resource_exists::<GetNavmeshInputRequestTask>),
            poll_navmesh_input.run_if(resource_exists::<GetNavmeshInputRequestTask>),
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

#[derive(Resource)]
enum GetNavmeshInputRequestTask {
    Generate {
        task: Task<Result<GenerateEditorInputResponse, anyhow::Error>>,
        url: String,
    },
    Poll(Task<Result<PollEditorInputResponse, anyhow::Error>>),
}

/// Marker for visual meshes fetched from the remote scene.
#[derive(Component)]
pub struct SceneVisualMesh;

/// Marker for obstacle gizmo entity.
#[derive(Component)]
pub struct ObstacleGizmo;

/// Fetch the latest scene mesh from the connected game so the navmesh
/// can be rebuilt from it.
#[operator(
    id = "navmesh.fetch",
    label = "Fetch Scene",
    description = "Fetch the latest scene mesh from the connected game."
)]
pub(crate) fn navmesh_fetch(
    _: In<OperatorParameters>,
    mut commands: Commands,
    regions: Query<&jackdaw_jsn::NavmeshRegion>,
    maybe_task: Option<Res<GetNavmeshInputRequestTask>>,
    mut state: ResMut<NavmeshState>,
) -> OperatorResult {
    if maybe_task.is_some() {
        return OperatorResult::Cancelled;
    }
    let Some(region) = regions.iter().next() else {
        warn!("No NavmeshRegion entity found");
        return OperatorResult::Cancelled;
    };

    let url = region.connection_url.clone();
    let settings = super::build::region_to_settings_without_transform(region);

    state.status = NavmeshStatus::FetchingScene;

    let url_clone = url.clone();
    let future = async move {
        let params = GenerateEditorInputParams {
            backend_input: settings,
        };
        let json = serde_json::to_value(params)?;
        let req = BrpRequest {
            jsonrpc: String::from("2.0"),
            method: String::from(BRP_GENERATE_EDITOR_INPUT),
            id: None,
            params: Some(json),
        };
        let request = ehttp::Request::json(&url_clone, &req)?;
        let resp = ehttp::fetch_async(request)
            .await
            .map_err(|s| anyhow!("{s}"))?;

        let mut v: serde_json::Value = resp.json()?;

        let Some(val) = v.get_mut("result") else {
            let Some(error) = v.get("error") else {
                return Err(anyhow!(
                    "BRP error: Response returned neither 'result' nor 'error' field"
                ));
            };
            return Err(anyhow!("BRP error: {error}"));
        };
        let val = val.take();

        let response: GenerateEditorInputResponse = serde_json::from_value(val)?;
        Ok(response)
    };

    let task = IoTaskPool::get().spawn(future);
    commands.insert_resource(GetNavmeshInputRequestTask::Generate { task, url });
    OperatorResult::Finished
}

fn poll_remote_navmesh_input(
    mut commands: Commands,
    mut task: ResMut<GetNavmeshInputRequestTask>,
    mut state: ResMut<NavmeshState>,
) -> Result {
    let GetNavmeshInputRequestTask::Generate { task, url } = task.as_mut() else {
        return Ok(());
    };
    let Some(result) = future::block_on(future::poll_once(task)) else {
        return Ok(());
    };
    let url = url.clone();
    let response = result.inspect_err(|e| {
        state.status = NavmeshStatus::Error(e.to_string());
        commands.remove_resource::<GetNavmeshInputRequestTask>();
    })?;

    let future = async move {
        let params = PollEditorInputParams { id: response.id };
        let json = serde_json::to_value(params)?;
        let req = BrpRequest {
            jsonrpc: String::from("2.0"),
            method: String::from(BRP_POLL_EDITOR_INPUT),
            id: None,
            params: Some(json),
        };
        let request = ehttp::Request::json(&url, &req)?;
        let resp = ehttp::fetch_async(request)
            .await
            .map_err(|s| anyhow!("{s}"))?;

        let mut v: serde_json::Value = resp.json()?;

        let Some(val) = v.get_mut("result") else {
            let Some(error) = v.get("error") else {
                return Err(anyhow!(
                    "BRP error: Response returned neither 'result' nor 'error' field"
                ));
            };
            return Err(anyhow!("BRP error: {error}"));
        };
        let val = val.take();

        let response: PollEditorInputResponse = deserialize(&val)?;
        Ok(response)
    };

    let task = AsyncComputeTaskPool::get().spawn(future);
    commands.insert_resource(GetNavmeshInputRequestTask::Poll(task));
    Ok(())
}

fn poll_navmesh_input(
    mut task: ResMut<GetNavmeshInputRequestTask>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    existing_visuals: Query<Entity, With<SceneVisualMesh>>,
    existing_obstacles: Query<Entity, With<ObstacleGizmo>>,
    regions: Query<Entity, With<jackdaw_jsn::NavmeshRegion>>,
    mut navmesh_handle: ResMut<NavmeshHandleRes>,
    mut state: ResMut<NavmeshState>,
) -> Result {
    let GetNavmeshInputRequestTask::Poll(task) = task.as_mut() else {
        return Ok(());
    };
    let Some(result) = future::block_on(future::poll_once(task)) else {
        return Ok(());
    };
    commands.remove_resource::<GetNavmeshInputRequestTask>();
    let response = result.inspect_err(|e| {
        state.status = NavmeshStatus::Error(e.to_string());
    })?;

    // Despawn old visual meshes and obstacles
    for entity in existing_visuals.iter() {
        commands.entity(entity).despawn();
    }
    for entity in existing_obstacles.iter() {
        commands.entity(entity).despawn();
    }

    // Spawn obstacle trimesh
    let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all())
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            response.obstacles.vertices.clone(),
        )
        .with_inserted_indices(Indices::U32(
            response
                .obstacles
                .indices
                .iter()
                .flat_map(UVec3::to_array)
                .collect(),
        ))
        .with_computed_normals();

    commands.spawn((
        Transform::default(),
        Mesh3d(meshes.add(mesh)),
        Gizmo {
            handle: gizmo_assets.add(GizmoAsset::default()),
            line_config: GizmoLineConfig {
                width: 1.5,
                perspective: true,
                ..default()
            },
            ..default()
        },
        ObstacleGizmo,
        EditorEntity,
    ));
    // Compute AABB from obstacle vertices and update region entity bounds
    if let Some(region_entity) = regions.iter().next()
        && !response.obstacles.vertices.is_empty()
    {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for v in &response.obstacles.vertices {
            let v = Vec3::from(*v);
            min = min.min(v);
            max = max.max(v);
        }
        let center = (min + max) / 2.0;
        let size = max - min;
        commands.entity(region_entity).insert(Transform {
            translation: center,
            scale: size,
            ..default()
        });
    }

    commands.insert_resource(NavmeshObstacles(response.obstacles));

    // Spawn visual meshes
    let mut image_indices: HashMap<u32, Handle<Image>> = HashMap::new();
    let mut material_indices: HashMap<u32, Handle<StandardMaterial>> = HashMap::new();
    let mut mesh_indices: HashMap<u32, Handle<Mesh>> = HashMap::new();
    let fallback_material = materials.add(Color::WHITE);

    for visual in response.visual_meshes {
        let mesh = if let Some(mesh_handle) = mesh_indices.get(&visual.mesh) {
            mesh_handle.clone()
        } else {
            let serialized_mesh = response.meshes[visual.mesh as usize].clone();
            let mut mesh = serialized_mesh.into_mesh();
            mesh.remove_attribute(Mesh::ATTRIBUTE_JOINT_INDEX);
            mesh.remove_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT);
            let handle = meshes.add(mesh);
            mesh_indices.insert(visual.mesh, handle.clone());
            handle
        };

        let material = if let Some(index) = visual.material {
            if let Some(material_handle) = material_indices.get(&index) {
                material_handle.clone()
            } else {
                let serialized_material = response.materials[index as usize].clone();
                let material = serialized_material.into_standard_material(
                    &mut image_indices,
                    &mut images,
                    &response.images,
                );
                let handle = materials.add(material.clone());
                material_indices.insert(index, handle.clone());
                handle
            }
        } else {
            fallback_material.clone()
        };

        commands.spawn((
            visual.transform.compute_transform(),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            SceneVisualMesh,
            EditorEntity,
        ));
    }

    // Clear previous navmesh
    navmesh_handle.0 = Default::default();
    state.status = NavmeshStatus::Idle;

    Ok(())
}
