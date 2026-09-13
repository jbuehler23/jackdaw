//! Filesystem operators for the asset browser and project files panel.
//!
//! `file.delete` confirms via a dialog before removing the path from disk,
//! `file.convert_to_binary` and `file.convert_to_text` rewrite one document in
//! the other form, and `project.export_binary` writes a whole tree out as
//! binary for a shipped game. Both the asset browser and the project files
//! panel reach these from a right-click menu.

use std::path::{Path, PathBuf};

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
    ctx.register_operator::<FileConvertToBinaryOp>();
    ctx.register_operator::<FileConvertToTextOp>();
    ctx.register_operator::<ProjectExportBinaryOp>();
}

/// The folder an export writes into when none is named: a sibling of the tree.
fn default_export_dir(source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "assets".to_string());
    source
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{name}-binary"))
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

/// Rewrite one document as its binary twin, in place.
#[operator(
    id = "file.convert_to_binary",
    label = "Convert to Binary",
    description = "Rewrite one BSN document in the binary form, removing the text file.",
    allows_undo = false,
    params(path(String, doc = "The document to convert."))
)]
pub fn file_convert_to_binary(
    params: In<OperatorParameters>,
    scenes: Option<Res<crate::scenes::Scenes>>,
) -> OperatorResult {
    convert_document(
        params.as_str("path"),
        jackdaw_bsn::DocumentForm::Binary,
        scenes.as_deref(),
    )
}

/// Rewrite one document as `.bsn` text, in place.
#[operator(
    id = "file.convert_to_text",
    label = "Convert to Text",
    description = "Rewrite one BSN document in the text form, removing the binary file.",
    allows_undo = false,
    params(path(String, doc = "The document to convert."))
)]
pub fn file_convert_to_text(
    params: In<OperatorParameters>,
    scenes: Option<Res<crate::scenes::Scenes>>,
) -> OperatorResult {
    convert_document(
        params.as_str("path"),
        jackdaw_bsn::DocumentForm::Text,
        scenes.as_deref(),
    )
}

fn convert_document(
    path: Option<&str>,
    to: jackdaw_bsn::DocumentForm,
    scenes: Option<&crate::scenes::Scenes>,
) -> OperatorResult {
    let Some(path) = path.map(PathBuf::from) else {
        warn!("convert: no path provided");
        return OperatorResult::Cancelled;
    };
    if !jackdaw_bsn::is_document_path(&path) {
        warn!("convert: {} is not a BSN document", path.display());
        return OperatorResult::Cancelled;
    }
    if scenes.is_some_and(|scenes| is_open_in_a_tab(scenes, &path)) {
        warn!(
            "convert: {} is open; close its tab before changing the form it is held in",
            path.display()
        );
        return OperatorResult::Cancelled;
    }
    let converted = match to {
        jackdaw_bsn::DocumentForm::Binary => jackdaw_bsn::convert_to_binary(&path),
        jackdaw_bsn::DocumentForm::Text => jackdaw_bsn::convert_to_text(&path),
    };
    match converted {
        Ok(written) => {
            info!("{} is now {}", path.display(), written.display());
            OperatorResult::Finished
        }
        Err(err) => {
            warn!("convert: {err}");
            OperatorResult::Cancelled
        }
    }
}

/// Whether a tab holds the document at `path`, whose file a conversion moves.
fn is_open_in_a_tab(scenes: &crate::scenes::Scenes, path: &Path) -> bool {
    let held = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    scenes.tabs.iter().any(|tab| {
        tab.path
            .as_ref()
            .map(|open| open.canonicalize().unwrap_or_else(|_| open.clone()))
            .is_some_and(|open| open == held)
    })
}

/// Write a tree of documents out in the binary form, for a shipped game.
#[operator(
    id = "project.export_binary",
    label = "Export Binary Assets",
    description = "Write every BSN document under a folder out in the binary form, leaving the \
                   source files as they are and copying everything else through.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "The folder to convert. Defaults to the project's assets folder."
        ),
        out(
            String,
            doc = "The folder to write into. Defaults to a sibling of the source."
        )
    )
)]
pub fn project_export_binary(
    params: In<OperatorParameters>,
    project: Option<Res<crate::project::ProjectRoot>>,
) -> OperatorResult {
    let source = params
        .as_str("path")
        .map(PathBuf::from)
        .or_else(|| project.as_ref().map(|project| project.assets_dir()));
    let Some(source) = source else {
        warn!("project.export_binary: no folder to convert");
        return OperatorResult::Cancelled;
    };
    if !source.is_dir() {
        warn!(
            "project.export_binary: {} is not a folder",
            source.display()
        );
        return OperatorResult::Cancelled;
    }
    let destination = params
        .as_str("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_export_dir(&source));
    match jackdaw_bsn::export_binary(&source, &destination) {
        Ok(converted) => {
            info!(
                "exported {converted} documents from {} into {}",
                source.display(),
                destination.display()
            );
            OperatorResult::Finished
        }
        Err(err) => {
            warn!("project.export_binary: {err}");
            OperatorResult::Cancelled
        }
    }
}
