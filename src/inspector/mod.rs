mod brush_display;
mod component_display;
mod component_picker;
mod custom_props_display;
mod material_display;
mod reflect_fields;

use crate::EditorEntity;
use std::any::TypeId;

use bevy::prelude::*;

const MAX_REFLECT_DEPTH: usize = 4;

/// Extract a human-readable module group name from a module path.
/// e.g., "bevy_pbr::material" -> "Render", "bevy_transform" -> "Transform"
fn extract_module_group(module_path: Option<&str>) -> String {
    let Some(path) = module_path else {
        return "Other".to_string();
    };
    // Get first path segment
    let first = path.split("::").next().unwrap_or(path);
    // Strip "bevy_" prefix and capitalize
    let name = first.strip_prefix("bevy_").unwrap_or(first);
    // Map common module names to cleaner labels
    match name {
        "pbr" | "core_pipeline" => "Render".to_string(),
        "render" => "Render".to_string(),
        "transform" => "Transform".to_string(),
        "ecs" => "ECS".to_string(),
        "hierarchy" => "Hierarchy".to_string(),
        "window" | "winit" => "Window".to_string(),
        "input" | "picking" => "Input".to_string(),
        "asset" => "Asset".to_string(),
        "scene" => "Scene".to_string(),
        "gltf" => "GLTF".to_string(),
        "ui" => "UI".to_string(),
        "text" => "Text".to_string(),
        "audio" => "Audio".to_string(),
        "animation" => "Animation".to_string(),
        "sprite" => "Sprite".to_string(),
        _ => {
            // Capitalize first letter
            let mut chars = name.chars();
            match chars.next() {
                None => "Other".to_string(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        }
    }
}

/// Trait for annotating components with editor metadata.
/// Implement this on your components and add `EditorMeta` to `#[reflect(...)]`
/// to give them descriptions and categories in the component picker.
pub trait EditorMeta {
    fn description() -> &'static str {
        ""
    }
    fn category() -> &'static str {
        ""
    }
}

/// Type data for [`EditorMeta`]. Captured at registration time via `FromType<T>`,
/// so metadata is available without a component instance.
#[derive(Clone)]
pub struct ReflectEditorMeta {
    pub description: &'static str,
    pub category: &'static str,
}

impl<T: EditorMeta> bevy::reflect::FromType<T> for ReflectEditorMeta {
    fn from_type() -> Self {
        ReflectEditorMeta {
            description: T::description(),
            category: T::category(),
        }
    }
}

#[reflect_trait]
pub trait Displayable {
    fn display(&self, entity: &mut EntityCommands, source: Entity);
}

/// Marker for Name field text_edit inputs in the inspector.
#[derive(Component)]
struct NameFieldInput(Entity);

impl Displayable for Name {
    fn display(&self, entity: &mut EntityCommands, source: Entity) {
        entity
            .insert(jackdaw_feathers::text_edit::text_edit(
                jackdaw_feathers::text_edit::TextEditProps::default()
                    .with_placeholder("Name...")
                    .with_default_value(self.to_string())
                    .allow_empty(),
            ))
            .insert(NameFieldInput(source));
    }
}

#[derive(Component)]
#[require(EditorEntity)]
pub struct Inspector;

pub struct InspectorPlugin;

impl Plugin for InspectorPlugin {
    fn build(&self, app: &mut App) {
        app.register_type_data::<Name, ReflectDisplayable>()
            .add_observer(component_display::remove_component_displays)
            .add_observer(component_display::add_component_displays)
            .add_observer(component_display::on_inspector_dirty)
            .add_observer(component_picker::on_add_component_button_click)
            .add_observer(reflect_fields::on_checkbox_commit)
            .add_observer(reflect_fields::on_text_edit_commit)
            .add_observer(custom_props_display::on_custom_property_checkbox_commit)
            .add_observer(custom_props_display::on_custom_property_text_commit)
            .add_observer(brush_display::handle_clear_texture)
            .add_observer(brush_display::handle_clear_material)
            .add_observer(brush_display::handle_apply_texture_to_all)
            .add_observer(brush_display::handle_uv_scale_preset)
            .add_observer(brush_display::on_brush_face_text_commit)
            .add_observer(on_name_field_commit)
            .add_observer(material_display::on_material_text_commit)
            .add_systems(
                Update,
                (
                    reflect_fields::refresh_inspector_fields,
                    component_picker::filter_component_picker,
                    brush_display::update_brush_face_properties,
                    component_display::filter_inspector_components,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// Handle TextEditCommitEvent for Name field inputs.
fn on_name_field_commit(
    event: On<jackdaw_feathers::text_edit::TextEditCommitEvent>,
    name_inputs: Query<&NameFieldInput>,
    child_of_query: Query<&ChildOf>,
    mut names: Query<&mut Name>,
) {
    // Walk up from the committed entity to find a NameFieldInput
    let mut current = event.entity;
    let mut source = None;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        if let Ok(name_input) = name_inputs.get(child_of.parent()) {
            source = Some(name_input.0);
            break;
        }
        current = child_of.parent();
    }

    let Some(source_entity) = source else {
        return;
    };

    if let Ok(mut name) = names.get_mut(source_entity) {
        *name = Name::new(event.text.clone());
    }
}

#[derive(Component)]
pub struct ComponentDisplay;

#[derive(Component)]
pub(super) struct ComponentDisplayBody;

#[derive(Component)]
pub(super) struct AddComponentButton;

/// Marker for the component picker panel
#[derive(Component)]
pub(super) struct ComponentPicker;

/// Marker for the search input in the component picker
#[derive(Component)]
pub(super) struct ComponentPickerSearch;

/// A selectable component entry in the picker list
#[derive(Component)]
pub(super) struct ComponentPickerEntry {
    pub(super) short_name: String,
    pub(super) module_path: String,
    pub(super) category: String,
    pub(super) description: String,
}

/// Section header in the component picker (e.g. "Game", "Bevy", or a custom category)
#[derive(Component)]
pub(super) struct ComponentPickerSectionHeader {
    pub(super) group: String,
}

/// Marker for the search input in the inspector.
#[derive(Component)]
pub(super) struct InspectorSearch;

/// Marker for the collapse-all / expand-all toggle button.
#[derive(Component)]
pub(super) struct CollapseAllButton;

/// Stores the component short name on a `ComponentDisplay` for search filtering.
#[derive(Component)]
pub(super) struct ComponentName(pub(super) String);

/// Marker for a group section (the `CollapsibleSection` that wraps a group header + body).
#[derive(Component)]
pub(super) struct InspectorGroupSection;

/// Tracks which inspector field entity maps to which source entity + component + field path.
#[derive(Component)]
pub(super) struct FieldBinding {
    pub(super) source_entity: Entity,
    pub(super) component_type_id: TypeId,
    pub(super) field_path: String,
}

/// Container for brush face properties (texture, UV, etc). Populated dynamically.
#[derive(Component)]
pub(super) struct BrushFacePropsContainer;

/// Binding for a brush face UV field.
#[derive(Component)]
#[allow(dead_code)]
pub(super) struct BrushFaceFieldBinding {
    pub(super) field: BrushFaceField,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BrushFaceField {
    UvOffsetX,
    UvOffsetY,
    UvScaleX,
    UvScaleY,
    UvRotation,
}

/// Tracks a custom property field binding (property name + source entity).
#[derive(Component)]
pub(super) struct CustomPropertyBinding {
    pub(super) source_entity: Entity,
    pub(super) property_name: String,
}

/// Marker for the "Add Property" row container.
#[derive(Component)]
pub(super) struct CustomPropertyAddRow;

/// Marker for the type selector ComboBox in the "Add Property" row.
#[derive(Component)]
pub(super) struct CustomPropertyTypeSelector;

/// Marker for the property name text input in the "Add Property" row.
#[derive(Component)]
pub(super) struct CustomPropertyNameInput;

// --- Axis colors for Vec3/Vec2 fields ---
pub(super) const AXIS_X_COLOR: Color = Color::srgb(0.8, 0.3, 0.3);
pub(super) const AXIS_Y_COLOR: Color = Color::srgb(0.3, 0.7, 0.3);
pub(super) const AXIS_Z_COLOR: Color = Color::srgb(0.3, 0.5, 0.8);

/// Stores the entity currently being inspected.
#[derive(Component)]
pub(super) struct InspectorTarget(pub Entity);

/// Marker inserted on a selected entity to signal the inspector needs rebuilding.
#[derive(Component)]
pub(super) struct InspectorDirty;

/// Force inspector rebuild by marking the source entity dirty.
pub(super) fn rebuild_inspector(world: &mut World, source_entity: Entity) {
    world.entity_mut(source_entity).insert(InspectorDirty);
}
