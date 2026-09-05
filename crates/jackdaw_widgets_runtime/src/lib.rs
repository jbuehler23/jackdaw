//! Value behaviour and load-time defaults for authored UI widgets.
//!
//! `bevy_ui_widgets` is external-state: a widget emits [`ValueChange`] and
//! changes nothing about itself. This crate holds the markers that survive a
//! load and the observers that turn those events into state, so the editor and
//! `jackdaw_runtime` cannot drift apart.
//!
//! Observers are global rather than per-entity `observe()` calls: an observer
//! entity is not a component, so nothing would write it to a document or
//! recreate it on load. [`AuthoredWidget`] gates them, so panels running their
//! own checkbox state machines are left alone.

#![deny(missing_docs)]

use bevy::{
    prelude::*,
    text::{EditableText, EditableTextSystems, TextCursorStyle, TextEdit},
    ui::Checked,
    ui_widgets::{Button, Checkbox, RadioButton, RadioGroup, Slider, SliderValue, ValueChange},
};

/// The text an authored input holds, in a form a scene document can carry.
///
/// `bevy_text::EditableText` is not `Reflect`, so this reflectable half is what
/// a document and a binding see; [`AuthoredWidgetPlugin`] keeps the two in
/// step. [`TextCursorStyle`] is required because `bevy_ui_render`'s
/// editable-text extract queries it without an `Option`.
#[derive(Component, Reflect, Default, Debug, Clone, PartialEq, Eq)]
#[reflect(Component, Default)]
#[require(EditableText, TextCursorStyle)]
pub struct TextValue(
    /// The text the input holds.
    pub String,
);

/// Marks an authored toggle switch.
///
/// A switch and a checkbox are the same `Checkbox` component with different
/// theme tokens, so only this tells them apart.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct ToggleSwitch;

/// Marks an authored separator: the hairline rule between two groups.
///
/// A separator has no axis of its own; it takes the one across the flow it
/// sits in, which `separator_follows_parent_axis` applies.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct Separator;

/// Marks an authored spacer: a node whose whole job is to take up the room its
/// siblings leave.
///
/// Its `Node` values are the ones a plain container carries too, so only this
/// says it was placed as a spacer.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct Spacer;

/// How far along a [`Progress`] bar's fill sits, as a fraction.
///
/// Values outside `0.0..=1.0` are clamped when the fill is written.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Progress {
    /// The fraction filled, from empty at `0.0` to full at `1.0`.
    pub value: f32,
}

/// An authored option picker: the list it offers and which of them is chosen.
///
/// The picker's chrome is not authored; it is rebuilt from this component
/// whenever the options change.
///
/// A component equal to its `Default` emits as a bare type path, so changing
/// this `Default` silently reinterprets every scene already saved that way.
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
/// from the list. Its `Default` is a persisted contract, as [`Dropdown`]'s is.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Component, Default)]
pub struct RadioOptions {
    /// The choices, in the order they are shown.
    pub options: Vec<String>,
    /// Which choice is taken, as an index into `options`. An index past the
    /// end is clamped to the last choice when the rows are drawn.
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
/// The strip of segments is chrome. The panes are the widget's authored
/// children, in order, and `active` says which of them is shown. Its `Default`
/// is a persisted contract, as [`Dropdown`]'s is.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Component, Default)]
pub struct TabStrip {
    /// The tab labels, in the order the strip shows them.
    pub labels: Vec<String>,
    /// Which tab is in front, as an index into `labels` and into the panes.
    /// An index past the end is clamped to the last tab when the strip is
    /// drawn.
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
/// A `TextureSlicer`'s four insets are almost always the same number, and that
/// number is the only part a screen changes; this is it, written into the image
/// mode beside it. It requires [`ImageNode`] because a document carrying the
/// border alone has nothing to write it to.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Default)]
#[reflect(Component, Default)]
#[require(ImageNode)]
pub struct NineSlice {
    /// The inset, in texture pixels, of all four slicing lines.
    pub border: f32,
}

/// Marks chrome a widget's own system built, as opposed to a node a document
/// authored.
///
/// A rebuild despawns these and leaves authored children alone.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedPart;

/// The list a widget's chrome was last built from, so a changed choice moves a
/// marker between existing parts rather than despawning the row under the
/// pointer.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct ChromeBuiltFrom(Vec<String>);

/// Marks the generated menu button whose caption shows a [`Dropdown`]'s
/// choice, so the caption can be rewritten without rebuilding the menu.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
struct DropdownCaption;

/// Which option a generated [`Dropdown`] row stands for.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropdownOption(
    /// The index into the dropdown's `options`.
    pub usize,
);

/// The child of a [`Progress`] track that draws the filled part.
///
/// It is an authored child carrying its own styling; only its `width` is
/// written for it.
#[derive(Component, Reflect, Default, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct ProgressFill;

/// Where a binding writes a widget's text: [`TextValue`]'s only field, spelled
/// the way a bind path spells one.
pub fn text_value_write_path() -> String {
    format!("{}.0", <TextValue as bevy::reflect::TypePath>::type_path())
}

/// Marks an entity as authored content, as opposed to chrome the host
/// application built for itself.
///
/// [`AuthoredWidgetPlugin`]'s self-update observers answer only for entities
/// carrying this. It is not `Reflect`; each side re-derives it on spawn.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthoredWidget;

/// The `PostUpdate` systems that reconcile [`TextValue`] with the editor beside
/// it.
///
/// Anything that writes `TextValue` belongs ahead of this set, or a frame can
/// reconcile a typed edit and then overwrite it with a stale value.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthoredTextSystems;

/// The `PostUpdate` systems that write the `Node` values an authored widget
/// derives rather than stores, such as a progress bar's fill width.
///
/// They run before layout, and a binding evaluator belongs ahead of them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthoredNodeSystems;

/// The `PostUpdate` systems that rebuild the chrome a list-shaped widget is
/// drawn from: a dropdown's menu, a radio group's rows, a tab strip's segments.
///
/// A binding that writes the list belongs ahead of them, as with
/// [`AuthoredNodeSystems`].
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthoredChromeSystems;

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
            app.add_systems(Update, (hydrate_button_hover, authored_check_styles));
            app.add_systems(
                PostUpdate,
                (
                    dropdown_chrome_follows_options,
                    radio_rows_follow_options,
                    tab_chrome_follows_labels,
                )
                    .in_set(AuthoredChromeSystems)
                    .before(bevy::ui::UiSystems::Layout),
            );
            app.add_observer(dropdown_option_activated);
            app.add_observer(radio_option_chosen);
            app.add_observer(tab_segment_chosen);
        }
    }

    /// Installs the dark theme if nothing else did, since an empty `UiTheme`
    /// resolves every design token to the missing-token colour.
    ///
    /// In `finish` rather than `build` because `FeathersPlugins` may be added
    /// after this plugin.
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

/// Registers `ReflectDefault` for each widget marker, which neither
/// `bevy_ui_widgets` nor `bevy_feathers` does, so a loaded document can rebuild
/// them.
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

        app.register_type::<EntityCursor>();
    }
}

/// Gives a loaded button the `Hovered` picking state a document does not
/// record, without which `update_button_styles` never restyles it.
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
/// Pairs rather than a per-widget lookup because a checkbox and a toggle switch
/// share the same marker; the token an entity carries is what identifies it.
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

/// Border pairs, which for a radio are its whole checked treatment.
#[cfg(feature = "feathers")]
fn border_token_pairs() -> [TokenPair; 3] {
    use bevy::feathers::tokens;
    [
        (tokens::CHECKBOX_BORDER, tokens::CHECKBOX_BORDER_CHECKED),
        (tokens::SWITCH_BORDER, tokens::SWITCH_BORDER_CHECKED),
        (tokens::RADIO_BORDER, tokens::RADIO_BORDER_CHECKED),
    ]
}

/// The other half of `current`'s pair, or `None` when it is already correct or
/// belongs to no pair here.
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

/// Shows an authored checkbox, radio, or toggle switch in its checked colours.
///
/// `bevy_feathers` does this from systems that walk a multi-entity widget
/// through private markers, none of which reach a one-entity authored widget.
/// Hover and press treatments are not mirrored: they read picking state a
/// document does not carry.
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

/// A widget can be despawned between the event and the command answering it, so
/// the lookup is fallible where `EntityCommands` would panic.
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

/// `SliderValue` is immutable, so a sync is a re-insert; the equality guard
/// keeps a two-way-bound slider from re-inserting every frame.
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
/// The menu root is a child rather than the widget entity because feathers
/// hangs the open/close observer off the entity its `FeathersMenu` scene
/// spawns.
#[cfg(feature = "feathers")]
fn dropdown_chrome_follows_options(
    dropdowns: Query<
        (
            Entity,
            &Dropdown,
            Option<&Children>,
            Option<&ChromeBuiltFrom>,
        ),
        Changed<Dropdown>,
    >,
    generated: Query<(), With<GeneratedPart>>,
    descendants: Query<&Children>,
    captions: Query<(), With<DropdownCaption>>,
    texts: Query<(), With<Text>>,
    mut commands: Commands,
) {
    use bevy::feathers::controls::{
        FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
    };
    use bevy::feathers::theme::ThemedText;

    for (entity, dropdown, children, built) in &dropdowns {
        let caption = dropdown
            .options
            .get(dropdown.selected)
            .cloned()
            .unwrap_or_default();

        // A new choice out of the same list is a caption, not a new menu.
        if built.is_some_and(|built| built.0 == dropdown.options) {
            if let Some(button) =
                find_descendant(&descendants, entity, &|child| captions.contains(child))
                && let Some(text) =
                    find_descendant(&descendants, button, &|child| texts.contains(child))
            {
                commands.entity(text).insert(Text::new(caption));
            }
            continue;
        }

        for child in children.into_iter().flatten() {
            if generated.contains(*child) {
                commands.entity(*child).despawn();
            }
        }

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
            .insert((DropdownCaption, ChildOf(menu)));
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
        commands
            .entity(entity)
            .insert(ChromeBuiltFrom(dropdown.options.clone()));
    }
}

/// The nearest descendant of `root` that `wanted` accepts, breadth first.
#[cfg(feature = "feathers")]
fn find_descendant(
    children: &Query<&Children>,
    root: Entity,
    wanted: &dyn Fn(Entity) -> bool,
) -> Option<Entity> {
    let mut frontier = vec![root];
    // The bound stops a cycle in the hierarchy; generated scenes are shallower.
    for _ in 0..8 {
        let mut next = Vec::new();
        for entity in frontier {
            for child in children.get(entity).into_iter().flatten() {
                if wanted(*child) {
                    return Some(*child);
                }
                next.push(*child);
            }
        }
        if next.is_empty() {
            return None;
        }
        frontier = next;
    }
    None
}

/// An index into a list of `len` entries, clamped to the last one and warned
/// about once.
#[cfg(feature = "feathers")]
fn clamped_index(index: usize, len: usize, what: &str) -> usize {
    if len == 0 || index < len {
        return index;
    }
    bevy::log::warn_once!("{what} is set to {index} with only {len} to choose from");
    len - 1
}

/// Writes a picked option back into the [`Dropdown`] it belongs to and
/// announces it as a [`ValueChange`], so a binding can hear it without knowing
/// what a menu is.
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
    // The cap stops a cycle in `ChildOf`; the generated chrome is three deep.
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
    groups: Query<
        (
            Entity,
            &RadioOptions,
            Option<&Children>,
            Option<&ChromeBuiltFrom>,
        ),
        Changed<RadioOptions>,
    >,
    generated: Query<(), With<GeneratedPart>>,
    rows: Query<&RadioOptionIndex>,
    mut commands: Commands,
) {
    use bevy::feathers::controls::FeathersRadio;
    use bevy::feathers::theme::ThemedText;

    for (entity, group, children, built) in &groups {
        let selected = clamped_index(group.selected, group.options.len(), "a radio group");

        // Rebuilding the rows would throw away the one just clicked.
        if built.is_some_and(|built| built.0 == group.options) {
            for child in children.into_iter().flatten() {
                let Ok(row) = rows.get(*child) else {
                    continue;
                };
                let (row_entity, checked) = (*child, row.0 == selected);
                commands.queue(move |world: &mut World| set_checked(world, row_entity, checked));
            }
            continue;
        }

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
            if index == selected {
                row.insert(Checked);
            }
        }
        commands
            .entity(entity)
            .insert(ChromeBuiltFrom(group.options.clone()));
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

/// Puts `index` into whichever list component `owner` carries and announces it,
/// unless it is already the one taken.
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
    strips: Query<
        (
            Entity,
            &TabStrip,
            Option<&Children>,
            Option<&ChromeBuiltFrom>,
        ),
        Changed<TabStrip>,
    >,
    generated: Query<(), With<GeneratedPart>>,
    descendants: Query<&Children>,
    segments: Query<&TabSegment>,
    mut nodes: Query<&mut Node>,
    mut commands: Commands,
) {
    use bevy::feathers::controls::FeathersRadio;
    use bevy::feathers::theme::ThemedText;
    use bevy::ui_widgets::RadioGroup;

    for (entity, tabs, children, built) in &strips {
        let active = clamped_index(tabs.active, tabs.labels.len(), "a tab strip");
        let in_place = built.is_some_and(|built| built.0 == tabs.labels);

        let mut panes = Vec::new();
        for child in children.into_iter().flatten() {
            if generated.contains(*child) {
                if in_place {
                    // The strip stays: only the segment in front changes.
                    for segment_entity in descendants.get(*child).into_iter().flatten() {
                        let Ok(segment) = segments.get(*segment_entity) else {
                            continue;
                        };
                        let (segment_entity, checked) = (*segment_entity, segment.0 == active);
                        commands.queue(move |world: &mut World| {
                            set_checked(world, segment_entity, checked);
                        });
                    }
                } else {
                    commands.entity(*child).despawn();
                }
            } else {
                panes.push(*child);
            }
        }

        for (index, pane) in panes.iter().enumerate() {
            let Ok(mut node) = nodes.get_mut(*pane) else {
                continue;
            };
            let display = if index == active {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
        }

        if in_place {
            continue;
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
        commands.entity(entity).insert_children(0, &[strip]);
        for (index, label) in tabs.labels.iter().enumerate() {
            let mut segment = commands.spawn_scene(bsn! {
                @FeathersRadio {
                    @caption: bsn! { Text({label.clone()}) ThemedText },
                }
            });
            segment.insert((TabSegment(index), ChildOf(strip)));
            if index == active {
                segment.insert(Checked);
            }
        }
        commands
            .entity(entity)
            .insert(ChromeBuiltFrom(tabs.labels.clone()));
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
/// of a panel skin keep their size while the middle stretches. A border of zero
/// leaves the image whole.
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

/// Lays a [`Separator`] across the flow it sits in: a hairline the full width of
/// a column, or the full height of a row.
///
/// The thickness is whichever of `width` and `height` is already a pixel value,
/// so an authored 2px rule keeps its weight.
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
        // The default `flex_shrink` lets a 1px child give up its only pixel.
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

/// Carries a typed edit into [`TextValue`] and announces it as a
/// [`ValueChange`].
///
/// Runs ahead of `authored_text_follows_value`, so when both halves move at
/// once the edit wins rather than snapping the box back mid-word. A newly
/// inserted editor is exempt: a load inserts both halves at once and the editor
/// arrives empty, which would otherwise read as an edit that wipes the value.
/// `is_final` is false because a keystroke is the middle of typing.
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

/// Puts a [`TextValue`] written from outside into the box the user reads.
///
/// The equality guard stops this and `authored_text_self_update` trading writes.
/// `set_text` drops whatever the IME was composing, which cannot be merged.
fn authored_text_follows_value(
    mut inputs: Query<(&mut EditableText, &TextValue), Changed<TextValue>>,
) {
    for (mut editable, value) in &mut inputs {
        let shown: String = editable.value().into_iter().collect();
        if shown == value.0 {
            continue;
        }
        editable.editor_mut().set_text(&value.0);
        // A shorter string leaves the caret outside the text entirely.
        editable.queue_edit(TextEdit::TextEnd(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How often each side was written, counted over whole frames.
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

    /// The pair with bevy's text pipeline under it, so a queued `TextEdit` is
    /// actually applied; every other test here runs the two systems alone.
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

    /// A keystroke as bevy delivers one: queued, then applied by the pipeline.
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

    /// The queued caret fix-up flags `EditableText` again, which is where a pair
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

    /// The load shape: the document brings the value and `require` supplies an
    /// empty editor.
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

    /// Reporting a write from outside would send whoever wrote it their own
    /// value back.
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
    /// so emptiness rather than absence is what gets filled in.
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

    /// A document can carry the border alone, so the image mode has to come
    /// with it.
    #[test]
    fn a_nine_slice_brings_an_image_node_with_it() {
        let mut app = app();
        let panel = app.world_mut().spawn(NineSlice { border: 8.0 }).id();
        assert!(app.world().get::<ImageNode>(panel).is_some());
    }

    /// A load builds the value through `ReflectDefault`.
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

    /// A toggle switch carries the same `Checkbox` marker, so its resting token
    /// is what tells the two apart.
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

    /// Editor chrome runs its own checkbox state machines, so the observer gate
    /// keeps the styling off it too.
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

    /// Re-inserting an agreed token every frame would keep change detection hot
    /// and mark the document dirty for nothing.
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
