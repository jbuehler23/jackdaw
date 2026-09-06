//! A model in the outliner is one row.
//!
//! A `GltfSource` entity is an instance of a world asset and the loader spawns
//! the asset's own tree under it. None of that is in the document, and for a
//! moment the scene root is a named unparented entity, which was long enough for
//! the outliner to give it a top-level row of its own. What the asset spawned is
//! drawn below the instance, in the asset-part tone, and reaches no saved file.

use crate::util;

use bevy::prelude::*;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw_feathers::tree_view::category_color;
use jackdaw_widgets::tree_view::{
    EntityCategory, TreeIndex, TreeNode, TreeNodeExpanded, TreeRowContent, TreeRowDot,
};

/// A model the repository ships, so a real glTF's spawn order is exercised.
const MODEL: &str = "models/dungeon.glb";

/// An outliner panel over a scene holding `count` model instances, ticked
/// until the loader has spawned what the assets hold.
fn panel_over_models(count: usize) -> (App, Entity, Vec<Entity>) {
    let mut app = util::editor_test_app();
    let panel = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();

    let world = app.world_mut();
    let instances: Vec<Entity> = (0..count)
        .map(|i| {
            let entity = world
                .spawn((
                    Name::new(format!("Model_{i}")),
                    Transform::default(),
                    Visibility::default(),
                    jackdaw_scene_types::GltfSource {
                        path: MODEL.to_string(),
                        scene_index: 0,
                    },
                ))
                .id();
            jackdaw::scene_io::register_entity_in_ast(world, entity);
            entity
        })
        .collect();

    for _ in 0..600 {
        app.update();
        let spawned = instances.iter().all(|&instance| {
            app.world()
                .get::<Children>(instance)
                .is_some_and(|children| !children.is_empty())
        });
        if spawned {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // The loader's parent link and the outliner's answer to it land on later
    // flushes; give both room before reading the rows back.
    for _ in 0..8 {
        app.update();
    }
    (app, panel, instances)
}

/// The sources of the rows sitting at the panel's own top level.
fn top_level_sources(app: &App, panel: Entity) -> Vec<Entity> {
    let world = app.world();
    world
        .get::<Children>(panel)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| world.get::<TreeNode>(child).map(|node| node.0))
                .collect()
        })
        .unwrap_or_default()
}

/// The row for `source`, whether at the top level or below a branch.
fn row_for(app: &App, source: Entity, panel: Entity) -> Option<Entity> {
    app.world().resource::<TreeIndex>().get(panel, source)
}

/// The colour the row's glyph is drawn in.
fn glyph_color(app: &App, row: Entity) -> Color {
    let world = app.world();
    let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
        world
            .get::<Children>(parent)?
            .iter()
            .find(|&child| has(child))
    };
    let content =
        child_with(row, &|e| world.get::<TreeRowContent>(e).is_some()).expect("a row has content");
    let dot = child_with(content, &|e| world.get::<TreeRowDot>(e).is_some())
        .expect("a row draws a glyph");
    let glyph = world
        .get::<Children>(dot)
        .and_then(|children| children.iter().next())
        .expect("the glyph is a text child");
    world
        .get::<TextColor>(glyph)
        .expect("the glyph is coloured")
        .0
}

/// Open `source`'s branch and let the rows below it be built.
fn expand(app: &mut App, source: Entity, panel: Entity) {
    let row = row_for(app, source, panel).expect("the entity has a row to open");
    app.world_mut()
        .entity_mut(row)
        .insert(TreeNodeExpanded(true));
    for _ in 0..6 {
        app.update();
    }
}

#[test]
fn a_model_instance_is_one_top_level_row() {
    let (app, panel, instances) = panel_over_models(1);
    assert_eq!(
        top_level_sources(&app, panel),
        instances,
        "the instance is the whole of the model at the top level: what the \
         loader spawned under it belongs below it, not beside it"
    );
}

#[test]
fn every_instance_in_a_scene_is_one_row() {
    // The scene this was reported from holds 527 models and drew 1054 top-level
    // rows; a handful of instances exercises the same path.
    let (app, panel, instances) = panel_over_models(6);
    assert_eq!(top_level_sources(&app, panel).len(), instances.len());
}

#[test]
fn opening_an_instance_shows_what_the_asset_spawned() {
    let (mut app, panel, instances) = panel_over_models(1);
    let instance = instances[0];

    let internals: Vec<Entity> = app
        .world()
        .get::<Children>(instance)
        .expect("the loader spawned the asset's tree")
        .iter()
        .collect();
    assert!(!internals.is_empty());
    for &internal in &internals {
        assert!(
            row_for(&app, internal, panel).is_none(),
            "a closed instance costs no rows for its internals"
        );
    }

    expand(&mut app, instance, panel);
    for &internal in &internals {
        let row =
            row_for(&app, internal, panel).expect("opening the instance builds its internals");
        assert_eq!(
            glyph_color(&app, row),
            category_color(EntityCategory::AssetPart, false),
            "an internal is drawn in the asset-part tone, not as an authored entity"
        );
    }
}

#[test]
fn clicking_an_internal_selects_the_instance() {
    use jackdaw::selection::Selection;
    use jackdaw_widgets::tree_view::TreeRowClicked;

    let (mut app, panel, instances) = panel_over_models(1);
    let instance = instances[0];
    expand(&mut app, instance, panel);

    let internal = app
        .world()
        .get::<Children>(instance)
        .expect("the loader spawned the asset's tree")
        .iter()
        .next()
        .expect("at least one internal");
    let row = row_for(&app, internal, panel).expect("the internal has a row");
    let content = app
        .world()
        .get::<Children>(row)
        .and_then(|children| {
            children
                .iter()
                .find(|&child| app.world().get::<TreeRowContent>(child).is_some())
        })
        .expect("the row has content");

    app.world_mut().trigger(TreeRowClicked {
        entity: content,
        source_entity: internal,
    });
    app.update();

    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![instance],
        "an internal has no node in the document, so the click is aimed at \
         the instance holding it"
    );
}

#[test]
fn a_save_writes_the_instance_and_none_of_its_internals() {
    // Giving the internals rows makes them look authored, and the document is
    // where that would show: the file names the model, not the spawned tree.
    let (mut app, _panel, instances) = panel_over_models(1);
    let directory = std::env::temp_dir();
    let saved = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &directory);
    assert!(
        saved.contains("GltfSource"),
        "the instance is what the document holds:\n{saved}"
    );
    for internal in app
        .world()
        .get::<Children>(instances[0])
        .expect("the loader spawned the asset's tree")
        .iter()
    {
        let name = app
            .world()
            .get::<Name>(internal)
            .map(|name| name.as_str().to_string())
            .unwrap_or_default();
        assert!(
            name.is_empty() || !saved.contains(&format!("Name(\"{name}\")")),
            "the internal {name:?} was written into the document:\n{saved}"
        );
    }
}

#[test]
fn show_all_still_reaches_the_internals() {
    // "Show All" is the escape hatch from every outliner filter, so the internals
    // stay reachable through it.
    let (mut app, panel, instances) = panel_over_models(1);
    app.world_mut().insert_resource(HierarchyShowAll(true));
    for _ in 0..4 {
        app.update();
    }
    expand(&mut app, instances[0], panel);
    let internal = app
        .world()
        .get::<Children>(instances[0])
        .expect("the loader spawned the asset's tree")
        .iter()
        .next()
        .expect("at least one internal");
    assert!(row_for(&app, internal, panel).is_some());
}
