use crate::EditorEntity;
use crate::selection::{Selected, Selection};
use std::any::TypeId;
use std::collections::HashSet;

use bevy::ecs::archetype::Archetype;
use bevy::ecs::component::Components;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::picker::{
    Category, Matchable, PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text,
    picker_item,
};
use jackdaw_feathers::tokens;

use super::{AddComponentButton, ComponentPicker, Inspector, ReflectEditorMeta};

/// Grouping key for sorting: custom categories first, then Game, then Bevy.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
enum GroupOrder {
    Custom(String),
    Game,
    Bevy,
}

impl GroupOrder {
    fn name(self) -> String {
        match self {
            GroupOrder::Custom(name) => name,
            GroupOrder::Game => String::from("Game"),
            GroupOrder::Bevy => String::from("Bevy"),
        }
    }

    fn order(&self) -> i32 {
        match *self {
            GroupOrder::Custom(_) => 2,
            GroupOrder::Game => 1,
            GroupOrder::Bevy => 0,
        }
    }
}

struct ComponentInfo {
    short_name: String,
    module_path: String,
    group: GroupOrder,
    description: String,
    type_path_full: String,
}

impl Matchable for ComponentInfo {
    fn haystack(&self) -> String {
        self.short_name.clone()
    }

    fn category(&self) -> Category {
        Category {
            name: Some(self.group.clone().name()),
            order: self.group.order(),
        }
    }
}

/// Handle click on the "+" button to open the component picker.
pub(crate) fn on_add_component_button_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    add_buttons: Query<&ChildOf, With<AddComponentButton>>,
    existing_pickers: Query<Entity, With<ComponentPicker>>,
    mut commands: Commands,
    selection: Res<Selection>,
    type_registry: Res<AppTypeRegistry>,
    components: &Components,
    entity_query: Query<&Archetype, (With<Selected>, Without<EditorEntity>)>,
    _inspector: Single<Entity, With<Inspector>>,
) {
    // Check if this click is on an AddComponentButton
    if add_buttons.get(event.entity).is_err() {
        return;
    }

    // Toggle: if picker already open, close it
    if let Some(picker) = existing_pickers.iter().next() {
        commands.entity(picker).despawn();
        return;
    }

    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok(archetype) = entity_query.get(primary) else {
        return;
    };

    // Collect existing component TypeIds on the entity
    let existing_types: HashSet<TypeId> = archetype
        .iter_components()
        .filter_map(|cid| {
            components
                .get_info(cid)
                .and_then(bevy::ecs::component::ComponentInfo::type_id)
        })
        .collect();

    let registry = type_registry.read();

    // Collect all registered components that have ReflectComponent + ReflectDefault
    let mut searchable_components: Vec<ComponentInfo> = vec![];
    for registration in registry.iter() {
        let type_id = registration.type_id();

        // Must have ReflectComponent and ReflectDefault
        if registration.data::<ReflectComponent>().is_none()
            || registration.data::<ReflectDefault>().is_none()
        {
            continue;
        }

        // Skip components already on the entity
        if existing_types.contains(&type_id) {
            continue;
        }

        // Skip editor-internal types
        let table = registration.type_info().type_path_table();
        let full_path = table.path();
        if full_path.starts_with("jackdaw") && !full_path.starts_with("jackdaw_avian_integration") {
            continue;
        }

        // Skip if no component ID is registered for this type
        if components.get_id(type_id).is_none() {
            continue;
        }

        let short_name = table.short_path().to_string();
        let module = table.module_path().unwrap_or("").to_string();

        // Read EditorMeta if present
        let (category, description) = if let Some(meta) = registration.data::<ReflectEditorMeta>() {
            (meta.category.to_string(), meta.description.to_string())
        } else {
            (String::new(), String::new())
        };

        // Determine group
        let group = if !category.is_empty() {
            GroupOrder::Custom(category.clone())
        } else if module.starts_with("bevy") {
            GroupOrder::Bevy
        } else {
            GroupOrder::Game
        };

        searchable_components.push(ComponentInfo {
            short_name,
            module_path: module,
            group,
            description,
            type_path_full: full_path.to_string(),
        });
    }

    let picker = PickerProps::new(spawn_item, on_select)
        .items(searchable_components)
        .title("Add Component")
        .placeholder(Some("Search Components.."));

    commands.spawn((
        picker,
        EditorEntity,
        crate::BlocksCameraInput,
        ComponentPicker(primary),
    ));
}

fn on_select(
    input: In<SelectInput>,
    items: Query<(&ComponentPicker, &PickerItems<ComponentInfo>)>,
    mut commands: Commands,
) -> Result {
    let (picker, items) = items.get(input.entities.picker)?;
    let info = items.at(input.index)?;

    commands
        .operator(crate::inspector::ops::ComponentAddOp::ID)
        .param("entity", picker.0)
        .param("type_path", info.type_path_full.clone())
        .call();

    commands.entity(input.entities.picker).try_despawn();

    Ok(())
}

fn spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    items: Query<&PickerItems<ComponentInfo>>,
    mut commands: Commands,
) -> Result {
    let info = items.get(entities.picker)?.at(matched.index)?;

    let category = info.group.clone().name();
    let description = info.description.clone();
    let module_path = info.module_path.clone();

    // Subtitle: description takes priority, otherwise module path
    let subtitle = if !description.is_empty() {
        description.clone()
    } else {
        module_path.clone()
    };

    let entry_id = commands
        .spawn((picker_item(matched.index), ChildOf(entities.list)))
        .id();

    // Line 1: short name + optional category badge
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(entry_id),
        ))
        .id();

    commands.spawn((match_text(matched.segments), ChildOf(row)));

    if !category.is_empty() {
        commands.spawn((
            Text::new(category),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(row),
        ));
    }

    // Line 2: subtitle (description or module path)
    if !subtitle.is_empty() {
        commands.spawn((
            Text::new(subtitle),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(entry_id),
        ));
    }

    Ok(())
}
