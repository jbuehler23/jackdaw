#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> bevy::app::AppExit {
    jackdaw::launch::run_with(
        jackdaw::launch::LaunchCommands {
            export_terrain: Some(jackdaw_editor_terrain::export::run_export_terrain_cli),
        },
        jackdaw::add_feature_plugins,
    )
}
