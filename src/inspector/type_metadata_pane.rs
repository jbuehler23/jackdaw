//! Type-settings sub-pane toggled from an inspector card header.

use bevy::feathers::controls::{ButtonVariant, FeathersToolButton};
use bevy::feathers::cursor::EntityCursor;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy::ui_widgets::{Activate, ToggleChecked};
use bevy::window::{PrimaryWindow, RawHandleWrapper, SystemCursorIcon};
use jackdaw_feathers::icons::{Icon, icon_scene};
use jackdaw_feathers::text_edit::{self, TextEditCommitEvent, TextEditProps};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::utils::find_ancestor;
use jackdaw_widgets::collapsible::CollapsibleSection;
use rfd::AsyncFileDialog;

use crate::preview_model::import_preview_model;
use crate::project::ProjectRoot;
use crate::type_metadata::{TypeChrome, TypeMetadata, set_category, set_description, set_preview};

use super::component_display::{ComponentDisplayCard, DisclosureSection};

#[derive(Component)]
struct TypeMetadataToggle;

#[derive(Component)]
struct TypeMetadataPane;

#[derive(Clone, Copy)]
enum TypeMetadataKind {
    Category,
    Description,
}

#[derive(Component)]
struct TypeMetadataInput {
    type_path: String,
    kind: TypeMetadataKind,
}

#[derive(Component)]
struct TypeMetadataPreview {
    type_path: String,
    preview: String,
    applied: bool,
}

#[derive(Component)]
struct TypeMetadataPreviewClear {
    type_path: String,
}

#[derive(Resource)]
struct PreviewPickTask {
    type_path: String,
    task: Task<Option<rfd::FileHandle>>,
}

pub(super) fn plugin(app: &mut App) {
    app.add_observer(on_type_metadata_commit).add_systems(
        Update,
        (poll_preview_pick, update_preview_thumbnails).run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn spawn_type_metadata_ui(
    commands: &mut Commands,
    card: &ComponentDisplayCard,
    type_path: &str,
    chrome: &TypeChrome,
    type_metadata: &TypeMetadata,
) {
    let display = Display::None;
    let pane = commands
        .spawn_scene(bsn! {
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                display: {display},
                flex_shrink: 0.0,
                padding: UiRect::all(px(tokens::SPACING_MD)),
                row_gap: px(tokens::SPACING_SM),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(tokens::COMPONENT_CARD_RADIUS)),
                margin: UiRect::bottom(px(tokens::SPACING_SM)),
            }
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG)
            BorderColor::all(tokens::COMPONENT_CARD_BORDER)
        })
        .insert((TypeMetadataPane, ChildOf(card.body)))
        .id();

    spawn_preview_row(
        commands,
        pane,
        type_path,
        &chrome.preview,
        overlay_has_preview(type_metadata, type_path),
    );

    commands.spawn((
        TypeMetadataInput {
            type_path: type_path.to_string(),
            kind: TypeMetadataKind::Category,
        },
        text_edit::text_edit(
            TextEditProps::default()
                .with_label("Category")
                .with_placeholder("Actor, Gameplay, ...")
                .with_default_value(chrome.category.clone())
                .allow_empty(),
        ),
        ChildOf(pane),
    ));

    commands.spawn((
        TypeMetadataInput {
            type_path: type_path.to_string(),
            kind: TypeMetadataKind::Description,
        },
        text_edit::text_edit(
            TextEditProps::default()
                .with_label("Description")
                .with_default_value(chrome.description.clone())
                .allow_empty()
                .multiline(),
        ),
        ChildOf(pane),
    ));

    commands
        .spawn_scene(type_metadata_toggle_button())
        .insert((ChildOf(card.header), TypeMetadataToggle))
        .observe(on_toggle_type_metadata);
}

fn spawn_preview_row(
    commands: &mut Commands,
    pane: Entity,
    type_path: &str,
    preview: &str,
    overlay: bool,
) {
    let column = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: Val::Px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(pane),
        ))
        .id();

    commands.spawn((
        Text::new("Preview"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(column),
    ));

    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Start,
                column_gap: Val::Px(tokens::SPACING_SM),
                ..default()
            },
            ChildOf(column),
        ))
        .id();

    let size = Val::Px(tokens::PREVIEW_IMAGE_SIZE);
    let preview_box = commands
        .spawn((
            TypeMetadataPreview {
                type_path: type_path.to_string(),
                preview: preview.to_string(),
                applied: preview.is_empty(),
            },
            Hovered::default(),
            Pickable::default(),
            EntityCursor::System(SystemCursorIcon::Pointer),
            Tooltip::title("Select preview model"),
            Node {
                width: size,
                height: size,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(tokens::INPUT_BG),
            BorderColor::all(tokens::COMPONENT_CARD_BORDER),
            ChildOf(row),
        ))
        .observe(on_preview_box_click)
        .id();

    spawn_preview_placeholder(commands, preview_box);

    let clear_display = if overlay {
        Display::Flex
    } else {
        Display::None
    };
    let clear = commands
        .spawn((
            TypeMetadataPreviewClear {
                type_path: type_path.to_string(),
            },
            Node {
                display: clear_display,
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(row),
        ))
        .id();
    commands
        .spawn_scene(clear_preview_button())
        .insert(ChildOf(clear))
        .observe(on_preview_clear_click);
}

fn spawn_preview_placeholder(commands: &mut Commands, parent: Entity) {
    let glyph = String::from(Icon::Plus.unicode());
    commands
        .spawn_scene(icon_scene(glyph, tokens::ICON_LG_PX))
        .insert((
            TextColor(tokens::TEXT_SECONDARY),
            Pickable::IGNORE,
            ChildOf(parent),
        ));
}

fn type_metadata_toggle_button() -> impl Scene {
    let glyph = String::from(Icon::Ellipsis.unicode());
    bsn! {
        @FeathersToolButton {
            @caption: bsn! { icon_scene(glyph, tokens::TEXT_SIZE_SM_PX) },
            @variant: {ButtonVariant::Plain}
        }
        Tooltip::title("Type settings")
    }
}

fn clear_preview_button() -> impl Scene {
    let glyph = String::from(Icon::X.unicode());
    bsn! {
        @FeathersToolButton {
            @caption: bsn! { icon_scene(glyph, tokens::TEXT_SIZE_PX) },
            @variant: {ButtonVariant::Plain}
        }
        Tooltip::title("Clear preview")
    }
}

fn on_toggle_type_metadata(
    activate: On<Activate>,
    mut commands: Commands,
    toggles: Query<&TypeMetadataToggle>,
    child_of: Query<&ChildOf>,
    children: Query<&Children>,
    sections: Query<&CollapsibleSection>,
    disclosures: Query<(), With<DisclosureSection>>,
    mut panes: Query<&mut Node, With<TypeMetadataPane>>,
) {
    if find_ancestor(activate.event_target(), &toggles, &child_of).is_none() {
        return;
    }
    let Some((section_entity, section)) =
        find_ancestor(activate.event_target(), &sections, &child_of)
    else {
        return;
    };
    let collapsed = section.collapsed;
    let Some(pane_entity) = children
        .iter_descendants(section_entity)
        .find(|&e| panes.contains(e))
    else {
        return;
    };
    let Ok(mut node) = panes.get_mut(pane_entity) else {
        return;
    };
    let pane_open = node.display != Display::None;

    if !collapsed && pane_open {
        node.display = Display::None;
        return;
    }
    if collapsed
        && let Some(disclosure) = children
            .iter_descendants(section_entity)
            .find(|&e| disclosures.contains(e))
    {
        commands.trigger(ToggleChecked { entity: disclosure });
    }
    node.display = Display::Flex;
}

fn on_type_metadata_commit(
    event: On<TextEditCommitEvent>,
    inputs: Query<&TypeMetadataInput>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some((_, input)) = find_ancestor(event.entity, &inputs, &child_of) else {
        return;
    };
    let type_path = input.type_path.clone();
    let kind = input.kind;
    let value = event.text.trim().to_string();
    commands.queue(move |world: &mut World| {
        let Some(root) = world.get_resource::<ProjectRoot>().map(|p| p.root.clone()) else {
            return;
        };
        let result = match kind {
            TypeMetadataKind::Category => set_category(world, &root, &type_path, &value),
            TypeMetadataKind::Description => set_description(world, &root, &type_path, &value),
        };
        if let Err(err) = result {
            let field = match kind {
                TypeMetadataKind::Category => "category",
                TypeMetadataKind::Description => "description",
            };
            warn!("failed to write type {field} to jackdaw_metadata.bsn: {err}");
        }
    });
}

fn on_preview_box_click(
    click: On<Pointer<Click>>,
    previews: Query<&TypeMetadataPreview>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some((_, preview)) = find_ancestor(click.event_target(), &previews, &child_of) else {
        return;
    };
    let type_path = preview.type_path.clone();
    commands.queue(move |world: &mut World| {
        open_preview_picker(world, type_path);
    });
}

fn on_preview_clear_click(
    activate: On<Activate>,
    clears: Query<&TypeMetadataPreviewClear>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some((_, clear)) = find_ancestor(activate.event_target(), &clears, &child_of) else {
        return;
    };
    let type_path = clear.type_path.clone();
    commands.queue(move |world: &mut World| {
        let Some(root) = world.get_resource::<ProjectRoot>().map(|p| p.root.clone()) else {
            return;
        };
        if let Err(err) = set_preview(world, &root, &type_path, "") {
            warn!("failed to clear type preview in jackdaw_metadata.bsn: {err}");
            return;
        }
        refresh_preview_slots(world, &type_path);
    });
}

fn open_preview_picker(world: &mut World, type_path: String) {
    if world.contains_resource::<PreviewPickTask>() {
        return;
    }
    let assets_dir = world.get_resource::<ProjectRoot>().map(|p| p.assets_dir());
    let raw_handle = world
        .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .cloned();
    let mut dialog = AsyncFileDialog::new()
        .set_title("Select preview model")
        .add_filter("glTF", &["glb", "gltf"]);
    if let Some(dir) = &assets_dir {
        dialog = dialog.set_directory(dir);
    }
    if let Some(ref rh) = raw_handle {
        // SAFETY: called on the main thread from an exclusive context
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(PreviewPickTask { type_path, task });
}

fn poll_preview_pick(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<PreviewPickTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.task)) else {
        return;
    };
    let type_path = task_res.type_path.clone();
    world.remove_resource::<PreviewPickTask>();
    let Some(file_handle) = result else {
        return;
    };
    let picked = file_handle.path().to_path_buf();
    let Some(project) = world.get_resource::<ProjectRoot>() else {
        return;
    };
    let root = project.root.clone();
    let assets_dir = project.assets_dir();
    match import_preview_model(&assets_dir, &picked) {
        Ok(relative) => {
            if let Err(err) = set_preview(world, &root, &type_path, &relative) {
                warn!("failed to write type preview to jackdaw_metadata.bsn: {err}");
                return;
            }
            refresh_preview_slots(world, &type_path);
        }
        Err(err) => {
            warn!("failed to import preview model: {err}");
        }
    }
}

fn refresh_preview_slots(world: &mut World, type_path: &str) {
    let overlay = overlay_has_preview(world.resource::<TypeMetadata>(), type_path);
    let preview = resolved_preview(world, type_path);

    let mut boxes = Vec::new();
    {
        let mut query = world.query::<(Entity, &mut TypeMetadataPreview)>();
        for (entity, mut slot) in query.iter_mut(world) {
            if slot.type_path != type_path {
                continue;
            }
            slot.preview = preview.clone();
            slot.applied = preview.is_empty();
            boxes.push(entity);
        }
    }
    for entity in boxes {
        refill_preview_box(world, entity);
    }

    let mut query = world.query::<(&TypeMetadataPreviewClear, &mut Node)>();
    for (clear, mut node) in query.iter_mut(world) {
        if clear.type_path == type_path {
            node.display = if overlay {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

fn resolved_preview(world: &World, type_path: &str) -> String {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let project_types = world.resource::<crate::project_types::ProjectTypes>();
    world
        .resource::<TypeMetadata>()
        .resolve(type_path, &registry, project_types)
        .preview
}

fn refill_preview_box(world: &mut World, box_entity: Entity) {
    let kids: Vec<Entity> = world
        .get::<Children>(box_entity)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for kid in kids {
        world.entity_mut(kid).despawn();
    }
    spawn_preview_placeholder(&mut world.commands(), box_entity);
    world.flush();
}

fn update_preview_thumbnails(
    mut commands: Commands,
    mut thumbnails: ResMut<crate::model_thumbnail::ModelThumbnails>,
    project: Option<Res<ProjectRoot>>,
    mut slots: Query<(Entity, &mut TypeMetadataPreview, Option<&Children>)>,
) {
    let Some(assets_dir) = project.map(|p| p.assets_dir()) else {
        return;
    };
    for (entity, mut slot, children) in &mut slots {
        if slot.applied || slot.preview.is_empty() {
            continue;
        }
        let source = assets_dir.join(&slot.preview);
        let Some(handle) = thumbnails.ready(&source) else {
            if thumbnails.is_failed(&source) {
                slot.applied = true;
                continue;
            }
            thumbnails.request(&source);
            continue;
        };
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).try_despawn();
            }
        }
        commands.spawn((
            ImageNode::new(handle),
            Node {
                width: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                height: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(entity),
        ));
        slot.applied = true;
    }
}

fn overlay_has_preview(type_metadata: &TypeMetadata, type_path: &str) -> bool {
    type_metadata
        .get(type_path)
        .and_then(|m| m.preview.as_deref())
        .is_some_and(|s| !s.is_empty())
}
