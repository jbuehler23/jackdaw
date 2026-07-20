//! File > Extensions dialog. Toggles compiled-in extensions at runtime
//! and persists the current state to `extensions.json`.

use std::path::PathBuf;

use bevy::{
    feathers::{
        controls::{FeathersButton, FeathersCheckbox},
        display::label_dim,
        theme::{ThemeBorderColor, ThemedText},
        tokens,
    },
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    ui::Checked,
    ui_widgets::{Activate, ValueChange},
};
use jackdaw_api::prelude::ExtensionKind;
use jackdaw_api_internal::{
    extensions_config::persist_current_enabled,
    lifecycle::{Extension, ExtensionCatalog},
    paths::config_dir,
};
use jackdaw_feathers::{
    dialog::{CloseDialogEvent, DialogChildrenSlot, OpenDialogEvent},
    tooltip::Tooltip,
};
use rfd::{AsyncFileDialog, FileHandle};

use crate::extension_resolution;
use jackdaw_api_internal::lifecycle::{disable_extension, enable_extension};

pub struct ExtensionsDialogPlugin;

impl Plugin for ExtensionsDialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExtensionsDialogOpen>()
            .init_resource::<InstallStatus>()
            .add_systems(Update, populate_extensions_dialog)
            .add_systems(Update, poll_install_task)
            .add_observer(on_dialog_closed);
    }
}

fn on_dialog_closed(_: On<CloseDialogEvent>, mut open: ResMut<ExtensionsDialogOpen>) {
    open.0 = false;
}

#[derive(Resource, Default)]
struct ExtensionsDialogOpen(bool);

/// Marks the status text row that sits under the install button.
/// Whenever an install finishes (or fails), the task poller replaces
/// its text.
#[derive(Component)]
struct InstallStatusText;

/// Marks the top-level list node inside the dialog. Cascade-
/// despawned after an install succeeds so
/// `populate_extensions_dialog` rebuilds from the updated catalog.
#[derive(Component)]
struct ExtensionsDialogContent;

/// Holds the in-flight file-picker task, if any. Populated when the
/// user clicks the install button; drained by `poll_install_task`
/// once the user picks (or cancels). `pub` so hot-reload can surface
/// its own status messages through the same UI slot.
#[derive(Resource, Default)]
pub struct InstallStatus {
    pub task: Option<Task<Option<FileHandle>>>,
    /// Last user-visible message. Survives dialog re-opens so users
    /// can click around and come back to the success/failure line.
    pub message: Option<String>,
}

pub fn open_extensions_dialog(world: &mut World) {
    world.resource_mut::<ExtensionsDialogOpen>().0 = true;
    world.trigger(
        OpenDialogEvent::new("Extensions", "Close")
            .without_cancel()
            .with_max_width(Val::Px(380.0)),
    );
}

/// Fill the dialog's children slot with a row per catalog entry.
///
/// The slot is found by marker presence rather than `&Children` because
/// a freshly-spawned `DialogChildrenSlot` has no `Children` component
/// yet. The `ExtensionsDialogContent` marker on the list root guards
/// against double-populating a re-opened dialog.
fn populate_extensions_dialog(
    mut commands: Commands,
    catalog: Res<ExtensionCatalog>,
    open: Res<ExtensionsDialogOpen>,
    slots: Query<Entity, With<DialogChildrenSlot>>,
    loaded: Query<&Extension>,
    existing: Query<(), With<ExtensionsDialogContent>>,
) {
    if !open.0 {
        return;
    }
    if !existing.is_empty() {
        return;
    }
    let Some(slot_entity) = slots.iter().next() else {
        return;
    };

    // Split catalog entries into Built-in vs. Custom. Membership comes
    // from each extension's declared `ExtensionKind`.
    let enabled_names: std::collections::HashSet<String> =
        loaded.iter().map(|e| e.id.clone()).collect();
    let mut builtin_rows: Vec<(String, String, bool)> = Vec::new();
    let mut custom_rows: Vec<(String, String, bool)> = Vec::new();
    for (id, label, _description, kind) in catalog.iter_with_content() {
        // Required extensions are load-bearing (the editor panics
        // without them), so they're not user-toggleable. Omit them
        // from the dialog entirely rather than rendering a locked
        // checkbox; they're implementation detail, not a user
        // choice.
        if extension_resolution::is_required(&id) {
            continue;
        }
        let row = (
            id.to_string(),
            label.to_string(),
            enabled_names.contains(&id),
        );
        match kind {
            ExtensionKind::Builtin => builtin_rows.push(row),
            ExtensionKind::Regular => custom_rows.push(row),
        }
    }
    builtin_rows.sort_by(|a, b| a.0.cmp(&b.0));
    custom_rows.sort_by(|a, b| a.0.cmp(&b.0));

    let list = commands
        .spawn_scene(extensions_list_container())
        .insert((ChildOf(slot_entity), ExtensionsDialogContent))
        .id();

    commands
        .spawn_scene(section_header("Built-in"))
        .insert(ChildOf(list));
    for (id, label, checked) in builtin_rows {
        spawn_extension_row(&mut commands, list, id, label, checked);
    }

    commands
        .spawn_scene(section_header("Regular"))
        .insert(ChildOf(list));
    if custom_rows.is_empty() {
        commands
            .spawn_scene(empty_regular_notice())
            .insert(ChildOf(list));
    } else {
        for (id, label, checked) in custom_rows {
            spawn_extension_row(&mut commands, list, id, label, checked);
        }
    }

    spawn_install_row(&mut commands, list);
}

/// Spawn one extension checkbox under `list`, seeding its initial
/// `Checked` state. The checkbox carries its own `ValueChange<bool>`
/// observer (see [`extension_checkbox`]) which toggles the extension
/// and persists the enabled set.
fn spawn_extension_row(
    commands: &mut Commands,
    list: Entity,
    id: String,
    label: String,
    checked: bool,
) {
    let mut row = commands.spawn_scene(extension_checkbox(id, label));
    if checked {
        // Checkboxes don't seed their own `Checked` state, so an enabled
        // extension is marked here at spawn.
        row.insert(Checked);
    }
    row.insert(ChildOf(list));
}

/// The list root spawned into the dialog slot. The
/// `ExtensionsDialogContent` marker is inserted at the spawn site so an
/// install can cascade-despawn this subtree and trigger a rebuild.
fn extensions_list_container() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            min_width: px(280),
        }
    }
}

/// A checkbox bound to one extension. The observer keeps the visual
/// `Checked` state in sync, since the checkbox doesn't self-update, and
/// runs the enable/disable and persist pipeline.
fn extension_checkbox(id: String, label: String) -> impl Scene {
    bsn! {
        @FeathersCheckbox {
            @caption: bsn! { Text(label) ThemedText }
        }
        on(move |change: On<ValueChange<bool>>, mut commands: Commands| {
            let checked = change.value;
            let source = change.source;

            // Belt-and-suspenders: required extensions shouldn't have a
            // checkbox in the first place (see `populate_extensions_dialog`),
            // but if one slipped through we refuse to disable it and keep
            // it visually enabled rather than letting the editor end up in
            // a broken state.
            if !checked && extension_resolution::is_required(&id) {
                warn!("Refusing to disable required extension `{id}`");
                commands.entity(source).insert(Checked);
                return;
            }

            if checked {
                commands.entity(source).insert(Checked);
            } else {
                commands.entity(source).remove::<Checked>();
            }

            let name = id.clone();
            commands.queue(move |world: &mut World| {
                if checked {
                    enable_extension(world, &name);
                    // Re-apply the keymap so newly registered operator
                    // actions get bindings without requiring a restart.
                    crate::extension_lifecycle::apply_active_keymap(world);
                } else {
                    disable_extension(world, &name);
                }
                persist_current_enabled(world);
            });
        })
    }
}

/// Underlined section heading.
fn section_header(label: impl Into<String>) -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            padding: UiRect::new(px(12), px(12), px(8), px(2)),
            border: UiRect::bottom(px(1)),
        }
        ThemeBorderColor(tokens::PANE_HEADER_DIVIDER)
        Children [ label_dim(label) ]
    }
}

/// Placeholder shown when no regular (non-built-in) extensions exist.
fn empty_regular_notice() -> impl Scene {
    bsn! {
        Node {
            padding: UiRect::axes(px(12), px(4)),
        }
        Children [ label_dim("No regular extensions installed") ]
    }
}

/// Compose the install button plus the shared status line under it.
///
/// Only "install a prebuilt .so" lives in the editor. Source-tree builds
/// happen at the launcher (File > Home) so every build carries its
/// potential process-restart with it. This keeps a sudden restart when
/// clicking Build out of the mid-session editor experience.
fn spawn_install_row(commands: &mut Commands, list: Entity) {
    let row = commands
        .spawn_scene(install_row_container())
        .insert(ChildOf(list))
        .id();

    commands.spawn_scene(install_button()).insert((
        ChildOf(row),
        Tooltip::title("Install Extension").with_description(
            "Pick a prebuilt extension dylib (.so / .dll / .dylib) and copy \
                 it into the user extensions directory. The extension loads on \
                 the next editor restart.",
        ),
    ));

    commands
        .spawn_scene(label_dim(String::new()))
        .insert((ChildOf(row), InstallStatusText));
}

fn install_row_container() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::axes(px(12), px(4)),
            row_gap: px(2),
        }
    }
}

/// Button that opens an rfd file picker for a prebuilt dylib. Skips if a
/// picker is already in flight; rfd can't run two at once on some
/// platforms, and it would be confusing UX.
fn install_button() -> impl Scene {
    bsn! {
        @FeathersButton {
            @caption: bsn! { Text("Install prebuilt dylib...") ThemedText }
        }
        on(|_: On<Activate>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                if world.resource::<InstallStatus>().task.is_some() {
                    return;
                }
                let dialog = AsyncFileDialog::new().add_filter(
                    "Extension dylib",
                    // Platform-specific extensions mirror what the loader
                    // recognises (`jackdaw_loader::is_dylib`).
                    &["so", "dylib", "dll"],
                );
                let task =
                    AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
                world.resource_mut::<InstallStatus>().task = Some(task);
                world.resource_mut::<InstallStatus>().message =
                    Some("Select a dylib file...".into());
            });
        })
    }
}

/// Drive the file picker task to completion. On selection, queue a
/// command that copies the file into the extensions directory,
/// attempts a live-load (so the extension activates without
/// restarting), and refreshes the dialog list.
fn poll_install_task(
    mut status: ResMut<InstallStatus>,
    mut texts: Query<&mut Text, With<InstallStatusText>>,
    mut commands: Commands,
) {
    let Some(task) = status.task.as_mut() else {
        sync_status_text(&status.message, &mut texts);
        return;
    };

    let Some(handle) = future::block_on(future::poll_once(task)) else {
        sync_status_text(&status.message, &mut texts);
        return;
    };

    status.task = None;

    match handle {
        Some(picked) => {
            let src = picked.path().to_path_buf();
            commands.queue(move |world: &mut World| {
                if let Err(err) = world.run_system_cached_with(handle_install, src) {
                    error!("Failed to install extension: {err}");
                }
            });
        }
        None => {
            status.message = None;
        }
    }

    sync_status_text(&status.message, &mut texts);
}

fn sync_status_text(
    message: &Option<String>,
    texts: &mut Query<&mut Text, With<InstallStatusText>>,
) {
    let desired = message.as_deref().unwrap_or("");
    for mut text in texts.iter_mut() {
        if text.0 != desired {
            text.0 = desired.to_string();
        }
    }
}

/// Copy the picked file into the extensions directory, then live-
/// load it from the copy. Updates `InstallStatus.message` and
/// despawns the dialog's content so the list rebuilds on the next
/// frame.
/// Route a freshly-built `.so` / `.dylib` / `.dll` through the
/// install pipeline: copy to `extensions/`, try to live-load, and
/// set an `InstallStatus` message describing the result.
///
/// Returns the extension id on success or `Err(LoadError)` so
/// callers can inspect the failure. Use
/// `LoadError::is_symbol_mismatch()` for "SDK rebuilt, stale project
/// cache" recovery.
pub fn handle_install_from_path(
    world: &mut World,
    src: std::path::PathBuf,
) -> Result<String, jackdaw_loader::LoadError> {
    world
        .run_system_cached_with(handle_install, src)
        .map_err(BevyError::from)
        .map_err(jackdaw_loader::LoadError::from)
        .flatten()
}

fn handle_install(
    In(src): In<PathBuf>,
    world: &mut World,
    extension_dialogs: &mut QueryState<Entity, With<ExtensionsDialogContent>>,
) -> Result<String, jackdaw_loader::LoadError> {
    let dest = match install_picked_file(&src) {
        Ok(d) => d,
        Err(err) => {
            warn!("Failed to install dylib: {err}");
            world.resource_mut::<InstallStatus>().message = Some(format!("Install failed: {err}"));
            return Err(jackdaw_loader::LoadError::InstallIo(err.to_string()));
        }
    };
    info!("Installed dylib to {}", dest.display());

    let result = jackdaw_loader::load_from_path(world, &dest);
    let msg = match &result {
        Ok(name) => {
            info!("Live-loaded extension `{name}` from {}", dest.display());
            format!("Loaded extension `{name}`. BEI keybinds (if any) activate on next restart.")
        }
        Err(err) => {
            warn!("Live-load failed for {}: {err}", dest.display());
            if err.is_symbol_mismatch() {
                // Soft-fail: caller will detect this and run the
                // auto-clean-and-retry recovery path. Don't update
                // the install-status message; the retry UI owns it.
                "SDK mismatch detected; cleaning project cache...".to_string()
            } else {
                format!(
                    "Installed to {}, but live-load failed: {err}. Restart the editor to retry.",
                    dest.display()
                )
            }
        }
    };
    world.resource_mut::<InstallStatus>().message = Some(msg);

    // Despawn the existing list so `populate_extensions_dialog`
    // rebuilds it from the now-updated catalog.
    let targets: Vec<Entity> = extension_dialogs.iter(world).collect();
    for entity in targets {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }

    result
}

/// Install a built dylib into the per-user extensions directory.
/// Returns the destination path on success. Creates the directory
/// if missing.
///
/// Uses write-to-tempfile + rename instead of `std::fs::copy` so we
/// never truncate a file that's currently mmapped by the running
/// process. Truncating a live-mapped `.so` corrupts its pages in
/// place and segfaults the editor the next time anything touches
/// the loaded library's code or static data (including `dlopen`
/// walking `/proc/self/maps`).
///
/// Install the picked `.so` into the per-user dir with a **unique
/// filename per install** (e.g., `libmy_ext-1745678901234.so`), then
/// clean up any prior sibling files matching the same basename so the
/// dir doesn't accumulate stale copies.
///
/// Why the unique filename: glibc's `dlopen` caches loaded libraries
/// by absolute path after realpath resolution. A second `dlopen` of
/// the same path returns the original handle even if the on-disk
/// file was atomically replaced; the mapping doesn't re-check inode.
/// Re-installing a rebuilt extension by overwriting one
/// `libmy_ext.so` path would silently return the first-loaded code
/// forever. Giving each install a fresh path forces glibc to mmap
/// the new file. The old mapping stays valid for any currently-held
/// fn pointers until its `libloading::Library` handle is dropped
/// (which the loader never does).
///
/// Cleanup after the rename removes any sibling files that share the
/// same stem (e.g. `libmy_ext-*.so`), so at most one file per
/// extension lives in the dir after a successful install. Cleanup
/// failure is a warning, not an error: the load has already
/// succeeded, and the stale file will be cleaned on the next
/// install.
fn install_picked_file(src: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let Some(config) = config_dir() else {
        return Err(std::io::Error::other(
            "platform config directory is unavailable",
        ));
    };
    let dest_dir = config.join("extensions");
    std::fs::create_dir_all(&dest_dir)?;
    let file_name = src.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "picked path has no file name",
        )
    })?;

    // Split `libmy_ext.so` into `"libmy_ext"` + `".so"` (or `"libmy_ext.dylib"`
    // / `"my_ext.dll"` on other platforms). We suffix the stem with a
    // monotonic millisecond timestamp.
    let file_name_str = file_name.to_string_lossy();
    let (stem, ext_with_dot) = match file_name_str.rfind('.') {
        Some(i) => (&file_name_str[..i], &file_name_str[i..]),
        None => (file_name_str.as_ref(), ""),
    };
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let unique_name = format!("{stem}-{ts_ms}{ext_with_dot}");
    let dest = dest_dir.join(&unique_name);

    // Write to a sibling temp path, then atomic-rename. A unique
    // suffix keeps concurrent installs from clobbering each other's
    // temp file. The prefix is shared with the extension watcher
    // so the watcher ignores our in-flight rename.
    let temp_name = format!(
        "{}{}-{}",
        jackdaw_loader::INSTALL_TEMPFILE_PREFIX,
        std::process::id(),
        unique_name
    );
    let temp = dest_dir.join(temp_name);
    std::fs::copy(src, &temp)?;
    if let Err(e) = std::fs::rename(&temp, &dest) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    // Remove older sibling installs matching the same stem so disk
    // doesn't accumulate. We keep only the file we just installed.
    cleanup_prior_installs(&dest_dir, stem, ext_with_dot, &dest);

    Ok(dest)
}

/// Delete sibling files in `dir` whose name is `<stem>-*<ext>` (the
/// shape produced by [`install_picked_file`]), except for `keep`.
/// Also removes the plain `<stem><ext>` (pre-unique-name legacy) if
/// it exists, so upgrading from the old single-filename install
/// scheme doesn't leave a stale file behind.
fn cleanup_prior_installs(
    dir: &std::path::Path,
    stem: &str,
    ext_with_dot: &str,
    keep: &std::path::Path,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip anything that isn't a sibling install for this stem.
        // Legacy filename: exactly `<stem><ext>`.
        // Timestamped filename: `<stem>-<digits><ext>`.
        let is_legacy = name == format!("{stem}{ext_with_dot}");
        let is_timestamped = name
            .strip_prefix(&format!("{stem}-"))
            .and_then(|rest| rest.strip_suffix(ext_with_dot))
            .is_some_and(|middle| middle.bytes().all(|b| b.is_ascii_digit()));
        if !is_legacy && !is_timestamped {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("Failed to clean up prior install {}: {e}", path.display());
        }
    }
}
