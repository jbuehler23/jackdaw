use std::{fs::File, io};

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    window::{PrimaryWindow, RawHandleWrapper},
};
use bevy_rerecast::Navmesh;
use jackdaw_api::prelude::*;
use rfd::{AsyncFileDialog, FileHandle};

use super::{NavmeshHandleRes, NavmeshState, NavmeshStatus};

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<WriteTasks>()
        .init_resource::<ReadTasks>();
    app.add_systems(
        Update,
        (
            poll_save_task.run_if(resource_exists::<SaveTask>),
            poll_write_tasks,
            poll_load_task.run_if(resource_exists::<LoadTask>),
            poll_read_tasks,
        )
            .run_if(in_state(crate::AppState::Editor)),
    );
}

// -- Save --

#[derive(Resource, Deref, DerefMut)]
struct SaveTask(Task<Option<FileHandle>>);

#[derive(Resource, Default, Deref, DerefMut)]
struct WriteTasks(Vec<Task<Result<(), SaveError>>>);

#[derive(Debug)]
enum SaveError {
    CreateFile(io::Error),
    WriteNavmesh(bincode::error::EncodeError),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateFile(e) => write!(f, "Failed to create file: {e}"),
            Self::WriteNavmesh(e) => write!(f, "Failed to encode navmesh: {e}"),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        Self::CreateFile(e)
    }
}

impl From<bincode::error::EncodeError> for SaveError {
    fn from(e: bincode::error::EncodeError) -> Self {
        Self::WriteNavmesh(e)
    }
}

/// Save the baked navmesh to disk.
#[operator(
    id = "navmesh.save",
    label = "Save",
    description = "Save the baked navmesh to disk."
)]
pub fn navmesh_save(
    _: In<OperatorParameters>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) -> OperatorResult {
    let mut dialog = AsyncFileDialog::new()
        .add_filter("Navmesh", &["nav"])
        .set_file_name("navmesh.nav");

    if let Ok(rh) = raw_handle.single() {
        // SAFETY: the primary window is open, so its `RawHandleWrapper`
        // points to a live OS handle. The returned wrapper is only used
        // to parent the modal dialog within this scope.
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }

    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.save_file().await });
    commands.insert_resource(SaveTask(task));
    OperatorResult::Finished
}

fn poll_save_task(
    mut commands: Commands,
    mut task: ResMut<SaveTask>,
    navmesh: Res<NavmeshHandleRes>,
    navmeshes: Res<Assets<Navmesh>>,
    mut write_tasks: ResMut<WriteTasks>,
) {
    let Some(file_handle) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<SaveTask>();
    let Some(file) = file_handle else {
        return;
    };

    let Some(navmesh) = navmeshes.get(navmesh.id()) else {
        warn!("No navmesh to save");
        return;
    };

    let navmesh = navmesh.clone();
    let future = async move {
        let path = file.path();
        let mut file = File::create(path)?;
        let config = bincode::config::standard();
        bincode::serde::encode_into_std_write(navmesh, &mut file, config)?;
        Ok(())
    };
    write_tasks.push(AsyncComputeTaskPool::get().spawn(future));
}

fn poll_write_tasks(mut write_tasks: ResMut<WriteTasks>) {
    write_tasks.retain_mut(|task| {
        let Some(result) = future::block_on(future::poll_once(task)) else {
            return true;
        };
        match result {
            Ok(()) => {
                info!("Navmesh saved");
                false
            }
            Err(err) => {
                error!("Failed to save navmesh: {err}");
                false
            }
        }
    });
}

// -- Load --

#[derive(Resource, Deref, DerefMut)]
struct LoadTask(Task<Option<FileHandle>>);

#[derive(Resource, Default, Deref, DerefMut)]
struct ReadTasks(Vec<Task<Result<Navmesh, LoadError>>>);

#[derive(Debug)]
enum LoadError {
    OpenFile(io::Error),
    ReadNavmesh(bincode::error::DecodeError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFile(e) => write!(f, "Failed to open file: {e}"),
            Self::ReadNavmesh(e) => write!(f, "Failed to decode navmesh: {e}"),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(e: io::Error) -> Self {
        Self::OpenFile(e)
    }
}

impl From<bincode::error::DecodeError> for LoadError {
    fn from(e: bincode::error::DecodeError) -> Self {
        Self::ReadNavmesh(e)
    }
}

/// Load a navmesh from disk.
#[operator(
    id = "navmesh.load",
    label = "Load",
    description = "Load a navmesh from disk."
)]
pub fn navmesh_load(
    _: In<OperatorParameters>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) -> OperatorResult {
    let mut dialog = AsyncFileDialog::new().add_filter("Navmesh", &["nav"]);

    if let Ok(rh) = raw_handle.single() {
        // SAFETY: the primary window is open, so its `RawHandleWrapper`
        // points to a live OS handle. The returned wrapper is only used
        // to parent the modal dialog within this scope.
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }

    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    commands.insert_resource(LoadTask(task));
    OperatorResult::Finished
}

fn poll_load_task(
    mut commands: Commands,
    mut task: ResMut<LoadTask>,
    mut read_tasks: ResMut<ReadTasks>,
) {
    let Some(file_handle) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<LoadTask>();
    let Some(file) = file_handle else {
        return;
    };

    let future = async move {
        let path = file.path();
        let mut file = File::open(path)?;
        let config = bincode::config::standard();
        let content: Navmesh = bincode::serde::decode_from_std_read(&mut file, config)?;
        Ok(content)
    };
    read_tasks.push(AsyncComputeTaskPool::get().spawn(future));
}

fn poll_read_tasks(
    mut read_tasks: ResMut<ReadTasks>,
    mut commands: Commands,
    mut navmeshes: ResMut<Assets<Navmesh>>,
    mut regions: Query<&mut jackdaw_jsn::NavmeshRegion>,
    mut state: ResMut<NavmeshState>,
) {
    read_tasks.retain_mut(|task| {
        let Some(result) = future::block_on(future::poll_once(task)) else {
            return true;
        };
        match result {
            Ok(navmesh) => {
                // Update NavmeshRegion from loaded settings
                if let Some(mut region) = regions.iter_mut().next() {
                    let s = &navmesh.settings;
                    region.agent_radius = s.agent_radius;
                    region.agent_height = s.agent_height;
                    region.walkable_climb = s.walkable_climb;
                    region.walkable_slope_degrees = s.walkable_slope_angle.to_degrees();
                    region.cell_size_fraction = s.cell_size_fraction;
                    region.cell_height_fraction = s.cell_height_fraction;
                    region.min_region_size = s.min_region_size;
                    region.merge_region_size = s.merge_region_size;
                    region.max_simplification_error = s.max_simplification_error;
                    region.max_vertices_per_polygon = s.max_vertices_per_polygon;
                    region.edge_max_len_factor = s.edge_max_len_factor;
                    region.detail_sample_dist = s.detail_sample_dist;
                    region.detail_sample_max_error = s.detail_sample_max_error;
                    region.tiling = s.tiling;
                    region.tile_size = s.tile_size;
                }
                commands.insert_resource(NavmeshHandleRes(navmeshes.add(navmesh)));
                state.status = NavmeshStatus::Ready;
                info!("Navmesh loaded");
                false
            }
            Err(err) => {
                error!("Failed to load navmesh: {err}");
                state.status = NavmeshStatus::Error(err.to_string());
                false
            }
        }
    });
}
