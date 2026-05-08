pub(crate) mod anim_diamond;
mod brush_display;
pub(crate) mod component_display;
pub mod component_picker;
pub(crate) mod component_tooltip;
mod custom_props_display;
mod material_display;
pub(crate) mod ops;
pub(crate) mod physics_display;
pub(crate) mod reflect_fields;

use crate::EditorEntity;

use bevy::ecs::archetype::ArchetypeId;
use bevy::prelude::*;

const MAX_REFLECT_DEPTH: usize = 4;

/// Extract a human-readable module group name from a module path.
/// e.g., "`bevy_pbr::material`" -> "Render", "`bevy_transform`" -> "Transform"
fn extract_module_group(module_path: Option<&str>) -> String {
    let Some(path) = module_path else {
        return "Other".to_string();
    };
    // Get first path segment
    let first = path.split("::").next().unwrap_or(path);
    // Group jackdaw's avian wrapper alongside avian3d's own types
    // so AvianCollider sits in the same inspector section as
    // RigidBody, Sensor, etc. instead of getting its own
    // `Jackdaw_avian_integration` heading.
    if first == "avian3d" || first == "jackdaw_avian_integration" {
        return "Avian3d".to_string();
    }
    // Strip "bevy_" prefix and capitalize
    let name = first.strip_prefix("bevy_").unwrap_or(first);
    // Map common module names to cleaner labels
    match name {
        "pbr" | "core_pipeline" | "render" => "Render".to_string(),
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

// Editor display metadata as Bevy reflect custom attributes.
// Newtypes live in `jackdaw_jsn`, re-exported here.
pub use jackdaw_runtime::{EditorCategory, EditorDescription, EditorHidden, SkipSerialization};

#[reflect_trait]
pub trait Displayable {
    fn display(&self, entity: &mut EntityCommands, source: Entity);
}

/// Marker for Name field `text_edit` inputs in the inspector.
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
        let mut denylist = component_picker::PickerDenylist::default();
        component_picker::populate_avian_picker_denylist(&mut denylist);
        app.insert_resource(denylist);

        app.register_type_data::<Name, ReflectDisplayable>()
            .add_plugins(component_tooltip::plugin)
            .add_observer(component_display::remove_component_displays)
            .add_observer(component_display::add_component_displays)
            .add_observer(component_display::on_inspector_dirty)
            .add_observer(component_picker::on_add_component_button_click)
            .add_observer(reflect_fields::on_checkbox_commit)
            .add_observer(reflect_fields::on_text_edit_commit)
            .add_observer(custom_props_display::on_custom_property_checkbox_commit)
            .add_observer(custom_props_display::on_custom_property_text_commit)
            .add_observer(brush_display::on_brush_face_text_commit)
            .add_observer(on_name_field_commit)
            .add_observer(material_display::on_material_text_commit)
            .add_observer(anim_diamond::on_diamond_click)
            .add_systems(
                Update,
                (
                    reflect_fields::refresh_inspector_fields,
                    reflect_fields::refresh_enum_variants,
                    brush_display::update_brush_face_properties,
                    component_display::filter_inspector_components,
                    anim_diamond::decorate_animatable_fields,
                    anim_diamond::update_anim_diamond_highlights,
                    refresh_name_field,
                    flag_inspector_dirty_on_archetype_change,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// Keep the Name field's text input in sync with the underlying Name component.
/// Needed so that undo/redo and external edits are reflected in the UI without
/// having to deselect and reselect the entity.
fn refresh_name_field(world: &mut World) {
    use bevy::input_focus::InputFocus;
    use jackdaw_feathers::text_edit::{
        TextEditDragging, TextEditValue, TextInputQueue, set_text_input_value,
    };

    // Gather (outer, source_entity, current_value) tuples.
    let mut targets: Vec<(Entity, Entity, String)> = Vec::new();
    let mut query = world.query::<(Entity, &NameFieldInput, &TextEditValue)>();
    for (outer, input, value) in query.iter(world) {
        targets.push((outer, input.0, value.0.clone()));
    }
    if targets.is_empty() {
        return;
    }

    let input_focus = world.resource::<InputFocus>().0;

    for (outer, source, current) in targets {
        let Some(name) = world.get::<Name>(source) else {
            continue;
        };
        let expected = name.as_str().to_string();
        if current == expected {
            continue;
        }
        // Walk outer -> children (and grandchildren) to find the wrapper.
        let Some((wrapper_entity, inner_entity)) = find_text_edit_entities_local(world, outer)
        else {
            continue;
        };
        // Skip if the user is currently dragging or typing in this field.
        if world.get::<TextEditDragging>(wrapper_entity).is_some() {
            continue;
        }
        if input_focus == Some(inner_entity) {
            continue;
        }
        if let Some(mut queue) = world.get_mut::<TextInputQueue>(inner_entity) {
            set_text_input_value(&mut queue, expected);
        }
    }
}

fn find_text_edit_entities_local(world: &World, outer_entity: Entity) -> Option<(Entity, Entity)> {
    use jackdaw_feathers::text_edit::TextEditWrapper;
    let children = world.get::<Children>(outer_entity)?;
    for child in children.iter() {
        if let Some(wrapper) = world.get::<TextEditWrapper>(child) {
            return Some((child, wrapper.0));
        }
        if let Some(grandchildren) = world.get::<Children>(child) {
            for gc in grandchildren.iter() {
                if let Some(wrapper) = world.get::<TextEditWrapper>(gc) {
                    return Some((gc, wrapper.0));
                }
            }
        }
    }
    None
}

/// Handle `TextEditCommitEvent` for Name field inputs.
/// Pushes a `SetJsnField` command so the rename can be undone.
fn on_name_field_commit(
    event: On<jackdaw_feathers::text_edit::TextEditCommitEvent>,
    name_inputs: Query<&NameFieldInput>,
    child_of_query: Query<&ChildOf>,
    names: Query<&Name>,
    mut commands: Commands,
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

    let old_name = names
        .get(source_entity)
        .map(|n| n.as_str().to_string())
        .unwrap_or_default();
    let new_name = event.text.clone();

    if old_name == new_name {
        return;
    }

    commands.queue(move |world: &mut World| {
        let cmd = crate::commands::SetJsnField {
            entity: source_entity,
            type_path: "bevy_ecs::name::Name".to_string(),
            field_path: String::new(),
            old_value: serde_json::Value::String(old_name),
            new_value: serde_json::Value::String(new_name),
            was_derived: false,
        };
        let mut cmd: Box<dyn jackdaw_commands::EditorCommand> = Box::new(cmd);
        cmd.execute(world);
        world
            .resource_mut::<crate::commands::CommandHistory>()
            .push_executed(cmd);
    });
}

#[derive(Component)]
pub struct ComponentDisplay;

#[derive(Component)]
pub(super) struct ComponentDisplayBody;

#[derive(Component)]
pub struct AddComponentButton;

/// The component picker panel
#[derive(Component)]
pub(super) struct ComponentPicker(pub Entity);

/// Marker for the search input in the inspector.
#[derive(Component)]
pub(super) struct InspectorSearch;

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
    pub(super) type_path: String,
    pub(super) field_path: String,
}

/// Marker on the row-level container of a labeled inspector field,
/// carrying the **root** `(component_type_path, field_path)` for the
/// row regardless of how many scalar inputs sit inside it.
///
/// For scalar fields (e.g. `f32 intensity`) the row has one
/// `FieldBinding` with the same path as this marker. For composite
/// fields like `Vec3 translation` the row contains three axis
/// `FieldBinding`s with paths `translation.x` / `.y` / `.z`, but
/// there's still only one `InspectorFieldRow` on the outer column ;
/// so per-field decorations (like the animation keyframe diamond)
/// can hang off this marker without being duplicated per axis.
#[derive(Component)]
pub(super) struct InspectorFieldRow {
    pub(super) source_entity: Entity,
    pub(super) type_path: String,
    pub(super) field_path: String,
}

/// Marker on the container that holds an enum combobox + its current variant's
/// field rows. `refresh_enum_variants` polls these and rebuilds the subtree
/// (combobox + field rows) in place whenever the ECS variant changes.
#[derive(Component)]
pub(super) struct EnumVariantHost {
    pub(super) source_entity: Entity,
    pub(super) type_path: String,
    pub(super) field_path: String,
    pub(super) depth: usize,
    pub(super) current_variant: String,
}

/// Container for brush face properties (texture, UV, etc). Populated dynamically.
#[derive(Component)]
pub(super) struct BrushFacePropsContainer;

/// Binding for a brush face UV field.
#[derive(Component)]
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

/// Marker for the type selector `ComboBox` in the "Add Property" row.
#[derive(Component)]
pub(super) struct CustomPropertyTypeSelector;

/// Marker for the property name text input in the "Add Property" row.
#[derive(Component)]
pub(super) struct CustomPropertyNameInput;

/// Stores the entity currently being inspected.
#[derive(Component)]
pub(super) struct InspectorTarget(pub Entity);

/// Marker inserted on a selected entity to signal the inspector needs rebuilding.
#[derive(Component)]
pub(crate) struct InspectorDirty;

/// Force inspector rebuild by marking the source entity dirty.
pub(super) fn rebuild_inspector(world: &mut World, source_entity: Entity) {
    world.entity_mut(source_entity).insert(InspectorDirty);
}

/// Rebuild the inspector when the displayed entity's archetype changes.
///
/// Some component picks pull in extra components asynchronously (e.g.
/// `AvianCollider` triggers our `PostUpdate` sync that builds a real
/// `Collider`, after which avian's own systems insert `Position`,
/// `Rotation`, `ColliderAabb`, mass aggregates, etc.). Without this
/// watcher the panel built by `add_component_displays` keeps showing
/// only the originally-added component until the user reselects.
///
/// Comparing the cached `ArchetypeId` to the live one is O(1) and only
/// fires on real archetype transitions, so the rebuild cost matches
/// the user's edit rate, not the framerate.
fn flag_inspector_dirty_on_archetype_change(
    world: &mut World,
    mut last: Local<Option<(Entity, ArchetypeId)>>,
) {
    let mut targets = world.query_filtered::<&InspectorTarget, With<Inspector>>();
    let target = targets.iter(world).next().map(|t| t.0);

    let Some(target) = target else {
        *last = None;
        return;
    };

    let current = world.get_entity(target).ok().map(|e| e.archetype().id());

    let stored = last.as_ref().filter(|(e, _)| *e == target).map(|(_, a)| *a);

    if stored == current {
        return;
    }

    if stored.is_some()
        && current.is_some()
        && let Ok(mut e) = world.get_entity_mut(target)
    {
        e.insert(InspectorDirty);
    }

    *last = current.map(|arch| (target, arch));
}
