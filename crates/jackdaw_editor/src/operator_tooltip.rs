//! Operator bridge into the generic feathers tooltip pipeline.
//!
//! [`jackdaw_feathers::tooltip`] owns hover/render and reads only the
//! generic [`Tooltip`] component. This module's `Add,
//! ButtonOperatorCall` observer looks up the matching
//! [`OperatorEntity`] and inserts a `Tooltip` carrying its label,
//! description, and concrete call signature.
//!
//! Other tooltip sources follow the same shape (one source
//! component, one `Add` observer). See
//! `src/inspector/component_tooltip.rs` for the reflection-driven
//! counterpart.

use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_feathers::{button::ButtonOperatorCall, tooltip::Tooltip};

pub struct OperatorTooltipPlugin;

impl Plugin for OperatorTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(auto_attach_button_tooltip);
    }
}

/// Derive a [`Tooltip`] from the operator backing a freshly-added
/// [`ButtonOperatorCall`] and insert it on the same entity.
/// Silently skips when the operator id doesn't resolve (e.g.
/// extension not loaded yet); the button just renders without a
/// tooltip until the next layout pass.
fn auto_attach_button_tooltip(
    trigger: On<Add, ButtonOperatorCall>,
    calls: Query<&ButtonOperatorCall>,
    operators: Query<&OperatorEntity>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(call) = calls.get(entity) else {
        return;
    };
    let Some(op) = operators.iter().find(|o| o.id() == call.id.as_ref()) else {
        return;
    };
    commands.entity(entity).insert(
        Tooltip::title(op.label())
            .with_description(op.description())
            .with_footer(call.to_string()),
    );
}
