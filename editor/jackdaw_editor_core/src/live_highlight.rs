//! Mirrors the editor selection into the focused running game as a highlight
//! box. The game draws the box through its own cameras, so it appears in the
//! streamed frame.

use bevy::prelude::*;

use crate::pie::{InstanceKey, PieInstances};
use crate::selection::Selection;

/// The last highlight sent, to send only on change: the game it went to and the
/// document node it named.
#[derive(Resource, Default)]
pub(crate) struct LastHighlight(Option<(InstanceKey, Option<u64>)>);

/// Mirror the editor selection into the focused game as a highlight box: the
/// document node of the primary selected entity, sent as a `Highlight` control
/// event when that or the focused game differs from the last sent.
pub(crate) fn sync_selection_highlight(world: &mut World) {
    let Some(focused) = world.resource::<PieInstances>().focused.clone() else {
        world.get_resource_or_insert_with(LastHighlight::default).0 = None;
        return;
    };
    let node = world
        .resource::<Selection>()
        .primary()
        .and_then(|entity| selected_node(world, entity));
    let desired = Some((focused, node));
    if world
        .get_resource::<LastHighlight>()
        .is_some_and(|last| last.0 == desired)
    {
        return;
    }
    crate::pie::send_control_to_focused(
        world,
        jackdaw_pie_protocol::ControlEvent::Highlight { node },
    );
    world.get_resource_or_insert_with(LastHighlight::default).0 = desired;
}

fn selected_node(world: &World, entity: Entity) -> Option<u64> {
    let ast = world.get_resource::<jackdaw_bsn::SceneBsnAst>()?;
    ast.stable_id_of(ast.ast_for(entity)?)
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
        (world, entity)
    }

    fn sent(world: &World) -> Option<(InstanceKey, Option<u64>)> {
        world.resource::<LastHighlight>().0.clone()
    }

    #[test]
    fn the_highlight_names_the_selected_node_of_the_document() {
        let (mut world, entity) = world_with_node();
        world.resource_mut::<PieInstances>().focused = Some(game(1));
        world.resource_mut::<Selection>().entities.push(entity);

        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), Some(NODE))));

        world.resource_mut::<Selection>().entities.clear();
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), None)));
    }

    #[test]
    fn a_selection_made_before_focus_or_kept_across_it_reaches_the_game() {
        let (mut world, entity) = world_with_node();
        world.resource_mut::<Selection>().entities.push(entity);

        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), None, "nothing goes with no focused game");

        world.resource_mut::<PieInstances>().focused = Some(game(1));
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(1), Some(NODE))));

        world.resource_mut::<PieInstances>().focused = Some(game(2));
        sync_selection_highlight(&mut world);
        assert_eq!(sent(&world), Some((game(2), Some(NODE))));
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
        assert_eq!(sent(&world), Some((game(1), Some(NODE))));
    }
}
