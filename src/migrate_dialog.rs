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

use bevy::picking::events::{Click, Pointer};
use bevy::prelude::*;
use jackdaw_feathers::{icons::EditorFont, tokens};
use std::path::{Path, PathBuf};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PendingMigration>()
        .init_resource::<PendingFileConversion>()
        .add_systems(OnEnter(crate::AppState::Editor), prompt_for_legacy_project);
}

/// `Some(count)` while the migration dialog is displayed, holding the number
/// of legacy files it offered to convert.
#[derive(Resource, Default)]
pub struct PendingMigration {
    pub file_count: Option<usize>,
}

/// Marker on the dialog root (the scrim node).
#[derive(Component)]
pub struct MigrateDialogRoot;

/// The dialog's two actions.
#[derive(Component, Clone, Copy)]
pub enum MigrateDialogButton {
    Convert,
    NotNow,
}

/// How a confirmed per-file conversion continues: opening a scene tab or
/// loading into the single-scene flow.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConversionOpenTarget {
    Tab,
    Scene,
}

/// The `.jsn` file a per-file conversion prompt is waiting on, with the
/// open flow to resume when the user confirms.
#[derive(Resource, Default)]
pub struct PendingFileConversion {
    pub pending: Option<(PathBuf, ConversionOpenTarget)>,
}

/// Marker on the per-file conversion dialog root (the scrim node).
#[derive(Component)]
pub struct ConvertFileDialogRoot;

/// The per-file dialog's two actions.
#[derive(Component, Clone, Copy)]
pub enum ConvertFileButton {
    ConvertAndOpen,
    Cancel,
}

/// Route an interactive open through the conversion gate: `.bsn` paths (and
/// worlds with no dialog font, e.g. headless harnesses) open immediately;
/// a `.jsn` path raises a confirmation, and the open resumes only when the
/// user picks Convert and Open. The open paths themselves convert the file
/// on disk, so confirming both converts and opens.
pub fn request_open_with_conversion(world: &mut World, path: &Path, target: ConversionOpenTarget) {
    let is_legacy = path.extension().is_some_and(|e| e == "jsn");
    let headless = world.get_resource::<EditorFont>().is_none();
    if !is_legacy || headless {
        run_open(world, path, target);
        return;
    }
    world.resource_mut::<PendingFileConversion>().pending = Some((path.to_path_buf(), target));
    spawn_convert_file_dialog(world, path);
}

fn run_open(world: &mut World, path: &Path, target: ConversionOpenTarget) {
    match target {
        ConversionOpenTarget::Tab => crate::scenes::operators::scene_open_system(world, path),
        ConversionOpenTarget::Scene => crate::scene_io::load_scene_from_file(world, path),
    }
}

/// Count the project's legacy `.jsn` files (scenes, prefabs, catalog;
/// config and backups excluded).
pub fn count_legacy_files(root: &std::path::Path) -> usize {
    let mut files = Vec::new();
    crate::jsn_to_bsn::collect_jsn_files(root, &mut files);
    files.len()
}

/// On entering the editor, offer to convert any legacy files found in the
/// opened project.
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
    spawn_migrate_dialog(world, count);
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

/// Spawn the migration dialog. Skips UI when `EditorFont` is absent
/// (headless tests); `PendingMigration` is still set so logic-level tests
/// can drive [`resolve_migration`] directly.
fn spawn_migrate_dialog(world: &mut World, count: usize) {
    let Some(editor_font) = world.get_resource::<EditorFont>().map(|f| f.0.clone()) else {
        return;
    };

    let scrim = world
        .spawn((
            MigrateDialogRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(1000),
        ))
        .id();

    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(20.0)),
                max_width: Val::Px(460.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    world.spawn((
        Text::new("Legacy Scene Format"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    world.spawn((
        Text::new(format!(
            "This project contains {count} scene file(s) in the legacy .jsn \
             format. Convert them to .bsn now? Originals are kept as \
             .jsn.bak backups. Legacy files cannot be opened until they \
             are converted."
        )),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    let button_row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                column_gap: Val::Px(8.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    spawn_button(
        world,
        button_row,
        editor_font.clone(),
        "Not Now",
        MigrateDialogButton::NotNow,
        tokens::TOOLBAR_BG,
    );
    spawn_button(
        world,
        button_row,
        editor_font,
        "Convert",
        MigrateDialogButton::Convert,
        tokens::SELECTED_BG,
    );
}

fn spawn_button(
    world: &mut World,
    parent: Entity,
    editor_font: Handle<Font>,
    label: &str,
    kind: MigrateDialogButton,
    bg: Color,
) {
    let btn = world
        .spawn((
            kind,
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(bg),
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(label.to_string()),
        TextFont {
            font: editor_font.into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Pickable::IGNORE,
        ChildOf(btn),
    ));

    world.entity_mut(btn).observe(on_migrate_button_click);
}

fn on_migrate_button_click(
    trigger: On<Pointer<Click>>,
    buttons: Query<&MigrateDialogButton>,
    dialog: Query<Entity, With<MigrateDialogRoot>>,
    mut commands: Commands,
) -> Result<(), BevyError> {
    let Ok(kind) = buttons.get(trigger.event_target()) else {
        return Ok(());
    };
    let convert = matches!(kind, MigrateDialogButton::Convert);

    for root in dialog.iter() {
        commands.entity(root).despawn();
    }
    commands.queue(move |world: &mut World| {
        resolve_migration(world, convert);
    });
    Ok(())
}

/// Spawn the per-file conversion confirmation. Skips UI when `EditorFont`
/// is absent; `request_open_with_conversion` never routes here headless.
fn spawn_convert_file_dialog(world: &mut World, path: &Path) {
    let Some(editor_font) = world.get_resource::<EditorFont>().map(|f| f.0.clone()) else {
        return;
    };

    let scrim = world
        .spawn((
            ConvertFileDialogRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(1000),
        ))
        .id();

    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(20.0)),
                max_width: Val::Px(460.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    world.spawn((
        Text::new("Legacy Scene Format"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    world.spawn((
        Text::new(format!(
            "{file_name} uses the legacy .jsn format, which can no longer be \
             opened directly. Convert it to .bsn and open? The original is \
             kept as a .jsn.bak backup."
        )),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    let button_row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                column_gap: Val::Px(8.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    spawn_convert_file_button(
        world,
        button_row,
        editor_font.clone(),
        "Cancel",
        ConvertFileButton::Cancel,
        tokens::TOOLBAR_BG,
    );
    spawn_convert_file_button(
        world,
        button_row,
        editor_font,
        "Convert and Open",
        ConvertFileButton::ConvertAndOpen,
        tokens::SELECTED_BG,
    );
}

fn spawn_convert_file_button(
    world: &mut World,
    parent: Entity,
    editor_font: Handle<Font>,
    label: &str,
    kind: ConvertFileButton,
    bg: Color,
) {
    let btn = world
        .spawn((
            kind,
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(bg),
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(label.to_string()),
        TextFont {
            font: editor_font.into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Pickable::IGNORE,
        ChildOf(btn),
    ));

    world.entity_mut(btn).observe(on_convert_file_button_click);
}

fn on_convert_file_button_click(
    trigger: On<Pointer<Click>>,
    buttons: Query<&ConvertFileButton>,
    dialog: Query<Entity, With<ConvertFileDialogRoot>>,
    mut commands: Commands,
) -> Result<(), BevyError> {
    let Ok(kind) = buttons.get(trigger.event_target()) else {
        return Ok(());
    };
    let convert = matches!(kind, ConvertFileButton::ConvertAndOpen);

    for root in dialog.iter() {
        commands.entity(root).despawn();
    }
    commands.queue(move |world: &mut World| {
        let pending = world.resource_mut::<PendingFileConversion>().pending.take();
        let Some((path, target)) = pending else {
            return;
        };
        if convert {
            run_open(world, &path, target);
        } else {
            info!(
                "Left {} unconverted; it cannot be opened until converted",
                path.display()
            );
        }
    });
    Ok(())
}
