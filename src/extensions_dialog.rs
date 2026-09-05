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
};
use jackdaw_feathers::{
    dialog::{CloseDialogEvent, DialogChildrenSlot, OpenDialogEvent},
    tooltip::Tooltip,
};
use rfd::{AsyncFileDialog, FileHandle};
#[cfg(feature = "dylib")]
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

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
    let installed_names: std::collections::HashSet<String> =
        jackdaw_loader::package::list_installed()
            .unwrap_or_default()
            .into_iter()
            .map(|extension| extension.manifest.id)
            .collect();
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
        spawn_extension_row(&mut commands, list, id, label, checked, false);
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
            let removable = installed_names.contains(&id);
            spawn_extension_row(&mut commands, list, id, label, checked, removable);
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
    removable: bool,
) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                ..Default::default()
            },
            ChildOf(list),
        ))
        .id();
    let mut checkbox = commands.spawn_scene(extension_checkbox(id.clone(), label));
    if checked {
        // Checkboxes don't seed their own `Checked` state, so an enabled
        // extension is marked here at spawn.
        checkbox.insert(Checked);
    }
    checkbox.insert(ChildOf(row));
    if removable {
        commands
            .spawn_scene(extension_remove_button(id))
            .insert(ChildOf(row));
    }
}

fn extension_remove_button(id: String) -> impl Scene {
    bsn! {
        @FeathersButton {
            @caption: bsn! { Text("Uninstall") ThemedText }
        }
        on(move |_: On<Activate>, mut commands: Commands| {
            let id = id.clone();
            commands.queue(move |world: &mut World| {
                disable_extension(world, &id);
                match jackdaw_loader::package::uninstall(&id) {
                    Ok(true) => {
                        jackdaw_api_internal::lifecycle::unregister_dylib_extension(world, &id);
                        jackdaw_api_internal::extensions_config::set_extension_enabled(&id, false);
                        world.resource_mut::<InstallStatus>().message =
                            Some(format!("Uninstalled `{id}`. Retired files will be cleaned next launch."));
                        let mut query =
                            world.query_filtered::<Entity, With<ExtensionsDialogContent>>();
                        let roots: Vec<Entity> = query.iter(world).collect();
                        for root in roots {
                            if let Ok(entity) = world.get_entity_mut(root) {
                                entity.despawn();
                            }
                        }
                    }
                    Ok(false) => {
                        world.resource_mut::<InstallStatus>().message =
                            Some(format!("`{id}` is not a marketplace installation."));
                    }
                    Err(error) => {
                        world.resource_mut::<InstallStatus>().message =
                            Some(format!("Uninstall failed: {error}"));
                    }
                }
            });
        })
    }
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
/// Marketplace bundles install and activate without restarting.
fn spawn_install_row(commands: &mut Commands, list: Entity) {
    let row = commands
        .spawn_scene(install_row_container())
        .insert(ChildOf(list))
        .id();

    commands.spawn_scene(install_button()).insert((
        ChildOf(row),
        Tooltip::title("Install Extension").with_description(
            "Pick a signed .jdext bundle. Jackdaw verifies compatibility and \
             publisher trust before loading its native code.",
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

/// Button that opens an rfd file picker for a signed bundle. Skips if a
/// picker is already in flight; rfd can't run two at once on some
/// platforms, and it would be confusing UX.
fn install_button() -> impl Scene {
    bsn! {
        @FeathersButton {
            @caption: bsn! { Text("Install .jdext bundle...") ThemedText }
        }
        on(|_: On<Activate>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                if world.resource::<InstallStatus>().task.is_some() {
                    return;
                }
                let dialog =
                    AsyncFileDialog::new().add_filter("Jackdaw extension bundle", &["jdext"]);
                let task =
                    AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
                world.resource_mut::<InstallStatus>().task = Some(task);
                world.resource_mut::<InstallStatus>().message =
                    Some("Select a signed .jdext bundle...".into());
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

/// Route a signed bundle or a development-build dylib through the live loader.
/// Raw dylibs are accepted only by internal source-build callers; the user
/// picker exposes `.jdext` exclusively and never persists loose libraries.
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
    #[cfg(not(feature = "dylib"))]
    {
        let _ = (src, extension_dialogs);
        let message = "This self-contained Jackdaw build cannot safely load native extensions. \
            Install a precompiled Jackdaw release, or rebuild with `--features dylib`."
            .to_string();
        world.resource_mut::<InstallStatus>().message = Some(message.clone());
        Err(jackdaw_loader::LoadError::InstallIo(message))
    }

    #[cfg(feature = "dylib")]
    {
        let is_bundle = src.extension().and_then(|value| value.to_str()) == Some("jdext");
        let mut installed_id = None;
        let library = if is_bundle {
            let first = jackdaw_loader::package::install_bundle(
                &src,
                jackdaw_loader::package::TrustDecision::RequireTrusted,
            );
            let installed = match first {
                Ok(installed) => installed,
                Err(jackdaw_loader::package::PackageError::TrustRequired {
                    publisher,
                    fingerprint,
                }) => {
                    let result = MessageDialog::new()
                        .set_level(MessageLevel::Warning)
                        .set_title("Trust native extension publisher?")
                        .set_description(format!(
                            "`{publisher}` ({fingerprint}) can execute native code with \
                         your full user permissions. Trust this publisher and install?"
                        ))
                        .set_buttons(MessageButtons::YesNo)
                        .show();
                    if result != MessageDialogResult::Yes {
                        let error = "publisher trust was not granted".to_string();
                        world.resource_mut::<InstallStatus>().message =
                            Some(format!("Install cancelled: {error}"));
                        return Err(jackdaw_loader::LoadError::InstallIo(error));
                    }
                    jackdaw_loader::package::install_bundle(
                        &src,
                        jackdaw_loader::package::TrustDecision::TrustPublisher,
                    )
                    .map_err(|error| jackdaw_loader::LoadError::InstallIo(error.to_string()))?
                }
                Err(error) => {
                    world.resource_mut::<InstallStatus>().message =
                        Some(format!("Install failed: {error}"));
                    return Err(jackdaw_loader::LoadError::InstallIo(error.to_string()));
                }
            };
            installed_id = Some(installed.manifest.id);
            installed.library_path
        } else {
            src
        };

        let result = if is_bundle {
            jackdaw_loader::load_installed_from_path(world, &library)
        } else {
            jackdaw_loader::load_from_path(world, &library)
        };
        let msg = match &result {
            Ok(name) => {
                info!("Live-loaded extension `{name}` from {}", library.display());
                format!("Loaded extension `{name}`.")
            }
            Err(err) => {
                warn!("Live-load failed for {}: {err}", library.display());
                let rollback_message = if let Some(id) = installed_id.as_deref() {
                    match jackdaw_loader::package::rollback(id) {
                        Ok(Some(previous)) => {
                            let restored = jackdaw_loader::load_installed_from_path(
                                world,
                                &previous.library_path,
                            );
                            Some(match restored {
                                Ok(_) => format!(
                                    "Update failed: {err}. Restored {}.",
                                    previous.manifest.version
                                ),
                                Err(restore_error) => format!(
                                    "Update failed: {err}. Rollback activation also failed: \
                                 {restore_error}"
                                ),
                            })
                        }
                        Ok(None) => {
                            let _ = jackdaw_loader::package::uninstall(id);
                            Some(format!(
                                "Activation failed: {err}. Installation was removed."
                            ))
                        }
                        Err(rollback_error) => {
                            warn!("Could not roll back `{id}`: {rollback_error}");
                            Some(format!(
                                "Activation failed: {err}. Rollback failed: {rollback_error}"
                            ))
                        }
                    }
                } else {
                    None
                };
                if let Some(message) = rollback_message {
                    message
                } else if err.is_symbol_mismatch() {
                    // Soft-fail: caller will detect this and run the
                    // auto-clean-and-retry recovery path. Don't update
                    // the install-status message; the retry UI owns it.
                    "SDK mismatch detected; cleaning project cache...".to_string()
                } else {
                    format!(
                        "Installed to {}, but live activation failed: {err}.",
                        library.display()
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
}
