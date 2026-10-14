use bevy::prelude::*;
use jackdaw_api_internal::inspector::InspectorChip;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button},
    combobox::{ComboBoxChangeEvent, ComboBoxOptionData, combobox_with_selected},
    tokens,
};

use crate::material_browser::{ApplyMaterialDefToFaces, MaterialRegistry};
use crate::selection::Selection;
use jackdaw_api::prelude::*;

use super::category_strip::ActiveInspectorCategory;
use super::component_picker::InspectorAddComponentButton;

/// Marker placed on the add-header host entity in the content column.
/// `on_add_header_mount_added` installs layout; [`rebuild_add_header`]
/// fills children when the category, selection, card set, material
/// registry, or mount changes.
#[derive(Component)]
pub(crate) struct InspectorAddHeaderMount;

/// Populate the add-header layout when the mount node appears. Children
/// come from [`rebuild_add_header`], which also catches a mount that
/// appears after selection is already set.
pub(crate) fn on_add_header_mount_added(
    trigger: On<Add<InspectorAddHeaderMount>>,
    mut commands: Commands,
) {
    let host = trigger.event_target();
    commands.entity(host).insert(Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        width: Val::Percent(100.0),
        padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
        flex_shrink: 0.0,
        ..Default::default()
    });
}

/// Replace the add-header children when the active category, selection,
/// card set, material registry or registered chips change, or when a mount
/// is added so a late-spawned inspector catches up to the current selection.
pub(crate) fn rebuild_add_header(
    active: Res<ActiveInspectorCategory>,
    selection: Res<Selection>,
    material_registry: Option<Res<MaterialRegistry>>,
    mounts: Query<(Entity, Option<&Children>), With<InspectorAddHeaderMount>>,
    added_mounts: Query<(), Added<InspectorAddHeaderMount>>,
    added_cards: Query<(), Added<super::ComponentDisplay>>,
    mut removed_cards: RemovedComponents<super::ComponentDisplay>,
    added_chips: Query<(), Added<InspectorChip>>,
    mut removed_chips: RemovedComponents<InspectorChip>,
    mut commands: Commands,
) {
    let cards_changed = !added_cards.is_empty() || removed_cards.read().next().is_some();
    let chips_changed = !added_chips.is_empty() || removed_chips.read().next().is_some();
    let registry_changed = material_registry.is_some_and(|r| r.is_changed());
    let should_rebuild = active.is_changed()
        || selection.is_changed()
        || cards_changed
        || chips_changed
        || registry_changed
        || !added_mounts.is_empty();
    if !should_rebuild {
        return;
    }

    for (host, children) in &mounts {
        if let Some(children) = children {
            let old: Vec<Entity> = children.iter().collect();
            commands.queue(move |world: &mut World| {
                for child in old {
                    if let Ok(ec) = world.get_entity_mut(child) {
                        ec.despawn();
                    }
                }
            });
        }
        build_add_header_children(&mut commands, host, active.0.as_ref());
    }
}

/// Spawn the appropriate add-UI children under `host` for `category`.
fn build_add_header_children(commands: &mut Commands, host: Entity, category: &str) {
    match category {
        "modifiers" => {
            commands.spawn((
                button(ButtonProps::new("Add Modifier").align_left()),
                ButtonOperatorCall::new("modifier.add").with_param("kind", "mirror"),
                ChildOf(host),
            ));
        }
        "material" => {
            // Defer so we can read `MaterialRegistry` from the world.
            let host_captured = host;
            commands.queue(move |world: &mut World| {
                spawn_material_header(world, host_captured);
            });
        }
        _ => {
            let category = category.to_string();
            commands.queue(move |world: &mut World| {
                spawn_category_header(world, host, &category);
            });
        }
    }
}

/// A category's header: the toggles extensions registered for it, or the
/// Add Component button when it has none.
fn spawn_category_header(world: &mut World, host: Entity, category: &str) {
    if world.get_entity(host).is_err() {
        return;
    }
    let mut chips_query = world.query::<(Entity, &InspectorChip)>();
    let mut chips: Vec<(Entity, InspectorChip)> = chips_query
        .iter(world)
        .filter(|(_, chip)| chip.category == category)
        .map(|(entity, chip)| (entity, chip.clone()))
        .collect();
    if chips.is_empty() {
        world.spawn((
            button(ButtonProps::new("Add Component").align_left()),
            InspectorAddComponentButton,
            ChildOf(host),
        ));
        return;
    }
    chips.sort_by_key(|(entity, _)| *entity);
    let primary = world.resource::<Selection>().primary();
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(host),
        ))
        .id();
    for (_, chip) in chips {
        let present =
            primary.is_some_and(|entity| has_component_at_path(world, entity, &chip.type_path));
        spawn_component_chip(world, row, chip, present);
    }
}

/// Whether `entity` has the component reflected at `type_path`: authored in
/// the scene document for an entity the document tracks, so a companion a
/// plugin derived does not read as one the person added, and live otherwise.
fn has_component_at_path(world: &World, entity: Entity, type_path: &str) -> bool {
    if world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .is_some_and(|ast| ast.ast_for(entity).is_some())
    {
        return jackdaw_api_internal::document::SceneDocument::authored_components(world, entity)
            .iter()
            .any(|authored| authored == type_path);
    }
    let type_id = {
        let registry = world.resource::<AppTypeRegistry>().read();
        let Some(registration) = registry.get_with_type_path(type_path) else {
            return false;
        };
        registration.type_id()
    };
    world.components().get_id(type_id).is_some_and(|component| {
        world
            .get_entity(entity)
            .is_ok_and(|e| e.contains_id(component))
    })
}

/// Spawn a single add/remove toggle inside `row`.
///
/// When `present` is true the button renders with `ButtonVariant::Active` and
/// a trailing X label; clicking dispatches `component.remove`. When `present`
/// is false it renders as a plain default button and clicking dispatches
/// `component.add`.
fn spawn_component_chip(world: &mut World, row: Entity, chip: InspectorChip, present: bool) {
    // The selected entity is only known at click time (the user can reselect
    // while the header is shown), so the chip carries a marker and
    // `on_component_chip_click` reads the current `Selection` and dispatches the
    // add/remove operator with the resolved entity. A `ButtonOperatorCall` is
    // deliberately NOT attached: it would auto-dispatch a second time without
    // the required `entity` param.
    let variant = if present {
        ButtonVariant::Active
    } else {
        ButtonVariant::Default
    };
    let display_label = if present {
        format!("{}  x", chip.label)
    } else {
        chip.label.to_string()
    };
    world.spawn((
        button(
            ButtonProps::new(display_label)
                .with_variant(variant)
                .with_size(ButtonSize::MD),
        ),
        ComponentChipButton {
            type_path: chip.type_path.to_string(),
            present,
        },
        ChildOf(row),
    ));
}

/// Marker on a component chip button. Carries the `type_path` and presence
/// flag so the click observer can forward the right operator without holding
/// a stale entity capture.
#[derive(Component)]
pub(crate) struct ComponentChipButton {
    pub(crate) type_path: String,
    pub(crate) present: bool,
}

/// Observer wired by `InspectorPlugin`: intercepts `ButtonClickEvent` on any
/// `ComponentChipButton` and dispatches `component.add` or `component.remove`
/// against the current primary selection.
pub(crate) fn on_component_chip_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    chips: Query<&ComponentChipButton>,
    selection: Res<Selection>,
    mut commands: Commands,
) {
    let Ok(chip) = chips.get(event.entity) else {
        return;
    };
    let Some(entity) = selection.primary() else {
        return;
    };
    let type_path = chip.type_path.clone();
    let operator = if chip.present {
        "component.remove"
    } else {
        "component.add"
    };
    commands
        .operator(operator)
        .param("entity", entity)
        .param("type_path", type_path)
        .call();
}

// -- Material header ---------------------------------------------------------

/// Spawn the material-tab add-header inside `host`. Reads `MaterialRegistry`
/// from the world so it runs as a deferred world-exclusive command.
fn spawn_material_header(world: &mut World, host: Entity) {
    // A panel relayout/teardown may have despawned the host between queueing
    // this closure and its flush; bail rather than parenting children under a
    // dead entity (the "ChildOf relates to an entity that does not exist" class).
    if world.get_entity(host).is_err() {
        return;
    }

    // Collect the material names and handles up front.
    let entries: Vec<(String, Handle<StandardMaterial>)> = world
        .get_resource::<MaterialRegistry>()
        .map(|reg| {
            reg.entries
                .iter()
                .map(|e| (e.name.clone(), e.handle.clone()))
                .collect()
        })
        .unwrap_or_default();

    // Resolve the material currently assigned to the primary selection so the
    // combobox opens on it instead of defaulting to the "None" entry. The
    // registry seeds a `Handle::default()` "None" entry at index 0, so an
    // unassigned brush lands there.
    let primary = world
        .get_resource::<Selection>()
        .and_then(Selection::primary);
    let current = primary.and_then(|entity| {
        super::material_card_routing::resolve_brush_material_handle(world, entity)
    });

    let selected = material_combobox_selected_index(&entries, current.as_ref());

    let mut commands = world.commands();

    // ---- Row container -------------------------------------------------
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(host),
        ))
        .id();

    // ---- "Assign" combobox -------------------------------------------------
    // One option per registry entry. Selecting an option applies that
    // material to the current selection via `ApplyMaterialDefToFaces`.
    // When the registry is empty the combobox is still shown (with a
    // placeholder) so the layout is stable.
    let options: Vec<ComboBoxOptionData> = if entries.is_empty() {
        vec![ComboBoxOptionData::new("(none)")]
    } else {
        entries
            .iter()
            .map(|(name, _)| ComboBoxOptionData::new(name.clone()))
            .collect()
    };

    let handles_for_observe = entries.clone();
    commands
        .spawn((combobox_with_selected(options, selected), ChildOf(row)))
        .observe(
            move |event: On<ComboBoxChangeEvent>, mut commands: Commands| {
                let idx = event.selected;
                if let Some((_, handle)) = handles_for_observe.get(idx) {
                    commands.trigger(ApplyMaterialDefToFaces {
                        material: crate::worn_material::WornMaterial::Standard(handle.clone()),
                    });
                }
            },
        );

    // ---- "New Material" button ------------------------------------------
    // Creates a blank material via the existing operator, then assigns it
    // to the current selection.
    commands.spawn((
        button(ButtonProps::new("New").with_size(ButtonSize::MD)),
        MaterialNewButton,
        ChildOf(row),
    ));
}

/// Pick the combobox option index that reflects the selection's assigned
/// material. `entries` are the registry rows (the registry seeds a
/// `Handle::default()` "None" row at index 0). `current` is the handle the
/// brush actually uses, or `None` when unassigned; an unassigned brush (or a
/// material missing from the registry) lands on the "None" row, falling back
/// to index 0 when the registry has no such row.
fn material_combobox_selected_index(
    entries: &[(String, Handle<StandardMaterial>)],
    current: Option<&Handle<StandardMaterial>>,
) -> usize {
    match current {
        Some(handle) => entries.iter().position(|(_, h)| h == handle),
        None => entries.iter().position(|(_, h)| *h == Handle::default()),
    }
    .unwrap_or(0)
}

/// Marker on the "New Material" button in the material add-header.
/// The click observer creates a fresh material and immediately applies it to
/// the selection so the new entry is visible in the Material tab.
#[derive(Component)]
pub(crate) struct MaterialNewButton;

/// Observer wired by `InspectorPlugin`: clicking `MaterialNewButton` runs the
/// `material.create` operator and then triggers `ApplyMaterialDefToFaces` with
/// the newly-minted handle so the selected brushes receive the material.
pub(crate) fn on_material_new_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    buttons: Query<(), With<MaterialNewButton>>,
    mut commands: Commands,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    // First create the material (operator sets preview_state.active_material).
    commands.operator("material.create").call();
    // Then apply it to the selection via a deferred world-exclusive step so
    // the new handle is in the registry before we read it.
    commands.queue(|world: &mut World| {
        let handle = world
            .get_resource::<crate::material_preview::MaterialPreviewState>()
            .and_then(|s| s.active_material.clone());
        if let Some(handle) = handle {
            world.trigger(ApplyMaterialDefToFaces {
                material: crate::worn_material::WornMaterial::Standard(handle),
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mint distinct `StandardMaterial` handles the same way the editor does
    // (against real asset storage); `Handle` has no public synthetic-id ctor.
    fn asset_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<StandardMaterial>();
        app
    }

    fn mint(app: &mut App) -> Handle<StandardMaterial> {
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default())
    }

    fn rows(handles: &[Handle<StandardMaterial>]) -> Vec<(String, Handle<StandardMaterial>)> {
        handles
            .iter()
            .enumerate()
            .map(|(i, h)| (format!("m{i}"), h.clone()))
            .collect()
    }

    #[test]
    fn assigned_material_selects_its_registry_row() {
        let mut app = asset_app();
        let mat = mint(&mut app);
        let entries = rows(&[Handle::default(), mat.clone()]);
        assert_eq!(material_combobox_selected_index(&entries, Some(&mat)), 1);
    }

    #[test]
    fn unassigned_selects_the_none_row() {
        let mut app = asset_app();
        let mat = mint(&mut app);
        // "None" (default handle) sits at index 1 here; unassigned must find it.
        let entries = rows(&[mat, Handle::default()]);
        assert_eq!(material_combobox_selected_index(&entries, None), 1);
    }

    #[test]
    fn material_absent_from_registry_falls_back_to_zero() {
        let mut app = asset_app();
        let in_registry = mint(&mut app);
        let missing = mint(&mut app);
        let entries = rows(&[in_registry]);
        assert_eq!(
            material_combobox_selected_index(&entries, Some(&missing)),
            0
        );
    }

    #[test]
    fn empty_registry_is_index_zero() {
        let entries: Vec<(String, Handle<StandardMaterial>)> = Vec::new();
        assert_eq!(material_combobox_selected_index(&entries, None), 0);
    }
}
