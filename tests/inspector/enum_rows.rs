//! Enum fields in the generic card: each has its field's label, unit and data
//! enums at one depth start at the same inset, and the lone value of a tuple
//! variant keeps a caption only when no label names it already.

use bevy::prelude::*;
use jackdaw::selection::Selection;

use crate::util;

#[derive(Reflect, Clone, PartialEq, Debug, Default)]
enum Mode {
    #[default]
    Fast,
    Slow,
}

#[derive(Reflect, Clone, PartialEq, Debug)]
struct Inner {
    mode: Mode,
    limit: Option<f32>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            mode: Mode::Fast,
            limit: Some(3.0),
        }
    }
}

#[derive(Component, Reflect, Clone, PartialEq, Debug, Default)]
#[reflect(Component, Default)]
struct Tuning {
    inner: Inner,
}

#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default)]
enum Weight {
    Kilos(f32),
    Unknown,
}

impl Default for Weight {
    fn default() -> Self {
        Self::Kilos(2.0)
    }
}

fn inspected(component: impl Bundle) -> App {
    let mut app = util::editor_test_app();
    app.register_type::<Tuning>().register_type::<Weight>();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.update();
    let entity = app
        .world_mut()
        .spawn((Name::new("tuned"), Transform::default(), component))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, entity);
        });
    app.world_mut().flush();
    for _ in 0..6 {
        app.update();
    }
    app
}

/// The text entity reading `label`.
fn label(app: &mut App, label: &str) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .filter(|(_, text)| text.0 == label)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one `{label}` label");
    found[0]
}

/// The left padding and margin `entity` and every ancestor add up to.
fn inset(app: &App, entity: Entity) -> f32 {
    let px = |value: Val| match value {
        Val::Px(px) => px,
        _ => 0.0,
    };
    let mut total = 0.0;
    let mut at = Some(entity);
    while let Some(current) = at {
        if let Some(node) = app.world().get::<Node>(current) {
            total += px(node.padding.left) + px(node.margin.left);
        }
        at = app.world().get::<ChildOf>(current).map(ChildOf::parent);
    }
    total
}

#[test]
fn a_nested_unit_enum_and_option_are_labelled_and_line_up() {
    let mut app = inspected(Tuning::default());
    let mode = label(&mut app, "mode");
    let limit = label(&mut app, "limit");
    assert_eq!(
        inset(&app, mode),
        inset(&app, limit),
        "a unit and a data enum at one depth start together"
    );
    let text = jackdaw::inspector::component_card_text(
        app.world_mut(),
        <Tuning as bevy::reflect::TypePath>::type_path(),
    );
    assert!(
        !text.iter().any(|line| line == "0"),
        "the labelled option's value has no bare index: {text:?}"
    );
}

#[test]
fn a_whole_component_enum_keeps_a_caption_for_its_value() {
    let mut app = inspected(Weight::default());
    let text = jackdaw::inspector::component_card_text(
        app.world_mut(),
        <Weight as bevy::reflect::TypePath>::type_path(),
    );
    assert!(
        text.iter().any(|line| line.trim_end_matches(':') == "0"),
        "the value is captioned: {text:?}"
    );
}
