//! A picker widget for selecting from a list of items.
//!
//! A picker is created by spawning an entity with the [`PickerProps`] component.
//!
//! Example:
//! ```
//! use bevy::prelude::*;
//! use jackdaw_feathers::picker::*;
//!
//! fn spawn_picker(mut commands: Commands) {
//!     let items = vec!["Hello".to_string(), "Hi there".to_string()];
//!     let props = PickerProps::new(spawn_item, on_select).items(items);
//!     commands.spawn(props);
//! }
//!
//! // This is just a bevy system!
//! fn spawn_item(input: In<SpawnItemInput>, mut commands: Commands) -> Result {
//!     commands.entity(input.entities.picker).with_child((
//!         // Spawn your item here!
//!     ));
//!
//!     Ok(())
//! }
//!
//! fn on_select(input: In<SelectInput>, items: Query<&PickerItems<String>>) -> Result {
//!     let item = items.get(input.entities.picker)?.at(input.index)?;
//!     // do whatever you want with your item!
//!
//!     Ok(())
//! }
//! ```

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::relationship::RelatedSpawner;
use bevy::ecs::system::SystemId;
use bevy::ecs::world::DeferredWorld;
use bevy::feathers::font_styles::InheritableFont;
use bevy::feathers::handle_or_path::HandleOrPath;
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::InputFocus;
use bevy::input_focus::tab_navigation::{TabGroup, TabIndex};
use bevy::prelude::*;
use bevy_ui_text_input::SubmitText;
use jackdaw_fuzzy::FuzzyMatcher;
pub use jackdaw_fuzzy::{Category, Match, Matchable, MatchedStr};
use lucide_icons::Icon;

use crate::button::{
    ButtonClickEvent, ButtonSize, ButtonVariant, IconButtonProps, button_base, icon_button,
};
use crate::icons::{EditorFont, IconFont};
use crate::scroll::scrollbar;
use crate::separator::{SeparatorProps, separator};
use crate::text_edit::{TextEditProps, TextEditValue, text_edit};
use crate::tokens;

/// This trait is implemented for anything that implements [`Matchable`], [`Send`] and [`Sync`].
/// It's used for trait bounds when creating a [`Picker`]
pub trait Pickable: Matchable + Send + Sync + 'static {}

impl<T: Matchable + Send + Sync + 'static> Pickable for T {}

/// A picker, used for selecting from a list of items.
/// Created by spawning an entity with the [`PickerProps`] component. See the [module docs](crate::picker) for more info
#[derive(Component)]
#[component(on_replace)]
pub struct Picker {
    dismissible: bool,
    matcher: FuzzyMatcher<Item>,
    spawn_item: SystemId<In<SpawnItemInput>, Result>,
    on_select: SystemId<In<SelectInput>, Result>,
    on_dismiss: SystemId<In<PickerEntities>, Result>,
}

/// Relationship target representing the text input of a [`Picker`]
#[derive(Component, Deref, Debug, PartialEq, Clone)]
#[relationship_target(relationship = PickerInputOf)]
pub struct WithPickerInput(Entity);

/// Relationship target representing the list which contains the items of a [`Picker`]
#[derive(Component, Deref, Debug, PartialEq, Clone)]
#[relationship_target(relationship = PickerListOf)]
pub struct WithPickerList(Entity);

/// Relationship representing the text input of a [`Picker`]
#[derive(Component, Deref, Debug, PartialEq, Clone)]
#[relationship(relationship_target = WithPickerInput)]
pub struct PickerInputOf(pub Entity);

/// Relationship representing the list which contains the items of a [`Picker`]
#[derive(Component, Deref, Debug, PartialEq, Clone)]
#[relationship(relationship_target = WithPickerList)]
pub struct PickerListOf(pub Entity);

/// The entities related to a [`Picker`]
#[derive(Debug, PartialEq, Clone)]
pub struct PickerEntities {
    /// The actual picker entity itself
    pub picker: Entity,
    /// The entity which has the text input used by the picker
    pub input: Entity,
    /// The entity which items of the picker should be spawned as chilren of
    pub list: Entity,
}

/// The input to the system which spawns the items in a [`Picker`]. See [`PickerProps`]
#[derive(Debug, PartialEq, Clone)]
pub struct SpawnItemInput {
    /// This item's [`Match`]
    pub matched: Match,
    /// The entities related to this picker
    pub entities: PickerEntities,
}

/// The input to the system which spawns the items in a [`Picker`]. See [`PickerProps`]
#[derive(Debug, PartialEq, Clone)]
pub struct SelectInput {
    /// The index of the selected item
    pub index: usize,
    /// The entities related to this picker
    pub entities: PickerEntities,
}

/// An event triggered when a picker's item should be selected.
/// It's usually triggered by a [`picker_item`] and consumed by a [`Picker`]
#[derive(EntityEvent, Debug, PartialEq, Clone)]
pub struct PickerSelect {
    /// The picker entity
    pub entity: Entity,
    /// The index of the item
    pub index: usize,
}

/// Properties for creating a [`Picker`].
///
/// A picker uses bevy systems for spawning an item, selecting an item and dismissing the picker.
#[derive(Component)]
#[component(on_insert)]
pub struct PickerProps<T: Pickable> {
    items: Option<Vec<T>>,
    title: Option<String>,
    placeholder: Option<String>,
    dismissible: bool,
    register_spawn_item: Option<
        Box<dyn FnOnce(&mut Commands) -> SystemId<In<SpawnItemInput>, Result> + Send + Sync>,
    >,
    register_on_select:
        Option<Box<dyn FnOnce(&mut Commands) -> SystemId<In<SelectInput>, Result> + Send + Sync>>,
    register_on_dismiss: Option<
        Box<dyn FnOnce(&mut Commands) -> SystemId<In<PickerEntities>, Result> + Send + Sync>,
    >,
}

impl<T: Pickable> PickerProps<T> {
    fn on_insert(mut world: DeferredWorld, ctx: HookContext) {
        let font = world.resource::<EditorFont>().0.clone();
        let icon_font = world.resource::<IconFont>().0.clone();

        let (mut entities, mut commands) = world.entities_and_commands();
        let Ok(mut entity) = entities.get_mut(ctx.entity) else {
            return;
        };

        let Some(mut props) = entity.get_mut::<Self>() else {
            return;
        };

        let Some(register_spawn_item) = props.register_spawn_item.take() else {
            return;
        };
        let Some(register_on_select) = props.register_on_select.take() else {
            return;
        };
        let Some(register_on_dismiss) = props.register_on_dismiss.take() else {
            return;
        };

        let spawn_item = (register_spawn_item)(&mut commands);
        let on_select = (register_on_select)(&mut commands);
        let on_dismiss = (register_on_dismiss)(&mut commands);

        let items = props.items.take().unwrap_or_default();
        let erased_items = items.iter().map(|item| Item {
            haystack: item.haystack(),
            category: item.category(),
        });
        let matcher = FuzzyMatcher::from_items(erased_items);

        let picker = Picker {
            matcher,
            spawn_item,
            on_select,
            on_dismiss,
            dismissible: props.dismissible,
        };

        let mut text_edit_props = TextEditProps::default().auto_focus();

        if let Some(placeholder) = props.placeholder.take() {
            text_edit_props = text_edit_props.with_placeholder(placeholder);
        }

        let input = commands.spawn(text_edit(text_edit_props)).id();

        let list = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                width: percent(100),
                max_height: px(400),
                overflow: Overflow::scroll_y(),
                ..default()
            })
            .id();

        let scrollbar = commands.spawn(scrollbar(list)).id();

        let mut dismiss = if props.dismissible {
            Some((
                PickerDismissButton,
                icon_button(
                    IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                    &icon_font,
                ),
            ))
        } else {
            None
        };

        let mut header_items = vec![];

        if let Some(title) = props.title.take() {
            let dismiss = dismiss.take();
            let titlebar = commands
                .spawn((
                    Node {
                        align_items: AlignItems::Center,
                        width: percent(100),
                        ..default()
                    },
                    Children::spawn(SpawnWith(|spawner: &mut RelatedSpawner<ChildOf>| {
                        spawner.spawn((
                            Node {
                                flex_grow: 1.0,
                                justify_content: JustifyContent::Center,
                                ..default()
                            },
                            children![(
                                Text(title),
                                TextFont {
                                    font,
                                    font_size: tokens::TEXT_SIZE_XL,
                                    weight: FontWeight::SEMIBOLD,
                                    ..default()
                                }
                            )],
                        ));

                        if let Some(dismiss) = dismiss {
                            spawner.spawn(dismiss);
                        }
                    })),
                ))
                .id();

            header_items.push(titlebar);
        }

        let mut input_container = commands.spawn(Node {
            width: percent(100),
            column_gap: px(tokens::SPACING_SM),
            align_items: AlignItems::Center,
            ..default()
        });

        input_container.add_child(input);

        if let Some(dismiss) = dismiss {
            // if we put the dismiss button in the title bar with no title, it looks ugly
            // because there's a lot of empty space so we put it after the input instead
            input_container.with_child(dismiss);
        }

        header_items.push(input_container.id());

        let picker_entity = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    border: px(1).all(),
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                    row_gap: px(tokens::SPACING_MD),
                    width: px(600),
                    ..default()
                },
                BorderColor::all(tokens::BORDER_COLOR),
                BackgroundColor(tokens::PANEL_BG),
                TabGroup::modal(),
                BoxShadow(vec![ShadowStyle {
                    x_offset: Val::ZERO,
                    y_offset: Val::Px(4.0),
                    blur_radius: Val::Px(16.0),
                    spread_radius: Val::ZERO,
                    color: tokens::SHADOW_COLOR,
                }]),
            ))
            .with_child((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: ButtonSize::MD.padding().all().with_bottom(px(0)),
                    row_gap: px(tokens::SPACING_MD),
                    ..default()
                },
                Children::spawn(WithRelated::new(header_items)),
            ))
            .with_child((
                Node {
                    width: percent(100),
                    ..default()
                },
                Children::spawn(WithRelated::new([scrollbar, list])),
            ))
            .id();

        commands
            .entity(ctx.entity)
            .insert((
                Node {
                    height: percent(100),
                    width: percent(100),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                PickerItems(items.into_boxed_slice()),
                GlobalZIndex(200),
                picker,
            ))
            .add_one_related::<PickerInputOf>(input)
            .add_one_related::<PickerListOf>(list)
            .add_child(picker_entity);

        // we no longer need it here, it's done its job
        commands.entity(ctx.entity).remove::<Self>();
    }
}

/// The items of a [`Picker`]
#[derive(Component, Debug, Default, PartialEq, Clone)]
pub struct PickerItems<T: Pickable>(Box<[T]>);

impl<T: Pickable> PickerItems<T> {
    /// Get a reference to the slice of items
    pub fn items(&self) -> &[T] {
        &self.0
    }

    /// Get the item at the given index, returning a `Result<&T, BevyError>`
    /// for easy error propagation
    pub fn at(&self, index: usize) -> Result<&T> {
        // return a `BevyError` so you can just `?` it
        self.0
            .get(index)
            .ok_or_else(|| BevyError::from(format!("No item at index {index}")))
    }
}

/// A single item in a [`Picker`]. Created with the [`picker_item`] function
#[derive(Component, Debug, Default, PartialEq, Clone, Copy)]
pub struct PickerItem(pub usize);

/// Creates a selectable [`Picker`] item with the given index.
#[must_use]
pub fn picker_item(index: usize) -> impl Bundle {
    (
        button_base(
            ButtonVariant::Ghost,
            ButtonSize::MD,
            true,
            FlexDirection::Column,
            BorderRadius::ZERO,
        ),
        PickerItem(index),
        // if everything is the same tab index, it's ordered by the child index
        TabIndex(0),
    )
}

fn on_picker_item_activated(
    trigger: On<ButtonClickEvent>,
    item: Query<&PickerItem>,
    list: Query<&PickerListOf>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Ok(item) = item.get(trigger.entity) else {
        return;
    };

    let Some(list_of) = std::iter::once(trigger.entity)
        .chain(child_of.iter_ancestors(trigger.entity))
        .find_map(|e| list.get(e).ok())
    else {
        return;
    };

    commands.trigger(PickerSelect {
        entity: list_of.0,
        index: item.0,
    });
}

fn scroll_to_picker_item(
    picker_items: Query<(&ComputedNode, &UiGlobalTransform, &ChildOf), With<PickerItem>>,
    mut scroll_position: Query<(&mut ScrollPosition, &ComputedNode, &UiGlobalTransform)>,
    focus: Res<InputFocus>,
) {
    if !focus.is_changed() {
        return;
    };

    let Some(focused) = focus.0 else {
        return;
    };

    let Ok((computed, transform, parent)) = picker_items.get(focused) else {
        return;
    };

    let Ok((mut scroll_position, parent_computed, parent_transform)) =
        scroll_position.get_mut(parent.0)
    else {
        return;
    };

    let child_top = transform.translation.y - computed.size().y / 2.0;
    let child_bottom = transform.translation.y + computed.size().y / 2.0;
    let parent_top = parent_transform.translation.y - parent_computed.content_box().size().y / 2.0;

    // since scrolling changes the child positions, we add back the scroll to counteract that
    let child_top_relative = child_top - parent_top + scroll_position.y;
    let child_bottom_relative = child_bottom - parent_top + scroll_position.y;

    // the bottom most visible point
    let bottom_visible = scroll_position.y + parent_computed.content_box().size().y;

    // ui position increases downwards, so if the top is above the scroll position, we scroll
    if child_top_relative < scroll_position.y {
        // off screen at the top
        scroll_position.y = child_top_relative;
    }

    // and if the bottom is below the bottom most visible point, we scroll
    if child_bottom_relative > bottom_visible {
        // off screen at the bottom
        // subtract to account for the parent size
        scroll_position.y = f32::max(
            child_bottom_relative - parent_computed.content_box().size().y,
            0.0,
        );
    }
}

#[derive(Component)]
struct PickerDismissButton;

/// Trigger this event to dismiss a [`Picker`]
#[derive(EntityEvent)]
pub struct DismissPickerEvent(pub Entity);

fn on_dismiss_activated(
    trigger: On<ButtonClickEvent>,
    picker_dismiss_query: Query<(), With<PickerDismissButton>>,
    child_of: Query<&ChildOf>,
    picker_query: Query<Entity, With<Picker>>,
    mut commands: Commands,
) {
    if picker_dismiss_query.get(trigger.entity).is_err() {
        return;
    };

    let Some(picker_entity) = std::iter::once(trigger.entity)
        .chain(child_of.iter_ancestors(trigger.entity))
        .find_map(|e| picker_query.get(e).ok())
    else {
        return;
    };

    commands.trigger(DismissPickerEvent(picker_entity));
}

fn on_picker_dismissed(
    trigger: On<DismissPickerEvent>,
    pickers: Query<(&Picker, &WithPickerInput, &WithPickerList)>,
    mut commands: Commands,
) {
    let Ok((picker, input, list)) = pickers.get(trigger.0) else {
        return;
    };

    if !picker.dismissible {
        return;
    }

    let picker_entity = trigger.0;

    let entities = PickerEntities {
        picker: picker_entity,
        input: input.0,
        list: list.0,
    };

    let on_dismiss = picker.on_dismiss;

    commands.queue(move |world: &mut World| {
        if let Err(e) = world.run_system_with(on_dismiss, entities) {
            error!("Error when dismissing picker {picker_entity}: {e}");
        }
    });
}

#[derive(Component)]
#[component(on_insert)]
struct MatchText;

impl MatchText {
    fn on_insert(mut world: DeferredWorld, ctx: HookContext) {
        let font = world.resource::<EditorFont>().0.clone();
        let mut commands = world.commands();
        commands.entity(ctx.entity).insert(InheritableFont {
            font: HandleOrPath::Handle(font),
            font_size: tokens::TEXT_SIZE,
        });
    }
}

/// Create a [`Text`] with multiple [`TextSpan`]s corresponding to the different segments.
/// The segments that match the input string are highlighted with the text accent color. See [`MatchedStr`]
pub fn match_text(segments: Box<[MatchedStr]>) -> impl Bundle {
    let mut spans = Vec::with_capacity(segments.len());

    for segment in segments {
        let color = if segment.is_match {
            tokens::TEXT_ACCENT
        } else {
            tokens::TEXT_PRIMARY
        };
        spans.push((TextSpan(segment.text), ThemedText, TextColor(color)));
    }

    (
        Text::default(),
        Children::spawn(spans),
        MatchText,
        ThemedText,
    )
}

fn process_pickers(
    pickers: Query<(Entity, &mut Picker, &WithPickerInput, &WithPickerList)>,
    text_edits: Query<&TextEditValue, Changed<TextEditValue>>,
    font: Res<EditorFont>,
    mut commands: Commands,
) {
    for (picker_entity, mut picker, input_entity, list) in pickers {
        let Ok(input) = text_edits.get(input_entity.0) else {
            continue;
        };
        commands.entity(list.0).despawn_children();

        picker.matcher.update_pattern(&input.0);

        let spawn_item = picker.spawn_item;

        let matches = picker.matcher.matches();
        for (index, category) in matches.into_iter().enumerate() {
            let font = font.0.clone();
            let name = category.category.name;

            // don't spawn it if the first category is unnamed
            if name.is_some() || index != 0 {
                commands.entity(list.0).with_child((
                    Node {
                        margin: px(tokens::SPACING_SM).top(),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    Children::spawn(SpawnWith(move |spawner: &mut RelatedSpawner<ChildOf>| {
                        if let Some(name) = name {
                            spawner.spawn((
                                Node {
                                    margin: px(tokens::SPACING_MD).horizontal(),
                                    ..default()
                                },
                                Text(name),
                                TextFont::from(font).with_font_size(tokens::TEXT_SIZE_SM),
                                TextColor(tokens::TEXT_MUTED_COLOR.into()),
                            ));
                        }
                        spawner.spawn(separator(SeparatorProps::horizontal()));
                    })),
                ));
            }

            for matched in category.items {
                let input = SpawnItemInput {
                    matched,
                    entities: PickerEntities {
                        picker: picker_entity,
                        input: input_entity.0,
                        list: list.0,
                    },
                };

                commands.queue(move |world: &mut World| {
                    if let Err(e) = world.run_system_with(spawn_item, input) {
                        error!("Error when spawning item for picker {picker_entity}: {e}");
                    }
                });
            }
        }
    }
}

fn on_picker_selected(
    trigger: On<PickerSelect>,
    pickers: Query<(&Picker, &WithPickerInput, &WithPickerList)>,
    mut commands: Commands,
) {
    let Ok((picker, input, list)) = pickers.get(trigger.entity) else {
        return;
    };

    let picker_entity = trigger.entity;

    let input = SelectInput {
        index: trigger.index,
        entities: PickerEntities {
            picker: picker_entity,
            input: input.0,
            list: list.0,
        },
    };

    let on_select = picker.on_select;
    commands.queue(move |world: &mut World| {
        if let Err(e) = world.run_system_with(on_select, input) {
            error!("Error when selecting item on picker {picker_entity}: {e}");
        }
    });
}

fn on_text_edit_submit(
    mut submit_messages: MessageReader<SubmitText>,
    inputs: Query<&PickerInputOf>,
    child_of: Query<&ChildOf>,
    mut pickers: Query<(Entity, &mut Picker)>,
    mut commands: Commands,
) {
    for submit in submit_messages.read() {
        // please give me relational queries i'm begging
        let Some(input_of) = std::iter::once(submit.entity)
            .chain(child_of.iter_ancestors(submit.entity))
            .find_map(|e| inputs.get(e).ok())
        else {
            continue;
        };

        let Ok((picker_entity, mut picker)) = pickers.get_mut(input_of.0) else {
            continue;
        };

        picker.matcher.update_pattern(&submit.text);
        let matches = picker.matcher.matches();
        let Some(first) = matches.first().and_then(|c| c.items.first()) else {
            continue;
        };

        commands.trigger(PickerSelect {
            entity: picker_entity,
            index: first.index,
        });
    }
}

impl<T: Pickable> PickerProps<T> {
    /// Create a new [`PickerProps`] with two systems:
    /// - one to spawn the item, with a [`SpawnItemInput`]
    /// - one that gets triggered when an item is selected, with a [`SelectInput`]
    #[must_use]
    pub fn new<S1, M1, S2, M2>(spawn_item: S1, on_select: S2) -> Self
    where
        S1: IntoSystem<In<SpawnItemInput>, Result, M1>,
        S2: IntoSystem<In<SelectInput>, Result, M2>,
    {
        let spawn_item = IntoSystem::into_system(spawn_item);
        let on_select = IntoSystem::into_system(on_select);
        Self {
            items: Some(vec![]),
            dismissible: true,
            placeholder: Some(String::from("Search")),
            title: None,
            register_spawn_item: Some(Box::new(move |commands| {
                commands.register_system(spawn_item)
            })),
            register_on_select: Some(Box::new(move |commands| {
                commands.register_system(on_select)
            })),
            register_on_dismiss: Some(Box::new(move |commands| {
                commands.register_system(|entities: In<PickerEntities>, mut commands: Commands| {
                    commands.entity(entities.picker).try_despawn();
                    Ok(())
                })
            })),
        }
    }

    /// Sets the placeholder of the text input. Default is `Some("Search")`
    #[must_use]
    pub fn placeholder(mut self, placeholder: Option<impl Into<String>>) -> Self {
        self.placeholder = placeholder.map(Into::into);
        self
    }

    /// Sets the picker's items from the  given iterator
    #[must_use]
    pub fn items(mut self, items: impl IntoIterator<Item = T>) -> Self {
        self.items.get_or_insert_default().extend(items);
        self
    }

    /// Pushes a single item to the picker's items
    #[must_use]
    pub fn item(mut self, item: T) -> Self {
        self.items.get_or_insert_default().push(item);
        self
    }

    /// Sets the title of the picker
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets whether the picker should be dismissible with ESC or the dismiss icon. True by default
    #[must_use]
    pub fn dismissible(mut self, value: bool) -> Self {
        self.dismissible = value;
        self
    }

    /// Set the system to be run when the picker is dismissed
    #[must_use]
    pub fn on_dismiss<S, M>(mut self, on_dismiss: S) -> Self
    where
        S: IntoSystem<In<PickerEntities>, Result, M>,
    {
        let on_dismiss = IntoSystem::into_system(on_dismiss);
        self.register_on_dismiss = Some(Box::new(move |commands| {
            commands.register_system(on_dismiss)
        }));

        self
    }
}

impl Picker {
    fn on_replace(mut world: DeferredWorld, ctx: HookContext) {
        let entity = world.entity(ctx.entity);
        let Some(picker) = entity.get::<Self>() else {
            return;
        };

        let (spawn_item, on_select, on_dismiss) =
            (picker.spawn_item, picker.on_select, picker.on_dismiss);
        let mut commands = world.commands();

        // Clean up after ourselves!
        commands.unregister_system(spawn_item);
        commands.unregister_system(on_select);
        commands.unregister_system(on_dismiss);
    }
}

struct Item {
    haystack: String,
    category: Category,
}

impl Matchable for Item {
    fn haystack(&self) -> String {
        self.haystack.clone()
    }

    fn category(&self) -> Category {
        self.category.clone()
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (process_pickers, on_text_edit_submit, scroll_to_picker_item),
    )
    .add_observer(on_picker_selected)
    .add_observer(on_picker_dismissed)
    .add_observer(on_picker_item_activated)
    .add_observer(on_dismiss_activated);
}
