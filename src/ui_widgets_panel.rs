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
        let parent = if definition_id == "layout.canvas" {
            None
        } else {
            selected_ui_parent(world).or_else(|| first_canvas(world))
        };
        let parent = match parent {
            Some(parent) => Some(parent),
            None if definition_id != "layout.canvas" => UiAuthoring::instantiate(
                world,
                "layout.canvas",
                WidgetInstantiateContext::default(),
            )
            .ok(),
            None => None,
        };
        if let Err(error) =
            UiAuthoring::instantiate(world, &definition_id, WidgetInstantiateContext { parent })
        {
            warn!("could not create `{definition_id}` from UI palette: {error}");
        }
    });
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

fn first_canvas(world: &mut World) -> Option<Entity> {
    let mut query = world.query_filtered::<Entity, (With<UiCanvas>, Without<ProjectedFrom>)>();
    query.iter(world).min_by_key(|entity| entity.to_bits())
}
