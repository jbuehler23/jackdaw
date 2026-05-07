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

use bevy::reflect::{TypeInfo, attributes::CustomAttributes};
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_runtime::{EditorCategory, EditorDescription, EditorHidden};

use super::{AddComponentButton, ComponentPicker, Inspector};

// `custom_attributes()` lives on the variant types
// (`StructInfo`, `EnumInfo`, etc.), not on `TypeInfo` itself.
fn type_info_custom_attributes(info: &TypeInfo) -> Option<&CustomAttributes> {
    match info {
        TypeInfo::Struct(s) => Some(s.custom_attributes()),
        TypeInfo::TupleStruct(s) => Some(s.custom_attributes()),
        TypeInfo::Enum(e) => Some(e.custom_attributes()),
        TypeInfo::Tuple(_)
        | TypeInfo::List(_)
        | TypeInfo::Array(_)
        | TypeInfo::Map(_)
        | TypeInfo::Set(_)
        | TypeInfo::Opaque(_) => None,
    }
}

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

/// Public view of one row the component picker would render.
/// Matches the UI's filter rules so tests can assert what users
/// will actually see.
pub struct PickableComponent {
    pub short_name: String,
    pub module_path: String,
    pub category: String,
    pub description: String,
    pub type_path_full: String,
}

/// Enumerate every component the picker would display for a
/// target entity. Filters: must be a `Component`, must be
/// default-constructible (via [`build_reflective_default`]), not
/// already on `existing_types`, and not editor-internal. Reads
/// `EditorCategory` / `EditorDescription` from custom reflect
/// attributes; falls back to the reflected doc comment for
/// description.
///
/// [`build_reflective_default`]: crate::reflect_default::build_reflective_default
pub fn enumerate_pickable_components(
    registry: &bevy::reflect::TypeRegistry,
    existing_types: &HashSet<TypeId>,
) -> Vec<PickableComponent> {
    let mut out = Vec::new();
    for registration in registry.iter() {
        let type_id = registration.type_id();

        if registration.data::<ReflectComponent>().is_none() {
            continue;
        }
        if crate::reflect_default::build_reflective_default(type_id, registry).is_none() {
            continue;
        }
        if existing_types.contains(&type_id) {
            continue;
        }

        let info = registration.type_info();
        let custom_attrs = type_info_custom_attributes(info);

        // Single mechanism for picker hiding: types opt out via the
        // `@EditorHidden` reflect attribute (defined alongside
        // `EditorCategory` / `EditorDescription` in `jackdaw_jsn`).
        // Used by jackdaw's own scene types and available to
        // extension/game authors for their own helper Components.
        if custom_attrs.is_some_and(|a| a.get::<EditorHidden>().is_some()) {
            continue;
        }

        let table = registration.type_info().type_path_table();
        let full_path = table.path();

        let description = custom_attrs
            .and_then(|a| a.get::<EditorDescription>())
            .map(|d| d.0.to_string())
            .or_else(|| {
                info.docs()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
        let category = custom_attrs
            .and_then(|a| a.get::<EditorCategory>())
            .map(|c| c.0.to_string())
            .unwrap_or_default();

        out.push(PickableComponent {
            short_name: table.short_path().to_string(),
            module_path: table.module_path().unwrap_or("").to_string(),
            category,
            description,
            type_path_full: full_path.to_string(),
        });
    }
    out
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
    if add_buttons.get(event.entity).is_err() {
        return;
    }

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

    let existing_types: HashSet<TypeId> = archetype
        .iter_components()
        .filter_map(|cid| {
            components
                .get_info(cid)
                .and_then(bevy::ecs::component::ComponentInfo::type_id)
        })
        .collect();

    let registry = type_registry.read();
    let searchable_components: Vec<ComponentInfo> =
        enumerate_pickable_components(&registry, &existing_types)
            .into_iter()
            .map(|p| {
                let group = if !p.category.is_empty() {
                    GroupOrder::Custom(p.category)
                } else if p.module_path.starts_with("bevy") {
                    GroupOrder::Bevy
                } else {
                    GroupOrder::Game
                };
                ComponentInfo {
                    short_name: p.short_name,
                    module_path: p.module_path,
                    group,
                    description: p.description,
                    type_path_full: p.type_path_full,
                }
            })
            .collect();

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

    let entry_id = commands
        .spawn((
            picker_item(matched.index),
            ChildOf(entities.list),
            Tooltip::title(matched.haystack)
                .with_description(description.clone())
                .with_footer(format!("{} - {}", module_path, category)),
            children![match_text(matched.segments)],
        ))
        .id();

    // Line 2: subtitle (module path)
    if !module_path.is_empty() {
        commands.spawn((
            Text::new(module_path),
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
