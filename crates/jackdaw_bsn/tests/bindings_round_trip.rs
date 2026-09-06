//! `Bindings` through the document and back.
//!
//! A binding is a list of enums holding raw path strings, options and tuples,
//! every shape the read path is weakest on gathered into one component.
//!
//! These pin the whole circuit: a hand-authored document loads as exactly the
//! value it spells, and saving what loaded reproduces the same document.

use bevy::ecs::name::Name;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::world::World;

use jackdaw_bind::{BindPath, Binding, Bindings};
use jackdaw_bsn::{apply_dirty_ast_patches, parse_bsn_text, serialize_to_bsn, spawn_from_ast};

/// The document under test, written the way a user's scene file holds it:
/// full type paths throughout, a filled `via` option, a percent flag, and an
/// action's field mappings as pairs.
const AUTHORED: &str = r#"#Bar
jackdaw_bind::types::Bindings([
    jackdaw_bind::types::Binding::Field { read: [jackdaw_bind::types::BindPath { raw: "game::hud::Health.current" }, jackdaw_bind::types::BindPath { raw: "game::hud::Health.max" }], via: core::option::Option::Some("ratio"), write: jackdaw_bind::types::BindPath { raw: "bevy_ui::ui_node::Node.width" }, as_percent: true },
    jackdaw_bind::types::Binding::Action { event: "game::hud::RetryPressed", fields: [["slot", jackdaw_bind::types::BindPath { raw: "game::hud::Save.slot" }]] },
])
"#;

/// The same value in Rust. It has to match [`AUTHORED`] read by eye; the two
/// are kept in step by hand.
fn authored_value() -> Bindings {
    Bindings(vec![
        Binding::Field {
            read: vec![
                BindPath::new("game::hud::Health.current"),
                BindPath::new("game::hud::Health.max"),
            ],
            via: Some("ratio".to_string()),
            write: BindPath::new("bevy_ui::ui_node::Node.width"),
            as_percent: true,
        },
        Binding::Action {
            event: "game::hud::RetryPressed".to_string(),
            fields: vec![("slot".to_string(), BindPath::new("game::hud::Save.slot"))],
            literals: Vec::new(),
        },
    ])
}

/// Parse, spawn and apply: the read path a scene file takes.
fn load(text: &str) -> World {
    let mut world = World::new();
    let registry = AppTypeRegistry::default();
    registry.write().register::<Bindings>();
    world.insert_resource(registry);

    let ast = parse_bsn_text(text).expect("the document parses");
    world.insert_resource(ast);
    spawn_from_ast(&mut world);
    apply_dirty_ast_patches(&mut world);
    world
}

fn bindings_of(world: &mut World, name: &str) -> Bindings {
    world
        .query::<(&Name, &Bindings)>()
        .iter(world)
        .find(|(entity_name, _)| entity_name.as_str() == name)
        .map(|(_, bindings)| bindings.clone())
        .unwrap_or_else(|| panic!("no entity named {name} carries Bindings"))
}

#[test]
fn an_authored_binding_loads_as_the_value_it_spells() {
    let mut world = load(AUTHORED);
    assert_eq!(
        bindings_of(&mut world, "Bar"),
        authored_value(),
        "every path, option and pair survives the read path exactly",
    );
}

#[test]
fn saving_what_loaded_reproduces_the_document() {
    let world = load(AUTHORED);
    let saved = serialize_to_bsn(&world);
    assert!(
        saved.contains("game::hud::Health.current"),
        "the raw paths reach the file as written:\n{saved}",
    );

    let mut reloaded = load(&saved);
    assert_eq!(
        bindings_of(&mut reloaded, "Bar"),
        authored_value(),
        "load(save(doc)) is the same value as load(doc)",
    );
    assert_eq!(
        serialize_to_bsn(&reloaded),
        saved,
        "and saving it again is a fixpoint",
    );
}

/// The document as it was written before an action could carry constants. The
/// field it leaves out is one the value takes a default for, so the whole
/// binding still arrives.
const BEFORE_LITERALS: &str = r#"#Bar
jackdaw_bind::types::Bindings([
    jackdaw_bind::types::Binding::Action { event: "game::hud::RetryPressed", fields: [["slot", jackdaw_bind::types::BindPath { raw: "game::hud::Save.slot" }]] },
])
"#;

#[test]
fn an_action_saved_without_literals_still_loads() {
    let mut world = load(BEFORE_LITERALS);
    assert_eq!(
        bindings_of(&mut world, "Bar"),
        Bindings(vec![Binding::Action {
            event: "game::hud::RetryPressed".to_string(),
            fields: vec![("slot".to_string(), BindPath::new("game::hud::Save.slot"))],
            literals: Vec::new(),
        }]),
        "an older document keeps its mapping and takes no constants",
    );
}
