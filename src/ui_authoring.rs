//! Editor-side UI authoring facade.
//!
//! Widget definitions own metadata and a world callback. This module owns the
//! editor invariants shared by built-ins: reflected ECS entities, BSN
//! registration, shared selection, and command-history integration.

use std::{borrow::Cow, sync::Arc};

use bevy::{feathers::controls::ButtonVariant, prelude::*};
use jackdaw_api::{
    WidgetDefinition, WidgetInstantiateContext, WidgetPreviewState, WidgetProperty,
    WidgetPropertyKind, WidgetRegistry, WidgetSlot,
};
use jackdaw_commands::EditorCommand;
use jackdaw_feathers::icons::Icon;
use jackdaw_ui::{UiButton, UiCanvas, UiCheckbox, UiSlider, UiTextInput, UiToggle};

use crate::{
    commands::CommandHistory,
    selection::{Selected, Selection},
};

/// Failure returned by the generic UI authoring entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAuthoringError {
    UnknownDefinition(String),
    InvalidParent(Entity),
    Instantiate(String),
}

impl std::fmt::Display for UiAuthoringError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDefinition(id) => write!(formatter, "unknown UI widget definition `{id}`"),
            Self::InvalidParent(parent) => write!(formatter, "UI parent {parent} does not exist"),
            Self::Instantiate(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for UiAuthoringError {}

/// Deep editor facade for creating any registered UI widget.
pub struct UiAuthoring;

impl UiAuthoring {
    /// Instantiate a widget definition into the authored scene as one
    /// undoable step.
    pub fn instantiate(
        world: &mut World,
        id: &str,
        context: WidgetInstantiateContext,
    ) -> Result<Entity, UiAuthoringError> {
        let (entity, command) = Self::execute_instantiate(world, id, context)?;
        world
            .resource_mut::<CommandHistory>()
            .push_executed(command);
        Ok(entity)
    }

    /// Run a widget definition and return its executed command without
    /// recording it, so a caller can group several creations (a widget plus
    /// the canvas it needed) into one history entry.
    pub fn execute_instantiate(
        world: &mut World,
        id: &str,
        context: WidgetInstantiateContext,
    ) -> Result<(Entity, Box<dyn EditorCommand>), UiAuthoringError> {
        if let Some(parent) = context.parent
            && world.get_entity(parent).is_err()
        {
            return Err(UiAuthoringError::InvalidParent(parent));
        }
        let definition = world
            .get_resource::<WidgetRegistry>()
            .and_then(|registry| registry.get(id))
            .ok_or_else(|| UiAuthoringError::UnknownDefinition(id.to_string()))?;
        let mut command = InstantiateWidgetCommand {
            label: format!("Create {}", definition.name),
            definition,
            context,
            spawned: None,
            error: None,
        };
        command.execute(world);
        if let Some(error) = command.error.take() {
            return Err(UiAuthoringError::Instantiate(error));
        }
        let entity = command
            .spawned
            .expect("a successful widget callback must return a live entity");
        Ok((entity, Box::new(command)))
    }

    /// Record an authored layout edit that a live drag already applied to the
    /// ECS. One drag is one history entry, and the document learns the final
    /// value only once.
    pub fn push_layout_edit(world: &mut World, entity: Entity, before: Node, after: Node) {
        if before == after {
            return;
        }
        let command = SetUiNode {
            entity,
            before,
            after,
        };
        command.sync_after_external_execute(world);
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(command));
    }
}

/// Undoable edit of one authored UI [`Node`].
pub struct SetUiNode {
    pub entity: Entity,
    pub before: Node,
    pub after: Node,
}

impl SetUiNode {
    fn apply(&self, world: &mut World, value: &Node) {
        if let Some(mut node) = world.get_mut::<Node>(self.entity) {
            *node = value.clone();
        }
        crate::commands::sync_component_to_ast::<Node>(
            world,
            self.entity,
            "bevy_ui::ui_node::Node",
            value,
        );
    }
}

impl EditorCommand for SetUiNode {
    fn execute(&mut self, world: &mut World) {
        let after = self.after.clone();
        self.apply(world, &after);
    }

    fn undo(&mut self, world: &mut World) {
        let before = self.before.clone();
        self.apply(world, &before);
    }

    fn description(&self) -> &str {
        "Edit UI layout"
    }

    fn sync_after_external_execute(&self, world: &mut World) {
        let after = self.after.clone();
        crate::commands::sync_component_to_ast::<Node>(
            world,
            self.entity,
            "bevy_ui::ui_node::Node",
            &after,
        );
    }
}

struct InstantiateWidgetCommand {
    label: String,
    definition: Arc<WidgetDefinition>,
    context: WidgetInstantiateContext,
    spawned: Option<Entity>,
    error: Option<String>,
}

impl EditorCommand for InstantiateWidgetCommand {
    fn execute(&mut self, world: &mut World) {
        match (self.definition.instantiate)(world, self.context) {
            Ok(entity) if world.get_entity(entity).is_ok() => {
                register_authored_subtree(world, entity);
                select_in_world(world, entity);
                self.spawned = Some(entity);
                self.error = None;
            }
            Ok(entity) => {
                self.spawned = None;
                self.error = Some(format!(
                    "widget `{}` returned missing entity {entity}",
                    self.definition.id
                ));
            }
            Err(error) => {
                self.spawned = None;
                self.error = Some(error);
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        let Some(entity) = self.spawned.take() else {
            return;
        };
        world
            .resource_mut::<jackdaw_bsn::SceneBsnAst>()
            .remove_entity_node(entity);
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

fn register_authored_subtree(world: &mut World, root: Entity) {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if world.get::<jackdaw_ui::UiGeneratedPart>(entity).is_some() {
            continue;
        }
        let children = world
            .get::<Children>(entity)
            .map(|children| children.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        crate::scene_io::register_entity_in_ast(world, entity);
        stack.extend(children.into_iter().rev());
    }
}

/// Built-in root Canvas definition.
pub fn canvas_definition() -> WidgetDefinition {
    WidgetDefinition::new("layout.canvas", "Canvas", "Layout", |world, _context| {
        // `UiCanvas` requires a full-target absolute `Node`; authoring an
        // explicit one here would freeze that policy into every document.
        Ok(world
            .spawn((Name::new("UI Canvas"), UiCanvas::default()))
            .id())
    })
    .with_icon(Icon::LayoutTemplate)
    .with_property(property(
        "reference_size",
        "Reference Size",
        WidgetPropertyKind::Number,
    ))
    .with_property(property(
        "scale_mode",
        "Scale Mode",
        WidgetPropertyKind::Enum,
    ))
    .with_property(property("order", "Order", WidgetPropertyKind::Number))
    .with_property(property("enabled", "Enabled", WidgetPropertyKind::Bool))
    .with_slot(content_slot())
}

/// Built-in Feathers Button definition.
pub fn button_definition() -> WidgetDefinition {
    WidgetDefinition::new("feathers.button", "Button", "Controls", |world, context| {
        Ok(spawn_under(
            world,
            context.parent,
            (
                Name::new("Button"),
                UiButton {
                    label: "Button".into(),
                    variant: ButtonVariant::Normal,
                    disabled: false,
                },
                Node {
                    min_width: px(96),
                    min_height: px(32),
                    padding: UiRect::horizontal(px(10)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
            ),
        ))
    })
    .with_icon(Icon::MousePointer)
    .with_property(property("label", "Label", WidgetPropertyKind::String))
    .with_property(property("variant", "Variant", WidgetPropertyKind::Enum))
    .with_property(property("disabled", "Disabled", WidgetPropertyKind::Bool))
    .with_preview_state(preview("normal", "Normal"))
    .with_preview_state(preview("hovered", "Hovered"))
    .with_preview_state(preview("pressed", "Pressed"))
    .with_preview_state(preview("disabled", "Disabled"))
}

pub fn checkbox_definition() -> WidgetDefinition {
    WidgetDefinition::new(
        "feathers.checkbox",
        "Checkbox",
        "Controls",
        |world, context| {
            Ok(spawn_under(
                world,
                context.parent,
                (
                    Name::new("Checkbox"),
                    UiCheckbox {
                        label: "Checkbox".into(),
                        ..default()
                    },
                    Node {
                        min_height: px(28),
                        align_items: AlignItems::Center,
                        column_gap: px(6),
                        ..default()
                    },
                ),
            ))
        },
    )
    .with_icon(Icon::Check)
    .with_property(property("label", "Label", WidgetPropertyKind::String))
    .with_property(property("checked", "Checked", WidgetPropertyKind::Bool))
    .with_property(property("disabled", "Disabled", WidgetPropertyKind::Bool))
    .with_preview_state(preview("normal", "Normal"))
    .with_preview_state(preview("checked", "Checked"))
    .with_preview_state(preview("disabled", "Disabled"))
}

pub fn toggle_definition() -> WidgetDefinition {
    WidgetDefinition::new(
        "feathers.toggle",
        "Toggle Switch",
        "Controls",
        |world, context| {
            Ok(spawn_under(
                world,
                context.parent,
                (
                    Name::new("Toggle Switch"),
                    UiToggle::default(),
                    Node {
                        width: px(44),
                        height: px(24),
                        ..default()
                    },
                ),
            ))
        },
    )
    .with_icon(Icon::ToggleLeft)
    .with_property(property("checked", "Checked", WidgetPropertyKind::Bool))
    .with_property(property("disabled", "Disabled", WidgetPropertyKind::Bool))
    .with_preview_state(preview("normal", "Normal"))
    .with_preview_state(preview("checked", "Checked"))
    .with_preview_state(preview("disabled", "Disabled"))
}

pub fn slider_definition() -> WidgetDefinition {
    WidgetDefinition::new("feathers.slider", "Slider", "Controls", |world, context| {
        Ok(spawn_under(
            world,
            context.parent,
            (
                Name::new("Slider"),
                UiSlider::default(),
                Node {
                    width: px(180),
                    height: px(32),
                    padding: UiRect::horizontal(px(8)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
            ),
        ))
    })
    .with_icon(Icon::SlidersHorizontal)
    .with_property(property("value", "Value", WidgetPropertyKind::Number))
    .with_property(property("min", "Minimum", WidgetPropertyKind::Number))
    .with_property(property("max", "Maximum", WidgetPropertyKind::Number))
    .with_property(property("disabled", "Disabled", WidgetPropertyKind::Bool))
    .with_preview_state(preview("normal", "Normal"))
    .with_preview_state(preview("disabled", "Disabled"))
}

pub fn text_input_definition() -> WidgetDefinition {
    WidgetDefinition::new(
        "feathers.text_input",
        "Text Input",
        "Controls",
        |world, context| {
            Ok(spawn_under(
                world,
                context.parent,
                (
                    Name::new("Text Input"),
                    UiTextInput {
                        placeholder: "Enter text".into(),
                        ..default()
                    },
                    Node {
                        width: px(200),
                        min_height: px(32),
                        padding: UiRect::horizontal(px(8)),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                ),
            ))
        },
    )
    .with_icon(Icon::TextCursorInput)
    .with_property(property("value", "Value", WidgetPropertyKind::String))
    .with_property(property(
        "placeholder",
        "Placeholder",
        WidgetPropertyKind::String,
    ))
    .with_property(property(
        "max_characters",
        "Max Characters",
        WidgetPropertyKind::Number,
    ))
    .with_property(property("disabled", "Disabled", WidgetPropertyKind::Bool))
    .with_preview_state(preview("normal", "Normal"))
    .with_preview_state(preview("focused", "Focused"))
    .with_preview_state(preview("disabled", "Disabled"))
}

pub fn panel_definition() -> WidgetDefinition {
    layout_definition(
        "layout.panel",
        "Panel",
        Icon::PanelTop,
        FlexDirection::Column,
        Some(Color::srgb(0.10, 0.11, 0.14)),
    )
}

pub fn row_definition() -> WidgetDefinition {
    layout_definition(
        "layout.row",
        "Row",
        Icon::PanelsTopLeft,
        FlexDirection::Row,
        None,
    )
}

pub fn column_definition() -> WidgetDefinition {
    layout_definition(
        "layout.column",
        "Column",
        Icon::LayoutPanelTop,
        FlexDirection::Column,
        None,
    )
}

pub fn label_definition() -> WidgetDefinition {
    WidgetDefinition::new("display.label", "Label", "Display", |world, context| {
        Ok(spawn_under(
            world,
            context.parent,
            (
                Name::new("Label"),
                Text::new("Label"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ),
        ))
    })
    .with_icon(Icon::TextCursorInput)
    .with_property(property("text", "Text", WidgetPropertyKind::String))
    .with_property(property(
        "font_size",
        "Font Size",
        WidgetPropertyKind::Number,
    ))
    .with_property(property("color", "Color", WidgetPropertyKind::Color))
}

fn layout_definition(
    id: &'static str,
    name: &'static str,
    icon: Icon,
    direction: FlexDirection,
    background: Option<Color>,
) -> WidgetDefinition {
    WidgetDefinition::new(id, name, "Layout", move |world, context| {
        Ok(spawn_under(
            world,
            context.parent,
            (
                Name::new(name),
                Node {
                    min_width: px(96),
                    min_height: px(48),
                    flex_direction: direction,
                    align_items: AlignItems::Stretch,
                    row_gap: px(6),
                    column_gap: px(6),
                    padding: UiRect::all(px(8)),
                    ..default()
                },
                BackgroundColor(background.unwrap_or(Color::NONE)),
                Visibility::default(),
            ),
        ))
    })
    .with_icon(icon)
    .with_property(property("layout", "Layout", WidgetPropertyKind::Enum))
    .with_property(property(
        "background",
        "Background",
        WidgetPropertyKind::Color,
    ))
    .with_slot(content_slot())
}

fn spawn_under(world: &mut World, parent: Option<Entity>, bundle: impl Bundle) -> Entity {
    let mut entity = world.spawn(bundle);
    if let Some(parent) = parent {
        entity.insert(ChildOf(parent));
    }
    entity.id()
}

fn select_in_world(world: &mut World, entity: Entity) {
    let selected = std::mem::take(&mut world.resource_mut::<Selection>().entities);
    for old in selected {
        if old != entity
            && let Ok(mut old) = world.get_entity_mut(old)
        {
            old.remove::<Selected>();
        }
    }
    world.resource_mut::<Selection>().entities = vec![entity];
    world.entity_mut(entity).insert(Selected);
}

fn property(id: &'static str, label: &'static str, kind: WidgetPropertyKind) -> WidgetProperty {
    WidgetProperty {
        id: Cow::Borrowed(id),
        label: Cow::Borrowed(label),
        kind,
    }
}

fn preview(id: &'static str, label: &'static str) -> WidgetPreviewState {
    WidgetPreviewState {
        id: Cow::Borrowed(id),
        label: Cow::Borrowed(label),
    }
}

fn content_slot() -> WidgetSlot {
    WidgetSlot {
        id: Cow::Borrowed("content"),
        label: Cow::Borrowed("Content"),
        accepts_multiple: true,
    }
}
