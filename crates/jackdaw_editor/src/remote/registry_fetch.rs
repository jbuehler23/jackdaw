use bevy::{prelude::*, tasks::Task, tasks::futures_lite::future};
use jackdaw_remote::schema::JsnRegistry;

use super::brp;

/// In-flight registry fetch task.
#[derive(Resource)]
pub struct RegistryFetchTask(pub Task<Result<serde_json::Value, anyhow::Error>>);

/// Start fetching the type registry from the connected game.
/// Uses `registry.schema` (built-in BRP method) to get the full Bevy type schema.
pub fn start_registry_fetch(commands: &mut Commands, endpoint: &str) {
    let task = brp::brp_request(endpoint, "registry.schema", None);
    commands.insert_resource(RegistryFetchTask(task));
}

/// Poll the in-flight registry fetch task.
pub fn poll_registry_task(
    mut commands: Commands,
    mut manager: ResMut<super::ConnectionManager>,
    task: Option<ResMut<RegistryFetchTask>>,
    project: Option<Res<crate::project::ProjectRoot>>,
) {
    let Some(mut task) = task else { return };

    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<RegistryFetchTask>();

    match result {
        Ok(schema_value) => {
            // Build a JsnRegistry from the raw schema response
            let app_info = match &manager.state {
                super::ConnectionState::Connected { app_info } => app_info.clone(),
                _ => return,
            };

            // Parse the schema types. registry.schema returns a map of type paths to type defs.
            let types = match schema_value {
                serde_json::Value::Object(map) => map.into_iter().collect(),
                _ => std::collections::HashMap::new(),
            };

            let registry = JsnRegistry {
                jsn: jackdaw_remote::schema::JsnRegistryHeader {
                    format_version: [1, 0, 0],
                },
                extracted_at: timestamp_now(),
                source: jackdaw_remote::schema::JsnRegistrySource {
                    app_name: Some(app_info.app_name),
                    endpoint: manager.endpoint.clone(),
                    bevy_version: app_info.bevy_version,
                },
                types,
                components: std::collections::HashMap::new(),
            };

            info!("Fetched remote registry: {} types", registry.types.len());

            // Cache to disk if project is open
            if let Some(project) = project {
                cache_registry_to_disk(&project.jsn_dir(), &registry);
            }

            manager.registry = Some(registry);
        }
        Err(e) => {
            warn!("Failed to fetch remote registry: {e}");
        }
    }
}

/// Write registry to `.jsn/registry.jsn` for offline caching.
fn cache_registry_to_disk(jsn_dir: &std::path::Path, registry: &JsnRegistry) {
    let _ = std::fs::create_dir_all(jsn_dir);
    let path = jsn_dir.join("registry.jsn");
    match serde_json::to_string_pretty(registry) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, &data) {
                warn!("Failed to cache registry to {}: {e}", path.display());
            } else {
                info!("Cached remote registry to {}", path.display());
            }
        }
        Err(e) => {
            warn!("Failed to serialize registry: {e}");
        }
    }
}

fn timestamp_now() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}
