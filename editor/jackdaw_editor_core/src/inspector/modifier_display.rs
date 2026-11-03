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
    inspector_card::{InspectorCardOpts, spawn_inspector_card},
    tokens,
};

use jackdaw_geometry::{Modifier, ModifierEntry, ModifierStack};

use super::{ComponentDisplay, ComponentDisplayTypePath, ComponentName, reflect_fields};

/// Capitalize the modifier kind label for the sub-card header.
fn kind_label(modifier: &Modifier) -> String {
    let kind = modifier.kind_str();
    let mut chars = kind.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Render one top-level inspector card per modifier entry under `parent`.
/// The "Add Modifier" button lives in the Modifiers tab header (see
/// `add_header.rs`), so no footer is emitted here.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the reflected-field call site; threading the registry, names query, and fonts is unavoidable"
)]
pub(super) fn spawn_modifier_display(
    commands: &mut Commands,
    parent: Entity,
    source_entity: Entity,
    stack: &ModifierStack,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
    collapse_state: &super::InspectorCollapseState,
) {
    let stack_type_path = <ModifierStack as TypePath>::type_path();

    for (i, entry) in stack.modifiers.iter().enumerate() {
        spawn_entry_card(
            commands,
            parent,
            source_entity,
            i,
            entry,
            stack_type_path,
            entity_names,
            type_registry,
            editor_font,
            icon_font,
            collapse_state,
        );
    }
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
    collapse_state: &super::InspectorCollapseState,
) {
    let label = kind_label(&entry.modifier);
    let card_ents = spawn_inspector_card(
        commands,
        parent,
        &label,
        icon_font,
        InspectorCardOpts {
            collapsible: true,
            removable: false,
            collapsed: collapse_state.collapsed(&label),
            ..Default::default()
        },
    );

    // Tag the section so the category filter places it in the Modifiers tab.
    commands.entity(card_ents.section).insert((
        ComponentDisplay,
        ComponentName(label.clone()),
        ComponentDisplayTypePath(stack_type_path.to_string()),
    ));

    let card = card_ents.body;
    let header = card_ents.header;

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
