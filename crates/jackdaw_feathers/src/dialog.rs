use bevy::prelude::*;
use jackdaw_commands::KeymapCapture;
use lucide_icons::Icon;

use crate::button::{
    ButtonClickEvent, ButtonProps, ButtonVariant, IconButtonProps, button, icon_button,
};
use crate::icons::EditorFont;
use crate::tokens::{
    BACKGROUND_COLOR, BORDER_COLOR, TEXT_DISPLAY_COLOR, TEXT_MUTED_COLOR, TEXT_SIZE_LG,
    TEXT_SIZE_XL,
};

const BACKDROP_OPACITY: f32 = 0.8;

pub fn plugin(app: &mut App) {
    app.add_observer(on_open_dialog)
        .add_observer(on_open_confirmation_dialog)
        .add_observer(on_action_button_click)
        .add_observer(on_secondary_action_button_click)
        .add_observer(on_cancel_button_click)
        .add_observer(on_close_button_click)
        .add_observer(on_close_dialog)
        .add_systems(
            Update,
            (
                sync_children_slot_visibility,
                handle_backdrop_click,
                handle_esc_key,
            ),
        );
}

#[derive(Component)]
pub struct EditorDialog;

/// What a dialog is asking and what it will take for an answer.
///
/// The footer's buttons carry their labels in `Text` children several
/// levels down, which is enough to draw them and not enough to answer
/// one from anywhere but a pointer. Recorded here at open time so a
/// caller with no pointer can read the question and name a button --
/// see the editor's `dialog.answer` operator.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogChoices {
    pub title: Option<String>,
    pub description: Option<String>,
    /// The primary button, on the right.
    pub action: Option<String>,
    /// The secondary button, on the far left, when there is one.
    pub secondary: Option<String>,
    /// The cancel button. Dismissing the dialog any other way (Esc, a
    /// click outside, the close cross) means the same thing.
    pub cancel: Option<String>,
}

impl DialogChoices {
    /// The buttons a caller may name, in the order their indices run:
    /// the primary action first, because that is the answer a dialog is
    /// asking for.
    pub fn labels(&self) -> Vec<String> {
        [&self.action, &self.secondary, &self.cancel]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }
}

/// Which button of a dialog an answer means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogChoice {
    Action,
    Secondary,
    Cancel,
}

/// Why [`resolve_dialog_choice`] pressed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogChoiceError {
    /// Nothing on the dialog is spelled or numbered like this.
    NoMatch,
    /// The prefix fits more than one button, named here in the order the
    /// dialog offers them. Pressing one of them would be a guess, so the
    /// caller is asked to say which.
    Ambiguous(Vec<String>),
}

impl core::fmt::Display for DialogChoiceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoMatch => write!(f, "no button on this dialog answers to it"),
            Self::Ambiguous(candidates) => {
                write!(f, "it is a prefix of more than one button: {candidates:?}")
            }
        }
    }
}

/// The button `choice` names: a label, or an index into
/// [`DialogChoices::labels`].
///
/// A label matches exactly first, so a button literally spelled `1` is
/// reachable by name. Failing that, a value that parses as an integer is
/// an index. Failing that, one case-insensitive prefix match answers, so
/// `Reload` presses a button spelled `Reload (discards your unsaved
/// changes)` -- a caller should not have to reproduce the parenthetical
/// the editor appends when a tab is dirty.
///
/// A prefix that fits two buttons presses neither: `D` on a dialog
/// offering `Don't Save` and `Delete` is a coin toss over data, so it
/// comes back as [`DialogChoiceError::Ambiguous`] naming both.
pub fn resolve_dialog_choice(
    choices: &DialogChoices,
    choice: &str,
) -> Result<DialogChoice, DialogChoiceError> {
    let slots = [
        (DialogChoice::Action, choices.action.as_deref()),
        (DialogChoice::Secondary, choices.secondary.as_deref()),
        (DialogChoice::Cancel, choices.cancel.as_deref()),
    ];
    let present: Vec<(DialogChoice, &str)> = slots
        .iter()
        .filter_map(|(which, label)| label.map(|label| (*which, label)))
        .collect();

    let wanted = choice.trim();
    if let Some((which, _)) = present.iter().find(|(_, label)| *label == wanted) {
        return Ok(*which);
    }
    if let Ok(index) = wanted.parse::<usize>() {
        return present
            .get(index)
            .map(|(which, _)| *which)
            .ok_or(DialogChoiceError::NoMatch);
    }

    let lowered = wanted.to_lowercase();
    let matches: Vec<&(DialogChoice, &str)> = present
        .iter()
        .filter(|(_, label)| label.to_lowercase().starts_with(&lowered))
        .collect();
    match matches.as_slice() {
        [(which, _)] => Ok(*which),
        [] => Err(DialogChoiceError::NoMatch),
        many => Err(DialogChoiceError::Ambiguous(
            many.iter().map(|(_, label)| (*label).to_string()).collect(),
        )),
    }
}

/// Press one of `dialog`'s buttons, as a click on it would.
///
/// The same two steps every footer button takes: fire the event the
/// button fires, then take the dialog down. Cancel fires nothing, which
/// is what makes a dismissal and a cancel the same answer.
pub fn answer_dialog(commands: &mut Commands, dialog: Entity, choice: DialogChoice) {
    match choice {
        DialogChoice::Action => commands.trigger(DialogActionEvent { entity: dialog }),
        DialogChoice::Secondary => {
            commands.trigger(DialogSecondaryActionEvent { entity: dialog });
        }
        DialogChoice::Cancel => {}
    }
    dismiss_dialog(commands, dialog);
}

#[derive(Component)]
struct DialogBackdrop;

#[derive(Component)]
struct DialogPanel;

#[derive(Component)]
struct DialogCloseButton;

#[derive(Component)]
struct DialogCancelButton;

#[derive(Component)]
struct DialogActionButton;

#[derive(Component)]
struct DialogSecondaryActionButton;

#[derive(Component)]
pub struct DialogChildrenSlot;

#[derive(Component, Default, Clone, Copy)]
pub enum DialogVariant {
    #[default]
    Default,
    Destructive,
}

impl DialogVariant {
    fn action_button_variant(&self) -> ButtonVariant {
        match self {
            Self::Default => ButtonVariant::Primary,
            Self::Destructive => ButtonVariant::Destructive,
        }
    }
}

#[derive(Component)]
struct DialogConfig {
    close_on_click_outside: bool,
    close_on_esc: bool,
}

#[derive(EntityEvent)]
pub struct DialogActionEvent {
    pub entity: Entity,
}

#[derive(EntityEvent)]
pub struct DialogSecondaryActionEvent {
    pub entity: Entity,
}

#[derive(Event)]
pub struct CloseDialogEvent;

#[derive(Event)]
pub struct OpenDialogEvent {
    pub title: Option<String>,
    pub description: Option<String>,
    pub action: Option<String>,
    pub secondary_action: Option<String>,
    pub cancel: Option<String>,
    pub variant: DialogVariant,
    pub has_close_button: bool,
    pub close_on_click_outside: bool,
    pub close_on_esc: bool,
    pub max_width: Option<Val>,
    pub content_padding: UiRect,
}

impl OpenDialogEvent {
    pub fn new(title: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            description: None,
            action: Some(action.into()),
            secondary_action: None,
            cancel: Some("Cancel".into()),
            variant: DialogVariant::Default,
            has_close_button: true,
            close_on_click_outside: true,
            close_on_esc: true,
            max_width: None,
            content_padding: UiRect::all(px(24)),
        }
    }

    pub fn without_cancel(mut self) -> Self {
        self.cancel = None;
        self
    }

    pub fn with_secondary_action(mut self, label: impl Into<String>) -> Self {
        self.secondary_action = Some(label.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_variant(mut self, variant: DialogVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn with_close_button(mut self, has_close_button: bool) -> Self {
        self.has_close_button = has_close_button;
        self
    }

    pub fn with_close_on_click_outside(mut self, close_on_click_outside: bool) -> Self {
        self.close_on_click_outside = close_on_click_outside;
        self
    }

    pub fn with_max_width(mut self, max_width: Val) -> Self {
        self.max_width = Some(max_width);
        self
    }

    pub fn without_content_padding(mut self) -> Self {
        self.content_padding = UiRect::ZERO;
        self
    }
}

#[derive(Event)]
pub struct OpenConfirmationDialogEvent {
    pub title: String,
    pub description: Option<String>,
    pub action: String,
}

impl OpenConfirmationDialogEvent {
    pub fn new(title: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: None,
            action: action.into(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl From<&OpenConfirmationDialogEvent> for OpenDialogEvent {
    fn from(event: &OpenConfirmationDialogEvent) -> Self {
        let mut dialog = OpenDialogEvent::new(&event.title, &event.action)
            .with_variant(DialogVariant::Destructive)
            .with_close_button(false)
            .with_close_on_click_outside(false);
        dialog.description = event.description.clone();
        dialog
    }
}

fn on_open_dialog(
    event: On<OpenDialogEvent>,
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    icon_font: Res<crate::icons::IconFont>,
    existing: Query<Entity, With<EditorDialog>>,
) {
    if !existing.is_empty() {
        return;
    }
    spawn_dialog(&mut commands, &editor_font.0, &icon_font.0, &event);
}

fn on_open_confirmation_dialog(
    event: On<OpenConfirmationDialogEvent>,
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    icon_font: Res<crate::icons::IconFont>,
    existing: Query<Entity, With<EditorDialog>>,
) {
    if !existing.is_empty() {
        return;
    }
    let dialog_event: OpenDialogEvent = event.event().into();
    spawn_dialog(&mut commands, &editor_font.0, &icon_font.0, &dialog_event);
}

fn spawn_dialog(
    commands: &mut Commands,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
    event: &OpenDialogEvent,
) {
    let font = editor_font.clone();

    let backdrop_id = commands
        .spawn((
            DialogBackdrop,
            Interaction::None,
            Node {
                width: percent(100),
                height: percent(100),
                position_type: PositionType::Absolute,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(BACKDROP_OPACITY)),
        ))
        .id();

    let has_header = event.title.is_some() || event.description.is_some();
    let has_footer =
        event.action.is_some() || event.cancel.is_some() || event.secondary_action.is_some();

    let header_id = if has_header {
        let mut header = commands.spawn((
            Node {
                padding: UiRect::all(px(24)),
                border: UiRect::bottom(px(1)),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
            BorderColor::all(BORDER_COLOR),
        ));

        if let Some(title) = &event.title {
            header.with_child((
                Text::new(title),
                TextFont {
                    font: font.clone().into(),
                    font_size: TEXT_SIZE_XL,
                    weight: FontWeight::SEMIBOLD,
                    ..default()
                },
                TextColor(TEXT_DISPLAY_COLOR.into()),
            ));
        }

        if let Some(desc) = &event.description {
            header.with_child((
                Text::new(desc),
                TextFont {
                    font: font.clone().into(),
                    font_size: TEXT_SIZE_LG,
                    ..default()
                },
                TextColor(TEXT_MUTED_COLOR.into()),
            ));
        }

        Some(header.id())
    } else {
        None
    };

    let footer_id = if has_footer {
        let mut footer = commands.spawn(Node {
            padding: UiRect::all(px(24)),
            column_gap: px(6),
            justify_content: JustifyContent::End,
            ..default()
        });

        if let Some(cancel) = &event.cancel {
            footer.with_child((DialogCancelButton, button(ButtonProps::new(cancel))));
        }

        if let Some(action) = &event.action {
            footer.with_child((
                DialogActionButton,
                button(
                    ButtonProps::new(action).with_variant(event.variant.action_button_variant()),
                ),
            ));
        }

        let footer_id = footer.id();

        // Secondary action on the far left (margin-right: auto pushes it left).
        // The button is wrapped in a layout node to avoid a duplicate `Node` component
        // (since `button()` already provides one via `button_base()`).
        if let Some(secondary) = &event.secondary_action {
            let btn = commands
                .spawn((
                    DialogSecondaryActionButton,
                    button(ButtonProps::new(secondary)),
                ))
                .id();
            let wrapper = commands
                .spawn(Node {
                    margin: UiRect {
                        right: Val::Auto,
                        ..default()
                    },
                    ..default()
                })
                .id();
            commands.entity(wrapper).add_child(btn);
            commands.entity(footer_id).insert_child(0, wrapper);
        }

        Some(footer_id)
    } else {
        None
    };

    let mut panel = commands.spawn((
        DialogPanel,
        Interaction::None,
        Node {
            width: percent(100),
            max_width: event.max_width.unwrap_or(px(448)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(6)),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(BACKGROUND_COLOR.into()),
        BorderColor::all(BORDER_COLOR),
    ));

    if let Some(header_id) = header_id {
        panel.add_child(header_id);
    }

    panel.with_child((
        DialogChildrenSlot,
        Node {
            display: Display::None,
            padding: event.content_padding,
            border: UiRect::bottom(px(1)),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
            ..default()
        },
        BorderColor::all(BORDER_COLOR),
    ));

    if let Some(footer_id) = footer_id {
        panel.add_child(footer_id);
    }

    if event.has_close_button {
        panel.with_child((
            Node {
                position_type: PositionType::Absolute,
                top: px(20),
                right: px(20),
                ..default()
            },
            children![(
                DialogCloseButton,
                icon_button(
                    IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                    icon_font,
                ),
            )],
        ));
    }

    let panel_id = panel.id();
    commands.entity(backdrop_id).add_child(panel_id);

    commands
        .spawn((
            EditorDialog,
            DialogChoices {
                title: event.title.clone(),
                description: event.description.clone(),
                action: event.action.clone(),
                secondary: event.secondary_action.clone(),
                cancel: event.cancel.clone(),
            },
            event.variant,
            DialogConfig {
                close_on_click_outside: event.close_on_click_outside,
                close_on_esc: event.close_on_esc,
            },
            Node {
                width: percent(100),
                height: percent(100),
                position_type: PositionType::Absolute,
                ..default()
            },
            GlobalZIndex(200),
            Pickable::IGNORE,
        ))
        .add_child(backdrop_id);
}

fn dismiss_dialog(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).try_despawn();
}

fn sync_children_slot_visibility(
    mut slots: Query<(&Children, &mut Node), (With<DialogChildrenSlot>, Changed<Children>)>,
) {
    for (children, mut node) in &mut slots {
        node.display = if children.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
    }
}

fn handle_backdrop_click(
    interactions: Query<(&Interaction, &ChildOf), (Changed<Interaction>, With<DialogBackdrop>)>,
    panels: Query<&Interaction, With<DialogPanel>>,
    dialogs: Query<&DialogConfig, With<EditorDialog>>,
    mut commands: Commands,
) {
    for (interaction, child_of) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }

        let Ok(config) = dialogs.get(child_of.parent()) else {
            continue;
        };

        if !config.close_on_click_outside {
            continue;
        }

        if panels.iter().any(|i| *i == Interaction::Pressed) {
            continue;
        }

        dismiss_dialog(&mut commands, child_of.parent());
    }
}

/// Escape closes a dialog -- except while the keybind settings are
/// recording, when the press belongs to the recorder. Without this the
/// one dialog the user can record in is the one Escape closes, so Escape
/// could never be bound to anything.
fn handle_esc_key(
    keyboard: Res<ButtonInput<KeyCode>>,
    capture: Option<Res<KeymapCapture>>,
    dialogs: Query<(Entity, &DialogConfig), With<EditorDialog>>,
    mut commands: Commands,
) {
    if KeymapCapture::is_recording(capture.as_deref()) {
        return;
    }
    if !keyboard.just_pressed(KeyCode::Escape) {
        return;
    }

    for (entity, config) in &dialogs {
        if config.close_on_esc {
            dismiss_dialog(&mut commands, entity);
        }
    }
}

fn on_close_dialog(
    _event: On<CloseDialogEvent>,
    dialogs: Query<Entity, With<EditorDialog>>,
    mut commands: Commands,
) {
    for entity in &dialogs {
        dismiss_dialog(&mut commands, entity);
    }
}

fn on_action_button_click(
    event: On<ButtonClickEvent>,
    action_buttons: Query<&ChildOf, With<DialogActionButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<Entity, With<EditorDialog>>,
    mut commands: Commands,
) {
    let Ok(button_parent) = action_buttons.get(event.entity) else {
        return;
    };

    if let Some(dialog_entity) = find_dialog_ancestor(button_parent.parent(), &parents, &dialogs) {
        commands.trigger(DialogActionEvent {
            entity: dialog_entity,
        });
        dismiss_dialog(&mut commands, dialog_entity);
    }
}

fn on_secondary_action_button_click(
    event: On<ButtonClickEvent>,
    secondary_buttons: Query<&ChildOf, With<DialogSecondaryActionButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<Entity, With<EditorDialog>>,
    mut commands: Commands,
) {
    let Ok(button_parent) = secondary_buttons.get(event.entity) else {
        return;
    };

    if let Some(dialog_entity) = find_dialog_ancestor(button_parent.parent(), &parents, &dialogs) {
        commands.trigger(DialogSecondaryActionEvent {
            entity: dialog_entity,
        });
        dismiss_dialog(&mut commands, dialog_entity);
    }
}

fn on_cancel_button_click(
    event: On<ButtonClickEvent>,
    cancel_buttons: Query<&ChildOf, With<DialogCancelButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<Entity, With<EditorDialog>>,
    mut commands: Commands,
) {
    let Ok(button_parent) = cancel_buttons.get(event.entity) else {
        return;
    };

    if let Some(dialog_entity) = find_dialog_ancestor(button_parent.parent(), &parents, &dialogs) {
        dismiss_dialog(&mut commands, dialog_entity);
    }
}

fn on_close_button_click(
    event: On<ButtonClickEvent>,
    close_buttons: Query<&ChildOf, With<DialogCloseButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<Entity, With<EditorDialog>>,
    mut commands: Commands,
) {
    let Ok(button_parent) = close_buttons.get(event.entity) else {
        return;
    };

    if let Some(dialog_entity) = find_dialog_ancestor(button_parent.parent(), &parents, &dialogs) {
        dismiss_dialog(&mut commands, dialog_entity);
    }
}

fn find_dialog_ancestor(
    start: Entity,
    parents: &Query<&ChildOf>,
    dialogs: &Query<Entity, With<EditorDialog>>,
) -> Option<Entity> {
    let mut current = start;
    loop {
        if dialogs.get(current).is_ok() {
            return Some(current);
        }
        let Ok(child_of) = parents.get(current) else {
            return None;
        };
        current = child_of.parent();
    }
}
