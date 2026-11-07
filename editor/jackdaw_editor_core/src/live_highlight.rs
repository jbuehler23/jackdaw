//! Mirrors the editor selection into the focused running game as a highlight
//! box. The game draws the box through its own cameras, so it appears in the
//! streamed frame.

use bevy::prelude::*;
use jackdaw_pie_protocol::HighlightPlace;
use jackdaw_prefab::{IsA, Prefab, PrefabEntityId};

use crate::pie::{InstanceKey, PieInstances};
use crate::scenes::TabKind;
use crate::selection::Selection;

/// The last highlight sent, to send only on change: the game it went to and the
/// places it named.
#[derive(Resource, Default)]
pub(crate) struct LastHighlight(Option<(InstanceKey, Vec<HighlightPlace>)>);

/// Mirror the editor selection into the focused game as a highlight box, sent as a
/// `Highlight` control event when the places or the focused game differ from the
/// last sent.
pub(crate) fn sync_selection_highlight(world: &mut World) {
    let Some(focused) = world.resource::<PieInstances>().focused.clone() else {
        world.get_resource_or_insert_with(LastHighlight::default).0 = None;
        return;
    };
    let places = highlight_places(world, &focused);
    let desired = Some((focused, places.clone()));
    if world
        .get_resource::<LastHighlight>()
        .is_some_and(|last| last.0 == desired)
    {
        return;
    }
    crate::pie::send_control_to_focused(
        world,
        jackdaw_pie_protocol::ControlEvent::Highlight { places },
    );
    world.get_resource_or_insert_with(LastHighlight::default).0 = desired;
}

/// Where the game `key` holds the primary selection, for the document in front: the
/// scenes it plays that document as, and the instances of it if it is a prefab.
#[doc(hidden)]
pub fn highlight_places(world: &World, key: &InstanceKey) -> Vec<HighlightPlace> {
    let Some(entity) = world.resource::<Selection>().primary() else {
        return Vec::new();
    };
    let mut levels = vec![entity];
    let mut at = entity;
    while let Some(child_of) = world.get::<ChildOf>(at) {
        at = child_of.parent();
        if world.get::<IsA>(at).is_some() {
            levels.push(at);
        }
    }
    levels.reverse();
    let top = levels[0];
    let Some(inner) = levels[1..]
        .iter()
        .map(|&level| world.get::<PrefabEntityId>(level).map(|id| id.0))
        .collect::<Option<Vec<u32>>>()
    else {
        return Vec::new();
    };

    let mut places = Vec::new();
    let node = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .and_then(|ast| ast.stable_id_of(ast.ast_for(top)?));
    if let Some(node) = node {
        for asked in crate::play_scene::scenes_playing_active_tab(world, key) {
            places.push(HighlightPlace::Node {
                scene: asked.requested,
                node,
                inner: inner.clone(),
            });
        }
    }
    if let Some(prefab) = front_prefab(world) {
        let inner = if world.get::<Prefab>(top).is_some() {
            Some(inner)
        } else {
            world.get::<PrefabEntityId>(top).map(|id| {
                let mut below = vec![id.0];
                below.extend(inner);
                below
            })
        };
        if let Some(inner) = inner {
            places.push(HighlightPlace::Instances { prefab, inner });
        }
    }
    places
}

/// The asset path of the prefab in the tab in front.
fn front_prefab(world: &World) -> Option<String> {
    let scenes = world.get_resource::<crate::scenes::Scenes>()?;
    if scenes.tabs.get(scenes.active)?.kind != TabKind::Prefab {
        return None;
    }
    crate::play_scene::tab_asset_path(world, scenes.active)
}

#[cfg(test)]
mod tests {
    use jackdaw_bsn::SceneBsnAst;
    use jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH;

    use super::*;

    const NODE: u64 = 7;

    fn game(instance: u32) -> InstanceKey {
        InstanceKey {
            config: "game".to_string(),
            instance,
        }
    }

    fn world_with_node() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<LastHighlight>();
        world.init_resource::<PieInstances>();
        let entity = world.spawn_empty().id();
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::TupleStruct(
            jackdaw_bsn::BsnTupleStructData {
                type_path: SCENE_NODE_ID_TYPE_PATH.to_string(),
                values: vec![jackdaw_bsn::BsnValue::Int(NODE as i128)],
            },
        )]);
        ast.add_to_roots(node);
        ast.link(entity, node);
        world.insert_resource(ast);
        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(std::path::PathBuf::from("/game/assets/zone.bsn"));
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });
        world.insert_resource(crate::project::ProjectRoot::new(
            std::path::PathBuf::from("/game"),
            crate::project::ProjectConfig::default(),
        ));
        world.init_resource::<crate::play_scene::PlayAssetNames>();
        for instance in [1, 2] {
            playing(&mut world, instance);
        }
        (world, entity)
    }

    fn sent(world: &World) -> Option<(InstanceKey, Vec<HighlightPlace>)> {
        world.resource::<LastHighlight>().0.clone()
    }

    fn at_the_node() -> Vec<HighlightPlace> {
        vec![HighlightPlace::Node {
            scene: "zone.bsn".to_string(),
            node: NODE,
            inner: Vec::new(),
        }]
    }

    /// The game `instance` has asked for the scene in front.
    fn playing(world: &mut World, instance: u32) {
        crate::play_scene::note_test_asked_scene(world, &game(instance), "zone.bsn");
    }

    #[test]
    fn the_highlight_names_the_selected_node_of_the_document() {
        let (mut world, entity) = world_with_node();
        world.resource_mut::<PieInstances>().focused = Some(game(1));
        world.resource_mut::<Selection>().entities.push(entity);

        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), at_the_node())));

        world.resource_mut::<Selection>().entities.clear();
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), Vec::new())));
    }

    #[test]
    fn a_selection_made_before_focus_or_kept_across_it_reaches_the_game() {
        let (mut world, entity) = world_with_node();
        world.resource_mut::<Selection>().entities.push(entity);

        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), None, "nothing goes with no focused game");

        world.resource_mut::<PieInstances>().focused = Some(game(1));
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), at_the_node())));

        world.resource_mut::<PieInstances>().focused = Some(game(2));
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(2), at_the_node())));
    }

    #[test]
    fn a_new_session_highlights_the_standing_selection_again() {
        let (mut world, entity) = world_with_node();
        world.resource_mut::<Selection>().entities.push(entity);
        world.resource_mut::<PieInstances>().focused = Some(game(1));
        sync_selection_highlight(&mut world);

        world.resource_mut::<PieInstances>().focused = None;
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), None);

        world.resource_mut::<PieInstances>().focused = Some(game(1));
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), at_the_node())));
    }
}
