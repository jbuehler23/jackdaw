//! The asset row naming the material an inspected entity wears.
//!
//! The material cards edit the asset a handle names. The row above them names
//! the handle itself, so a mesh entity's material can be picked, cleared or
//! created without leaving the inspector, and a pick swaps which asset the
//! cards below are editing.

use std::path::Path;

use bevy::prelude::*;
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_feathers::icons::IconFont;

use super::asset_row::{
    AssetFieldReader, AssetFieldTarget, AssetFieldWriter, AssetRowProps, spawn_asset_row,
};

/// Reflect type path of the component a mesh entity wears its material on.
/// The inspector lists it whether or not the document authors it, so a mesh
/// that took its material from the model it was loaded from still has a row
/// to pick one on.
pub(crate) const MESH_MATERIAL_TYPE_PATH: &str =
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>";

/// The tuple field of `MeshMaterial3d` holding the handle.
const HANDLE_FIELD: &str = "0";

/// The field the row writes, as the operators name it. The material a brush
/// face wears is no component field at all, and the one a mesh wears sits
/// under a tuple index no one would type, so both rows answer to this.
const MATERIAL_FIELD: &str = "material";

/// What the row calls itself.
const LABEL: &str = "Material";

/// Trigger that refreshes every material card, so the cards under the row
/// follow a pick onto the asset it chose.
const MATERIAL_CARDS: &str = "material_card::";

/// Put the row naming the entity's material at the top of a material card.
pub(crate) fn spawn_material_asset_row(world: &mut World, source: Entity, body: Entity) {
    let brush = world.get::<crate::brush::Brush>(source).is_some();
    if !brush
        && world
            .get::<MeshMaterial3d<StandardMaterial>>(source)
            .is_none()
    {
        return;
    }
    let icon_font = world
        .get_resource::<IconFont>()
        .map(|font| font.0.clone())
        .unwrap_or_default();
    let props = AssetRowProps {
        target: AssetFieldTarget::Inspected {
            source,
            type_path: MESH_MATERIAL_TYPE_PATH.to_string(),
        },
        field_path: MATERIAL_FIELD.to_string(),
        asset_type_path: StandardMaterial::type_path().to_string(),
        label: LABEL.to_string(),
        indent: 0,
    };
    let row = spawn_asset_row(&mut world.commands(), body, props, &icon_font);
    world.flush();
    let Ok(mut row_mut) = world.get_entity_mut(row) else {
        return;
    };
    row_mut.insert(AssetFieldReader(Box::new(move |world| {
        Some(material_path(world, source))
    })));
    if brush {
        row_mut.insert(AssetFieldWriter(Box::new(|world, path| {
            write_face_material(world, path)
        })));
    } else {
        row_mut.insert(AssetFieldWriter(Box::new(move |world, path| {
            write_entity_material(world, source, path)
        })));
    }
    world.flush();
    super::asset_row::show_asset_row_path(world, row);
}

// -- The material a mesh entity wears --------------------------------------

/// Write a path into the entity's material handle, or take the override off
/// when the path is empty.
fn write_entity_material(world: &mut World, source: Entity, path: &str) -> bool {
    if !authored(world, source) {
        return wear_until_reloaded(world, source, path);
    }
    if path.is_empty() {
        return clear_entity_material(world, source);
    }
    let worn = jackdaw_bsn::BsnValue::String(material_path(world, source));
    let json = serde_json::Value::String(path.to_string());
    crate::commands::field_edit_commit_on_from(
        world,
        source,
        MESH_MATERIAL_TYPE_PATH,
        HANDLE_FIELD,
        &json,
        worn,
    )
}

/// Whether the document holds a node for an entity, and so can carry what the
/// row writes. A part of a loaded model has none: the model file is the whole
/// of what the scene says about it.
fn authored(world: &World, source: Entity) -> bool {
    world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .and_then(|ast| ast.ast_for(source))
        .is_some()
}

/// Put a material on a part of a loaded model, which lasts as long as the
/// editor holds the model and reaches no scene file.
fn wear_until_reloaded(world: &mut World, source: Entity, path: &str) -> bool {
    let Some(previous) = world
        .get::<MeshMaterial3d<StandardMaterial>>(source)
        .map(|material| material.0.clone())
    else {
        return false;
    };
    let chosen = if path.is_empty() {
        Handle::default()
    } else {
        let Some(chosen) = material_named(world, path) else {
            crate::status_bar::notify_error(world, format!("{path} holds no material"));
            return false;
        };
        chosen
    };
    let mut command: Box<dyn EditorCommand> = Box::new(WearMaterial {
        entity: source,
        previous,
        chosen,
    });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    if !path.is_empty() {
        crate::status_bar::notify_warn(world, "kept until the model is loaded again");
    }
    true
}

/// Swap the material an entity wears, and swap it back.
struct WearMaterial {
    entity: Entity,
    previous: Handle<StandardMaterial>,
    chosen: Handle<StandardMaterial>,
}

impl EditorCommand for WearMaterial {
    fn execute(&mut self, world: &mut World) {
        wear(world, self.entity, self.chosen.clone());
    }

    fn undo(&mut self, world: &mut World) {
        wear(world, self.entity, self.previous.clone());
    }

    fn description(&self) -> &str {
        "Set material"
    }
}

/// Put a material handle on an entity's mesh.
fn wear(world: &mut World, entity: Entity, material: Handle<StandardMaterial>) {
    if let Ok(mut entity) = world.get_entity_mut(entity) {
        entity.insert(MeshMaterial3d(material));
    }
}

/// Drop the material the document overrides on an entity, so the entity is
/// back to the material it derives, as one undo entry.
fn clear_entity_material(world: &mut World, source: Entity) -> bool {
    let Some(previous) = world
        .get::<MeshMaterial3d<StandardMaterial>>(source)
        .map(|material| material.0.clone())
    else {
        return false;
    };
    let authored = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .and_then(|ast| ast.ast_for(source))
        .and_then(|node| {
            let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
            jackdaw_bsn::get_bsn_field(ast, node, MESH_MATERIAL_TYPE_PATH, HANDLE_FIELD)
        });
    let mut command: Box<dyn EditorCommand> = Box::new(ClearEntityMaterial {
        entity: source,
        authored,
        previous,
    });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    true
}

/// Take the material a document overrides off an entity, and put it back.
struct ClearEntityMaterial {
    entity: Entity,
    authored: Option<jackdaw_bsn::BsnValue>,
    previous: Handle<StandardMaterial>,
}

impl EditorCommand for ClearEntityMaterial {
    fn execute(&mut self, world: &mut World) {
        if let Some(mut ast) = world.get_resource_mut::<jackdaw_bsn::SceneBsnAst>()
            && let Some(node) = ast.ast_for(self.entity)
        {
            ast.remove_component_patch(node, MESH_MATERIAL_TYPE_PATH);
        }
        wear(world, self.entity, Handle::default());
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(authored) = self.authored.clone() {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let registry = registry.read();
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity) {
                jackdaw_bsn::set_bsn_field(
                    &mut ast,
                    node,
                    MESH_MATERIAL_TYPE_PATH,
                    HANDLE_FIELD,
                    authored,
                    &registry,
                );
            }
        }
        wear(world, self.entity, self.previous.clone());
    }

    fn description(&self) -> &str {
        "Clear material"
    }
}

/// The file behind the material the inspected entity wears, as the empty
/// string when it wears one no file holds.
fn material_path(world: &World, source: Entity) -> String {
    use path_slash::PathExt as _;

    let Some(handle) = super::material_display::resolve_material_handle(world, source) else {
        return String::new();
    };
    let untyped = handle.clone().untyped();
    if let Some(indexed) = world
        .get_resource::<crate::asset_index::AssetIndex>()
        .and_then(|index| index.by_handle(&untyped))
    {
        return indexed.path.to_slash_lossy().into_owned();
    }
    world
        .get_resource::<AssetServer>()
        .and_then(|server| server.get_path(handle.id()))
        .map(|path| path.to_string())
        .unwrap_or_default()
}

// -- The material a brush face wears ---------------------------------------

/// Put a material on the selected brush faces, or the default one back when
/// the path is empty.
fn write_face_material(world: &mut World, path: &str) -> bool {
    let material = if path.is_empty() {
        Handle::default()
    } else {
        let Some(handle) = material_named(world, path) else {
            crate::status_bar::notify_error(world, format!("{path} holds no material"));
            return false;
        };
        handle
    };
    world.trigger(crate::material_browser::ApplyMaterialDefToFaces { material });
    world.flush();
    true
}

/// The material a project file holds, for the writers that take a path rather
/// than a field.
fn material_named(world: &World, path: &str) -> Option<Handle<StandardMaterial>> {
    world
        .get_resource::<crate::asset_index::AssetIndex>()
        .and_then(|index| index.get(Path::new(path)))
        .and_then(|entry| entry.value.handle().cloned())
        .and_then(|handle| handle.try_typed::<StandardMaterial>().ok())
}

// -- Keeping the cards under the row on the asset it names -----------------

/// Keep the cards under the row on the material the inspected entity is
/// wearing, so a pick, an undo or an edit from another panel moves them with
/// it rather than leaving them editing the asset they opened on.
pub(crate) fn follow_the_material_the_entity_wears(
    world: &mut World,
    mut shown: Local<Option<(Entity, Option<AssetId<StandardMaterial>>)>>,
) {
    let Some(source) = world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary)
    else {
        *shown = None;
        return;
    };
    let worn =
        super::material_display::resolve_material_handle(world, source).map(|handle| handle.id());
    let moved = matches!(*shown, Some((was, before)) if was == source && before != worn);
    *shown = Some((source, worn));
    if moved {
        refresh_material_cards(world, source);
    }
}

/// Point the cards below the row at whatever asset the row now names, through
/// the card-body refresh a material apply already uses, so the panel around
/// them stays up and the widgets it holds are not taken down mid-setup.
fn refresh_material_cards(world: &mut World, source: Entity) {
    world.commands().queue(move |world: &mut World| {
        world.trigger(super::material_card_routing::RefreshInspectorCardBody {
            source,
            type_path: MATERIAL_CARDS.to_string(),
        });
    });
}
