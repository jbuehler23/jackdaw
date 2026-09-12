//! Creating an asset file from wherever the user is: a folder in the asset
//! browser or the project files tree, an entry in the Add menu, or the New
//! beside an asset field.
//!
//! The user never picks a file flavour. One list offers every kind the editor
//! can write a default value for, searchable by its name and by the type it
//! holds, and picking one writes `<kind>_<n>.bsn` in the folder the list was
//! opened on and puts its card in the inspector.

use std::path::{Path, PathBuf};

use bevy::asset::ReflectAsset;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use bevy::reflect::prelude::ReflectDefault;
use jackdaw_api::prelude::{AssetKind, AssetKinds};
use jackdaw_feathers::picker::{PickerProps, SelectInput, SpawnItemInput, match_text, picker_item};

use crate::prelude::*;

/// What the context-menu item that opens the list of kinds reads as. Each
/// panel names the action itself, since every panel sees every action.
pub const NEW_ASSET_LABEL: &str = "New Asset...";

/// The list of kinds a folder's New Asset put up, and where its choice lands.
#[derive(Component)]
pub struct NewAssetList {
    folder: PathBuf,
    /// The kind each row stands for, in the order the rows were built.
    kinds: Vec<String>,
}

/// The kinds the editor can write a default value of, in the order a list
/// shows them.
pub fn creatable_kinds(world: &World) -> Vec<AssetKind> {
    let Some(kinds) = world.get_resource::<AssetKinds>() else {
        return Vec::new();
    };
    let Some(registry) = world.get_resource::<AppTypeRegistry>() else {
        return Vec::new();
    };
    let registry = registry.read();
    let mut creatable: Vec<AssetKind> = kinds
        .iter()
        .filter(|kind| has_default_value(kind, &registry))
        .cloned()
        .collect();
    creatable.sort_by(|one, other| one.label.cmp(&other.label));
    creatable
}

/// Whether a fresh file of this kind can be written: a schema kind holds a
/// patch that authors nothing, and a compiled one needs a default value its
/// store can take.
fn has_default_value(kind: &AssetKind, registry: &TypeRegistry) -> bool {
    if kind.schema_backed() {
        return true;
    }
    registry
        .get_with_type_path(&kind.type_path)
        .is_some_and(|registration| {
            registration.data::<ReflectAsset>().is_some()
                && registration.data::<ReflectDefault>().is_some()
        })
}

/// The line a kind reads as in the list: its name, then the type it holds, so
/// a search over the line matches either.
pub fn kind_line(kind: &AssetKind) -> String {
    format!("{}  {}", kind.label, kind.type_path)
}

/// Open the list of asset kinds, creating the one picked in a folder.
#[operator(
    id = "asset.new_picker",
    label = "New Asset...",
    description = "Open the list of asset kinds and create the one picked.",
    allows_undo = false,
    params(path(
        String,
        doc = "Folder to create in. Defaults to the folder the browser is showing."
    ))
)]
pub fn asset_new_picker(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = params.as_str("path").map(PathBuf::from);
    commands.queue(move |world: &mut World| {
        open_new_asset_list(world, path.as_deref());
    });
    OperatorResult::Finished
}

/// Put up the list of kinds for a folder, in place of one already open.
pub fn open_new_asset_list(world: &mut World, folder: Option<&Path>) {
    let Some(folder) = folder_for_new_asset(world, folder) else {
        crate::status_bar::notify_error(world, "no project is open");
        return;
    };
    let kinds = creatable_kinds(world);
    if kinds.is_empty() {
        crate::status_bar::notify_warn(world, "this project registers no asset kinds");
        return;
    }
    close_new_asset_list(world);
    let items: Vec<String> = kinds.iter().map(kind_line).collect();
    let list = NewAssetList {
        folder,
        kinds: kinds.into_iter().map(|kind| kind.kind).collect(),
    };
    world.commands().spawn((
        PickerProps::new(spawn_kind_item, pick_kind)
            .items(items)
            .title("New Asset")
            .placeholder(Some("Search asset kinds..")),
        list,
        crate::EditorEntity,
        crate::BlocksCameraInput,
    ));
    world.flush();
}

/// Drop a list left open, so a second New Asset replaces it rather than
/// stacking another list over it.
pub fn close_new_asset_list(world: &mut World) {
    let open: Vec<Entity> = world
        .query_filtered::<Entity, With<NewAssetList>>()
        .iter(world)
        .collect();
    for list in open {
        if let Ok(entity) = world.get_entity_mut(list) {
            entity.despawn();
        }
    }
}

/// The folder a list writes into: the one it was opened on, the folder holding
/// the file it was opened on, or the one the browser is showing.
fn folder_for_new_asset(world: &World, folder: Option<&Path>) -> Option<PathBuf> {
    let asked = folder.map(|folder| crate::definition_assets::resolve_project_path(world, folder));
    let chosen = match asked {
        Some(folder) if folder.is_dir() => Some(folder),
        Some(file) => file
            .parent()
            .filter(|parent| parent.is_dir())
            .map(Path::to_path_buf),
        None => None,
    };
    chosen.or_else(|| crate::definition_assets::new_definition_dir(world))
}

fn spawn_kind_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    mut commands: Commands,
) -> Result {
    commands.spawn((
        picker_item(matched.index),
        ChildOf(entities.list),
        children![match_text(matched.segments)],
    ));
    Ok(())
}

fn pick_kind(
    input: In<SelectInput>,
    lists: Query<&NewAssetList>,
    mut commands: Commands,
) -> Result {
    let picker = input.entities.picker;
    let list = lists.get(picker)?;
    let chosen = list.kinds.get(input.index).cloned();
    let folder = list.folder.clone();
    commands.entity(picker).try_despawn();
    let Some(kind) = chosen else {
        return Ok(());
    };
    commands.queue(move |world: &mut World| {
        create_in_folder(world, &kind, &folder);
    });
    Ok(())
}

/// Write a fresh asset of a kind into a folder, open its card, and take the
/// browser to the file so it can be renamed where it sits.
pub fn create_in_folder(world: &mut World, kind: &str, folder: &Path) -> Option<PathBuf> {
    let path = crate::definition_assets::new_definition(world, kind, None, Some(folder))?;
    show_in_browser(world, &path);
    Some(path)
}

/// Point the asset browser at a file the editor just wrote.
fn show_in_browser(world: &mut World, indexed: &Path) {
    let file = crate::asset_index::absolute_path(world, indexed);
    let Some(folder) = file.parent().map(Path::to_path_buf) else {
        return;
    };
    let Some(mut browser) = world.get_resource_mut::<crate::asset_browser::AssetBrowserState>()
    else {
        return;
    };
    if folder.starts_with(&browser.root_directory) {
        browser.current_directory = folder;
    }
    browser.selected_file = Some(file.to_string_lossy().into_owned());
    browser.needs_refresh = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_reads_as_its_name_and_the_type_it_holds() {
        let kind = AssetKind::from_schema("my_game::content::ItemDef");
        assert_eq!(kind_line(&kind), "Item  my_game::content::ItemDef");
    }
}
