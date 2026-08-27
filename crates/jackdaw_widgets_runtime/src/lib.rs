//! Value behaviour and load-time defaults for authored UI widgets.
//!
//! `bevy_ui_widgets` is external-state: a checkbox, radio group, or slider
//! *emits* [`ValueChange`] and changes nothing about itself. Turning that into
//! state is an opt-in observer the consumer adds. The Jackdaw editor and a game
//! built on `jackdaw_runtime` both need the widget markers to survive a load and
//! the value observers to be attached; this crate holds both so the two sides
//! cannot drift.
//!
//! # Text
//!
//! Every other widget value is a reflected component, so a save writes it and a
//! load puts it back. `bevy_text::EditableText` holds the string inside a
//! `parley::PlainEditor` and derives no `Reflect`, so a text input round-trips
//! as an empty box. [`TextValue`] is the reflectable half, and two
//! equality-guarded systems keep it and the editor in step. A typed edit is
//! announced as a `ValueChange<String>`, the same way a checkbox announces a
//! toggle, so something outside this crate can act on it.
//!
//! # Theme
//!
//! Under the `feathers` feature the plugin installs the standard dark theme when
//! nothing else has. An empty `UiTheme` is not a neutral one: it answers every
//! design token with the missing-token colour. A game that installs its own
//! theme keeps it, whenever it installs it.
//!
//! # Global observers rather than `observe()` per widget
//!
//! A per-entity `observe()` attaches an observer *entity* watching that target.
//! It is not a component, so nothing writes it to the document and nothing
//! recreates it on load, leaving a widget inert after the session that spawned
//! it. Global observers keyed on the widget markers are attached to the app
//! rather than to any one entity.
//!
//! # Observer gating
//!
//! The Extensions dialog, the material panel, and the inspector run their own
//! checkbox state machines, some of which refuse a toggle they cannot honour, so
//! a blanket self-update would check boxes the editor left alone. The gate is
//! [`AuthoredWidget`]: whoever spawns authored content puts it on, and the
//! observers answer for nothing else. The editor mirrors it onto every entity
//! its scene document has a node for; `jackdaw_runtime` puts it on every entity
//! a scene spawns.

#![deny(missing_docs)]

use bevy::{
    prelude::*,
    text::{EditableText, EditableTextSystems, TextCursorStyle, TextEdit},
    ui::Checked,
    ui_widgets::{Button, Checkbox, RadioButton, RadioGroup, Slider, SliderValue, ValueChange},
};

/// The text an authored input holds, in a form a scene document can carry.
///
/// `bevy_text::EditableText` owns a `parley::PlainEditor` and does not derive
/// `Reflect`, so it cannot be written to a document. This is the reflectable
/// half: the string lives here, the editing state stays where it is, and
/// [`AuthoredWidgetPlugin`] keeps the two in step. It is also what a binding
/// reads and writes; a bind path names `TextValue.0`, never the editor.
///
/// It requires [`EditableText`] because a load has nothing else to rebuild the
/// input from. [`TextCursorStyle`] comes with it for the same reason and one
/// more: `bevy_ui_render`'s editable-text extract queries it *without* an
/// `Option`, so an input rebuilt without one draws no caret and no selection.
/// These defaults match `bevy_feathers`' own text input until a theme changes
/// them.
#[derive(Component, Reflect, Default, Debug, Clone, PartialEq, Eq)]
#[reflect(Component, Default)]
#[require(EditableText, TextCursorStyle)]
pub struct TextValue(
    /// The text the input holds.
    pub String,
);

/// Where a binding writes a widget's text: [`TextValue`]'s only field, spelled
/// the way a bind path spells one.
///
/// `jackdaw_bind` does not depend on this crate and bevy has no component
/// holding a text input's value, so the path is taken from the type here rather
/// than written out on the binding side.
pub fn text_value_write_path() -> String {
    format!("{}.0", <TextValue as bevy::reflect::TypePath>::type_path())
}

/// Marks an entity as authored content: something a scene document produced,
/// as opposed to chrome the host application built for itself.
///
/// [`AuthoredWidgetPlugin`]'s self-update observers answer only for entities
/// carrying this. It holds no data and is not [`Reflect`], so it cannot reach a
/// scene document: each side re-derives it on spawn.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthoredWidget;

/// The `PostUpdate` systems that reconcile [`TextValue`] with the editor
/// beside it: an edit into the value, then a value written from outside into
/// the box.
///
/// Named so it can be ordered from outside: anything that writes `TextValue`
/// belongs ahead of this pair, or a frame can reconcile the typed edit and only
/// then take a write of the value the source held before it. `bevy_text`'s
/// `EditableTextSystems` sits inside `UiSystems::Content`, which is chained
/// ahead of `Layout`, so waiting on it does not by itself place this pair
/// relative to a binding evaluator. Whoever composes this crate with one
/// declares that edge; see `jackdaw_runtime`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthoredTextSystems;

/// Registers the widget defaults and attaches the self-update observers.
///
/// Add this wherever authored UI is spawned. Whatever spawns that UI is
/// responsible for putting [`AuthoredWidget`] on it; nothing here fires for an
/// unmarked entity.
pub struct AuthoredWidgetPlugin;

impl Plugin for AuthoredWidgetPlugin {
    fn build(&self, app: &mut App) {
        register_widget_defaults(app);
        app.add_observer(authored_checkbox_self_update)
            .add_observer(authored_radio_self_update)
            .add_observer(authored_slider_self_update);
        app.add_systems(
            PostUpdate,
            (authored_text_self_update, authored_text_follows_value)
                .chain()
                .in_set(AuthoredTextSystems)
                .after(EditableTextSystems)
                .before(bevy::ui::UiSystems::Layout),
        );
        #[cfg(feature = "feathers")]
        app.add_systems(Update, (hydrate_button_hover, authored_check_styles));
    }

    /// Gives the app a theme if nothing else did.
    ///
    /// `FeathersCorePlugin` only calls `init_resource::<UiTheme>()`, and the
    /// default is an empty token map: every `ThemeBackgroundColor` an authored
    /// widget carries then resolves to the missing-token colour.
    ///
    /// This runs in `finish` rather than `build` because the app may add
    /// `FeathersPlugins` after this one, and the emptiness checked for here is
    /// only settled once every plugin has built.
    ///
    /// A theme installed later still wins: this writes the resource once and
    /// nothing here watches it afterwards.
    #[cfg(feature = "feathers")]
    fn finish(&self, app: &mut App) {
        use bevy::feathers::dark_theme::create_dark_theme;
        use bevy::feathers::theme::UiTheme;

        if app
            .world()
            .get_resource::<UiTheme>()
            .is_none_or(|theme| theme.0.color.is_empty())
        {
            app.insert_resource(UiTheme(create_dark_theme()));
        }
    }
}

/// Teaches the type registry how to build each widget marker from nothing.
///
/// Loading a component from a document builds it through `ReflectDefault`, and
/// neither `bevy_ui_widgets` nor `bevy_feathers` registers that data on its
/// markers, only `#[reflect(Component)]`. Without this the loader warns and
/// drops the marker, leaving a styled box with no widget behaviour in it.
///
/// [`AuthoredWidgetPlugin`] calls this; call it directly only when the
/// observers are unwanted.
pub fn register_widget_defaults(app: &mut App) {
    use bevy::reflect::prelude::ReflectDefault;
    use bevy::ui_widgets::{ScrollArea, SliderRange};

    app.register_type::<TextValue>();

    app.register_type::<Button>()
        .register_type::<Checkbox>()
        .register_type::<RadioButton>()
        .register_type::<RadioGroup>()
        .register_type::<Slider>()
        .register_type::<SliderValue>()
        .register_type::<SliderRange>()
        .register_type::<ScrollArea>();

    app.register_type_data::<Button, ReflectDefault>()
        .register_type_data::<Checkbox, ReflectDefault>()
        .register_type_data::<RadioButton, ReflectDefault>()
        .register_type_data::<RadioGroup, ReflectDefault>()
        .register_type_data::<Slider, ReflectDefault>()
        .register_type_data::<SliderValue, ReflectDefault>()
        .register_type_data::<SliderRange, ReflectDefault>()
        .register_type_data::<ScrollArea, ReflectDefault>();

    #[cfg(feature = "feathers")]
    {
        use bevy::feathers::cursor::EntityCursor;
        use bevy::feathers::theme::ThemedText;

        app.register_type::<ThemedText>()
            .register_type_data::<ThemedText, ReflectDefault>();

        // An authored widget names the cursor it wants, and `bevy_feathers`
        // registers none of its own types. Without this the loader cannot find
        // the enum behind `EntityCursor::System` and drops the cursor.
        app.register_type::<EntityCursor>();
    }
}

/// `update_button_styles` queries `&Hovered`, and `Hovered` is picking state no
/// document records. Without this a button loaded from disk keeps its resting
/// colour through every hover and press.
#[cfg(feature = "feathers")]
fn hydrate_button_hover(
    buttons: Query<
        Entity,
        (
            With<bevy::feathers::controls::ButtonVariant>,
            Without<bevy::picking::hover::Hovered>,
        ),
    >,
    mut commands: Commands,
) {
    for button in &buttons {
        commands
            .entity(button)
            .insert(bevy::picking::hover::Hovered::default());
    }
}

/// The resting and checked halves of one theme token pair.
///
/// Named pairs rather than a per-widget lookup because the three authored
/// two-state widgets are not distinguishable by their markers: a checkbox and
/// a toggle switch both carry [`Checkbox`], and only the token they were
/// spawned with says which is which. So the resting token an entity is
/// carrying is what selects its pair, and a widget themed with something else
/// entirely is left alone rather than guessed at.
#[cfg(feature = "feathers")]
type TokenPair = (
    bevy::feathers::theme::ThemeToken,
    bevy::feathers::theme::ThemeToken,
);

/// Background pairs: the checkbox box fills, and so does the switch track.
#[cfg(feature = "feathers")]
fn background_token_pairs() -> [TokenPair; 2] {
    use bevy::feathers::tokens;
    [
        (tokens::CHECKBOX_BG, tokens::CHECKBOX_BG_CHECKED),
        (tokens::SWITCH_BG, tokens::SWITCH_BG_CHECKED),
    ]
}

/// Border pairs. The radio's ring is the only part feathers themes on the one
/// entity an authored radio is, so the border is its whole checked treatment.
#[cfg(feature = "feathers")]
fn border_token_pairs() -> [TokenPair; 3] {
    use bevy::feathers::tokens;
    [
        (tokens::CHECKBOX_BORDER, tokens::CHECKBOX_BORDER_CHECKED),
        (tokens::SWITCH_BORDER, tokens::SWITCH_BORDER_CHECKED),
        (tokens::RADIO_BORDER, tokens::RADIO_BORDER_CHECKED),
    ]
}

/// The token `current` should be, given whether the widget is checked: its
/// pair's other half, or `None` when it already is that or belongs to no pair
/// here.
#[cfg(feature = "feathers")]
fn swapped_token(
    pairs: &[TokenPair],
    current: &bevy::feathers::theme::ThemeToken,
    checked: bool,
) -> Option<bevy::feathers::theme::ThemeToken> {
    let (resting, checked_token) = pairs
        .iter()
        .find(|(resting, checked_token)| current == resting || current == checked_token)?;
    let wanted = if checked { checked_token } else { resting };
    (wanted != current).then(|| wanted.clone())
}

/// Show an authored checkbox, radio, or toggle switch in its checked colours.
///
/// `bevy_feathers` switches these tokens from systems that walk a multi-entity
/// widget through private marker types (`CheckboxOutline` and friends are not
/// public). An authored widget is one entity carrying the outline's tokens, so
/// none of that reaches it and the box keeps its resting colours whatever
/// [`Checked`] says -- a binding could drive the state correctly while the
/// widget looked identical either way. This is the same swap, done on the one
/// entity there is, using feathers' own checked tokens so an authored widget
/// and a feathers one agree in a theme.
///
/// Hover and press treatments are deliberately not mirrored. Those tokens read
/// picking state the document does not carry, and a resting-versus-checked
/// difference is the one a person authoring a screen has to be able to see.
///
/// The theme components are immutable, so a change is a re-insert; the
/// equality guard in [`swapped_token`] keeps an idle frame quiet.
#[cfg(feature = "feathers")]
fn authored_check_styles(
    widgets: Query<
        (
            Entity,
            Has<bevy::ui::Checked>,
            Option<&bevy::feathers::theme::ThemeBackgroundColor>,
            Option<&bevy::feathers::theme::ThemeBorderColor>,
        ),
        (
            With<AuthoredWidget>,
            Or<(With<Checkbox>, With<RadioButton>)>,
        ),
    >,
    mut commands: Commands,
) {
    use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor};

    let backgrounds = background_token_pairs();
    let borders = border_token_pairs();
    for (entity, checked, background, border) in &widgets {
        if let Some(background) = background
            && let Some(token) = swapped_token(&backgrounds, &background.0, checked)
        {
            commands.entity(entity).insert(ThemeBackgroundColor(token));
        }
        if let Some(border) = border
            && let Some(token) = swapped_token(&borders, &border.0, checked)
        {
            commands.entity(entity).insert(ThemeBorderColor(token));
        }
    }
}

fn authored_checkbox_self_update(
    change: On<ValueChange<bool>>,
    checkboxes: Query<(), (With<Checkbox>, With<AuthoredWidget>)>,
    mut commands: Commands,
) {
    if checkboxes.get(change.source).is_err() {
        return;
    }
    let (target, checked) = (change.source, change.value);
    commands.queue(move |world: &mut World| set_checked(world, target, checked));
}

/// A widget can be despawned between the event and the command that answers it:
/// an inspector rebuild, an undo, a document swap. `EntityCommands` would panic
/// on the missing entity, so the lookup is fallible here.
fn set_checked(world: &mut World, entity: Entity, checked: bool) {
    let Ok(mut entity) = world.get_entity_mut(entity) else {
        return;
    };
    if checked {
        entity.insert(Checked);
    } else {
        entity.remove::<Checked>();
    }
}

/// A radio change is addressed to the group rather than the button, so this
/// only fires for an authored `RadioGroup`.
fn authored_radio_self_update(
    change: On<ValueChange<Entity>>,
    groups: Query<&Children, (With<RadioGroup>, With<AuthoredWidget>)>,
    radios: Query<Entity, With<RadioButton>>,
    mut commands: Commands,
) {
    let Ok(children) = groups.get(change.source) else {
        return;
    };
    let chosen = change.value;
    let members: Vec<Entity> = radios.iter_many(children).collect();
    commands.queue(move |world: &mut World| {
        for radio in members {
            set_checked(world, radio, radio == chosen);
        }
    });
}

/// `SliderValue` is immutable, so a sync is a re-insert. The equality guard
/// keeps a slider also driven by a two-way binding from re-inserting an agreed
/// value every frame.
fn authored_slider_self_update(
    change: On<ValueChange<f32>>,
    sliders: Query<&SliderValue, (With<Slider>, With<AuthoredWidget>)>,
    mut commands: Commands,
) {
    let Ok(current) = sliders.get(change.source) else {
        return;
    };
    if current.0 == change.value {
        return;
    }
    let (target, value) = (change.source, change.value);
    commands.queue(move |world: &mut World| {
        if let Ok(mut entity) = world.get_entity_mut(target) {
            entity.insert(SliderValue(value));
        }
    });
}

/// Carries a typed edit into [`TextValue`], the half a save writes and a
/// binding reads.
///
/// Runs after the edits queued this frame have been applied and ahead of
/// [`authored_text_follows_value`], so an edit made this frame leaves both
/// halves agreeing in the same frame. When both move at once the edit wins;
/// snapping the box back mid-word would take the caret with it.
///
/// A newly inserted editor is exempt. A load inserts both halves at once and
/// the editor arrives empty however much text the document carried, so taking
/// that as an edit would wipe the value on every load.
///
/// An edit is also announced as a [`ValueChange`], the only way something
/// outside this crate can hear it. A value written from outside raises nothing:
/// it is not an edit, and echoing it would hand whoever wrote it their own value
/// back.
///
/// `is_final` is false because a keystroke is the middle of typing. The editor's
/// own `ValueChange<String>` handlers return early unless the change is final,
/// so a final change from here would point the inspector write-back paths at
/// authored content.
fn authored_text_self_update(
    mut inputs: Query<(Entity, Ref<EditableText>, &mut TextValue), Changed<EditableText>>,
    mut commands: Commands,
) {
    for (entity, editable, mut value) in &mut inputs {
        if editable.is_added() {
            continue;
        }
        let typed: String = editable.value().into_iter().collect();
        if value.0 != typed {
            value.0.clone_from(&typed);
            commands.trigger(ValueChange {
                source: entity,
                value: typed,
                is_final: false,
            });
        }
    }
}

/// Puts a [`TextValue`] written from outside, by a load, a binding or game
/// code, into the box the user reads.
///
/// The equality guard keeps this from fighting
/// [`authored_text_self_update`]: writing the editor flags it, which would bring
/// the other system straight back round, so the write only happens when the two
/// disagree.
///
/// `set_text` clears whatever the IME was composing, so a value written while
/// the user is mid-composition drops the preedit. The preedit is not text yet
/// and cannot be merged, and holding the write until composition ends would
/// leave the value and the box disagreeing for as long as the user keeps typing.
fn authored_text_follows_value(
    mut inputs: Query<(&mut EditableText, &TextValue), Changed<TextValue>>,
) {
    for (mut editable, value) in &mut inputs {
        let shown: String = editable.value().into_iter().collect();
        if shown == value.0 {
            continue;
        }
        editable.editor_mut().set_text(&value.0);
        // Text set from outside leaves the caret wherever the old string put
        // it, which for a shorter string is out of the text entirely.
        editable.queue_edit(TextEdit::TextEnd(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How often each side was written, counted over whole frames. A pair that
    /// trades writes instead of settling shows up as a rising count.
    #[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
    struct Touches {
        value: usize,
        editor: usize,
    }

    fn count_touches(
        values: Query<(), Changed<TextValue>>,
        editors: Query<(), Changed<EditableText>>,
        mut touches: ResMut<Touches>,
    ) {
        touches.value += values.iter().count();
        touches.editor += editors.iter().count();
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(AuthoredWidgetPlugin)
            .init_resource::<Touches>()
            .add_systems(Last, count_touches);
        app
    }

    /// An input with both halves, run far enough that the spawn has stopped
    /// showing as a change.
    fn settled_input(app: &mut App, text: &str) -> Entity {
        let input = app
            .world_mut()
            .spawn((TextValue(text.to_string()), EditableText::new(text)))
            .id();
        app.update();
        app.update();
        *app.world_mut().resource_mut::<Touches>() = Touches::default();
        input
    }

    /// The pair with bevy's text pipeline under it. Every other test here runs
    /// the two systems alone, where a queued [`TextEdit`] stays pending: it is
    /// only text once `apply_text_edits` has run, the system
    /// [`EditableTextSystems`] names and this pair orders itself against.
    /// `authored_text_follows_value` queues an edit on every write it makes.
    fn text_pipeline_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::text::TextPlugin,
        ))
        .add_plugins(AuthoredWidgetPlugin)
        .init_resource::<Touches>()
        .add_systems(Last, count_touches);
        app
    }

    /// A keystroke as bevy delivers one: queued, then applied by the text
    /// pipeline. The value follows it and nothing writes again afterwards.
    #[test]
    fn a_keystroke_through_bevys_text_pipeline_reaches_the_value_and_settles() {
        let mut app = text_pipeline_app();
        let input = settled_input(&mut app, "");

        app.world_mut()
            .get_mut::<EditableText>(input)
            .expect("the input keeps its editor")
            .queue_edit(TextEdit::Insert("Ada".into()));
        app.update();
        app.update();

        assert_eq!(
            editor_text(app.world(), input),
            "Ada",
            "the pipeline applied the queued edit",
        );
        assert_eq!(
            app.world().get::<TextValue>(input).map(|v| v.0.clone()),
            Some("Ada".to_string()),
            "and the value followed the box",
        );
        assert_still(&mut app);
    }

    /// The other direction, where the queued caret fix-up is an edit the
    /// pipeline applies. That flags `EditableText` again, which is where a pair
    /// that did not agree would start trading writes.
    #[test]
    fn a_written_value_settles_once_the_pipeline_has_moved_the_caret() {
        let mut app = text_pipeline_app();
        let input = settled_input(&mut app, "Adalbert");

        app.world_mut()
            .get_mut::<TextValue>(input)
            .expect("the input keeps its value")
            .0 = "Bo".to_string();
        app.update();
        app.update();

        assert_eq!(editor_text(app.world(), input), "Bo");
        assert_eq!(
            app.world().get::<TextValue>(input).map(|v| v.0.clone()),
            Some("Bo".to_string()),
            "the shortened box did not read back over the value",
        );
        assert_still(&mut app);
    }

    fn editor_text(world: &World, entity: Entity) -> String {
        world
            .get::<EditableText>(entity)
            .expect("the input keeps its editor")
            .value()
            .into_iter()
            .collect()
    }

    fn set_editor_text(app: &mut App, entity: Entity, text: &str) {
        app.world_mut()
            .get_mut::<EditableText>(entity)
            .expect("the input keeps its editor")
            .editor_mut()
            .set_text(text);
    }

    /// Runs on past the change and asserts neither side writes again.
    fn assert_still(app: &mut App) {
        let settled = *app.world().resource::<Touches>();
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(
            *app.world().resource::<Touches>(),
            settled,
            "neither side writes again once the two agree",
        );
    }

    #[test]
    fn a_typed_edit_reaches_the_value() {
        let mut app = app();
        let input = settled_input(&mut app, "");

        set_editor_text(&mut app, input, "Ada");
        app.update();

        assert_eq!(
            app.world().get::<TextValue>(input).map(|v| v.0.clone()),
            Some("Ada".to_string()),
            "what the user typed is what a save would write",
        );
        assert_still(&mut app);
    }

    #[test]
    fn a_value_set_from_code_reaches_the_input() {
        let mut app = app();
        let input = settled_input(&mut app, "");

        app.world_mut()
            .get_mut::<TextValue>(input)
            .expect("the input keeps its value")
            .0 = "Ada".to_string();
        app.update();

        assert_eq!(
            editor_text(app.world(), input),
            "Ada",
            "a value written by a binding or a load shows in the box",
        );
        assert_still(&mut app);
    }

    /// Both sides move in one frame. The edit wins and the disagreement ends
    /// rather than alternating.
    #[test]
    fn a_typed_edit_and_a_written_value_in_one_frame_settle() {
        let mut app = app();
        let input = settled_input(&mut app, "");

        set_editor_text(&mut app, input, "typed");
        app.world_mut()
            .get_mut::<TextValue>(input)
            .expect("the input keeps its value")
            .0 = "written".to_string();
        app.update();
        app.update();

        assert_eq!(
            app.world().get::<TextValue>(input).map(|v| v.0.clone()),
            Some("typed".to_string()),
        );
        assert_eq!(editor_text(app.world(), input), "typed");
        assert_still(&mut app);
    }

    /// The load shape: the document brings the value and nothing else, so the
    /// editor is the empty one `require` supplies. The value wins.
    #[test]
    fn a_value_that_arrives_with_an_empty_editor_is_not_wiped_by_it() {
        let mut app = app();
        let input = app.world_mut().spawn(TextValue("Ada".to_string())).id();
        assert!(
            app.world().get::<EditableText>(input).is_some(),
            "the value brings an editor with it: the document cannot carry one",
        );
        assert!(
            app.world().get::<TextCursorStyle>(input).is_some(),
            "and a cursor style, which the renderer queries without an Option",
        );
        app.update();

        assert_eq!(
            app.world().get::<TextValue>(input).map(|v| v.0.clone()),
            Some("Ada".to_string()),
            "a loaded value is not read back out of the empty box beside it",
        );
        assert_eq!(editor_text(app.world(), input), "Ada");
        assert_still(&mut app);
    }

    /// The `ValueChange<String>` events raised, in the order they arrived.
    #[derive(Resource, Default)]
    struct Reported(Vec<(Entity, String)>);

    fn watching_app() -> App {
        let mut app = app();
        app.init_resource::<Reported>();
        app.add_observer(
            |change: On<ValueChange<String>>, mut reported: ResMut<Reported>| {
                reported.0.push((change.source, change.value.clone()));
            },
        );
        app
    }

    #[test]
    fn a_typed_edit_is_reported_as_a_value_change() {
        let mut app = watching_app();
        let input = settled_input(&mut app, "");

        set_editor_text(&mut app, input, "Ada");
        app.update();

        assert_eq!(
            app.world().resource::<Reported>().0,
            vec![(input, "Ada".to_string())],
        );
    }

    /// A value written from outside is not an edit, and reporting it would send
    /// whoever wrote it their own value back.
    #[test]
    fn a_value_written_from_code_is_not_reported_as_an_edit() {
        let mut app = watching_app();
        let input = settled_input(&mut app, "");

        app.world_mut()
            .get_mut::<TextValue>(input)
            .expect("the input keeps its value")
            .0 = "Ada".to_string();
        app.update();
        app.update();

        assert!(app.world().resource::<Reported>().0.is_empty());
    }

    #[test]
    fn an_idle_input_is_written_by_neither_side() {
        let mut app = app();
        let _input = settled_input(&mut app, "Ada");
        assert_still(&mut app);
    }

    /// An app carrying nothing but this plugin resolves its design tokens. An
    /// empty theme answers every token with the missing-token colour.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_game_that_installs_no_theme_still_resolves_its_tokens() {
        use bevy::feathers::theme::UiTheme;
        use bevy::feathers::tokens;

        let mut app = App::new();
        app.add_plugins(AuthoredWidgetPlugin);
        app.finish();

        let theme = app.world().resource::<UiTheme>();
        assert_ne!(
            theme.color(&tokens::BUTTON_BG),
            bevy::color::palettes::basic::FUCHSIA.into(),
            "an authored button resolved its background to the missing-token colour",
        );
    }

    /// `FeathersCorePlugin` inits the resource and leaves the token map empty,
    /// so the condition checked is emptiness rather than absence.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_theme_that_is_there_but_holds_nothing_is_filled_in() {
        use bevy::feathers::theme::UiTheme;
        use bevy::feathers::tokens;

        let mut app = App::new();
        app.init_resource::<UiTheme>();
        app.add_plugins(AuthoredWidgetPlugin);
        app.finish();

        assert_ne!(
            app.world().resource::<UiTheme>().color(&tokens::BUTTON_BG),
            bevy::color::palettes::basic::FUCHSIA.into(),
        );
    }

    /// A theme installed before this plugin builds is left as it is.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_theme_the_game_installed_first_survives() {
        use bevy::feathers::theme::{ThemeProps, UiTheme};
        use bevy::feathers::tokens;

        let mut theirs = ThemeProps::default();
        theirs.color.insert(tokens::BUTTON_BG, Color::WHITE);

        let mut app = App::new();
        app.insert_resource(UiTheme(theirs));
        app.add_plugins(AuthoredWidgetPlugin);
        app.finish();

        assert_eq!(
            app.world().resource::<UiTheme>().color(&tokens::BUTTON_BG),
            Color::WHITE,
        );
    }

    /// A load builds the value through `ReflectDefault`, so the registry has to
    /// carry it.
    #[test]
    fn the_value_is_registered_with_a_default() {
        use bevy::reflect::prelude::ReflectDefault;

        let app = app();
        let registry = app.world().resource::<AppTypeRegistry>().read();
        let registration = registry
            .get(std::any::TypeId::of::<TextValue>())
            .expect("the plugin registers the text value");
        assert!(registration.data::<ReflectDefault>().is_some());
        assert!(registration.data::<ReflectComponent>().is_some());
    }

    /// Gap 12: an authored checkbox showed its resting colours whatever
    /// `Checked` said, so a correctly bound box looked identical in both
    /// states. The tokens are feathers' own, so a themed screen agrees with
    /// a feathers control beside it.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_checked_authored_checkbox_takes_the_checked_tokens() {
        use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor};
        use bevy::feathers::tokens;
        use bevy::ui::Checked;

        let mut app = app();
        let checkbox = app
            .world_mut()
            .spawn((
                Node::default(),
                Checkbox,
                AuthoredWidget,
                ThemeBackgroundColor(tokens::CHECKBOX_BG),
                ThemeBorderColor(tokens::CHECKBOX_BORDER),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<ThemeBackgroundColor>(checkbox)
                .map(|t| t.0.clone()),
            Some(tokens::CHECKBOX_BG),
            "an unchecked box rests where it was authored",
        );

        app.world_mut().entity_mut(checkbox).insert(Checked);
        app.update();
        assert_eq!(
            app.world()
                .get::<ThemeBackgroundColor>(checkbox)
                .map(|t| t.0.clone()),
            Some(tokens::CHECKBOX_BG_CHECKED),
            "checking it has to be visible",
        );
        assert_eq!(
            app.world()
                .get::<ThemeBorderColor>(checkbox)
                .map(|t| t.0.clone()),
            Some(tokens::CHECKBOX_BORDER_CHECKED),
        );

        app.world_mut().entity_mut(checkbox).remove::<Checked>();
        app.update();
        assert_eq!(
            app.world()
                .get::<ThemeBackgroundColor>(checkbox)
                .map(|t| t.0.clone()),
            Some(tokens::CHECKBOX_BG),
            "and unchecking it goes back, not to some third colour",
        );
    }

    /// A toggle switch carries the same `Checkbox` marker, so the resting
    /// token it was spawned with is what tells the two apart.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_toggle_switch_takes_the_switch_tokens_not_the_checkbox_ones() {
        use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor};
        use bevy::feathers::tokens;
        use bevy::ui::Checked;

        let mut app = app();
        let toggle = app
            .world_mut()
            .spawn((
                Node::default(),
                Checkbox,
                AuthoredWidget,
                Checked,
                ThemeBackgroundColor(tokens::SWITCH_BG),
                ThemeBorderColor(tokens::SWITCH_BORDER),
            ))
            .id();
        app.update();

        assert_eq!(
            app.world()
                .get::<ThemeBackgroundColor>(toggle)
                .map(|t| t.0.clone()),
            Some(tokens::SWITCH_BG_CHECKED),
        );
        assert_eq!(
            app.world()
                .get::<ThemeBorderColor>(toggle)
                .map(|t| t.0.clone()),
            Some(tokens::SWITCH_BORDER_CHECKED),
        );
    }

    /// A radio is one entity too, and its ring is the only part feathers
    /// themes there.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_chosen_authored_radio_takes_the_checked_ring() {
        use bevy::feathers::theme::ThemeBorderColor;
        use bevy::feathers::tokens;
        use bevy::ui::Checked;

        let mut app = app();
        let radio = app
            .world_mut()
            .spawn((
                Node::default(),
                RadioButton,
                AuthoredWidget,
                ThemeBorderColor(tokens::RADIO_BORDER),
            ))
            .id();
        app.update();

        app.world_mut().entity_mut(radio).insert(Checked);
        app.update();
        assert_eq!(
            app.world()
                .get::<ThemeBorderColor>(radio)
                .map(|t| t.0.clone()),
            Some(tokens::RADIO_BORDER_CHECKED),
        );
    }

    /// Editor chrome runs its own checkbox state machines, some of which
    /// refuse a toggle. The gate that keeps the value observers off it keeps
    /// the styling off it too.
    #[cfg(feature = "feathers")]
    #[test]
    fn an_unauthored_checkbox_is_left_alone() {
        use bevy::feathers::theme::ThemeBackgroundColor;
        use bevy::feathers::tokens;
        use bevy::ui::Checked;

        let mut app = app();
        let chrome = app
            .world_mut()
            .spawn((
                Node::default(),
                Checkbox,
                Checked,
                ThemeBackgroundColor(tokens::CHECKBOX_BG),
            ))
            .id();
        app.update();

        assert_eq!(
            app.world()
                .get::<ThemeBackgroundColor>(chrome)
                .map(|t| t.0.clone()),
            Some(tokens::CHECKBOX_BG),
            "the host application styles its own chrome",
        );
    }

    /// The swap is idempotent: a settled widget is not re-inserted every
    /// frame, which would keep change detection hot and mark the document
    /// dirty for nothing.
    #[cfg(feature = "feathers")]
    #[test]
    fn a_settled_widget_is_not_rewritten_every_frame() {
        use bevy::feathers::theme::ThemeBackgroundColor;
        use bevy::feathers::tokens;
        use bevy::ui::Checked;

        let mut app = app();
        let checkbox = app
            .world_mut()
            .spawn((
                Node::default(),
                Checkbox,
                AuthoredWidget,
                Checked,
                ThemeBackgroundColor(tokens::CHECKBOX_BG),
            ))
            .id();
        for _ in 0..4 {
            app.update();
        }
        let ticks = app
            .world()
            .entity(checkbox)
            .get_ref::<ThemeBackgroundColor>()
            .map(|token| token.last_changed());
        app.update();
        assert_eq!(
            app.world()
                .entity(checkbox)
                .get_ref::<ThemeBackgroundColor>()
                .map(|token| token.last_changed()),
            ticks,
            "an agreed token is left where it is",
        );
    }
}
