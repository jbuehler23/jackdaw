//! Reflected-type bridge into the feathers tooltip pipeline.
//!
//! Attach [`ReflectedTypeTooltip`] to any UI entity that
//! displays a reflected Bevy type. The observer derives a
//! [`Tooltip`]: short name as title, resolved type chrome
//! description as body, full type path as footer.
//!
//! Same bridge shape as the `ButtonOperatorCall` to `Tooltip` path
//! in `src/operator_tooltip.rs`.

use std::borrow::Cow;

use crate::project_types::ProjectTypes;
use crate::type_metadata::TypeMetadata;
use bevy::prelude::*;
use jackdaw_feathers::tooltip::Tooltip;

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
/// same entity. Falls back to the project schema when the type is not
/// in the editor registry.
fn auto_attach_reflected_type_tooltip(
    trigger: On<Add, ReflectedTypeTooltip>,
    sources: Query<&ReflectedTypeTooltip>,
    type_registry: Res<AppTypeRegistry>,
    project_types: Res<ProjectTypes>,
    type_metadata: Res<TypeMetadata>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(source) = sources.get(entity) else {
        return;
    };
    let type_path = source.type_path.as_ref();
    let registry = type_registry.read();
    let title = if let Some(registration) = registry.get_with_type_path(type_path) {
        registration
            .type_info()
            .type_path_table()
            .short_path()
            .to_string()
    } else if let Some(schema) = project_types.component(type_path) {
        schema.short_name.clone()
    } else {
        return;
    };
    let description = type_metadata
        .resolve(type_path, &registry, &project_types)
        .description;
    let footer = source.type_path.to_string();
    commands.entity(entity).insert(
        Tooltip::title(title)
            .with_description(description)
            .with_footer(footer),
    );
}
