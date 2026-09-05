//! Open registry of UI widgets exposed by editor extensions.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use bevy::prelude::*;
use lucide_icons::Icon;

/// Context supplied when a palette or extension creates an authored widget.
#[derive(Clone, Copy, Debug, Default)]
pub struct WidgetInstantiateContext {
    /// Authored parent/slot owner, if the widget is inserted into a container.
    pub parent: Option<Entity>,
}

/// Reflected value shape shown by a generic widget-property editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetPropertyKind {
    String,
    Bool,
    Number,
    Color,
    Enum,
    Asset,
}

/// Public editable property exposed by a widget definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetProperty {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub kind: WidgetPropertyKind,
}

/// Stable named child location exposed by a widget definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetSlot {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub accepts_multiple: bool,
}

/// Runtime interaction state that the editor can force for preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetPreviewState {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
}

pub type WidgetInstantiateFn = Arc<
    dyn Fn(&mut World, WidgetInstantiateContext) -> Result<Entity, String> + Send + Sync + 'static,
>;

/// One selectable widget type in the UI Widgets palette.
#[derive(Clone)]
pub struct WidgetDefinition {
    pub id: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub category: Cow<'static, str>,
    pub icon: Option<Icon>,
    pub properties: Vec<WidgetProperty>,
    pub slots: Vec<WidgetSlot>,
    pub preview_states: Vec<WidgetPreviewState>,
    pub instantiate: WidgetInstantiateFn,
}

impl WidgetDefinition {
    pub fn new(
        id: impl Into<Cow<'static, str>>,
        name: impl Into<Cow<'static, str>>,
        category: impl Into<Cow<'static, str>>,
        instantiate: impl Fn(&mut World, WidgetInstantiateContext) -> Result<Entity, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category: category.into(),
            icon: None,
            properties: Vec::new(),
            slots: Vec::new(),
            preview_states: Vec::new(),
            instantiate: Arc::new(instantiate),
        }
    }

    #[must_use]
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub fn with_property(mut self, property: WidgetProperty) -> Self {
        self.properties.push(property);
        self
    }

    #[must_use]
    pub fn with_slot(mut self, slot: WidgetSlot) -> Self {
        self.slots.push(slot);
        self
    }

    #[must_use]
    pub fn with_preview_state(mut self, state: WidgetPreviewState) -> Self {
        self.preview_states.push(state);
        self
    }
}

/// Live widget vocabulary assembled from built-in and extension definitions.
#[derive(Resource, Default)]
pub struct WidgetRegistry {
    definitions: HashMap<String, Vec<RegisteredDefinition>>,
    next_registration: u64,
}

struct RegisteredDefinition {
    registration: WidgetRegistrationId,
    definition: Arc<WidgetDefinition>,
}

impl WidgetRegistry {
    pub fn register(&mut self, definition: WidgetDefinition) {
        self.register_scoped(definition);
    }

    pub(crate) fn register_scoped(&mut self, definition: WidgetDefinition) -> WidgetRegistrationId {
        let registration = WidgetRegistrationId(self.next_registration);
        self.next_registration += 1;
        self.definitions
            .entry(definition.id.to_string())
            .or_default()
            .push(RegisteredDefinition {
                registration,
                definition: Arc::new(definition),
            });
        registration
    }

    pub fn unregister(&mut self, id: &str) -> bool {
        self.definitions.remove(id).is_some()
    }

    pub(crate) fn unregister_scoped(
        &mut self,
        id: &str,
        registration: WidgetRegistrationId,
    ) -> bool {
        let Some(definitions) = self.definitions.get_mut(id) else {
            return false;
        };
        let Some(index) = definitions
            .iter()
            .position(|candidate| candidate.registration == registration)
        else {
            return false;
        };
        definitions.remove(index);
        if definitions.is_empty() {
            self.definitions.remove(id);
        }
        true
    }

    pub fn get(&self, id: &str) -> Option<Arc<WidgetDefinition>> {
        self.definitions
            .get(id)
            .and_then(|definitions| definitions.last())
            .map(|registered| Arc::clone(&registered.definition))
    }

    pub fn iter(&self) -> impl Iterator<Item = &WidgetDefinition> {
        self.definitions.values().filter_map(|definitions| {
            definitions
                .last()
                .map(|registered| registered.definition.as_ref())
        })
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WidgetRegistrationId(u64);

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(name: &'static str) -> WidgetDefinition {
        WidgetDefinition::new("sample.control", name, "Sample", |world, _| {
            Ok(world.spawn_empty().id())
        })
    }

    #[test]
    fn scoped_override_is_restored_when_the_override_unregisters() {
        let mut registry = WidgetRegistry::default();
        let base = registry.register_scoped(definition("Base"));
        let override_registration = registry.register_scoped(definition("Override"));

        assert_eq!(registry.get("sample.control").unwrap().name, "Override");
        assert!(registry.unregister_scoped("sample.control", override_registration));
        assert_eq!(registry.get("sample.control").unwrap().name, "Base");
        assert!(registry.unregister_scoped("sample.control", base));
        assert!(registry.get("sample.control").is_none());
    }

    #[test]
    fn unregistering_a_shadowed_definition_keeps_the_override() {
        let mut registry = WidgetRegistry::default();
        let base = registry.register_scoped(definition("Base"));
        let override_registration = registry.register_scoped(definition("Override"));

        assert!(registry.unregister_scoped("sample.control", base));
        assert_eq!(registry.get("sample.control").unwrap().name, "Override");
        assert!(registry.unregister_scoped("sample.control", override_registration));
        assert!(registry.is_empty());
    }
}
