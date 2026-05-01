use std::path::PathBuf;

use crate::EditorEntity;
use crate::project::ProjectRoot;
use bevy::prelude::*;
use jackdaw_feathers::icons::{EditorFont, Icon, IconFont, icon};
use jackdaw_feathers::picker::{
    Matchable, PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text, picker_item,
};
use jackdaw_feathers::tokens;

#[derive(Component)]
struct PrefabPicker;

struct PrefabPickerEntry {
    pub display_name: String,
    pub path: String,
}

/// Open (or close, if already open) the prefab picker overlay.
pub fn open_prefab_picker(world: &mut World) {
    // Toggle: if picker already open, close it
    let existing: Vec<Entity> = world
        .query_filtered::<Entity, With<PrefabPicker>>()
        .iter(world)
        .collect();
    if !existing.is_empty() {
        for e in existing {
            if let Ok(ec) = world.get_entity_mut(e) {
                ec.despawn();
            }
        }
        return;
    }

    // Scan for .jsn files
    let assets_dir = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(super::project::ProjectRoot::assets_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));

    let mut prefabs: Vec<PrefabPickerEntry> = Vec::new();
    scan_jsn_files(&assets_dir, &assets_dir, &mut prefabs);
    let picker = PickerProps::new(spawn_item, on_select)
        .items(prefabs)
        .title("Select Prefab")
        .placeholder(Some("Search Prefabs..."));

    // Spawn picker
    let mut commands = world.commands();
    commands.spawn((PrefabPicker, crate::BlocksCameraInput, EditorEntity, picker));
}

/// Recursively scan a directory for .jsn scene files.
fn scan_jsn_files(dir: &PathBuf, _assets_root: &PathBuf, results: &mut Vec<PrefabPickerEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        warn!("Prefab picker: failed to read directory {:?}", dir);
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_jsn_files(&path, _assets_root, results);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jsn"))
        {
            // Skip project.jsn files, they aren't scenes.
            if path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("project.jsn"))
            {
                continue;
            }

            let path_str = path.to_string_lossy().to_string();

            // Try to read metadata name from the file without deserializing the
            // entire scene (which can be very large for complex scenes).
            let display_name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .and_then(|v| {
                    v.get("metadata")?
                        .get("name")?
                        .as_str()
                        .map(std::string::ToString::to_string)
                })
                .filter(|name| !name.is_empty() && name != "Untitled")
                .unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                });

            info!("Prefab picker: found {:?} -> {:?}", path_str, display_name);
            results.push(PrefabPickerEntry {
                display_name,
                path: path_str,
            });
        }
    }
}

fn spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    items: Query<&PickerItems<PrefabPickerEntry>>,
    project_root: Option<Res<ProjectRoot>>,
    font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    mut commands: Commands,
) -> Result {
    let item = items.get(entities.picker)?.at(matched.index)?;
    let path = if let Some(project_root) = project_root {
        project_root
            .to_relative(item.path.clone())
            .to_string_lossy()
            .to_string()
    } else {
        item.path.clone()
    };

    commands.entity(entities.list).with_child((
        picker_item(matched.index),
        children![(
            Node {
                row_gap: px(tokens::SPACING_SM),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            children![
                (
                    Node {
                        column_gap: px(tokens::SPACING_SM),
                        ..default()
                    },
                    children![
                        // Icon
                        (icon(Icon::Blocks, tokens::TEXT_SIZE, icon_font.0.clone())),
                        // Display name
                        (match_text(matched.segments))
                    ]
                ),
                (
                    // Path
                    Text(path),
                    TextFont {
                        font: font.0.clone(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY)
                )
            ],
        )],
    ));

    Ok(())
}

fn on_select(
    input: In<SelectInput>,
    items: Query<&PickerItems<PrefabPickerEntry>>,
    mut commands: Commands,
) -> Result {
    let item = items.get(input.entities.picker)?.at(input.index)?;
    let path = item.path.clone();

    commands.queue(move |world: &mut World| {
        crate::entity_templates::instantiate_jsn_prefab(world, &path, Vec3::ZERO);
    });

    commands.entity(input.entities.picker).try_despawn();

    Ok(())
}

impl Matchable for PrefabPickerEntry {
    fn haystack(&self) -> String {
        self.display_name.clone()
    }
}
