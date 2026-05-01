use bevy::feathers::FeathersPlugins;
use bevy::input_focus::InputDispatchPlugin;
use bevy::prelude::*;
use jackdaw_feathers::EditorFeathersPlugin;
use jackdaw_feathers::picker::{
    PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text, picker_item,
};
use jackdaw_fuzzy::{Category, Matchable};

struct Searchable {
    haystack: String,
    category: Option<String>,
}

impl Searchable {
    fn new(haystack: impl Into<String>, category: Option<&'static str>) -> Self {
        Self {
            haystack: haystack.into(),
            category: category.map(Into::into),
        }
    }
}

impl Matchable for Searchable {
    fn haystack(&self) -> String {
        self.haystack.clone()
    }

    fn category(&self) -> Category {
        Category {
            order: 0,
            name: self.category.clone(),
        }
    }
}

fn spawn_picker(mut commands: Commands) {
    commands.spawn(Camera2d);

    let items = vec![
        Searchable::new("Hello world", Some("Greetings")),
        Searchable::new("Hello there", Some("Greetings")),
        Searchable::new("Hi there", Some("Greetings")),
        Searchable::new("Some text", Some("Fillers")),
        Searchable::new("Some more text", Some("Fillers")),
        Searchable::new("Another bit of text", Some("Fillers")),
        Searchable::new("A bunch more text", Some("Fillers")),
        Searchable::new("And another item to search", Some("Fillers")),
        Searchable::new("Yet more items to search", Some("Fillers")),
        Searchable::new("I'm running out of things to say", None),
        Searchable::new("Hello world 2", Some("Greetings 2")),
        Searchable::new("Hello there 2", Some("Greetings 2")),
        Searchable::new("Hi there 2", Some("Greetings 2")),
        Searchable::new("Some text 2", Some("Fillers 2")),
        Searchable::new("Some more text 2", Some("Fillers 2")),
        Searchable::new("Another bit of text 2", Some("Fillers 2")),
        Searchable::new("A bunch more text 2", Some("Fillers 2")),
        Searchable::new("And another item to search 2", Some("Fillers 2")),
        Searchable::new("Yet more items to search 2", Some("Fillers 2")),
        Searchable::new("I'm running out of things to say 2", None),
    ];

    let props = PickerProps::new(spawn_item, on_select)
        .items(items)
        .title("Hello world!");

    commands.spawn(props);
}

fn spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    mut commands: Commands,
) -> Result {
    commands.entity(entities.list).with_child((
        picker_item(matched.index),
        children![match_text(matched.segments)],
    ));

    Ok(())
}

fn on_select(input: In<SelectInput>, items: Query<&PickerItems<Searchable>>) -> Result {
    let item = &items.get(input.entities.picker)?.at(input.index)?;
    info!("Got item '{}'", item.haystack);

    Ok(())
}

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            // text edit enables InputDispatchPlugin unconditionally
            FeathersPlugins.build().disable::<InputDispatchPlugin>(),
            EditorFeathersPlugin,
        ))
        .add_systems(Startup, spawn_picker)
        .insert_resource(ClearColor(jackdaw_feathers::tokens::WINDOW_BG))
        .run()
}
