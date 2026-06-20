use avian3d::prelude::RigidBody;
use bevy::prelude::*;
use jackdaw_avian_integration::AvianCollider;
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
/// `on_add_header_mount_added` fires on `Add` and builds the initial header;
/// `rebuild_add_header` replaces the children whenever the active category or
/// the card set changes.
#[derive(Component)]
pub(crate) struct InspectorAddHeaderMount;

/// Populate the add-header when the mount node appears in the world.
pub(crate) fn on_add_header_mount_added(
    trigger: On<Add, InspectorAddHeaderMount>,
    active: Res<ActiveInspectorCategory>,
    selection: Res<Selection>,
    has_rb: Query<Has<RigidBody>>,
    has_ac: Query<Has<AvianCollider>>,
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
    let (rb, ac) = physics_presence(&selection, &has_rb, &has_ac);
    build_add_header_children(&mut commands, host, active.0.as_ref(), rb, ac);
}

/// Each frame: replace the add-header children when the active category
/// changes, the selection changes, or when the displayed card set changes
/// (component added or removed). The physics branch additionally tracks
/// whether the selected entity carries `RigidBody` / `AvianCollider` so the
/// grid buttons can reflect their active state without waiting for a
/// re-select.
pub(crate) fn rebuild_add_header(
    active: Res<ActiveInspectorCategory>,
    selection: Res<Selection>,
    material_registry: Option<Res<MaterialRegistry>>,
    mounts: Query<(Entity, Option<&Children>), With<InspectorAddHeaderMount>>,
    has_rb: Query<Has<RigidBody>>,
    has_ac: Query<Has<AvianCollider>>,
    added_cards: Query<(), Added<super::ComponentDisplay>>,
    mut removed_cards: RemovedComponents<super::ComponentDisplay>,
    mut commands: Commands,
) {
    let cards_changed = !added_cards.is_empty() || removed_cards.read().next().is_some();
    // The Material tab's combobox reflects the registry contents and the
    // selection's assigned material; rebuild it when the registry changes
    // (a material was created, applied, or rescanned) so it stays in sync.
    let registry_changed = material_registry.is_some_and(|r| r.is_changed());
    let should_rebuild =
        active.is_changed() || selection.is_changed() || cards_changed || registry_changed;
    if !should_rebuild {
        return;
    }

    let (rb, ac) = physics_presence(&selection, &has_rb, &has_ac);

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
        build_add_header_children(&mut commands, host, active.0.as_ref(), rb, ac);
    }
}

/// Return `(has_rigid_body, has_avian_collider)` for the primary selected entity.
fn physics_presence(
    selection: &Selection,
    has_rb: &Query<Has<RigidBody>>,
    has_ac: &Query<Has<AvianCollider>>,
) -> (bool, bool) {
    let Some(entity) = selection.primary() else {
        return (false, false);
    };
    let rb = has_rb.get(entity).unwrap_or(false);
    let ac = has_ac.get(entity).unwrap_or(false);
    (rb, ac)
}

/// Spawn the appropriate add-UI children under `host` for `category`.
fn build_add_header_children(
    commands: &mut Commands,
    host: Entity,
    category: &str,
    has_rigid_body: bool,
    has_collider: bool,
) {
    match category {
        "components" => {
            commands.spawn((
                button(ButtonProps::new("Add Component").align_left()),
                InspectorAddComponentButton,
                ChildOf(host),
            ));
        }
        "modifiers" => {
            commands.spawn((
                button(ButtonProps::new("Add Modifier").align_left()),
                ButtonOperatorCall::new("modifier.add").with_param("kind", "mirror"),
                ChildOf(host),
            ));
        }
        "physics" => {
            // A two-button row: Rigid Body and Collider.
            // Each button shows its active/remove state when the component is
            // already present on the selected entity.
            let row = commands
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

            spawn_physics_chip(
                commands,
                row,
                "Rigid Body",
                super::physics_display::RIGID_BODY_TYPE_PATH,
                has_rigid_body,
            );
            spawn_physics_chip(
                commands,
                row,
                "Collider",
                super::physics_display::AVIAN_COLLIDER_TYPE_PATH,
                has_collider,
            );
        }
        "material" => {
            // Defer so we can read `MaterialRegistry` from the world.
            let host_captured = host;
            commands.queue(move |world: &mut World| {
                spawn_material_header(world, host_captured);
            });
        }
        _ => {}
    }
}

/// Spawn a single physics attach/detach chip button inside `row`.
///
/// When `present` is true the button renders with `ButtonVariant::Active` and
/// a trailing X label; clicking dispatches `component.remove`. When `present`
/// is false it renders as a plain default button and clicking dispatches
/// `component.add`.
fn spawn_physics_chip(
    commands: &mut Commands,
    row: Entity,
    label: &'static str,
    type_path: &'static str,
    present: bool,
) {
    // The selected entity is only known at click time (the user can reselect
    // while the header is shown), so the chip carries a marker and
    // `on_physics_chip_click` reads the current `Selection` and dispatches the
    // add/remove operator with the resolved entity. A `ButtonOperatorCall` is
    // deliberately NOT attached: it would auto-dispatch a second time without
    // the required `entity` param.
    let variant = if present {
        ButtonVariant::Active
    } else {
        ButtonVariant::Default
    };

    let display_label = if present {
        format!("{label}  x")
    } else {
        label.to_string()
    };

    commands.spawn((
        button(
            ButtonProps::new(display_label)
                .with_variant(variant)
                .with_size(ButtonSize::MD),
        ),
        PhysicsChipButton { type_path, present },
        ChildOf(row),
    ));
}

/// Marker on a physics chip button. Carries the `type_path` and presence flag
/// so the click observer can forward the right operator without holding a
/// stale entity capture.
#[derive(Component)]
pub(crate) struct PhysicsChipButton {
    pub(crate) type_path: &'static str,
    pub(crate) present: bool,
}

/// Observer wired by `InspectorPlugin`: intercepts `ButtonClickEvent` on any
/// `PhysicsChipButton` and dispatches `component.add` or `component.remove`
/// against the current primary selection.
pub(crate) fn on_physics_chip_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    chips: Query<&PhysicsChipButton>,
    selection: Res<Selection>,
    mut commands: Commands,
) {
    let Ok(chip) = chips.get(event.entity) else {
        return;
    };
    let Some(entity) = selection.primary() else {
        return;
    };
    let type_path = chip.type_path;
    if chip.present {
        commands
            .operator("component.remove")
            .param("entity", entity)
            .param("type_path", type_path)
            .call();
    } else {
        commands
            .operator("component.add")
            .param("entity", entity)
            .param("type_path", type_path)
            .call();
    }
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
                        material: handle.clone(),
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
            world.trigger(ApplyMaterialDefToFaces { material: handle });
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
