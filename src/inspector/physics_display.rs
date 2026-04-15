//! Dedicated "Physics" section in the inspector. Combines RigidBody +
//! AvianCollider into a single enable/configure UI.

use avian3d::prelude::*;
use bevy::{ecs::reflect::AppTypeRegistry, prelude::*};
use jackdaw_avian_integration::AvianCollider;
use jackdaw_feathers::{
    checkbox::{CheckboxCommitEvent, CheckboxProps, checkbox},
    combobox::{ComboBoxChangeEvent, combobox_with_selected},
    icons::Icon,
    tokens,
};
use jackdaw_widgets::collapsible::{
    CollapsibleBody, CollapsibleHeader, CollapsibleSection, ToggleCollapsible,
};

use crate::commands::{AddComponent, CommandGroup, CommandHistory, EditorCommand};
use crate::inspector::FieldBinding;
use crate::selection::Selection;

/// Marker for the Physics section checkbox.
#[derive(Component)]
pub(super) struct PhysicsEnableCheckbox(pub Entity);

const RIGID_BODY_TYPE_PATH: &str = "avian3d::dynamics::rigid_body::RigidBody";
const AVIAN_COLLIDER_TYPE_PATH: &str = "jackdaw_avian_integration::AvianCollider";

/// Spawn the Physics section in the inspector. Always visible; shows an
/// "Enable" checkbox. When enabled, shows Body type and Collider type
/// dropdowns.
pub(super) fn spawn_physics_section(
    commands: &mut Commands,
    inspector_entity: Entity,
    source_entity: Entity,
    entity_ref: EntityRef,
    icon_font: &Handle<Font>,
    editor_font: &Handle<Font>,
    type_registry: &AppTypeRegistry,
    entity_names: &Query<&Name>,
) {
    let has_rb = entity_ref.contains::<RigidBody>();
    let has_collider = entity_ref.contains::<AvianCollider>();
    let is_enabled = has_rb || has_collider;

    // Collapsible section (card styling)
    let section = commands
        .spawn((
            super::ComponentDisplay,
            CollapsibleSection { collapsed: false },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_BG),
            BorderColor::all(tokens::COMPONENT_CARD_BORDER),
            BoxShadow(vec![ShadowStyle {
                x_offset: Val::ZERO,
                y_offset: Val::ZERO,
                blur_radius: Val::Px(1.0),
                spread_radius: Val::ZERO,
                color: tokens::SHADOW_COLOR,
            }]),
            ChildOf(inspector_entity),
        ))
        .id();

    // Header (card header styling)
    let header = commands
        .spawn((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                width: Val::Percent(100.0),
                border_radius: BorderRadius::top(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG),
            ChildOf(section),
        ))
        .id();

    // Physics icon
    commands.spawn((
        Text::new(String::from(Icon::Zap.unicode())),
        TextFont {
            font: icon_font.clone(),
            font_size: tokens::FONT_MD,
            ..Default::default()
        },
        TextColor(tokens::CATEGORY_ENTITY),
        ChildOf(header),
    ));

    // "Physics" label
    commands.spawn((
        Text::new("Physics"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_MD,
            weight: FontWeight::BOLD,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(header),
    ));

    // Click to collapse/expand
    let section_for_toggle = section;
    commands
        .entity(header)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleCollapsible {
                entity: section_for_toggle,
            });
        });

    // Body
    let body = commands
        .spawn((
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
                row_gap: Val::Px(tokens::SPACING_SM),
                ..Default::default()
            },
            ChildOf(section),
        ))
        .id();

    // Enable checkbox
    commands.spawn((
        checkbox(
            CheckboxProps {
                label: "Enable".into(),
                checked: is_enabled,
            },
            editor_font,
            icon_font,
        ),
        PhysicsEnableCheckbox(source_entity),
        ChildOf(body),
    ));

    if !is_enabled {
        return;
    }

    // Body type dropdown
    let body_variants = vec!["Dynamic", "Static", "Kinematic"];
    let current_body = entity_ref
        .get::<RigidBody>()
        .map(|rb| match rb {
            RigidBody::Dynamic => 0,
            RigidBody::Static => 1,
            RigidBody::Kinematic => 2,
        })
        .unwrap_or(0);

    spawn_labeled_row(commands, body, "Body:", editor_font);
    let body_combo = commands
        .spawn((
            combobox_with_selected(body_variants, current_body),
            FieldBinding {
                source_entity,
                type_path: RIGID_BODY_TYPE_PATH.to_string(),
                field_path: String::new(),
            },
            ChildOf(body),
        ))
        .id();

    commands.entity(body_combo).observe(
        move |event: On<ComboBoxChangeEvent>, mut commands: Commands| {
            let variant = event.label.clone();
            commands.queue(move |world: &mut World| {
                let new_json = serde_json::Value::String(variant.clone());

                let registry = world.resource::<AppTypeRegistry>().clone();
                let reg = registry.read();
                let selection = world.resource::<Selection>();
                let targets: Vec<Entity> = selection.entities.clone();

                let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
                for &target in &targets {
                    let old_json = world
                        .resource::<jackdaw_jsn::SceneJsnAst>()
                        .get_component_field(target, RIGID_BODY_TYPE_PATH, "", &reg)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    sub_commands.push(Box::new(crate::commands::SetJsnField {
                        entity: target,
                        type_path: RIGID_BODY_TYPE_PATH.to_string(),
                        field_path: String::new(),
                        old_value: old_json,
                        new_value: new_json.clone(),
                    }));
                }
                drop(reg);

                if sub_commands.is_empty() {
                    return;
                }
                let mut cmd: Box<dyn EditorCommand> = if sub_commands.len() == 1 {
                    sub_commands.pop().unwrap()
                } else {
                    Box::new(CommandGroup {
                        label: "Set body type".to_string(),
                        commands: sub_commands,
                    })
                };
                cmd.execute(world);
                let mut history = world.resource_mut::<CommandHistory>();
                history.undo_stack.push(cmd);
                history.redo_stack.clear();

                // Rebuild inspector to reflect the change
                for &t in &targets {
                    if let Ok(mut ec) = world.get_entity_mut(t) {
                        ec.insert(super::InspectorDirty);
                    }
                }
            });
        },
    );

    // Collider type dropdown
    let registry = type_registry.read();
    let collider_variants: Vec<String> = if let Some(reg) = registry
        .get_with_type_path("avian3d::collision::collider::constructor::ColliderConstructor")
    {
        if let bevy::reflect::TypeInfo::Enum(enum_info) = reg.type_info() {
            enum_info
                .variant_names()
                .iter()
                .map(|n| n.to_string())
                .collect()
        } else {
            vec!["TrimeshFromMesh".to_string()]
        }
    } else {
        vec!["TrimeshFromMesh".to_string()]
    };

    let current_collider = entity_ref
        .get::<AvianCollider>()
        .and_then(|ac| {
            let variant_name = {
                use bevy::reflect::Enum;
                ac.0.variant_name().to_string()
            };
            collider_variants.iter().position(|n| *n == variant_name)
        })
        .unwrap_or(0);

    drop(registry);

    spawn_labeled_row(commands, body, "Collider:", editor_font);
    let collider_combo = commands
        .spawn((
            combobox_with_selected(collider_variants, current_collider),
            FieldBinding {
                source_entity,
                type_path: AVIAN_COLLIDER_TYPE_PATH.to_string(),
                field_path: "0".to_string(),
            },
            ChildOf(body),
        ))
        .id();

    commands.entity(collider_combo).observe(
        move |event: On<ComboBoxChangeEvent>, mut commands: Commands| {
            let variant = event.label.clone();
            commands.queue(move |world: &mut World| {
                crate::inspector::reflect_fields::apply_enum_variant_with_undo_public(
                    world,
                    source_entity,
                    AVIAN_COLLIDER_TYPE_PATH,
                    "0",
                    &variant,
                );
                // Rebuild inspector so the new variant's fields appear
                if let Ok(mut ec) = world.get_entity_mut(source_entity) {
                    ec.insert(super::InspectorDirty);
                }
            });
        },
    );

    // Collider variant fields (radius for Sphere, dimensions for Cuboid, etc.)
    if let Some(ac) = entity_ref.get::<AvianCollider>() {
        let enum_ref: &dyn bevy::reflect::Enum = &ac.0;
        let field_count = enum_ref.field_len();
        for i in 0..field_count {
            let Some(field_value) = enum_ref.field_at(i) else {
                continue;
            };
            let field_name = enum_ref
                .name_at(i)
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("{i}"));
            let child_path = format!("0.{field_name}");

            crate::inspector::reflect_fields::spawn_field_row_public(
                commands,
                body,
                &field_name,
                field_value,
                1,
                child_path,
                source_entity,
                AVIAN_COLLIDER_TYPE_PATH,
                entity_names,
                type_registry,
                editor_font,
                icon_font,
            );
        }
    }

    // Advanced sub-section: all other avian3d components
    spawn_advanced_section(
        commands,
        body,
        source_entity,
        entity_ref,
        editor_font,
        icon_font,
        type_registry,
        entity_names,
    );
}

/// Collapsible "Advanced" sub-section showing all avian3d internal components
/// (CollisionLayers, ColliderDensity, LinearVelocity, etc.)
fn spawn_advanced_section(
    commands: &mut Commands,
    parent: Entity,
    source_entity: Entity,
    entity_ref: EntityRef,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
    type_registry: &AppTypeRegistry,
    entity_names: &Query<&Name>,
) {
    let registry = type_registry.read();

    // Collect avian3d components on this entity (excluding RigidBody + AvianCollider
    // which are shown above, and Collider which is read-only/opaque)
    let skip_paths: &[&str] = &[
        RIGID_BODY_TYPE_PATH,
        AVIAN_COLLIDER_TYPE_PATH,
        "avian3d::collision::collider::Collider",
    ];

    let mut avian_components: Vec<(&str, &dyn bevy::reflect::Reflect)> = Vec::new();
    for registration in registry.iter() {
        let type_path = registration.type_info().type_path_table().path();
        if !type_path.starts_with("avian3d::") {
            continue;
        }
        if skip_paths.contains(&type_path) {
            continue;
        }
        let Some(reflect_component) = registration.data::<bevy::ecs::reflect::ReflectComponent>()
        else {
            continue;
        };
        let Some(reflected) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        avian_components.push((type_path, reflected));
    }

    if avian_components.is_empty() {
        drop(registry);
        return;
    }

    // Sort by short name for consistent ordering
    avian_components.sort_by_key(|(path, _)| path.rsplit("::").next().unwrap_or(path).to_string());

    // Collapsible "Advanced" sub-section (collapsed by default)
    let section = commands
        .spawn((
            CollapsibleSection { collapsed: true },
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                margin: UiRect::top(Val::Px(tokens::SPACING_SM)),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    let header = commands
        .spawn((
            CollapsibleHeader,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            ChildOf(section),
        ))
        .id();

    commands.spawn((
        Text::new("Advanced"),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            weight: FontWeight::BOLD,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(header),
    ));

    // Click to collapse/expand Advanced
    let adv_section = section;
    commands
        .entity(header)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ToggleCollapsible {
                entity: adv_section,
            });
        });

    let adv_body = commands
        .spawn((
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: Val::Px(tokens::SPACING_XS),
                padding: UiRect::left(Val::Px(tokens::SPACING_SM)),
                ..Default::default()
            },
            ChildOf(section),
        ))
        .id();

    // Render each avian component using the generic reflection display
    for (type_path, reflected) in &avian_components {
        let short_name = type_path.rsplit("::").next().unwrap_or(type_path);

        // Component label
        commands.spawn((
            Text::new(short_name.to_string()),
            TextFont {
                font: editor_font.clone(),
                font_size: tokens::FONT_SM,
                weight: FontWeight::BOLD,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                ..Default::default()
            },
            ChildOf(adv_body),
        ));

        // Component fields via generic reflection
        crate::inspector::reflect_fields::spawn_reflected_fields(
            commands,
            adv_body,
            *reflected,
            1,
            String::new(),
            source_entity,
            type_path,
            entity_names,
            type_registry,
            editor_font,
            icon_font,
        );
    }

    drop(registry);
}

fn spawn_labeled_row(commands: &mut Commands, parent: Entity, label: &str, font: &Handle<Font>) {
    commands.spawn((
        Text::new(label),
        TextFont {
            font: font.clone(),
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(parent),
    ));
}

/// Handle the Enable checkbox toggle.
pub(super) fn on_physics_enable_toggle(
    event: On<CheckboxCommitEvent>,
    checkboxes: Query<&PhysicsEnableCheckbox>,
    mut commands: Commands,
) {
    let Ok(physics_cb) = checkboxes.get(event.entity) else {
        return;
    };
    let source_entity = physics_cb.0;
    let checked = event.checked;

    commands.queue(move |world: &mut World| {
        if checked {
            enable_physics(world, source_entity);
        } else {
            disable_physics(world, source_entity);
        }
        // Rebuild inspector
        if let Ok(mut ec) = world.get_entity_mut(source_entity) {
            ec.insert(super::InspectorDirty);
        }
    });
}

fn enable_physics(world: &mut World, entity: Entity) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let components_res = world.components();

    // Build AddComponent for RigidBody
    let rb_type_id = std::any::TypeId::of::<RigidBody>();
    let rb_component_id = components_res.get_id(rb_type_id);

    // Build AddComponent for AvianCollider
    let ac_type_id = std::any::TypeId::of::<AvianCollider>();
    let ac_component_id = components_res.get_id(ac_type_id);

    drop(reg);

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();

    // Add AvianCollider FIRST so the Collider is built before RigidBody
    // triggers mass computation (avoids "no mass or inertia" warning).
    if let Some(ac_cid) = ac_component_id {
        if !world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<AvianCollider>())
        {
            sub_commands.push(Box::new(AddComponent::new(
                entity,
                ac_type_id,
                ac_cid,
                AVIAN_COLLIDER_TYPE_PATH.to_string(),
            )));
        }
    }

    if let Some(rb_cid) = rb_component_id {
        if !world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<RigidBody>())
        {
            sub_commands.push(Box::new(AddComponent::new(
                entity,
                rb_type_id,
                rb_cid,
                RIGID_BODY_TYPE_PATH.to_string(),
            )));
        }
    }

    if sub_commands.is_empty() {
        return;
    }

    let mut cmd: Box<dyn EditorCommand> = if sub_commands.len() == 1 {
        sub_commands.pop().unwrap()
    } else {
        Box::new(CommandGroup {
            label: "Enable physics".to_string(),
            commands: sub_commands,
        })
    };
    cmd.execute(world);
    let mut history = world.resource_mut::<CommandHistory>();
    history.undo_stack.push(cmd);
    history.redo_stack.clear();
}

fn disable_physics(world: &mut World, entity: Entity) {
    // Remove both components + clean up AST
    if let Ok(mut ec) = world.get_entity_mut(entity) {
        ec.remove::<RigidBody>();
        ec.remove::<AvianCollider>();
        ec.remove::<Collider>();
    }
    if let Some(node) = world
        .resource_mut::<jackdaw_jsn::SceneJsnAst>()
        .node_for_entity_mut(entity)
    {
        node.components.remove(RIGID_BODY_TYPE_PATH);
        node.components.remove(AVIAN_COLLIDER_TYPE_PATH);
        // Also remove derived components that came with them
        node.derived_components.clear();
        node.components.retain(|k, _| !k.starts_with("avian3d::"));
    }
}
