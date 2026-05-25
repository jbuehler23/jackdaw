//! Filesystem operators for the asset browser and project files panel.
//!
//! Currently exposes `file.delete`, which confirms via a dialog before
//! removing the path from disk. Both the asset browser and the project
//! files panel call into this operator when the user picks the Delete
//! entry from a right-click menu.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::dialog::{DialogActionEvent, OpenConfirmationDialogEvent};

pub struct FileOpsPlugin;

impl Plugin for FileOpsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingFileDelete>()
            .add_observer(on_file_delete_confirmed);
    }
}

/// Path queued for deletion. Set by `file.delete` when the operator
/// opens the confirmation dialog; consumed by the dialog action observer
/// when the user clicks Delete.
#[derive(Resource, Default)]
pub struct PendingFileDelete {
    pub path: Option<PathBuf>,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<FileDeleteOp>();
}

/// Confirm and delete a file or directory from disk. The path is taken
/// either from the `path` param (preferred) or, if absent, from the
/// asset browser's currently selected file.
#[operator(
    id = "file.delete",
    label = "Delete File",
    description = "Remove a file or directory from disk after user confirmation.",
    allows_undo = false,
    params(path(String, doc = "Absolute path to the file or directory."))
)]
pub fn file_delete(
    params: In<OperatorParameters>,
    mut commands: Commands,
    mut pending: ResMut<PendingFileDelete>,
    browser: Option<Res<crate::asset_browser::AssetBrowserState>>,
) -> OperatorResult {
    let path: Option<PathBuf> = params.as_str("path").map(PathBuf::from).or_else(|| {
        browser
            .as_ref()
            .and_then(|b| b.selected_file.as_ref())
            .map(PathBuf::from)
    });
    let Some(path) = path else {
        warn!("file.delete: no path provided and no asset browser selection");
        return OperatorResult::Cancelled;
    };
    if !path.exists() {
        warn!("file.delete: {} does not exist", path.display());
        return OperatorResult::Cancelled;
    }
    let display = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    pending.path = Some(path);
    commands.trigger(
        OpenConfirmationDialogEvent::new("Delete file", "Delete")
            .with_description(format!("Permanently delete {display}?")),
    );
    OperatorResult::Finished
}

fn on_file_delete_confirmed(
    _event: On<DialogActionEvent>,
    mut pending: ResMut<PendingFileDelete>,
    asset_browser: Option<ResMut<crate::asset_browser::AssetBrowserState>>,
) {
    let Some(path) = pending.path.take() else {
        return;
    };
    let result = if path.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };
    match result {
        Ok(()) => info!("file.delete: removed {}", path.display()),
        Err(err) => warn!("file.delete: failed to remove {}: {err}", path.display()),
    }
    // Clear any stale asset-browser selection that pointed at the
    // deleted path so the breadcrumb / highlight don't lag.
    if let Some(mut browser) = asset_browser {
        let path_str = path.to_string_lossy().to_string();
        if browser.selected_file.as_deref() == Some(path_str.as_str()) {
            browser.selected_file = None;
            browser.needs_refresh = true;
        }
    }
}
