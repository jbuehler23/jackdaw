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
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_project_build::modules::ModuleCatalog;

use super::ComponentPicker;
use crate::project_types::ProjectTypes;
use crate::type_metadata::TypeMetadata;

/// Marker on the "Add Component" button in the add-header of every category
/// without an add action of its own. `on_add_component_button_click` guards on
/// this to avoid reacting to unrelated button clicks.
#[derive(Component)]
pub struct InspectorAddComponentButton;

/// Type-path filter consulted by [`enumerate_pickable_components`] to
/// hide reflected components that should never appear in the picker
/// (e.g. solver internals, derived caches). Populated by the editor
/// plugin and extensions; downstream code can extend it via
/// [`PickerDenylist::deny_path`] / [`PickerDenylist::deny_prefix`].
#[derive(Resource, Default)]
pub struct PickerDenylist {
    paths: HashSet<&'static str>,
    prefixes: Vec<&'static str>,
}

impl PickerDenylist {
    /// Hide a single fully-qualified type path.
    pub fn deny_path(&mut self, path: &'static str) -> &mut Self {
        self.paths.insert(path);
        self
    }

    /// Hide every type whose full path starts with `prefix`.
    pub fn deny_prefix(&mut self, prefix: &'static str) -> &mut Self {
        self.prefixes.push(prefix);
        self
    }

    /// True when `type_path` is filtered.
    pub fn contains(&self, type_path: &str) -> bool {
        self.paths.contains(type_path) || self.prefixes.iter().any(|p| type_path.starts_with(p))
    }
}

/// Hides the components each module's catalog entry lists, such as a
/// physics engine's solver state and derived caches. The components people
/// author stay in.
pub fn populate_module_picker_denylist(denylist: &mut PickerDenylist) {
    for module in ModuleCatalog::builtin().modules() {
        for hidden in &module.hidden_components {
            if hidden.ends_with("::") {
                denylist.deny_prefix(hidden);
            } else {
                denylist.deny_path(hidden);
            }
        }
    }
}

/// Empty group string is the Game section.
fn picker_section(type_path: &str, authored_category: &str, group: &str) -> (String, i32) {
    let name = if group.is_empty() {
        String::from("Game")
    } else {
        group.to_string()
    };
    (
        name,
        crate::type_metadata::group_order(type_path, authored_category),
    )
}

struct ComponentInfo {
    short_name: String,
    module_path: String,
    group: String,
    authored_category: String,
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
    pub authored_category: String,
    pub description: String,
    pub type_path_full: String,
}

/// Enumerate every component the picker would display for a
/// target entity. Filters: must be a `Component`, must be
/// default-constructible (via [`build_reflective_default`]), not
/// already on `existing_types`, not editor-hidden, and not denylisted.
/// Grouping, description, and hidden come from [`TypeMetadata`].
///
/// [`build_reflective_default`]: crate::reflect_default::build_reflective_default
pub fn enumerate_pickable_components(
    registry: &bevy::reflect::TypeRegistry,
    existing_types: &HashSet<TypeId>,
    denylist: &PickerDenylist,
    type_metadata: &TypeMetadata,
    project_types: &ProjectTypes,
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

        let table = registration.type_info().type_path_table();
        let full_path = table.path();

        if denylist.contains(full_path) {
            continue;
        }

        let chrome = type_metadata.resolve(full_path, registry, project_types);
        if chrome.hidden {
            continue;
        }

        out.push(PickableComponent {
            short_name: table.short_path().to_string(),
            module_path: table.module_path().unwrap_or("").to_string(),
            category: chrome.group(full_path),
            authored_category: chrome.category.clone(),
            description: chrome.description,
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
        let (name, order) =
            picker_section(&self.type_path_full, &self.authored_category, &self.group);
        Category {
            name: Some(name),
            order,
        }
    }
}

/// Handle click on an "Add Component" button in the per-category
/// add-header. Opens the unscoped component picker listing all
/// addable components for the selected entity.
pub(crate) fn on_add_component_button_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    add_buttons: Query<(), With<InspectorAddComponentButton>>,
    existing_pickers: Query<Entity, With<ComponentPicker>>,
    mut commands: Commands,
    selection: Res<Selection>,
    type_registry: Res<AppTypeRegistry>,
    components: &Components,
    entity_query: Query<&Archetype, (With<Selected>, Without<EditorEntity>)>,
    denylist: Res<PickerDenylist>,
    project_types: Res<crate::project_types::ProjectTypes>,
    type_metadata: Res<crate::type_metadata::TypeMetadata>,
    doc: Res<jackdaw_bsn::SceneBsnAst>,
) {
    if add_buttons.get(event.entity).is_err() {
        return;
    }

    if let Some(picker) = existing_pickers.iter().next() {
        commands.entity(picker).despawn();
        return;
    }

    // The primary selection is the entity to add the component to in both
    // Scene and Live mode. In Live mode it is the selected preview entity
    // (which carries the projected live state), and the add operator
    // routes the change to the running game via `pie_live_target_bits`.
    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok(archetype) = entity_query.get(primary) else {
        return;
    };
    let (target, archetype) = (primary, archetype);

    let existing_types: HashSet<TypeId> = archetype
        .iter_components()
        .filter_map(|cid| {
            components
                .get_info(cid)
                .and_then(bevy::ecs::component::ComponentInfo::type_id)
        })
        .collect();

    let registry = type_registry.read();

    let mut searchable_components: Vec<ComponentInfo> = enumerate_pickable_components(
        &registry,
        &existing_types,
        &denylist,
        &type_metadata,
        &project_types,
    )
    .into_iter()
    .map(|p| ComponentInfo {
        short_name: p.short_name,
        module_path: p.module_path,
        group: p.category,
        authored_category: p.authored_category,
        description: p.description,
        type_path_full: p.type_path_full,
    })
    .collect();

    // Project (schema-reported) components are not real ECS components in the
    // editor, so they never appear in the registry pass above. They live as
    // schema entries; add each one the target does not already carry in the
    // scene document.
    let authored: HashSet<String> = doc
        .ast_for(target)
        .map(|node| doc.component_type_paths(node).into_iter().collect())
        .unwrap_or_default();
    for schema in project_types.components() {
        let chrome = type_metadata.resolve(&schema.type_path, &registry, &project_types);
        if chrome.hidden
            || jackdaw_bsn::type_paths_include(
                authored.iter().map(String::as_str),
                &schema.type_path,
            )
        {
            continue;
        }
        searchable_components.push(ComponentInfo {
            short_name: schema.short_name.clone(),
            module_path: schema.module_path.clone(),
            group: chrome.group(&schema.type_path),
            authored_category: chrome.category.clone(),
            description: chrome.description,
            type_path_full: schema.type_path.clone(),
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
        ComponentPicker(target),
    ));
}

fn on_select(
    input: In<SelectInput>,
    items: Query<(&ComponentPicker, &PickerItems<ComponentInfo>)>,
    categories: Res<jackdaw_api_internal::inspector::InspectorRegistry>,
    mut active: ResMut<super::category_strip::ActiveInspectorCategory>,
    mut commands: Commands,
) -> Result {
    let (picker, items) = items.get(input.entities.picker)?;
    let info = items.at(input.index)?;

    // Show the category the new card lands in, so adding from another
    // category's view does not leave the user looking at no change.
    let category = categories.category_for(&info.type_path_full);
    if active.0 != category {
        active.0 = std::borrow::Cow::Owned(category.to_string());
    }

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

    let (category, _) = picker_section(&info.type_path_full, &info.authored_category, &info.group);
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
