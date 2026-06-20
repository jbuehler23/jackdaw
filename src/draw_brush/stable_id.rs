use bevy::prelude::*;

/// Stable identifier that survives the snapshot round-trip (undo
/// respawns fresh entity ids; selection is restored by matching
/// on this).
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct BrushStableId(pub(crate) u64);

#[derive(Resource, Default)]
pub(crate) struct StableIdCounter(u64);

impl StableIdCounter {
    pub(crate) fn next(&mut self) -> BrushStableId {
        self.0 += 1;
        BrushStableId(self.0)
    }
}

/// Mint a fresh `BrushStableId` by advancing the global counter.
/// Used by paste to assign new IDs to pasted brush entities so they don't
/// collide with the originals they were copied from.
pub(crate) fn mint_stable_id(world: &mut World) -> BrushStableId {
    world.resource_mut::<StableIdCounter>().next()
}

/// Initialize the `StableIdCounter` resource on a world that doesn't have the
/// full `DrawBrushPlugin` loaded. Useful for headless integration tests.
#[cfg(test)]
pub(crate) fn init_stable_id_counter(world: &mut World) {
    world.init_resource::<StableIdCounter>();
}

/// Find the current Entity for a given stable ID, if it exists.
pub(crate) fn entity_by_stable_id(world: &mut World, id: BrushStableId) -> Option<Entity> {
    world
        .query::<(Entity, &BrushStableId)>()
        .iter(world)
        .find(|(_, sid)| **sid == id)
        .map(|(e, _)| e)
}

/// Lazily give every `Brush` a `BrushStableId` so the undo selection-restore
/// path (`apply_ast_to_world`) can match selections across scene reloads.
/// Brushes loaded from JSN carry the serialized id if they had one; fresh
/// draws that didn't insert one explicitly get one here.
pub(crate) fn assign_missing_brush_stable_ids(
    mut commands: Commands,
    mut counter: ResMut<StableIdCounter>,
    brushes: Query<Entity, (With<crate::brush::Brush>, Without<BrushStableId>)>,
) {
    for entity in &brushes {
        let sid = counter.next();
        if let Ok(mut ec) = commands.get_entity(entity) {
            ec.insert(sid);
        }
    }
}
