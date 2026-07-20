use bevy::prelude::*;

// Re-export types from jackdaw_scene_types.
pub use jackdaw_scene_types::{CustomProperties, PropertyValue};

pub struct CustomPropertiesPlugin;

impl Plugin for CustomPropertiesPlugin {
    fn build(&self, _app: &mut App) {
        // Note: Type registration is handled by SceneTypesPlugin
    }
}

/// Undo command that stores old/new snapshots of the entire `CustomProperties` component.
pub struct SetCustomProperties {
    pub entity: Entity,
    pub old_properties: CustomProperties,
    pub new_properties: CustomProperties,
}

impl crate::commands::EditorCommand for SetCustomProperties {
    fn execute(&mut self, world: &mut World) {
        if let Some(mut cp) = world.get_mut::<CustomProperties>(self.entity) {
            *cp = self.new_properties.clone();
        }
        sync_custom_props_to_ast(world, self.entity, &self.new_properties);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(mut cp) = world.get_mut::<CustomProperties>(self.entity) {
            *cp = self.old_properties.clone();
        }
        sync_custom_props_to_ast(world, self.entity, &self.old_properties);
    }

    fn description(&self) -> &str {
        "Set custom properties"
    }
}

fn sync_custom_props_to_ast(world: &mut World, entity: Entity, props: &CustomProperties) {
    crate::commands::sync_component_to_ast(
        world,
        entity,
        "jackdaw_scene_types::types::custom_properties::CustomProperties",
        props,
    );
}
