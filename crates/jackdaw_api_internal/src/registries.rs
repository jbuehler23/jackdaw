//! Panel-extension registry. Operators now live as entities (see
//! [`crate::lifecycle::OperatorEntity`]) and keybinds go through BEI, so
//! this file is much smaller than in v1; only the panel-extension mapping
//! remains.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::SectionBuildFn;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<PanelExtensionRegistry>();
}

#[derive(Resource, Default)]
pub(crate) struct PanelExtensionRegistry {
    extensions: HashMap<String, Vec<SectionBuildFn>>,
}

impl PanelExtensionRegistry {
    pub(crate) fn add(&mut self, panel_id: String, section: SectionBuildFn) {
        self.extensions.entry(panel_id).or_default().push(section);
    }

    pub(crate) fn remove(&mut self, panel_id: &str, section_index: usize) {
        if let Some(sections) = self.extensions.get_mut(panel_id) {
            if section_index < sections.len() {
                sections.remove(section_index);
            }
            if sections.is_empty() {
                self.extensions.remove(panel_id);
            }
        }
    }

    pub(crate) fn get(&self, panel_id: &str) -> &[SectionBuildFn] {
        self.extensions
            .get(panel_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}
