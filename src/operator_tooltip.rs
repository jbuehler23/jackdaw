//! Operator-domain bridge into the generic feathers tooltip pipeline.
//!
//! The actual hover/render machinery lives in
//! [`jackdaw_feathers::tooltip`] and reads only the generic
//! [`Tooltip`] component. This module owns the bridge from
//! [`ButtonOperatorCall`] (the editor's "this button calls operator
//! X with concrete params" component) into a derived `Tooltip` —
//! when a `ButtonOperatorCall` is added to an entity, an observer
//! looks up the registered [`OperatorEntity`], formats the
//! signature from the button's concrete params, and inserts the
//! corresponding `Tooltip`.
//!
//! Future tooltip sources follow the same shape: a small source
//! component carrying the data the surface owns, plus one observer
//! on `Add, <SourceComponent>` that derives a `Tooltip` and inserts
//! it. See `src/inspector/component_tooltip.rs` for the
//! reflection-driven counterpart.

use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonParamValue},
    tooltip::Tooltip,
};

pub struct OperatorTooltipPlugin;

impl Plugin for OperatorTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(auto_attach_button_tooltip);
    }
}

/// Derive a [`Tooltip`] from the operator backing a freshly-added
/// [`ButtonOperatorCall`] and insert it on the same entity.
///
/// Skips the insert silently when the operator id doesn't resolve
/// (e.g. extension not loaded yet) — the entity stays without a
/// tooltip and nothing is rendered, which is the right fallback for
/// the rare race where a button outraces operator registration.
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
            .with_footer(format_button_signature(op.id(), &call.params)),
    );
}

/// Render `id(name: value, ...)` for the given button params. Uses
/// the button's concrete values, so two buttons calling the same
/// operator with different args produce distinct signatures.
fn format_button_signature(
    id: &str,
    params: &[(std::borrow::Cow<'static, str>, ButtonParamValue)],
) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(id.len() + 2);
    out.push_str(id);
    out.push('(');
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(k);
        out.push_str(": ");
        match v {
            ButtonParamValue::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            ButtonParamValue::Int(n) => {
                let _ = write!(out, "{n}");
            }
            ButtonParamValue::Float(f) => {
                let _ = write!(out, "{f}");
            }
            ButtonParamValue::Str(s) => {
                let _ = write!(out, "\"{s}\"");
            }
        }
    }
    out.push(')');
    out
}
