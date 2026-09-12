//! The inspector card for the open definition asset.
//!
//! The card is the generic component card with the definition's reflected
//! fields in its body, so scalars, enum menus and list rows behave exactly as
//! they do on a component. Its header carries the Save action and says when
//! the definition has unsaved edits, and the card opens expanded, since it is
//! all the panel has to show.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::reflect::ReflectFromReflect;
use jackdaw_api::op::Operator as _;
use jackdaw_api::prelude::AssetKinds;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button},
    icons::{EditorFont, IconFont},
    tokens,
};
use jackdaw_widgets::collapsible::CollapsibleHeader;

use crate::definition_assets::{AssetSaveOp, DefinitionAssetEdit};

use super::component_display::{ComponentDisplaySpec, spawn_component_display};
use super::schema_fields::{SchemaFieldContext, spawn_schema_fields};

/// What a definition card puts in its body: a reflected value the editor can
/// walk, or a value it has only the project's schema for.
enum CardBody {
    Reflected(Box<dyn Reflect>),
    Schema(Box<jackdaw_schema::TypeSchema>, serde_json::Value),
}

/// Marks the card a definition puts up: it stands for a whole file, so no
/// category tab files it or hides it.
#[derive(Component)]
pub(crate) struct DefinitionCard;

/// Drop the card the inspector already holds for a definition, so filling it
/// twice in a frame leaves one card rather than two.
fn despawn_existing_card(world: &mut World, inspector: Entity) {
    let Some(children) = world.get::<Children>(inspector) else {
        return;
    };
    let cards: Vec<Entity> = children
        .iter()
        .filter(|&child| world.get::<DefinitionCard>(child).is_some())
        .collect();
    for card in cards {
        if let Ok(entity) = world.get_entity_mut(card) {
            entity.despawn();
        }
    }
}

/// Build the card for the definition `source` is editing under `inspector`.
pub(crate) fn fill_definition_card(world: &mut World, inspector: Entity, source: Entity) {
    despawn_existing_card(world, inspector);
    let Some((kind, name, type_path, path, dirty)) =
        world.get::<DefinitionAssetEdit>(source).map(|edit| {
            (
                edit.kind.clone(),
                edit.name.clone(),
                edit.type_path.clone(),
                edit.path.clone(),
                edit.dirty,
            )
        })
    else {
        return;
    };
    let registered = world
        .get_resource::<AssetKinds>()
        .and_then(|types| types.by_kind(&kind))
        .cloned();
    let label = registered
        .as_ref()
        .map_or_else(|| kind.clone(), |definition| definition.label.clone());
    let schema_backed = registered.is_some_and(|definition| definition.schema_backed());
    let body = if schema_backed {
        let Some(schema) = crate::definition_assets::definition_schema(world, &kind) else {
            bevy::log::warn_once!(
                "this project reports no shape for {type_path}, which its {kind} files hold"
            );
            return;
        };
        let Some(value) = crate::definition_assets::schema_definition_json(world, &kind, &path)
        else {
            bevy::log::warn_once!(
                "no {kind} named '{name}' is loaded, so its {type_path} card is empty"
            );
            return;
        };
        CardBody::Schema(Box::new(schema), value)
    } else {
        match definition_snapshot(world, source, &type_path) {
            Some(value) => CardBody::Reflected(value),
            None => {
                bevy::log::warn_once!(
                    "the editor has no registration for {type_path}, which {kind} files hold"
                );
                return;
            }
        }
    };

    let registry = world.resource::<AppTypeRegistry>().clone();
    let server = world.get_resource::<AssetServer>().cloned();
    let icon_font = world.resource::<IconFont>().0.clone();
    let editor_font = world.resource::<EditorFont>().0.clone();
    let mut collapse_state =
        super::InspectorCollapseState(world.resource::<super::InspectorCollapseState>().0.clone());

    let card_name = format!("{name} ({label})");
    collapse_state.0.entry(card_name.clone()).or_insert(false);
    let card = {
        let mut state: SystemState<Commands> = SystemState::new(world);
        let Ok(mut commands) = state.get_mut(world) else {
            return;
        };
        let card = spawn_component_display(
            &mut commands,
            ComponentDisplaySpec {
                name: &card_name,
                type_path: &type_path,
                entity: source,
                component: None,
                is_overridden: false,
                is_derived: false,
                prefab_ctx: None,
                revert_through_prefab: false,
                icon_font: &icon_font,
                editor_font: &editor_font,
                collapse_state: &collapse_state,
            },
        );
        commands.entity(card.section).insert(DefinitionCard);
        jackdaw_feathers::utils::attach_or_despawn(&mut commands, inspector, card.section);
        state.apply(world);
        card
    };

    match body {
        CardBody::Reflected(value) => {
            let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
            let Ok((mut commands, names)) = state.get_mut(world) else {
                return;
            };
            super::reflect_fields::spawn_reflected_fields(
                &mut commands,
                card.body,
                value.as_ref(),
                0,
                String::new(),
                source,
                &type_path,
                &names,
                &registry,
                &editor_font,
                &icon_font,
            );
            state.apply(world);
        }
        CardBody::Schema(schema, value) => {
            let asset_types = super::schema_fields::asset_path_types(world);
            world.resource_scope(|world, types: Mut<crate::project_types::ProjectTypes>| {
                let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
                let Ok((mut commands, names)) = state.get_mut(world) else {
                    return;
                };
                let ctx = SchemaFieldContext {
                    types: &types,
                    source,
                    type_path: &type_path,
                    names: &names,
                    registry: &registry,
                    server: server.as_ref(),
                    asset_types: &asset_types,
                    editor_font: &editor_font,
                    icon_font: &icon_font,
                };
                spawn_schema_fields(&mut commands, card.body, &ctx, &schema, &value, "", 0);
                state.apply(world);
            });
        }
    }

    spawn_save_action(world, card.section, source, dirty);
}

/// The definition's value, owned, so the field rows can be spawned while the
/// world is borrowed for commands.
fn definition_snapshot(world: &World, source: Entity, type_path: &str) -> Option<Box<dyn Reflect>> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let value = crate::definition_assets::definition_value(world, source, type_path, &registry)?;
    registry
        .get_with_type_path(type_path)?
        .data::<ReflectFromReflect>()?
        .from_reflect(value.as_partial_reflect())
}

/// The header's unsaved-edits marker, following the definition it names.
#[derive(Component)]
pub(crate) struct UnsavedMarker(Entity);

/// Show the marker exactly while the definition it names has unsaved edits.
pub(crate) fn keep_unsaved_marker_in_step(
    mut markers: Query<(&UnsavedMarker, &mut Node)>,
    edits: Query<&DefinitionAssetEdit>,
) {
    for (marker, mut node) in &mut markers {
        let display = if edits.get(marker.0).is_ok_and(|edit| edit.dirty) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

/// Put Save, and the unsaved-edits marker, in the card's header.
fn spawn_save_action(world: &mut World, section: Entity, source: Entity, dirty: bool) {
    let Some(header) = world.get::<Children>(section).and_then(|children| {
        children
            .iter()
            .find(|&child| world.get::<CollapsibleHeader>(child).is_some())
    }) else {
        return;
    };
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(header),
        ))
        .id();
    world.spawn((
        Text::new("Unsaved"),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            display: if dirty { Display::Flex } else { Display::None },
            ..default()
        },
        UnsavedMarker(source),
        ChildOf(row),
    ));
    let save = world
        .spawn((
            button(
                ButtonProps::new("Save")
                    .with_variant(ButtonVariant::Default)
                    .with_size(ButtonSize::MD),
            ),
            ButtonOperatorCall::new(AssetSaveOp::ID),
        ))
        .id();
    world.entity_mut(save).insert(ChildOf(row));
}
