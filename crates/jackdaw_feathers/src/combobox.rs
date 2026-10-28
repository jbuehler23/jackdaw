//! Dropdowns.
//!
//! A combobox is a [`FeathersSelect`]: the trigger is its menu button, the list
//! is its popup of [`FeathersListRow`]s, and the picked row is the list's
//! [`Selected`] row. What the editor adds on top is the option data (values and
//! icons), a caption that can stay fixed for a combobox used as a menu, and the
//! [`ComboBoxChangeEvent`] callers listen for.

use bevy::app::PropagateOver;
use bevy::feathers::controls::{
    FeathersListRow, FeathersMenuButton, FeathersMenuPopup, FeathersSelect, OptionIndex,
};
use bevy::feathers::display::caption;
use bevy::feathers::font_styles::InheritableFont;
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::scene::SceneList;
use bevy::text::{FontSize, FontSourceTemplate, LineBreak};
use bevy::ui::Selected;
use bevy::ui_widgets::ValueChange;
use lucide_icons::Icon;

use crate::icons::font_paths;
use crate::tokens;

pub fn plugin(app: &mut App) {
    app.add_observer(setup_combobox)
        .add_observer(emit_change_on_pick)
        .add_systems(
            PostUpdate,
            (apply_selected_index, release_menu_picks, paint_triggers)
                .chain()
                .in_set(crate::WidgetSetupSystems),
        );
}

#[derive(Component)]
pub struct EditorComboBox;

/// The menu button that opens the list, naming the combobox it belongs to.
#[derive(Component)]
pub struct ComboBoxTrigger(pub Entity);

/// The popup holding the list, naming the combobox it belongs to.
#[derive(Component)]
pub struct ComboBoxPopover(pub Entity);

/// The text and icon the trigger draws, found once when the combobox is built.
#[derive(Component)]
struct ComboBoxParts {
    caption: Entity,
    glyph: Option<Entity>,
}

/// Marks the trigger's caption text with the combobox it belongs to.
#[derive(Component)]
struct ComboBoxCaption(Entity);

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

#[derive(Component)]
pub(crate) struct ComboBoxConfig {
    options: Vec<ComboBoxOptionData>,
    pub(crate) selected: usize,
    /// A caption the trigger keeps whatever is picked, for a combobox used as
    /// a menu. Such a combobox never holds a selected row.
    label_override: Option<String>,
}

impl ComboBoxConfig {
    fn caption(&self) -> String {
        self.label_override
            .clone()
            .or_else(|| self.options.get(self.selected).map(|o| o.label.clone()))
            .unwrap_or_default()
    }

    fn glyph(&self) -> Option<Icon> {
        if self.label_override.is_some() {
            return None;
        }
        self.options.get(self.selected).and_then(|o| o.icon)
    }
}

#[derive(EntityEvent)]
pub struct ComboBoxChangeEvent {
    pub entity: Entity,
    pub selected: usize,
    pub label: String,
    pub value: Option<String>,
}

/// Which option a combobox is showing, outside the widget. The config the widget
/// reads is crate-private, so this is the only handle a caller has on the
/// selection.
///
/// Out-of-range values are ignored rather than clamped.
#[derive(Component)]
pub struct ComboBoxSelectedIndex(pub usize);

pub fn combobox(options: Vec<impl Into<ComboBoxOptionData>>) -> impl Bundle {
    combobox_with_selected(options, 0)
}

pub fn combobox_with_selected(
    options: Vec<impl Into<ComboBoxOptionData>>,
    selected: usize,
) -> impl Bundle {
    combobox_bundle(options, selected, None, percent(100))
}

/// A combobox whose trigger always reads `label`, for a list of actions rather
/// than a choice of value.
pub fn combobox_with_label(
    options: Vec<impl Into<ComboBoxOptionData>>,
    label: impl Into<String>,
) -> impl Bundle {
    combobox_bundle(options, 0, Some(label.into()), percent(100))
}

/// A combobox as wide as the option it shows, for a row with no width to give.
pub fn combobox_compact_with_selected(
    options: Vec<impl Into<ComboBoxOptionData>>,
    selected: usize,
) -> impl Bundle {
    combobox_bundle(options, selected, None, Val::Auto)
}

fn combobox_bundle(
    options: Vec<impl Into<ComboBoxOptionData>>,
    selected: usize,
    label_override: Option<String>,
    width: Val,
) -> impl Bundle {
    (
        EditorComboBox,
        ComboBoxConfig {
            options: options.into_iter().map(Into::into).collect(),
            selected,
            label_override,
        },
        Node { width, ..default() },
    )
}

/// Build the select the moment the combobox is spawned, so its first frame
/// draws the trigger with its caption rather than an empty node.
fn setup_combobox(add: On<Add<ComboBoxConfig>>, mut commands: Commands) {
    let entity = add.entity;
    commands.queue(move |world: &mut World| build_combobox(world, entity));
}

fn build_combobox(world: &mut World, entity: Entity) {
    let Ok(combobox) = world.get_entity(entity) else {
        return;
    };
    let Some(config) = combobox.get::<ComboBoxConfig>() else {
        return;
    };
    let width = combobox.get::<Node>().map(|node| node.width);
    let selected = combobox
        .get::<ComboBoxSelectedIndex>()
        .map(|index| index.0)
        .filter(|index| *index < config.options.len())
        .unwrap_or(config.selected);
    let rows: Vec<Box<dyn SceneList>> = config
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| option_row(index, option))
        .collect();
    let rows: Box<dyn SceneList> = Box::new(rows);
    let with_glyph =
        config.label_override.is_none() && config.options.iter().any(|o| o.icon.is_some());

    let applied = world
        .entity_mut(entity)
        .apply_scene(bsn! { @FeathersSelect { @options: {rows} } });
    if let Err(error) = applied {
        error!("a combobox did not spawn: {error}");
        return;
    }

    let parts = descendants_of(world, entity);
    let Some(trigger) = parts
        .iter()
        .copied()
        .find(|part| world.get::<FeathersMenuButton>(*part).is_some())
    else {
        return;
    };
    let Some(popup) = parts
        .iter()
        .copied()
        .find(|part| world.get::<FeathersMenuPopup>(*part).is_some())
    else {
        return;
    };
    let Some(caption) = descendants_of(world, trigger)
        .into_iter()
        .find(|part| world.get::<Text>(*part).is_some())
    else {
        return;
    };

    // The caption sits in a slot of its own that cuts it at its edge, so a
    // narrow row shrinks the trigger instead of the trigger overhanging the
    // row. A node cannot cut its own text.
    let slot = world
        .spawn((
            Node {
                flex_shrink: 1.0,
                min_width: px(0),
                overflow: Overflow::clip_x(),
                ..default()
            },
            ThemedText,
        ))
        .id();
    let mut trigger_entity = world.entity_mut(trigger);
    trigger_entity
        .insert(ComboBoxTrigger(entity))
        .insert_children(0, &[slot]);
    if let Some(mut node) = trigger_entity.get_mut::<Node>() {
        node.min_width = px(0);
    }
    // The menu button hands its caption feathers' own size; the editor's text
    // is smaller. Inserted again rather than edited, since the font reaches
    // the caption on insert.
    if let Some(font) = trigger_entity.get::<InheritableFont>().cloned() {
        trigger_entity.insert(InheritableFont {
            font_size: tokens::TEXT_SIZE,
            ..font
        });
    }
    world.entity_mut(slot).add_child(caption);
    world.entity_mut(caption).insert((
        ComboBoxCaption(entity),
        TextLayout {
            linebreak: LineBreak::NoWrap,
            ..default()
        },
    ));
    world.entity_mut(popup).insert(ComboBoxPopover(entity));
    let glyph = if with_glyph {
        let spawned = world.spawn_scene(bsn! {
            @glyph(Icon::Circle)
            Node { margin: UiRect::right(px(6)) }
        });
        match spawned {
            Ok(spawned) => Some(spawned.id()),
            Err(error) => {
                error!("a combobox icon did not spawn: {error}");
                None
            }
        }
    } else {
        None
    };
    if let Some(glyph) = glyph {
        world.entity_mut(trigger).insert_children(0, &[glyph]);
    }

    let mut combobox = world.entity_mut(entity);
    combobox.insert(ComboBoxParts { caption, glyph });
    if let Some(width) = width
        && let Some(mut node) = combobox.get_mut::<Node>()
    {
        node.width = width;
    }
    if let Some(mut config) = combobox.get_mut::<ComboBoxConfig>() {
        config.selected = selected;
    }

    select_row(world, entity);
    paint_trigger(world, entity);
}

/// One row of the list: the option's icon, if it has one, then its label.
/// The icon sits a level below the label, so the select's own caption sync,
/// which takes a row's first text, reads the label.
fn option_row(index: usize, option: &ComboBoxOptionData) -> Box<dyn SceneList> {
    let label = option.label.clone();
    match option.icon {
        Some(icon) => Box::new(bsn_list! {
            @FeathersListRow
            InheritableFont { font_size: {tokens::TEXT_SIZE} }
            OptionIndex(index)
            Children [
                Node { margin: UiRect::right(px(6)) }
                ThemedText
                Children [ @glyph(icon) ]
                --
                @caption(label)
            ]
        }),
        None => Box::new(bsn_list! {
            @FeathersListRow
            InheritableFont { font_size: {tokens::TEXT_SIZE} }
            OptionIndex(index)
            Children [ @caption(label) ]
        }),
    }
}

/// A Lucide glyph coloured like the text beside it, in its own font.
fn glyph(icon: Icon) -> impl Scene {
    let glyph = String::from(icon.unicode());
    bsn! {
        Text(glyph)
        TextFont {
            font: FontSourceTemplate::Handle(font_paths::LUCIDE),
            font_size: FontSize::Px(tokens::ICON_SM_PX),
        }
        ~{PropagateOver::<TextFont>::default()}
        ThemedText
    }
}

fn descendants_of(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Some(children) = world.get::<Children>(entity) {
            out.extend(children.iter());
            stack.extend(children.iter().rev());
        }
    }
    out
}

/// Mark the configured option's row as the list's selected row, clearing the
/// rest. A combobox used as a menu keeps no row selected.
fn select_row(world: &mut World, combobox: Entity) {
    let Some(config) = world.get::<ComboBoxConfig>(combobox) else {
        return;
    };
    let selected = config.label_override.is_none().then_some(config.selected);
    for row in descendants_of(world, combobox) {
        let Some(index) = world.get::<OptionIndex>(row).map(|index| index.0) else {
            continue;
        };
        let is_selected = world.get::<Selected>(row).is_some();
        if Some(index) == selected && !is_selected {
            world.entity_mut(row).insert(Selected);
        } else if Some(index) != selected && is_selected {
            world.entity_mut(row).remove::<Selected>();
        }
    }
}

fn paint_trigger(world: &mut World, combobox: Entity) {
    let Some(config) = world.get::<ComboBoxConfig>(combobox) else {
        return;
    };
    let Some(parts) = world.get::<ComboBoxParts>(combobox) else {
        return;
    };
    let caption = config.caption();
    let glyph = config.glyph();
    let (caption_entity, glyph_entity) = (parts.caption, parts.glyph);
    if let Some(mut text) = world.get_mut::<Text>(caption_entity)
        && text.0 != caption
    {
        text.0 = caption;
    }
    if let Some(glyph_entity) = glyph_entity {
        paint_glyph(world, glyph_entity, glyph);
    }
}

fn paint_glyph(world: &mut World, entity: Entity, icon: Option<Icon>) {
    let display = if icon.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    if let Some(mut node) = world.get_mut::<Node>(entity)
        && node.display != display
    {
        node.display = display;
    }
    if let Some(icon) = icon
        && let Some(mut text) = world.get_mut::<Text>(entity)
    {
        let glyph = String::from(icon.unicode());
        if text.0 != glyph {
            text.0 = glyph;
        }
    }
}

/// The select reports a pick as the row it chose; callers hear it as the
/// option's index, label and value.
fn emit_change_on_pick(
    pick: On<ValueChange<Entity>>,
    mut configs: Query<&mut ComboBoxConfig>,
    indices: Query<&OptionIndex>,
    mut commands: Commands,
) {
    let combobox = pick.source;
    let Ok(mut config) = configs.get_mut(combobox) else {
        return;
    };
    let Ok(&OptionIndex(index)) = indices.get(pick.value) else {
        return;
    };
    let Some(option) = config.options.get(index).cloned() else {
        return;
    };
    config.selected = index;
    // Keep the outside-visible index on the user's own pick too, so a reader
    // never sees a stale selection and a later write of the same index is not
    // mistaken for a no-op.
    commands
        .entity(combobox)
        .insert(ComboBoxSelectedIndex(index));
    commands.trigger(ComboBoxChangeEvent {
        entity: combobox,
        selected: index,
        label: option.label,
        value: option.value,
    });
}

/// Push an externally-set index into the widget, selecting its row.
fn apply_selected_index(
    mut commands: Commands,
    mut combos: Query<
        (Entity, &ComboBoxSelectedIndex, &mut ComboBoxConfig),
        Changed<ComboBoxSelectedIndex>,
    >,
) {
    for (entity, index, mut config) in &mut combos {
        if index.0 < config.options.len() && config.selected != index.0 {
            config.selected = index.0;
            commands.queue(move |world: &mut World| select_row(world, entity));
        }
    }
}

/// A combobox used as a menu offers the same entry again next time, so the row
/// the list selected on a pick is let go.
fn release_menu_picks(
    mut commands: Commands,
    picked: Query<Entity, (With<OptionIndex>, Added<Selected>)>,
    parents: Query<&ChildOf>,
    configs: Query<&ComboBoxConfig>,
) {
    for row in &picked {
        let menu = parents
            .iter_ancestors(row)
            .find_map(|ancestor| configs.get(ancestor).ok())
            .is_some_and(|config| config.label_override.is_some());
        if menu {
            commands.entity(row).remove::<Selected>();
        }
    }
}

/// Keep the trigger reading the combobox's own caption: after a pick or an
/// outside index change, and over the select's sync, which copies the picked
/// row's text even for a combobox used as a menu.
fn paint_triggers(
    mut commands: Commands,
    changed: Query<Entity, (With<ComboBoxParts>, Changed<ComboBoxConfig>)>,
    captions: Query<&ComboBoxCaption, Changed<Text>>,
) {
    let combos = changed
        .iter()
        .chain(captions.iter().map(|caption| caption.0));
    for combobox in combos {
        commands.queue(move |world: &mut World| paint_trigger(world, combobox));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ui_widgets::ListBox;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            plugin,
        ));
        app.init_asset::<bevy::text::Font>();
        app.init_asset::<Image>();
        app.configure_sets(PostUpdate, crate::WidgetSetupSystems);
        app
    }

    fn app_with_combobox() -> (App, Entity) {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn(combobox_with_selected(vec!["px", "%", "auto"], 0))
            .id();
        app.update();
        (app, entity)
    }

    /// The label the trigger is currently drawing.
    fn label(app: &App, combobox: Entity) -> String {
        let caption = app
            .world()
            .get::<ComboBoxParts>(combobox)
            .expect("the combobox is built")
            .caption;
        app.world()
            .get::<Text>(caption)
            .expect("the caption is text")
            .0
            .clone()
    }

    fn rows(app: &App, combobox: Entity) -> Vec<Entity> {
        descendants_of(app.world(), combobox)
            .into_iter()
            .filter(|entity| app.world().get::<OptionIndex>(*entity).is_some())
            .collect()
    }

    fn selected_rows(app: &App, combobox: Entity) -> Vec<usize> {
        rows(app, combobox)
            .into_iter()
            .filter(|row| app.world().get::<Selected>(*row).is_some())
            .filter_map(|row| app.world().get::<OptionIndex>(row).map(|index| index.0))
            .collect()
    }

    /// The list a pick is made in, as the select's list box.
    fn list(app: &App, combobox: Entity) -> Entity {
        descendants_of(app.world(), combobox)
            .into_iter()
            .find(|entity| app.world().get::<ListBox>(*entity).is_some())
            .expect("the select holds a list")
    }

    /// The list box reports a pick on a row the way a click or Enter does.
    fn pick(app: &mut App, combobox: Entity, index: usize) {
        let list = list(app, combobox);
        let row = rows(app, combobox)[index];
        app.world_mut().trigger(ValueChange {
            source: list,
            value: row,
            is_final: true,
        });
        app.world_mut().flush();
    }

    /// A dropdown is a feathers select with its caption, rows and selection in
    /// place as soon as the spawn lands, before any frame has run.
    #[test]
    fn a_dropdown_is_complete_on_the_frame_it_spawns() {
        let mut app = app();
        let combobox = app
            .world_mut()
            .spawn(combobox_with_selected(vec!["px", "%", "auto"], 1))
            .id();
        app.world_mut().flush();

        let world = app.world();
        assert!(world.get::<FeathersSelect>(combobox).is_some());
        assert_eq!(
            world.get::<Node>(combobox).map(|node| node.width),
            Some(percent(100)),
            "the select keeps the width it was spawned with",
        );
        let parts: Vec<Entity> = descendants_of(world, combobox);
        assert!(
            parts
                .iter()
                .any(|e| world.get::<ComboBoxTrigger>(*e).is_some()
                    && world.get::<FeathersMenuButton>(*e).is_some()),
            "the trigger is the select's menu button",
        );
        assert!(
            parts
                .iter()
                .any(|e| world.get::<ComboBoxPopover>(*e).is_some()
                    && world.get::<FeathersMenuPopup>(*e).is_some()),
            "the list is the select's menu popup",
        );
        assert_eq!(label(&app, combobox), "%");
        assert_eq!(rows(&app, combobox).len(), 3, "one row per option");
        assert_eq!(selected_rows(&app, combobox), vec![1]);
    }

    /// The trigger and the rows set their text at the editor's size, not at
    /// the larger size feathers gives a menu.
    #[test]
    fn a_dropdown_reads_at_the_editor_text_size() {
        let (app, combobox) = app_with_combobox();
        let world = app.world();
        let sized: Vec<Entity> = descendants_of(world, combobox)
            .into_iter()
            .filter(|e| {
                world.get::<FeathersMenuButton>(*e).is_some()
                    || world.get::<OptionIndex>(*e).is_some()
            })
            .collect();
        assert_eq!(sized.len(), 4, "the trigger and three rows");
        for entity in sized {
            assert_eq!(
                world
                    .get::<InheritableFont>(entity)
                    .map(|font| font.font_size),
                Some(tokens::TEXT_SIZE),
            );
        }
    }

    /// Choosing a row sends the change event with that option, and the trigger
    /// reads the new option.
    #[test]
    fn choosing_an_option_sends_the_change_event() {
        let mut app = app();
        let combobox = app
            .world_mut()
            .spawn(combobox_with_selected(
                vec![
                    ComboBoxOptionData::new("Linear").with_value("linear"),
                    ComboBoxOptionData::new("Step").with_value("step"),
                ],
                0,
            ))
            .id();
        app.update();

        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = heard.clone();
        app.world_mut()
            .add_observer(move |change: On<ComboBoxChangeEvent>| {
                sink.lock().expect("no other thread holds it").push((
                    change.entity,
                    change.selected,
                    change.label.clone(),
                    change.value.clone(),
                ));
            });

        pick(&mut app, combobox, 1);
        app.update();

        assert_eq!(
            heard.lock().expect("no other thread holds it").as_slice(),
            [(combobox, 1, "Step".to_string(), Some("step".to_string()))],
        );
        assert_eq!(label(&app, combobox), "Step");
        assert_eq!(selected_rows(&app, combobox), vec![1]);
        assert_eq!(
            app.world()
                .get::<ComboBoxSelectedIndex>(combobox)
                .map(|index| index.0),
            Some(1),
        );
    }

    /// An option's icon is drawn on its row and, once picked, on the trigger.
    #[test]
    fn an_option_icon_shows_on_its_row_and_on_the_trigger() {
        let mut app = app();
        let combobox = app
            .world_mut()
            .spawn(combobox_with_selected(
                vec![
                    ComboBoxOptionData::new("Plain"),
                    ComboBoxOptionData::new("Box").with_icon(Icon::Box),
                ],
                0,
            ))
            .id();
        app.update();

        let glyph = String::from(Icon::Box.unicode());
        let row = rows(&app, combobox)[1];
        assert!(
            descendants_of(app.world(), row)
                .iter()
                .any(|e| app.world().get::<Text>(*e).is_some_and(|t| t.0 == glyph)),
            "the row draws its option's icon",
        );
        let trigger_glyph = app
            .world()
            .get::<ComboBoxParts>(combobox)
            .and_then(|parts| parts.glyph)
            .expect("a combobox with icons has a trigger glyph");
        assert_eq!(
            app.world().get::<Node>(trigger_glyph).map(|n| n.display),
            Some(Display::None),
            "an option without an icon shows none",
        );

        pick(&mut app, combobox, 1);
        app.update();

        assert_eq!(
            app.world().get::<Node>(trigger_glyph).map(|n| n.display),
            Some(Display::Flex),
        );
        assert_eq!(
            app.world().get::<Text>(trigger_glyph).map(|t| t.0.clone()),
            Some(glyph),
        );
        assert_eq!(label(&app, combobox), "Box");
    }

    /// A combobox used as a menu keeps the caption it was given, and the same
    /// entry can be picked twice.
    #[test]
    fn a_caption_of_its_own_survives_a_pick() {
        let mut app = app();
        let combobox = app
            .world_mut()
            .spawn(combobox_with_label(vec!["Field", "Action"], "Add Binding"))
            .id();
        app.update();
        assert_eq!(label(&app, combobox), "Add Binding");
        assert!(selected_rows(&app, combobox).is_empty());

        let picks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = picks.clone();
        app.world_mut()
            .add_observer(move |change: On<ComboBoxChangeEvent>| {
                sink.lock()
                    .expect("no other thread holds it")
                    .push(change.selected);
            });

        pick(&mut app, combobox, 1);
        app.update();
        // The select's caption sync writes the picked row's text over the
        // caption; the combobox puts its own back before layout.
        let caption = app.world().get::<ComboBoxParts>(combobox).unwrap().caption;
        app.world_mut().get_mut::<Text>(caption).unwrap().0 = "Action".into();
        app.update();
        assert_eq!(label(&app, combobox), "Add Binding");
        assert!(selected_rows(&app, combobox).is_empty());

        pick(&mut app, combobox, 1);
        app.update();
        assert_eq!(
            picks.lock().expect("no other thread holds it").as_slice(),
            [1, 1],
        );

        app.world_mut()
            .entity_mut(combobox)
            .insert(ComboBoxSelectedIndex(0));
        app.update();
        assert_eq!(label(&app, combobox), "Add Binding");
    }

    /// The label follows a selection written from outside, not only a
    /// pick, so the inspector can put a combobox back where an undo left it.
    #[test]
    fn an_externally_set_index_repaints_the_label() {
        let (mut app, combobox) = app_with_combobox();
        assert_eq!(label(&app, combobox), "px");

        app.world_mut()
            .entity_mut(combobox)
            .insert(ComboBoxSelectedIndex(2));
        app.update();

        assert_eq!(label(&app, combobox), "auto");
        assert_eq!(selected_rows(&app, combobox), vec![2]);
    }

    /// An index that names no option is a caller bug; the widget keeps
    /// showing what it has rather than landing somewhere arbitrary.
    #[test]
    fn an_out_of_range_index_leaves_the_selection_alone() {
        let (mut app, combobox) = app_with_combobox();

        app.world_mut()
            .entity_mut(combobox)
            .insert(ComboBoxSelectedIndex(9));
        app.update();

        assert_eq!(label(&app, combobox), "px");
        assert_eq!(selected_rows(&app, combobox), vec![0]);
    }
}
