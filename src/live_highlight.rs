//! Mirrors the editor selection into the focused running game as a highlight
//! box. The game draws the box through its own cameras, so it appears in the
//! streamed frame. Works whether the entity was picked in the Game panel or
//! selected in the Live tree, since both routes update [`Selection`].

use bevy::prelude::*;

use crate::selection::Selection;

/// Last highlight bits sent to the focused game, to send only on change.
#[derive(Resource, Default)]
pub(crate) struct LastHighlight(pub(crate) Option<u64>);

/// Mirror the editor selection into the focused game as a highlight box.
/// Resolves the primary selected preview entity to its live game bits and
/// sends a `Highlight` control event when it differs from the last sent.
pub(crate) fn sync_selection_highlight(world: &mut World) {
    let desired = {
        let primary = world.resource::<Selection>().primary();
        match primary {
            Some(entity) => {
                let projection = world.resource::<crate::pie_projection::PieProjection>();
                crate::live_edits::live_bits_for_preview(projection, entity)
            }
            None => None,
        }
    };
    let last = world.get_resource::<LastHighlight>().and_then(|l| l.0);
    if desired == last {
        return;
    }
    // Record only what was actually sent. While no instance is focused the
    // memo stays stale, so a selection made before an instance focuses still
    // pushes its box once focus arrives rather than being silently swallowed.
    let focused = world
        .resource::<crate::pie_mirror::PieInstances>()
        .focused
        .is_some();
    if !focused {
        return;
    }
    crate::pie::send_control_to_focused(
        world,
        jackdaw_pie_protocol::ControlEvent::Highlight { entity: desired },
    );
    world.get_resource_or_insert_with(LastHighlight::default).0 = desired;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pie_mirror::PieInstances;
    use crate::pie_projection::PieProjection;

    fn focus_world() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<PieProjection>();
        world.init_resource::<LastHighlight>();
        world.init_resource::<PieInstances>();
        let preview = world.spawn_empty().id();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(7, preview);
        (world, preview)
    }

    fn focus_an_instance(world: &mut World) {
        world.resource_mut::<PieInstances>().focused = Some(crate::pie::InstanceKey {
            config: "game".to_string(),
            instance: 0,
        });
    }

    #[test]
    fn highlight_syncs_on_selection_change_while_focused() {
        let (mut world, preview) = focus_world();
        focus_an_instance(&mut world);
        // The send is a no-op (no PieSession), so only LastHighlight moves.
        world.resource_mut::<Selection>().entities.push(preview);

        sync_selection_highlight(&mut world);
        assert_eq!(
            world.resource::<LastHighlight>().0,
            Some(7),
            "selecting the mapped preview records its live bits"
        );

        world.resource_mut::<Selection>().entities.clear();
        sync_selection_highlight(&mut world);
        assert_eq!(
            world.resource::<LastHighlight>().0,
            None,
            "clearing the selection clears the highlight"
        );
    }

    #[test]
    fn highlight_waits_for_focus_before_recording() {
        let (mut world, preview) = focus_world();
        world.resource_mut::<Selection>().entities.push(preview);

        // No focused instance yet: the box cannot be drawn, so nothing is
        // recorded and the memo stays stale.
        sync_selection_highlight(&mut world);
        assert_eq!(
            world.resource::<LastHighlight>().0,
            None,
            "an unfocused session records nothing, so a later focus re-sends"
        );

        // Once an instance focuses, the standing selection pushes its box.
        focus_an_instance(&mut world);
        sync_selection_highlight(&mut world);
        assert_eq!(world.resource::<LastHighlight>().0, Some(7));
    }
}
