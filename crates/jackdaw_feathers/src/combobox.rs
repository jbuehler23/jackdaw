use bevy::prelude::*;
use lucide_icons::Icon;

use crate::button::{
    ButtonClickEvent, ButtonProps, ButtonSize, ButtonVariant, IconButtonProps, button, icon_button,
    set_button_variant,
};
use crate::popover::{EditorPopover, PopoverPlacement, PopoverProps, popover};
use crate::utils::is_descendant_of;

pub fn plugin(app: &mut App) {
    app.add_observer(handle_trigger_click)
        .add_observer(handle_option_click)
        .add_systems(
            Update,
            (
                setup_combobox,
                handle_combobox_popover_closed,
                sync_combobox_selection,
            ),
        );
}

#[derive(Component)]
pub struct EditorComboBox;

#[derive(Component)]
pub struct ComboBoxTrigger(pub Entity);

#[derive(Component)]
pub struct ComboBoxPopover(pub Entity);

#[derive(Component, Default)]
struct ComboBoxState {
    popover: Option<Entity>,
    last_synced_selected: Option<usize>,
}

#[derive(Component, Clone)]
struct ComboBoxOption {
    combobox: Entity,
    index: usize,
    label: String,
    value: Option<String>,
}

#[derive(Clone)]
pub struct ComboBoxOptionData {
    pub label: String,
    pub value: Option<String>,
    pub icon: Option<Icon>,
}

impl ComboBoxOptionData {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: None,
            icon: None,
        }
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl<T: Into<String>> From<T> for ComboBoxOptionData {
    fn from(label: T) -> Self {
        Self::new(label)
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum ComboBoxStyle {
    #[default]
    Default,
    IconOnly,
}

#[derive(Component)]
pub(crate) struct ComboBoxConfig {
    options: Vec<ComboBoxOptionData>,
    pub(crate) selected: usize,
    style: ComboBoxStyle,
    label_override: Option<String>,
    highlight_selected: bool,
    initialized: bool,
}

#[derive(EntityEvent)]
pub struct ComboBoxChangeEvent {
    pub entity: Entity,
    pub selected: usize,
    pub label: String,
    pub value: Option<String>,
}

/// Selected index component for external mutation of combobox selection.
#[derive(Component)]
pub struct ComboBoxSelectedIndex(pub usize);

pub fn combobox(options: Vec<impl Into<ComboBoxOptionData>>) -> impl Bundle {
    combobox_with_selected(options, 0)
}

pub fn combobox_with_selected(
    options: Vec<impl Into<ComboBoxOptionData>>,
    selected: usize,
) -> impl Bundle {
    (
        EditorComboBox,
        ComboBoxConfig {
            options: options.into_iter().map(Into::into).collect(),
            selected,
            style: ComboBoxStyle::Default,
            label_override: None,
            highlight_selected: true,
            initialized: false,
        },
        ComboBoxState::default(),
        Node {
            width: percent(100),
            ..default()
        },
    )
}

pub fn combobox_with_label(
    options: Vec<impl Into<ComboBoxOptionData>>,
    label: impl Into<String>,
) -> impl Bundle {
    (
        EditorComboBox,
        ComboBoxConfig {
            options: options.into_iter().map(Into::into).collect(),
            selected: 0,
            style: ComboBoxStyle::Default,
            label_override: Some(label.into()),
            highlight_selected: false,
            initialized: false,
        },
        ComboBoxState::default(),
        Node {
            width: percent(100),
            ..default()
        },
    )
}

pub fn combobox_icon(options: Vec<impl Into<ComboBoxOptionData>>) -> impl Bundle {
    (
        EditorComboBox,
        ComboBoxConfig {
            options: options.into_iter().map(Into::into).collect(),
            selected: 0,
            style: ComboBoxStyle::IconOnly,
            label_override: None,
            highlight_selected: false,
            initialized: false,
        },
        ComboBoxState::default(),
        Node::default(),
    )
}

pub fn combobox_icon_with_selected(
    options: Vec<impl Into<ComboBoxOptionData>>,
    selected: usize,
) -> impl Bundle {
    (
        EditorComboBox,
        ComboBoxConfig {
            options: options.into_iter().map(Into::into).collect(),
            selected,
            style: ComboBoxStyle::IconOnly,
            label_override: None,
            highlight_selected: true,
            initialized: false,
        },
        ComboBoxState::default(),
        Node::default(),
    )
}

fn setup_combobox(
    mut commands: Commands,
    icon_font: Res<crate::icons::IconFont>,
    mut configs: Query<(Entity, &mut ComboBoxConfig)>,
) {
    for (entity, mut config) in &mut configs {
        if config.initialized {
            continue;
        }
        config.initialized = true;

        // The previous flow spawned the trigger via `commands.spawn`,
        // then best-effort attached it as a child of `entity` with
        // `commands.get_entity(entity).add_child(...)`. `get_entity`
        // only guards at *queue* time, not flush time; if the
        // combobox was cascade-despawned before the spawn + add_child
        // commands drained, the trigger ended up orphaned with a
        // `ChildOf` pointing at a dead parent, producing the
        // `Entity despawned … is invalid` errors. Wrap the whole
        // setup in a queued closure that runs with `&mut World`, do
        // a single synchronous liveness check, and spawn the trigger
        // inside `with_children` so parent + child land atomically.
        let style = config.style;
        let icon_font_handle = icon_font.0.clone();
        let selected_option = config.options.get(config.selected).cloned();
        let label_override = config.label_override.clone();
        commands.queue(move |world: &mut World| {
            let Ok(mut ec) = world.get_entity_mut(entity) else {
                return;
            };
            match style {
                ComboBoxStyle::IconOnly => {
                    ec.with_children(|parent| {
                        parent.spawn((
                            ComboBoxTrigger(entity),
                            icon_button(
                                IconButtonProps::new(Icon::Ellipsis).variant(ButtonVariant::Ghost),
                                &icon_font_handle,
                            ),
                        ));
                    });
                }
                ComboBoxStyle::Default => {
                    let label = label_override
                        .or_else(|| selected_option.as_ref().map(|o| o.label.clone()))
                        .unwrap_or_default();
                    let selected_icon = selected_option.and_then(|o| o.icon);

                    let mut button_props = ButtonProps::new(label)
                        .with_size(ButtonSize::MD)
                        .align_left()
                        .with_right_icon(Icon::ChevronDown);

                    if let Some(icon) = selected_icon {
                        button_props = button_props.with_left_icon(icon);
                    }

                    ec.with_children(|parent| {
                        parent.spawn((ComboBoxTrigger(entity), button(button_props)));
                    });
                }
            }
        });
    }
}

fn handle_trigger_click(
    trigger: On<ButtonClickEvent>,
    mut commands: Commands,
    triggers: Query<&ComboBoxTrigger>,
    configs: Query<&ComboBoxConfig>,
    mut states: Query<&mut ComboBoxState>,
    existing_popovers: Query<(Entity, &ComboBoxPopover)>,
    all_popovers: Query<Entity, With<EditorPopover>>,
    mut button_styles: Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
    parents: Query<&ChildOf>,
) {
    let Ok(combo_trigger) = triggers.get(trigger.entity) else {
        return;
    };
    let Ok(config) = configs.get(combo_trigger.0) else {
        return;
    };
    let Ok(mut state) = states.get_mut(combo_trigger.0) else {
        return;
    };

    // If popover is already open, close it
    for (popover_entity, popover_ref) in &existing_popovers {
        if popover_ref.0 == combo_trigger.0 {
            commands.entity(popover_entity).try_despawn();
            state.popover = None;
            let base = if config.style == ComboBoxStyle::IconOnly {
                ButtonVariant::Ghost
            } else {
                ButtonVariant::Default
            };
            if let Ok((mut bg, mut border, mut variant)) = button_styles.get_mut(trigger.entity) {
                *variant = base;
                set_button_variant(base, &mut bg, &mut border);
            }
            return;
        }
    }

    // Don't open if another non-nested popover is open
    let any_popover_open = !all_popovers.is_empty();
    if any_popover_open {
        let is_nested = all_popovers
            .iter()
            .any(|pop| is_descendant_of(combo_trigger.0, pop, &parents));
        if !is_nested {
            return;
        }
    }

    let combobox_entity = combo_trigger.0;

    // Activate the trigger button
    if let Ok((mut bg, mut border, mut variant)) = button_styles.get_mut(trigger.entity) {
        *variant = ButtonVariant::ActiveAlt;
        set_button_variant(ButtonVariant::ActiveAlt, &mut bg, &mut border);
    }

    // Create popover with options
    let popover_entity = commands
        .spawn((
            ComboBoxPopover(combobox_entity),
            popover(
                PopoverProps::new(trigger.entity)
                    .with_placement(PopoverPlacement::BottomStart)
                    .with_padding(4.0)
                    .with_z_index(200)
                    .with_node(Node {
                        min_width: px(120.0),
                        ..default()
                    }),
            ),
        ))
        .id();

    state.popover = Some(popover_entity);

    for (index, option) in config.options.iter().enumerate() {
        let variant = if config.highlight_selected && index == config.selected {
            ButtonVariant::Active
        } else {
            ButtonVariant::Ghost
        };

        let mut button_props = ButtonProps::new(&option.label)
            .with_variant(variant)
            .align_left();

        if let Some(icon) = option.icon {
            button_props = button_props.with_left_icon(icon);
        }

        commands.entity(popover_entity).with_child((
            ComboBoxOption {
                combobox: combobox_entity,
                index,
                label: option.label.clone(),
                value: option.value.clone(),
            },
            button(button_props),
        ));
    }
}

fn handle_combobox_popover_closed(
    _commands: Commands,
    mut states: Query<(&mut ComboBoxState, &ComboBoxConfig, &Children), With<EditorComboBox>>,
    popovers: Query<Entity, With<EditorPopover>>,
    triggers: Query<Entity, With<ComboBoxTrigger>>,
    mut button_styles: Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
) {
    for (mut state, config, combobox_children) in &mut states {
        let Some(popover_entity) = state.popover else {
            continue;
        };

        if popovers.get(popover_entity).is_ok() {
            continue;
        }

        state.popover = None;

        let base = if config.style == ComboBoxStyle::IconOnly {
            ButtonVariant::Ghost
        } else {
            ButtonVariant::Default
        };

        for child in combobox_children.iter() {
            if triggers.get(child).is_ok() {
                if let Ok((mut bg, mut border, mut variant)) = button_styles.get_mut(child) {
                    *variant = base;
                    set_button_variant(base, &mut bg, &mut border);
                }
                break;
            }
        }
    }
}

fn handle_option_click(
    trigger: On<ButtonClickEvent>,
    mut commands: Commands,
    options: Query<&ComboBoxOption>,
    mut configs: Query<&mut ComboBoxConfig>,
    popovers: Query<(Entity, &ComboBoxPopover)>,
    triggers: Query<(Entity, &ComboBoxTrigger, &Children)>,
    mut texts: Query<&mut Text>,
) {
    let Ok(option) = options.get(trigger.entity) else {
        return;
    };

    let Ok(mut config) = configs.get_mut(option.combobox) else {
        return;
    };

    let has_label_override = config.label_override.is_some();
    let is_icon_only = config.style == ComboBoxStyle::IconOnly;
    config.selected = option.index;

    commands.trigger(ComboBoxChangeEvent {
        entity: option.combobox,
        selected: option.index,
        label: option.label.clone(),
        value: option.value.clone(),
    });

    // Update trigger button text
    if !is_icon_only && !has_label_override {
        for (_trigger_entity, combo_trigger, children) in &triggers {
            if combo_trigger.0 != option.combobox {
                continue;
            }
            for child in children.iter() {
                if let Ok(mut text) = texts.get_mut(child) {
                    **text = option.label.clone();
                    break;
                }
            }
        }
    }

    // Close popover
    for (popover_entity, popover_ref) in &popovers {
        if popover_ref.0 == option.combobox {
            commands.entity(popover_entity).try_despawn();
        }
    }
}

fn sync_combobox_selection(
    mut combos: Query<(Entity, &ComboBoxConfig, &mut ComboBoxState)>,
    triggers: Query<(&ComboBoxTrigger, &Children)>,
    mut texts: Query<&mut Text>,
) {
    for (entity, config, mut state) in &mut combos {
        if !config.initialized {
            continue;
        }
        let Some(option) = config.options.get(config.selected) else {
            continue;
        };
        let index_changed = state.last_synced_selected != Some(config.selected);
        for (trigger_ref, children) in &triggers {
            if trigger_ref.0 != entity {
                continue;
            }
            for child in children.iter() {
                if let Ok(mut text) = texts.get_mut(child) {
                    if index_changed || **text != option.label {
                        **text = option.label.clone();
                        state.last_synced_selected = Some(config.selected);
                    }
                    break;
                }
            }
            break;
        }
    }
}
