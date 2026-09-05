//! Unified Add Entity picker, shared by the toolbar Add menu and the
//! scene-tree Add Entity button. Both read the single creation vocabulary in
//! [`crate::creation_taxonomy`], so a menu row and a picker entry cannot drift
//! apart.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::picker::{
    Category, Matchable, PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text,
    picker_item,
};
use jackdaw_feathers::tooltip::Tooltip;

use crate::creation_taxonomy::CreationTaxonomy;
pub use crate::creation_taxonomy::{
    EXTENSIONS_SECTION, GENERAL_SECTION, QUALIFIED_LABEL_SEPARATOR, WIDGET_ACTION_PREFIX,
};

/// Marker for the scene-tree Add Entity button.
#[derive(Component)]
pub struct AddEntityButton;

/// Backdrop and panel root for the picker. Despawning it tears down
/// the whole dialog.
#[derive(Component)]
pub struct AddEntityPicker;

#[derive(Component)]
pub struct AddEntityPickerSearch;

/// Prefixes a widget category in a picker heading with the UI group's label.
/// The Add menu shows the same categories as sections inside its `UI` row,
/// where the prefix is omitted.
pub const UI_SECTION_PREFIX: &str = "UI: ";

/// One row in the Add menu or Add Entity picker, in the shape the picker's
/// fuzzy matcher takes: a label to match on, a category to group under, and the
/// action `handle_menu_action` dispatches.
#[derive(Clone)]
pub struct AddMenuItem {
    pub action: String,
    pub label: String,
    pub category: Category,
}

/// Every creatable thing, in the taxonomy's grouping. Read by both the toolbar
/// Add menu and the scene-tree Add Entity picker.
pub fn collect_add_menu_items(world: &mut World) -> Vec<AddMenuItem> {
    let taxonomy = CreationTaxonomy::collect(world);
    let mut items = Vec::new();
    for group in taxonomy.groups() {
        let name = taxonomy
            .qualified_label(&group.id)
            .unwrap_or_else(|| group.label.clone());
        for entry in taxonomy.entries_of(&group.id) {
            items.push(AddMenuItem {
                action: entry.action.clone(),
                label: entry.label.clone(),
                category: Category {
                    name: Some(name.clone()),
                    order: group.order,
                },
            });
        }
    }
    items
}

/// The Add menu's dropdown rows: the general vocabulary in the menu itself,
/// every other group behind a row that expands on hover.
pub fn add_menu_rows(world: &mut World) -> Vec<(String, String)> {
    CreationTaxonomy::collect(world).menu_rows()
}

/// Open the searchable Add Entity picker, the way the scene tree's Add Entity
/// button does. Shares [`collect_add_menu_items`] with the Add menu, so the two
/// offer the same vocabulary. Calling it again closes it.
#[operator(
    id = "entity.add_picker",
    label = "Add Entity",
    description = "Open the searchable Add Entity picker.",
    allows_undo = false,
    is_available = crate::entity_ops::can_act_on_entities
)]
pub(crate) fn entity_add_picker(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Err(err) = world.run_system_cached(open_add_entity_picker) {
            warn!("entity.add_picker: {err}");
        }
    });
    OperatorResult::Finished
}

/// Open the Add Entity picker as a centered blocking dialog. Styled
/// to match the Add Component dialog. Toggles off if already open.
pub fn open_add_entity_picker(
    world: &mut World,
    entity_pickers: &mut QueryState<Entity, With<AddEntityPicker>>,
) {
    let existing: Vec<Entity> = entity_pickers.iter(world).collect();
    if !existing.is_empty() {
        for e in existing {
            if let Ok(ec) = world.get_entity_mut(e) {
                ec.despawn();
            }
        }
        return;
    }

    let items = collect_add_menu_items(world);

    let picker = PickerProps::new(spawn_item, on_select)
        .items(items)
        .title("Add Entity")
        .placeholder(Some("Search Entities.."));

    let mut commands = world.commands();

    commands.spawn((
        AddEntityPicker,
        crate::EditorEntity,
        crate::BlocksCameraInput,
        picker,
    ));
}

fn spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    items: Query<&PickerItems<AddMenuItem>>,
    mut commands: Commands,
) -> Result {
    let item = items.get(entities.picker)?.at(matched.index)?;

    let mut tooltip = Tooltip::title(matched.haystack);
    if let Some(category) = &item.category.name {
        tooltip = tooltip.with_footer(category);
    }

    commands.spawn((
        picker_item(matched.index),
        ChildOf(entities.list),
        tooltip,
        children![match_text(matched.segments)],
    ));

    Ok(())
}

fn on_select(
    input: In<SelectInput>,
    items: Query<&PickerItems<AddMenuItem>>,
    mut commands: Commands,
) -> Result {
    let item = items.get(input.entities.picker)?.at(input.index)?;

    commands.trigger(jackdaw_widgets::menu_bar::MenuAction {
        action: item.action.clone(),
    });
    commands.entity(input.entities.picker).try_despawn();

    Ok(())
}

impl Matchable for AddMenuItem {
    fn haystack(&self) -> String {
        self.label.to_string()
    }

    fn category(&self) -> Category {
        self.category.clone()
    }
}
