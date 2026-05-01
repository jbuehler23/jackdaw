//! Unified Add Entity picker, shared by the toolbar Add menu and the
//! scene-tree Add Entity button. Sources items from built-in templates
//! plus extension-contributed `RegisteredMenuEntry` rows under
//! `menu == "Add"`.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::picker::{
    Category, Matchable, PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text,
    picker_item,
};
use jackdaw_feathers::tokens;

use crate::entity_ops::{
    EntityAddCameraOp, EntityAddCubeOp, EntityAddDirectionalLightOp, EntityAddEmptyOp,
    EntityAddNavmeshOp, EntityAddPointLightOp, EntityAddPrefabOp, EntityAddSphereOp,
    EntityAddSpotLightOp, EntityAddTerrainOp,
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

#[derive(Component)]
pub struct AddEntityPickerEntry {
    pub label: String,
    pub category: String,
}

#[derive(Component)]
pub struct AddEntityPickerSectionHeader {
    pub category: String,
}

/// Build an `op:` action string for the given operator type. Keeps
/// operator ids out of UI code; callers pass the `Op` type, not a
/// hand-typed string.
fn op_action<O: Operator>() -> String {
    format!("op:{}", O::ID)
}

/// Built-in Add items grouped by category. Order here is the order in
/// the picker and in the toolbar Add menu.
fn builtin_groups() -> Vec<AddMenuItem> {
    let shapes = Category {
        name: Some(String::from("Shapes")),
        order: 0,
    };
    let lights = Category {
        name: Some(String::from("Lights")),
        order: -1,
    };
    let cameras_entities = Category {
        name: Some(String::from("Cameras & Entities")),
        order: -2,
    };
    let regions = Category {
        name: Some(String::from("Regions")),
        order: -3,
    };
    let prefabs = Category {
        name: Some(String::from("Prefabs")),
        order: -4,
    };

    vec![
        AddMenuItem {
            action: op_action::<EntityAddCubeOp>(),
            label: "Cube".into(),
            category: shapes.clone(),
        },
        AddMenuItem {
            action: op_action::<EntityAddSphereOp>(),
            label: "Sphere".into(),
            category: shapes,
        },
        AddMenuItem {
            action: op_action::<EntityAddPointLightOp>(),
            label: "Point Light".into(),
            category: lights.clone(),
        },
        AddMenuItem {
            action: op_action::<EntityAddDirectionalLightOp>(),
            label: "Directional Light".into(),
            category: lights.clone(),
        },
        AddMenuItem {
            action: op_action::<EntityAddSpotLightOp>(),
            label: "Spot Light".into(),
            category: lights,
        },
        AddMenuItem {
            action: op_action::<EntityAddCameraOp>(),
            label: "Camera".into(),
            category: cameras_entities.clone(),
        },
        AddMenuItem {
            action: op_action::<EntityAddEmptyOp>(),
            label: "Empty".into(),
            category: cameras_entities,
        },
        AddMenuItem {
            action: op_action::<EntityAddNavmeshOp>(),
            label: "Navmesh Region".into(),
            category: regions.clone(),
        },
        AddMenuItem {
            action: op_action::<EntityAddTerrainOp>(),
            label: "Terrain".into(),
            category: regions,
        },
        AddMenuItem {
            action: op_action::<EntityAddPrefabOp>(),
            label: "Prefab...".into(),
            category: prefabs,
        },
    ]
}

/// One row in the Add menu or Add Entity picker. `action` is handled
/// by `handle_menu_action` (e.g. `"add.cube"` or
/// `"op:viewable_camera.place"`).
#[derive(Clone)]
pub struct AddMenuItem {
    pub action: String,
    pub label: String,
    pub category: Category,
}

/// Shared source of truth for Add menu contents, consumed by both the
/// toolbar Add menu and the scene-tree Add Entity picker.
pub fn collect_add_menu_items(world: &mut World) -> Vec<AddMenuItem> {
    let mut items: Vec<AddMenuItem> = builtin_groups();

    // Extension items grouped by owning extension so entries cluster by
    // author in the picker.
    let mut q = world.query::<(
        &jackdaw_api_internal::lifecycle::RegisteredMenuEntry,
        Option<&ChildOf>,
    )>();
    let mut ext_entries: Vec<(Entity, String, String)> = Vec::new();
    for (entry, parent) in q.iter(world) {
        if entry.menu != TopLevelMenu::Add {
            continue;
        }
        let ext_entity = parent.map(ChildOf::parent).unwrap_or(Entity::PLACEHOLDER);
        ext_entries.push((
            ext_entity,
            format!("op:{}", entry.operator_id),
            entry.label.clone(),
        ));
    }
    for (ext_entity, action, label) in ext_entries {
        let category = world
            .get::<jackdaw_api_internal::lifecycle::Extension>(ext_entity)
            .map(|e| e.id.clone())
            .unwrap_or_else(|| "Extensions".to_string());
        items.push(AddMenuItem {
            action,
            label,
            category: Category {
                name: Some(category),
                order: -5,
            },
        });
    }

    items
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

    let entry_id = commands
        .spawn((picker_item(matched.index), ChildOf(entities.list)))
        .id();

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

    if let Some(name) = item.category.clone().name {
        commands.spawn((
            Text::new(name),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(row),
        ));
    }

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
