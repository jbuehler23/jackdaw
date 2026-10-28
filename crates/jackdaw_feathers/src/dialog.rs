use bevy::feathers::controls::{
    FeathersButton, FeathersDialog, FeathersDialogBody, FeathersDialogClose, FeathersDialogFooter,
    FeathersDialogHeader,
};
use bevy::feathers::display::{caption, label_dim};
use bevy::feathers::theme::ThemeBackgroundColor;
use bevy::prelude::*;
use bevy::ui_widgets::RequestClose;
use jackdaw_commands::KeymapCapture;

use crate::button::{ButtonClickEvent, ButtonVariant, EditorButton};
use crate::tokens::DIALOG_BACKDROP;

const DEFAULT_WIDTH: Val = Val::Px(448.0);

pub fn plugin(app: &mut App) {
    app.add_observer(on_open_dialog)
        .add_observer(on_open_confirmation_dialog)
        .add_observer(on_dialog_button_click)
        .add_observer(on_close_button)
        .add_observer(on_close_dialog)
        .add_observer(handle_backdrop_click)
        .add_systems(Update, handle_esc_key)
        .add_systems(
            PostUpdate,
            sync_children_slot_visibility.in_set(crate::WidgetSetupSystems),
        );
}

/// The root of an open dialog: the feathers modal barrier, which holds the
/// dialog panel.
#[derive(Component, Default, Clone)]
pub struct EditorDialog;

/// What a dialog is asking and what it will take for an answer.
///
/// The footer's buttons carry their labels in `Text` children several levels
/// down, which is enough to draw them and not enough to answer one from anywhere
/// but a pointer. Recorded here at open time so a caller with no pointer can read
/// the question and name a button.
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
/// A label matches exactly first, so a button literally spelled `1` is reachable
/// by name. Failing that, a value that parses as an integer is an index. Failing
/// that, one case-insensitive prefix match answers, so `Reload` presses a button
/// spelled `Reload (discards your unsaved changes)`.
///
/// A prefix that fits two buttons presses neither and comes back as
/// [`DialogChoiceError::Ambiguous`] naming both.
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

/// Press one of `dialog`'s buttons, as a click on it would: fire the event the
/// button fires, then take the dialog down. Cancel fires
/// [`DialogDismissEvent`], which is what makes a dismissal and a cancel the
/// same answer.
pub fn answer_dialog(commands: &mut Commands, dialog: Entity, choice: DialogChoice) {
    match choice {
        DialogChoice::Action => {
            commands.trigger(DialogActionEvent { entity: dialog });
            dismiss_dialog(commands, dialog);
        }
        DialogChoice::Secondary => {
            commands.trigger(DialogSecondaryActionEvent { entity: dialog });
            dismiss_dialog(commands, dialog);
        }
        DialogChoice::Cancel => cancel_dialog(commands, dialog),
    }
}

/// A footer button, and the answer pressing it gives.
#[derive(Component, Clone)]
struct DialogButton(DialogChoice);

#[derive(Component, Default, Clone)]
struct DialogCloseButton;

/// The body section a caller fills with the dialog's own content. Hidden
/// while empty.
#[derive(Component, Default, Clone)]
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

#[derive(Component, Clone)]
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

/// A dialog going down unanswered: cancelled, closed, dismissed with Escape or
/// a click outside, or closed from code.
#[derive(EntityEvent)]
pub struct DialogDismissEvent {
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
    /// The panel's width; the editor's default dialog width when `None`.
    pub max_width: Option<Val>,
    /// Padding around the caller's content; the feathers body padding when
    /// `None`.
    pub content_padding: Option<UiRect>,
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
            content_padding: None,
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
        self.content_padding = Some(UiRect::ZERO);
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
    existing: Query<(), With<EditorDialog>>,
) {
    if !existing.is_empty() {
        return;
    }
    commands.spawn_scene(dialog_scene(&event));
}

fn on_open_confirmation_dialog(
    event: On<OpenConfirmationDialogEvent>,
    mut commands: Commands,
    existing: Query<(), With<EditorDialog>>,
) {
    if !existing.is_empty() {
        return;
    }
    let dialog_event: OpenDialogEvent = event.event().into();
    commands.spawn_scene(dialog_scene(&dialog_event));
}

/// A [`FeathersDialog`] holding a header, the caller's content slot and a
/// footer of feathers buttons.
///
/// The editor keeps its own layer and backdrop: dialogs sit above the editor's
/// popovers and drag previews, and feathers leaves the barrier unpainted.
fn dialog_scene(event: &OpenDialogEvent) -> impl Scene {
    let mut contents: Vec<Box<dyn SceneList>> = Vec::new();
    if event.title.is_some() || event.has_close_button {
        contents.push(Box::new(header(
            event.title.clone().unwrap_or_default(),
            event.has_close_button,
        )));
    }
    if let Some(description) = event.description.clone() {
        contents.push(Box::new(bsn_list! {
            @FeathersDialogBody
            Children [ @{label_dim(description)} ]
        }));
    }
    contents.push(Box::new(children_slot(event.content_padding)));

    let buttons: Vec<Box<dyn SceneList>> = [
        (
            &event.secondary_action,
            DialogChoice::Secondary,
            ButtonVariant::Default,
        ),
        (&event.cancel, DialogChoice::Cancel, ButtonVariant::Default),
        (
            &event.action,
            DialogChoice::Action,
            event.variant.action_button_variant(),
        ),
    ]
    .into_iter()
    .filter_map(|(label, choice, variant)| {
        label.clone().map(|label| {
            Box::new(bsn_list! { @{dialog_button(label, choice, variant)} }) as Box<dyn SceneList>
        })
    })
    .collect();
    if !buttons.is_empty() {
        contents.push(Box::new(bsn_list! {
            @FeathersDialogFooter
            Children [ {buttons} ]
        }));
    }

    let contents: Box<dyn SceneList> = Box::new(contents);
    let width = event.max_width.unwrap_or(DEFAULT_WIDTH);
    let choices = DialogChoices {
        title: event.title.clone(),
        description: event.description.clone(),
        action: event.action.clone(),
        secondary: event.secondary_action.clone(),
        cancel: event.cancel.clone(),
    };
    let config = DialogConfig {
        close_on_click_outside: event.close_on_click_outside,
        close_on_esc: event.close_on_esc,
    };
    let variant = event.variant;
    bsn! {
        @FeathersDialog {
            @width: {width},
            @contents: {contents},
        }
        EditorDialog
        ~{choices}
        ~{variant}
        ~{config}
        GlobalZIndex(200)
        BackgroundColor(DIALOG_BACKDROP)
    }
}

fn header(title: String, close_button: bool) -> impl SceneList {
    let close: Box<dyn SceneList> = if close_button {
        Box::new(bsn_list! { @FeathersDialogClose DialogCloseButton })
    } else {
        Box::new(bsn_list! {})
    };
    bsn_list! {
        @FeathersDialogHeader
        Children [
            @{caption(title)}
            --
            {close}
        ]
    }
}

fn children_slot(padding: Option<UiRect>) -> impl SceneList {
    let padding = padding.map(|padding| bsn! { Node { padding: {padding} } });
    bsn_list! {
        @FeathersDialogBody
        @{padding}
        DialogChildrenSlot
        Node {
            display: Display::None,
            row_gap: px(12),
        }
    }
}

/// A footer button. A secondary action stands apart at the far left.
fn dialog_button(label: String, choice: DialogChoice, variant: ButtonVariant) -> impl Scene {
    let margin = if choice == DialogChoice::Secondary {
        UiRect::right(Val::Auto)
    } else {
        UiRect::ZERO
    };
    let background = variant
        .background_token(false)
        .map(|token| bsn! { ~{ThemeBackgroundColor(token)} });
    bsn! {
        @FeathersButton {
            @caption: bsn! { @{caption(label)} },
            @variant: {variant.feathers()},
        }
        @{background}
        Node { margin: {margin} }
        EditorButton
        ~{variant}
        ~{DialogButton(choice)}
    }
}

fn dismiss_dialog(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).try_despawn();
}

/// Take a dialog down with its question unanswered, telling whoever put it up.
fn cancel_dialog(commands: &mut Commands, entity: Entity) {
    commands.trigger(DialogDismissEvent { entity });
    dismiss_dialog(commands, entity);
}

fn dialog_of(
    entity: Entity,
    parents: &Query<&ChildOf>,
    dialogs: &Query<(), With<EditorDialog>>,
) -> Option<Entity> {
    parents
        .iter_ancestors(entity)
        .find(|ancestor| dialogs.contains(*ancestor))
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

/// A primary press on the barrier itself, not on the panel above it, cancels
/// a dialog that closes on a click outside.
fn handle_backdrop_click(
    press: On<PointerPress>,
    dialogs: Query<&DialogConfig, With<EditorDialog>>,
    mut commands: Commands,
) {
    if press.button != PointerButton::Primary || press.original_event_target() != press.entity {
        return;
    }
    let Ok(config) = dialogs.get(press.entity) else {
        return;
    };
    if config.close_on_click_outside {
        cancel_dialog(&mut commands, press.entity);
    }
}

/// Escape closes a dialog -- except while the keybind settings are recording, when
/// the press belongs to the recorder. Without this Escape could never be bound to
/// anything.
///
/// The feathers barrier also asks to close on Escape and on a click outside; that
/// request is left unanswered so these two rules, and the recorder, decide.
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
            cancel_dialog(&mut commands, entity);
        }
    }
}

fn on_close_dialog(
    _event: On<CloseDialogEvent>,
    dialogs: Query<Entity, With<EditorDialog>>,
    mut commands: Commands,
) {
    for entity in &dialogs {
        cancel_dialog(&mut commands, entity);
    }
}

fn on_dialog_button_click(
    event: On<ButtonClickEvent>,
    buttons: Query<&DialogButton>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut commands: Commands,
) {
    let Ok(DialogButton(choice)) = buttons.get(event.entity) else {
        return;
    };
    if let Some(dialog) = dialog_of(event.entity, &parents, &dialogs) {
        answer_dialog(&mut commands, dialog, *choice);
    }
}

/// The header's close cross asks to close through feathers; that is a cancel.
fn on_close_button(
    close: On<RequestClose>,
    close_buttons: Query<(), With<DialogCloseButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut commands: Commands,
) {
    let button = close.event_target();
    if !close_buttons.contains(button) {
        return;
    }
    if let Some(dialog) = dialog_of(button, &parents, &dialogs) {
        cancel_dialog(&mut commands, dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::feathers::tokens;
    use bevy::ui_widgets::{Activate, ModalDialog, ModalDialogBarrier};

    #[derive(Resource, Default)]
    struct Answers {
        actions: usize,
        dismissals: usize,
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            crate::button::plugin,
            plugin,
        ));
        app.init_asset::<Font>();
        app.init_asset::<Image>();
        app.init_resource::<Answers>();
        app.add_observer(|_: On<DialogActionEvent>, mut answers: ResMut<Answers>| {
            answers.actions += 1;
        });
        app.add_observer(|_: On<DialogDismissEvent>, mut answers: ResMut<Answers>| {
            answers.dismissals += 1;
        });
        app
    }

    fn open(app: &mut App, event: OpenDialogEvent) -> Entity {
        app.world_mut().trigger(event);
        app.world_mut().flush();
        dialogs(app).first().copied().expect("a dialog is up")
    }

    fn dialogs(app: &mut App) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<EditorDialog>>()
            .iter(app.world())
            .collect()
    }

    fn find<C: Component>(world: &World, root: Entity) -> Vec<Entity> {
        let mut found = Vec::new();
        let mut pending = vec![root];
        while let Some(entity) = pending.pop() {
            if world.get::<C>(entity).is_some() {
                found.push(entity);
            }
            if let Some(children) = world.get::<Children>(entity) {
                pending.extend(children.iter().rev());
            }
        }
        found
    }

    fn texts(world: &World, root: Entity) -> Vec<String> {
        find::<Text>(world, root)
            .into_iter()
            .filter_map(|entity| world.get::<Text>(entity).map(|text| text.0.clone()))
            .collect()
    }

    fn action_button(world: &World, root: Entity) -> Entity {
        find::<DialogButton>(world, root)
            .into_iter()
            .find(|button| {
                matches!(
                    world.get::<DialogButton>(*button),
                    Some(DialogButton(DialogChoice::Action))
                )
            })
            .expect("the dialog has an action button")
    }

    #[test]
    fn a_dialog_is_built_from_feathers_parts_on_the_frame_it_opens() {
        let mut app = app();
        let root = open(
            &mut app,
            OpenDialogEvent::new("Scene Changed on Disk", "Reload")
                .with_description("level.bsn changed on disk.")
                .with_secondary_action("Keep Both"),
        );
        let world = app.world();

        let dialog = world.entity(root);
        assert!(dialog.contains::<FeathersDialog>());
        assert!(dialog.contains::<ModalDialogBarrier>());

        let panel = find::<ModalDialog>(world, root);
        assert_eq!(panel.len(), 1, "one feathers panel");
        assert_eq!(
            world
                .get::<ThemeBackgroundColor>(panel[0])
                .map(|bg| bg.0.clone()),
            Some(tokens::DIALOG_BG),
            "painted from the feathers dialog token",
        );

        let header = find::<FeathersDialogHeader>(world, root);
        assert_eq!(header.len(), 1);
        assert_eq!(texts(world, header[0]), vec!["Scene Changed on Disk"]);
        let close = find::<DialogCloseButton>(world, header[0]);
        assert_eq!(close.len(), 1, "the header holds the close cross");
        assert!(world.entity(close[0]).contains::<FeathersDialogClose>());

        let bodies = find::<FeathersDialogBody>(world, root);
        assert_eq!(bodies.len(), 2, "the description and the content slot");
        assert_eq!(texts(world, bodies[0]), vec!["level.bsn changed on disk."]);
        assert!(world.entity(bodies[1]).contains::<DialogChildrenSlot>());
        assert_eq!(
            world.get::<Node>(bodies[1]).map(|node| node.display),
            Some(Display::None),
            "the slot is hidden until a caller fills it",
        );

        let footer = find::<FeathersDialogFooter>(world, root);
        assert_eq!(footer.len(), 1);
        let buttons = find::<FeathersButton>(world, footer[0]);
        let captions: Vec<String> = buttons
            .iter()
            .flat_map(|button| texts(world, *button))
            .collect();
        assert_eq!(captions, vec!["Keep Both", "Cancel", "Reload"]);
        let nodes: Vec<&Node> = buttons
            .iter()
            .filter_map(|button| world.get::<Node>(*button))
            .collect();
        assert_eq!(
            nodes[0].margin.right,
            Val::Auto,
            "the secondary stands apart"
        );
        assert!(
            nodes
                .iter()
                .all(|node| node.height == nodes[1].height && node.height != Val::Auto),
            "every button keeps the feathers frame",
        );
    }

    #[test]
    fn a_destructive_action_is_painted_red_from_its_first_frame() {
        let mut app = app();
        let root = open(
            &mut app,
            (&OpenConfirmationDialogEvent::new("Delete Scene", "Delete")).into(),
        );
        let world = app.world();
        let action = action_button(world, root);
        assert_eq!(
            world
                .get::<ThemeBackgroundColor>(action)
                .map(|bg| bg.0.clone()),
            Some(crate::button::BUTTON_DESTRUCTIVE_BG),
        );
    }

    #[test]
    fn pressing_the_primary_button_answers_with_the_action() {
        let mut app = app();
        let root = open(&mut app, OpenDialogEvent::new("Save Changes", "Save"));
        let world = app.world();
        let action = action_button(world, root);

        app.world_mut().trigger(Activate { entity: action });
        app.world_mut().flush();

        assert!(dialogs(&mut app).is_empty());
        let answers = app.world().resource::<Answers>();
        assert_eq!((answers.actions, answers.dismissals), (1, 0));
    }

    #[test]
    fn the_close_cross_cancels_the_dialog() {
        let mut app = app();
        let root = open(&mut app, OpenDialogEvent::new("Save Changes", "Save"));
        let close = find::<DialogCloseButton>(app.world(), root)[0];

        app.world_mut().trigger(RequestClose { source: close });
        app.world_mut().flush();

        assert!(dialogs(&mut app).is_empty());
        let answers = app.world().resource::<Answers>();
        assert_eq!((answers.actions, answers.dismissals), (0, 1));
    }

    /// The barrier asks to close on Escape and on a click outside. A dialog that
    /// must be answered stays up through both.
    #[test]
    fn the_barrier_asking_to_close_leaves_a_dialog_that_must_be_answered() {
        let mut app = app();
        let root = open(
            &mut app,
            (&OpenConfirmationDialogEvent::new("Delete Scene", "Delete")).into(),
        );

        app.world_mut().trigger(RequestClose { source: root });
        app.world_mut().flush();

        assert_eq!(dialogs(&mut app), vec![root]);
        assert_eq!(app.world().resource::<Answers>().dismissals, 0);
    }
}
