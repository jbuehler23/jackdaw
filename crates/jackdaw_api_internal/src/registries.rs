//! Panel-extension registry. Operators now live as entities (see
//! [`crate::lifecycle::OperatorEntity`]) and keybinds go through input actions, so
//! this file is much smaller than in v1; only the panel-extension mapping
//! remains, alongside the host registries an extension writes into.

use std::{borrow::Cow, collections::HashMap};

use bevy::prelude::*;
use jackdaw_panels::WindowRegistry;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<WindowExtensionRegistry>()
        .init_resource::<WindowRegistry>()
        .init_resource::<crate::widgets::WidgetRegistry>()
        .init_resource::<crate::asset_kinds::AssetKinds>()
        .add_observer(crate::asset_kinds::cleanup_asset_kind_on_remove);
}

#[derive(Resource, Default)]
pub(crate) struct WindowExtensionRegistry {
    extensions: HashMap<Cow<'static, str>, Vec<Box<dyn Fn(&mut ChildSpawner) + Send + Sync>>>,
}

impl WindowExtensionRegistry {
    pub(crate) fn add(
        &mut self,
        panel_id: impl Into<Cow<'static, str>>,
        section: impl Fn(&mut ChildSpawner) + Send + Sync + 'static,
    ) {
        self.extensions
            .entry(panel_id.into())
            .or_default()
            .push(Box::new(section));
    }

    pub(crate) fn remove(&mut self, panel_id: &str, section_index: usize) {
        if let Some(sections) = self.extensions.get_mut(panel_id) {
            if section_index < sections.len() {
                let _old_build = sections.remove(section_index);
            }
            if sections.is_empty() {
                self.extensions.remove(panel_id);
            }
        }
    }

    pub(crate) fn get(
        &self,
        panel_id: &str,
    ) -> impl Iterator<Item = &(dyn Fn(&mut ChildSpawner) + Send + Sync)> + '_ {
        self.extensions
            .get(panel_id)
            .into_iter()
            .flatten()
            .map(|v| v.as_ref())
    }
}
