use std::path::PathBuf;

use crate::EditorEntity;
use crate::core_extension::CoreExtensionInputContext;
use bevy::{picking::hover::Hovered, prelude::*, ui_widgets::observe};
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    icons::Icon,
    text_edit::{self, TextEditProps, TextEditValue},
    tokens,
};

pub struct PrefabPickerPlugin;

impl Plugin for PrefabPickerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (filter_prefab_picker, close_prefab_picker_on_outside_click)
                .run_if(in_state(crate::AppState::Editor)),
        );
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PrefabPickerCloseOp>();
    let ext = ctx.id();
    ctx.spawn((
        Action::<PrefabPickerCloseOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Escape, Press::default())],
    ));
}

fn picker_open(picker: Query<(), With<PrefabPicker>>) -> bool {
    !picker.is_empty()
}

/// Close the prefab picker. Triggered by Escape via BEI; click-outside
/// dismissal stays in [`close_prefab_picker_on_outside_click`] because
/// it's a positional gesture, not a key event.
#[operator(
    id = "prefab_picker.close",
    label = "Close Prefab Picker",
    description = "Close the prefab picker.",
    is_available = picker_open,
    allows_undo = false,
)]
pub(crate) fn prefab_picker_close(
    _: In<OperatorParameters>,
    picker: Query<Entity, With<PrefabPicker>>,
    mut commands: Commands,
) -> OperatorResult {
    for entity in &picker {
        commands.entity(entity).despawn();
    }
    OperatorResult::Finished
}

#[derive(Component)]
struct PrefabPicker;

#[derive(Component)]
struct PrefabPickerSearch;

#[derive(Component)]
struct PrefabPickerEntry {
    display_name: String,
}

/// Open (or close, if already open) the prefab picker overlay.
pub fn open_prefab_picker(world: &mut World) {
    // Toggle: if picker already open, close it
    let existing: Vec<Entity> = world
        .query_filtered::<Entity, With<PrefabPicker>>()
        .iter(world)
        .collect();
    if !existing.is_empty() {
        for e in existing {
            if let Ok(ec) = world.get_entity_mut(e) {
                ec.despawn();
            }
        }
        return;
    }

    // Scan for .jsn files
    let assets_dir = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(super::project::ProjectRoot::assets_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));

    let mut prefabs: Vec<(String, String)> = Vec::new(); // (path, display_name)
    scan_jsn_files(&assets_dir, &assets_dir, &mut prefabs);
    prefabs.sort_by_key(|a| a.1.to_lowercase());

    // Find the viewport entity to parent the picker to
    let viewport_entity = world
        .query_filtered::<Entity, With<crate::viewport::SceneViewport>>()
        .iter(world)
        .next();

    let Some(parent_entity) = viewport_entity else {
        warn!("No viewport found for prefab picker");
        return;
    };

    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();

    // Spawn picker
    let mut commands = world.commands();
    let picker = commands
        .spawn((
            PrefabPicker,
            crate::BlocksCameraInput,
            EditorEntity,
            Hovered::default(),
            Node {
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                width: Val::Px(420.0),
                max_height: Val::Px(600.0),
                top: Val::Px(40.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-210.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            GlobalZIndex(100),
            ChildOf(parent_entity),
        ))
        .id();

    // Search input
    commands.spawn((
        PrefabPickerSearch,
        text_edit::text_edit(
            TextEditProps::default()
                .with_placeholder("Search prefabs...")
                .allow_empty(),
        ),
        ChildOf(picker),
    ));

    // Scrollable list
    let list = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                overflow: Overflow::scroll_y(),
                ..Default::default()
            },
            ChildOf(picker),
        ))
        .id();

    if prefabs.is_empty() {
        commands.spawn((
            Text::new("No .jsn files found"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
                ..Default::default()
            },
            ChildOf(list),
        ));
    }

    for (path, display_name) in &prefabs {
        let entry_path = path.clone();
        let entry_display = display_name.clone();
        let icon_font_clone = icon_font.clone();

        let entry_id = commands
            .spawn((
                PrefabPickerEntry {
                    display_name: entry_display.clone(),
                },
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(tokens::SPACING_LG), Val::Px(tokens::SPACING_SM)),
                    column_gap: Val::Px(tokens::SPACING_SM),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                BackgroundColor(Color::NONE),
                ChildOf(list),
                observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                    let path = entry_path.clone();
                    commands.queue(move |world: &mut World| {
                        // Close picker
                        let pickers: Vec<Entity> = world
                            .query_filtered::<Entity, With<PrefabPicker>>()
                            .iter(world)
                            .collect();
                        for e in pickers {
                            if let Ok(ec) = world.get_entity_mut(e) {
                                ec.despawn();
                            }
                        }
                        // Instantiate
                        crate::entity_templates::instantiate_jsn_prefab(world, &path, Vec3::ZERO);
                    });
                }),
                observe(
                    move |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                        if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                            bg.0 = tokens::HOVER_BG;
                        }
                    },
                ),
                observe(
                    move |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                        if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                            bg.0 = Color::NONE;
                        }
                    },
                ),
            ))
            .id();

        // Icon
        commands.spawn((
            Text::new(String::from(Icon::Blocks.unicode())),
            TextFont {
                font: icon_font_clone,
                font_size: tokens::FONT_MD,
                ..Default::default()
            },
            TextColor(tokens::FILE_ICON_COLOR),
            ChildOf(entry_id),
        ));

        // Display name
        commands.spawn((
            Text::new(entry_display),
            TextFont {
                font_size: tokens::FONT_MD,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(entry_id),
        ));
    }

    world.flush();
}

/// Recursively scan a directory for .jsn scene files.
fn scan_jsn_files(dir: &PathBuf, _assets_root: &PathBuf, results: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        warn!("Prefab picker: failed to read directory {:?}", dir);
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_jsn_files(&path, _assets_root, results);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jsn"))
        {
            // Skip project.jsn files, they aren't scenes.
            if path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("project.jsn"))
            {
                continue;
            }

            let path_str = path.to_string_lossy().to_string();

            // Try to read metadata name from the file without deserializing the
            // entire scene (which can be very large for complex scenes).
            let display_name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .and_then(|v| {
                    v.get("metadata")?
                        .get("name")?
                        .as_str()
                        .map(std::string::ToString::to_string)
                })
                .filter(|name| !name.is_empty() && name != "Untitled")
                .unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                });

            info!("Prefab picker: found {:?} -> {:?}", path_str, display_name);
            results.push((path_str, display_name));
        }
    }
}

/// Left-click outside the picker dispatches [`PrefabPickerCloseOp`]
/// (the same operator Escape fires via BEI). Right and middle clicks
/// are ignored so the fly-camera right-drag doesn't dismiss it. A
/// pure BEI binding can't express "clicked outside this entity".
fn close_prefab_picker_on_outside_click(
    mouse: Res<ButtonInput<MouseButton>>,
    picker: Query<&Hovered, With<PrefabPicker>>,
    mut commands: Commands,
) {
    let Ok(hovered) = picker.single() else {
        return;
    };
    if mouse.just_pressed(MouseButton::Left) && !hovered.get() {
        commands.operator(PrefabPickerCloseOp::ID).call();
    }
}

/// Filter the prefab picker list based on search input.
fn filter_prefab_picker(
    search_query: Query<&TextEditValue, (With<PrefabPickerSearch>, Changed<TextEditValue>)>,
    mut entries: Query<(Entity, &PrefabPickerEntry, &mut Node), Without<PrefabPickerSearch>>,
) {
    let Ok(search) = search_query.single() else {
        return;
    };
    let filter = search.0.trim().to_lowercase();

    for (_entity, entry, mut node) in &mut entries {
        let matches = filter.is_empty() || entry.display_name.to_lowercase().contains(&filter);
        node.display = if matches {
            Display::Flex
        } else {
            Display::None
        };
    }
}
