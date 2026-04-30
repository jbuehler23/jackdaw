use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_commands::keybinds::{EditorAction, Keybind, KeybindRegistry};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonProps, ButtonVariant, button},
    dialog::{
        CloseDialogEvent, DialogActionEvent, DialogChildrenSlot, EditorDialog, OpenDialogEvent,
    },
    text_edit::{self, TextEditProps, TextEditValue},
    tokens,
};

pub struct KeybindSettingsPlugin;

impl Plugin for KeybindSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeybindRecordingState>()
            .init_resource::<KeyFilterState>()
            .add_observer(open_keybind_settings)
            .add_observer(on_keybind_settings_save)
            .add_observer(on_rebind_click)
            .add_observer(on_reset_click)
            .add_observer(on_reset_all_click)
            .add_observer(on_key_filter_click)
            .add_systems(
                Update,
                (
                    populate_keybind_dialog,
                    capture_keybind_recording,
                    capture_key_filter,
                    apply_keybind_filter,
                    cleanup_on_dialog_close,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

#[derive(Event)]
pub struct OpenKeybindSettingsEvent;

/// Working copy of keybind changes, applied on Save.
#[derive(Resource)]
struct PendingKeybindChanges(HashMap<EditorAction, Vec<Keybind>>);

/// Tracks which action/binding is being re-recorded.
#[derive(Resource, Default)]
struct KeybindRecordingState {
    target: Option<(EditorAction, usize)>,
    /// When set, a conflict was detected and we're waiting for the user to confirm.
    conflict: Option<PendingConflict>,
}

struct PendingConflict {
    new_bind: Keybind,
    /// The action that already has this binding.
    conflicting_action: EditorAction,
}

/// Inserted when dialog is open, removed on close/cancel.
#[derive(Resource)]
struct KeybindSettingsOpen;

/// Marker for the text filter input.
#[derive(Component)]
struct KeybindFilterInput;

/// Marker on the key-capture filter button.
#[derive(Component)]
struct KeyFilterButton;

/// Resource tracking the captured key filter.
#[derive(Resource, Default)]
struct KeyFilterState {
    /// When true, next non-modifier keypress sets the filter key.
    capturing: bool,
    /// The currently active key filter, if any.
    active_key: Option<KeyCode>,
}

/// Marker on each keybind row, storing its action for filtering.
#[derive(Component)]
struct KeybindRowAction(EditorAction);

/// Marker on category headers, storing category name for filtering.
#[derive(Component)]
struct KeybindCategoryHeader(String);

/// The text element showing the binding string for an action.
#[derive(Component)]
struct KeybindDisplayText(EditorAction);

/// Rebind button: (action, binding index).
#[derive(Component)]
struct KeybindRebindButton(EditorAction, usize);

/// Per-row reset to default button.
#[derive(Component)]
struct KeybindResetButton(EditorAction);

/// Reset All button marker.
#[derive(Component)]
struct KeybindResetAllButton;

/// Flag to prevent double-populating.
#[derive(Component)]
struct KeybindDialogPopulated;

fn open_keybind_settings(
    _event: On<OpenKeybindSettingsEvent>,
    mut commands: Commands,
    registry: Res<KeybindRegistry>,
    existing: Option<Res<KeybindSettingsOpen>>,
) {
    if existing.is_some() {
        return;
    }

    commands.insert_resource(PendingKeybindChanges(registry.bindings.clone()));
    commands.insert_resource(KeybindSettingsOpen);

    let mut dialog_event = OpenDialogEvent::new("Keybinds", "Save")
        .with_max_width(px(700))
        .with_close_on_click_outside(false)
        .without_content_padding();
    dialog_event.close_on_esc = false;
    commands.trigger(dialog_event);
}

fn format_bindings(bindings: &[Keybind]) -> String {
    if bindings.is_empty() {
        return "Unbound".to_string();
    }
    bindings
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(" / ")
}

fn populate_keybind_dialog(
    mut commands: Commands,
    pending: Option<Res<PendingKeybindChanges>>,
    slots: Query<Entity, (With<DialogChildrenSlot>, Added<DialogChildrenSlot>)>,
    populated: Query<(), With<KeybindDialogPopulated>>,
) {
    let Some(pending) = pending else { return };

    for slot_entity in &slots {
        if !populated.is_empty() {
            continue;
        }

        commands.entity(slot_entity).insert(KeybindDialogPopulated);

        // Top-level column: filter bar + scrollable list
        let wrapper = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                width: percent(100),
                ..Default::default()
            })
            .id();
        commands.entity(slot_entity).add_child(wrapper);

        // Filter bar: text input + key capture button
        let filter_row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_MD),
                padding: UiRect::all(px(tokens::SPACING_LG)),
                width: percent(100),
                ..Default::default()
            })
            .id();
        let mut filter_props = TextEditProps::default()
            .with_placeholder("Search actions...")
            .allow_empty();
        filter_props.grow = true;
        // Wrap text_edit in a flex-grow container to avoid duplicate Node
        let filter_input_wrapper = commands
            .spawn((
                KeybindFilterInput,
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
                children![text_edit::text_edit(filter_props)],
            ))
            .id();
        // Key capture filter button
        let key_filter_btn = commands
            .spawn((
                KeyFilterButton,
                button(ButtonProps::new("Key: Any").with_variant(ButtonVariant::Default)),
            ))
            .id();
        // Reset All button (top bar)
        let reset_all_btn = commands
            .spawn((
                KeybindResetAllButton,
                button(
                    ButtonProps::new("Reset All to Defaults").with_variant(ButtonVariant::Default),
                ),
            ))
            .id();
        commands.entity(filter_row).add_children(&[
            filter_input_wrapper,
            key_filter_btn,
            reset_all_btn,
        ]);
        commands.entity(wrapper).add_child(filter_row);

        // Scrollable list
        let scroll = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                max_height: px(460.0),
                overflow: Overflow::scroll_y(),
                width: percent(100),
                ..Default::default()
            })
            .id();
        commands.entity(wrapper).add_child(scroll);

        // Group actions by category
        let mut current_category = "";
        for &action in EditorAction::all() {
            let category = action.category();
            if category != current_category {
                current_category = category;
                let header = commands
                    .spawn((
                        KeybindCategoryHeader(category.to_string()),
                        Node {
                            padding: UiRect {
                                top: px(tokens::SPACING_LG),
                                bottom: px(tokens::SPACING_SM),
                                left: px(tokens::SPACING_LG),
                                right: px(tokens::SPACING_LG),
                            },
                            border: UiRect::bottom(px(2.0)),
                            margin: UiRect::top(px(tokens::SPACING_SM)),
                            ..Default::default()
                        },
                        BorderColor::all(tokens::BORDER_STRONG),
                        children![(
                            Text::new(category),
                            TextFont {
                                font_size: tokens::FONT_LG,
                                weight: FontWeight::BOLD,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_PRIMARY),
                        )],
                    ))
                    .id();
                commands.entity(scroll).add_child(header);
            }

            let bindings = pending.0.get(&action).cloned().unwrap_or_default();
            let binding_text = format_bindings(&bindings);

            // Row
            let row = commands
                .spawn((
                    KeybindRowAction(action),
                    Node {
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        width: percent(100),
                        padding: UiRect::axes(px(tokens::SPACING_LG), px(tokens::SPACING_SM)),
                        border: UiRect::bottom(px(1.0)),
                        ..Default::default()
                    },
                    BorderColor::all(tokens::BORDER_COLOR),
                ))
                .id();

            // Action name
            let name_label = commands
                .spawn((
                    Text::new(action.to_string()),
                    TextFont {
                        font_size: tokens::FONT_MD,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                    Node {
                        width: px(200.0),
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                ))
                .id();

            // Right side
            let right = commands
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_MD),
                    flex_grow: 1.0,
                    justify_content: JustifyContent::End,
                    ..Default::default()
                })
                .id();

            let text_color = if bindings.is_empty() {
                tokens::TEXT_SECONDARY
            } else {
                tokens::TEXT_PRIMARY
            };
            let binding_label = commands
                .spawn((
                    KeybindDisplayText(action),
                    Text::new(binding_text),
                    TextFont {
                        font_size: tokens::FONT_MD,
                        ..Default::default()
                    },
                    TextColor(text_color),
                    Node {
                        min_width: px(100.0),
                        ..Default::default()
                    },
                ))
                .id();

            let rebind_btn = commands
                .spawn((
                    KeybindRebindButton(action, 0),
                    button(ButtonProps::new("Rebind").with_variant(ButtonVariant::Default)),
                ))
                .id();

            let reset_btn = commands
                .spawn((
                    KeybindResetButton(action),
                    button(ButtonProps::new("Reset").with_variant(ButtonVariant::Ghost)),
                ))
                .id();

            commands
                .entity(right)
                .add_children(&[binding_label, rebind_btn, reset_btn]);
            commands.entity(row).add_children(&[name_label, right]);
            commands.entity(scroll).add_child(row);
        }
    }
}

fn on_key_filter_click(
    event: On<ButtonClickEvent>,
    key_filter_buttons: Query<&ChildOf, With<KeyFilterButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut key_filter: ResMut<KeyFilterState>,
    mut registry: ResMut<KeybindRegistry>,
    recording: Res<KeybindRecordingState>,
    children_query: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    let Ok(_) = key_filter_buttons.get(event.entity) else {
        return;
    };
    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }
    if recording.target.is_some() {
        return;
    }

    if key_filter.active_key.is_some() {
        key_filter.active_key = None;
        key_filter.capturing = false;
        set_button_text(event.entity, "Key: Any", &children_query, &mut texts);
    } else if key_filter.capturing {
        key_filter.capturing = false;
        registry.recording = false;
        set_button_text(event.entity, "Key: Any", &children_query, &mut texts);
    } else {
        key_filter.capturing = true;
        registry.recording = true;
        set_button_text(event.entity, "Press a key...", &children_query, &mut texts);
    }
}

/// Set the text content of a button's first Text child.
fn set_button_text(
    button_entity: Entity,
    label: &str,
    children_query: &Query<&Children>,
    texts: &mut Query<&mut Text>,
) {
    let Ok(children) = children_query.get(button_entity) else {
        return;
    };
    for child in children.iter() {
        if let Ok(mut text) = texts.get_mut(child) {
            text.0 = label.to_string();
            return;
        }
    }
}

fn capture_key_filter(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut key_filter: ResMut<KeyFilterState>,
    mut registry: ResMut<KeybindRegistry>,
    recording: Res<KeybindRecordingState>,
    key_filter_btns: Query<Entity, With<KeyFilterButton>>,
    children_query: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    if !key_filter.capturing {
        return;
    }
    if recording.target.is_some() {
        return;
    }

    // Right-click or Escape cancels
    if mouse.just_pressed(MouseButton::Right) || keyboard.just_pressed(KeyCode::Escape) {
        key_filter.capturing = false;
        registry.recording = false;
        for btn in &key_filter_btns {
            set_button_text(btn, "Key: Any", &children_query, &mut texts);
        }
        return;
    }

    let modifier_keys = [
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
    ];

    for key in keyboard.get_just_pressed() {
        if modifier_keys.contains(key) {
            continue;
        }

        key_filter.capturing = false;
        key_filter.active_key = Some(*key);
        registry.recording = false;

        let label = format!(
            "Key: {} (click to clear)",
            jackdaw_commands::keybinds::key_display_name(*key)
        );
        for btn in &key_filter_btns {
            set_button_text(btn, &label, &children_query, &mut texts);
        }
        return;
    }
}

/// Show/hide rows and category headers based on both text and key filters.
fn apply_keybind_filter(
    filter_wrappers: Query<&Children, With<KeybindFilterInput>>,
    text_values: Query<&TextEditValue, Changed<TextEditValue>>,
    all_text_values: Query<&TextEditValue>,
    key_filter: Res<KeyFilterState>,
    pending: Option<Res<PendingKeybindChanges>>,
    mut rows: Query<(&KeybindRowAction, &mut Node)>,
    mut headers: Query<(&KeybindCategoryHeader, &mut Node), Without<KeybindRowAction>>,
) {
    // Find the text value from the filter wrapper's child
    let filter_text = filter_wrappers.iter().find_map(|children| {
        children
            .iter()
            .find_map(|child| all_text_values.get(child).ok())
    });
    let Some(filter_value) = filter_text else {
        return;
    };

    // Only re-filter when text changes or key filter changes
    let text_changed = filter_wrappers
        .iter()
        .any(|children| children.iter().any(|child| text_values.get(child).is_ok()));
    if !text_changed && !key_filter.is_changed() {
        return;
    }
    let Some(pending) = pending else { return };

    let text_query = filter_value.0.trim().to_lowercase();
    let key_filter_active = key_filter.active_key;

    let mut visible_categories: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for (row, mut node) in &mut rows {
        let action = row.0;
        let action_name = action.to_string().to_lowercase();
        let bindings = pending.0.get(&action).cloned().unwrap_or_default();
        let binding_str = format_bindings(&bindings).to_lowercase();

        // Text filter: matches action name, binding string, or category
        let text_match = text_query.is_empty()
            || action_name.contains(&text_query)
            || binding_str.contains(&text_query)
            || action.category().to_lowercase().contains(&text_query);

        // Key filter: matches if any binding uses this key
        let key_match = match key_filter_active {
            Some(key) => bindings.iter().any(|b| b.key == key),
            None => true,
        };

        let visible = text_match && key_match;

        node.display = if visible {
            visible_categories.insert(action.category());
            Display::Flex
        } else {
            Display::None
        };
    }

    for (header, mut node) in &mut headers {
        let has_filters = !text_query.is_empty() || key_filter_active.is_some();
        node.display = if !has_filters || visible_categories.contains(header.0.as_str()) {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn on_rebind_click(
    event: On<ButtonClickEvent>,
    rebind_buttons: Query<(&KeybindRebindButton, &ChildOf)>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut recording_state: ResMut<KeybindRecordingState>,
    mut registry: ResMut<KeybindRegistry>,
    mut texts: Query<(&KeybindDisplayText, &mut Text, &mut TextColor)>,
) {
    let Ok((btn, _)) = rebind_buttons.get(event.entity) else {
        return;
    };

    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }

    let action = btn.0;
    let index = btn.1;

    recording_state.target = Some((action, index));
    registry.recording = true;

    for (display, mut text, mut color) in &mut texts {
        if display.0 == action {
            text.0 = "Press a key...".to_string();
            color.0 = tokens::TEXT_ACCENT;
        }
    }
}

/// Find which other action (if any) already uses this exact keybind in the pending changes.
fn find_conflict(
    pending: &HashMap<EditorAction, Vec<Keybind>>,
    new_bind: &Keybind,
    exclude_action: EditorAction,
) -> Option<EditorAction> {
    for (&other_action, bindings) in pending {
        if other_action == exclude_action {
            continue;
        }
        if bindings.iter().any(|b| b == new_bind) {
            return Some(other_action);
        }
    }
    None
}

/// Apply a keybind to the target action and remove it from any conflicting action.
fn apply_rebind(
    pending: &mut HashMap<EditorAction, Vec<Keybind>>,
    action: EditorAction,
    index: usize,
    new_bind: Keybind,
    conflict: Option<EditorAction>,
    texts: &mut Query<(&KeybindDisplayText, &mut Text, &mut TextColor)>,
) {
    // Remove the conflicting binding from the other action
    if let Some(conflicting_action) = conflict
        && let Some(bindings) = pending.get_mut(&conflicting_action)
    {
        bindings.retain(|b| b != &new_bind);
        let text_str = format_bindings(bindings);
        let text_color = if bindings.is_empty() {
            tokens::TEXT_SECONDARY
        } else {
            tokens::TEXT_PRIMARY
        };
        for (display, mut text, mut color) in texts.iter_mut() {
            if display.0 == conflicting_action {
                text.0 = text_str.clone();
                color.0 = text_color;
            }
        }
    }

    // Apply the new binding to the target action
    let bindings = pending.entry(action).or_default();
    if index < bindings.len() {
        bindings[index] = new_bind;
    } else {
        *bindings = vec![new_bind];
    }

    let text_str = format_bindings(bindings);
    for (display, mut text, mut color) in texts.iter_mut() {
        if display.0 == action {
            text.0 = text_str.clone();
            color.0 = tokens::TEXT_PRIMARY;
        }
    }
}

fn capture_keybind_recording(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut recording_state: ResMut<KeybindRecordingState>,
    mut registry: ResMut<KeybindRegistry>,
    mut pending: Option<ResMut<PendingKeybindChanges>>,
    mut texts: Query<(&KeybindDisplayText, &mut Text, &mut TextColor)>,
    mut commands: Commands,
    dialog_exists: Query<(), With<EditorDialog>>,
    settings_open: Option<Res<KeybindSettingsOpen>>,
) {
    let Some((action, index)) = recording_state.target else {
        // If not recording, handle ESC to close dialog
        if settings_open.is_some()
            && !dialog_exists.is_empty()
            && keyboard.just_pressed(KeyCode::Escape)
        {
            commands.trigger(CloseDialogEvent);
        }
        return;
    };

    // Right-click cancels recording (including conflict confirmation)
    if mouse.just_pressed(MouseButton::Right) {
        recording_state.target = None;
        recording_state.conflict = None;
        registry.recording = false;

        if let Some(ref pending) = pending {
            let bindings = pending.0.get(&action).cloned().unwrap_or_default();
            let text_str = format_bindings(&bindings);
            let text_color = if bindings.is_empty() {
                tokens::TEXT_SECONDARY
            } else {
                tokens::TEXT_PRIMARY
            };
            for (display, mut text, mut color) in &mut texts {
                if display.0 == action {
                    text.0 = text_str.clone();
                    color.0 = text_color;
                }
            }
        }
        return;
    }

    let modifier_keys = [
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
    ];

    for key in keyboard.get_just_pressed() {
        if modifier_keys.contains(key) {
            continue;
        }

        let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

        let new_bind = Keybind {
            key: *key,
            ctrl,
            shift,
            alt,
        };

        // If we're in conflict confirmation mode, check if user pressed the same key
        if let Some(ref conflict) = recording_state.conflict {
            if new_bind == conflict.new_bind {
                // User confirmed. Apply the rebind and clear the conflict.
                let conflicting_action = conflict.conflicting_action;
                if let Some(ref mut pending) = pending {
                    apply_rebind(
                        &mut pending.0,
                        action,
                        index,
                        new_bind,
                        Some(conflicting_action),
                        &mut texts,
                    );
                }
                recording_state.target = None;
                recording_state.conflict = None;
                registry.recording = false;
                return;
            }
            // Different key pressed, treat as a fresh recording (fall through below).
            recording_state.conflict = None;
        }

        if let Some(ref mut pending) = pending {
            // Check for conflicts with other actions
            if let Some(conflicting_action) = find_conflict(&pending.0, &new_bind, action) {
                // Show warning and wait for confirmation
                let bind_str = new_bind.to_string();
                recording_state.conflict = Some(PendingConflict {
                    new_bind,
                    conflicting_action,
                });

                let warning = format!(
                    "{bind_str} is bound to {conflicting_action}. Press again to override."
                );
                for (display, mut text, mut color) in &mut texts {
                    if display.0 == action {
                        text.0 = warning.clone();
                        color.0 = Color::srgb(1.0, 0.75, 0.2);
                    }
                }
                return;
            }

            // No conflict, apply directly.
            apply_rebind(&mut pending.0, action, index, new_bind, None, &mut texts);
        }

        recording_state.target = None;
        recording_state.conflict = None;
        registry.recording = false;
        return;
    }
}

fn on_reset_click(
    event: On<ButtonClickEvent>,
    reset_buttons: Query<(&KeybindResetButton, &ChildOf)>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut pending: Option<ResMut<PendingKeybindChanges>>,
    mut texts: Query<(&KeybindDisplayText, &mut Text, &mut TextColor)>,
) {
    let Ok((btn, _)) = reset_buttons.get(event.entity) else {
        return;
    };

    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }

    let action = btn.0;
    let defaults = KeybindRegistry::default();
    let default_bindings = defaults.bindings.get(&action).cloned().unwrap_or_default();

    if let Some(ref mut pending) = pending {
        pending.0.insert(action, default_bindings.clone());
    }

    let text_str = format_bindings(&default_bindings);
    let text_color = if default_bindings.is_empty() {
        tokens::TEXT_SECONDARY
    } else {
        tokens::TEXT_PRIMARY
    };
    for (display, mut text, mut color) in &mut texts {
        if display.0 == action {
            text.0 = text_str.clone();
            color.0 = text_color;
        }
    }
}

fn on_reset_all_click(
    event: On<ButtonClickEvent>,
    reset_all_buttons: Query<&ChildOf, With<KeybindResetAllButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut pending: Option<ResMut<PendingKeybindChanges>>,
    mut texts: Query<(&KeybindDisplayText, &mut Text, &mut TextColor)>,
) {
    let Ok(_) = reset_all_buttons.get(event.entity) else {
        return;
    };

    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }

    let defaults = KeybindRegistry::default();

    if let Some(ref mut pending) = pending {
        pending.0 = defaults.bindings.clone();
    }

    for (display, mut text, mut color) in &mut texts {
        let action = display.0;
        let bindings = defaults.bindings.get(&action).cloned().unwrap_or_default();
        let text_str = format_bindings(&bindings);
        text.0 = text_str;
        color.0 = if bindings.is_empty() {
            tokens::TEXT_SECONDARY
        } else {
            tokens::TEXT_PRIMARY
        };
    }
}

fn on_keybind_settings_save(
    _event: On<DialogActionEvent>,
    mut commands: Commands,
    pending: Option<Res<PendingKeybindChanges>>,
    settings_open: Option<Res<KeybindSettingsOpen>>,
    mut registry: ResMut<KeybindRegistry>,
) {
    if settings_open.is_none() {
        return;
    }

    if let Some(pending) = pending {
        registry.bindings = pending.0.clone();
    }
    registry.recording = false;

    crate::keybinds::save_keybinds(&registry);

    commands.remove_resource::<PendingKeybindChanges>();
    commands.remove_resource::<KeybindSettingsOpen>();
}

fn cleanup_on_dialog_close(
    mut commands: Commands,
    settings_open: Option<Res<KeybindSettingsOpen>>,
    dialogs: Query<(), With<EditorDialog>>,
    mut registry: ResMut<KeybindRegistry>,
    mut recording_state: ResMut<KeybindRecordingState>,
    mut key_filter: ResMut<KeyFilterState>,
) {
    if settings_open.is_none() {
        return;
    }
    if !dialogs.is_empty() {
        return;
    }

    registry.recording = false;
    recording_state.target = None;
    recording_state.conflict = None;
    *key_filter = KeyFilterState::default();
    commands.remove_resource::<PendingKeybindChanges>();
    commands.remove_resource::<KeybindSettingsOpen>();
}

fn is_in_dialog(
    start: Entity,
    parents: &Query<&ChildOf>,
    dialogs: &Query<(), With<EditorDialog>>,
) -> bool {
    let mut current = start;
    loop {
        if dialogs.get(current).is_ok() {
            return true;
        }
        let Ok(child_of) = parents.get(current) else {
            return false;
        };
        current = child_of.parent();
    }
}
