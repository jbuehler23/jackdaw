//! Prompts that gate legacy `.jsn` content behind conversion to `.bsn`.
//!
//! Entering the editor with a project that still contains `.jsn` scene,
//! prefab, or catalog files opens a modal offering to convert them all.
//! Conversion runs [`crate::jsn_to_bsn::convert_project`]: every converted
//! source is kept as a `.jsn.bak` backup. Declining leaves the files
//! untouched, but legacy files cannot be opened until converted; the prompt
//! returns on the next project open until the project has no legacy files
//! left.
//!
//! Opening an individual `.jsn` file through a file dialog raises a
//! per-file confirmation instead: converting opens the resulting `.bsn`,
//! cancelling aborts the open.

use bevy::prelude::*;
use jackdaw_feathers::dialog::{DialogActionEvent, EditorDialog, OpenDialogEvent};
use jackdaw_feathers::icons::EditorFont;
use std::path::{Path, PathBuf};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PendingMigration>()
        .init_resource::<PendingFileConversion>()
        .add_observer(on_dialog_action)
        .add_systems(OnEnter(crate::AppState::Editor), prompt_for_legacy_project)
        .add_systems(Update, resolve_dismissed_prompt);
}

/// `Some(count)` while the migration dialog is displayed, holding the number
/// of legacy files it offered to convert.
#[derive(Resource, Default)]
pub struct PendingMigration {
    pub file_count: Option<usize>,
}

/// The `.jsn` file a per-file conversion prompt is waiting on.
#[derive(Resource, Default)]
pub struct PendingFileConversion {
    pub pending: Option<PathBuf>,
}

/// Which conversion prompt is currently displayed as an editor dialog.
/// Absent headless, where prompts never open UI.
#[derive(Resource, Clone, Copy)]
enum OpenPrompt {
    Project,
    File,
}

/// Route an interactive open through the conversion gate: `.bsn` paths (and
/// worlds with no dialog font, e.g. headless harnesses) open immediately;
/// a `.jsn` path raises a confirmation, and the open resumes only when the
/// user picks Convert and Open. The open paths themselves convert the file
/// on disk, so confirming both converts and opens.
pub fn request_open_with_conversion(world: &mut World, path: &Path) {
    let is_legacy = path.extension().is_some_and(|e| e == "jsn");
    let headless = world.get_resource::<EditorFont>().is_none();
    if !is_legacy || headless {
        crate::scenes::operators::scene_open_system(world, path);
        return;
    }
    world.resource_mut::<PendingFileConversion>().pending = Some(path.to_path_buf());
    world.insert_resource(OpenPrompt::File);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    world.commands().trigger(
        OpenDialogEvent::new("Legacy Scene Format", "Convert and Open")
            .with_description(format!(
                "{file_name} uses the legacy .jsn format, which can no longer be \
                 opened directly. Convert it to .bsn and open? The original is \
                 kept as a .jsn.bak backup."
            ))
            .with_close_button(false)
            .with_close_on_click_outside(false),
    );
    world.flush();
}

/// Count the project's legacy `.jsn` files (scenes, prefabs, catalog;
/// config and backups excluded).
pub fn count_legacy_files(root: &std::path::Path) -> usize {
    let mut files = Vec::new();
    crate::jsn_to_bsn::collect_jsn_files(root, &mut files);
    files.len()
}

/// On entering the editor, offer to convert any legacy files found in the
/// opened project. Skips the dialog when `EditorFont` is absent (headless
/// tests); `PendingMigration` is still set so logic-level tests can drive
/// [`resolve_migration`] directly.
fn prompt_for_legacy_project(world: &mut World) {
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        return;
    };
    let count = count_legacy_files(&root);
    if count == 0 {
        return;
    }
    world.resource_mut::<PendingMigration>().file_count = Some(count);

    if world.get_resource::<EditorFont>().is_none() {
        return;
    }
    world.insert_resource(OpenPrompt::Project);

    let mut dialog = OpenDialogEvent::new("Legacy Scene Format", "Convert")
        .with_description(format!(
            "This project contains {count} scene file(s) in the legacy .jsn \
             format. Convert them to .bsn now? Originals are kept as \
             .jsn.bak backups. Legacy files cannot be opened until they \
             are converted."
        ))
        .with_close_button(false)
        .with_close_on_click_outside(false);
    dialog.cancel = Some("Not Now".into());
    world.commands().trigger(dialog);
    world.flush();
}

/// Apply the user's choice: convert the project (keeping `.jsn.bak`
/// backups) or leave it as-is. Clears the pending state either way.
pub fn resolve_migration(world: &mut World, convert: bool) {
    let count = world.resource_mut::<PendingMigration>().file_count.take();
    if !convert {
        info!(
            "Left {} legacy .jsn file(s) unconverted; they cannot be opened until \
             converted, and the prompt returns on next open",
            count.unwrap_or(0)
        );
        return;
    }
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        return;
    };
    let report = crate::jsn_to_bsn::convert_project(world, &root);
    info!(
        "Converted {} scene(s)/prefab(s) and {} catalog(s) to BSN; originals kept as .jsn.bak",
        report.scenes.len(),
        report.catalogs.len()
    );
    for (path, err) in &report.failures {
        warn!("Could not convert {}: {err}", path.display());
    }
}

/// Confirm the displayed prompt: convert the project, or convert and open
/// the stashed file.
fn on_dialog_action(
    _event: On<DialogActionEvent>,
    prompt: Option<Res<OpenPrompt>>,
    mut commands: Commands,
) -> Result<(), BevyError> {
    let Some(prompt) = prompt else {
        return Ok(());
    };
    let kind = *prompt;
    commands.remove_resource::<OpenPrompt>();
    commands.queue(move |world: &mut World| {
        resolve_confirmed_prompt(world, kind);
    });
    Ok(())
}

fn resolve_confirmed_prompt(world: &mut World, kind: OpenPrompt) {
    match kind {
        OpenPrompt::Project => resolve_migration(world, true),
        OpenPrompt::File => {
            let pending = world.resource_mut::<PendingFileConversion>().pending.take();
            if let Some(path) = pending {
                crate::scenes::operators::scene_open_system(world, &path);
            }
        }
    }
}

/// A prompt whose dialog is gone without its action firing was declined
/// (cancel button or Esc): resolve it as such.
fn resolve_dismissed_prompt(
    prompt: Option<Res<OpenPrompt>>,
    dialogs: Query<(), With<EditorDialog>>,
    mut commands: Commands,
) {
    let Some(prompt) = prompt else {
        return;
    };
    if !dialogs.is_empty() {
        return;
    }
    let kind = *prompt;
    commands.remove_resource::<OpenPrompt>();
    commands.queue(move |world: &mut World| {
        resolve_declined_prompt(world, kind);
    });
}

fn resolve_declined_prompt(world: &mut World, kind: OpenPrompt) {
    match kind {
        OpenPrompt::Project => resolve_migration(world, false),
        OpenPrompt::File => {
            let pending = world.resource_mut::<PendingFileConversion>().pending.take();
            if let Some(path) = pending {
                info!(
                    "Left {} unconverted; it cannot be opened until converted",
                    path.display()
                );
            }
        }
    }
}
