//! The rows an animation card carries beyond its reflected fields: the graphs
//! a rig can be pointed at, and the way from a set of clips to a graph.

use bevy::prelude::*;
use jackdaw_animation_runtime::{AnimationGraphRef, AnimationSet};
use jackdaw_api::op::Operator as _;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button},
    tokens,
};

use crate::animation::graph_doc::graph_files;
use crate::animation::graph_ops::{AnimationGraphAssignOp, AnimationGraphConvertSetOp};

/// List the project's graph files under an [`AnimationGraphRef`] card, so the
/// path is picked rather than typed.
pub(super) fn fill_graph_reference_picker(world: &mut World, entity: Entity, body: Entity) {
    let held = world
        .get::<AnimationGraphRef>(entity)
        .map(|reference| reference.path.clone())
        .unwrap_or_default();
    let files = graph_files(world);
    let column = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(tokens::SPACING_XS),
                margin: UiRect::bottom(Val::Px(tokens::SPACING_XS)),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    if files.is_empty() {
        world.spawn((
            Text::new("No graph file in this project yet."),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(column),
        ));
        return;
    }
    for path in files {
        let variant = if path == held {
            ButtonVariant::Active
        } else {
            ButtonVariant::Ghost
        };
        let row = world
            .spawn((
                button(
                    ButtonProps::new(path.clone())
                        .with_variant(variant)
                        .with_size(ButtonSize::MD)
                        .align_left(),
                ),
                ButtonOperatorCall::new(AnimationGraphAssignOp::ID).with_param("path", path),
            ))
            .id();
        world.entity_mut(row).insert(ChildOf(column));
    }
}

/// Offer the way to a graph on an [`AnimationSet`] card that has none.
pub(super) fn fill_animation_set_actions(world: &mut World, entity: Entity, body: Entity) {
    if world.get::<AnimationSet>(entity).is_none()
        || world.get::<AnimationGraphRef>(entity).is_some()
    {
        return;
    }
    let action = world
        .spawn((
            button(
                ButtonProps::new("Convert to graph")
                    .with_variant(ButtonVariant::Default)
                    .with_size(ButtonSize::MD),
            ),
            ButtonOperatorCall::new(AnimationGraphConvertSetOp::ID),
        ))
        .id();
    world.entity_mut(action).insert(ChildOf(body));
}
