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

use jackdaw_schema::{ProjectSchema, TypeSchema};

/// Editor resource: the project's dynamic (schema-reported) component
/// and resource types, keyed by reflect type path. Refreshed from the
/// extractor on each project build.
#[derive(Resource, Default)]
pub struct ProjectTypes {
    components: HashMap<String, TypeSchema>,
    resources: HashMap<String, TypeSchema>,
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

    /// Whether any project types are known yet.
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
    }
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
