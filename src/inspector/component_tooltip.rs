//! Reflected-type bridge into the feathers tooltip pipeline.
//!
//! Attach [`ReflectedTypeTooltip`] to any UI entity that
//! displays a reflected Bevy type. The observer derives a
//! [`Tooltip`]: short name as title, the
//! [`EditorDescription`] attribute (or the reflected doc
//! comment when absent) as body, full type path as footer.
//!
//! Mirrors the `ButtonOperatorCall` to `Tooltip` bridge in
//! `src/operator_tooltip.rs`.

use std::borrow::Cow;

use bevy::prelude::*;
use jackdaw_feathers::tooltip::Tooltip;

use jackdaw_runtime::EditorDescription;

/// Source component for type-reflection-driven tooltips. Carries
/// the fully-qualified `type_path` of a Bevy reflected type that
/// has been registered in [`AppTypeRegistry`]; the auto-attach
/// observer below resolves the registry entry and inserts a
/// [`Tooltip`] derived from it.
#[derive(Component, Clone, Debug)]
pub struct ReflectedTypeTooltip {
    pub type_path: Cow<'static, str>,
}

impl ReflectedTypeTooltip {
    pub fn new(type_path: impl Into<Cow<'static, str>>) -> Self {
        Self {
            type_path: type_path.into(),
        }
    }
}

pub(super) fn plugin(app: &mut App) {
    app.add_observer(auto_attach_reflected_type_tooltip);
}

/// Derive a [`Tooltip`] from the type registry entry pointed at by
/// a freshly-added [`ReflectedTypeTooltip`] and insert it on the
/// same entity. Skips the insert silently if the type isn't
/// registered.
fn auto_attach_reflected_type_tooltip(
    trigger: On<Add, ReflectedTypeTooltip>,
    sources: Query<&ReflectedTypeTooltip>,
    type_registry: Res<AppTypeRegistry>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(source) = sources.get(entity) else {
        return;
    };
    let registry = type_registry.read();
    let Some(registration) = registry.get_with_type_path(source.type_path.as_ref()) else {
        return;
    };
    let info = registration.type_info();
    let title = info.type_path_table().short_path().to_string();
    // `custom_attributes()` lives on the variant types
    // (StructInfo / EnumInfo / ...), so reach in via a match.
    let attrs = match info {
        bevy::reflect::TypeInfo::Struct(s) => Some(s.custom_attributes()),
        bevy::reflect::TypeInfo::TupleStruct(s) => Some(s.custom_attributes()),
        bevy::reflect::TypeInfo::Enum(e) => Some(e.custom_attributes()),
        _ => None,
    };
    let description = attrs
        .and_then(|a| a.get::<EditorDescription>())
        .map(|d| d.0.to_string())
        .or_else(|| {
            info.docs()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_default();
    let footer = source.type_path.to_string();
    commands.entity(entity).insert(
        Tooltip::title(title)
            .with_description(description)
            .with_footer(footer),
    );
}
