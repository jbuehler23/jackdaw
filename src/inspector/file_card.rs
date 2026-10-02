//! The inspector card for the file selected in the Project window.
//!
//! An asset file already has a card of its own, so this one answers for the
//! rest: an image, with what its header says and the action that applies it,
//! and a scene or a prefab, with the name its root carries and the button that
//! opens it in a tab.
//!
//! The References list every card ends with is built here too, since it says
//! the same thing about a file whichever card is standing for it.

use std::path::{Path, PathBuf};

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use jackdaw_api::op::Operator as _;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::icons::{Icon, IconFont};
use jackdaw_feathers::panel_card::{
    DisclosureSection, PanelCardCollapseState, PanelCardProps, spawn_panel_card,
};
use jackdaw_feathers::tokens;

use crate::EditorEntity;
use crate::asset_files::AssetFileKind;
use crate::texture_files::{AssetPreviewState, TextureInfo, is_image_file_path};

use super::definition_card::DefinitionCard;
use super::{ComponentDisplay, ComponentDisplayTypePath, ComponentName};

/// The type path the card is filed under, so a rebuild can find it.
const FILE_CARD_TYPE: &str = "file_card";

/// The file the inspector is showing, on an editor entity of its own so the
/// selection carries it exactly as it carries an entity.
#[derive(Component)]
#[require(EditorEntity)]
pub struct SelectedFile {
    pub path: PathBuf,
    pub kind: AssetFileKind,
}

/// The entity carrying the selected file, if one is selected.
#[derive(Resource, Default)]
pub struct OpenFileCard(pub Option<Entity>);

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<OpenFileCard>().add_systems(
        Update,
        (
            drop_a_file_that_is_gone,
            drop_a_definition_that_is_gone,
            drop_a_file_the_selection_left,
            rebuild_on_preview_change,
        )
            .run_if(in_state(crate::AppState::Editor)),
    );
}

/// Put the card for `path` in the inspector without opening a tab. A file
/// holding a registered asset kind has a card of its own, so it goes through
/// the operator that opens one.
pub fn show_file(world: &mut World, path: &Path) {
    let holds_an_asset = {
        let Some(kinds) = world.get_resource::<jackdaw_api::prelude::AssetKinds>() else {
            return;
        };
        crate::asset_files::read_asset_kind(path, kinds)
            .type_path()
            .is_some_and(|type_path| kinds.by_type_path(type_path).is_some())
    };
    clear_selected_file(world);
    if holds_an_asset {
        crate::definition_assets::open_definition_file(world, path);
        return;
    }
    crate::definition_assets::close_open_definition(world);

    let Some(kind) = world
        .get_resource::<jackdaw_api::prelude::AssetKinds>()
        .map(|kinds| crate::asset_files::read_asset_kind(path, kinds))
    else {
        return;
    };
    if is_image_file_path(path) {
        let info = {
            let server = world.resource::<AssetServer>();
            TextureInfo::read(path, server)
        };
        if let Some(mut preview) = world.get_resource_mut::<AssetPreviewState>() {
            preview.show(path.to_path_buf(), info);
        }
    } else if let Some(mut preview) = world.get_resource_mut::<AssetPreviewState>() {
        preview.clear();
    }

    let entity = world
        .spawn((
            Name::new(file_name(path)),
            SelectedFile {
                path: path.to_path_buf(),
                kind,
            },
        ))
        .id();
    if let Some(mut open) = world.get_resource_mut::<OpenFileCard>() {
        open.0 = Some(entity);
    }
    crate::selection::select_only(world, entity);
}

/// Drop the entity a file selection was carried on. The selection it holds is
/// cleared with it, so the inspector falls back to whatever is selected next.
pub fn clear_selected_file(world: &mut World) {
    let Some(entity) = world
        .get_resource_mut::<OpenFileCard>()
        .and_then(|mut open| open.0.take())
    else {
        return;
    };
    if let Ok(entity_mut) = world.get_entity_mut(entity) {
        entity_mut.despawn();
    }
    if world
        .get_resource::<crate::selection::Selection>()
        .is_some_and(|selection| selection.entities.contains(&entity))
    {
        crate::selection::clear_selection_in_world(world);
    }
    if let Some(mut preview) = world.get_resource_mut::<AssetPreviewState>() {
        preview.clear();
    }
}

/// A file deleted or renamed while its card was up leaves the card standing
/// for nothing, so take it down.
fn drop_a_file_that_is_gone(
    open: Res<OpenFileCard>,
    files: Query<&SelectedFile>,
    mut commands: Commands,
) {
    let Some(entity) = open.0 else {
        return;
    };
    let Ok(file) = files.get(entity) else {
        return;
    };
    if file.path.exists() {
        return;
    }
    commands.queue(|world: &mut World| {
        clear_selected_file(world);
    });
}

/// An asset file deleted while its own card was up leaves that card saving
/// through a path that is no longer there, so close it.
fn drop_a_definition_that_is_gone(
    open: Res<crate::definition_assets::OpenDefinition>,
    definitions: Query<&crate::definition_assets::DefinitionAssetEdit>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) {
    let Some(entity) = open.0 else {
        return;
    };
    let Ok(edit) = definitions.get(entity) else {
        return;
    };
    let Some(project) = project else {
        return;
    };
    if project.assets_dir().join(&edit.path).exists() {
        return;
    }
    commands.queue(|world: &mut World| {
        crate::definition_assets::close_open_definition(world);
    });
}

/// The selection moved to something else: an entity in the outliner, or an
/// asset file with a card of its own. The file card stands for nothing now.
fn drop_a_file_the_selection_left(
    open: Res<OpenFileCard>,
    selection: Res<crate::selection::Selection>,
    mut commands: Commands,
) {
    let Some(entity) = open.0 else {
        return;
    };
    if selection.primary() == Some(entity) {
        return;
    }
    commands.queue(|world: &mut World| {
        clear_selected_file(world);
    });
}

/// Draw the open file card again, for an edit that changes what it shows.
pub fn refresh_file_card(world: &mut World) {
    let Some(entity) = world.get_resource::<OpenFileCard>().and_then(|open| open.0) else {
        return;
    };
    if let Ok(mut card) = world.get_entity_mut(entity) {
        card.insert(super::InspectorDirty);
    }
}

/// Stepping an array texture's layer changes what the card draws, so mark it
/// for a rebuild.
fn rebuild_on_preview_change(
    preview: Res<AssetPreviewState>,
    open: Res<OpenFileCard>,
    mut commands: Commands,
) {
    if !preview.is_changed() {
        return;
    }
    let Some(entity) = open.0 else {
        return;
    };
    commands.entity(entity).try_insert(super::InspectorDirty);
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The name a document's first root carries, which is what a scene or a prefab
/// is called wherever it is used.
fn root_name(path: &Path) -> Option<String> {
    let text = jackdaw_bsn::read_document_text(path).ok()?;
    text.lines()
        .filter_map(|line| line.trim().strip_prefix('#'))
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

/// Build the card for the file `source` stands for, under `inspector`.
pub(crate) fn fill_file_card(world: &mut World, inspector: Entity, source: Entity) {
    let Some((path, kind)) = world
        .get::<SelectedFile>(source)
        .map(|file| (file.path.clone(), file.kind.clone()))
    else {
        return;
    };
    let icon_font = world.resource::<IconFont>().0.clone();
    let title = file_name(&path);
    let icon = match &kind {
        AssetFileKind::Prefab => Icon::Package,
        _ if is_image_file_path(&path) => Icon::Image,
        _ => Icon::File,
    };

    let body = {
        let mut state: SystemState<Commands> = SystemState::new(world);
        let Ok(mut commands) = state.get_mut(world) else {
            return;
        };
        let card = spawn_panel_card(
            &mut commands,
            inspector,
            PanelCardProps::new(&title).with_icon(icon),
            &icon_font,
            &PanelCardCollapseState::default(),
        );
        commands.entity(card.section).insert((
            ComponentDisplay,
            DefinitionCard,
            ComponentName(title.clone()),
            ComponentDisplayTypePath(FILE_CARD_TYPE.to_string()),
        ));
        commands
            .entity(card.disclosure)
            .insert(DisclosureSection(card.section));
        commands
            .entity(card.body)
            .insert(super::ComponentDisplayBody);
        let body = card.body;
        state.apply(world);
        body
    };

    fill_body(world, body, &path, &kind);
}

fn fill_body(world: &mut World, body: Entity, path: &Path, kind: &AssetFileKind) {
    let as_string = path.to_string_lossy().into_owned();
    let shown_path = crate::asset_index::indexed_path(world, path)
        .map(|indexed| indexed.to_string_lossy().into_owned())
        .unwrap_or_else(|| as_string.clone());

    if is_image_file_path(path) {
        fill_image_body(world, body, path, &as_string, &shown_path);
        spawn_references(world, body, path);
        return;
    }

    let is_document = matches!(kind, AssetFileKind::Prefab) || jackdaw_bsn::is_document_path(path);
    spawn_row(world, body, "Path", &shown_path);
    if is_document {
        let name = root_name(path).unwrap_or_else(|| jackdaw_bsn::path_stem(path));
        let label = match kind {
            AssetFileKind::Prefab => "Prefab",
            _ => "Scene",
        };
        spawn_row(world, body, label, &name);
        spawn_action(
            world,
            body,
            "Open",
            crate::project_window::ProjectOpenOp::ID,
            &as_string,
        );
    }
    if is_model_file(path) {
        spawn_model_lod_rows(world, body, path);
    }
    spawn_references(world, body, path);
}

fn is_model_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "gltf" | "glb"))
}

/// The model's levels of detail as its import settings hold them, and the
/// buttons that find them again and write them.
fn spawn_model_lod_rows(world: &mut World, body: Entity, path: &Path) {
    use jackdaw_scene_types::model_import::{LevelShow, LodImportSource, ModelLodIndex};

    let assets = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir);
    let asset_path = assets
        .as_deref()
        .and_then(|assets| path.strip_prefix(assets).ok())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let index = world.resource::<ModelLodIndex>();
    let lod = if index.is_known(&asset_path) {
        index.get(&asset_path).map(|lod| (**lod).clone())
    } else {
        crate::model_lod::levels_on_disk(path)
    };
    let unsaved = world
        .resource::<crate::model_lod::UnsavedModelSettings>()
        .0
        .contains(&asset_path);

    let Some(lod) = lod else {
        spawn_row(world, body, "LOD", "None");
        spawn_action(
            world,
            body,
            "Import LOD Levels",
            crate::model_lod::ModelLodImportOp::ID,
            &asset_path,
        );
        return;
    };
    let source = match lod.source {
        LodImportSource::Authored => "set by hand",
        LodImportSource::SiblingFiles => "from files beside the model",
        LodImportSource::NodeSuffixes => "from nodes in the model",
    };
    let summary = format!(
        "{} levels, {source}{}",
        lod.levels.len(),
        if unsaved { ", not saved" } else { "" }
    );
    spawn_row(world, body, "LOD", &summary);
    crate::lod_bar::spawn_model_controls(world, body, &asset_path);
    for (index, level) in lod.levels.iter().enumerate() {
        let shows = match &level.show {
            LevelShow::Model => "The model".to_string(),
            LevelShow::File(file) => file.clone(),
            LevelShow::Nodes(nodes) => nodes.join(", "),
        };
        let triangles = triangles_of(world, &asset_path, &level.show)
            .map(|count| format!(", {count} triangles"))
            .unwrap_or_default();
        let value = format!("{shows}{triangles}, from {}", percent(level.screen_height));
        spawn_row(world, body, &format!("LOD{index}"), &value);
    }
    if let Some(last) = lod.levels.last() {
        spawn_row(
            world,
            body,
            "Culled",
            &format!("below {}", percent(last.screen_height)),
        );
    }
    let fade = match lod.fade {
        jackdaw_scene_types::LodFade::Snap => "Snap".to_string(),
        jackdaw_scene_types::LodFade::CrossFade { width } => {
            format!("Cross-fade over {}", percent(width))
        }
    };
    spawn_row(world, body, "Fade", &fade);
    spawn_row(world, body, "Size", &format!("{:.2}", lod.size));
    spawn_action(
        world,
        body,
        "Reimport LOD Levels",
        crate::model_lod::ModelLodImportOp::ID,
        &asset_path,
    );
    if unsaved {
        spawn_action(
            world,
            body,
            "Apply",
            crate::model_lod::ModelLodApplyOp::ID,
            &asset_path,
        );
    }
}

/// A share of the screen as a percentage, to one decimal where it needs one.
fn percent(share: f32) -> String {
    let value = share * 100.0;
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

/// The triangles a level draws, once its model has loaded.
fn triangles_of(
    world: &World,
    model: &str,
    show: &jackdaw_scene_types::model_import::LevelShow,
) -> Option<usize> {
    let key = jackdaw_scene_types::model_import::level_key(model, show);
    let flat = world
        .get_resource::<jackdaw_scene_types::model_parts::ModelParts>()?
        .get(&key)?
        .clone();
    let meshes = world.get_resource::<Assets<Mesh>>()?;
    Some(
        flat.parts
            .iter()
            .filter_map(|part| meshes.get(&part.mesh))
            .map(|mesh| match mesh.indices() {
                Some(indices) => indices.len() / 3,
                None => mesh.count_vertices() / 3,
            })
            .sum(),
    )
}

fn fill_image_body(
    world: &mut World,
    body: Entity,
    path: &Path,
    as_string: &str,
    shown_path: &str,
) {
    let (info, layer, layers) = {
        let preview = world.resource::<AssetPreviewState>();
        (
            preview.selected_info.clone(),
            preview.current_layer,
            preview.layer_images.clone(),
        )
    };
    let Some(info) = info.filter(|_| {
        world
            .resource::<AssetPreviewState>()
            .selected_path
            .as_deref()
            == Some(path)
    }) else {
        spawn_row(world, body, "Path", shown_path);
        return;
    };

    let shown = info.image_handle.clone().or_else(|| {
        layers
            .get((layer as usize).min(layers.len().saturating_sub(1)))
            .cloned()
    });
    if let Some(image) = shown {
        world.spawn((
            ImageNode::new(image),
            Node {
                width: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                height: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                align_self: AlignSelf::Center,
                ..default()
            },
            ChildOf(body),
        ));
    }

    spawn_row(world, body, "Path", shown_path);
    spawn_row(world, body, "Texture", &info.description());

    if info.is_array && !layers.is_empty() {
        let icon_font = world.resource::<IconFont>().0.clone();
        let row = world
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    align_self: AlignSelf::Center,
                    column_gap: Val::Px(tokens::SPACING_SM),
                    margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                    ..default()
                },
                ChildOf(body),
            ))
            .id();
        spawn_layer_step(world, row, &icon_font, Icon::ChevronLeft, -1);
        world.spawn((
            Text::new(format!("Layer {} of {}", layer + 1, info.layer_count)),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(row),
        ));
        spawn_layer_step(world, row, &icon_font, Icon::ChevronRight, 1);
    }

    if crate::texture_import::is_importable(path) {
        spawn_import_rows(world, body, path, as_string);
    }

    if info.is_plain_2d() {
        spawn_action(world, body, "Apply", "material.apply_texture", as_string);
    }
}

/// How the texture is imported, a button for each way it can be, and what an
/// import asks of the app that loads it.
fn spawn_import_rows(world: &mut World, body: Entity, path: &Path, as_string: &str) {
    let summary = if crate::texture_import::is_imported(path) {
        "Compressed, mipmaps"
    } else {
        "Original file"
    };
    spawn_row(world, body, "Import", summary);
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_self: AlignSelf::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    for (label, compression) in [("Compress", "vram_compressed"), ("Original", "none")] {
        let action = world
            .spawn((
                button(ButtonProps::new(label.to_string())),
                ButtonOperatorCall::new(crate::texture_import::TextureImportOp::ID)
                    .with_param("path", as_string.to_string())
                    .with_param("compression", compression.to_string()),
            ))
            .id();
        world.entity_mut(action).insert(ChildOf(row));
    }
    world.spawn((
        Text::new(
            "Compressing writes a Bevy .meta beside the texture. Only apps built on \
             jackdaw_runtime, or running Bevy's asset processor, can load it then. \
             Textures a terrain uses stay as they are.",
        ),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
            ..default()
        },
        ChildOf(body),
    ));
}

fn spawn_layer_step(
    world: &mut World,
    row: Entity,
    icon_font: &Handle<Font>,
    icon: Icon,
    direction: i64,
) {
    let button = world
        .spawn((
            jackdaw_feathers::button::icon_button(
                jackdaw_feathers::button::IconButtonProps::new(icon).variant(ButtonVariant::Ghost),
                icon_font,
            ),
            ButtonOperatorCall::new(crate::texture_files::AssetCycleArrayLayerOp::ID)
                .with_param("direction", direction),
        ))
        .id();
    world.entity_mut(button).insert(ChildOf(row));
}

fn spawn_row(world: &mut World, body: Entity, label: &str, value: &str) {
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    world.spawn((
        Text::new(label.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: Val::Px(70.0),
            ..default()
        },
        ChildOf(row),
    ));
    world.spawn((
        Text::new(value.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            overflow: Overflow::clip(),
            ..default()
        },
        ChildOf(row),
    ));
}

/// Marks a row naming a document that references the file a card stands for.
#[derive(Component)]
pub struct ReferenceRow(pub PathBuf);

/// How many referrers a card lists before it counts the rest.
const REFERENCES_SHOWN: usize = 20;

/// List the documents whose patches reference the file at `path`, each a click
/// away, under `parent`.
///
/// A file nothing points at gets no list, and a file too many point at gets
/// the first of them and a count of the rest. The list says what is on disk,
/// so a referring document with unsaved edits reads as it was last saved. The
/// entity the list sits on comes back, so a card can mark it as its own.
pub(crate) fn spawn_references(world: &mut World, parent: Entity, path: &Path) -> Option<Entity> {
    let indexed = crate::asset_index::indexed_path(world, path)?;
    let referrers = world
        .get_resource::<crate::asset_index::AssetIndex>()
        .map(|index| index.referrers(&indexed).to_vec())
        .unwrap_or_default();
    if referrers.is_empty() {
        return None;
    }
    let list = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    world.spawn((
        Text::new("References"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(list),
    ));
    for referrer in referrers.iter().take(REFERENCES_SHOWN) {
        spawn_reference_row(world, list, referrer);
    }
    let rest = referrers.len().saturating_sub(REFERENCES_SHOWN);
    if rest > 0 {
        world.spawn((
            Text::new(format!("and {rest} more")),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(list),
        ));
    }
    Some(list)
}

/// One referrer, as the button that takes the editor to it: a scene opens in a
/// tab, and anything else is selected into the inspector.
fn spawn_reference_row(world: &mut World, body: Entity, referrer: &Path) {
    let file = crate::asset_index::absolute_path(world, referrer);
    world.get_resource_or_init::<crate::asset_files::AssetKindCache>();
    let opens_a_tab = world.resource_scope(
        |world, mut cache: Mut<crate::asset_files::AssetKindCache>| {
            world
                .get_resource::<jackdaw_api::prelude::AssetKinds>()
                .is_some_and(|kinds| cache.check(&file, kinds) == AssetFileKind::Scene)
        },
    );
    let operator = match opens_a_tab {
        true => crate::project_window::ProjectOpenOp::ID,
        false => crate::project_window::ProjectSelectOp::ID,
    };
    let label = referrer.to_string_lossy().into_owned();
    let row = world
        .spawn((
            button(
                ButtonProps::new(label)
                    .with_variant(ButtonVariant::Ghost)
                    .align_left(),
            ),
            ReferenceRow(referrer.to_path_buf()),
            ButtonOperatorCall::new(operator)
                .with_param("path", file.to_string_lossy().into_owned()),
        ))
        .id();
    world.entity_mut(row).insert(ChildOf(body));
}

fn spawn_action(world: &mut World, body: Entity, label: &str, operator: &'static str, path: &str) {
    let slot = world
        .spawn((
            Node {
                align_self: AlignSelf::Center,
                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    let action = world
        .spawn((
            button(ButtonProps::new(label.to_string())),
            ButtonOperatorCall::new(operator).with_param("path", path.to_string()),
        ))
        .id();
    world.entity_mut(action).insert(ChildOf(slot));
}
