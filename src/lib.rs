pub mod alignment_guides;
pub mod asset_browser;
pub mod brush;
pub mod commands;
pub mod custom_properties;
pub mod draw_brush;
pub mod entity_ops;
pub mod entity_templates;
pub mod face_grid;
pub mod gizmos;
pub mod hierarchy;
pub mod inspector;
pub use inspector::{EditorMeta, ReflectEditorMeta};
pub mod layout;
pub mod material_browser;
pub mod material_definition;
pub mod material_preview;
pub mod modal_transform;
pub mod navmesh;
pub mod project;
pub mod project_select;
pub mod scene_io;
pub mod selection;
pub mod snapping;
pub mod status_bar;
pub mod terrain;
pub mod texture_browser;
pub mod view_modes;
pub mod viewport;
pub mod viewport_overlays;
pub mod viewport_select;
pub mod viewport_util;

use bevy::{
    ecs::system::SystemState,
    feathers::{FeathersPlugins, dark_theme::create_dark_theme, theme::UiTheme},
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::InputDispatchPlugin,
    picking::hover::HoverMap,
    prelude::*,
};
use jackdaw_feathers::EditorFeathersPlugin;
use jackdaw_widgets::menu_bar::MenuAction;
use selection::Selection;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    ProjectSelect,
    Editor,
}

#[derive(Component, Default)]
pub struct EditorEntity;

/// Tag component that hides an entity from the hierarchy panel.
/// Auto-applied to unnamed child entities (likely Bevy internals like shadow cascades).
/// Users can remove it to make hidden entities visible, or add it to hide their own.
#[derive(Component, Default)]
pub struct EditorHidden;

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        // Disable InputDispatchPlugin from FeathersPlugins because bevy_ui_text_input's
        // TextInputPlugin also adds it unconditionally and panics on duplicates.
        app.init_state::<AppState>()
            .add_plugins((
                FeathersPlugins.build().disable::<InputDispatchPlugin>(),
                EditorFeathersPlugin,
                jackdaw_jsn::JsnPlugin,
                project_select::ProjectSelectPlugin,
                inspector::InspectorPlugin,
                hierarchy::HierarchyPlugin,
                viewport::ViewportPlugin,
                gizmos::TransformGizmosPlugin,
                commands::CommandHistoryPlugin,
                selection::SelectionPlugin,
                entity_ops::EntityOpsPlugin,
                scene_io::SceneIoPlugin,
                asset_browser::AssetBrowserPlugin,
                viewport_select::ViewportSelectPlugin,
                snapping::SnappingPlugin,
            ))
            .add_plugins((
                viewport_overlays::ViewportOverlaysPlugin,
                view_modes::ViewModesPlugin,
                status_bar::StatusBarPlugin,
                modal_transform::ModalTransformPlugin,
                custom_properties::CustomPropertiesPlugin,
                entity_templates::EntityTemplatesPlugin,
                brush::BrushPlugin,
                material_browser::MaterialBrowserPlugin,
                draw_brush::DrawBrushPlugin,
                face_grid::FaceGridPlugin,
                alignment_guides::AlignmentGuidesPlugin,
                navmesh::NavmeshPlugin,
                terrain::TerrainPlugin,
            ))
            .insert_resource(UiTheme(create_dark_theme()))
            .init_resource::<layout::KeybindHelpPopover>()
            .add_systems(
                OnEnter(AppState::Editor),
                (spawn_layout, populate_menu).chain(),
            )
            .add_systems(
                Update,
                (
                    send_scroll_events,
                    layout::update_toolbar_highlights,
                    layout::update_space_toggle_label,
                    layout::update_edit_tool_highlights,
                    auto_hide_internal_entities,
                )
                    .run_if(in_state(AppState::Editor)),
            )
            .add_observer(on_scroll)
            .add_observer(handle_menu_action);
    }
}

/// Auto-hide unnamed child entities (likely Bevy internals like shadow cascades).
/// Skips GLTF descendants so they appear in the hierarchy panel.
fn auto_hide_internal_entities(
    mut commands: Commands,
    new_entities: Query<
        (Entity, Option<&Name>, Option<&ChildOf>),
        (
            Added<Transform>,
            Without<EditorEntity>,
            Without<EditorHidden>,
            Without<brush::BrushFaceEntity>,
        ),
    >,
    parent_query: Query<&ChildOf>,
    gltf_sources: Query<(), With<entity_ops::GltfSource>>,
) {
    for (entity, name, parent) in &new_entities {
        if name.is_none() && parent.is_some() {
            // Skip GLTF descendants — they'll be shown in the hierarchy
            let mut current = entity;
            let mut is_gltf_descendant = false;
            while let Ok(&ChildOf(p)) = parent_query.get(current) {
                if gltf_sources.contains(p) {
                    is_gltf_descendant = true;
                    break;
                }
                current = p;
            }
            if is_gltf_descendant {
                continue;
            }

            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(EditorHidden);
            }
        }
    }
}

fn spawn_layout(mut commands: Commands, icon_font: Res<jackdaw_feathers::icons::IconFont>) {
    commands.spawn((Camera2d, EditorEntity));
    commands.spawn(layout::editor_layout(&icon_font));
}

fn populate_menu(world: &mut World) {
    let menu_bar_entity = world
        .query_filtered::<Entity, With<jackdaw_feathers::menu_bar::MenuBarRoot>>()
        .iter(world)
        .next();
    let Some(menu_bar_entity) = menu_bar_entity else {
        return;
    };
    jackdaw_feathers::menu_bar::populate_menu_bar(
        world,
        menu_bar_entity,
        vec![
            (
                "File",
                vec![
                    ("file.new", "New"),
                    ("file.open", "Open"),
                    ("---", ""),
                    ("file.save", "Save"),
                    ("file.save_as", "Save As..."),
                    ("---", ""),
                    ("file.save_template", "Save Selection as Template"),
                ],
            ),
            (
                "Edit",
                vec![
                    ("edit.undo", "Undo"),
                    ("edit.redo", "Redo"),
                    ("---", ""),
                    ("edit.delete", "Delete"),
                    ("edit.duplicate", "Duplicate"),
                ],
            ),
            (
                "View",
                vec![
                    ("view.wireframe", "Toggle Wireframe"),
                    ("view.bounding_boxes", "Toggle Bounding Boxes"),
                    ("view.bounding_box_mode", "Cycle Bounding Box Mode"),
                    ("view.face_grid", "Toggle Face Grid"),
                    ("view.brush_wireframe", "Toggle Brush Wireframe"),
                    ("view.alignment_guides", "Toggle Alignment Guides"),
                ],
            ),
            (
                "Add",
                vec![
                    ("add.cube", "Cube"),
                    ("add.sphere", "Sphere"),
                    ("---", ""),
                    ("add.point_light", "Point Light"),
                    ("add.directional_light", "Directional Light"),
                    ("add.spot_light", "Spot Light"),
                    ("---", ""),
                    ("add.camera", "Camera"),
                    ("add.empty", "Empty"),
                    ("---", ""),
                    ("add.navmesh", "Navmesh Region"),
                    ("add.terrain", "Terrain"),
                ],
            ),
        ],
    );
}

fn handle_menu_action(event: On<MenuAction>, mut commands: Commands) {
    match event.action.as_str() {
        "file.new" => {
            commands.queue(|world: &mut World| {
                scene_io::new_scene(world);
            });
        }
        "file.save" => {
            commands.queue(|world: &mut World| {
                scene_io::save_scene(world);
            });
        }
        "file.save_as" => {
            commands.queue(|world: &mut World| {
                scene_io::save_scene_as(world);
            });
        }
        "file.open" => {
            commands.queue(|world: &mut World| {
                scene_io::load_scene(world);
            });
        }
        "file.save_template" => {
            // Use a default name based on the selected entity
            commands.queue(|world: &mut World| {
                let selection = world.resource::<Selection>();
                let name = selection
                    .primary()
                    .and_then(|e| world.get::<Name>(e).map(|n| n.as_str().to_string()))
                    .unwrap_or_else(|| "template".to_string());
                entity_templates::save_entity_template(world, &name);
            });
        }
        "edit.undo" => {
            commands.queue(|world: &mut World| {
                world.resource_scope(|world, mut history: Mut<commands::CommandHistory>| {
                    history.undo(world);
                });
            });
        }
        "edit.redo" => {
            commands.queue(|world: &mut World| {
                world.resource_scope(|world, mut history: Mut<commands::CommandHistory>| {
                    history.redo(world);
                });
            });
        }
        "edit.delete" => {
            commands.queue(|world: &mut World| {
                entity_ops::delete_selected(world);
            });
        }
        "edit.duplicate" => {
            commands.queue(|world: &mut World| {
                entity_ops::duplicate_selected(world);
            });
        }
        "view.wireframe" => {
            commands.queue(|world: &mut World| {
                let mut settings = world.resource_mut::<view_modes::ViewModeSettings>();
                settings.wireframe = !settings.wireframe;
            });
        }
        "view.bounding_boxes" => {
            commands.queue(|world: &mut World| {
                let mut settings = world.resource_mut::<viewport_overlays::OverlaySettings>();
                settings.show_bounding_boxes = !settings.show_bounding_boxes;
            });
        }
        "view.bounding_box_mode" => {
            commands.queue(|world: &mut World| {
                let mut settings = world.resource_mut::<viewport_overlays::OverlaySettings>();
                settings.bounding_box_mode = match settings.bounding_box_mode {
                    viewport_overlays::BoundingBoxMode::Aabb => {
                        viewport_overlays::BoundingBoxMode::ConvexHull
                    }
                    viewport_overlays::BoundingBoxMode::ConvexHull => {
                        viewport_overlays::BoundingBoxMode::Aabb
                    }
                };
            });
        }
        "view.face_grid" => {
            commands.queue(|world: &mut World| {
                let mut settings = world.resource_mut::<viewport_overlays::OverlaySettings>();
                settings.show_face_grid = !settings.show_face_grid;
            });
        }
        "view.brush_wireframe" => {
            commands.queue(|world: &mut World| {
                let mut settings = world.resource_mut::<viewport_overlays::OverlaySettings>();
                settings.show_brush_wireframe = !settings.show_brush_wireframe;
            });
        }
        "view.alignment_guides" => {
            commands.queue(|world: &mut World| {
                let mut settings = world.resource_mut::<viewport_overlays::OverlaySettings>();
                settings.show_alignment_guides = !settings.show_alignment_guides;
            });
        }
        "add.cube" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Cube);
            });
        }
        "add.sphere" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Sphere);
            });
        }
        "add.point_light" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::PointLight);
            });
        }
        "add.directional_light" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(
                    world,
                    entity_ops::EntityTemplate::DirectionalLight,
                );
            });
        }
        "add.spot_light" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::SpotLight);
            });
        }
        "add.camera" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Camera3d);
            });
        }
        "add.empty" => {
            commands.queue(|world: &mut World| {
                entity_ops::create_entity_in_world(world, entity_ops::EntityTemplate::Empty);
            });
        }
        "add.navmesh" => {
            commands.queue(|world: &mut World| {
                let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                    SystemState::new(world);
                let (mut commands, mut selection) = system_state.get_mut(world);
                let entity = navmesh::spawn_navmesh_entity(&mut commands);
                selection.select_single(&mut commands, entity);
                system_state.apply(world);
            });
        }
        "add.terrain" => {
            commands.queue(|world: &mut World| {
                let mut system_state: SystemState<(Commands, ResMut<Selection>)> =
                    SystemState::new(world);
                let (mut commands, mut selection) = system_state.get_mut(world);
                let entity = terrain::spawn_terrain_entity(&mut commands);
                selection.select_single(&mut commands, entity);
                system_state.apply(world);
            });
        }
        _ => {}
    }
}

const SCROLL_LINE_HEIGHT: f32 = 21.0;

#[derive(EntityEvent, Debug)]
#[entity_event(propagate, auto_propagate)]
struct Scroll {
    entity: Entity,
    delta: Vec2,
}

fn send_scroll_events(
    mut mouse_wheel: MessageReader<MouseWheel>,
    hover_map: Res<HoverMap>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
) {
    for event in mouse_wheel.read() {
        let mut delta = -Vec2::new(event.x, event.y);
        if event.unit == MouseScrollUnit::Line {
            delta *= SCROLL_LINE_HEIGHT;
        }
        if keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
            std::mem::swap(&mut delta.x, &mut delta.y);
        }
        for pointer_map in hover_map.values() {
            for entity in pointer_map.keys().copied() {
                commands.trigger(Scroll { entity, delta });
            }
        }
    }
}

fn on_scroll(
    mut scroll: On<Scroll>,
    mut query: Query<(&mut ScrollPosition, &Node, &ComputedNode)>,
) {
    let Ok((mut scroll_position, node, computed)) = query.get_mut(scroll.entity) else {
        return;
    };
    let max_offset = (computed.content_size() - computed.size()) * computed.inverse_scale_factor();
    let delta = &mut scroll.delta;

    if node.overflow.x == OverflowAxis::Scroll && delta.x != 0. {
        let at_limit = if delta.x > 0. {
            scroll_position.x >= max_offset.x
        } else {
            scroll_position.x <= 0.
        };
        if !at_limit {
            scroll_position.x += delta.x;
            delta.x = 0.;
        }
    }

    if node.overflow.y == OverflowAxis::Scroll && delta.y != 0. {
        let at_limit = if delta.y > 0. {
            scroll_position.y >= max_offset.y
        } else {
            scroll_position.y <= 0.
        };
        if !at_limit {
            scroll_position.y += delta.y;
            delta.y = 0.;
        }
    }

    if *delta == Vec2::ZERO {
        scroll.propagate(false);
    }
}
