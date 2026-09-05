//! Data-driven widget palette for UI authoring.

use bevy::{
    ecs::hierarchy::Children,
    picking::events::{Click, Pointer},
    prelude::*,
    ui_widgets::Button,
};
use jackdaw_api::{WidgetInstantiateContext, WidgetRegistry};
use jackdaw_scene_types::SceneNodeId;
use jackdaw_ui::{UiCanvas, UiGeneratedPart};

use crate::{
    EditorEntity, selection::Selection, ui_authoring::UiAuthoring, ui_projection::ProjectedFrom,
};

pub const UI_WIDGETS_WINDOW_ID: &str = "jackdaw.ui_widgets";

#[derive(Component, Default)]
pub struct UiWidgetsPanel;

#[derive(Component, Default)]
struct PaletteRevision(Vec<(String, String, String)>);

#[derive(Component)]
struct PaletteItem {
    definition_id: String,
}

pub struct UiWidgetsPanelPlugin;

impl Plugin for UiWidgetsPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_palette_item_click)
            .add_systems(Update, refresh_widget_palettes);
    }
}

/// Build a palette host. Its rows are reconciled from [`WidgetRegistry`] so
/// extension enable/disable appears without reopening the dock window.
pub fn build_ui_widgets_panel(world: &mut World, host: Entity) {
    world.spawn((
        UiWidgetsPanel,
        PaletteRevision::default(),
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            padding: UiRect::all(px(8)),
            row_gap: px(4),
            ..default()
        },
        ScrollPosition::default(),
        BackgroundColor(Color::srgb(0.075, 0.08, 0.095)),
        ChildOf(host),
    ));
}

fn refresh_widget_palettes(world: &mut World) {
    let mut definitions = world
        .get_resource::<WidgetRegistry>()
        .map(|registry| {
            registry
                .iter()
                .map(|definition| {
                    (
                        definition.id.to_string(),
                        definition.name.to_string(),
                        definition.category.to_string(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    definitions
        .sort_by(|left, right| (&left.2, &left.1, &left.0).cmp(&(&right.2, &right.1, &right.0)));

    let mut query = world.query::<(Entity, &PaletteRevision)>();
    let dirty = query
        .iter(world)
        .filter(|(_, revision)| revision.0 != definitions)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();

    for panel in dirty {
        let children = world
            .get::<Children>(panel)
            .map(|children| children.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children {
            if let Ok(child) = world.get_entity_mut(child) {
                child.despawn();
            }
        }
        spawn_palette_rows(world, panel, &definitions);
        world
            .entity_mut(panel)
            .insert(PaletteRevision(definitions.clone()));
    }
}

fn spawn_palette_rows(world: &mut World, panel: Entity, definitions: &[(String, String, String)]) {
    if definitions.is_empty() {
        world.spawn((
            EditorEntity,
            Text::new("No UI widgets registered"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb(0.55, 0.57, 0.62)),
            ChildOf(panel),
        ));
        return;
    }

    let mut category = "";
    for (id, name, item_category) in definitions {
        if category != item_category {
            category = item_category;
            world.spawn((
                EditorEntity,
                Text::new(item_category.clone()),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgb(0.57, 0.64, 0.78)),
                Node {
                    margin: UiRect::top(px(7)),
                    ..default()
                },
                ChildOf(panel),
            ));
        }

        let row = world
            .spawn((
                PaletteItem {
                    definition_id: id.clone(),
                },
                EditorEntity,
                Button,
                Node {
                    width: percent(100),
                    min_height: px(28),
                    padding: UiRect::horizontal(px(8)),
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(Color::srgb(0.13, 0.14, 0.17)),
                ChildOf(panel),
            ))
            .id();
        world.spawn((
            EditorEntity,
            Text::new(name.clone()),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb(0.87, 0.88, 0.91)),
            Pickable::IGNORE,
            ChildOf(row),
        ));
    }
}

fn on_palette_item_click(
    event: On<Pointer<Click>>,
    items: Query<&PaletteItem>,
    mut commands: Commands,
) {
    let Ok(item) = items.get(event.event_target()) else {
        return;
    };
    let definition_id = item.definition_id.clone();
    commands.queue(move |world: &mut World| {
        if let Err(error) = instantiate_widget(world, &definition_id) {
            warn!("could not create `{definition_id}` from UI palette: {error}");
        }
    });
}

/// Create a widget using the same parent/canvas fallback from every editor
/// entry point (Canvas palette, menu bar, and Add Entity picker).
///
/// Creating a widget that needs a canvas creates both in one history entry: one
/// palette click is one undo.
pub fn instantiate_widget(
    world: &mut World,
    definition_id: &str,
) -> Result<Entity, crate::ui_authoring::UiAuthoringError> {
    if definition_id == "layout.canvas" {
        return UiAuthoring::instantiate(world, definition_id, WidgetInstantiateContext::default());
    }

    let existing = selected_ui_parent(world)
        .or_else(|| edited_canvas(world))
        .or_else(|| first_canvas(world));
    if let Some(parent) = existing {
        return UiAuthoring::instantiate(
            world,
            definition_id,
            WidgetInstantiateContext {
                parent: Some(parent),
            },
        );
    }

    let (canvas, create_canvas) = UiAuthoring::execute_instantiate(
        world,
        "layout.canvas",
        WidgetInstantiateContext::default(),
    )?;
    let (widget, create_widget) = UiAuthoring::execute_instantiate(world, definition_id, {
        WidgetInstantiateContext {
            parent: Some(canvas),
        }
    })?;
    let label = create_widget.description().to_string();
    world
        .resource_mut::<jackdaw_commands::CommandHistory>()
        .push_executed(Box::new(jackdaw_commands::CommandGroup {
            commands: vec![create_canvas, create_widget],
            label,
        }));
    Ok(widget)
}

fn selected_ui_parent(world: &World) -> Option<Entity> {
    let selected = world.get_resource::<Selection>()?.primary()?;
    if world.get::<ProjectedFrom>(selected).is_some()
        || world.get::<UiGeneratedPart>(selected).is_some()
        || world.get::<SceneNodeId>(selected).is_none()
    {
        return None;
    }
    let mut current = Some(selected);
    while let Some(entity) = current {
        if world.get::<UiCanvas>(entity).is_some() {
            return Some(selected);
        }
        current = world.get::<ChildOf>(entity).map(ChildOf::parent);
    }
    None
}

/// The canvas an open UI Canvas panel is editing. New widgets belong to the
/// canvas the user is looking at, not to whichever one happens to sort first.
fn edited_canvas(world: &mut World) -> Option<Entity> {
    let mut query = world.query::<&crate::ui_canvas::UiCanvasPanelHost>();
    query
        .iter(world)
        .find_map(|host| host.canvas)
        .filter(|canvas| world.get_entity(*canvas).is_ok())
}

fn first_canvas(world: &mut World) -> Option<Entity> {
    crate::ui_canvas::authored_canvases(world).first().copied()
}
