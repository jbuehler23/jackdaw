//! Editing a node's text on the canvas, where it is drawn.
//!
//! A double click on a node carrying `Text` opens a text entry over that
//! node's rect, seeded with what it says and with the whole of it selected,
//! so typing replaces it. Enter and a click away commit; Escape puts back
//! what was there.
//!
//! The entry is editor chrome parented into the panel's stage, like the
//! selection outline, so it follows the canvas as it is panned and zoomed
//! and nothing it does can reach the authored tree except through the
//! commit.
//!
//! The commit goes through the same field-set path the inspector's own text
//! row uses, so it authors the document, mints exactly one history entry,
//! and streams to a running game. A commit that changes nothing writes
//! nothing at all.

use bevy::{
    feathers::controls::{FeathersTextInput, FeathersTextInputContainer},
    input::keyboard::KeyboardInput,
    input_focus::{FocusCause, FocusedInput, InputFocus},
    picking::prelude::Pickable,
    prelude::*,
    text::{EditableText, TextEdit},
    ui_widgets::ValueChange,
};

use crate::{EditorEntity, ui_stage::node_overlay_rect};

/// The entry open over one node's text, if any.
///
/// One at a time: a second double click commits the first, because two
/// entries over one canvas would both claim the keyboard and only one of
/// them would be the one the user is looking at.
#[derive(Resource, Default)]
pub struct TextEditSession {
    open: Option<OpenEdit>,
}

impl TextEditSession {
    /// The authored node being edited, or `None` when no entry is open.
    pub fn editing(&self) -> Option<Entity> {
        self.open.as_ref().map(|open| open.node)
    }
}

struct OpenEdit {
    /// The authored node whose `Text` is being edited.
    node: Entity,
    /// The panel content entity carrying the stage the entry is drawn on.
    host: Entity,
    /// The chrome: the container, and the entry inside it.
    overlay: Entity,
    input: Entity,
    /// What the node said when the entry opened, for Escape and for the
    /// "changed nothing" test the commit makes.
    before: String,
    /// Whether the entry has actually held the keyboard yet.
    ///
    /// Focus does not settle on the frame the entry is spawned: the press
    /// that opened it is still being handled, and whatever that press moves
    /// the focus to arrives after. So the entry asks for the focus until it
    /// has it, and only once it has held it does losing it mean a click
    /// away -- which is the commit.
    focused: bool,
}

/// Marker on the container, so a test can find what is drawn.
#[derive(Component)]
pub struct TextEditOverlay {
    /// The panel content entity carrying this stage's viewport host.
    pub host: Entity,
}

/// Draw order of the entry: over the outline it replaces for the moment.
const EDITOR_Z: i32 = 60;

pub struct UiTextEditPlugin;

impl Plugin for UiTextEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TextEditSession>()
            .add_observer(on_entry_changed)
            .add_systems(Update, (cancel_text_edit, sync_text_edit_overlay).chain());
    }
}

/// Whether `entity` carries text this editor can edit.
pub fn is_editable_text(world: &World, entity: Entity) -> bool {
    world.get::<Text>(entity).is_some() && world.get::<EditorEntity>(entity).is_none()
}

/// Open the entry over `node`, closing whatever was open before.
///
/// A second double click while an entry is open commits the first: the
/// user has moved on, and the text they typed is what they meant.
pub fn open_text_editor(world: &mut World, node: Entity, host: Entity) {
    if world.resource::<TextEditSession>().open.is_some() {
        commit_text_edit(world);
    }
    let Some(before) = world.get::<Text>(node).map(|text| text.0.clone()) else {
        return;
    };
    let Some(stage) = world
        .get::<crate::viewport_2d::Viewport2dPanelHost>(host)
        .map(|host| host.stage)
    else {
        return;
    };

    let Ok(mut overlay) = world.spawn_scene(entry_scene()) else {
        return;
    };
    overlay.insert((
        TextEditOverlay { host },
        EditorEntity,
        ZIndex(EDITOR_Z),
        ChildOf(stage),
    ));
    let overlay = overlay.id();
    // The container's own node is the frame the entry is drawn in; the
    // sync pass puts it over the node's rect on the next frame, and this
    // is what makes it absolute so that placement means anything.
    if let Some(mut node) = world.get_mut::<Node>(overlay) {
        node.position_type = PositionType::Absolute;
    }

    let Some(input) = descendant_with_editable_text(world, overlay) else {
        world.entity_mut(overlay).despawn();
        return;
    };
    if let Some(mut editable) = world.get_mut::<EditableText>(input) {
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(before.clone().into()));
        editable.queue_edit(TextEdit::SelectAll);
    }
    world
        .resource_mut::<InputFocus>()
        .set(input, FocusCause::Navigated);

    world.resource_mut::<TextEditSession>().open = Some(OpenEdit {
        node,
        host,
        overlay,
        input,
        before,
        focused: false,
    });
}

/// The container framing the entry, the same shape the inspector's string
/// row is built from.
fn entry_scene() -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        Children [
            @FeathersTextInput
            on(entry_on_enter_key)
        ]
    }
}

/// Enter is a commit: emit the final value the observer writes back.
fn entry_on_enter_key(
    key_input: On<FocusedInput<KeyboardInput>>,
    entries: Query<&EditableText>,
    mut commands: Commands,
) {
    if key_input.input.key_code != KeyCode::Enter {
        return;
    }
    let entry = key_input.event_target();
    if let Ok(editable) = entries.get(entry) {
        commands.trigger(ValueChange {
            source: entry,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

fn on_entry_changed(change: On<ValueChange<String>>, mut commands: Commands) {
    let source = change.source;
    let value = change.value.clone();
    commands.queue(move |world: &mut World| {
        let ours = world
            .resource::<TextEditSession>()
            .open
            .as_ref()
            .is_some_and(|open| open.input == source);
        if !ours {
            return;
        }
        write_text_edit(world, value);
    });
}

/// The entity under `overlay` that holds the editable buffer.
fn descendant_with_editable_text(world: &World, overlay: Entity) -> Option<Entity> {
    if world.get::<EditableText>(overlay).is_some() {
        return Some(overlay);
    }
    for child in world
        .get::<Children>(overlay)
        .into_iter()
        .flatten()
        .copied()
    {
        if let Some(found) = descendant_with_editable_text(world, child) {
            return Some(found);
        }
    }
    None
}

/// Commit whatever the entry currently holds.
pub fn commit_text_edit(world: &mut World) {
    let Some(input) = world
        .resource::<TextEditSession>()
        .open
        .as_ref()
        .map(|open| open.input)
    else {
        return;
    };
    let value = world
        .get::<EditableText>(input)
        .map(|editable| editable.value().to_string());
    match value {
        Some(value) => write_text_edit(world, value),
        None => close_text_edit(world),
    }
}

/// Write `value` onto the edited node through the inspector's own field
/// path, then take the entry down.
///
/// Text that came back the same as it went in writes nothing: an entry
/// opened and dismissed is not an edit, and a history entry for it would
/// have to be undone before the real one.
fn write_text_edit(world: &mut World, value: String) {
    let Some((node, before)) = world
        .resource::<TextEditSession>()
        .open
        .as_ref()
        .map(|open| (open.node, open.before.clone()))
    else {
        return;
    };
    close_text_edit(world);
    if value == before {
        return;
    }
    crate::selection::select_for_edit(world, node);
    crate::commands::field_edit_commit(
        world,
        Text::type_path(),
        "0",
        &serde_json::Value::String(value),
        TEXT_EDIT_LABEL,
    );
}

/// Undo label an in-place text edit lands under.
const TEXT_EDIT_LABEL: &str = "Edit text";

/// Take the entry down without writing anything.
pub fn cancel_text_edit(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<TextEditSession>,
    mut commands: Commands,
) {
    if session.open.is_none() || !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    commands.queue(|world: &mut World| {
        // The node was never written while the entry was open, so putting
        // it back is only a matter of taking the entry away.
        close_text_edit(world);
    });
}

/// Keep the keyboard on the open entry, and commit when it is taken away.
///
/// Says whether the entry is still open afterwards.
fn follow_focus(world: &mut World) -> bool {
    let Some((input, focused)) = world
        .resource::<TextEditSession>()
        .open
        .as_ref()
        .map(|open| (open.input, open.focused))
    else {
        return false;
    };
    let holds = world.resource::<InputFocus>().get() == Some(input);
    match (holds, focused) {
        (true, false) => {
            if let Some(open) = world.resource_mut::<TextEditSession>().open.as_mut() {
                open.focused = true;
            }
        }
        (false, false) => {
            world
                .resource_mut::<InputFocus>()
                .set(input, FocusCause::Navigated);
        }
        (false, true) => {
            commit_text_edit(world);
            return false;
        }
        (true, true) => {}
    }
    true
}

fn close_text_edit(world: &mut World) {
    let Some(open) = world.resource_mut::<TextEditSession>().open.take() else {
        return;
    };
    if world.get_entity(open.input).is_ok()
        && world.resource::<InputFocus>().get() == Some(open.input)
    {
        world.resource_mut::<InputFocus>().clear();
    }
    if let Ok(entity) = world.get_entity_mut(open.overlay) {
        entity.despawn();
    }
}

/// Keep the entry over the rect of the node it is editing, so it follows a
/// pan or a zoom made while it is open, and goes when the node does.
fn sync_text_edit_overlay(world: &mut World) {
    let Some((node, host, overlay)) = world
        .resource::<TextEditSession>()
        .open
        .as_ref()
        .map(|open| (open.node, open.host, open.overlay))
    else {
        return;
    };
    if world.get_entity(node).is_err() {
        close_text_edit(world);
        return;
    }
    if !follow_focus(world) {
        return;
    }
    let Some(rect) = node_overlay_rect(world, node, host) else {
        return;
    };
    if let Some(mut value) = world.get_mut::<Node>(overlay) {
        value.left = px(rect.min.x);
        value.top = px(rect.min.y);
        value.width = px(rect.width().max(1.0));
        value.height = px(rect.height().max(1.0));
    }
    // The frame lies over the node it is editing; the press that dismisses
    // it belongs to whatever is under it, not to the frame.
    if let Ok(mut entity) = world.get_entity_mut(overlay)
        && entity.get::<Pickable>().is_none()
    {
        entity.insert(Pickable::default());
    }
}
