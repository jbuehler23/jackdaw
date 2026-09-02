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

/// Marks an authored toggle switch.
///
/// A switch and a checkbox are the same `Checkbox` component with
/// different theme tokens, so nothing on the entity says which of the two
/// the author asked for -- and everything that has to tell them apart, the
/// outliner's icon first among them, had to guess. This says it.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct ToggleSwitch;

/// Marks an authored separator: the hairline rule between two groups.
///
/// It is also what says which way the line runs. A separator has no axis of
/// its own; it takes the one across the flow it sits in, so the same widget
/// is a horizontal rule in a column and a vertical one in a row.
/// [`separator_follows_parent_axis`] does that, and it needs a marker to know
/// which nodes to ask about.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct Separator;

/// Marks an authored spacer: a node whose whole job is to take up the room
/// its siblings leave.
///
/// A spacer is a `Node` with `flex_grow` and nothing drawn, so its values are
/// the ones a plain container carries too, and only this says it was placed
/// as a spacer -- which is what the outliner draws its glyph from.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct Spacer;

/// How far along a [`Progress`] bar's fill sits, as a fraction.
///
/// Values outside `0.0..=1.0` are clamped when the fill is written, so a bar
/// driven from game state that overshoots stays inside its track rather than
/// spilling past it.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Progress {
    /// The fraction filled, from empty at `0.0` to full at `1.0`.
    pub value: f32,
}

/// An authored option picker: the list it offers and which of them is chosen.
///
/// The picker's chrome -- the button, the popup, one row per option -- is not
/// authored. It is rebuilt from this component whenever the options or the
/// choice change, so editing the list in the inspector redraws the widget, and
/// a document carries the list rather than the entities that draw it.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Component, Default)]
pub struct Dropdown {
    /// The options, in the order the popup lists them.
    pub options: Vec<String>,
    /// Which option is chosen, as an index into `options`. An index past the
    /// end shows no caption rather than refusing to draw.
    pub selected: usize,
}

/// An authored set of radio buttons: the choices it offers and which one is
/// taken.
///
/// The entity carrying this is the `RadioGroup`; the rows are chrome rebuilt
/// from the list, for the same reason a [`Dropdown`]'s popup is.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Component, Default)]
pub struct RadioOptions {
    /// The choices, in the order they are shown.
    pub options: Vec<String>,
    /// Which choice is taken, as an index into `options`.
    pub selected: usize,
}

/// Which choice a generated [`RadioOptions`] row stands for.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioOptionIndex(
    /// The index into the group's `options`.
    pub usize,
);

/// An authored tab strip: the tab labels and which tab is in front.
///
/// The strip of segments is chrome. The panes are not: they are the widget's
/// authored children, in order, and `active` says which of them is shown. A
/// screen therefore builds a tabbed panel by putting content under the tabs
/// and nothing else.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Component, Default)]
pub struct TabStrip {
    /// The tab labels, in the order the strip shows them.
    pub labels: Vec<String>,
    /// Which tab is in front, as an index into `labels` and into the panes.
    pub active: usize,
}

/// Which tab a generated [`TabStrip`] segment stands for.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabSegment(
    /// The index into the strip's `labels`.
    pub usize,
);

/// How wide the fixed border of a nine-patch image is.
///
/// `NodeImageMode::Sliced` holds a `TextureSlicer`, which a document can carry
/// but nothing can author usefully: the four insets are almost always the same
/// number, and that number is the only part a screen changes. This is that
/// number, written into the image mode beside it.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NineSlice {
    /// The inset, in texture pixels, of all four slicing lines.
    pub border: f32,
}

/// Marks chrome a widget's own system built, as opposed to a node a document
/// authored.
///
/// Rebuilding is despawn-and-respawn, and the only thing separating the parts
/// to throw away from a child the author put there by hand is this.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedPart;

/// Which option a generated [`Dropdown`] row stands for.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropdownOption(
    /// The index into the dropdown's `options`.
    pub usize,
);

/// The child of a [`Progress`] track that draws the filled part.
///
/// The fill is an authored child rather than a generated one: it carries its
/// own colour and corner radius, so a screen can restyle the bar without any
/// of this crate knowing. Its `width` is the one value it does not own.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct ProgressFill;

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

/// The `PostUpdate` systems that write the `Node` values an authored widget
/// derives rather than stores: a separator's thickness and a progress bar's
/// fill width.
///
/// Both run before layout so the frame that changes a value is the frame that
/// shows it. Named so a binding evaluator can be ordered ahead of them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthoredNodeSystems;

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
        app.add_systems(
            PostUpdate,
            (
                separator_follows_parent_axis,
                progress_fill_follows_value,
                nine_slice_follows_border,
            )
                .in_set(AuthoredNodeSystems)
                .before(bevy::ui::UiSystems::Layout),
        );
        #[cfg(feature = "feathers")]
        {
            app.add_systems(
                Update,
                (
                    hydrate_button_hover,
                    authored_check_styles,
                    dropdown_chrome_follows_options,
                    radio_rows_follow_options,
                    tab_chrome_follows_labels,
                ),
            );
            app.add_observer(dropdown_option_activated);
            app.add_observer(radio_option_chosen);
            app.add_observer(tab_segment_chosen);
        }
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
    app.register_type::<ToggleSwitch>();
    app.register_type::<Separator>();
    app.register_type::<Spacer>();
    app.register_type::<Progress>();
    app.register_type::<ProgressFill>();
    app.register_type::<Dropdown>();
    app.register_type::<RadioOptions>();
    app.register_type::<TabStrip>();
    app.register_type::<NineSlice>();

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

/// Builds the chrome a [`Dropdown`] is drawn from: a menu button showing the
/// chosen option and a popup listing all of them.
///
/// The parts are generated rather than authored so that the list is the one
/// thing a document carries. A change to the component throws the old parts
/// away and builds them again, which is also what puts a new choice in the
/// button's caption.
///
/// The menu root is a child rather than the widget entity itself: feathers
/// hangs the observer that opens and closes the popup off the entity its
/// `FeathersMenu` scene spawns, and an observer is not something that can be
/// added to an entity a document loaded.
#[cfg(feature = "feathers")]
fn dropdown_chrome_follows_options(
    dropdowns: Query<(Entity, &Dropdown, Option<&Children>), Changed<Dropdown>>,
    generated: Query<(), With<GeneratedPart>>,
    mut commands: Commands,
) {
    use bevy::feathers::controls::{
        FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
    };
    use bevy::feathers::theme::ThemedText;

    for (entity, dropdown, children) in &dropdowns {
        for child in children.into_iter().flatten() {
            if generated.contains(*child) {
                commands.entity(*child).despawn();
            }
        }

        let caption = dropdown
            .options
            .get(dropdown.selected)
            .cloned()
            .unwrap_or_default();
        let menu = commands
            .spawn_scene(bsn! { @FeathersMenu })
            .insert((GeneratedPart, ChildOf(entity)))
            .id();
        commands
            .spawn_scene(bsn! {
                @FeathersMenuButton {
                    @caption: bsn! { Text({caption}) ThemedText },
                }
            })
            .insert(ChildOf(menu));
        let popup = commands
            .spawn_scene(bsn! { @FeathersMenuPopup })
            .insert(ChildOf(menu))
            .id();
        for (index, option) in dropdown.options.iter().enumerate() {
            commands
                .spawn_scene(bsn! {
                    @FeathersMenuItem {
                        @caption: bsn! { Text({option.clone()}) ThemedText },
                    }
                })
                .insert((DropdownOption(index), ChildOf(popup)));
        }
    }
}

/// Writes a picked option back into the [`Dropdown`] it belongs to, and says
/// so as a [`ValueChange`].
///
/// `bevy_ui_widgets` raises `Activate` for the row and closes the popup; the
/// choice itself is state, and this is what makes it state. The change is
/// announced the way a slider or a checkbox announces one, so a binding can
/// hear it without knowing what a menu is.
#[cfg(feature = "feathers")]
fn dropdown_option_activated(
    activate: On<bevy::ui_widgets::Activate>,
    options: Query<&DropdownOption>,
    parents: Query<&ChildOf>,
    dropdowns: Query<(), With<Dropdown>>,
    mut commands: Commands,
) {
    let Ok(option) = options.get(activate.entity) else {
        return;
    };
    let index = option.0;
    let mut current = activate.entity;
    // The generated chrome is three deep; the cap stops a cycle in `ChildOf`
    // rather than bounding anything the builder above can produce.
    let mut owner = None;
    for _ in 0..8 {
        let Ok(child_of) = parents.get(current) else {
            break;
        };
        current = child_of.parent();
        if dropdowns.contains(current) {
            owner = Some(current);
            break;
        }
    }
    let Some(owner) = owner else {
        return;
    };
    commands.queue(move |world: &mut World| {
        let Ok(mut entity) = world.get_entity_mut(owner) else {
            return;
        };
        let Some(mut dropdown) = entity.get_mut::<Dropdown>() else {
            return;
        };
        if dropdown.selected == index {
            return;
        }
        dropdown.selected = index;
        world.trigger(ValueChange {
            source: owner,
            value: index,
            is_final: true,
        });
    });
}

/// Builds the rows a [`RadioOptions`] group is drawn from, one feathers radio
/// per option, and marks the taken one.
#[cfg(feature = "feathers")]
fn radio_rows_follow_options(
    groups: Query<(Entity, &RadioOptions, Option<&Children>), Changed<RadioOptions>>,
    generated: Query<(), With<GeneratedPart>>,
    mut commands: Commands,
) {
    use bevy::feathers::controls::FeathersRadio;
    use bevy::feathers::theme::ThemedText;

    for (entity, group, children) in &groups {
        for child in children.into_iter().flatten() {
            if generated.contains(*child) {
                commands.entity(*child).despawn();
            }
        }
        for (index, option) in group.options.iter().enumerate() {
            let mut row = commands.spawn_scene(bsn! {
                @FeathersRadio {
                    @caption: bsn! { Text({option.clone()}) ThemedText },
                }
            });
            row.insert((GeneratedPart, RadioOptionIndex(index), ChildOf(entity)));
            if index == group.selected {
                row.insert(Checked);
            }
        }
    }
}

/// Writes a taken choice back into the [`RadioOptions`] it belongs to.
///
/// `bevy_ui_widgets` addresses a radio change to the group and names the
/// button, so the index has to be looked up from the row that was clicked.
#[cfg(feature = "feathers")]
fn radio_option_chosen(
    change: On<ValueChange<Entity>>,
    groups: Query<(), With<RadioOptions>>,
    rows: Query<&RadioOptionIndex>,
    mut commands: Commands,
) {
    if groups.get(change.source).is_err() {
        return;
    }
    let Ok(index) = rows.get(change.value).map(|row| row.0) else {
        return;
    };
    let owner = change.source;
    commands.queue(move |world: &mut World| set_chosen_index(world, owner, index));
}

/// Puts `index` into whichever list component `owner` carries and announces
/// it, unless it is already the one taken.
///
/// A radio group and a tab strip differ only in which component holds the
/// number, and both answer a click the same way.
#[cfg(feature = "feathers")]
fn set_chosen_index(world: &mut World, owner: Entity, index: usize) {
    let Ok(mut entity) = world.get_entity_mut(owner) else {
        return;
    };
    if let Some(mut group) = entity.get_mut::<RadioOptions>() {
        if group.selected == index {
            return;
        }
        group.selected = index;
    } else if let Some(mut tabs) = entity.get_mut::<TabStrip>() {
        if tabs.active == index {
            return;
        }
        tabs.active = index;
    } else {
        return;
    }
    world.trigger(ValueChange {
        source: owner,
        value: index,
        is_final: true,
    });
}

/// Builds the strip of segments a [`TabStrip`] is drawn from, and shows the
/// pane the active tab names.
///
/// The strip is one generated child holding the segments, so the panes stay
/// the widget's own children and their order is the tab order.
#[cfg(feature = "feathers")]
fn tab_chrome_follows_labels(
    strips: Query<(Entity, &TabStrip, Option<&Children>), Changed<TabStrip>>,
    generated: Query<(), With<GeneratedPart>>,
    mut nodes: Query<&mut Node>,
    mut commands: Commands,
) {
    use bevy::feathers::controls::FeathersRadio;
    use bevy::feathers::theme::ThemedText;
    use bevy::ui_widgets::RadioGroup;

    for (entity, tabs, children) in &strips {
        let mut panes = Vec::new();
        for child in children.into_iter().flatten() {
            if generated.contains(*child) {
                commands.entity(*child).despawn();
            } else {
                panes.push(*child);
            }
        }

        for (index, pane) in panes.iter().enumerate() {
            let Ok(mut node) = nodes.get_mut(*pane) else {
                continue;
            };
            let display = if index == tabs.active {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
        }

        let strip = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    ..default()
                },
                RadioGroup,
                GeneratedPart,
                ChildOf(entity),
            ))
            .id();
        // The strip is spawned ahead of the panes it names, which is where a
        // reader expects a tab bar to be.
        commands.entity(entity).insert_children(0, &[strip]);
        for (index, label) in tabs.labels.iter().enumerate() {
            let mut segment = commands.spawn_scene(bsn! {
                @FeathersRadio {
                    @caption: bsn! { Text({label.clone()}) ThemedText },
                }
            });
            segment.insert((TabSegment(index), ChildOf(strip)));
            if index == tabs.active {
                segment.insert(Checked);
            }
        }
    }
}

/// Writes a clicked tab back into the [`TabStrip`] it belongs to.
///
/// The `RadioGroup` is the generated strip rather than the widget, so the
/// change arrives addressed to a child and is carried up one level.
#[cfg(feature = "feathers")]
fn tab_segment_chosen(
    change: On<ValueChange<Entity>>,
    strips: Query<&ChildOf, With<GeneratedPart>>,
    owners: Query<(), With<TabStrip>>,
    segments: Query<&TabSegment>,
    mut commands: Commands,
) {
    let Ok(child_of) = strips.get(change.source) else {
        return;
    };
    let owner = child_of.parent();
    if owners.get(owner).is_err() {
        return;
    }
    let Ok(index) = segments.get(change.value).map(|segment| segment.0) else {
        return;
    };
    commands.queue(move |world: &mut World| set_chosen_index(world, owner, index));
}

/// Puts a [`NineSlice`]'s border into the image mode beside it, so the corners
/// of a panel skin keep their size while the middle stretches.
///
/// A border of zero leaves the image whole: nothing is sliced off, and
/// `NodeImageMode::Auto` is what says so.
fn nine_slice_follows_border(
    mut images: Query<(&NineSlice, &mut ImageNode), Or<(Changed<NineSlice>, Changed<ImageNode>)>>,
) {
    use bevy::sprite::TextureSlicer;
    use bevy::ui::widget::NodeImageMode;

    for (slice, mut image) in &mut images {
        let wanted = if slice.border > 0.0 {
            NodeImageMode::Sliced(TextureSlicer {
                border: BorderRect::all(slice.border),
                ..default()
            })
        } else {
            NodeImageMode::Auto
        };
        if image.image_mode != wanted {
            image.image_mode = wanted;
        }
    }
}

/// Lays a [`Separator`] across the flow it sits in: a hairline the full width
/// of a column, or the full height of a row.
///
/// The thickness is whichever of `width` and `height` is already a pixel
/// value, so a screen can author a 2px rule and keep it. A separator with no
/// parent, or one under something that is not a `Node`, is left as authored.
fn separator_follows_parent_axis(
    parents: Query<&Node, Without<Separator>>,
    mut separators: Query<(&ChildOf, &mut Node), With<Separator>>,
) {
    for (child_of, mut node) in &mut separators {
        let Ok(parent) = parents.get(child_of.parent()) else {
            continue;
        };
        let horizontal = matches!(
            parent.flex_direction,
            FlexDirection::Column | FlexDirection::ColumnReverse
        );
        let (across, along) = if horizontal {
            (node.height, node.width)
        } else {
            (node.width, node.height)
        };
        let thickness = match across {
            Val::Px(px) => Val::Px(px),
            _ => match along {
                Val::Px(px) => Val::Px(px),
                _ => Val::Px(1.0),
            },
        };
        let (width, height) = if horizontal {
            (Val::Percent(100.0), thickness)
        } else {
            (thickness, Val::Percent(100.0))
        };
        if node.width != width {
            node.width = width;
        }
        if node.height != height {
            node.height = height;
        }
        // A hairline in a flex line is squeezed to nothing without this: the
        // default `flex_shrink` lets a 1px child give up its only pixel.
        if node.flex_shrink != 0.0 {
            node.flex_shrink = 0.0;
        }
    }
}

/// Sizes a [`ProgressFill`] to the [`Progress`] on its parent track.
///
/// The fill is a percentage of the track rather than a computed pixel width,
/// so a bar keeps its proportion through a resize without this running again.
fn progress_fill_follows_value(
    tracks: Query<(&Progress, &Children)>,
    mut fills: Query<&mut Node, With<ProgressFill>>,
) {
    for (progress, children) in &tracks {
        let width = Val::Percent(progress.value.clamp(0.0, 1.0) * 100.0);
        for child in children.iter() {
            let Ok(mut node) = fills.get_mut(child) else {
                continue;
            };
            if node.width != width {
                node.width = width;
            }
        }
    }
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
