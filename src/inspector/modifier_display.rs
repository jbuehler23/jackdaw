//! Inspector card for a brush's `ModifierStack`. Renders one sub-card per
//! stack entry with its per-entry flag toggles, reorder / apply / remove
//! controls, and the modifier's editable fields.
//!
//! Structural changes (add / remove / reorder / toggle / apply) dispatch the
//! `modifier.*` operators through `ButtonOperatorCall`; the UI never mutates
//! the stack directly. Field values (the mirror axes, offset, clip, merge,
//! bisect) flow through the generic reflected-field rows, which commit via the
//! same undoable AST path every reflected component uses.

use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button},
    icons::Icon,
    tokens,
};

use jackdaw_geometry::{Modifier, ModifierEntry, ModifierStack};

use super::reflect_fields;

/// Capitalize the modifier kind label for the sub-card header.
fn kind_label(modifier: &Modifier) -> String {
    let kind = modifier.kind_str();
    let mut chars = kind.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Render the modifier-stack card under `body_entity`.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the reflected-field call site; threading the registry, names query, and fonts is unavoidable"
)]
pub(super) fn spawn_modifier_display(
    commands: &mut Commands,
    body_entity: Entity,
    source_entity: Entity,
    stack: &ModifierStack,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    let stack_type_path = <ModifierStack as TypePath>::type_path();

    for (i, entry) in stack.modifiers.iter().enumerate() {
        spawn_entry_card(
            commands,
            body_entity,
            source_entity,
            i,
            entry,
            stack_type_path,
            entity_names,
            type_registry,
            editor_font,
            icon_font,
        );
    }

    // Footer affordance to append another modifier. Mirror is the only
    // kind, so a single button is enough. The button bundle carries its own
    // Node, so the top margin lives on a wrapping container.
    let footer = commands
        .spawn((
            Node {
                margin: UiRect::top(px(tokens::SPACING_SM)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(body_entity),
        ))
        .id();
    commands.spawn((
        button(
            ButtonProps::new("Add Mirror")
                .with_variant(ButtonVariant::Default)
                .with_left_icon(Icon::Plus)
                .align_left(),
        ),
        ButtonOperatorCall::new("modifier.add").with_param("kind", "mirror"),
        ChildOf(footer),
    ));
}

#[expect(
    clippy::too_many_arguments,
    reason = "one sub-card needs the entry index, fonts, registry, and names query to render both its controls and its reflected fields"
)]
fn spawn_entry_card(
    commands: &mut Commands,
    parent: Entity,
    source_entity: Entity,
    index: usize,
    entry: &ModifierEntry,
    stack_type_path: &str,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    let card = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::all(px(tokens::SPACING_SM)),
                margin: UiRect::top(px(tokens::SPACING_XS)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::COMPONENT_CARD_RADIUS)),
                ..Default::default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_HEADER_BG),
            BorderColor::all(tokens::COMPONENT_CARD_BORDER),
            ChildOf(parent),
        ))
        .id();

    // Header row: kind label on the left, flag toggles on the right.
    let header = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    commands.spawn((
        Text::new(kind_label(&entry.modifier)),
        TextFont {
            font: editor_font.clone(),
            font_size: tokens::FONT_SM,
            weight: FontWeight::MEDIUM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(header),
    ));

    let flags = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(header),
        ))
        .id();

    // Per-entry flag toggles. Active variant marks the flag as on; the
    // dim default reads as off. Each dispatches `modifier.toggle` with
    // the flag name.
    spawn_flag_toggle(commands, flags, index, "enabled", Icon::Eye, entry.enabled);
    spawn_flag_toggle(
        commands,
        flags,
        index,
        "in_game",
        Icon::Gamepad2,
        entry.in_game,
    );
    spawn_flag_toggle(
        commands,
        flags,
        index,
        "on_mesh",
        Icon::Pencil,
        entry.on_mesh,
    );

    // Action row: reorder, apply (bake), and remove.
    let actions = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    spawn_action_button(
        commands,
        actions,
        "modifier.move_up",
        index,
        Icon::ArrowUp,
        ButtonVariant::Ghost,
    );
    spawn_action_button(
        commands,
        actions,
        "modifier.move_down",
        index,
        Icon::ArrowDown,
        ButtonVariant::Ghost,
    );
    spawn_action_button(
        commands,
        actions,
        "modifier.apply",
        index,
        Icon::Check,
        ButtonVariant::Ghost,
    );
    spawn_action_button(
        commands,
        actions,
        "modifier.remove",
        index,
        Icon::Trash2,
        ButtonVariant::Destructive,
    );

    // Editable modifier fields. The reflected-field commit path resolves
    // `modifiers[i].modifier` against the `ModifierStack` type, unwraps the
    // `Modifier::Mirror` variant, and writes the named field. Same generic,
    // undoable path the inspector uses for every other reflected component.
    let Modifier::Mirror(mirror) = &entry.modifier;
    reflect_fields::spawn_reflected_fields(
        commands,
        card,
        mirror,
        0,
        // `.0` addresses the `MeshMirror` payload of the `Modifier::Mirror`
        // newtype variant; the path navigator flattens it back to the inner
        // struct so each field commits through the standard reflected path.
        format!("modifiers[{index}].modifier.0"),
        source_entity,
        stack_type_path,
        entity_names,
        type_registry,
        editor_font,
        icon_font,
    );
}

/// Spawn one per-entry flag toggle that dispatches `modifier.toggle`.
fn spawn_flag_toggle(
    commands: &mut Commands,
    parent: Entity,
    index: usize,
    flag: &'static str,
    icon: Icon,
    on: bool,
) {
    let variant = if on {
        ButtonVariant::Active
    } else {
        ButtonVariant::Ghost
    };
    commands.spawn((
        button(
            ButtonProps::new("")
                .with_variant(variant)
                .with_size(ButtonSize::IconSM)
                .with_left_icon(icon),
        ),
        ButtonOperatorCall::new("modifier.toggle")
            .with_param("index", index as i64)
            .with_param("flag", flag),
        ChildOf(parent),
    ));
}

/// Spawn one index-carrying action button that dispatches an operator.
fn spawn_action_button(
    commands: &mut Commands,
    parent: Entity,
    operator: &'static str,
    index: usize,
    icon: Icon,
    variant: ButtonVariant,
) {
    commands.spawn((
        button(
            ButtonProps::new("")
                .with_variant(variant)
                .with_size(ButtonSize::IconSM)
                .with_left_icon(icon),
        ),
        ButtonOperatorCall::new(operator).with_param("index", index as i64),
        ChildOf(parent),
    ));
}
