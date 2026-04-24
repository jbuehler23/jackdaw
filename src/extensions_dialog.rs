//! File > Extensions dialog. Toggles compiled-in extensions at runtime
//! and persists the current state to `extensions.json`.

use std::path::PathBuf;

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
};
use jackdaw_api::prelude::ExtensionKind;
use jackdaw_api_internal::lifecycle::{Extension, ExtensionCatalog};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonProps, ButtonSize, ButtonVariant, button},
    checkbox::{CheckboxCommitEvent, CheckboxProps, checkbox},
    dialog::{CloseDialogEvent, DialogChildrenSlot, OpenDialogEvent},
    icons::{EditorFont, Icon, IconFont},
    tokens,
};
use rfd::{AsyncFileDialog, FileHandle};

use crate::extensions_config;
use jackdaw_api_internal::lifecycle::{disable_extension, enable_extension};

pub struct ExtensionsDialogPlugin;

impl Plugin for ExtensionsDialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExtensionsDialogOpen>()
            .init_resource::<InstallStatus>()
            .add_systems(Update, populate_extensions_dialog)
            .add_systems(Update, poll_install_task)
            .add_observer(on_extension_checkbox_commit)
            .add_observer(on_install_button_click)
            .add_observer(on_dialog_closed);
    }
}

fn on_dialog_closed(_: On<CloseDialogEvent>, mut open: ResMut<ExtensionsDialogOpen>) {
    open.0 = false;
}

#[derive(Resource, Default)]
struct ExtensionsDialogOpen(bool);

/// Records the extension name on each checkbox so the commit observer
/// can look up which one to toggle.
#[derive(Component)]
struct ExtensionCheckbox {
    extension_id: String,
}

/// Marks the "Install from file..." button. A single click observer
/// resolves the button entity by querying for this component, so
/// adding more buttons won't cross-fire.
#[derive(Component)]
struct InstallFromFileButton;

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
/// yet. Checking for existing `ExtensionCheckbox` entities prevents
/// double-populating a re-opened dialog.
fn populate_extensions_dialog(
    mut commands: Commands,
    catalog: Res<ExtensionCatalog>,
    open: Res<ExtensionsDialogOpen>,
    slots: Query<Entity, With<DialogChildrenSlot>>,
    loaded: Query<&Extension>,
    editor_font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    existing: Query<(), With<ExtensionCheckbox>>,
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

    let font = editor_font.0.clone();
    let ifont = icon_font.0.clone();

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
        // checkbox — they're implementation detail, not a user
        // choice.
        if extensions_config::is_required(&id) {
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
        .spawn((
            ChildOf(slot_entity),
            ExtensionsDialogContent,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(tokens::SPACING_XS),
                min_width: Val::Px(280.0),
                ..default()
            },
        ))
        .id();

    spawn_section_header(&mut commands, list, "Built-in");
    for (id, label, checked) in builtin_rows {
        commands.spawn((
            ChildOf(list),
            ExtensionCheckbox {
                extension_id: id.clone(),
            },
            checkbox(CheckboxProps::new(label).checked(checked), &font, &ifont),
        ));
    }

    spawn_section_header(&mut commands, list, "Custom");
    if custom_rows.is_empty() {
        commands.spawn((
            ChildOf(list),
            Node {
                padding: UiRect::axes(Val::Px(tokens::SPACING_LG), Val::Px(tokens::SPACING_SM)),
                ..default()
            },
            children![(
                Text::new("No custom extensions installed"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            )],
        ));
    } else {
        for (id, label, checked) in custom_rows {
            commands.spawn((
                ChildOf(list),
                ExtensionCheckbox {
                    extension_id: id.clone(),
                },
                checkbox(CheckboxProps::new(label).checked(checked), &font, &ifont),
            ));
        }
    }

    spawn_install_row(&mut commands, list);
}

/// Compose the install/build buttons plus the shared status line
/// under them. Lives inside `populate_extensions_dialog` so it's
/// rebuilt every time the dialog opens.
fn spawn_install_row(commands: &mut Commands, list: Entity) {
    let row = commands
        .spawn((
            ChildOf(list),
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::axes(Val::Px(tokens::SPACING_LG), Val::Px(tokens::SPACING_SM)),
                row_gap: Val::Px(tokens::SPACING_XS),
                ..default()
            },
        ))
        .id();

    // Only "install a prebuilt .so" lives in the editor: source-tree
    // builds happen at the launcher (File > Home) so every build
    // carries its potential process-restart with it. This keeps
    // mid-session surprises (sudden restart when clicking Build)
    // out of the editor experience.
    commands.spawn((
        ChildOf(row),
        InstallFromFileButton,
        button(
            ButtonProps::new("Install prebuilt dylib…")
                .with_variant(ButtonVariant::Default)
                .with_size(ButtonSize::MD)
                .with_left_icon(Icon::FilePlus),
        ),
    ));

    commands.spawn((
        ChildOf(row),
        InstallStatusText,
        Text::new(String::new()),
        TextFont {
            font_size: tokens::FONT_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
    ));
}

/// Underlined heading matching the Add Component dialog's style.
fn spawn_section_header(commands: &mut Commands, list: Entity, label: &str) {
    let header = commands
        .spawn((
            ChildOf(list),
            Node {
                padding: UiRect::new(
                    Val::Px(tokens::SPACING_LG),
                    Val::Px(tokens::SPACING_LG),
                    Val::Px(tokens::SPACING_MD),
                    Val::Px(tokens::SPACING_XS),
                ),
                width: Val::Percent(100.0),
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
        ))
        .id();

    commands.spawn((
        ChildOf(header),
        Text::new(label.to_string()),
        TextFont {
            font_size: tokens::FONT_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
    ));
}

/// Enable or disable the matching extension when a checkbox commits,
/// then persist the new enabled list.
fn on_extension_checkbox_commit(
    event: On<CheckboxCommitEvent>,
    checkboxes: Query<&ExtensionCheckbox>,
    mut commands: Commands,
) {
    let Ok(cb) = checkboxes.get(event.entity) else {
        return;
    };
    let name = cb.extension_id.clone();
    let checked = event.checked;

    // Belt-and-suspenders: required extensions shouldn't have a
    // checkbox in the first place (see `populate_extensions_dialog`),
    // but if one slipped through we refuse to disable it rather than
    // letting the editor end up in a broken state.
    if !checked && extensions_config::is_required(&name) {
        warn!("Refusing to disable required extension `{name}`");
        return;
    }

    commands.queue(move |world: &mut World| {
        if checked {
            enable_extension(world, &name);
        } else {
            disable_extension(world, &name);
        }
        extensions_config::persist_current_enabled(world);
    });
}

/// Spawn an rfd file picker when the install button is clicked.
/// Skips if a picker is already in flight (rfd can't run two at
/// once on some platforms, and it'd be confusing UX).
fn on_install_button_click(
    event: On<ButtonClickEvent>,
    buttons: Query<(), With<InstallFromFileButton>>,
    mut commands: Commands,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
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
        let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
        world.resource_mut::<InstallStatus>().task = Some(task);
        world.resource_mut::<InstallStatus>().message = Some("Select a dylib file…".into());
    });
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
                // load_from_path handles games in-process: if the
                // name was already loaded, it runs the prior
                // teardown first and then calls the new build.
                // No restart needed.
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
/// install pipeline: peek kind, copy to `extensions/` or `games/`,
/// try to live-load, and set an `InstallStatus` message describing
/// the result.
///
/// Returns `Ok(kind)` on success or `Err(LoadError)` so callers
/// can inspect the failure. Use `LoadError::is_symbol_mismatch()`
/// for "SDK rebuilt, stale project cache" recovery.
pub fn handle_install_from_path(
    world: &mut World,
    src: std::path::PathBuf,
) -> Result<jackdaw_loader::LoadedKind, jackdaw_loader::LoadError> {
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
) -> Result<jackdaw_loader::LoadedKind, jackdaw_loader::LoadError> {
    let target = classify_for_install(&src);
    let dest = match install_picked_file(&src, target) {
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
        Ok(jackdaw_loader::LoadedKind::Extension(name)) => {
            info!("Live-loaded extension `{name}` from {}", dest.display());
            format!("Loaded extension `{name}`. BEI keybinds (if any) activate on next restart.")
        }
        Ok(jackdaw_loader::LoadedKind::Game(name)) => {
            info!("Game `{name}` loaded from {}", dest.display());
            format!("Loaded game `{name}`.")
        }
        Err(err) => {
            warn!("Live-load failed for {}: {err}", dest.display());
            if err.is_symbol_mismatch() {
                // Soft-fail: caller will detect this and run the
                // auto-clean-and-retry recovery path. Don't update
                // the install-status message; the retry UI owns it.
                "SDK mismatch detected; cleaning project cache…".to_string()
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

/// Which directory under the user's config root a given dylib
/// should be installed to.
enum InstallTarget {
    Extension,
    Game,
}

/// Install a built dylib into the per-user subdirectory for its
/// kind. Returns the destination path on success. Creates the
/// directory if missing.
///
/// Uses write-to-tempfile + rename instead of `std::fs::copy` so we
/// never truncate a file that's currently mmapped by the running
/// process. Truncating a live-mapped `.so` corrupts its pages in
/// place and segfaults the editor the next time anything touches
/// the loaded library's code or static data (including `dlopen`
/// walking `/proc/self/maps`).
///
/// Install the picked `.so` into the per-user dir with a **unique
/// filename per install** (e.g., `libmy_game-1745678901234.so`), then
/// clean up any prior sibling files matching the same basename so the
/// dir doesn't accumulate stale copies.
///
/// Why the unique filename: glibc's `dlopen` caches loaded libraries
/// by absolute path after realpath resolution. A second `dlopen` of
/// the same path returns the original handle even if the on-disk
/// file was atomically replaced; the mapping doesn't re-check inode.
/// Hot-reloading a game by overwriting one `libmy_game.so` path
/// would silently return the first-loaded code forever. Giving each
/// install a fresh path forces glibc to mmap the new file. The old
/// mapping stays valid for any currently-held fn pointers (like the
/// catalog's prior `teardown` we call before swapping) until its
/// `libloading::Library` handle is dropped.
///
/// Cleanup after the rename removes any sibling files that share the
/// same stem (e.g. `libmy_game-*.so`), so at most one file per game
/// lives in the dir after a successful install. Cleanup failure is
/// a warning, not an error: the load has already succeeded, and the
/// stale file will be cleaned on the next install.
fn install_picked_file(
    src: &std::path::Path,
    target: InstallTarget,
) -> std::io::Result<std::path::PathBuf> {
    let Some(config) = crate::project::config_dir() else {
        return Err(std::io::Error::other(
            "platform config directory is unavailable",
        ));
    };
    let subdir = match target {
        InstallTarget::Extension => "extensions",
        InstallTarget::Game => "games",
    };
    let dest_dir = config.join(subdir);
    std::fs::create_dir_all(&dest_dir)?;
    let file_name = src.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "picked path has no file name",
        )
    })?;

    // Split `libmy_game.so` into `"libmy_game"` + `".so"` (or `"libmy_game.dylib"`
    // / `"my_game.dll"` on other platforms). We suffix the stem with a
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

/// Peek at the dylib's entry symbol to decide whether it belongs in
/// `extensions/` or `games/`. Falls back to Extension if the peek
/// fails; the caller's own load-from-path will surface the real
/// error on the follow-up dlopen.
fn classify_for_install(path: &std::path::Path) -> InstallTarget {
    match jackdaw_loader::peek_kind(path) {
        Ok(jackdaw_loader::LoadedKind::Game(_)) => InstallTarget::Game,
        _ => InstallTarget::Extension,
    }
}
