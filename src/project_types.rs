//! The editor's knowledge of the open project's reflected types,
//! held as data rather than as loaded code.
//!
//! Project component types are never registered as real ECS components
//! in the editor: a loaded dylib can never be unmapped, so loading
//! project code into the editor would leak on every refresh. Instead
//! the out-of-process schema extractor
//! ([`jackdaw_schema`]) reports each type's shape, the
//! editor stores it here, and project components live as dynamic data
//! backed by the scene document. Their real types exist only in the
//! game binary at Play time.
//!
//! Only types the editor does NOT already know natively are kept here.
//! Native types (bevy, avian, jackdaw) keep their real registrations
//! and their existing real-component handling.

use std::collections::HashMap;
use std::collections::HashSet;

use bevy::prelude::*;

use jackdaw_schema::{FunctionSchema, ProjectSchema, TypeSchema};

/// Editor resource: the project's dynamic (schema-reported) component
/// and resource types, keyed by reflect type path. Refreshed from the
/// extractor on each project build.
#[derive(Resource, Default)]
pub struct ProjectTypes {
    components: HashMap<String, TypeSchema>,
    resources: HashMap<String, TypeSchema>,
    events: HashMap<String, TypeSchema>,
    functions: Vec<FunctionSchema>,
}

impl ProjectTypes {
    /// The schema for a project component type, or `None` if the editor
    /// knows the type natively or has never seen it.
    pub fn component(&self, type_path: &str) -> Option<&TypeSchema> {
        self.components.get(type_path)
    }

    /// Whether `type_path` is a dynamic project component (not a native
    /// type). The apply and inspector paths branch on this.
    pub fn is_project_component(&self, type_path: &str) -> bool {
        self.components.contains_key(type_path)
    }

    /// Every project component type, for the picker.
    pub fn components(&self) -> impl Iterator<Item = &TypeSchema> {
        self.components.values()
    }

    /// Every project resource type, for the binding path picker.
    pub fn resources(&self) -> impl Iterator<Item = &TypeSchema> {
        self.resources.values()
    }

    /// Every event type the game can raise, for the action picker.
    ///
    /// Unlike components and resources these are not filtered against the
    /// editor's own registrations: the editor raises no game events, and a
    /// binding may name any event the game knows.
    pub fn events(&self) -> impl Iterator<Item = &TypeSchema> {
        self.events.values()
    }

    /// The schema for one event type, for filling an action's fields.
    pub fn event(&self, type_path: &str) -> Option<&TypeSchema> {
        self.events.get(type_path)
    }

    /// Every function a binding can call, for the transform picker.
    pub fn functions(&self) -> impl Iterator<Item = &FunctionSchema> {
        self.functions.iter()
    }

    /// Whether any project component or resource types are known yet.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty() && self.resources.is_empty()
    }

    /// Replace the stored project types with a fresh extraction,
    /// dropping any type the editor already knows natively (`native`
    /// holds every type path in the editor's `AppTypeRegistry`). Native
    /// types keep their real registrations and real-component handling;
    /// only genuinely project-provided types become dynamic entries.
    pub fn update(&mut self, schema: &ProjectSchema, native: &HashSet<String>) {
        self.components = schema
            .components
            .iter()
            .filter(|c| !native.contains(&c.type_path))
            .map(|c| (c.type_path.clone(), c.clone()))
            .collect();
        self.resources = schema
            .resources
            .iter()
            .filter(|r| !native.contains(&r.type_path))
            .map(|r| (r.type_path.clone(), r.clone()))
            .collect();
        // Events and functions are kept whole: they are never edited as
        // components, and a binding picker that hid natively registered ones
        // would offer less than the game can dispatch.
        self.events = schema
            .events
            .iter()
            .map(|e| (e.type_path.clone(), e.clone()))
            .collect();
        self.functions = schema.functions.clone();
    }
}

/// Tell the apply path which type paths are project components, so a document
/// naming one loads as authored rather than as unknown types. Call after every
/// [`ProjectTypes`] refresh.
///
/// Enums are carried separately, since an authored variant spells a path one
/// segment longer than the schema lists.
pub fn publish_document_only_types(world: &mut World) {
    let (types, enums) = world
        .get_resource::<ProjectTypes>()
        .map(|project| {
            let types = project
                .components()
                .map(|c| c.type_path.clone())
                .collect::<bevy::platform::collections::HashSet<_>>();
            let enums = project
                .components()
                .filter(|c| matches!(c.kind, jackdaw_schema::TypeKind::Enum))
                .map(|c| c.type_path.clone())
                .collect();
            (types, enums)
        })
        .unwrap_or_default();
    world.insert_resource(jackdaw_bsn::DocumentOnlyTypes::new(types, enums));
}

/// The set of type paths the editor already has real registrations
/// for. A project type appearing here is handled by the normal
/// real-component path, not the dynamic path.
pub fn native_type_paths(registry: &bevy::reflect::TypeRegistry) -> HashSet<String> {
    registry
        .iter()
        .map(|reg| reg.type_info().type_path().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_schema::{ArgOwnership, FunctionSchema, TypeKind};

    fn type_schema(type_path: &str) -> TypeSchema {
        TypeSchema {
            type_path: type_path.to_string(),
            short_name: type_path
                .rsplit("::")
                .next()
                .unwrap_or(type_path)
                .to_string(),
            module_path: String::new(),
            category: String::new(),
            description: String::new(),
            hidden: false,
            default_constructible: false,
            fields: Vec::new(),
            kind: TypeKind::Struct,
            default: None,
            variants: Vec::new(),
            entity_fields: Vec::new(),
            fills_gaps: false,
        }
    }

    /// The editor drops components it already knows, but a natively known
    /// event is still one the game can raise, so the action picker keeps it.
    #[test]
    fn native_filtering_skips_events_and_functions() {
        let schema = ProjectSchema {
            components: vec![type_schema("bevy_transform::components::Transform")],
            resources: Vec::new(),
            events: vec![type_schema("bevy_ui_widgets::Activate")],
            functions: vec![FunctionSchema {
                name: "my_game::double".to_string(),
                arg_type_paths: vec!["f32".to_string()],
                arg_ownerships: vec![ArgOwnership::Owned],
                return_type_path: "f32".to_string(),
                return_ownership: ArgOwnership::Owned,
                docs: None,
            }],
        };
        let native: HashSet<String> = [
            "bevy_transform::components::Transform".to_string(),
            "bevy_ui_widgets::Activate".to_string(),
        ]
        .into_iter()
        .collect();

        let mut types = ProjectTypes::default();
        types.update(&schema, &native);

        assert_eq!(types.components().count(), 0);
        assert_eq!(types.events().count(), 1);
        assert!(types.event("bevy_ui_widgets::Activate").is_some());
        assert_eq!(types.functions().count(), 1);
    }
}
