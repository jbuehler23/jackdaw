use bevy::input_focus::InputFocus;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{FontFeatureTag, FontFeatures};
use bevy_ui_text_input::actions::{TextInputAction, TextInputEdit};
use bevy_ui_text_input::*;
// Re-export key types from bevy_ui_text_input for consumers
pub use bevy_ui_text_input::{TextInputBuffer, TextInputQueue};

use crate::cursor::{ActiveCursor, HoverCursor};
use crate::icons::EditorFont;
use crate::tokens::{
    self, AXIS_LABEL_BG, BORDER_COLOR, ELEVATED_BG, PRIMARY_COLOR, SHADOW_COLOR_LIGHT,
    TEXT_BODY_COLOR, TEXT_MUTED_COLOR, TEXT_SIZE, TEXT_SIZE_SM,
};

pub fn plugin(app: &mut App) {
    if !app.is_plugin_added::<TextInputPlugin>() {
        app.add_plugins(TextInputPlugin);
    }
    app.add_systems(Update, setup_text_edit_input)
        .add_systems(
            Update,
            (
                handle_focus_style,
                handle_numeric_increment,
                (handle_unfocus, handle_clamp_on_unfocus).chain(),
                handle_drag_value,
                handle_click_to_focus,
                sync_text_edit_values,
            ),
        )
        .add_systems(PostUpdate, (apply_default_value, handle_suffix).chain());
}

pub fn set_text_input_value(queue: &mut TextInputQueue, text: String) {
    queue.add(TextInputAction::Edit(TextInputEdit::SelectAll));
    queue.add(TextInputAction::Edit(TextInputEdit::Paste(text)));
}

#[derive(Event)]
pub struct TextEditCommitEvent {
    pub entity: Entity,
    pub text: String,
}

/// Synced from the inner `TextInputBuffer` every frame. Attach to the outer wrapper entity
/// so consumers can poll the current text value without reaching into child entities.
#[derive(Component, Default, Clone)]
pub struct TextEditValue(pub String);

const INPUT_HEIGHT: f32 = 28.0;
const AFFIX_SIZE: u64 = 16;

#[derive(Component)]
pub struct EditorTextEdit;

#[derive(Component)]
pub struct TextEditWrapper(pub Entity);

/// Marker inserted on the wrapper entity while the user is drag-adjusting a numeric value.
/// Used by consumers to skip refresh/sync that would overwrite the in-flight drag value.
#[derive(Component)]
pub struct TextEditDragging;

#[derive(Component, Default, Clone, Copy, PartialEq)]
pub enum TextEditVariant {
    #[default]
    Default,
    NumericF32,
    NumericI32,
}

impl TextEditVariant {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::NumericF32 | Self::NumericI32)
    }
}

#[derive(Clone)]
pub enum TextEditPrefix {
    Label {
        label: String,
        size: f32,
        /// Optional accent color shown as a 2px left border on the label.
        color: Option<Color>,
    },
}

#[derive(Component)]
struct TextEditSuffix(String);

#[derive(Component)]
struct TextEditSuffixNode(Entity);

#[derive(Component)]
struct TextEditDefaultValue(String);

#[derive(Component, Default)]
struct DragHitbox {
    dragging: bool,
    start_x: f32,
    start_value: f64,
}

#[derive(Component, Clone, Copy)]
struct NumericRange {
    min: f64,
    max: f64,
}

#[derive(Component)]
struct AllowEmpty;

#[derive(Clone)]
pub enum FilterType {
    Decimal,
    Integer,
}

#[derive(Component)]
pub struct TextEditConfig {
    label: Option<String>,
    pub variant: TextEditVariant,
    filter: Option<FilterType>,
    prefix: Option<TextEditPrefix>,
    suffix: Option<String>,
    placeholder: String,
    default_value: Option<String>,
    min: f64,
    max: f64,
    auto_focus: bool,
    allow_empty: bool,
    drag_bottom: bool,
    disabled: bool,
    pub initialized: bool,
}

pub struct TextEditProps {
    pub label: Option<String>,
    pub placeholder: String,
    pub default_value: Option<String>,
    pub variant: TextEditVariant,
    pub filter: Option<FilterType>,
    pub prefix: Option<TextEditPrefix>,
    pub suffix: Option<String>,
    pub min: f64,
    pub max: f64,
    pub disabled: bool,
    pub auto_focus: bool,
    pub allow_empty: bool,
    pub drag_bottom: bool,
    pub grow: bool,
}

impl Default for TextEditProps {
    fn default() -> Self {
        Self {
            label: None,
            placeholder: String::new(),
            default_value: None,
            variant: TextEditVariant::Default,
            filter: None,
            prefix: None,
            suffix: None,
            min: f64::MIN,
            max: f64::MAX,
            disabled: false,
            auto_focus: false,
            allow_empty: false,
            drag_bottom: false,
            grow: false,
        }
    }
}

impl TextEditProps {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
    pub fn with_prefix(mut self, prefix: TextEditPrefix) -> Self {
        self.prefix = Some(prefix);
        self
    }
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }
    pub fn with_default_value(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }
    pub fn with_min(mut self, min: f64) -> Self {
        self.min = min;
        self
    }
    pub fn with_max(mut self, max: f64) -> Self {
        self.max = max;
        self
    }
    pub fn allow_empty(mut self) -> Self {
        self.allow_empty = true;
        self
    }
    pub fn drag_bottom(mut self) -> Self {
        self.drag_bottom = true;
        self
    }
    pub fn grow(mut self) -> Self {
        self.grow = true;
        self
    }
    pub fn auto_focus(mut self) -> Self {
        self.auto_focus = true;
        self
    }
    pub fn disabled(self) -> Self {
        self.with_disabled(true)
    }
    pub fn with_disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }
    pub fn numeric_f32(mut self) -> Self {
        self.variant = TextEditVariant::NumericF32;
        self.filter = Some(FilterType::Decimal);
        self.prefix = Some(TextEditPrefix::Label {
            label: "↔".to_string(),
            size: TEXT_SIZE,
            color: None,
        });
        self.min = f32::MIN as f64;
        self.max = f32::MAX as f64;
        self
    }
    pub fn numeric_i32(mut self) -> Self {
        self.variant = TextEditVariant::NumericI32;
        self.filter = Some(FilterType::Integer);
        self.prefix = Some(TextEditPrefix::Label {
            label: "↔".to_string(),
            size: TEXT_SIZE,
            color: None,
        });
        self.min = i32::MIN as f64;
        self.max = i32::MAX as f64;
        self
    }
}

pub fn text_edit(props: TextEditProps) -> impl Bundle {
    let TextEditProps {
        label,
        placeholder,
        default_value,
        variant,
        filter,
        prefix,
        suffix,
        min,
        max,
        auto_focus,
        allow_empty,
        drag_bottom,
        disabled,
        grow: _,
    } = props;

    (
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(3),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: px(0),
            ..default()
        },
        TextEditConfig {
            label,
            variant,
            filter,
            prefix,
            suffix,
            placeholder,
            default_value,
            min,
            max,
            auto_focus,
            allow_empty,
            drag_bottom,
            disabled,
            initialized: false,
        },
        TextEditValue::default(),
    )
}

fn setup_text_edit_input(
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    mut configs: Query<(Entity, &mut TextEditConfig)>,
    mut focus: ResMut<InputFocus>,
) {
    let font = editor_font.0.clone();
    let tabular_figures: FontFeatures = [FontFeatureTag::TABULAR_FIGURES].into();

    for (entity, mut config) in &mut configs {
        if config.initialized {
            continue;
        }
        config.initialized = true;

        if let Some(ref label) = config.label {
            let label_entity = commands
                .spawn((
                    Text::new(label),
                    TextFont {
                        font: font.clone(),
                        font_size: TEXT_SIZE_SM,
                        weight: FontWeight::MEDIUM,
                        ..default()
                    },
                    TextColor(TEXT_MUTED_COLOR.into()),
                ))
                .id();
            commands.entity(entity).add_child(label_entity);
        }

        let is_numeric = config.variant.is_numeric();
        let filter = config.filter.as_ref().map(|f| match f {
            FilterType::Decimal => TextInputFilter::Decimal,
            FilterType::Integer => TextInputFilter::Integer,
        });

        let has_prefix = config.prefix.is_some();
        let wrapper_entity = commands
            .spawn((
                Node {
                    width: percent(100),
                    height: px(INPUT_HEIGHT),
                    // If prefix, only a small bit of left padding so the label sits close to the edge
                    padding: if has_prefix {
                        UiRect::new(
                            px(tokens::SPACING_XS),
                            px(tokens::SPACING_MD),
                            px(tokens::SPACING_SM),
                            px(tokens::SPACING_SM),
                        )
                    } else {
                        UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM))
                    },
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                    // Stretch so prefix fills full height
                    align_items: if has_prefix {
                        AlignItems::Stretch
                    } else {
                        AlignItems::Center
                    },
                    column_gap: px(tokens::SPACING_MD),
                    ..default()
                },
                BackgroundColor(ELEVATED_BG),
                BoxShadow(vec![ShadowStyle {
                    x_offset: Val::ZERO,
                    y_offset: Val::ZERO,
                    blur_radius: Val::Px(1.0),
                    spread_radius: Val::Px(1.0),
                    color: SHADOW_COLOR_LIGHT,
                }]),
                Interaction::None,
                Hovered::default(),
                HoverCursor(bevy::window::SystemCursorIcon::Text),
            ))
            .observe(
                |mut ev: On<bevy::picking::events::Pointer<bevy::picking::events::DragStart>>| {
                    ev.propagate(false);
                },
            )
            .observe(
                |mut ev: On<bevy::picking::events::Pointer<bevy::picking::events::Drag>>| {
                    ev.propagate(false);
                },
            )
            .observe(
                |mut ev: On<bevy::picking::events::Pointer<bevy::picking::events::DragEnd>>| {
                    ev.propagate(false);
                },
            )
            .observe(
                |mut ev: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>| {
                    ev.propagate(false);
                },
            )
            .observe(
                |mut ev: On<bevy::picking::events::Pointer<bevy::picking::events::Press>>| {
                    ev.propagate(false);
                },
            )
            .id();

        commands.entity(entity).add_child(wrapper_entity);

        if is_numeric && !config.drag_bottom && !config.disabled {
            // When there's a prefix (XYZ label), the drag hitbox covers ONLY the
            // label area so clicking the value area still lets you type.
            // Without a prefix, the hitbox covers the left portion of the input.
            let (hitbox_left, hitbox_width) = if has_prefix {
                (0.0, AFFIX_SIZE as f32)
            } else {
                (0.0, INPUT_HEIGHT * 0.9)
            };
            let hitbox = commands
                .spawn((
                    DragHitbox::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        width: px(hitbox_width),
                        height: px(INPUT_HEIGHT),
                        left: px(hitbox_left),
                        ..default()
                    },
                    ZIndex(10),
                    Interaction::None,
                    Hovered::default(),
                    HoverCursor(bevy::window::SystemCursorIcon::ColResize),
                ))
                .id();
            commands.entity(wrapper_entity).add_child(hitbox);
        }

        if let Some(ref prefix) = config.prefix {
            let prefix_entity = match prefix {
                TextEditPrefix::Label { label, size, color } => {
                    let has_color = color.is_some();
                    let text_color = if has_color {
                        crate::tokens::TEXT_PRIMARY
                    } else {
                        TEXT_BODY_COLOR.with_alpha(0.5).into()
                    };

                    // Container node for layout (bg, border, sizing)
                    let prefix_id = commands
                        .spawn((
                            Node {
                                width: px(AFFIX_SIZE),
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                border: if has_color {
                                    UiRect::left(px(2))
                                } else {
                                    UiRect::default()
                                },
                                border_radius: if has_color {
                                    BorderRadius::left(px(2.5))
                                } else {
                                    BorderRadius::default()
                                },
                                ..default()
                            },
                            children![(
                                Text::new(label),
                                TextFont {
                                    font: font.clone(),
                                    font_size: *size,
                                    ..default()
                                },
                                TextColor(text_color),
                                TextLayout::new_with_justify(Justify::Center),
                            )],
                        ))
                        .id();
                    if let Some(c) = color {
                        commands
                            .entity(prefix_id)
                            .insert((BorderColor::all(*c), BackgroundColor(AXIS_LABEL_BG)));
                    }
                    prefix_id
                }
            };
            commands.entity(wrapper_entity).add_child(prefix_entity);
        }

        let placeholder = config
            .suffix
            .as_ref()
            .map(|s| format!("{}{}", config.placeholder, s))
            .unwrap_or_else(|| config.placeholder.clone());

        let mut text_input = commands.spawn((
            EditorTextEdit,
            config.variant,
            TextInputNode {
                mode: TextInputMode::SingleLine,
                clear_on_submit: false,
                unfocus_on_submit: true,
                is_enabled: !config.disabled,
                ..default()
            },
            TextFont {
                font: font.clone(),
                font_size: TEXT_SIZE,
                font_features: tabular_figures.clone(),
                ..default()
            },
            TextColor(TEXT_BODY_COLOR.into()),
            TextInputStyle {
                cursor_color: TEXT_BODY_COLOR.into(),
                cursor_width: 1.0,
                selection_color: PRIMARY_COLOR.with_alpha(0.3).into(),
                ..default()
            },
            TextInputPrompt {
                text: placeholder,
                color: Some(TEXT_BODY_COLOR.with_alpha(0.2).into()),
                ..default()
            },
            Node {
                flex_grow: 1.0,
                height: percent(100),
                justify_content: JustifyContent::Center,
                overflow: Overflow::clip(),
                ..default()
            },
        ));

        if config.auto_focus {
            focus.0 = Some(text_input.id());
        }

        if let Some(filter) = filter {
            text_input.insert(filter);
        }

        if let Some(ref suffix) = config.suffix {
            text_input.insert(TextEditSuffix(suffix.clone()));
        }

        if let Some(ref default_value) = config.default_value {
            text_input.insert(TextEditDefaultValue(default_value.clone()));
        }

        if is_numeric {
            text_input.insert(NumericRange {
                min: config.min,
                max: config.max,
            });
        }

        if config.allow_empty {
            text_input.insert(AllowEmpty);
        }

        let text_input_entity = text_input.id();

        commands.entity(wrapper_entity).add_child(text_input_entity);

        if let Some(ref suffix) = config.suffix {
            let suffix_entity = commands
                .spawn((
                    TextEditSuffixNode(text_input_entity),
                    Text::new(suffix.clone()),
                    TextFont {
                        font: font.clone(),
                        font_size: TEXT_SIZE,
                        font_features: tabular_figures.clone(),
                        ..default()
                    },
                    TextColor(TEXT_MUTED_COLOR.into()),
                    Node {
                        position_type: PositionType::Absolute,
                        top: px(5.5),
                        display: Display::None,
                        ..default()
                    },
                ))
                .id();
            commands.entity(wrapper_entity).add_child(suffix_entity);
        }
        commands
            .entity(wrapper_entity)
            .insert(TextEditWrapper(text_input_entity));
    }
}

fn handle_focus_style(
    focus: Res<InputFocus>,
    mut wrappers: Query<(&TextEditWrapper, &mut BorderColor, &Hovered)>,
) {
    for (wrapper, mut border_color, hovered) in &mut wrappers {
        let color = match (focus.0 == Some(wrapper.0), hovered.get()) {
            (true, _) => PRIMARY_COLOR,
            (_, true) => BORDER_COLOR.lighter(0.05),
            _ => BORDER_COLOR,
        };
        *border_color = BorderColor::all(color);
    }
}

fn apply_default_value(
    mut commands: Commands,
    mut text_edits: Query<(
        Entity,
        &TextEditDefaultValue,
        &TextEditVariant,
        &TextInputBuffer,
        &mut TextInputQueue,
        Option<&NumericRange>,
    )>,
) {
    for (entity, default_value, variant, buffer, mut queue, range) in &mut text_edits {
        if buffer.get_text().is_empty() {
            let text = if variant.is_numeric() {
                let value = clamp_value(default_value.0.parse().unwrap_or(0.0), range);
                format_numeric_value(value, *variant)
            } else {
                default_value.0.clone()
            };
            queue.add(TextInputAction::Edit(TextInputEdit::Paste(text)));
        }
        commands.entity(entity).remove::<TextEditDefaultValue>();
    }
}

fn handle_suffix(
    focus: Res<InputFocus>,
    text_edits: Query<
        (Entity, &TextInputBuffer, &TextInputLayoutInfo, &ChildOf),
        With<TextEditSuffix>,
    >,
    mut suffix_nodes: Query<(&TextEditSuffixNode, &mut Node), Without<TextEditWrapper>>,
    parents: Query<&ChildOf>,
    configs: Query<&TextEditConfig>,
) {
    const WRAPPER_PADDING: f32 = 8.0;
    const PREFIX_EXTRA: f32 = AFFIX_SIZE as f32 + 6.0;
    for (entity, buffer, layout_info, child_of) in &text_edits {
        let Some((_, mut node)) = suffix_nodes.iter_mut().find(|(link, _)| link.0 == entity) else {
            continue;
        };

        let has_prefix = parents
            .get(child_of.parent())
            .ok()
            .and_then(|wrapper_parent| configs.get(wrapper_parent.parent()).ok())
            .is_some_and(|config| config.prefix.is_some());

        let offset = WRAPPER_PADDING + if has_prefix { PREFIX_EXTRA } else { 0.0 };

        let show = focus.0 != Some(entity) && !buffer.get_text().is_empty();
        node.left = px(layout_info.size.x + offset);
        node.display = if show { Display::Flex } else { Display::None };
    }
}

fn handle_click_to_focus(
    mut focus: ResMut<InputFocus>,
    mouse: Res<ButtonInput<MouseButton>>,
    wrappers: Query<(&TextEditWrapper, &Interaction, &Children)>,
    drag_hitboxes: Query<&DragHitbox>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    for (wrapper, interaction, children) in &wrappers {
        let is_dragging = children
            .iter()
            .any(|c| drag_hitboxes.get(c).is_ok_and(|d| d.dragging));
        if *interaction == Interaction::Pressed && !is_dragging {
            focus.0 = Some(wrapper.0);
        }
    }
}

fn handle_unfocus(
    mut focus: ResMut<InputFocus>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    text_edits: Query<&ChildOf, With<EditorTextEdit>>,
    wrappers: Query<&Interaction, With<TextEditWrapper>>,
) {
    let Some(focused_entity) = focus.0 else {
        return;
    };
    let Ok(child_of) = text_edits.get(focused_entity) else {
        return;
    };
    let Ok(interaction) = wrappers.get(child_of.parent()) else {
        return;
    };

    let clicked_outside =
        mouse.get_just_pressed().next().is_some() && *interaction == Interaction::None;
    let key_dismiss = keyboard.just_pressed(KeyCode::Escape)
        || keyboard.just_pressed(KeyCode::Enter)
        || keyboard.just_pressed(KeyCode::NumpadEnter);

    if clicked_outside || key_dismiss {
        focus.0 = None;
    }
}

fn handle_clamp_on_unfocus(
    mut commands: Commands,
    focus: Res<InputFocus>,
    mut prev_focus: Local<Option<Entity>>,
    mut text_edits: Query<
        (
            &TextEditVariant,
            &TextInputBuffer,
            &mut TextInputQueue,
            Option<&TextEditSuffix>,
            Option<&NumericRange>,
            Option<&AllowEmpty>,
        ),
        With<EditorTextEdit>,
    >,
) {
    let prev = *prev_focus;
    *prev_focus = focus.0;

    let Some(was_focused) = prev else { return };
    if focus.0 == Some(was_focused) {
        return;
    }

    let Ok((variant, buffer, mut queue, suffix, range, allow_empty)) =
        text_edits.get_mut(was_focused)
    else {
        return;
    };

    let text = strip_suffix(&buffer.get_text(), suffix);

    commands.trigger(TextEditCommitEvent {
        entity: was_focused,
        text: text.clone(),
    });

    if !variant.is_numeric() {
        return;
    }

    if text.is_empty() && allow_empty.is_some() {
        return;
    }

    let value = text.parse().unwrap_or(0.0);
    update_input_value(&mut queue, value, *variant, range);
}

fn handle_numeric_increment(
    focus: Res<InputFocus>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut text_edits: Query<
        (
            Entity,
            &TextEditVariant,
            &TextInputBuffer,
            &mut TextInputQueue,
            Option<&TextEditSuffix>,
            Option<&NumericRange>,
        ),
        With<EditorTextEdit>,
    >,
) {
    let Some(focused_entity) = focus.0 else {
        return;
    };
    let Ok((_, variant, buffer, mut queue, suffix, range)) = text_edits.get_mut(focused_entity)
    else {
        return;
    };
    if !variant.is_numeric() {
        return;
    }

    let direction = match (
        keyboard.just_pressed(KeyCode::ArrowUp),
        keyboard.just_pressed(KeyCode::ArrowDown),
    ) {
        (true, _) => 1.0,
        (_, true) => -1.0,
        _ => return,
    };

    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);
    let step = if shift { 10.0 } else { 1.0 };
    let new_value = parse_numeric_value(&buffer.get_text(), suffix) + (direction * step);
    let rounded = (new_value * 100.0).round() / 100.0;

    update_input_value(&mut queue, rounded, *variant, range);
}

fn handle_drag_value(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut drag_hitboxes: Query<(Entity, &mut DragHitbox, &Interaction, &ChildOf)>,
    wrappers: Query<&TextEditWrapper>,
    mut text_edits: Query<
        (
            &TextEditVariant,
            &TextInputBuffer,
            &mut TextInputQueue,
            Option<&TextEditSuffix>,
            Option<&NumericRange>,
        ),
        With<EditorTextEdit>,
    >,
) {
    let Ok(window) = windows.single() else { return };
    let cursor_pos = window.cursor_position();

    for (entity, mut hitbox, interaction, child_of) in &mut drag_hitboxes {
        let Ok(wrapper) = wrappers.get(child_of.parent()) else {
            continue;
        };
        let input_entity = wrapper.0;

        if mouse.just_pressed(MouseButton::Left) && *interaction == Interaction::Pressed {
            if let Some(pos) = cursor_pos {
                let Ok((_, buffer, _, suffix, _)) = text_edits.get(input_entity) else {
                    continue;
                };
                hitbox.dragging = true;
                hitbox.start_x = pos.x;
                hitbox.start_value = parse_numeric_value(&buffer.get_text(), suffix);
                commands
                    .entity(entity)
                    .insert(ActiveCursor(bevy::window::SystemCursorIcon::ColResize));
                commands.entity(child_of.parent()).insert(TextEditDragging);
            }
        }

        if mouse.just_released(MouseButton::Left) {
            if hitbox.dragging {
                if let Ok((_, buffer, _, suffix, _)) = text_edits.get(input_entity) {
                    let text = strip_suffix(&buffer.get_text(), suffix);
                    commands.trigger(TextEditCommitEvent {
                        entity: input_entity,
                        text,
                    });
                }
                let parent = child_of.parent();
                commands.queue(move |world: &mut World| {
                    if let Ok(mut ec) = world.get_entity_mut(parent) {
                        ec.remove::<TextEditDragging>();
                    }
                });
            }
            hitbox.dragging = false;
            commands.queue(move |world: &mut World| {
                if let Ok(mut ec) = world.get_entity_mut(entity) {
                    ec.remove::<ActiveCursor>();
                }
            });
        }

        if hitbox.dragging {
            if let Some(pos) = cursor_pos {
                let Ok((variant, _, mut queue, _, range)) = text_edits.get_mut(input_entity) else {
                    continue;
                };

                let alt_mode = keyboard.pressed(KeyCode::SuperLeft)
                    || keyboard.pressed(KeyCode::SuperRight)
                    || keyboard.pressed(KeyCode::AltLeft)
                    || keyboard.pressed(KeyCode::AltRight);

                let (amount, sensitivity) = match (*variant, alt_mode) {
                    (TextEditVariant::NumericI32, false) => (1.0, 5.0),
                    (TextEditVariant::NumericI32, true) => (10.0, 10.0),
                    (_, false) => (0.1, 5.0),
                    (_, true) => (1.0, 10.0),
                };

                let steps = ((pos.x - hitbox.start_x) / sensitivity).floor() as f64;
                let new_value = hitbox.start_value + (steps * amount);
                let rounded = (new_value * 100.0).round() / 100.0;

                update_input_value(&mut queue, rounded, *variant, range);
            }
        }
    }
}

fn strip_suffix(text: &str, suffix: Option<&TextEditSuffix>) -> String {
    suffix
        .and_then(|s| text.strip_suffix(&format!(" {}", s.0)))
        .unwrap_or(text)
        .to_string()
}

fn parse_numeric_value(text: &str, suffix: Option<&TextEditSuffix>) -> f64 {
    strip_suffix(text, suffix).parse().unwrap_or(0.0)
}

pub fn format_numeric_value(value: f64, variant: TextEditVariant) -> String {
    match variant {
        TextEditVariant::NumericI32 => (value.round() as i32).to_string(),
        TextEditVariant::NumericF32 => {
            let rounded = (value * 100.0).round() / 100.0;
            format!("{rounded:.2}")
        }
        TextEditVariant::Default => value.to_string(),
    }
}

fn clamp_value(value: f64, range: Option<&NumericRange>) -> f64 {
    match range {
        Some(r) => value.clamp(r.min, r.max),
        None => value,
    }
}

fn update_input_value(
    queue: &mut TextInputQueue,
    value: f64,
    variant: TextEditVariant,
    range: Option<&NumericRange>,
) {
    let clamped = clamp_value(value, range);
    set_text_input_value(queue, format_numeric_value(clamped, variant));
}

fn sync_text_edit_values(
    mut configs: Query<(&TextEditConfig, &Children, &mut TextEditValue)>,
    wrappers: Query<&TextEditWrapper>,
    buffers: Query<&TextInputBuffer, With<EditorTextEdit>>,
) {
    for (config, children, mut value) in &mut configs {
        if !config.initialized {
            continue;
        }
        // Find wrapper child → TextEditWrapper → inner entity → TextInputBuffer
        for child in children.iter() {
            let Ok(wrapper) = wrappers.get(child) else {
                continue;
            };
            let Ok(buffer) = buffers.get(wrapper.0) else {
                continue;
            };
            let text = buffer.get_text();
            if value.0 != text {
                value.0 = text;
            }
            break;
        }
    }
}
