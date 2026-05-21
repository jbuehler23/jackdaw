use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_picking::hover::Hovered;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;
use lucide_icons::Icon;

use crate::button::{ButtonClickEvent, ButtonProps, ButtonVariant, EditorButton, button};
use crate::combobox::{ComboBoxChangeEvent, ComboBoxOptionData, combobox_with_selected};
use crate::icons::{EditorFont, IconFont};
use crate::inspector_field::path_to_label;
use crate::popover::{
    EditorPopover, PopoverHeaderProps, PopoverPlacement, PopoverProps, PopoverTracker,
    activate_trigger, deactivate_trigger, popover, popover_header,
};
use crate::tokens::{BORDER_COLOR, TEXT_MUTED_COLOR, TEXT_SIZE_SM};
use crate::utils::is_descendant_of;

pub fn plugin(app: &mut App) {
    app.add_observer(handle_variant_edit_click)
        .add_observer(handle_variant_combobox_change)
        .add_systems(Update, (setup_variant_edit, sync_variant_edit_button));
}

#[derive(Component)]
pub struct EditorVariantEdit;

#[derive(Clone)]
pub struct VariantDefinition {
    pub name: String,
    pub icon: Option<Icon>,
}

impl VariantDefinition {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            icon: None,
        }
    }

    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

#[derive(Component, Clone)]
pub struct VariantEditConfig {
    pub path: String,
    pub label: Option<String>,
    pub popover_title: Option<String>,
    pub variants: Vec<VariantDefinition>,
    pub selected_index: usize,
    pub popover_width: Option<f32>,
    initialized: bool,
}

#[derive(Component)]
struct VariantEditPopover;

#[derive(Component)]
pub struct VariantFieldsContainer(pub Entity);

#[derive(Component)]
pub struct VariantComboBox(pub Entity);

#[derive(Component, Default)]
struct VariantEditState {
    last_synced_index: Option<usize>,
}

pub struct VariantEditProps {
    pub path: String,
    pub label: Option<String>,
    pub popover_title: Option<String>,
    pub variants: Vec<VariantDefinition>,
    pub selected_index: usize,
    pub popover_width: Option<f32>,
}

impl VariantEditProps {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            label: None,
            popover_title: None,
            variants: Vec::new(),
            selected_index: 0,
            popover_width: Some(256.0),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_popover_title(mut self, title: impl Into<String>) -> Self {
        self.popover_title = Some(title.into());
        self
    }

    pub fn with_variants(mut self, variants: Vec<VariantDefinition>) -> Self {
        self.variants = variants;
        self
    }

    pub fn with_selected(mut self, index: usize) -> Self {
        self.selected_index = index;
        self
    }

    pub fn with_popover_width(mut self, width: f32) -> Self {
        self.popover_width = Some(width);
        self
    }
}

pub fn variant_edit(props: VariantEditProps) -> impl Bundle {
    let VariantEditProps {
        path,
        label,
        popover_title,
        variants,
        selected_index,
        popover_width,
    } = props;

    (
        EditorVariantEdit,
        VariantEditConfig {
            path,
            label,
            popover_title,
            variants,
            selected_index,
            popover_width,
            initialized: false,
        },
        VariantEditState::default(),
        PopoverTracker::default(),
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(3.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: px(0.0),
            ..default()
        },
    )
}

#[derive(Component)]
struct VariantEditButton;

fn setup_variant_edit(
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    mut configs: Query<(Entity, &mut VariantEditConfig)>,
) {
    let font = editor_font.0.clone();

    for (entity, mut config) in &mut configs {
        if config.initialized {
            continue;
        }
        config.initialized = true;

        let label = config
            .label
            .clone()
            .unwrap_or_else(|| path_to_label(&config.path));

        let label_entity = commands
            .spawn((
                Text::new(&label),
                TextFont {
                    font: font.clone(),
                    font_size: TEXT_SIZE_SM,
                    weight: FontWeight::MEDIUM,
                    ..default()
                },
                TextColor(TEXT_MUTED_COLOR.into()),
            ))
            .id();
        crate::utils::attach_or_despawn(&mut commands, entity, label_entity);

        let selected_variant = config.variants.get(config.selected_index);
        let value = selected_variant
            .map(|v| path_to_label(&v.name))
            .unwrap_or_default();

        let button_props = ButtonProps::new(&value)
            .align_left()
            .with_right_icon(Icon::ChevronDown);

        let button_entity = commands
            .spawn((VariantEditButton, button(button_props)))
            .id();

        crate::utils::attach_or_despawn(&mut commands, entity, button_entity);
    }
}

fn sync_variant_edit_button(
    mut variant_edits: Query<
        (&VariantEditConfig, &mut VariantEditState, &Children),
        With<EditorVariantEdit>,
    >,
    children_query: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    for (config, mut state, children) in &mut variant_edits {
        if state.last_synced_index == Some(config.selected_index) {
            continue;
        }

        let Some(selected_variant) = config.variants.get(config.selected_index) else {
            continue;
        };

        let Some(&button_entity) = children.last() else {
            continue;
        };
        let Ok(button_children) = children_query.get(button_entity) else {
            continue;
        };

        let mut text_updated = false;
        for child in button_children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                **text = path_to_label(&selected_variant.name);
                text_updated = true;
                break;
            }
        }

        if text_updated {
            state.last_synced_index = Some(config.selected_index);
        }
    }
}

fn handle_variant_edit_click(
    trigger: On<ButtonClickEvent>,
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    buttons: Query<&ChildOf, With<EditorButton>>,
    variant_edit_buttons: Query<&ChildOf, With<VariantEditButton>>,
    variant_edits: Query<(Entity, &VariantEditConfig, &Children), With<EditorVariantEdit>>,
    mut trackers: Query<&mut PopoverTracker>,
    existing_popovers: Query<Entity, With<VariantEditPopover>>,
    all_popovers: Query<Entity, With<EditorPopover>>,
    mut button_styles: Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
    parents: Query<&ChildOf>,
) {
    let Ok(child_of) = buttons.get(trigger.entity) else {
        return;
    };

    let variant_edit_entity =
        if let Ok(button_child_of) = variant_edit_buttons.get(child_of.parent()) {
            button_child_of.parent()
        } else {
            child_of.parent()
        };

    let Ok((entity, config, children)) = variant_edits.get(variant_edit_entity) else {
        return;
    };

    let Ok(mut tracker) = trackers.get_mut(entity) else {
        return;
    };

    let button_entity = children.last().copied();

    if let Some(popover_entity) = tracker.popover
        && existing_popovers.get(popover_entity).is_ok()
    {
        commands.entity(popover_entity).try_despawn();
        tracker.popover = None;
        if let Some(btn) = button_entity {
            deactivate_trigger(btn, &mut button_styles);
        }
        return;
    }

    let any_popover_open = !all_popovers.is_empty();
    if any_popover_open {
        let is_nested = all_popovers
            .iter()
            .any(|pop| is_descendant_of(entity, pop, &parents));
        if !is_nested {
            return;
        }
    }

    if let Some(btn) = button_entity {
        activate_trigger(btn, &mut button_styles);
    }

    let popover_title = config
        .popover_title
        .clone()
        .or_else(|| config.label.clone())
        .unwrap_or_else(|| path_to_label(&config.path));

    let options: Vec<ComboBoxOptionData> = config
        .variants
        .iter()
        .map(|v| {
            let mut opt = ComboBoxOptionData::new(path_to_label(&v.name)).with_value(&v.name);
            if let Some(icon) = v.icon {
                opt = opt.with_icon(icon);
            }
            opt
        })
        .collect();

    let default_width = 256.0;
    let popover_props = PopoverProps::new(trigger.entity)
        .with_placement(PopoverPlacement::Right)
        .with_padding(0.0);

    let popover_props = if let Some(width) = config.popover_width {
        popover_props.with_node(Node {
            width: px(width),
            min_width: px(default_width),
            ..default()
        })
    } else {
        popover_props.with_node(Node {
            min_width: px(default_width),
            ..default()
        })
    };

    let popover_entity = commands
        .spawn((VariantEditPopover, popover(popover_props)))
        .id();

    let header = commands
        .spawn(popover_header(
            PopoverHeaderProps::new(popover_title, popover_entity),
            &editor_font.0,
            &icon_font.0,
        ))
        .id();
    commands.entity(popover_entity).add_child(header);

    let combo_wrapper = commands
        .spawn((
            Node {
                width: percent(100),
                padding: UiRect::all(px(12.0)),
                border: UiRect::bottom(px(1.0)),
                ..default()
            },
            BorderColor::all(BORDER_COLOR),
        ))
        .id();

    let combo = commands
        .spawn((
            VariantComboBox(entity),
            combobox_with_selected(options, config.selected_index),
        ))
        .id();

    commands.entity(combo_wrapper).add_child(combo);
    commands.entity(popover_entity).add_child(combo_wrapper);

    let fields_container = commands
        .spawn((
            VariantFieldsContainer(entity),
            Hovered::default(),
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(12.0),
                padding: UiRect::all(px(12.0)),
                max_height: px(384.0),
                overflow: Overflow::scroll_y(),
                ..default()
            },
        ))
        .id();
    commands.entity(popover_entity).add_child(fields_container);

    if let Some(btn) = button_entity {
        tracker.open(popover_entity, btn);
    } else {
        tracker.popover = Some(popover_entity);
    }
}

fn handle_variant_combobox_change(
    trigger: On<ComboBoxChangeEvent>,
    variant_comboboxes: Query<&VariantComboBox>,
    mut variant_edits: Query<&mut VariantEditConfig, With<EditorVariantEdit>>,
    variant_edit_children: Query<&Children, With<EditorVariantEdit>>,
    mut texts: Query<&mut Text>,
    children_query: Query<&Children>,
) {
    let combobox_entity = trigger.entity;

    let Ok(variant_combobox) = variant_comboboxes.get(combobox_entity) else {
        return;
    };

    let variant_edit_entity = variant_combobox.0;
    let Ok(mut config) = variant_edits.get_mut(variant_edit_entity) else {
        return;
    };

    let new_index = trigger.selected;
    if new_index == config.selected_index {
        return;
    }

    config.selected_index = new_index;

    let Some(selected_variant) = config.variants.get(new_index).cloned() else {
        return;
    };

    if let Ok(children) = variant_edit_children.get(variant_edit_entity)
        && let Some(&button_entity) = children.last()
        && let Ok(button_children) = children_query.get(button_entity)
    {
        for child in button_children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                **text = path_to_label(&selected_variant.name);
                break;
            }
        }
    }
}
