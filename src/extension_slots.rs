//! Where extensions' toolbar buttons, inspector chips and viewport overlays
//! appear in the editor, and the editor's side of the scene document and
//! tool status APIs.

use bevy::prelude::*;
use jackdaw_api_internal::document::{DocumentReader, SceneDocument};
use jackdaw_api_internal::lifecycle::ActiveModalQuery;
use jackdaw_api_internal::tools::{ToolStatus, ToolbarButton};

use crate::layout::{toolbar_op_button, toolbar_separator};
use crate::toolbar_overflow::ToolbarTools;

/// On a toolbar node spawned for an extension's [`ToolbarButton`], so the
/// set can be rebuilt when buttons come and go.
#[derive(Component, Default, Clone)]
pub struct ExtensionToolbarItem;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ToolStatus>()
        .insert_resource(SceneDocument(Box::new(BsnDocumentReader)))
        .add_systems(
            Update,
            (sync_extension_toolbar, clear_tool_status_after_modal),
        );
}

/// Rebuild the extension buttons at the end of each toolbar's tool group
/// whenever an extension adds or removes one, or a toolbar is spawned.
fn sync_extension_toolbar(
    added_buttons: Query<(), Added<ToolbarButton>>,
    mut removed_buttons: RemovedComponents<ToolbarButton>,
    added_toolbars: Query<(), Added<ToolbarTools>>,
    buttons: Query<(Entity, &ToolbarButton)>,
    toolbars: Query<Entity, With<ToolbarTools>>,
    items: Query<Entity, With<ExtensionToolbarItem>>,
    mut commands: Commands,
) {
    let removed = removed_buttons.read().count() > 0;
    if added_buttons.is_empty() && added_toolbars.is_empty() && !removed {
        return;
    }
    for item in &items {
        commands.entity(item).despawn();
    }
    let mut ordered: Vec<(Entity, ToolbarButton)> = buttons
        .iter()
        .map(|(entity, button)| (entity, button.clone()))
        .collect();
    if ordered.is_empty() {
        return;
    }
    ordered.sort_by_key(|(entity, _)| *entity);
    for toolbar in &toolbars {
        commands
            .spawn_scene(toolbar_separator())
            .insert((ExtensionToolbarItem, ChildOf(toolbar)));
        for (_, button) in &ordered {
            commands
                .spawn_scene(toolbar_op_button(button.operator_id, button.icon))
                .insert((ExtensionToolbarItem, ChildOf(toolbar)));
        }
    }
}

/// A tool's status line belongs to the tool's run; drop it once no modal
/// operator is running.
fn clear_tool_status_after_modal(active: ActiveModalQuery, mut status: ResMut<ToolStatus>) {
    if !active.is_modal_running() && status.0.is_some() {
        status.0 = None;
    }
}

/// Answers [`SceneDocument`] from the open scene's BSN document.
struct BsnDocumentReader;

impl DocumentReader for BsnDocumentReader {
    fn authored_components(&self, world: &World, entity: Entity) -> Option<Vec<String>> {
        let ast = world.get_resource::<jackdaw_bsn::SceneBsnAst>()?;
        let node = ast.ast_for(entity)?;
        let registry = world.resource::<AppTypeRegistry>().read();
        Some(
            ast.component_type_paths(node)
                .into_iter()
                .map(|path| component_path(&registry, path))
                .collect(),
        )
    }
}

/// The component a document path names. An enum component written as one of
/// its variants, such as `RigidBody::Static`, names the enum.
fn component_path(registry: &bevy::reflect::TypeRegistry, path: String) -> String {
    if registry.get_with_type_path(&path).is_some() {
        return path;
    }
    match path.rsplit_once("::") {
        Some((parent, _)) if registry.get_with_type_path(parent).is_some() => parent.to_string(),
        _ => path,
    }
}
