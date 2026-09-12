//! The inspector row for a field that names an asset.
//!
//! A field holding a `Handle<T>`, an `Option<Handle<T>>`, or a path a schema
//! asset spells as a string, shows the file it names, with Pick, Clear and New
//! beside it and a drop target for the asset browser's drag. Every action
//! commits through the undoable field edit the scalar rows use, so one choice
//! is one history entry and undo puts the previous path back.

use std::path::{Path, PathBuf};

use bevy::asset::UntypedHandle;
use bevy::picking::events::{DragDrop, Pointer};
use bevy::prelude::*;
use bevy::reflect::PartialReflect;
use jackdaw_api::prelude::AssetKinds;
use jackdaw_feathers::picker::{
    PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text, picker_item,
};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonVariant, IconButtonProps, icon_button},
    field_row::{FieldRowProps, spawn_field_row},
    icons::Icon,
    tokens,
    tooltip::Tooltip,
};

use crate::asset_browser::ActiveAssetDrag;
use crate::asset_index::AssetIndex;

/// Shown in place of a path when the field names nothing.
const NOTHING: &str = "None";

/// Room a file name and the three actions need beside a label before the row
/// wraps the control onto a line of its own.
pub(crate) const ASSET_CONTROL_MIN_WIDTH: f32 = 144.0;

/// How many characters of a file name a row shows before it cuts the rest
/// away. The names in one folder differ at the front, so the front is what a
/// shortened one keeps.
const NAME_BUDGET: usize = 28;

/// The picker entry that hands the choice to the desktop's own file dialog,
/// for an asset kind whose files the project does not index.
const BROWSE: &str = "Browse...";

/// The file extensions an image field offers, for a type with no asset files
/// of its own to list.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "ktx2", "basis", "dds", "tga", "exr", "hdr", "webp", "bmp",
];

/// What a handle row writes to.
#[derive(Clone)]
pub(crate) enum AssetFieldTarget {
    /// A field of the component, or the open asset, the inspector is showing.
    Inspected { source: Entity, type_path: String },
    /// A field of an asset the editor holds a handle to, such as the material
    /// a texture slot sits on.
    Held(UntypedHandle),
}

/// A row showing, and writing, one field that names an asset.
#[derive(Component, Clone)]
pub(crate) struct AssetFieldRow {
    pub(crate) target: AssetFieldTarget,
    pub(crate) field_path: String,
    /// Reflect type path of the asset the field names.
    pub(crate) asset_type_path: String,
    /// The text showing the path, for a row that draws one of its own.
    pub(crate) path_text: Option<Entity>,
}

/// One of the row's actions, and the row it acts on.
#[derive(Component, Clone, Copy)]
pub(crate) struct AssetRowAction {
    row: Entity,
    action: Action,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Pick,
    Clear,
    New,
}

/// The three actions every asset row carries, as the glyph and the words that
/// stand for each.
const ACTIONS: [(Action, Icon, &str, &str); 3] = [
    (
        Action::Pick,
        Icon::FolderOpen,
        "Pick",
        "Choose the file this field names",
    ),
    (
        Action::Clear,
        Icon::X,
        "Clear",
        "Leave this field naming nothing",
    ),
    (
        Action::New,
        Icon::Plus,
        "New",
        "Create a file of this type and name it here",
    ),
];

/// A row whose host writes the path itself, for a slot that loads its file in
/// a way of its own. The hook reports whether it wrote.
#[derive(Component)]
pub(crate) struct AssetFieldWriter(pub(crate) Box<dyn Fn(&mut World, &str) -> bool + Send + Sync>);

/// The picker a row's Pick opened, so its choice reaches the row that asked.
#[derive(Component)]
pub(crate) struct AssetFieldPicker(pub(crate) Entity);

/// Everything a row needs to know about the field it stands for.
#[derive(Clone)]
pub(crate) struct AssetRowProps {
    pub(crate) target: AssetFieldTarget,
    pub(crate) field_path: String,
    pub(crate) asset_type_path: String,
    pub(crate) label: String,
    pub(crate) indent: u8,
}

/// Spawn a row of its own for an asset field: the label, the path, and the
/// actions that change it.
pub(crate) fn spawn_asset_row(
    commands: &mut Commands,
    parent: Entity,
    props: AssetRowProps,
    icon_font: &Handle<Font>,
) -> Entity {
    let (row, control) = if props.label.is_empty() {
        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(tokens::SPACING_XS),
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_width: Val::Px(0.0),
                    ..default()
                },
                ChildOf(parent),
            ))
            .id();
        (row, row)
    } else {
        let field = spawn_field_row(
            commands,
            parent,
            FieldRowProps::new(props.label.clone())
                .indented(props.indent)
                .with_control_min_width(ASSET_CONTROL_MIN_WIDTH),
        );
        commands
            .entity(field.row)
            .entry::<Node>()
            .and_modify(|mut node| {
                node.min_width = Val::Px(0.0);
                node.flex_shrink = 1.0;
            });
        (field.row, field.control)
    };
    let name_box = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::clip(),
                ..default()
            },
            ChildOf(control),
        ))
        .id();
    let path_text = commands
        .spawn((
            Text::new(NOTHING),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_DISABLED),
            Node {
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(name_box),
        ))
        .id();
    let actions = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(control),
        ))
        .id();
    attach_asset_field(commands, row, actions, props, Some(path_text), icon_font);
    row
}

/// Put the actions, the drop target and the row marker on a row another panel
/// already built, such as a material's texture slot.
pub(crate) fn attach_asset_field(
    commands: &mut Commands,
    row: Entity,
    actions: Entity,
    props: AssetRowProps,
    path_text: Option<Entity>,
    icon_font: &Handle<Font>,
) {
    commands.entity(row).insert(AssetFieldRow {
        target: props.target,
        field_path: props.field_path,
        asset_type_path: props.asset_type_path,
        path_text,
    });
    if let Some(text) = path_text {
        commands
            .entity(text)
            .insert((
                TextLayout {
                    linebreak: bevy::text::LineBreak::NoWrap,
                    ..default()
                },
                bevy::picking::hover::Hovered::default(),
                Tooltip::title(NOTHING),
            ))
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                commands.queue(move |world: &mut World| {
                    open_asset_picker(world, row);
                });
            });
    }
    for (action, icon, title, description) in ACTIONS {
        commands.spawn((
            icon_button(
                IconButtonProps::new(icon).variant(ButtonVariant::Ghost),
                icon_font,
            ),
            bevy::picking::hover::Hovered::default(),
            Tooltip::title(title).with_description(description),
            AssetRowAction { row, action },
            ChildOf(actions),
        ));
    }
    commands.entity(row).observe(
        move |mut event: On<Pointer<DragDrop>>,
              mut drag: ResMut<ActiveAssetDrag>,
              mut commands: Commands| {
            let Some(dropped) = drag.path.take().or_else(|| drag.image.take()) else {
                return;
            };
            event.propagate(false);
            commands.queue(move |world: &mut World| {
                accept_asset_drop(world, row, &dropped);
            });
        },
    );
    commands.queue(move |world: &mut World| {
        show_asset_row_path(world, row);
    });
}

/// The asset a field names, ready for a row, or `None` when the field names no
/// asset at all.
pub(crate) fn asset_type_of_field(
    registry: &AppTypeRegistry,
    value: &dyn PartialReflect,
) -> Option<String> {
    let type_id = value.get_represented_type_info()?.type_id();
    let registry = registry.read();
    crate::typed_values::asset_type_path(&registry, type_id)
}

// -- Reading what a row shows ----------------------------------------------

/// The path a row's field names, or `None` when the editor cannot read it.
fn field_path_text(world: &World, row: &AssetFieldRow) -> Option<String> {
    match &row.target {
        AssetFieldTarget::Inspected { source, type_path } => {
            if let Some(text) = crate::definition_assets::asset_field_text(
                world,
                *source,
                type_path,
                &row.field_path,
            ) {
                return Some(text);
            }
            component_field_text(world, *source, type_path, &row.field_path)
        }
        AssetFieldTarget::Held(handle) => {
            crate::definition_assets::handle_field_text(world, handle, &row.field_path)
        }
    }
}

/// The path a field of a component on a live entity names.
fn component_field_text(
    world: &World,
    source: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<String> {
    use bevy::reflect::GetPath as _;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry.get_with_type_path(type_path)?;
    let entity_ref = world.get_entity(source).ok()?;
    let value = super::reflect_fields::inspected_value(world, entity_ref, registration, &registry)?;
    let field = if field_path.is_empty() {
        value.as_partial_reflect()
    } else {
        value.reflect_path(field_path).ok()?
    };
    let server = world.get_resource::<AssetServer>();
    let index = world.get_resource::<AssetIndex>();
    let json = crate::typed_values::asset_path_json(&registry, server, index, field)?;
    Some(json.as_str().unwrap_or_default().to_string())
}

/// Write what the field names into the row's text, for a row that draws one.
pub(crate) fn show_asset_row_path(world: &mut World, row: Entity) {
    let Some(field) = world.get::<AssetFieldRow>(row).cloned() else {
        return;
    };
    let Some(text_entity) = field.path_text else {
        return;
    };
    let Some(path) = field_path_text(world, &field) else {
        return;
    };
    let shown = if path.is_empty() {
        NOTHING.to_string()
    } else {
        shown_name(&path)
    };
    let Ok(mut entity) = world.get_entity_mut(text_entity) else {
        return;
    };
    if entity.get::<Text>().is_some_and(|text| text.0 == shown) {
        return;
    }
    let told = if path.is_empty() {
        NOTHING.to_string()
    } else {
        path.clone()
    };
    entity.insert((
        Text::new(shown),
        Tooltip::title(told),
        TextColor(if path.is_empty() {
            tokens::TEXT_DISABLED
        } else {
            tokens::TEXT_TERTIARY
        }),
    ));
}

/// The file a path names, shortened to what a row can hold, so the row stays
/// one line. The whole path is on the row's tooltip.
pub(crate) fn shown_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.chars().count() <= NAME_BUDGET {
        return name.to_string();
    }
    let kept: String = name.chars().take(NAME_BUDGET - 3).collect();
    format!("{kept}...")
}

/// Keep every asset row showing what its field holds, so an undo, an operator
/// or an edit from another panel moves the text with it.
///
/// A row reading the inspected entity is read only when something it shows
/// changed, the way the scalar rows refresh, so a still panel costs nothing. A
/// row reading an asset of its own, such as a material's texture slot, sits
/// outside that entity and is read every run.
pub(crate) fn refresh_asset_rows(
    world: &mut World,
    mut last_run: Local<Option<bevy::ecs::change_detection::Tick>>,
) {
    let this_run = world.read_change_tick();
    let previous = last_run.replace(this_run);
    let inspected_changed = inspected_rows_changed(world, previous, this_run);
    let rows: Vec<Entity> = world
        .query_filtered::<Entity, With<AssetFieldRow>>()
        .iter(world)
        .collect();
    for row in rows {
        let held = world
            .get::<AssetFieldRow>(row)
            .is_some_and(|field| matches!(field.target, AssetFieldTarget::Held(_)));
        if held || inspected_changed {
            show_asset_row_path(world, row);
        }
    }
}

/// Whether the entity the inspector is showing changed since the last run.
fn inspected_rows_changed(
    world: &World,
    previous: Option<bevy::ecs::change_detection::Tick>,
    this_run: bevy::ecs::change_detection::Tick,
) -> bool {
    let Some(primary) = world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary)
    else {
        return false;
    };
    let Some(previous) = previous else {
        return true;
    };
    let Ok(entity_ref) = world.get_entity(primary) else {
        return false;
    };
    super::reflect_fields::entity_components_changed(entity_ref, previous, this_run)
}

// -- Writing --------------------------------------------------------------

/// Write a path into the field a row stands for, as one undo entry.
pub(crate) fn commit_asset_row(world: &mut World, row: Entity, path: &str) -> bool {
    let Some(field) = world.get::<AssetFieldRow>(row).cloned() else {
        return false;
    };
    if world.get::<AssetFieldWriter>(row).is_some() {
        let Some(writer) = world.entity_mut(row).take::<AssetFieldWriter>() else {
            return false;
        };
        let written = (writer.0)(world, path);
        if let Ok(mut entity) = world.get_entity_mut(row) {
            entity.insert(writer);
        }
        show_asset_row_path(world, row);
        return written;
    }
    let json = serde_json::Value::String(path.to_string());
    let written = match &field.target {
        AssetFieldTarget::Inspected { source, type_path } => {
            select_source(world, *source);
            crate::commands::field_edit_commit(
                world,
                type_path,
                &field.field_path,
                &json,
                "Set asset field",
            );
            field_path_text(world, &field).is_none_or(|now| now == path)
        }
        AssetFieldTarget::Held(handle) => {
            crate::definition_assets::commit_handle_field(world, handle, &field.field_path, &json)
        }
    };
    show_asset_row_path(world, row);
    written
}

/// Put the selection on the entity a row edits, since a field edit writes what
/// is selected.
fn select_source(world: &mut World, source: Entity) {
    if world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary)
        == Some(source)
    {
        return;
    }
    if world.get_entity(source).is_ok() {
        crate::selection::select_only(world, source);
    }
}

// -- Actions ---------------------------------------------------------------

/// Run a row's Pick, Clear or New when its button is clicked.
pub(crate) fn on_asset_row_button(
    event: On<ButtonClickEvent>,
    actions: Query<&AssetRowAction>,
    mut commands: Commands,
) {
    let Ok(&AssetRowAction { row, action }) = actions.get(event.entity) else {
        return;
    };
    commands.queue(move |world: &mut World| match action {
        Action::Pick => open_asset_picker(world, row),
        Action::Clear => {
            commit_asset_row(world, row, "");
        }
        Action::New => new_asset_for_row(world, row),
    });
}

/// The files this project holds of a row's asset type, as the paths that name
/// them.
pub(crate) fn assets_for_row(world: &World, row: &AssetFieldRow) -> Vec<String> {
    if let Some(kind) = world
        .get_resource::<AssetKinds>()
        .and_then(|kinds| kinds.by_type_path(&row.asset_type_path))
    {
        return world
            .get_resource::<AssetIndex>()
            .map(|index| index.paths_of_kind(&kind.kind))
            .unwrap_or_default();
    }
    let extensions = file_extensions(&row.asset_type_path);
    if extensions.is_empty() {
        return Vec::new();
    }
    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return Vec::new();
    };
    use path_slash::PathExt as _;
    jackdaw_bsn::walk_files_with_extensions(&assets, extensions)
        .into_iter()
        .filter_map(|path| {
            Some(
                path.strip_prefix(&assets)
                    .ok()?
                    .to_slash_lossy()
                    .into_owned(),
            )
        })
        .collect()
}

/// The extensions the files of an asset type carry, for a type whose files the
/// project does not index as assets of its own.
fn file_extensions(asset_type_path: &str) -> &'static [&'static str] {
    use bevy::reflect::TypePath as _;

    if asset_type_path == Image::type_path() {
        IMAGE_EXTENSIONS
    } else {
        &[]
    }
}

/// Put up the list of files the row's field can name, in place of the list
/// another row left open.
pub(crate) fn open_asset_picker(world: &mut World, row: Entity) {
    let Some(field) = world.get::<AssetFieldRow>(row).cloned() else {
        return;
    };
    close_open_asset_picker(world);
    let mut items = assets_for_row(world, &field);
    let browsable = !file_extensions(&field.asset_type_path).is_empty();
    if browsable {
        items.push(BROWSE.to_string());
    }
    if items.is_empty() {
        crate::status_bar::notify_warn(
            world,
            format!(
                "this project holds no {}",
                short_type(&field.asset_type_path)
            ),
        );
        return;
    }
    let title = format!("Pick {}", short_type(&field.asset_type_path));
    world.commands().spawn((
        PickerProps::new(spawn_picker_item, pick_asset)
            .items(items)
            .title(title)
            .placeholder(Some("Search assets..")),
        AssetFieldPicker(row),
        crate::EditorEntity,
        crate::BlocksCameraInput,
    ));
    world.flush();
}

/// The last segment of a type path, for a line a person reads.
fn short_type(type_path: &str) -> &str {
    type_path.rsplit("::").next().unwrap_or(type_path)
}

/// Drop the list a row put up, so a second Pick replaces it rather than
/// stacking another list over it.
pub(crate) fn close_open_asset_picker(world: &mut World) {
    let open: Vec<Entity> = world
        .query_filtered::<Entity, With<AssetFieldPicker>>()
        .iter(world)
        .collect();
    for picker in open {
        if let Ok(entity) = world.get_entity_mut(picker) {
            entity.despawn();
        }
    }
}

fn spawn_picker_item(
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

fn pick_asset(
    input: In<SelectInput>,
    items: Query<&PickerItems<String>>,
    pickers: Query<&AssetFieldPicker>,
    mut commands: Commands,
) -> Result {
    let picker = input.entities.picker;
    let chosen = items.get(picker)?.at(input.index)?.clone();
    let row = pickers.get(picker).map(|picker| picker.0);
    commands.entity(picker).try_despawn();
    let Ok(row) = row else { return Ok(()) };
    commands.queue(move |world: &mut World| {
        if chosen == BROWSE {
            browse_for_asset(world, row);
            return;
        }
        commit_asset_row(world, row, &chosen);
    });
    Ok(())
}

/// Hand the choice to the desktop's file dialog, for a file the project does
/// not hold as an asset of its own. The dialog opens beside the asset the
/// field belongs to, which is where its files usually sit.
fn browse_for_asset(world: &mut World, row: Entity) {
    let Some(field) = world.get::<AssetFieldRow>(row).cloned() else {
        return;
    };
    let extensions = file_extensions(&field.asset_type_path);
    if extensions.is_empty() {
        return;
    }
    let beside = folder_of_asset(world, &field);
    let dialog = match beside {
        Some(folder) => crate::native_dialog::dialog_starting_at(world, Some(folder)),
        None => {
            crate::native_dialog::file_dialog(world, crate::native_dialog::DialogPurpose::Texture)
        }
    }
    .set_title(format!("Select a file for {}", field.field_path))
    .add_filter(short_type(&field.asset_type_path), extensions);
    let task =
        bevy::tasks::AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(AssetBrowsePick { task, row });
}

/// A file dialog a row opened, waiting on what the user chose.
#[derive(Resource)]
struct AssetBrowsePick {
    task: bevy::tasks::Task<Option<rfd::FileHandle>>,
    row: Entity,
}

/// Assign what a row's file dialog came back with.
pub(crate) fn poll_asset_browse_pick(world: &mut World) {
    let Some(pick) = world.get_resource::<AssetBrowsePick>() else {
        return;
    };
    if !pick.task.is_finished() {
        return;
    }
    let Some(mut pick) = world.remove_resource::<AssetBrowsePick>() else {
        return;
    };
    let Some(chosen) = bevy::tasks::block_on(bevy::tasks::poll_once(&mut pick.task)).flatten()
    else {
        return;
    };
    let file = chosen.path().to_path_buf();
    crate::native_dialog::remember_pick(world, crate::native_dialog::DialogPurpose::Texture, &file);
    let Some(relative) = crate::asset_index::indexed_path(world, &file) else {
        crate::status_bar::notify_error(
            world,
            "that file is outside this project's assets".to_string(),
        );
        return;
    };
    use path_slash::PathExt as _;
    let path = relative.to_slash_lossy().into_owned();
    commit_asset_row(world, pick.row, &path);
}

// -- New -------------------------------------------------------------------

/// Write a fresh asset of the field's type beside whatever is being edited and
/// assign it, without taking the inspector off the field.
fn new_asset_for_row(world: &mut World, row: Entity) {
    let Some(field) = world.get::<AssetFieldRow>(row).cloned() else {
        return;
    };
    let Some(kind) = world
        .get_resource::<AssetKinds>()
        .and_then(|kinds| kinds.by_type_path(&field.asset_type_path))
        .map(|kind| kind.kind.clone())
    else {
        crate::status_bar::notify_error(
            world,
            format!("nothing creates a {}", short_type(&field.asset_type_path)),
        );
        return;
    };
    let dir = new_asset_dir(world, &field);
    let Some((_, _, path)) =
        crate::definition_assets::create_definition(world, &kind, None, dir.as_deref())
    else {
        crate::status_bar::notify_error(world, format!("no {kind} could be written there"));
        return;
    };
    use path_slash::PathExt as _;
    let path = path.to_slash_lossy().into_owned();
    commit_asset_row(world, row, &path);
}

/// Where a row's New writes: beside the asset the field belongs to, and in the
/// folder the browser is showing for anything else.
fn new_asset_dir(world: &World, field: &AssetFieldRow) -> Option<PathBuf> {
    folder_of_asset(world, field).or_else(|| crate::definition_assets::new_definition_dir(world))
}

/// The folder holding the asset whose field a row writes, when that asset came
/// from a file and the folder is still there.
fn folder_of_asset(world: &World, field: &AssetFieldRow) -> Option<PathBuf> {
    let file = match &field.target {
        AssetFieldTarget::Inspected { .. } => crate::definition_assets::open_definition_path(world),
        AssetFieldTarget::Held(handle) => world
            .get_resource::<AssetIndex>()
            .and_then(|index| index.by_handle(handle))
            .map(|entry| entry.path.clone()),
    };
    folder_beside(&crate::asset_index::assets_dir(world)?, &file?)
}

/// The folder an asset file sits in, as an absolute path, when it is there.
fn folder_beside(assets_root: &Path, asset: &Path) -> Option<PathBuf> {
    let folder = assets_root.join(asset.parent()?);
    folder.is_dir().then_some(folder)
}

// -- Dropping --------------------------------------------------------------

/// Take a file dragged out of the asset browser, when it holds what the field
/// names, and say so when it does not.
fn accept_asset_drop(world: &mut World, row: Entity, dropped: &Path) {
    let Some(field) = world.get::<AssetFieldRow>(row).cloned() else {
        return;
    };
    let Some(relative) = crate::asset_index::indexed_path(world, dropped) else {
        crate::status_bar::notify_error(world, "that file is outside this project's assets");
        return;
    };
    use path_slash::PathExt as _;
    let path = relative.to_slash_lossy().into_owned();
    if !assets_for_row(world, &field)
        .iter()
        .any(|held| held == &path)
    {
        crate::status_bar::notify_error(
            world,
            format!("{path} holds no {}", short_type(&field.asset_type_path)),
        );
        return;
    }
    commit_asset_row(world, row, &path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectConfig, ProjectRoot};

    fn row_naming(asset_type_path: &str) -> AssetFieldRow {
        AssetFieldRow {
            target: AssetFieldTarget::Inspected {
                source: Entity::PLACEHOLDER,
                type_path: String::new(),
            },
            field_path: "base_color_texture".to_string(),
            asset_type_path: asset_type_path.to_string(),
            path_text: None,
        }
    }

    /// No asset file holds an image, so an image field offers the image files
    /// the project holds rather than nothing.
    #[test]
    fn an_image_field_offers_the_image_files_the_project_holds() {
        use bevy::reflect::TypePath as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        let textures = tmp.path().join("assets/textures");
        std::fs::create_dir_all(&textures).expect("a textures folder");
        std::fs::write(textures.join("rock.png"), []).expect("an image file");
        std::fs::write(textures.join("rock.bsn"), []).expect("a document beside it");
        let mut world = World::new();
        world.insert_resource(ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: ProjectConfig::default(),
        });

        assert_eq!(
            assets_for_row(&world, &row_naming(Image::type_path())),
            vec!["textures/rock.png".to_string()],
            "the files offered are the ones an image field can name",
        );
    }

    /// A dialog a row opens starts in the folder the asset it edits sits in.
    #[test]
    fn a_browse_starts_in_the_folder_holding_the_asset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let assets = tmp.path();
        std::fs::create_dir_all(assets.join("materials/stone")).expect("the material folder");

        assert_eq!(
            folder_beside(assets, Path::new("materials/stone/granite.bsn")),
            Some(assets.join("materials/stone")),
        );
        assert_eq!(
            folder_beside(assets, Path::new("materials/gone/granite.bsn")),
            None,
            "a folder that is not there sends the dialog somewhere else",
        );
    }

    /// A type with neither asset files nor an extension of its own offers
    /// nothing rather than the whole project.
    #[test]
    fn a_type_with_no_files_of_its_own_offers_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("assets")).expect("an assets folder");
        let mut world = World::new();
        world.insert_resource(ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: ProjectConfig::default(),
        });

        assert!(assets_for_row(&world, &row_naming("my_game::Mystery")).is_empty());
    }
}
